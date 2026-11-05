# Copyright 2026 AsterSQL.
"""Database identity contract; independent libpq against fixed PostgreSQL 18.6."""
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
    record.append(dict(version=run(ref_admin, 'SELECT version()'), server_version_num=180006, libpq=libversion()))
    for role in (owner, reader):
        run(ref_admin, f"CREATE ROLE {role} LOGIN PASSWORD '{passwords[role]}'", 1, log=False)
        created_roles.append(role)
    for database in (a, b):
        run(ref_admin, f'CREATE DATABASE {database} OWNER {owner}', 1)
        created_dbs[0].append(database)
    admins = [open_conn(0, a), open_conn(1, a)]
    for side,conn in enumerate(admins):
        record.append(dict(side=side,phase='session identity',path=run(conn,'SHOW search_path'),
                           identity=run(conn,'SELECT current_database(), current_user FROM pg_catalog.pg_database LIMIT 1')))
    # Explicit migration of this disposable legacy fixture, with a real native owner account.
    run(admins[1], f'ALTER DATABASE {a} OWNER TO {owner}', 1)
    run(admins[1], f'CREATE DATABASE {b}', 1)
    created_dbs[1].append(b)
    new_owner = run(admins[1], f"SELECT pg_get_userbyid(datdba) FROM pg_catalog.pg_database WHERE datname = '{b}'")
    assert text(new_owner['rows'][0]) == ['root'], new_owner
    run(admins[1], f'ALTER DATABASE {b} OWNER TO {owner}', 1)
    # A disposable store may contain bootstrap databases; migration is a fixture map,
    # never automatic adoption by catalog reads or a production migration default.
    names = run(admins[1], 'SELECT datname FROM pg_catalog.pg_database')['rows']
    for row in names:
        name = text(row)[0]
        if name not in (a, b):
            assert name in ('test', 'mysql', 'sys', 'information_schema', 'performance_schema', 'metrics_schema'), name
            run(admins[1], f'ALTER DATABASE {name} OWNER TO root', 1)
        open_conn(1,name)
    base_sql = f"SELECT N.oid, N.datname, N.datdba, N.datistemplate, N.datallowconn FROM pg_catalog.pg_database N WHERE N.datname IN ('{a}', '{b}') ORDER BY N.datname"
    expected = [(26, 4, -1), (19, 64, -1), (26, 4, -1), (16, 1, -1), (16, 1, -1)]
    pair = contract_pair(base_sql, expected, False)
    identities = []
    for side, out in enumerate(pair):
        observed = [text(r) for r in out['rows']]
        assert len(observed) == 2 and all(r[3:] == ['f', 't'] for r in observed), out
        assert observed[0][2] == observed[1][2] and all(r[2] is not None for r in observed)
        identities.append({r[1]: r[0] for r in observed})
        assert run(open_conn(side, a), base_sql) == out
    for sql in [base_sql.replace(f"IN ('{a}', '{b}')", "= 'pg13_absent_never_created'"), base_sql + ' LIMIT 0']:
        result = contract_pair(sql, expected)
        assert not result[0]['rows'] and result[0]['command'] == 'SELECT 0'
    owner_sql = f"SELECT pg_get_userbyid(datdba) AS owner FROM pg_catalog.pg_database WHERE datname = '{a}'"
    contract_pair(owner_sql, [(19,64,-1)])
    for side in (0, 1):
        for user in (owner, reader):
            conn = open_conn(side, a, user)
            assert text(run(conn, owner_sql)['rows'][0]) == [owner]
        run(admins[side], f'REVOKE CONNECT ON DATABASE {b} FROM PUBLIC', 1)
        run(admins[side], f'REVOKE CONNECT ON DATABASE {b} FROM {reader}', 1)
        open_conn(side, b, reader, False)
        if side == 1:
            startup = socket.create_connection(('127.0.0.1',int(sys.argv[1])),timeout=15)
            try:
                body = struct.pack('!I',196608) + f'user\0{reader}\0database\0{b}\0\0'.encode()
                startup.sendall(struct.pack('!I',len(body)+4)+body)
                def exact(n):
                    data = b''
                    while len(data)<n:
                        part = startup.recv(n-len(data))
                        assert part
                        data += part
                    return data
                assert exact(1) == b'E'
                error = exact(struct.unpack('!I',exact(4))[0]-4)
                fields = {part[:1].decode():part[1:].decode() for part in error.split(b'\0') if part}
                assert fields['C'] == '42501',fields
                record.append(dict(side=side,phase='startup CONNECT denial',errors=fields))
            finally:
                startup.close()
        open_conn(side, b, owner)
        assert run(open_conn(side, a, reader), base_sql)['rows'] == pair[side]['rows']
        # An authenticated low role cannot migrate ownership, even for its current DB.
        denied = run(open_conn(side, a, reader), f'ALTER DATABASE {a} OWNER TO {reader}', 7)
        assert denied['errors']['C'] == '42501', denied
    list_sql = '''select N.oid::bigint as id,
       datname as name, D.description, datistemplate as is_template,
       datallowconn as allow_connections,
       pg_catalog.pg_get_userbyid(N.datdba) as "owner"
from pg_catalog.pg_database N
left join pg_catalog.pg_shdescription D on N.oid = D.objoid
order by case when datname = pg_catalog.current_database() then -1::bigint else N.oid::bigint end'''
    list_fields = []
    for side, conn in enumerate(admins):
        out = run(conn, list_sql)
        list_fields.append(out['fields'])
        statement = b'pg13_list'
        assert observe(prepare(conn,statement,list_sql.encode(),0,None))['status'] == 1
        desc = observe(describe(conn,statement))
        assert desc['fields'] == out['fields'], (desc,out)
        record.append(dict(side=side,sql=list_sql,phase='Describe statement',result=desc))
        for fmt in (0,1):
            extended = observe(prepared(conn,statement,0,None,None,None,fmt))
            record.append(dict(side=side,sql=list_sql,phase='Execute/Sync',format=fmt,result=extended,transaction=transaction(conn)))
            assert extended['status'] == 2 and transaction(conn) == 0
            assert [dict(f,format=0) for f in extended['fields']] == out['fields']
            assert len(extended['rows']) == len(out['rows'])
            if fmt == 0:
                assert extended == out
            else:
                for raw, textual in zip(extended['rows'],out['rows']):
                    assert raw[1:3] == textual[1:3] and raw[5] == textual[5]
                    assert int.from_bytes(bytes.fromhex(raw[0]),'big',signed=True) == int(text(textual)[0])
                    assert [v == '01' for v in raw[3:5]] == [v == 't' for v in text(textual)[3:5]]
        check_metadata(out, [(20,8,-1),(19,64,-1),(25,-1,-1),(16,1,-1),(16,1,-1),(19,64,-1)])
        assert text(out['rows'][0])[1] == a
        assert [text(r)[1] for r in out['rows'] if text(r)[1] in (a,b)] == [a,b]
        assert all(text(r)[2] is None and text(r)[5] == owner for r in out['rows'] if text(r)[1] in (a,b))
        assert [int(text(r)[0]) for r in out['rows'][1:]] == sorted(int(text(r)[0]) for r in out['rows'][1:])
        other = run(open_conn(side,b,owner), list_sql)
        assert text(other['rows'][0])[1] == b
    assert list_fields[0] == list_fields[1], list_fields
    count += 1
    # First Execute observes CREATE after Parse/Bind/Describe, on two live connections.
    fresh_sql = f"SELECT oid, datname FROM pg_catalog.pg_database WHERE datname = '{fresh}' ORDER BY oid"
    fresh_describe = []
    for side, conn in enumerate(admins):
        wire = Wire(conn)
        try:
            wire.send(b'P', b'pg13_fresh\0' + fresh_sql.encode() + b'\0\0\0')
            wire.send(b'B', b'pg13_portal\0pg13_fresh\0' + b'\0\0' * 3)
            wire.send(b'D', b'Ppg13_portal\0')
            # Flush, not Sync: PG drops transaction-scoped portals at idle Sync.
            wire.send(b'H')
            described = wire.described()
            assert ''.join(t for t,_ in described) == '12T', described
            fresh_describe.append(described[-1])
            run(ref_admin if side == 0 else open_conn(1,a), f'CREATE DATABASE {fresh}', 1)
            created_dbs[side].append(fresh)
            wire.send(b'E', b'pg13_portal\0' + struct.pack('!I',0))
            wire.send(b'S')
            got = wire.ready()
            assert ''.join(t for t,_ in got) == ('CZ' if side == 0 else 'DCZ'), got
            record.append(dict(side=side, sql=fresh_sql, phase='Parse/Bind/Describe then CREATE then Execute/Sync', describe=described, result=got))
        finally:
            wire.close()
        old = run(conn, fresh_sql)['rows'][0]
        other = open_conn(side,a)
        assert run(other,fresh_sql)['rows'] == [old]
        run(ref_admin if side == 0 else other, f'DROP DATABASE {fresh}',1)
        created_dbs[side].remove(fresh)
        assert not run(conn,fresh_sql)['rows'] and not run(other,fresh_sql)['rows']
        run(ref_admin if side == 0 else other, f'CREATE DATABASE {fresh}',1)
        created_dbs[side].append(fresh)
        assert run(conn,fresh_sql)['rows'][0][0] != old[0], 'recreated database reused identity'
        run(conn,'SET search_path TO pg_catalog',1)
        assert run(conn,base_sql)['rows'] == pair[side]['rows']
        assert text(run(conn,"SELECT current_database() FROM pg_catalog.pg_database LIMIT 1")['rows'][0]) == [a]
        assert run(other,'SHOW search_path')['rows'] != run(conn,'SHOW search_path')['rows']
    record.append(dict(portal_snapshot_difference='PG18.6 Bind retains old snapshot (0 rows); AsterSQL first Execute refreshes (1 row); new queries see committed CREATE on both'))
    assert fresh_describe[0] == fresh_describe[1], fresh_describe
    count += 1
    open_conn(1,a,base+'_unknown_role',False)
    for side, conn in enumerate(admins):
        wire = Wire(conn)
        try:
            wire.send(b'P', b'pg13_bad\0SELECT N.unknown FROM pg_catalog.pg_database N\0\0\0')
            wire.send(b'S')
            rejected = wire.ready()
            assert ''.join(t for t,_ in rejected) == 'EZ', rejected
            assert (b'C42703\0' if side == 0 else b'C0A000\0') in bytes.fromhex(rejected[0][1])
            record.append(dict(side=side,phase='Parse reject/Sync recovery',result=rejected))
        finally:
            wire.close()
        bad = run(conn,'SELECT N.unknown FROM pg_catalog.pg_database N',7)
        assert bad['errors']['C'] == ('42703' if side == 0 else '0A000'),bad
        assert text(run(conn,'SELECT 1 AS ok')['rows'][0]) == ['1']
        for policy in ('ALLOW_CONNECTIONS false', 'IS_TEMPLATE true'):
            run(conn,f'ALTER DATABASE {b} {policy}',1)
            run(conn,f'ALTER DATABASE {b} '+('ALLOW_CONNECTIONS true' if policy.startswith('ALLOW') else 'IS_TEMPLATE false'),1)
        assert run(conn,base_sql)['rows'] == pair[side]['rows']
        open_conn(side,base+'_absent',expect=False)
    count += 1
    print(f'pg13_001 differential: scenarios={count} PostgreSQL=180006 libpq={libversion()} BASE EMPTY LIST FRESH OWNER CONNECT POLICY REJECT verified',flush=True)
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
