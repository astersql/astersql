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
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('--case', choices=['commit', 'rollback', 'single_implicit', 'repeated_begin', 'outside_end', 'disconnect', 'failed_rollback', 'failed_commit', 'old_portal'], required=True)
p.add_argument('--host', choices=['127.0.0.1'], required=True)
p.add_argument('--port', type=int, required=True)
p.add_argument('--database', required=True)
a = p.parse_args()
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
        while True:
            t = self.read(1).decode()
            body = self.read(struct.unpack('!I', self.read(4))[0]-4)
            row['tags'].append(t)
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
def observe_transaction(side, admin_dsn):
    obs, connections = [], []
    record = {'side':side, 'repository_sha':head, 'client_version':libversion(),
              'storage':'PostgreSQL local persistent reference' if side == 'reference' else 'CreateAnalyzeSession shared test storage (not RealTiKV)',
              'observations':obs, 'cleanup':[]}
    admin = None
    created = False
    try:
        admin = Connection(admin_dsn, 'admin', obs)
        record['server_version_num'] = serverversion(admin.ptr)
        record['source_commit_sha'] = head if side == 'aster' else 'unverified: reference installed from release archive'
        if side == 'reference':
            admin.query(f'CREATE DATABASE {a.database}')
            created = True
        dsn = admin_dsn + f' dbname={a.database}'
        for extended in (False, True):
            for inserted in (('terminate', 'socket') if a.case == 'disconnect' else (('23505', '22012') if a.case in ('failed_rollback', 'failed_commit') else (('without_prefix','with_prefix') if a.case == 'old_portal' else ('30', 'NULL')))):
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
                    else:
                        barrier = Path(os.environ['PG12_CLEANUP_BARRIER'])
                        barrier.write_text(f'{extended}-{inserted}')
                        while barrier.exists():
                            assert time.monotonic() < deadline, 'Aster session cleanup timed out'
                            time.sleep(.01)
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
                # SQL NULL is represented as None, never as an empty string.
                A.query('DROP TABLE public.tx_contract')
                for conn in (A,B,C):
                    conn.close()
                record['cleanup'].append({'mode':'extended' if extended else 'simple','insert':inserted,'table_dropped':True,'connections_closed':True})
        record['cases_passed'] = 4
    finally:
        for conn in connections:
            if conn.ptr:
                conn.query('ROLLBACK', required=False)
                conn.query('DROP TABLE IF EXISTS public.tx_contract', required=False)
                conn.close()
        if admin:
            if created:
                admin.query(f'DROP DATABASE {a.database}')
                record['cleanup'].append({'database_dropped':True})
            admin.close()
        (evidence / f'{side}.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    return record

for side, dsn in [('reference', reference), ('aster', f'host={a.host} port={a.port} user=root dbname=test')]:
    outputs[side] = observe_transaction(side, dsn)
# Compare independently collected behavior, excluding endpoint identity and setup.
def behavior(record):
    return [{k:r[k] for k in (('sql','phase','transaction','columns','rows','command','affected','diagnostics','notices') if a.case in ('repeated_begin', 'outside_end') else ('sql','phase','transaction','columns','rows','command','affected','diagnostics'))}
            for r in record['observations']
            if r['sql'] in (O,'BEGIN','COMMIT','ROLLBACK','SELECT 1::int4') or (a.case in ('failed_rollback', 'failed_commit') and r['sql'] in ('SELECT 1/0 AS bad', 'SHOW search_path', 'SELECT oid,nspname FROM pg_catalog.pg_namespace')) or r['sql'].startswith(('INSERT INTO public.tx_contract','UPDATE public.tx_contract','DELETE FROM public.tx_contract'))]
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
