# Copyright 2026 AsterSQL.
"""Independent transaction observations through libpq; never owns the reference server."""
import argparse
import ctypes as c
import ctypes.util
import json
import os
import signal
import socket
import struct
import time
import threading
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('--case', choices=['active_cancel', 'commit', 'rollback', 'single_implicit', 'repeated_begin', 'outside_end', 'disconnect', 'failed_rollback', 'failed_commit', 'old_portal', 'savepoint_gate', 'query_block', 'extended_success', 'extended_error', 'explicit_sync', 'table_ddl_gate', 'other_ddl_gate', 'database_ddl_gate', 'mysql_ddl_coexist'], required=True)
p.add_argument('--host', choices=['127.0.0.1'], required=True)
p.add_argument('--port', type=int, required=True)
p.add_argument('--database', required=True)
p.add_argument('--storage', choices=['test', 'tikv'], default='test')
p.add_argument('--restart-check', action='store_true')
a = p.parse_args()
assert not a.restart_check or (a.storage == 'tikv' and a.case == 'commit')
def interrupted(signum, frame):
    raise RuntimeError('transaction client interrupted or timed out')
for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGALRM):
    signal.signal(sig, interrupted)
signal.alarm(90)
assert a.database.replace('_', '').isalnum(), 'invalid test database'
reference = os.environ.get('PG18_REFERENCE_DSN')
assert reference, 'PG18_REFERENCE_DSN is required (no skip)'
pq = c.CDLL(os.environ.get('PG_LIBPQ_LIBRARY') or ctypes.util.find_library('pq'))
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
P, I, S = c.c_void_p, c.c_int, c.c_char_p
connect = api('PQconnectdb', P, S)
status = api('PQstatus', I, P)
finish = api('PQfinish', None, P)
execq = api('PQexec', P, P, S)
prepare = api('PQprepare', P, P, S, S, I, P)
prepared = api('PQexecPrepared', P, P, S, I, P, P, P, I)
clear = api('PQclear', None, P)
rs = api('PQresultStatus', I, P)
nt = api('PQntuples', I, P)
nf = api('PQnfields', I, P)
val = api('PQgetvalue', S, P, I, I)
null = api('PQgetisnull', I, P, I, I)
field = api('PQfname', S, P, I)
typeoid = api('PQftype', c.c_uint, P, I)
size = api('PQfsize', I, P, I)
mod = api('PQfmod', I, P, I)
fmt = api('PQfformat', I, P, I)
tag = api('PQcmdStatus', S, P)
affected = api('PQcmdTuples', S, P)
diag = api('PQresultErrorField', S, P, I)
tx = api('PQtransactionStatus', I, P)
libversion = api('PQlibVersion', I)
pqsocket = api('PQsocket', I, P)
backendpid = api('PQbackendPID', I, P)
serverversion = api('PQserverVersion', I, P)
NoticeReceiver = c.CFUNCTYPE(None, P, P)
set_notice_receiver = api('PQsetNoticeReceiver', P, P, NoticeReceiver, P)
assert libversion() >= 180000, 'libpq18 required'
root = Path(__file__).resolve().parents[3]
evidence = root / 'target' / f'pg12-{a.case}-evidence' / a.database
evidence.mkdir(parents=True, exist_ok=True)
head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()

def text(v):
    return v.decode('utf8') if v is not None else None

class Connection:
    def __init__(self, dsn, name, observations):
        self.ptr = connect(dsn.encode())
        self.name, self.observations, self.sequence = name, observations, 0
        assert self.ptr and status(self.ptr) == 0, 'connection failed (DSN redacted)'
        self.pending_notices = []
        def receive_notice(arg, result):
            self.pending_notices.append({k:text(diag(result,ord(k))) for k in 'SVCMDHP'})
        self.notice_receiver = NoticeReceiver(receive_notice)
        set_notice_receiver(self.ptr, self.notice_receiver, None)
    def result(self, r, sql, phase):
        assert r, 'no libpq result'
        try:
            row = {'connection': self.name, 'sql': sql, 'phase': phase,
                   'status': rs(r), 'transaction': {0:'I', 1:'active', 2:'T', 3:'E', 4:'unknown'}[tx(self.ptr)],
                   'columns': [{'name':text(field(r,j)), 'oid':typeoid(r,j),
                                'typlen':size(r,j), 'typmod':mod(r,j), 'format':fmt(r,j),
                                'nullable':'unverified'} for j in range(nf(r))],
                   'rows': [[None if null(r,i,j) else text(val(r,i,j)) for j in range(nf(r))] for i in range(nt(r))],
                   'command':text(tag(r)), 'affected':text(affected(r)),
                   'diagnostics':{k:text(diag(r,ord(k))) for k in 'SVCMDHP'}}
            if a.case in ('repeated_begin', 'outside_end'):
                row['notices'] = self.pending_notices
            self.pending_notices = []
            self.observations.append(row)
            return row
        finally:
            clear(r)
    def query(self, sql, extended=False, required=True):
        if extended:
            self.sequence += 1
            name = f'commit_{self.sequence}'.encode()
            r = self.result(prepare(self.ptr,name,sql.encode(),0,None),sql,'Parse/Sync')
            if r['status'] != 1:
                if required:
                    assert r['status'] == 1, r
                return r
            r = self.result(prepared(self.ptr,name,0,None,None,None,0),sql,'Bind/Execute/Sync')
        else:
            r = self.result(execq(self.ptr,sql.encode()),sql,'Q')
        if required:
            assert r['status'] in (1,2), r
        return r
    def close(self):
        if self.ptr:
            finish(self.ptr)
            self.ptr = None

# libpq performs authentication; a duplicate of its idle, unencrypted socket
# then permits separate Bind/Execute/Flush messages (PQexecPrepared cannot keep
# a named, suspended portal). Never mix libpq reads with this wire session.
class PortalWire:
    def __init__(self, conn, observations):
        assert api('PQsslInUse', I, P)(conn.ptr) == 0, 'portal probe requires local non-TLS socket'
        self.sock = socket.socket(fileno=os.dup(pqsocket(conn.ptr)))
        self.sock.settimeout(10)
        self.observations = observations
        self.columns = {}
    @staticmethod
    def message(tag, body=b''):
        return tag + struct.pack('!I', len(body)+4) + body
    def read(self, n):
        data = b''
        while len(data) < n:
            chunk = self.sock.recv(n-len(data))
            assert chunk, 'portal socket closed'
            data += chunk
        return data
    def exchange(self, messages, sql, phase, portal=None, flush=False):
        self.sock.sendall(b''.join(self.message(t,b) for t,b in messages)
                          + (b'' if messages and messages[0][0] == b'Q' else self.message(b'H' if flush else b'S')))
        row = dict(connection='A', sql=sql, phase=phase, portal=portal,
                   transaction=None, columns=[], rows=[], command='', affected='',
                   diagnostics={k:None for k in 'SVCMDHP'}, tags=[])
        # Save partial observations too, so assertion failures retain evidence.
        self.observations.append(row)
        if a.case == 'query_block':
            row['commands'] = []
            row['wire_messages'] = []
        while True:
            t = self.read(1).decode()
            body = self.read(struct.unpack('!I', self.read(4))[0]-4)
            row['tags'].append(t)
            if a.case == 'query_block':
                row['wire_messages'].append(dict(tag=t, body_hex=body.hex()))
            if t == 'T':
                count, = struct.unpack('!H',body[:2])
                offset = 2
                for _ in range(count):
                    end = body.index(0, offset)
                    name = body[offset:end].decode()
                    _, _, oid, size, mod, fmt = struct.unpack('!IhIhih',body[end+1:end+19])
                    row['columns'].append(dict(name=name,oid=oid,typlen=size,typmod=mod,format=fmt,nullable='unverified'))
                    offset = end+19
                assert offset == len(body)
                self.columns[portal] = row['columns']
            elif t == 'D':
                count, = struct.unpack('!H',body[:2])
                offset, values = 2, []
                for _ in range(count):
                    size, = struct.unpack('!i',body[offset:offset+4])
                    offset += 4
                    values.append(None if size == -1 else body[offset:offset+size].decode())
                    if size != -1:
                        offset += size
                assert offset == len(body)
                row['rows'].append(values)
                row['columns'] = self.columns[portal]
            elif t == 'C':
                row['command'] = body[:-1].decode()
                if a.case == 'query_block':
                    row['commands'].append(row['command'])
                parts = row['command'].split()
                if parts[0] in ('SELECT','INSERT','UPDATE','DELETE'):
                    row['affected'] = parts[-1]
            elif t == 'E':
                fields = body[:-1].split(b'\0')
                row['diagnostics'] = {k:next((f[1:].decode() for f in fields if f[:1] == k.encode()),None) for k in 'SVCMDHP'}
            elif t == 'Z':
                row['transaction'] = body.decode()
                break
            if flush and t in ('s','C','E'):
                break
        return row
    def query(self, sql):
        return self.exchange([(b'Q',sql.encode()+b'\0')],sql,'Q')
    def bind(self, name, sql):
        n = name.encode()+b'\0'
        return self.exchange([(b'P',n+sql.encode()+b'\0'+b'\0\0'),
                              (b'B',n+n+b'\0'*6), (b'D',b'P'+n)],sql,'Parse/Bind/Describe/Sync',name)
    def execute(self, name, sql, limit=0, flush=False):
        return self.exchange([(b'E',name.encode()+b'\0'+struct.pack('!I',limit))],sql,
                             'Execute/Flush' if flush else 'Execute/Sync',name,flush)


