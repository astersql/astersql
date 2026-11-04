# Copyright 2026 AsterSQL.
"""Search-path differential. Arguments: target TCP port, target database.

Queries use PQexec/PQexecParams; libpq protocol traces preserve error phase,
RowDescription source, and ReadyForQuery. The idle socket is used only to probe
an extra Sync after a failed extended query.
"""
import copy
import ctypes as c
import ctypes.util
import json
import os
import socket
import struct
import sys
from pathlib import Path

archive = Path(os.environ["PG11_30_EVIDENCE_DIR"])
archive.mkdir(parents=True, exist_ok=False)
output = (archive / "observations.jsonl").open("w")
def record(data):
    print(json.dumps(data), file=output, flush=True)
    print(json.dumps(data), flush=True)

library = os.environ.get('PG_LIBPQ_LIBRARY') or ctypes.util.find_library('pq')
assert library, 'PG_LIBPQ_LIBRARY/libpq required (no skip)'
pq = c.CDLL(library)
def api(name, result, *args):
    f = getattr(pq, name)
    f.restype, f.argtypes = result, args
    return f
connect = api('PQconnectdb', c.c_void_p, c.c_char_p)
status = api('PQstatus', c.c_int, c.c_void_p)
finish = api('PQfinish', None, c.c_void_p)
fd = api('PQsocket', c.c_int, c.c_void_p)
assert api('PQlibVersion', c.c_int)() >= 180000, 'libpq 18 required'
exec_query = api('PQexec', c.c_void_p, c.c_void_p, c.c_char_p)
exec_params = api('PQexecParams', c.c_void_p, c.c_void_p, c.c_char_p,
                  c.c_int, c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p, c.c_int)
result_status = api('PQresultStatus', c.c_int, c.c_void_p)
ntuples = api('PQntuples', c.c_int, c.c_void_p)
nfields = api('PQnfields', c.c_int, c.c_void_p)
fname = api('PQfname', c.c_char_p, c.c_void_p, c.c_int)
ftype = api('PQftype', c.c_uint, c.c_void_p, c.c_int)
fsize = api('PQfsize', c.c_int, c.c_void_p, c.c_int)
fmod = api('PQfmod', c.c_int, c.c_void_p, c.c_int)
fformat = api('PQfformat', c.c_int, c.c_void_p, c.c_int)
ftable = api('PQftable', c.c_uint, c.c_void_p, c.c_int)
ftablecol = api('PQftablecol', c.c_int, c.c_void_p, c.c_int)
getvalue = api('PQgetvalue', c.c_void_p, c.c_void_p, c.c_int, c.c_int)
getlength = api('PQgetlength', c.c_int, c.c_void_p, c.c_int, c.c_int)
getisnull = api('PQgetisnull', c.c_int, c.c_void_p, c.c_int, c.c_int)
cmdstatus = api('PQcmdStatus', c.c_char_p, c.c_void_p)
error_field = api('PQresultErrorField', c.c_char_p, c.c_void_p, c.c_int)
transaction_status = api('PQtransactionStatus', c.c_int, c.c_void_p)
clear = api('PQclear', None, c.c_void_p)
pq_trace = api('PQtrace', None, c.c_void_p, c.c_void_p)
pq_untrace = api('PQuntrace', None, c.c_void_p)
trace_flags = api('PQsetTraceFlags', None, c.c_void_p, c.c_int)
notice_receiver = api('PQsetNoticeReceiver', c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p)
libc = c.CDLL(None)
libc.fopen.restype, libc.fopen.argtypes = c.c_void_p, [c.c_char_p,c.c_char_p]
libc.fclose.restype, libc.fclose.argtypes = c.c_int, [c.c_void_p]
libc.fflush.restype, libc.fflush.argtypes = c.c_int, [c.c_void_p]
fflush = libc.fflush
connection_id = 0

def diagnostics(result):
    return {field: value.decode() for field in 'SVCMPDHFLR'
            if (value := error_field(result,ord(field))) is not None}
