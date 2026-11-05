# Copyright 2026 AsterSQL.
"""Database owner and policy contract; independent libpq against fixed PostgreSQL 18.6."""
import ctypes as c
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import struct
import sys

pq = c.CDLL(os.environ['PG_LIBPQ_LIBRARY'])
p, s, i = c.c_void_p, c.c_char_p, c.c_int

def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn

connect = api('PQconnectdb', p, s)
params_connect = api('PQconnectdbParams', p, c.POINTER(s), c.POINTER(s), i)
connected = api('PQstatus', i, p)
finish = api('PQfinish', None, p)
execute = api('PQexec', p, p, s)
prepare = api('PQprepare', p, p, s, s, i, c.POINTER(c.c_uint))
describe = api('PQdescribePrepared', p, p, s)
prepared = api('PQexecPrepared', p, p, s, i, c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
status = api('PQresultStatus', i, p)
ef = api('PQresultErrorField', s, p, i)
rows = api('PQntuples', i, p)
cols = api('PQnfields', i, p)
value = api('PQgetvalue', p, p, i, i)
length = api('PQgetlength', i, p, i, i)
null = api('PQgetisnull', i, p, i, i)
clear = api('PQclear', None, p)
libversion = api('PQlibVersion', i)
serverversion = api('PQserverVersion', i, p)
transaction = api('PQtransactionStatus', i, p)
actual_db = api('PQdb',s,p)
actual_user = api('PQuser',s,p)
command = api('PQcmdStatus', s, p)
error_message = api('PQerrorMessage',s,p)
field_api = {n: api(f, t, p, i) for n, f, t in [
    ('name', 'PQfname', s), ('oid', 'PQftype', c.c_uint), ('size', 'PQfsize', i),
    ('mod', 'PQfmod', i), ('format', 'PQfformat', i), ('table', 'PQftable', c.c_uint),
    ('attribute', 'PQftablecol', i)]}
fd = api('PQsocket', i, p)
record = []
connections = []
created_dbs = [[], []]
created_roles = []
admins = []
count = 0
base = sys.argv[2]
assert base.replace('_', '').isalnum()
a, b, fresh = base, base + '_b', base + '_fresh'
owner, reader = base + '_owner', base + '_reader'
native_dsn = f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname={a}'
reference_dsn = os.environ['PG18_REFERENCE_DSN']
passwords = {owner: secrets.token_hex(24), reader: secrets.token_hex(24)}

def interrupted(signum, _frame):
    raise SystemExit(128 + signum)
signal.signal(signal.SIGTERM, interrupted)
signal.signal(signal.SIGINT, interrupted)


def observe(result):
    assert result, 'libpq returned no result'
    try:
        fields = []
        for n in range(cols(result)):
            f = {k: fn(result, n) for k, fn in field_api.items()}
            f['name'] = f['name'].decode()
            fields.append(f)
        return dict(status=status(result), errors={chr(k): (ef(result, k) or b'').decode()
                    for k in map(ord, 'SVCMDHP')}, fields=fields,
                    rows=[[None if null(result, r, n) else c.string_at(value(result, r, n), length(result, r, n)).hex()
                           for n in range(cols(result))] for r in range(rows(result))],
                    command=(command(result) or b'').decode())
    finally:
        clear(result)


def run(conn, sql, expected=2, log=True):
    out = observe(execute(conn, sql.encode()))
    if log:
        record.append(dict(sql=sql, parameters=[], protocol='Q', result=out, transaction=transaction(conn)))
    assert out['status'] == expected, (sql if log else 'credential setup', out)
    assert transaction(conn) == 0, (sql, transaction(conn))
    return out


def open_conn(side, database, user='root', expect=True):
    # Credentials are held only in memory, never emitted in evidence or process arguments.
    dsn = reference_dsn if side == 0 else native_dsn
    keys = (s * 5)(b'dbname', b'dbname', b'user', b'password', None)
    vals = (s * 5)(dsn.encode(), database.encode(), user.encode(),
                    passwords.get(user, '').encode() if side == 0 else b'', None)
    conn = params_connect(keys, vals, 1)
    ok = bool(conn) and connected(conn) == 0
    if not expect:
        assert not ok, ('unexpected connection success', side, database, user)
        diagnostic = (error_message(conn) or b'').decode() if conn else 'libpq allocation failed'
        if conn:
            finish(conn)
        record.append(dict(side=side, database=database, user=user, phase='startup', rejected=True,
                           diagnostic=diagnostic, structured_fields='not exposed by failed libpq connection'))
        return None
    assert ok, ('connection failed', side, database, user)
    connections.append(conn)
    record.append(dict(side=side, phase='startup', database=actual_db(conn).decode(),user=actual_user(conn).decode(),authenticated=True))
    return conn


def check_metadata(out, expected):
    assert [(f['oid'], f['size'], f['mod']) for f in out['fields']] == expected, out


def contract_pair(sql, expected, compare_rows=True):
    global count
    pair = []
    for side, conn in enumerate(admins):
        out = run(conn, sql)
        check_metadata(out, expected)
        pair.append(out)
        for fmt in (0, 1):
            name = f'pg13_{count}_{fmt}'.encode()
            assert observe(prepare(conn, name, sql.encode(), 0, None))['status'] == 1
            desc = observe(describe(conn, name))
            record.append(dict(side=side, sql=sql, phase='Describe statement', result=desc))
            assert desc['fields'] == out['fields'], (sql, desc, out)
            got = observe(prepared(conn, name, 0, None, None, None, fmt))
            record.append(dict(side=side, sql=sql, parameters=[], phase='Execute/Sync', format=fmt,
                               result=got, transaction=transaction(conn)))
            assert got['status'] == 2 and transaction(conn) == 0, got
            assert [dict(f, format=0) for f in got['fields']] == out['fields'], got
            assert len(got['rows']) == len(out['rows']), got
            if fmt == 0:
                assert got == out, (got, out)
            else:
                def decoded(rows, fields):
                    values = []
                    for row in rows:
                        decoded_row = []
                        for cell, field in zip(row, fields):
                            if cell is None:
                                decoded_row.append(None)
                            elif field['oid'] in (20, 26):
                                raw = bytes.fromhex(cell)
                                assert len(raw) == (8 if field['oid'] == 20 else 4)
                                decoded_row.append(str(int.from_bytes(raw,'big',signed=field['oid']==20)))
                            elif field['oid'] == 16:
                                assert cell in ('00','01')
                                decoded_row.append('t' if cell == '01' else 'f')
                            else:
                                decoded_row.append(bytes.fromhex(cell).decode())
                        values.append(decoded_row)
                    return values
                assert decoded(got['rows'],got['fields']) == [text(r) for r in out['rows']], (got,out)
    assert pair[0]['fields'] == pair[1]['fields'], (sql, pair)
    assert pair[0]['command'] == pair[1]['command'], (sql, pair)
    if compare_rows:
        assert pair[0]['rows'] == pair[1]['rows'], (sql, pair)
    count += 1
    return pair


def text(row):
    return [None if v is None else bytes.fromhex(v).decode() for v in row]


class Wire:
    def __init__(self, conn):
        self.socket = socket.socket(fileno=os.dup(fd(conn)))
        self.socket.settimeout(15)
    def send(self, tag, body=b''):
        self.socket.sendall(tag + struct.pack('!I', len(body) + 4) + body)
    def exact(self, n):
        out = b''
        while len(out) < n:
            part = self.socket.recv(n-len(out))
            assert part, 'wire closed'
            out += part
        return out
    def described(self):
        messages = []
        while True:
            tag = self.exact(1)
            body = self.exact(struct.unpack('!I', self.exact(4))[0] - 4)
            messages.append((tag.decode(),body.hex()))
            assert tag != b'E', messages
            if tag == b'T':
                return messages
    def ready(self):
        messages = []
        while True:
            tag = self.exact(1)
            body = self.exact(struct.unpack('!I', self.exact(4))[0] - 4)
            messages.append((tag.decode(), body.hex()))
            if tag == b'Z':
                assert body == b'I'
                return messages
    def close(self):
        self.socket.close()


try:
    ref_admin = connect(reference_dsn.encode())
    assert ref_admin and connected(ref_admin) == 0, 'reference unavailable'
    connections.append(ref_admin)
    assert serverversion(ref_admin) == libversion() == 180006, 'fixed PostgreSQL/libpq 18.6 required'
    record.append(dict(version=run(ref_admin,'SELECT version()'),server_version_num=180006,libpq=libversion()))
    for role in (owner,reader):
        run(ref_admin,f"CREATE ROLE {role} LOGIN PASSWORD '{passwords[role]}'",1,log=False)
        created_roles.append(role)
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database} OWNER {owner}',1)
        created_dbs[0].append(database)
    admins = [open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'ALTER DATABASE {a} OWNER TO {owner}',1)
    run(admins[1],f'CREATE DATABASE {b}',1)
    created_dbs[1].append(b)
    run(admins[1],f'ALTER DATABASE {b} OWNER TO {owner}',1)
    sql = f"SELECT datdba, datistemplate, datallowconn FROM pg_catalog.pg_database WHERE datname = '{b}'"
    expected = [(26,4,-1),(16,1,-1),(16,1,-1)]
    initial = contract_pair(sql,expected,False)
    owners = [text(out['rows'][0])[0] for out in initial]
    owner_sql = f"SELECT N.datdba, pg_catalog.pg_get_userbyid(N.datdba) AS owner FROM pg_catalog.pg_database N WHERE N.datname = '{b}'"
    owner_pair = contract_pair(owner_sql,[(26,4,-1),(19,64,-1)],False)
    for side,out in enumerate(owner_pair):
        assert text(out['rows'][0]) == [owners[side],owner]
    for side,conn in enumerate(admins):
        record.append(dict(side=side,path=run(conn,'SHOW search_path'),identity=run(conn,'SELECT current_database(), current_user FROM pg_catalog.pg_database LIMIT 1')))
        for user in (owner,reader):
            other = open_conn(side,a,user)
            assert run(other,sql)['rows'] == initial[side]['rows']
            assert text(run(other,f"SELECT pg_get_userbyid(datdba) FROM pg_catalog.pg_database WHERE datname = '{b}'")['rows'][0]) == [owner]
        role = run(conn,f"SELECT oid FROM pg_catalog.pg_authid WHERE rolname = '{owner}'")
        check_metadata(role,[(26,4,-1)])
        assert role['fields'][0]['table']==1260 and role['fields'][0]['attribute']==1
        assert text(role['rows'][0]) == [owners[side]]
        restricted=run(open_conn(side,a,reader),'SELECT oid FROM pg_catalog.pg_authid',7)
        assert restricted['errors']['C']=='42501'
        run(conn,f'REVOKE CONNECT ON DATABASE {b} FROM PUBLIC',1)
        run(conn,f'REVOKE CONNECT ON DATABASE {b} FROM {reader}',1)
        open_conn(side,b,reader,False)
        assert run(conn,sql)['rows'] == initial[side]['rows'], 'ACL must not change global policy'
        open_conn(side,b,owner)
    count += 1
    retained = [open_conn(side,b,owner) for side in (0,1)]
    for clause,values in [('ALLOW_CONNECTIONS false',['f','f']),('IS_TEMPLATE true',['t','f']),('ALLOW_CONNECTIONS true',['t','t']),('IS_TEMPLATE false',['f','t'])]:
        for side,conn in enumerate(admins):
            run(conn,f'ALTER DATABASE {b} {clause}',1)
            out = run(conn,sql)
            assert text(out['rows'][0]) == [owners[side]]+values
            assert run(retained[side],sql)==out, 'existing connection remains usable'
            for user in (owner,reader):
                observer = open_conn(side,a,user)
                assert run(observer,sql) == out
                open_conn(side,b,user,values[1]=='t' and user==owner)
        contract_pair(sql,expected,False)
    count += 1
    # The management database and live sessions are unaffected by changes to b.
    for side,conn in enumerate(admins):
        assert text(run(conn,f"SELECT datistemplate, datallowconn FROM pg_catalog.pg_database WHERE datname = '{a}'")['rows'][0]) == ['f','t']
        owner_conn = open_conn(side,a,owner)
        run(owner_conn,f'ALTER DATABASE {b} IS_TEMPLATE true',1)
        assert text(run(conn,sql)['rows'][0])[1:] == ['t','t']
        run(owner_conn,f'ALTER DATABASE {b} IS_TEMPLATE false',1)
        current = run(conn,f'ALTER DATABASE {a} ALLOW_CONNECTIONS false',7)
        assert current['errors']['C']=='22023',current
        denied = run(open_conn(side,a,reader),f'ALTER DATABASE {b} IS_TEMPLATE true',7)
        assert denied['errors']['C']=='42501',denied
        invalid = run(conn,f'ALTER DATABASE {b} ALLOW_CONNECTIONS nonsense',7)
        assert invalid['errors']['C']==('42601' if side==0 else '0A000'),invalid
        assert run(conn,sql)['rows']==initial[side]['rows']
    contract_pair(sql+' LIMIT 0',expected)
    null_sql = f"SELECT D.description FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_shdescription D ON D.objoid = N.oid AND D.classoid = 1262 WHERE N.datname = '{b}'"
    assert contract_pair(null_sql,[(25,-1,-1)])[0]['rows']==[[None]]
    for side,conn in enumerate(admins):
        wire=Wire(conn)
        try:
            wire.send(b'P',b'pg13_bad\0SELECT N.unknown FROM pg_catalog.pg_database N\0\0\0')
            wire.send(b'S')
            result=wire.ready()
            record.append(dict(side=side,phase='Parse/Sync reject',sql='SELECT N.unknown FROM pg_catalog.pg_database N',result=result))
            assert ''.join(t for t,_ in result)=='EZ',result
        finally:
            wire.close()
        bad=run(conn,'SELECT N.unknown FROM pg_catalog.pg_database N',7)
        assert bad['errors']['C']==('42703' if side==0 else '0A000')
        assert text(run(conn,'SELECT 1 AS ok')['rows'][0])==['1']
        # Capture ReadyForQuery-equivalent I/T/E through libpq's transaction state.
        for statement,expected_status,txstate in [('BEGIN',1,2),('SELECT N.unknown FROM pg_catalog.pg_database N',7,3),('SELECT 1 AS ok',7,3),('ROLLBACK',1,0)]:
            out=observe(execute(conn,statement.encode()))
            record.append(dict(side=side,sql=statement,phase='Q transaction recovery',result=out,transaction=transaction(conn)))
            assert out['status']==expected_status and transaction(conn)==txstate,(statement,out,transaction(conn))
            if statement=='SELECT 1 AS ok':
                assert out['errors']['C']=='25P02'
        assert text(run(conn,'SELECT 1 AS ok')['rows'][0])==['1']
    count+=1
    print(f'pg13_002 differential: scenarios={count} PostgreSQL=180006 libpq={libversion()} OWNER POLICY CONNECT NULL EMPTY REJECT recovery verified',flush=True)