def old_portals(A, B, check, side, record, extended, variant):
    observations = record.setdefault('portal_observations', [])
    wire = PortalWire(A, observations)
    try:
        assert wire.query('BEGIN')['transaction'] == 'T'
        statements = [('read', O), ('write','INSERT INTO public.tx_contract VALUES (4,40)'),
                      ('session','SHOW search_path'),
                      ('catalog','SELECT oid,nspname FROM pg_catalog.pg_namespace'),
                      ('page', O),
                      ('catalog_page', "SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname IN ('pg_catalog','public') ORDER BY nspname")]
        for name, sql in statements:
            r = wire.bind(name, sql)
            assert r['tags'][:2] == ['1','2'] and r['transaction'] == 'T', r
            assert not r['diagnostics']['C'], r
            if name in ('read','page'):
                assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('value',23,4,-1,0)], r
            if name == 'write':
                assert r['tags'][2] == 'n' and not r['columns'], r
        r = wire.bind('closed', O)
        assert not r['diagnostics']['C'] and r['transaction'] == 'T', r
        r = wire.exchange([(b'C', b'Pclosed\0')], O, 'Close/Sync', 'closed')
        assert r['tags'] == ['3','Z'] and r['transaction'] == 'T', r
        # Ordinary table reads may stream; catalog queries produce the
        # materialized Portal.messages/offset path. Suspend and retry both.
        for name, sql in [statements[4], statements[5]]:
            r = wire.execute(name, sql, 1, flush=True)
            expected = [['1','10']] if name == 'page' else [['pg_catalog']]
            assert r['tags'] == ['D','s'] and r['rows'] == expected, r
            r = wire.exchange([], '<sync after suspended portal>', 'Sync')
            assert r['transaction'] == 'T', r
        if variant == 'with_prefix':
            for sql, tag in [('UPDATE public.tx_contract SET value=11 WHERE id=1', 'UPDATE 1'),
                             ('INSERT INTO public.tx_contract VALUES (3,30)', 'INSERT 0 1')]:
                r = wire.query(sql)
                assert r['command'] == tag and r['transaction'] == 'T', r
                check(B, S0, 'I')
        failure = 'INSERT INTO public.tx_contract VALUES (1,99)'
        if extended:
            r = wire.bind('failure', failure)
            assert r['transaction'] == 'T' and not r['diagnostics']['C'], r
            r = wire.execute('failure', failure)
        else:
            r = wire.query(failure)
        assert r['diagnostics']['C'] == '23505' and r['transaction'] == 'E' and not r['rows'], r
        check(B, S0, 'I')
        for name, sql in statements:
            r = wire.execute(name, sql)
            # Observe the failure gate independently on each endpoint, including
            # a cached second page. A missing-portal diagnostic is a lifecycle
            # observation, never evidence of the 25P02 gate.
            expected = '25P02'
            assert r['diagnostics']['C'] == expected and r['transaction'] == 'E', r
            assert r['tags'] == ['E','Z'] and not r['rows'] and not r['command'], r
            r['lifecycle'] = 'bound before error; failed-state gate observed'
            check(B, S0, 'I')
        r = wire.execute('closed', O)
        expected = '34000'
        assert r['diagnostics']['C'] == expected and r['transaction'] == 'E' and r['tags'] == ['E','Z'], r
        r['lifecycle'] = 'explicitly closed; missing portal (not gate coverage)'
        check(B, S0, 'I')
        r = wire.query('ROLLBACK')
        assert r['command'] == 'ROLLBACK' and r['transaction'] == 'I' and not r['columns'], r
        assert wire.query('BEGIN')['transaction'] == 'T'
        r = wire.bind('recovered', O)
        assert r['transaction'] == 'T' and not r['diagnostics']['C'], r
        r = wire.execute('recovered', O)
        assert r['rows'] == S0 and r['command'] == 'SELECT 2' and r['transaction'] == 'T', r
        assert wire.query('ROLLBACK')['transaction'] == 'I'
        check(B, S0, 'I')
        r = wire.query('SELECT 1::int4')
        assert r['rows'] == [['1']] and r['transaction'] == 'I', r
        r = wire.query('DROP TABLE public.tx_contract')
        assert r['command'] == 'DROP TABLE' and r['transaction'] == 'I', r
    finally:
        try:
            wire.query('ROLLBACK')
        finally:
            wire.sock.close()
            A.close()

O = 'SELECT id,value FROM public.tx_contract ORDER BY id'
S0 = [['1','10'],['2','20']]
outputs = {}
def table_ddl_gate(A, B, side, record, extended, operation):
    # Both providers read live TableInfo metadata. The original unsupported
    # information_schema query remains a recorded limitation, never zero rows.
    def schema_version():
        request = Path(os.environ['PG12_DDL_SNAPSHOT'])
        reply = request.with_suffix('.reply')
        if reply.exists():
            reply.unlink()
        request.touch()
        deadline = time.monotonic() + 5
        while request.exists() or not reply.exists():
            assert time.monotonic() < deadline, 'canonical schema observation timed out'
            time.sleep(.005)
        return int(reply.read_text())
    sentinel = 'SELECT id,v FROM public.txddl_sentinel ORDER BY id'
    directory = "SELECT n.nspname,c.relname,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='txddl_probe'"
    columns = "SELECT attname,atttypid,attnotnull FROM pg_catalog.pg_attribute WHERE attrelid='public.txddl_sentinel'::regclass::oid AND attnum>0 AND NOT attisdropped ORDER BY attnum"
    constraints = "SELECT contype,conkey FROM pg_catalog.pg_constraint WHERE conrelid='public.txddl_sentinel'::regclass::oid AND contype='p' ORDER BY conkey"
    ddls = dict(create='CREATE TABLE public.txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)',
                drop='DROP TABLE public.txddl_sentinel',
                alter='ALTER TABLE public.txddl_sentinel ADD COLUMN extra integer',
                truncate='TRUNCATE TABLE public.txddl_sentinel')
    for exit_sql in ('ROLLBACK', 'COMMIT'):
        A.query('CREATE TABLE public.txddl_sentinel (id integer PRIMARY KEY,v integer NOT NULL)')
        A.query('INSERT INTO public.txddl_sentinel VALUES (1,10)')
        baseline = {}
        for sql in (sentinel, directory, columns, constraints):
            r = B.query(sql)
            baseline[sql] = r['rows']
        assert baseline[sentinel] == [['1','10']] and baseline[directory] == [], baseline
        assert baseline[columns] == [['id','23','t'],['v','23','t']], baseline
        assert baseline[constraints] == [['p','{1}']], baseline
        wire = PortalWire(A, record.setdefault('ddl_wire', []))
        def query(sql):
            if not extended:
                return wire.query(sql)
            n = f'ddl_gate_{len(record["ddl_wire"])}'.encode() + b'\0'
            return wire.exchange([(b'P', n + sql.encode() + b'\0\0\0'),
                                  (b'B', n + n + b'\0'*6),
                                  (b'E', n + struct.pack('!I', 0))], sql, 'Parse/Bind/Execute/Sync')
        def control(sql, tag, state):
            r = query(sql)
            assert r['command'] == tag and r['transaction'] == state, r
            assert not r['columns'] and not r['rows'] and not r['diagnostics']['C'], r
            return r
        def unchanged(conn):
            for sql, expected in baseline.items():
                r = conn.query(sql)
                assert r['rows'] == expected and r['transaction'] == 'I', r
            r = conn.query('SELECT id,v FROM public.txddl_probe ORDER BY id', required=False)
            assert r['status'] == 7 and r['diagnostics']['C'] == '42P01', r
        try:
            control('BEGIN', 'BEGIN', 'T')
            r = control('UPDATE public.txddl_sentinel SET v=11 WHERE id=1', 'UPDATE 1', 'T')
            assert r['affected'] == '1', r
            r = wire.query(sentinel)
            assert r['rows'] == [['1','11']] and r['transaction'] == 'T', r
            assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('v',23,4,-1,0)], r
            unchanged(B)
            before_version = schema_version() if side == 'aster' else None
            r = query(ddls[operation])
            boundary = dict(mode='extended' if extended else 'simple', operation=operation,
                            exit=exit_sql, ddl=r)
            record.setdefault('ddl_boundaries', []).append(boundary)
            if side == 'aster':
                assert r['diagnostics']['C'] == '0A000' and r['transaction'] == 'E', r
                assert not r['command'] and not r['rows'] and not r['columns'], r
                assert r['tags'] == ['E','Z'], r  # rejection before Bind/Execute
                after_version = schema_version()
                boundary['schema_versions'] = [before_version, after_version]
                assert after_version == before_version, boundary
                unchanged(B)
                r = query('SELECT 1::int4')
                assert r['diagnostics']['C'] == '25P02' and r['transaction'] == 'E', r
                control(exit_sql, 'ROLLBACK', 'I')
            else:
                assert r['command'] == dict(create='CREATE TABLE', drop='DROP TABLE', alter='ALTER TABLE', truncate='TRUNCATE TABLE')[operation], r
                assert r['transaction'] == 'T' and not r['columns'] and not r['rows'], r
                # Observe catalog MVCC without attempting a read behind the
                # reference's ACCESS EXCLUSIVE lock on the sentinel.
                assert B.query(directory)['rows'] == [], boundary
                if operation == 'create':
                    assert wire.query(directory)['rows'] == [['public','txddl_probe','r']]
                elif operation == 'alter':
                    assert wire.query(columns)['rows'][-1] == ['extra','23','f']
                elif operation == 'truncate':
                    assert wire.query(sentinel)['rows'] == []
                else:
                    r = wire.query("SELECT relname FROM pg_catalog.pg_class WHERE relname='txddl_sentinel'")
                    assert r['rows'] == [] and r['transaction'] == 'T', r
                if exit_sql == 'COMMIT':
                    # Make this a failed COMMIT recovery round on both ends;
                    # a successful PG DDL must also disappear after the error.
                    r = query('SELECT 1/0')
                    assert r['diagnostics']['C'] == '22012' and r['transaction'] == 'E', r
                control(exit_sql, 'ROLLBACK', 'I')
            unchanged(B)
            boundary['recovered'] = {}
            for sql, expected in baseline.items():
                r = wire.query(sql)
                assert r['rows'] == expected and r['transaction'] == 'I', r
                boundary['recovered'][sql] = r['rows']
            r = wire.query('SELECT 1::int4')
            assert r['rows'] == [['1']] and r['transaction'] == 'I', r
            assert wire.query('SELECT extra FROM public.txddl_sentinel')['diagnostics']['C'] == '42703'
            assert wire.query('SELECT id,v FROM public.txddl_probe ORDER BY id')['diagnostics']['C'] == '42P01'
        finally:
            try:
                wire.exchange([], '<cleanup sync>', 'cleanup/Sync')
                wire.query('ROLLBACK')
            finally:
                wire.sock.close()
        A.query('DROP TABLE public.txddl_sentinel')
        record['cleanup'].append(dict(operation=operation, exit=exit_sql, mode='extended' if extended else 'simple', table_dropped=True))
    A.close()
    B.close()