mysql_port = int(os.environ['PG11_MYSQL_PORT'])
assert 0 < mysql_port < 65536
reference = os.environ.get('PG18_REFERENCE_DSN')
assert reference, 'PG18_REFERENCE_DSN required (no skip)'
target = os.environ.get('PG11_TARGET_DSN') or (
    f'host=127.0.0.1 port={int(sys.argv[1])} dbname={sys.argv[2]} user=root sslmode=disable')

class Client:
    def __init__(self, dsn):
        self.conn = connect(dsn.encode())
        assert self.conn and status(self.conn) == 0, 'libpq connection failed (DSN redacted)'
        self.blocking = os.get_blocking(fd(self.conn))
        self.sock = socket.socket(fileno=os.dup(fd(self.conn)))
        self.sock.settimeout(10)
        global connection_id
        connection_id += 1
        self.trace_path = archive / f'libpq-{connection_id}.trace'
        self.trace = libc.fopen(os.fsencode(self.trace_path),b'w')
        assert self.trace, 'cannot open libpq trace'
        self.trace_offset = 0
        pq_trace(self.conn,self.trace)
        trace_flags(self.conn,1)  # PQTRACE_SUPPRESS_TIMESTAMPS
        self.notices = []
        @c.CFUNCTYPE(None,c.c_void_p,c.c_void_p)
        def receive_notice(arg,result):
            self.notices.append(diagnostics(result))
        self.receive_notice = receive_notice
        notice_receiver(self.conn,receive_notice,None)
    def close(self):
        if self.conn:
            pq_untrace(self.conn)
            assert libc.fclose(self.trace) == 0
            self.sock.close()
            os.set_blocking(fd(self.conn), self.blocking)
            finish(self.conn)
            self.conn = None
    def send(self, tag, body):
        self.sock.sendall(tag + struct.pack('!I', len(body) + 4) + body)
    def exact(self, n):
        out = b''
        while len(out) < n:
            part = self.sock.recv(n-len(out))
            assert part, 'unexpected EOF'
            out += part
        return out
    def run(self, sql, ext=False):
        self.notices.clear()
        result = (exec_params(self.conn, sql.encode(), 0, None, None, None, None, 0)
                  if ext else exec_query(self.conn, sql.encode()))
        assert result, 'libpq returned no result'
        try:
            code = result_status(result)
            assert code in (1, 2, 7), f'unexpected libpq result status {code}'
            o = dict(rows=[], metadata=[], origins=[], errors=[],
                     warnings=list(self.notices), command=[], ready='',
                     phase='parse' if ext else 'query')
            for col in range(nfields(result)):
                o['metadata'].append([fname(result,col).decode(), ftype(result,col),
                                      fsize(result,col), fmod(result,col), fformat(result,col)])
                o['origins'].append([ftable(result,col), ftablecol(result,col)])
            for row in range(ntuples(result)):
                o['rows'].append([
                    None if getisnull(result,row,col) else
                    c.string_at(getvalue(result,row,col), getlength(result,row,col)).decode()
                    for col in range(nfields(result))])
            if code == 7:
                o['errors'].append(diagnostics(result))
            else:
                command = cmdstatus(result).decode()
                if command: o['command'].append(command)
            fflush(self.trace)
            with self.trace_path.open() as stream:
                stream.seek(self.trace_offset)
                trace = stream.read()
                self.trace_offset = stream.tell()
            # Trace is produced by libpq itself, not inferred by replaying failed
            # SQL in a different transaction. Preserve it alongside PQresult fields.
            o['wire_trace'] = trace
            ready = None
            for line in trace.splitlines():
                fields = line.split('\t')
                if len(fields) < 3 or fields[0] != 'B': continue
                tag = fields[2]
                if tag == 'ParseComplete': o['phase'] = 'bind'
                elif tag == 'BindComplete': o['phase'] = 'describe-portal'
                elif tag in ('RowDescription','NoData'):
                    o['phase'] = 'execute' if ext else 'query'
                    if tag == 'RowDescription':
                        o['description_source'] = 'Describe(portal)' if ext else 'Query'
                elif tag == 'ReadyForQuery': ready = fields[3].strip()
            o['ready'] = {0:'I',2:'T',3:'E'}[transaction_status(self.conn)]
            assert ready == o['ready'], 'trace/libpq transaction status disagree'
            return o
        finally:
            clear(result)