finally:
    # Attempt both sides' teardown and preserve diagnostics even after a failure.
    cleanup = []
    cleanup_failed = False
    for conn in reversed(connections[1:]):
        finish(conn)
    if connections:
        admin = connections[0]
        for database in reversed(created_dbs[0]):
            out = observe(execute(admin, f'DROP DATABASE {database}'.encode()))
            cleanup.append((database, out))
        for role in reversed(created_roles):
            out = observe(execute(admin, f'DROP ROLE {role}'.encode()))
            cleanup.append((role,out))
        finish(admin)
    if created_dbs[1]:
        admin = connect(native_dsn.encode())
        if admin and connected(admin) == 0:
            try:
                for database in reversed(created_dbs[1]):
                    out = observe(execute(admin,f'DROP DATABASE {database}'.encode()))
                    cleanup.append((database,out))
            finally:
                finish(admin)
        else:
            cleanup_failed = True
            if admin:
                finish(admin)
    cleanup_failed |= any(out['status'] != 1 for _,out in cleanup)
    record.append(dict(cleanup=cleanup,connections_closed=True,cleanup_failed=cleanup_failed))
    evidence = Path(os.environ['PG13_EVIDENCE_DIR'])
    evidence.mkdir(parents=True,exist_ok=True)
    (evidence/'database.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