def database_ddl_gate(A, B, admin, side, record, extended, operation):
    # Both target names belong exclusively to this invocation; neither is the
    # connected fixture database. No connection is opened to the empty target.
    existing = a.database + '_empty'
    fresh = a.database + '_new'
    directory = f"SELECT datname FROM pg_catalog.pg_database WHERE datname IN ('{existing}','{fresh}') ORDER BY datname"
    sentinel = 'SELECT id,v FROM public.txddl_sentinel ORDER BY id'
    admin.query(f'CREATE DATABASE {existing}')
    try:
        A.query('CREATE TABLE public.txddl_sentinel (id integer PRIMARY KEY,v integer NOT NULL)')
        A.query('INSERT INTO public.txddl_sentinel VALUES (1,10)')
        wire = PortalWire(A, record.setdefault('database_ddl_wire', []))
        def query(sql):
            if not extended:
                return wire.query(sql)
            n = f'database_ddl_{len(record["database_ddl_wire"])}'.encode() + b'\0'
            return wire.exchange([(b'P', n + sql.encode() + b'\0\0\0'),
                                  (b'B', n + n + b'\0'*6),
                                  (b'E', n + struct.pack('!I', 0))], sql, 'Parse/Bind/Execute/Sync')
        def unchanged(conn):
            r = conn.query(sentinel)
            assert r['rows'] == [['1','10']] and r['transaction'] == 'I', r
            assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('v',23,4,-1,0)], r
            r = conn.query(directory)
            assert r['rows'] == [[existing]] and r['transaction'] == 'I', r
        try:
            unchanged(B)
            r = query('BEGIN')
            assert r['command'] == 'BEGIN' and r['transaction'] == 'T' and not r['columns'], r
            r = query('UPDATE public.txddl_sentinel SET v=11 WHERE id=1')
            assert r['command'] == 'UPDATE 1' and r['affected'] == '1' and r['transaction'] == 'T', r
            assert not r['columns'] and not r['rows'], r
            assert wire.query(sentinel)['rows'] == [['1','11']]
            unchanged(B)
            sql = f'CREATE DATABASE {fresh}' if operation == 'create' else f'DROP DATABASE {existing}'
            r = query(sql)
            boundary = dict(mode='extended' if extended else 'simple', operation=operation, ddl=r)
            record.setdefault('database_ddl_boundaries', []).append(boundary)
            assert r['diagnostics']['C'] == ('25001' if side == 'reference' else '0A000'), r
            assert r['transaction'] == 'E' and not r['command'] and not r['rows'] and not r['columns'], r
            assert r['tags'] == (['1','2','E','Z'] if extended and side == 'reference' else ['E','Z']), r
            boundary['protocol_stage'] = 'Execute' if extended and side == 'reference' else 'Parse' if extended else 'Q'
            unchanged(B)
            r = query('SELECT 1::int4')
            assert r['diagnostics']['C'] == '25P02' and r['transaction'] == 'E', r
            r = query('ROLLBACK')
            assert r['command'] == 'ROLLBACK' and r['transaction'] == 'I' and not r['columns'] and not r['rows'], r
            unchanged(B)
            unchanged(A)
            boundary['recovered'] = dict(sentinel=A.query(sentinel)['rows'], directory=A.query(directory)['rows'])
            r = wire.query('SELECT 1::int4')
            assert r['rows'] == [['1']] and r['transaction'] == 'I', r
        finally:
            try:
                wire.exchange([], '<cleanup sync>', 'cleanup/Sync')
                wire.query('ROLLBACK')
            finally:
                wire.sock.close()
        A.query('DROP TABLE public.txddl_sentinel')
    finally:
        # Management connection never enters the target databases; drop only
        # this invocation's unique names, including a wrongly created target.
        A.query('ROLLBACK', required=False)
        admin.query(f'DROP DATABASE IF EXISTS {fresh}')
        admin.query(f'DROP DATABASE {existing}')
        assert admin.query(directory)['rows'] == []
    A.close()
    B.close()
    record['cleanup'].append(dict(operation=operation, mode='extended' if extended else 'simple', databases_removed=True, connections_closed=True))

def other_ddl_gate(A, B, side, record, extended, operation):
    def schema_version():
        request = Path(os.environ['PG12_DDL_SNAPSHOT'])
        reply = request.with_suffix('.reply')
        if reply.exists():
            reply.unlink()
        request.touch()
        deadline = time.monotonic() + 5
        while request.exists() or not reply.exists():
            assert time.monotonic() < deadline, 'canonical schema observation timed out'
            time.sleep(.005)
        return int(reply.read_text())
    sentinel = 'SELECT id,v FROM public.txddl_sentinel ORDER BY id'
    directory = "SELECT n.nspname,c.relname,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='txddl_probe'"
    ddls = dict(create_view='CREATE VIEW public.txddl_probe AS SELECT id,v FROM public.txddl_sentinel',
                drop_view='DROP VIEW public.txddl_probe',
                create_index='CREATE INDEX txddl_probe ON public.txddl_sentinel (v)',
                drop_index='DROP INDEX public.txddl_probe',
                create_sequence='CREATE SEQUENCE public.txddl_probe',
                alter_sequence='ALTER SEQUENCE public.txddl_probe INCREMENT BY 2',
                drop_sequence='DROP SEQUENCE public.txddl_probe')
    A.query('CREATE TABLE public.txddl_sentinel (id integer PRIMARY KEY,v integer NOT NULL)')
    A.query('INSERT INTO public.txddl_sentinel VALUES (1,10)')
    setup = None
    if operation in ('drop_view', 'drop_index', 'alter_sequence', 'drop_sequence'):
        source = 'create_' + operation.split('_')[1]
        setup = A.query(ddls[source], required=False)
    baseline = B.query(directory)['rows']
    metadata = {}
    for sql in ("SELECT attname,atttypid,attnotnull FROM pg_catalog.pg_attribute WHERE attrelid='public.txddl_sentinel'::regclass::oid AND attnum>0 AND NOT attisdropped ORDER BY attnum",
                "SELECT contype,conkey FROM pg_catalog.pg_constraint WHERE conrelid='public.txddl_sentinel'::regclass::oid AND contype='p' ORDER BY conkey"):
        metadata[sql] = B.query(sql)['rows']
    assert list(metadata.values()) == [[['id','23','t'],['v','23','t']], [['p','{1}']]], metadata
    wire = PortalWire(A, record.setdefault('other_ddl_wire', []))
    def query(sql):
        if not extended:
            return wire.query(sql)
        n = f'other_ddl_{len(record["other_ddl_wire"])}'.encode() + b'\0'
        return wire.exchange([(b'P', n + sql.encode() + b'\0\0\0'),
                              (b'B', n + n + b'\0'*6),
                              (b'E', n + struct.pack('!I', 0))], sql, 'Parse/Bind/Execute/Sync')
    def unchanged(conn):
        r = conn.query(sentinel)
        assert r['rows'] == [['1','10']] and r['transaction'] == 'I', r
        assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('v',23,4,-1,0)], r
        assert conn.query(directory)['rows'] == baseline
        for sql, expected in metadata.items():
            assert conn.query(sql)['rows'] == expected
    try:
        assert query('BEGIN')['transaction'] == 'T'
        r = query('UPDATE public.txddl_sentinel SET v=11 WHERE id=1')
        assert r['command'] == 'UPDATE 1' and r['affected'] == '1' and r['transaction'] == 'T', r
        assert wire.query(sentinel)['rows'] == [['1','11']]
        unchanged(B)
        before_version = schema_version() if side == 'aster' else None
        r = query(ddls[operation])
        boundary = dict(mode='extended' if extended else 'simple', operation=operation, setup=setup, baseline=baseline, ddl=r)
        record.setdefault('other_ddl_boundaries', []).append(boundary)
        if side == 'aster':
            expected_code = '42P01' if operation in ('create_view','drop_view') else '42601' if operation == 'drop_index' else '0A000'
            assert r['diagnostics']['C'] == expected_code and r['transaction'] == 'E', r
            if operation in ('create_index','create_sequence','drop_sequence'):
                assert r['diagnostics']['M'] == 'DDL inside a PostgreSQL transaction is unsupported', r
            assert not r['command'] and not r['rows'] and not r['columns'], r
            assert r['tags'] == ['E','Z'], r
            after_version = schema_version()
            boundary['schema_versions'] = [before_version, after_version]
            assert before_version == after_version, boundary
            boundary['rejection_stage'] = dict(create_view='relation adaptation', drop_view='relation adaptation',
                drop_index='parser', alter_sequence='command adaptation').get(operation, 'transaction gate')
            boundary['protocol_stage'] = 'Parse' if extended else 'Q'
            unchanged(B)
            r = query('SELECT 1::int4')
            assert r['diagnostics']['C'] == '25P02' and r['transaction'] == 'E', r
        else:
            assert not r['diagnostics']['C'] and r['transaction'] == 'T' and r['command'], r
            assert B.query(directory)['rows'] == baseline
        r = query('ROLLBACK')
        assert r['command'] == 'ROLLBACK' and r['transaction'] == 'I', r
        unchanged(B)
        unchanged(A)
        boundary['recovered'] = dict(sentinel=A.query(sentinel)['rows'], directory=A.query(directory)['rows'], metadata=metadata)
        r = wire.query('SELECT 1::int4')
        assert r['rows'] == [['1']] and r['transaction'] == 'I', r
    finally:
        try:
            wire.exchange([], '<cleanup sync>', 'cleanup/Sync')
            wire.query('ROLLBACK')
        finally:
            wire.sock.close()
    if baseline:
        kind = baseline[0][2]
        if kind in ('v', 'S'):
            A.query(('DROP VIEW' if kind == 'v' else 'DROP SEQUENCE') + ' public.txddl_probe')
        else:
            assert kind == 'i', baseline
            # Dropping the owning table removes its index even on the product
            # whose PostgreSQL DROP INDEX syntax is rejected by the parser.
    A.query('DROP TABLE public.txddl_sentinel')
    assert B.query(directory)['rows'] == []
    record['cleanup'].append(dict(operation=operation, mode='extended' if extended else 'simple', objects_dropped=True))
    A.close()
    B.close()

