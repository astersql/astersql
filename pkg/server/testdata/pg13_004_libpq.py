# Copyright 2026 AsterSQL.
"""Namespace owner and ACL contract; independent libpq against fixed PostgreSQL 18.6."""
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
connection_sides = {}
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
        record.append(dict(side=connection_sides.get(conn,0),sql=sql, parameters=[], protocol='Q', result=out, transaction=transaction(conn)))
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
    connection_sides[conn] = side
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
        for fmt in ((0,) if any(t[0] == 1034 for t in expected) else (0, 1)):
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
    for side,conn in enumerate(admins):
        wire=Wire(conn)
        try:
            statement=f'portal_{count}'.encode()
            wire.send(b'P',statement+b'\0'+sql.encode()+b'\0'+struct.pack('!h',0))
            wire.send(b'B',b'acl_portal\0'+statement+b'\0'+struct.pack('!hhh',0,0,0))
            wire.send(b'D',b'Pacl_portal\0')
            wire.send(b'H')
            messages=wire.described()
            body=bytes.fromhex(messages[-1][1]); offset=2; fields=[]
            for _ in range(struct.unpack('!h',body[:2])[0]):
                end=body.index(0,offset)
                table,attribute,oid,size,mod,fmt=struct.unpack('!IhIhih',body[end+1:end+19])
                fields.append(dict(name=body[offset:end].decode(),table=table,attribute=attribute,oid=oid,size=size,mod=mod,format=fmt))
                offset=end+19
            assert fields==pair[side]['fields'],(fields,pair[side])
            wire.send(b'E',b'acl_portal\0'+struct.pack('!I',0))
            wire.send(b'S')
            result=wire.ready()
            assert sum(t=='D' for t,_ in result)==len(pair[side]['rows']),result
            assert all(t!='E' for t,_ in result),result
            record.append(dict(side=side,sql=sql,parameters=[],phase='Describe portal / Execute / Sync',fields=fields,result=result))
            wire.send(b'C',b'Pacl_portal\0'); wire.send(b'C',b'S'+statement+b'\0');wire.send(b'S')
            assert ''.join(t for t,_ in wire.ready())=='33Z'
        finally:
            wire.close()
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
    assert serverversion(ref_admin) == libversion() == 180006
    record.append(dict(version=run(ref_admin,'SELECT version()'),server_version_num=180006,libpq=libversion()))
    for role in (owner,reader):
        run(ref_admin,f"CREATE ROLE {role} LOGIN PASSWORD '{passwords[role]}'",1,log=False)
        created_roles.append(role)
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database}',1)
        created_dbs[0].append(database)
    admins = [open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'CREATE DATABASE {b}',1)
    created_dbs[1].append(b)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]
    readers=[open_conn(side,a,reader) for side in (0,1)]
    for side,conn in enumerate(admins):
        run(conn,f'CREATE SCHEMA ns_user AUTHORIZATION {owner}',1)
    sql="SELECT oid,nspname FROM pg_catalog.pg_namespace WHERE nspname IN ('ns_user','ns_private') ORDER BY nspname"
    desc=[(26,4,-1),(19,64,-1)]
    owner_sql="SELECT oid,nspowner FROM pg_catalog.pg_namespace WHERE nspname='ns_user'"
    pair=contract_pair(owner_sql,[(26,4,-1),(26,4,-1)],False)
    for side,conn in enumerate(admins):
        assert run(readers[side],owner_sql)==pair[side]
        role=run(conn,f"SELECT oid FROM pg_catalog.pg_authid WHERE rolname='{owner}'")
        assert text(pair[side]['rows'][0])[1]==text(role['rows'][0])[0]
        run(conn,f'ALTER SCHEMA ns_user OWNER TO {reader}',1)
        updated=run(conn,owner_sql)
        assert text(updated['rows'][0])[0]==text(pair[side]['rows'][0])[0]
        assert text(updated['rows'][0])[1]!=text(pair[side]['rows'][0])[1]
        assert run(readers[side],owner_sql)==updated
        role=run(conn,f"SELECT oid FROM pg_catalog.pg_authid WHERE rolname='{reader}'")
        assert text(updated['rows'][0])[1]==text(role['rows'][0])[0]
        run(conn,f'CREATE SCHEMA ns_private AUTHORIZATION {owner}',1)
    acl_sql="SELECT nspacl FROM pg_catalog.pg_namespace WHERE nspname='ns_private'"
    assert contract_pair(acl_sql,[(1034,-1,-1)])[0]['rows']==[[None]]
    for side,conn in enumerate(admins):
        run(conn,'REVOKE ALL ON SCHEMA ns_private FROM PUBLIC',1)
        run(conn,f'REVOKE ALL ON SCHEMA ns_private FROM {reader}',1)
        run(conn,'CREATE TABLE ns_private.ns_secret(id integer)',1)
        run(conn,'INSERT INTO ns_private.ns_secret VALUES (7)',1)
        run(conn,f'GRANT SELECT ON ns_private.ns_secret TO {reader}',1)
        before=run(readers[side],acl_sql)
        denied=run(readers[side],'SELECT id FROM ns_private.ns_secret',7)
        assert denied['errors']['C']=='42501',denied
        run(conn,f'GRANT USAGE ON SCHEMA ns_private TO {reader}',1)
        after=run(readers[side],acl_sql)
        assert after['rows']!=before['rows']
        assert text(run(readers[side],'SELECT id FROM ns_private.ns_secret')['rows'][0])==['7']
        run(conn,f'REVOKE USAGE ON SCHEMA ns_private FROM {reader}',1)
        assert run(readers[side],acl_sql)['rows']==before['rows']
        assert run(readers[side],'SELECT id FROM ns_private.ns_secret',7)['errors']['C']=='42501'
    # ACL arrays are compared semantically: quoting and element order are not identity.
    def acl_entries(out):
        import csv
        raw=text(out['rows'][0])[0]
        return sorted(next(csv.reader([raw[1:-1]],escapechar='\\')))
    acls=[run(conn,acl_sql) for conn in admins]
    # Identifier quoting may differ; remove identifier quotes after array parsing.
    assert [x.replace('"','') for x in acl_entries(acls[0])]==[x.replace('"','') for x in acl_entries(acls[1])],acls
    count+=7
    contract_pair("SELECT oid,nspowner,nspacl FROM pg_catalog.pg_namespace WHERE nspname='ns_missing'",[(26,4,-1),(26,4,-1),(1034,-1,-1)])
    contract_pair("SELECT oid,nspowner,nspacl FROM pg_catalog.pg_namespace LIMIT 0",[(26,4,-1),(26,4,-1),(1034,-1,-1)])
    for side,conn in enumerate(admins):
        for statement,expected_status,txstate in [('BEGIN',1,2),('SELECT unknown FROM pg_catalog.pg_namespace',7,3),('SELECT 1 AS ok',7,3),('ROLLBACK',1,0)]:
            out=observe(execute(conn,statement.encode()))
            record.append(dict(side=side,sql=statement,result=out,transaction=transaction(conn)))
            assert out['status']==expected_status and transaction(conn)==txstate,(statement,out,transaction(conn))
        run(conn,f'CREATE SCHEMA ns_empty AUTHORIZATION {owner}',1)
        default=run(readers[side],"SELECT nspacl FROM pg_catalog.pg_namespace WHERE nspname='ns_empty'")
        assert default['rows']==[[None]],default
        run(conn,'CREATE TABLE ns_empty.owner_probe(id integer)',1)
        run(conn,'INSERT INTO ns_empty.owner_probe VALUES (9)',1)
        run(conn,f'GRANT SELECT ON ns_empty.owner_probe TO {owner}',1)
        owners=open_conn(side,a,owner)
        run(conn,f'REVOKE ALL ON SCHEMA ns_empty FROM {owner}',1)
        empty=run(readers[side],"SELECT nspacl FROM pg_catalog.pg_namespace WHERE nspname='ns_empty'")
        assert text(empty['rows'][0])==['{}'],empty
        assert run(owners,'SELECT id FROM ns_empty.owner_probe',7)['errors']['C']=='42501'
        run(conn,f'GRANT USAGE ON SCHEMA ns_empty TO {owner}',1)
        assert text(run(owners,'SELECT id FROM ns_empty.owner_probe')['rows'][0])==['9']
        assert run(owners,'CREATE TABLE ns_empty.denied(id integer)',7)['errors']['C']=='42501'
        run(conn,'DROP TABLE ns_empty.owner_probe',1)
        for statement in ('DROP TABLE ns_private.ns_secret','DROP SCHEMA ns_user','DROP SCHEMA ns_private','DROP SCHEMA ns_empty'):
            run(conn,statement,1)
        assert run(conn,sql)['rows']==[]
    count+=2
    print(f'pg13_004 differential: scenarios={count} PostgreSQL=180006 libpq={libversion()} OWNER ACL NULL EMPTY recovery cleanup',flush=True)

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
    (evidence/'namespace.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