# Compare semantic observations, retaining full diagnostics and object identities
# separately. Local table OIDs must be linked within each endpoint, never equated.
def semantic(o):
    return {k: o[k] for k in ('rows', 'metadata', 'command', 'ready')} | {
        'errors': [e['C'] for e in o['errors']],
        'warnings': [e['C'] for e in o['warnings']]}
def equal(a, b):
    assert semantic(a) == semantic(b), (semantic(a), semantic(b))

# Task 1 requires a configured connection default, rather than a SET that
# RESET would discard. Real libpq sends this through StartupMessage.options.
probe_error = api('PQerrorMessage', c.c_char_p, c.c_void_p)
for side, dsn in [('reference', reference), ('target', target)]:
    probe = connect((dsn + " options='-c search_path=pg_catalog'").encode())
    accepted = bool(probe) and status(probe) == 0
    record(dict(case_id='startup-default.pg_catalog', side=side,
                         accepted=accepted,
                         diagnostic=None if accepted else probe_error(probe).decode()))
    if probe: finish(probe)
    assert accepted, side + ' lacks real-libpq configured search_path startup'

# No fixed objects are touched in the shared reference database: this run owns a
# freshly created database, dropped in finally even after an assertion fails.
admin = Client(reference)
database = f'pg1130_{os.getpid()}_{sys.argv[1]}'
created = False
role_created = False
role = os.environ['PG11_30_ROLE']
connections = []
count = 0
try:
    raw_target = Client(target)
    try:
        record(dict(case_id='native-default', side='target', observation=raw_target.run('SHOW search_path')))
    finally:
        raw_target.close()
    for sql in ['SELECT version()', 'SHOW server_version_num', 'SHOW search_path']:
        record(dict(side='reference', case_id='provenance', sql=sql, observation=admin.run(sql)))
    result = admin.run('CREATE DATABASE '+database)
    assert not result['errors'], result
    created = True
    result = admin.run('CREATE ROLE '+role)
    assert not result['errors'], result
    role_created = True
    ref = reference + " dbname=" + database + " options='-c search_path=public'"
    for ext in (False, True):
        for initial in ['public', 'pg_catalog', '']:
            configured = []
            try:
                for dsn in [target, reference + ' dbname=' + database]:
                    configured.append(Client(dsn + " options='-c search_path=" + initial + "'"))
                for startup_index, sql in enumerate(['SHOW search_path', 'SET search_path TO public',
                            'SET SESSION search_path = pg_catalog', "SET DateStyle TO 'ISO, DMY'", 'RESET search_path', 'SHOW DateStyle',
                            'SHOW search_path', 'SET search_path TO public',
                            'SET search_path TO DEFAULT', 'SHOW search_path',
                            "SET search_path TO ''", 'SHOW search_path',
                            'SELECT current_schema()', 'SET SESSION search_path TO NULL',
                            'SHOW search_path extra', 'SELECT 1', 'RESET search_path',
                            'SHOW search_path']):
                    left, right = [client.run(sql, ext) for client in configured]
                    record(dict(case_id=f'startup.{initial}.{ext}.{startup_index}', sql=sql, target=left, reference=right))
                    count += 1
                    equal(left,right)
            finally:
                for client in configured: client.close()
        clients = [Client(dsn) for dsn in (target, target, ref, ref)]
        connections.extend(clients)
        a,b,r,s = clients
        def pair(sql, who='a'):
            global count
            x,y = (a,r) if who == 'a' else (b,s)
            left, right = x.run(sql,ext), y.run(sql,ext)
            case = f'{"extended" if ext else "query"}.{count}.{who}'
            record(dict(case_id=case, sql=sql, target=left, reference=right))
            count += 1
            equal(left,right)
            return left
        for sql in ['CREATE TABLE public.path_doc (id integer PRIMARY KEY,v integer)',
                    'INSERT INTO public.path_doc VALUES (1,10)',
                    'CREATE TABLE public.pg_namespace (nspname text)',
                    "INSERT INTO public.pg_namespace VALUES ('user_marker')"]:
            pair(sql)
        def baseline():
            pair('SHOW search_path','b')
            pair('SELECT id,v FROM path_doc ORDER BY id','b')
        pair('SET search_path TO pg_catalog'); baseline()
        pair('SHOW search_path'); pair('SELECT id,v FROM path_doc ORDER BY id')
        pair('SELECT id,v FROM public.path_doc ORDER BY id'); pair('RESET search_path'); baseline()
        for path in ['public','pg_catalog, public','public, pg_catalog']:
            pair('SET search_path TO '+path)
            pair("SELECT nspname FROM pg_namespace WHERE nspname = 'pg_catalog'")
            pair("SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname = 'pg_catalog'")
            pair('SELECT nspname FROM public.pg_namespace ORDER BY nspname'); baseline()
        for path in ["''", 'missing', 'missing, pg_catalog', 'missing, public']:
            pair('SET search_path TO '+path); pair('SHOW search_path')
            pair('SELECT current_schema()'); pair('SELECT id FROM path_doc')
            pair('SELECT id FROM public.path_doc'); baseline()
        pair('RESET search_path')
        for sql in ['SELECT id FROM public.missing_t', 'SELECT id FROM private.path_doc',
                    'SET search_path TO', 'SET SESSION search_path TO NULL']:
            pair(sql); pair('SHOW search_path'); baseline()
        for end in ['COMMIT','ROLLBACK']:
            for setting in ['SET search_path TO pg_catalog','RESET search_path',
                            'SET LOCAL search_path TO pg_catalog']:
                pair('SET search_path TO '+('pg_catalog' if setting.startswith('RESET') else 'public'))
                pair('BEGIN'); pair(setting); pair('SHOW search_path'); baseline()
                pair(end); pair('SHOW search_path'); pair('SELECT id,v FROM public.path_doc ORDER BY id')
            pair('SET search_path TO public'); pair('BEGIN'); pair('SET search_path TO pg_catalog')
            pair('SET LOCAL search_path TO public'); pair('SHOW search_path'); pair(end); pair('SHOW search_path')
        pair('RESET search_path'); pair('SET LOCAL search_path TO pg_catalog'); pair('SHOW search_path')
        local_cases = [
            ['SET LOCAL search_path TO pg_catalog'],
            ['SET search_path TO pg_catalog','SET LOCAL search_path TO public'],
            ['SET LOCAL search_path TO pg_catalog','RESET search_path'],
            ['SET search_path TO pg_catalog','SET LOCAL search_path TO public','SET search_path TO public'],
            ['SET search_path TO pg_catalog','SET LOCAL search_path TO DEFAULT'],
            ["SET LOCAL search_path TO ''"],
            ['SET search_path TO pg_catalog','RESET search_path','SET LOCAL search_path TO pg_catalog']]
        for ending in ['COMMIT','ROLLBACK']:
            for settings in local_cases:
                pair('SET search_path TO public'); pair('BEGIN')
                for sql in settings:
                    pair(sql); pair('SHOW search_path'); pair('SELECT current_schema()'); baseline()
                pair('INSERT INTO public.path_doc VALUES (2,20)'); baseline()
                pair(ending); pair('SHOW search_path')
                pair('SELECT id,v FROM public.path_doc ORDER BY id'); baseline()
                pair('DELETE FROM public.path_doc WHERE id=2')
        pair('SET search_path TO public'); pair('BEGIN')
        pair('SET LOCAL search_path TO pg_catalog'); pair('INSERT INTO public.path_doc VALUES (3,30)')
        pair('SET LOCAL search_path TO NULL'); pair('SET LOCAL search_path TO public')
        pair('ROLLBACK'); pair('SHOW search_path'); pair('SELECT id,v FROM path_doc ORDER BY id'); baseline()
        for before in ['SET search_path TO pg_catalog', "SET search_path TO ''", 'SET LOCAL search_path TO pg_catalog']:
            pair('SET search_path TO public'); pair('BEGIN'); pair(before)
            pair('SHOW search_path'); pair('SELECT current_schema()')
            pair('UPDATE public.path_doc SET v=11 WHERE id=1'); baseline()
            pair('INSERT INTO public.path_doc VALUES (1,12)')
            for sql in ['SHOW search_path','SET search_path TO public','RESET search_path',
                        'SET LOCAL search_path TO public','SET search_path TO DEFAULT',
                        "SET LOCAL search_path TO ''", 'SELECT current_schema()', 'SHOW search_path']:
                pair(sql); baseline()
            if ext:
                for client in [a,r]:
                    client.send(b'S',b'')
                    assert client.exact(1) == b'Z'
                    assert struct.unpack('!I',client.exact(4))[0] == 5
                    assert client.exact(1) == b'E'
                record(dict(case_id=f'sync-preserves-failed.{before}',ready='E'))
                pair('SHOW search_path')
            pair('ROLLBACK'); pair('SHOW search_path')
            effects = pair('SELECT id,v FROM path_doc ORDER BY id'); baseline()
            for key,new in [('rows',[['1','11']]),('errors',[{'C':'22000'}]),('rows',[['1','99']])]:
                bad = copy.deepcopy(effects); bad[key] = new
                try: equal(effects,bad)
                except AssertionError: pass
                else: raise AssertionError('negative check did not detect '+key)
            record(dict(case_id=f'negative-self-checks.{ext}.{before}', value=True, sqlstate=True, side_effect_readback=True))
        pair('BEGIN'); pair('SET LOCAL search_path TO NULL'); pair('ROLLBACK'); baseline()
        print('PG11_30_MID',flush=True)
        assert sys.stdin.readline() == 'continue\n', 'MySQL checkpoint missing'
        for sql in ['DROP TABLE public.pg_namespace','DROP TABLE public.path_doc']:
            pair(sql)
        # Task 3: genuine schemas, same-name relations and restricted membership.
        other = role + '_other'
        for sql in [f'CREATE SCHEMA {role}', f'CREATE SCHEMA {other}',
                    f'CREATE TABLE {role}.path_doc (id integer)',
                    f'CREATE TABLE {other}.path_doc (id integer)',
                    f'INSERT INTO {role}.path_doc VALUES (11)',
                    f'INSERT INTO {other}.path_doc VALUES (22)',
                    f'GRANT USAGE ON SCHEMA {role} TO {role}',
                    f'GRANT USAGE ON SCHEMA {other} TO {role}',
                    f'CREATE TABLE {other}.pg_namespace (nspname text)',
                    f"INSERT INTO {other}.pg_namespace VALUES ('custom_marker')",
                    f'SET search_path TO {other},pg_catalog',
                    "SELECT nspname FROM pg_namespace WHERE nspname='custom_marker'",
                    f'SET search_path TO {other}',
                    "SELECT nspname FROM pg_namespace WHERE nspname='custom_marker'",
                    f'DROP TABLE {other}.pg_namespace']:
            pair(sql)
        for path in [f'{role},{other}', f'{other},{role}', f'missing,{role}']:
            pair('SET search_path TO '+path); pair('SELECT id FROM path_doc')
        pair(f'SELECT id FROM {other}.path_doc','b')
        pair('SELECT path_doc.id FROM path_doc')
        for sql in ['CREATE TABLE created_here (id integer)',
                    'INSERT INTO created_here VALUES (33)']:
            pair(sql)
        pair(f'SELECT id FROM {role}.created_here','b')
        pair('DROP TABLE created_here'); pair('DROP TABLE IF EXISTS created_here')
        # Link real table RowDescription OIDs to the same endpoint's catalog.
        identities = []
        for side, x, y in [('target', a,b), ('reference', r,s)]:
            ids = x.run(f"SELECT n.oid::bigint FROM pg_catalog.pg_namespace n WHERE n.nspname='{role}'",ext)
            assert ids['rows'] == y.run(f"SELECT n.oid::bigint FROM pg_catalog.pg_namespace n WHERE n.nspname='{role}'",ext)['rows']
            assert len(ids['rows']) == 1 and int(ids['rows'][0][0]) > 0
            table = x.run(f'SELECT id FROM {role}.path_doc',ext)
            oid = x.run(f"SELECT c.oid::bigint FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='{role}' AND c.relname='path_doc'",ext)
            assert int(oid['rows'][0][0]) == table['origins'][0][0] > 0
            assert table['origins'][0][1] == 1
            record(dict(case_id=f'identity.{ext}',side=side,namespace=ids,table=table,catalog=oid))
            identities.append(ids['rows'])
        pair("SELECT n.nspname,c.relname FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.relname='path_doc' ORDER BY n.nspname")
        pair('SHOW search_path','b'); pair('SELECT id FROM path_doc','b')
        # Native target SELECT permission is granted through MySQL by Rust.
        for schema in [role,other]:
            grant = r.run(f'GRANT SELECT ON {schema}.path_doc TO {role}')
            assert not grant['errors'], grant
        for sql in [f'SET ROLE {role}', f'SET search_path TO "$user",{other}',
                    'SELECT current_schema()', 'SELECT id FROM path_doc',
                    'CREATE TABLE permission_probe (id integer)']:
            pair(sql)
        pair(f'REVOKE USAGE ON SCHEMA {role} FROM {role}','b')
        for sql in ['SELECT current_schema()', 'SELECT id FROM path_doc',
                    f'SELECT id FROM {role}.path_doc', f'SELECT id FROM {role}.absent']:
            pair(sql)
        pair(f'REVOKE USAGE ON SCHEMA {other} FROM {role}','b')
        for sql in ['SELECT current_schema()', 'CREATE TABLE should_not_exist (id integer)',
                    'SELECT id FROM path_doc', 'SET search_path TO']:
            pair(sql)
        pair(f'GRANT USAGE ON SCHEMA {role} TO {role}','b')
        pair('SELECT id FROM path_doc'); pair('SET ROLE NONE')
        for sql in [f'DROP TABLE {role}.path_doc', f'DROP TABLE {other}.path_doc',
                    f'DROP SCHEMA {role}', f'DROP SCHEMA {other}']:
            pair(sql)
        pair('SELECT id FROM public.should_not_exist','b')
        pair(f'CREATE SCHEMA {role}','b')
        for index, client in enumerate([b,s]):
            new = client.run(f"SELECT n.oid::bigint FROM pg_catalog.pg_namespace n WHERE n.nspname='{role}'",ext)
            assert new['rows'] != identities[index]
            record(dict(case_id=f'identity-recreated.{ext}', side=['target','reference'][index], observation=new))
        pair(f'DROP SCHEMA {role}','b')
        for client in clients: client.close()
        # A fresh libpq connection must have independent startup state.
        a,r = Client(target),Client(ref); connections.extend([a,r])
        pair('SHOW search_path'); pair('SELECT 1')
        a.close(); r.close()
    print(f'pg11_30: {count} paired inputs; 18 negative checks passed',flush=True)
finally:
    for client in connections:
        if client.conn:
            client.run('ROLLBACK')
            client.close()
    if created:
        result = admin.run('DROP DATABASE '+database)
        assert not result['errors'], 'reference cleanup failed'
    if role_created:
        result = admin.run('DROP ROLE '+role)
        assert not result['errors'], 'reference role cleanup failed'
    admin.close()
    output.close()

# Reached only after finally completed successfully; no success on cleanup error.
(archive / 'summary.json').write_text(json.dumps(dict(
    passed=True, paired_inputs=count, protocols=2, role_rounds=2,
    startup_defaults=6, negative_checks=18,
    cleanup=dict(reference_database=True, reference_role=True, pg_connections=True))))