def savepoint_gate(A, B, check, side, record, extended, variant):
    initial = [['1','10'],['2','20']]
    updated = [['1','11'],['2','20']]
    def command(sql, tag, state, affected=''):
        r = A.query(sql, extended)
        assert (r['command'], r['transaction'], r['affected']) == (tag,state,affected), r
        assert not r['columns'] and not r['rows'], r
        return r
    command('BEGIN', 'BEGIN', 'T')
    command('UPDATE public.tx_contract SET value=11 WHERE id=1', 'UPDATE 1', 'T', '1')
    check(A, updated, 'T')
    check(B, initial, 'I')
    r = A.query('SAVEPOINT s', extended, required=False)
    if r['status'] == 7:
        assert side == 'aster' and r['diagnostics']['C'] in ('0A000','42601'), r
        assert r['transaction'] == 'E' and not r['rows'] and not r['command'], r
        record.setdefault('savepoint_boundaries', []).append(dict(
            mode='extended' if extended else 'simple', variant=variant,
            capability='rejected', rejection=r))
        check(B, initial, 'I')
        # A refused savepoint must not make ROLLBACK TO a successful full
        # rollback, even when sent directly in the failed transaction.
        sql = 'ROLLBACK TO SAVEPOINT s' if variant == '30' else 'ROLLBACK TO s'
        r = A.query(sql, extended, required=False)
        assert r['status'] == 7 and r['diagnostics']['C'] == '0A000' and r['transaction'] == 'E', r
        assert not r['command'] and not r['rows'] and not r['columns'], r
        check(B, initial, 'I')
        command('ROLLBACK', 'ROLLBACK', 'I')
    else:
        assert r['command'] == 'SAVEPOINT' and r['transaction'] == 'T' and not r['columns'], r
        command('INSERT INTO public.tx_contract VALUES (3,30)', 'INSERT 0 1', 'T', '1')
        r = A.query('INSERT INTO public.tx_contract VALUES (1,99)', extended, required=False)
        assert r['status'] == 7 and r['diagnostics']['C'] == '23505' and r['transaction'] == 'E', r
        check(B, initial, 'I')
        sql = 'ROLLBACK TO SAVEPOINT s' if variant == '30' else 'ROLLBACK TO s'
        command(sql, 'ROLLBACK', 'T')
        check(A, updated, 'T')
        check(B, initial, 'I')
        command('COMMIT', 'COMMIT', 'I')
        check(A, updated, 'I')
        check(B, updated, 'I')
        record.setdefault('savepoint_boundaries', []).append(dict(
            mode='extended' if extended else 'simple', variant=variant,
            capability='local recovery verified'))
        # Restore the fixture only after verifying committed visibility.
        A.query('UPDATE public.tx_contract SET value=10 WHERE id=1')
    for conn in (A,B):
        check(conn, initial, 'I')
        r = conn.query('SELECT 1::int4', extended)
        assert r['rows'] == [['1']] and r['transaction'] == 'I', r
    A.query('DROP TABLE public.tx_contract')
    A.close()
    B.close()
    record['cleanup'].append(dict(table_dropped=True, connections_closed=True))

def extended_success(A, B, check, record, explicit, inserted):
    wire = PortalWire(A, record.setdefault('extended_segments', []))
    expected = [['1','11'],['2','20'],['3',None if inserted == 'NULL' else '30']]
    try:
        r = wire.exchange([], '<empty sync>', 'Sync')
        assert r['tags'] == ['Z'] and r['transaction'] == 'I', r
        check(B, S0, 'I')
        if explicit:
            r = wire.query('BEGIN')
            assert r['command'] == 'BEGIN' and r['transaction'] == 'T', r
        for name, sql, tag in [
                ('update', 'UPDATE public.tx_contract SET value=11 WHERE id=1', 'UPDATE 1'),
                ('insert', f'INSERT INTO public.tx_contract VALUES (3,{inserted})', 'INSERT 0 1')]:
            n = name.encode() + b'\0'
            r = wire.exchange([(b'P', n + sql.encode() + b'\0\0\0'),
                               (b'B', n + n + b'\0'*6),
                               (b'E', n + struct.pack('!I', 0))],
                              sql, 'Parse/Bind/Execute/Flush', name, flush=True)
            assert r['tags'] == ['1','2','C'] and r['command'] == tag, r
            assert r['affected'] == '1' and r['transaction'] is None, r
            assert not r['columns'] and not r['rows'] and not r['diagnostics']['C'], r
            check(B, S0, 'I')
        state = 'T' if explicit else 'I'
        for phase in ('Sync', 'empty Sync after segment'):
            r = wire.exchange([], '<sync>', phase)
            assert r['tags'] == ['Z'] and r['transaction'] == state, r
            check(B, S0 if explicit else expected, 'I')
        r = wire.query(O)
        assert r['rows'] == expected and r['transaction'] == state, r
        assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('value',23,4,-1,0)], r
        if explicit:
            r = wire.query('COMMIT')
            assert r['command'] == 'COMMIT' and r['transaction'] == 'I', r
        check(B, expected, 'I')
        r = wire.query('SELECT 1::int4 AS ok')
        assert r['rows'] == [['1']] and r['transaction'] == 'I', r
        r = wire.query('DROP TABLE public.tx_contract')
        assert r['command'] == 'DROP TABLE' and r['transaction'] == 'I', r
    finally:
        wire.sock.close()
    A.close()
    B.close()
    record['cleanup'].append(dict(explicit=explicit, insert=inserted, table_dropped=True, connections_closed=True))

def extended_error(A, B, check, record, stage):
    wire = PortalWire(A, record.setdefault('error_segments', []))
    def pbx(name, sql):
        n = name.encode() + b'\0'
        return [(b'P', n + sql.encode() + b'\0\0\0'),
                (b'B', n + n + b'\0'*6),
                (b'E', n + struct.pack('!I', 0))]
    try:
        sql = 'UPDATE public.tx_contract SET value=11 WHERE id=1'
        r = wire.exchange(pbx('prefix', sql), sql, 'prefix/Flush', flush=True)
        assert r['tags'] == ['1','2','C'] and r['command'] == 'UPDATE 1', r
        assert r['transaction'] is None and r['affected'] == '1' and not r['columns'] and not r['rows'], r
        check(B, S0, 'I')
        if stage == 'Parse':
            sql = 'SELECT FROM'
            messages = [(b'P', b'bad\0' + sql.encode() + b'\0\0\0')]
            code, tags = '42601', ['E']
        elif stage == 'Bind':
            sql = 'SELECT id FROM public.tx_contract WHERE id=$1'
            messages = [(b'P', b'bad\0' + sql.encode() + b'\0' + struct.pack('!hI', 1, 23)),
                        (b'B', b'bad\0bad\0' + struct.pack('!hhI', 0, 1, 3) + b'bad' + struct.pack('!h', 0))]
            code, tags = '22P02', ['1','E']
        else:
            sql = 'INSERT INTO public.tx_contract VALUES (1,99)'
            messages = pbx('bad', sql)
            code, tags = '23505', ['1','2','E']
        r = wire.exchange(messages, sql, stage + '/Flush', flush=True)
        assert r['tags'] == tags and r['diagnostics']['C'] == code, r
        assert r['transaction'] is None and not r['rows'] and not r['command'], r
        check(B, S0, 'I')
        # A single Sync response proves the intervening messages were discarded;
        # no timeout is used as evidence of suppression (including ROLLBACK).
        discarded = pbx('discard', 'INSERT INTO public.tx_contract VALUES (3,30)')
        discarded += [(b'Q', b'INSERT INTO public.tx_contract VALUES (4,40)\0'),
                      (b'Q', b'ROLLBACK\0'), (b'H', b'')]
        r = wire.exchange(discarded, 'PBX INSERT (3,30); Q INSERT (4,40); Q ROLLBACK; Flush; Sync', stage + '/discard/Sync')
        assert r['tags'] == ['Z'] and r['transaction'] == 'I', r
        assert not r['rows'] and not r['columns'] and not r['command'], r
        check(B, S0, 'I')
        r = wire.query(O)
        assert r['rows'] == S0 and r['transaction'] == 'I', r
        r = wire.query('SELECT 1::int4')
        assert r['rows'] == [['1']] and r['transaction'] == 'I', r
        sql = 'INSERT INTO public.tx_contract VALUES (3,30)'
        r = wire.exchange(pbx('recovered', sql), sql, stage + '/new segment/Sync')
        assert r['tags'] == ['1','2','C','Z'] and r['command'] == 'INSERT 0 1' and r['transaction'] == 'I', r
        check(B, (S0 + [['3','30']]), 'I')
        r = wire.query(O)
        assert r['rows'] == (S0 + [['3','30']]) and r['transaction'] == 'I', r
        assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('value',23,4,-1,0)], r
        wire.query('DROP TABLE public.tx_contract')
    finally:
        # Restore libpq's idle socket before outer failure cleanup too.
        try:
            wire.exchange([], '<cleanup sync>', 'cleanup/Sync')
            wire.query('ROLLBACK')
        finally:
            wire.sock.close()
    A.close()
    B.close()
    record['cleanup'].append(dict(stage=stage, table_dropped=True, connections_closed=True))

def explicit_sync(A, B, check, record, stage):
    wire = PortalWire(A, record.setdefault('explicit_segments', []))
    def pbx(name, sql):
        n = name.encode() + b'\0'
        return [(b'P', n + sql.encode() + b'\0\0\0'),
                (b'B', n + n + b'\0'*6),
                (b'E', n + struct.pack('!I', 0))]
    try:
        r = wire.query('BEGIN')
        assert r['command'] == 'BEGIN' and r['transaction'] == 'T', r
        assert not r['columns'] and not r['rows'], r
        sql = 'UPDATE public.tx_contract SET value=11 WHERE id=1'
        r = wire.exchange(pbx('prefix', sql), sql, 'prefix/Flush', flush=True)
        assert r['tags'] == ['1','2','C'] and r['command'] == 'UPDATE 1', r
        assert r['transaction'] is None and r['affected'] == '1' and not r['columns'] and not r['rows'], r
        check(B, S0, 'I')
        if stage == 'success':
            r = wire.exchange([], '<successful explicit Sync>', 'success/Sync')
            assert r['tags'] == ['Z'] and r['transaction'] == 'T', r
            check(B, S0, 'I')
            r = wire.query(O)
            assert r['rows'] == [['1','11'],['2','20']] and r['transaction'] == 'T', r
            r = wire.query('ROLLBACK')
            assert r['command'] == 'ROLLBACK' and r['transaction'] == 'I', r
            check(B, S0, 'I')
        elif stage == 'Parse':
            sql = 'SELECT FROM'
            messages = [(b'P', b'bad\0' + sql.encode() + b'\0\0\0')]
            code, tags = '42601', ['E']
        elif stage == 'Bind':
            sql = 'SELECT id FROM public.tx_contract WHERE id=$1'
            messages = [(b'P', b'bad\0' + sql.encode() + b'\0' + struct.pack('!hI', 1, 23)),
                        (b'B', b'bad\0bad\0' + struct.pack('!hhI', 0, 1, 3) + b'bad' + struct.pack('!h', 0))]
            code, tags = '22P02', ['1','E']
        else:
            sql = 'INSERT INTO public.tx_contract VALUES (1,99)'
            messages = pbx('bad', sql)
            code, tags = '23505', ['1','2','E']
        if stage != 'success':
            r = wire.exchange(messages, sql, stage + '/Flush', flush=True)
            assert r['tags'] == tags and r['diagnostics']['C'] == code, r
            assert r['transaction'] is None and not r['rows'] and not r['command'], r
            check(B, S0, 'I')
            # A single Sync response proves the intervening messages were discarded;
            # no timeout is used as evidence of suppression (including ROLLBACK).
            discarded = pbx('discard', 'INSERT INTO public.tx_contract VALUES (3,30)')
            discarded += [(b'Q', b'INSERT INTO public.tx_contract VALUES (4,40)\0'),
                          (b'Q', b'ROLLBACK\0'), (b'H', b'')]
            r = wire.exchange(discarded, 'PBX INSERT (3,30); Q INSERT (4,40); Q ROLLBACK; Flush; Sync', stage + '/discard/Sync')
            assert r['tags'] == ['Z'] and r['transaction'] == 'E', r
            assert not r['rows'] and not r['columns'] and not r['command'], r
            check(B, S0, 'I')
            for sql in (O, 'INSERT INTO public.tx_contract VALUES (4,40)'):
                r = wire.query(sql)
                assert r['tags'] == ['E','Z'] and r['diagnostics']['C'] == '25P02' and r['transaction'] == 'E', r
                assert not r['rows'] and not r['command'], r
                check(B, S0, 'I')
            sql = 'INSERT INTO public.tx_contract VALUES (4,40)'
            r = wire.exchange(pbx('failed_write', sql), sql, stage + '/failed write/Sync')
            assert r['tags'] == ['E','Z'] and r['diagnostics']['C'] == '25P02' and r['transaction'] == 'E', r
            check(B, S0, 'I')
            r = wire.query('ROLLBACK')
            assert r['command'] == 'ROLLBACK' and r['transaction'] == 'I', r
            assert not r['columns'] and not r['rows'], r
            check(B, S0, 'I')
        r = wire.query(O)
        assert r['rows'] == S0 and r['transaction'] == 'I', r
        r = wire.query('SELECT 1::int4')
        assert r['rows'] == [['1']] and r['transaction'] == 'I', r
        sql = 'INSERT INTO public.tx_contract VALUES (3,30)'
        r = wire.exchange(pbx('recovered', sql), sql, stage + '/new segment/Sync')
        assert r['tags'] == ['1','2','C','Z'] and r['command'] == 'INSERT 0 1' and r['transaction'] == 'I', r
        check(B, (S0 + [['3','30']]), 'I')
        r = wire.query(O)
        assert r['rows'] == (S0 + [['3','30']]) and r['transaction'] == 'I', r
        assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('value',23,4,-1,0)], r
        wire.query('DROP TABLE public.tx_contract')
    finally:
        # Restore libpq's idle socket before outer failure cleanup too.
        try:
            wire.exchange([], '<cleanup sync>', 'cleanup/Sync')
            wire.query('ROLLBACK')
        finally:
            wire.sock.close()
    A.close()
    B.close()
    record['cleanup'].append(dict(stage=stage, table_dropped=True, connections_closed=True))

def active_cancel(A, B, check, side, record, variant):
    wire = PortalWire(A, record.setdefault('cancel_segments', []))
    explicit = variant == 'explicit'
    # Same input on both endpoints. FOR UPDATE provides the reference's real
    # lock wait; the comment activates an existing product scheduling hook.
    sql = O + ' FOR UPDATE /* sleep */'
    def pbx(name, sql, describe=False):
        n = name.encode() + b'\0'
        messages = [(b'P', n + sql.encode() + b'\0\0\0'), (b'B', n+n+b'\0'*6)]
        if describe: messages += [(b'D', b'P'+n)]
        return messages + [(b'E', n+struct.pack('!I',0))]
    paths = [Path(os.environ['PG12_CANCEL_BARRIER']).with_suffix('.'+suffix) for suffix in ('arm','entered','release')]
    worker = None
    cancel = api('PQgetCancel', P, P)(A.ptr)
    assert cancel
    outcome = {}
    try:
        # Normal completion proves that the identical SELECT is executable.
        r = wire.exchange(pbx('normal', sql, True), sql, 'normal/Sync', 'normal')
        assert r['rows'] == S0 and r['transaction'] == 'I' and r['command'] == 'SELECT 2', r
        assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('value',23,4,-1,0)], r
        if explicit: assert wire.query('BEGIN')['transaction'] == 'T'
        update = 'UPDATE public.tx_contract SET value=11 WHERE id=1'
        r = wire.exchange(pbx('prefix', update), update, 'prefix/Flush', flush=True)
        assert r['tags'] == ['1','2','C'] and r['command'] == 'UPDATE 1', r
        check(B, S0, 'I')
        if side == 'reference':
            B.query('BEGIN')
            B.query('SELECT id,value FROM public.tx_contract WHERE id=2 FOR UPDATE')
        else:
            for path in paths: path.unlink(missing_ok=True)
            paths[0].write_text('arm one request')
        def run():
            try: outcome['row'] = wire.exchange(pbx('cancelled', sql, True), sql, 'active Execute/Flush', 'cancelled', flush=True)
            except BaseException as error: outcome['error'] = error
        worker = threading.Thread(target=run)
        worker.start()
        deadline = time.monotonic()+10
        while True:
            if side == 'reference':
                B.query('SELECT pg_stat_clear_snapshot()')
                r = B.query(f"SELECT state,wait_event_type FROM pg_stat_activity WHERE pid={backendpid(A.ptr)}")
                entered = r['rows'] == [['active','Lock']]
            else:
                entered = paths[1].exists()
            if entered: break
            assert worker.is_alive(), outcome
            assert time.monotonic() < deadline, 'no controlled active execution'
            time.sleep(.002) # polling an observed barrier, never inferring activity
        record.setdefault('activity', []).append(dict(explicit=explicit, entered=True,
            control='pg_stat_activity active/Lock' if side == 'reference' else 'shared mockStmtSlow callback'))
        error = c.create_string_buffer(256)
        assert api('PQcancel', I, P, P, I)(cancel, error, len(error)) == 1, 'cancel send failed'
        # PQcancel waits for EOF of the short cancellation connection. Product
        # consumes the key synchronously before EOF, so release cannot race it.
        if side == 'aster': paths[2].write_text('release after cancellation EOF')
        worker.join(12)
        assert not worker.is_alive(), 'cancelled execution did not finish'
        if 'error' in outcome: raise outcome['error']
        r = outcome['row']
        assert r['diagnostics']['C'] == '57014' and not r['command'], r
        # PostgreSQL may send the first locked row before waiting on row 2;
        # partial DataRows are observations, never a successful statement.
        assert r['rows'] in ([], [['1','11']]), r
        assert r['transaction'] is None and [t for t in r['tags'] if t != 'D'] == ['1','2','T','E'], r
        if side == 'reference': B.query('ROLLBACK')
        r = wire.exchange(pbx('discard', 'INSERT INTO public.tx_contract VALUES (3,30)'),
                          'discard INSERT; Sync', 'discard/Sync')
        assert r['tags'] == ['Z'] and r['transaction'] == ('E' if explicit else 'I'), r
        check(B, S0, 'I')
        if explicit:
            r = wire.query('SELECT 1::int4')
            assert r['diagnostics']['C'] == '25P02' and r['transaction'] == 'E', r
            assert wire.query('ROLLBACK')['transaction'] == 'I'
        r = wire.query(O)
        assert r['rows'] == S0 and r['transaction'] == 'I', r
        assert api('PQcancel', I, P, P, I)(cancel, error, len(error)) == 1
        r = wire.exchange(pbx('idle_probe', 'SELECT 1::int4', True), 'SELECT 1::int4', 'idle cancel/new PBX', 'idle_probe')
        assert r['rows'] == [['1']] and r['transaction'] == 'I' and not r['diagnostics']['C'], r
        check(B, S0, 'I')
        assert wire.query('DROP TABLE public.tx_contract')['command'] == 'DROP TABLE'
    finally:
        paths[2].write_text('finally release') if side == 'aster' else None
        if worker: worker.join(12)
        try:
            B.query('ROLLBACK', required=False)
            wire.exchange([], '<cleanup sync>', 'cleanup/Sync')
            wire.query('ROLLBACK')
        finally:
            wire.sock.close()
            api('PQfreeCancel', None, P)(cancel)
            for path in paths: path.unlink(missing_ok=True)
    A.close(); B.close()
    record['cleanup'].append(dict(explicit=explicit, table_dropped=True, connections_closed=True, barrier_released=True))

def observe_transaction(side, admin_dsn):
    obs, connections = [], []
    record = {'side':side, 'repository_sha':head, 'client_version':libversion(),
              'storage':'PostgreSQL local persistent reference' if side == 'reference' else ('RealTiKV isolated playground' if a.storage == 'tikv' else 'CreateAnalyzeSession shared test storage (not RealTiKV)'),
              'observations':obs, 'cleanup':[]}
    admin = None
    created = False
    try:
        admin = Connection(admin_dsn, 'admin', obs)
        record['server_version_num'] = serverversion(admin.ptr)
        record['source_commit_sha'] = head if side == 'aster' else 'unverified: reference installed from release archive'
        if a.case == 'active_cancel' and side == 'reference':
            # Release builds have archive provenance rather than a Git checkout.
            record['source_archive_sha256'] = os.environ.get('PG18_REFERENCE_SOURCE_SHA256', 'unverified')
        if side == 'reference':
            admin.query(f'CREATE DATABASE {a.database}')
            created = True
        dsn = admin_dsn + f' dbname={a.database}'
        for extended in ((False,) if a.case in ('active_cancel', 'query_block', 'extended_error', 'explicit_sync') else (False, True)):
            for inserted in (('implicit', 'explicit') if a.case == 'active_cancel' else ('create', 'drop') if a.case == 'database_ddl_gate' else ('Parse', 'Bind', 'Execute', 'success') if a.case == 'explicit_sync' else ('Parse', 'Bind', 'Execute') if a.case == 'extended_error' else ('success', 'duplicate', 'lexical', 'syntax') if a.case == 'query_block' else (('terminate', 'socket') if a.case == 'disconnect' else (('23505', '22012') if a.case in ('failed_rollback', 'failed_commit') else (('without_prefix','with_prefix') if a.case == 'old_portal' else (('create', 'drop', 'alter', 'truncate') if a.case in ('table_ddl_gate', 'mysql_ddl_coexist') else (('create_index','drop_view','create_view','drop_index','create_sequence','alter_sequence','drop_sequence') if a.case == 'other_ddl_gate' else ('30', 'NULL'))))))):
                A, B = [Connection(dsn, n, obs) for n in ('A','B')]
                connections += [A,B]
                for conn in (A,B):
                    conn.query("SET TimeZone TO 'UTC'" if side == 'reference' else "SET time_zone = '+00:00'")
                    conn.query('SET search_path TO public')
                if side == 'reference':
                    A.query('SET default_transaction_isolation TO \'read committed\'')
                for sql in ('SELECT version()', 'SHOW server_version_num', ('SHOW transaction_isolation' if side == 'reference' else 'SELECT @@transaction_isolation'), ('SHOW TimeZone' if side == 'reference' else 'SELECT @@time_zone')):
                    r = A.query(sql, required=False)
                    if sql == 'SHOW server_version_num' and side == 'reference':
                        assert r['rows'] == [['180006']], 'exact PG18.6 required; record 18.4 separately'
                if a.case == 'database_ddl_gate':
                    database_ddl_gate(A, B, admin, side, record, extended, inserted)
                    continue
                if a.case == 'other_ddl_gate':
                    other_ddl_gate(A, B, side, record, extended, inserted)
                    continue
                if a.case in ('table_ddl_gate', 'mysql_ddl_coexist'):
                    table_ddl_gate(A, B, side, record, extended, inserted)
                    continue
                A.query('CREATE TABLE public.tx_contract (id integer PRIMARY KEY,value integer)')
                old = None if a.case == 'rollback' and inserted == 'NULL' else '10'
                initial = [['1',old],['2','20']]
                A.query(f"INSERT INTO public.tx_contract VALUES (1,{old or 'NULL'}),(2,20)")
                def check(conn, expected, state):
                    r = conn.query(O, extended)
                    assert r['rows'] == expected, r
                    assert r['transaction'] == state, r
                    assert [(f['name'],f['oid'],f['typlen'],f['typmod'],f['format']) for f in r['columns']] == [('id',23,4,-1,0),('value',23,4,-1,0)], r
                    return r
                check(B,initial,'I')
                if a.case == 'active_cancel':
                    active_cancel(A, B, check, side, record, inserted)
                    continue
                if a.case == 'explicit_sync':
                    explicit_sync(A, B, check, record, inserted)
                    continue
                if a.case == 'extended_error':
                    extended_error(A, B, check, record, inserted)
                    continue
                if a.case == 'extended_success':
                    extended_success(A, B, check, record, extended, inserted)
                    continue
                if a.case == 'query_block':
                    check(A, initial, 'I')
                    wire = PortalWire(A, record.setdefault('query_blocks', []))
                    try:
                        prefix = 'UPDATE public.tx_contract SET value=11 WHERE id=1; '
                        sql = {
                            'success': prefix + 'INSERT INTO public.tx_contract VALUES (3,30)',
                            'duplicate': prefix + 'INSERT INTO public.tx_contract VALUES (1,99); INSERT INTO public.tx_contract VALUES (4,40)',
                            'lexical': "; /* ; in block comment */ " + prefix + "SELECT ';' AS marker; -- ; inside comment\n ; INSERT INTO public.tx_contract VALUES (3,NULL);;",
                            'syntax': prefix + 'INSERT INTO public.tx_contract VALUES (3,30); SELECT FROM',
                        }[inserted]
                        r = wire.query(sql)
                        assert r['transaction'] == 'I', r
                        if inserted in ('duplicate', 'syntax'):
                            assert r['diagnostics']['C'] == ('23505' if inserted == 'duplicate' else '42601'), r
                            assert r['commands'] == (['UPDATE 1'] if inserted == 'duplicate' else []), r
                            expected = initial
                        else:
                            assert r['diagnostics']['C'] is None, r
                            assert r['commands'] == (['UPDATE 1','INSERT 0 1'] if inserted == 'success' else ['UPDATE 1','SELECT 1','INSERT 0 1']), r
                            if inserted == 'lexical':
                                assert r['rows'] == [[';']] and r['columns'][0]['name'] == 'marker', r
                            expected = [['1','11'],['2','20'],['3',None if inserted == 'lexical' else '30']]
                    finally:
                        wire.sock.close()
                    for conn in (A, B):
                        check(conn, expected, 'I')
                        probe = conn.query('SELECT 1::int4')
                        assert probe['rows'] == [['1']] and probe['transaction'] == 'I', probe
                    A.query('DROP TABLE public.tx_contract')
                    A.close()
                    B.close()
                    record['cleanup'].append(dict(scenario=inserted, table_dropped=True, connections_closed=True))
                    continue
                if a.case == 'savepoint_gate':
                    savepoint_gate(A, B, check, side, record, extended, inserted)
                    continue
                if a.case == 'old_portal':
                    old_portals(A, B, check, side, record, extended, inserted)
                    B.close()
                    record['cleanup'].append({'mode':'extended' if extended else 'simple','table_dropped':True,'connections_closed':True})
                    continue
                if a.case in ('failed_rollback', 'failed_commit'):
                    failure = 'INSERT INTO public.tx_contract VALUES (1,99)' if inserted == '23505' else 'SELECT 1/0 AS bad'
                    def error(sql, code, state):
                        r = A.query(sql, extended, required=False)
                        assert r['status'] == 7 and r['diagnostics']['C'] == code and r['transaction'] == state, r
                        assert not r['rows'] and not r['command'], r
                        check(B, initial, 'I')
                    def command(conn, sql, tag, state, affected=''):
                        r = conn.query(sql, extended)
                        assert r['command'] == tag and r['transaction'] == state and r['affected'] == affected and not r['columns'] and not r['rows'], r
                    def probe(conn):
                        r = conn.query('SELECT 1::int4', extended)
                        assert r['rows'] == [['1']] and r['transaction'] == 'I', r
                    command(A, 'BEGIN', 'BEGIN', 'T')
                    command(A, 'UPDATE public.tx_contract SET value=11 WHERE id=1', 'UPDATE 1', 'T', '1')
                    check(A, [['1','11'],['2','20']], 'T')
                    check(B, initial, 'I')
                    command(A, 'INSERT INTO public.tx_contract VALUES (3,30)', 'INSERT 0 1', 'T', '1')
                    check(A, [['1','11'],['2','20'],['3','30']], 'T')
                    check(B, initial, 'I')
                    error(failure, inserted, 'E')
                    if a.case == 'failed_commit':
                        command(A, 'COMMIT', 'ROLLBACK', 'I')
                        for conn in (A,B):
                            check(conn, initial, 'I')
                            probe(conn)
                        command(A, 'BEGIN', 'BEGIN', 'T')
                        command(A, 'INSERT INTO public.tx_contract VALUES (5,50)', 'INSERT 0 1', 'T', '1')
                        check(B, initial, 'I')
                        command(A, 'COMMIT', 'COMMIT', 'I')
                        for conn in (A,B):
                            check(conn, initial + [['5','50']], 'I')
                        A.query('DROP TABLE public.tx_contract')
                        for conn in (A,B):
                            conn.close()
                        record['cleanup'].append({'mode':'extended' if extended else 'simple','first_error':inserted,'table_dropped':True,'connections_closed':True})
                        continue
                    for sql in ('SELECT 1::int4', O,
                                'INSERT INTO public.tx_contract VALUES (4,40)',
                                'UPDATE public.tx_contract SET value=99 WHERE id=2',
                                'DELETE FROM public.tx_contract WHERE id=2',
                                'SHOW search_path', 'SELECT oid,nspname FROM pg_catalog.pg_namespace'):
                        error(sql, '25P02', 'E')
                    C = Connection(dsn, 'C', obs)
                    connections.append(C)
                    for conn in (B,C):
                        probe(conn)
                        check(conn, initial, 'I')
                    command(B, 'BEGIN', 'BEGIN', 'T')
                    command(B, 'UPDATE public.tx_contract SET value=21 WHERE id=2', 'UPDATE 1', 'T', '1')
                    check(B, [['1','10'],['2','21']], 'T')
                    command(B, 'ROLLBACK', 'ROLLBACK', 'I')
                    check(B, initial, 'I')
                    error('SELECT 1::int4', '25P02', 'E')
                    command(A, 'ROLLBACK', 'ROLLBACK', 'I')
                    for conn in (A,B,C):
                        check(conn, initial, 'I')
                        probe(conn)
                    error(failure, inserted, 'I')
                    probe(A)
                    check(A, initial, 'I')
                    command(A, 'BEGIN', 'BEGIN', 'T')
                    command(A, 'INSERT INTO public.tx_contract VALUES (5,50)', 'INSERT 0 1', 'T', '1')
                    check(B, initial, 'I')
                    command(A, 'COMMIT', 'COMMIT', 'I')
                    for conn in (A,B,C):
                        check(conn, initial + [['5','50']], 'I')
                    A.query('DROP TABLE public.tx_contract')
                    for conn in (A,B,C):
                        conn.close()
                    record['cleanup'].append({'mode':'extended' if extended else 'simple','first_error':inserted,'table_dropped':True,'connections_closed':True})
                    continue
                if a.case == 'disconnect':
                    pid = backendpid(A.ptr)
                    A.query('BEGIN', extended)
                    for sql, command in (
                        ('UPDATE public.tx_contract SET value=11 WHERE id=1', 'UPDATE 1'),
                        ('DELETE FROM public.tx_contract WHERE id=2', 'DELETE 1'),
                        ('INSERT INTO public.tx_contract VALUES (3,30)', 'INSERT 0 1'),
                    ):
                        r = A.query(sql, extended)
                        assert r['transaction'] == 'T' and r['command'] == command and r['affected'] == '1' and not r['columns'], r
                    check(A, [['1','11'],['3','30']], 'T')
                    check(B, initial, 'I')
                    if inserted == 'socket':
                        # Shut down a duplicate of libpq's real TCP socket before
                        # PQfinish: no Terminate can be sent on this connection.
                        with socket.socket(fileno=os.dup(pqsocket(A.ptr))) as raw:
                            raw.shutdown(socket.SHUT_RDWR)
                    A.close()
                    deadline = time.monotonic() + 10
                    if side == 'reference':
                        while admin.query(f'SELECT pid FROM pg_stat_activity WHERE pid={pid}')['rows']:
                            assert time.monotonic() < deadline, 'reference session cleanup timed out'
                            time.sleep(.01)
                    elif a.storage != 'tikv':
                        barrier = Path(os.environ['PG12_CLEANUP_BARRIER'])
                        barrier.write_text(f'{extended}-{inserted}')
                        while barrier.exists():
                            assert time.monotonic() < deadline, 'Aster session cleanup timed out'
                            time.sleep(.01)
                    # RealTiKV has no in-process test barrier. The independent
                    # reader plus the replacement writer's commit below prove
                    # rollback visibility and release of the same inserted key.
                    check(B, initial, 'I')
                    C = Connection(dsn, 'A2', obs)
                    connections.append(C)
                    check(C, initial, 'I')
                    r = C.query('SELECT 1::int4', extended)
                    assert r['rows'] == [['1']] and r['transaction'] == 'I', r
                    r = C.query('BEGIN', extended)
                    assert r['transaction'] == 'T' and r['command'] == 'BEGIN' and not r['columns'], r
                    r = C.query('INSERT INTO public.tx_contract VALUES (3,30)', extended)
                    assert r['command'] == 'INSERT 0 1' and r['affected'] == '1' and r['transaction'] == 'T', r
                    r = C.query('COMMIT', extended)
                    assert r['transaction'] == 'I' and r['command'] == 'COMMIT' and not r['columns'], r
                    check(C, initial + [['3','30']], 'I')
                    check(B, initial + [['3','30']], 'I')
                    C.query('DROP TABLE public.tx_contract')
                    C.close()
                    B.close()
                    record['cleanup'].append({'mode':'extended' if extended else 'simple', 'disconnect':inserted, 'session_cleanup_confirmed':True, 'table_dropped':True, 'connections_closed':True})
                    continue
                if a.case == 'outside_end':
                    check(A,initial,'I')
                    for sql in ('COMMIT', 'ROLLBACK'):
                        r = A.query(sql, extended)
                        assert r['transaction'] == 'I' and r['command'] == sql and not r['columns'] and not r['rows'] and r['affected'] == '', r
                        check(A,initial,'I')
                        check(B,initial,'I')
                        assert r['notices'] == [dict(S='WARNING', V='WARNING', C='25P01',
                            M='there is no transaction in progress', D=None, H=None, P=None)], r
                    r = A.query('BEGIN', extended)
                    assert r['transaction'] == 'T' and r['command'] == 'BEGIN' and not r['columns'] and not r['notices'], r
                    r = A.query('ROLLBACK', extended)
                    assert r['transaction'] == 'I' and r['command'] == 'ROLLBACK' and not r['columns'] and not r['notices'], r
                    r = A.query('SELECT 1::int4', extended)
                    assert r['rows'] == [['1']] and r['transaction'] == 'I', r
                    check(A,initial,'I')
                    check(B,initial,'I')
                    A.query('DROP TABLE public.tx_contract')
                    for conn in (A,B):
                        conn.close()
                    record['cleanup'].append({'mode':'extended' if extended else 'simple','insert':inserted,'table_dropped':True,'connections_closed':True})
                    continue
                if a.case == 'single_implicit':
                    r = A.query(f'INSERT INTO public.tx_contract VALUES (3,{inserted})', extended)
                    assert r['transaction'] == 'I' and r['command'] == 'INSERT 0 1' and r['affected'] == '1' and not r['columns'], r
                    expected = initial + [['3', None if inserted == 'NULL' else '30']]
                    check(A, expected, 'I')
                    check(B, expected, 'I')
                    r = A.query('INSERT INTO public.tx_contract VALUES (4,40),(1,99)', extended, required=False)
                    assert r['status'] == 7 and r['diagnostics']['C'] == '23505' and r['transaction'] == 'I', r
                    check(B, expected, 'I')
                    check(A, expected, 'I')
                    r = A.query('SELECT 1::int4', extended)
                    assert r['rows'] == [['1']] and r['transaction'] == 'I', r
                    A.query('DROP TABLE public.tx_contract')
                    for conn in (A, B):
                        conn.close()
                    record['cleanup'].append({'mode':'extended' if extended else 'simple','insert':inserted,'table_dropped':True,'connections_closed':True})
                    continue
                r = A.query('BEGIN', extended)
                assert r['transaction'] == 'T' and not r['columns'], r
                check(A,initial,'T')
                check(B,initial,'I')
                if a.case == 'rollback':
                    for sql, expected, command in (
                        ('UPDATE public.tx_contract SET value=11 WHERE id=1', [['1','11'],['2','20']], 'UPDATE 1'),
                        ('DELETE FROM public.tx_contract WHERE id=2', [['1','11']], 'DELETE 1'),
                    ):
                        r = A.query(sql, extended)
                        assert r['transaction'] == 'T' and r['affected'] == '1' and r['command'] == command and not r['columns'], r
                        check(A,expected,'T')
                        check(B,initial,'I')
                    r = A.query('ROLLBACK', extended)
                    assert r['transaction'] == 'I' and r['command'] == 'ROLLBACK' and not r['columns'], r
                    check(A,initial,'I')
                    check(B,initial,'I')
                    r = A.query('SELECT 1::int4', extended)
                    assert r['rows'] == [['1']] and r['transaction'] == 'I', r
                    A.query('BEGIN', extended)
                    r = A.query('INSERT INTO public.tx_contract VALUES (5,50)', extended)
                    assert r['affected'] == '1' and r['transaction'] == 'T', r
                    check(A,initial + [['5','50']],'T')
                    check(B,initial,'I')
                    r = A.query('COMMIT', extended)
                    assert r['command'] == 'COMMIT' and r['transaction'] == 'I', r
                    check(A,initial + [['5','50']],'I')
                    check(B,initial + [['5','50']],'I')
                    C = Connection(dsn,'C',obs)
                    connections.append(C)
                    check(C,initial + [['5','50']],'I')
                else:
                    r = A.query(f'INSERT INTO public.tx_contract VALUES (3,{inserted})', extended)
                    assert r['transaction'] == 'T' and r['affected'] == '1' and not r['columns'], r
                    S1 = S0 + [['3',None if inserted == 'NULL' else '30']]
                    check(A,S1,'T')
                    check(B,initial,'I')
                    if a.case == 'repeated_begin':
                        r = A.query('BEGIN', extended)
                        assert r['transaction'] == 'T' and r['command'] == 'BEGIN' and not r['columns'], r
                        check(A,S1,'T')
                        check(B,initial,'I')
                        assert r['notices'] == [dict(S='WARNING', V='WARNING', C='25001',
                            M='there is already a transaction in progress', D=None, H=None, P=None)], r
                        r = A.query('ROLLBACK', extended)
                        assert r['transaction'] == 'I' and r['command'] == 'ROLLBACK' and not r['columns'], r
                        check(A,initial,'I')
                        check(B,initial,'I')
                        A.query('DROP TABLE public.tx_contract')
                        for conn in (A,B):
                            conn.close()
                        record['cleanup'].append({'mode':'extended' if extended else 'simple','insert':inserted,'table_dropped':True,'connections_closed':True})
                        continue
                    r = A.query('COMMIT', extended)
                    assert r['transaction'] == 'I' and r['command'] == 'COMMIT' and not r['columns'], r
                    check(A,S1,'I')
                    check(B,S1,'I')
                    C = Connection(dsn,'C',obs)
                    connections.append(C)
                    check(C,S1,'I')
                # Preserve the final independently observed commit for restart;
                # never recreate the fixture in the restart reader.
                preserve = a.storage == 'tikv' and a.case == 'commit' and side == 'aster' and extended and inserted == 'NULL'
                if preserve:
                    (evidence / 'restart-baseline.json').write_text(json.dumps(obs[-1], indent=2))
                # SQL NULL is represented as None, never as an empty string.
                if not preserve:
                    A.query('DROP TABLE public.tx_contract')
                for conn in (A,B,C):
                    conn.close()
                record['cleanup'].append({'mode':'extended' if extended else 'simple','insert':inserted,'table_dropped':not preserve,'preserved_for_restart':preserve,'connections_closed':True})
        record['cases_passed'] = 2 if a.case == 'active_cancel' else 14 if a.case == 'other_ddl_gate' else 16 if a.case in ('table_ddl_gate', 'mysql_ddl_coexist') else 3 if a.case == 'extended_error' else 4
    finally:
        for conn in connections:
            if conn.ptr:
                conn.query('ROLLBACK', required=False)
                if a.case in ('table_ddl_gate', 'mysql_ddl_coexist'):
                    conn.query('DROP TABLE IF EXISTS public.txddl_probe', required=False)
                    conn.query('DROP TABLE IF EXISTS public.txddl_sentinel', required=False)
                if not (a.storage == 'tikv' and a.case == 'commit' and side == 'aster' and (evidence / 'restart-baseline.json').exists()):
                    conn.query('DROP TABLE IF EXISTS public.tx_contract', required=False)
                conn.close()
        if admin:
            if created:
                admin.query(f'DROP DATABASE {a.database}')
                record['cleanup'].append({'database_dropped':True})
            admin.close()
        (evidence / f'{side}.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    return record

if a.restart_check:
    baseline = json.loads((evidence / 'restart-baseline.json').read_text())
    observations = []
    reader = Connection(f'host={a.host} port={a.port} user=root dbname={a.database}', 'restart reader', observations)
    try:
        actual = reader.query(O)
        # Full values (including NULL), descriptors, command and transaction
        # state must survive server restart; no setup runs in this mode.
        for key in ('rows','columns','command','affected','transaction'):
            assert actual[key] == baseline[key], (key, actual, baseline)
        reader.query('DROP TABLE public.tx_contract')
        (evidence / 'restart.json').write_text(json.dumps(dict(repository_sha=head,
            storage='RealTiKV isolated playground', baseline=baseline,
            observations=observations, cases_passed=1), indent=2))
    finally:
        reader.close()
    print(json.dumps(dict(case='restart', cases_per_endpoint=1, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)

for side, dsn in [('reference', reference), ('aster', f'host={a.host} port={a.port} user=root dbname=test')]:
    outputs[side] = observe_transaction(side, dsn)
# Compare independently collected behavior, excluding endpoint identity and setup.
def behavior(record):
    return [{k:r[k] for k in (('sql','phase','transaction','columns','rows','command','affected','diagnostics','notices') if a.case in ('repeated_begin', 'outside_end') else ('sql','phase','transaction','columns','rows','command','affected','diagnostics'))}
            for r in record['observations']
            if r['sql'] in (O,'BEGIN','COMMIT','ROLLBACK','SELECT 1::int4') or (a.case in ('failed_rollback', 'failed_commit') and r['sql'] in ('SELECT 1/0 AS bad', 'SHOW search_path', 'SELECT oid,nspname FROM pg_catalog.pg_namespace')) or r['sql'].startswith(('INSERT INTO public.tx_contract','UPDATE public.tx_contract','DELETE FROM public.tx_contract'))]
if a.case == 'database_ddl_gate':
    assert all(v['cases_passed'] == 4 and len(v['database_ddl_boundaries']) == 4 for v in outputs.values())
    for ref, local in zip(outputs['reference']['database_ddl_boundaries'], outputs['aster']['database_ddl_boundaries']):
        for key in ('mode', 'operation', 'recovered'):
            assert ref[key] == local[key], (ref, local)
    (evidence / 'comparison.json').write_text(json.dumps(dict(cases_per_endpoint=4,
        boundaries={side:v['database_ddl_boundaries'] for side,v in outputs.items()}), indent=2))
    print(json.dumps(dict(case=a.case, cases_per_endpoint=4, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)
if a.case == 'other_ddl_gate':
    assert all(v['cases_passed'] == 14 and len(v['other_ddl_boundaries']) == 14 for v in outputs.values())
    for ref, local in zip(outputs['reference']['other_ddl_boundaries'], outputs['aster']['other_ddl_boundaries']):
        for key in ('operation', 'mode'):
            assert ref[key] == local[key], (ref, local)
        assert ref['recovered']['sentinel'] == local['recovered']['sentinel'] == [['1','10']]
        assert ref['recovered']['metadata'] == local['recovered']['metadata']
        assert ref['recovered']['directory'] == ref['baseline']
        assert local['recovered']['directory'] == local['baseline']
    (evidence / 'comparison.json').write_text(json.dumps(dict(cases_per_endpoint=14,
        reference='transactional DDL success then rollback', aster='early refusal without implicit commit',
        boundaries={side:v['other_ddl_boundaries'] for side,v in outputs.items()}), indent=2))
    print(json.dumps(dict(case=a.case, cases_per_endpoint=14, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)
if a.case in ('table_ddl_gate', 'mysql_ddl_coexist'):
    assert all(v['cases_passed'] == 16 for v in outputs.values())
    for ref, local in zip(outputs['reference']['ddl_boundaries'], outputs['aster']['ddl_boundaries']):
        for key in ('mode', 'operation', 'exit', 'recovered'):
            assert ref[key] == local[key], (key, ref, local)
    assert len(outputs['reference']['ddl_boundaries']) == len(outputs['aster']['ddl_boundaries']) == 16
    (evidence / 'comparison.json').write_text(json.dumps(dict(
        cases_per_endpoint=16, reference='transactional DDL success and rollback',
        aster='0A000 early refusal without implicit commit',
        boundaries={side:v['ddl_boundaries'] for side,v in outputs.items()}), indent=2))
    print(json.dumps(dict(case=a.case, cases_per_endpoint=16, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)
if a.case == 'active_cancel':
    ref, local = [outputs[side]['cancel_segments'] for side in ('reference','aster')]
    assert len(ref) == len(local) > 0
    for r,l in zip(ref,local):
        for key in ('sql','phase','transaction','rows','command','affected','tags'):
            if r['phase'] == 'active Execute/Flush' and key in ('rows','tags'):
                continue # independently asserted partial rows need not precede cancellation
            assert r[key] == l[key], (key,r,l)
        assert r['diagnostics']['C'] == l['diagnostics']['C'], (r,l)
        # Only the bare recovery expression has endpoint-specific labels.
        if r['sql'] == 'SELECT 1::int4':
            assert [{**f,'name':'expression'} for f in r['columns']] == [{**f,'name':'expression'} for f in l['columns']], (r,l)
        else:
            assert r['columns'] == l['columns'], (r,l)
    assert all(v['cases_passed'] == 2 and len(v['activity']) == 2 for v in outputs.values())
    (evidence/'comparison.json').write_text(json.dumps(dict(matched=True, cases_per_endpoint=2, wire_observations=len(ref),
        partial_rows=[dict(reference=r['rows'],aster=l['rows']) for r,l in zip(ref,local) if r['phase']=='active Execute/Flush'],
        diagnostics=[dict(reference=r['diagnostics'],aster=l['diagnostics']) for r,l in zip(ref,local) if r['diagnostics']['C']]),indent=2))
    print(json.dumps(dict(case=a.case,cases_per_endpoint=2,evidence=str(evidence),cleanup='passed')))
    raise SystemExit(0)
if a.case in ('extended_error', 'explicit_sync'):
    differences, probe_names = [], []
    ref, local = [outputs[side]['explicit_segments' if a.case == 'explicit_sync' else 'error_segments'] for side in ('reference', 'aster')]
    assert len(ref) == len(local) > 0
    for r, l in zip(ref, local):
        for key in ('sql','phase','transaction','rows','columns','command','affected','tags'):
            # Bare recovery expression names are already tracked separately by
            # the existing transaction suite; preserve raw descriptors here.
            if key == 'columns' and r['sql'] == 'SELECT 1::int4':
                if r[key] != l[key]:
                    probe_names.append(dict(reference=r[key], aster=l[key]))
                assert [{**f, 'name':'<expression>'} for f in r[key]] == [{**f, 'name':'<expression>'} for f in l[key]], (r,l)
            else:
                assert r[key] == l[key], (key,r,l)
        assert r['diagnostics']['C'] == l['diagnostics']['C'], (r,l)
        if r['diagnostics']['C']:
            differences.append(dict(stage=r['phase'], reference=r['diagnostics'], aster=l['diagnostics']))
    count = 4 if a.case == 'explicit_sync' else 3
    assert all(v['cases_passed'] == count for v in outputs.values())
    (evidence / 'comparison.json').write_text(json.dumps(dict(stages_per_endpoint=count, wire_observations=len(ref), matched=True, diagnostics=differences, recovery_probe_name_differences=probe_names),indent=2))
    print(json.dumps(dict(case=a.case, cases_per_endpoint=count, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)
if a.case == 'extended_success':
    ref, local = [outputs[side]['extended_segments'] for side in ('reference', 'aster')]
    assert len(ref) == len(local) and len(ref) > 0
    for r, l in zip(ref, local):
        for key in ('sql','phase','transaction','rows','columns','command','affected','tags','diagnostics'):
            assert r[key] == l[key], (key,r,l)
    assert all(v['cases_passed'] == 4 for v in outputs.values())
    (evidence / 'comparison.json').write_text(json.dumps(dict(segments_per_endpoint=4, wire_observations=len(ref), matched=True),indent=2))
    print(json.dumps(dict(case=a.case, cases_per_endpoint=4, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)
if a.case == 'query_block':
    differences = []
    ref, local = [outputs[side]['query_blocks'] for side in ('reference', 'aster')]
    assert len(ref) == len(local) == 4
    for r, l in zip(ref, local):
        for key in ('sql','phase','transaction','rows','columns','commands','tags'):
            assert r[key] == l[key], (key,r,l)
        assert r['diagnostics']['C'] == l['diagnostics']['C'], (r,l)
        differences.append(dict(sql=r['sql'], reference=r['diagnostics'], aster=l['diagnostics']))
    assert all(v['cases_passed'] == 4 for v in outputs.values())
    (evidence / 'diagnostic-differences.json').write_text(json.dumps(differences,indent=2))
    print(json.dumps(dict(case=a.case, cases_per_endpoint=4, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)
if a.case == 'savepoint_gate':
    # Success and safe rejection are distinct contracts; retain both independent
    # observations instead of demanding identical unsupported capabilities.
    assert all(v['cases_passed'] == 4 for v in outputs.values())
    (evidence / 'savepoint-boundaries.json').write_text(json.dumps(
        {side:v['savepoint_boundaries'] for side,v in outputs.items()}, indent=2))
    print(json.dumps(dict(case=a.case, cases_per_endpoint=4, evidence=str(evidence), cleanup='passed')))
    raise SystemExit(0)
reference_behavior, aster_behavior = [behavior(outputs[side]) for side in ('reference','aster')]
# The recovery probe's implicit expression name is a separate existing SQL
# naming difference, not a transaction outcome. Preserve raw metadata above
# and report the exact difference; compare all its other metadata unchanged.
probe_names = []
for ref, local in zip(reference_behavior, aster_behavior):
    if ref['sql'] == local['sql'] == 'SELECT 1::int4' and ref['phase'] != 'Parse/Sync' and ref['columns']:
        assert len(ref['columns']) == len(local['columns']) == 1
        names = (ref['columns'][0]['name'], local['columns'][0]['name'])
        assert names == ('int4','1::int4'), names
        probe_names.append({'phase':ref['phase'],'reference':names[0],'aster':names[1]})
        ref['columns'] = [dict(ref['columns'][0], name='<implicit recovery expression>')]
        local['columns'] = [dict(local['columns'][0], name='<implicit recovery expression>')]
(evidence / 'comparison.json').write_text(json.dumps({'recovery_probe_name_differences':probe_names},indent=2))
if a.case in ('single_implicit', 'failed_rollback', 'failed_commit'):
    # Raw diagnostics remain in endpoint evidence; compare SQLSTATE while
    # recording backend-specific wording and detail/position differences.
    differences = []
    assert len(reference_behavior) == len(aster_behavior)
    for ref, local in zip(reference_behavior, aster_behavior):
        if ref['diagnostics']['C'] in ('23505', '22012', '25P02'):
            differences.append({'sql':ref['sql'],'phase':ref['phase'],
                                'reference':ref['diagnostics'], 'aster':local['diagnostics']})
            assert local['diagnostics']['C'] == ref['diagnostics']['C']
            ref['diagnostics'] = {k:ref['diagnostics'][k] for k in 'SVC'}
            local['diagnostics'] = {k:local['diagnostics'][k] for k in 'SVC'}
    (evidence / 'diagnostic-differences.json').write_text(json.dumps(differences,indent=2))
assert reference_behavior == aster_behavior, 'endpoint transaction observations differ'
# Both independently observed endpoints must have passed the same four inputs.
assert outputs['reference']['cases_passed'] == outputs['aster']['cases_passed'] == 4

if a.case == 'old_portal':
    summary = {}
    for side in ('reference','aster'):
        rows = outputs[side]['portal_observations']
        gates = [r for r in rows if 'lifecycle' in r]
        assert len(gates) == 28, gates
        summary[side] = [{'portal':r['portal'],'sqlstate':r['diagnostics']['C'],'lifecycle':r['lifecycle']} for r in gates]
    (evidence / 'portal-lifecycle.json').write_text(json.dumps(summary,indent=2))
    # These are independently collected wire responses, not common expected
    # query results. Preserve full diagnostics/metadata and report differences.
    ref, local = [outputs[side]['portal_observations'] for side in ('reference','aster')]
    assert len(ref) == len(local)
    differences = []
    for r, l in zip(ref, local):
        keys = ('sql','phase','portal','transaction','rows','command','affected','tags')
        assert {k:r[k] for k in keys} == {k:l[k] for k in keys}, (r,l)
        assert r['diagnostics']['C'] == l['diagnostics']['C'], (r,l)
        if r['columns'] != l['columns'] or r['diagnostics'] != l['diagnostics']:
            differences.append({'sql':r['sql'],'phase':r['phase'],'portal':r['portal'],
                                'reference':{'columns':r['columns'],'diagnostics':r['diagnostics']},
                                'aster':{'columns':l['columns'],'diagnostics':l['diagnostics']}})
    (evidence / 'portal-differences.json').write_text(json.dumps(differences,indent=2))
print(json.dumps({'case':a.case,'cases_per_endpoint':4,'evidence':str(evidence),'cleanup':'passed'}))
