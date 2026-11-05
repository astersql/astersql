# Copyright 2026 AsterSQL.
"""Database-scoped namespace contract; independent libpq against fixed PostgreSQL 18.6."""
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
    sql="SELECT oid,nspname FROM pg_catalog.pg_namespace WHERE nspname IN ('public','pg_catalog','ns_user','ns_private','ns_renamed') ORDER BY nspname"
    desc=[(26,4,-1),(19,64,-1)]
    initial=contract_pair(sql,desc,False)
    for side,conn in enumerate(admins):
        assert [text(r)[1] for r in initial[side]['rows']]==['pg_catalog','public']
        assert run(others[side],sql)['rows']==initial[side]['rows']
        for path in ('pg_catalog',"''"):
            run(conn,f'SET search_path TO {path}',1)
            assert run(conn,sql)['rows']==initial[side]['rows']
        run(conn,'RESET search_path',1)
    contract_pair("SELECT oid,nspname FROM pg_catalog.pg_namespace WHERE nspname='ns_missing'",desc)
    contract_pair("SELECT oid,nspname FROM pg_catalog.pg_namespace LIMIT 0",desc)
    for side,conn in enumerate(admins):
        parameter_sql="SELECT oid,nspname FROM pg_catalog.pg_namespace WHERE nspname=$1::text ORDER BY oid"
        types=(c.c_uint*1)(25)
        out=observe(prepare(conn,b'param_ns',parameter_sql.encode(),1,types))
        assert out['status']==1,out
        for parameter in (b'public',b'ns_missing'):
            values=(s*1)(parameter)
            out=observe(prepared(conn,b'param_ns',1,values,None,None,0))
            record.append(dict(side=side,sql=parameter_sql,parameters=[parameter.decode()],format=0,phase='Bind Execute Sync',result=out))
            check_metadata(out,desc)
            assert len(out['rows'])==(1 if parameter==b'public' else 0),out
        wire=Wire(conn)
        try:
            wire.send(b'B',b'ns_portal\0param_ns\0'+struct.pack('!hh',0,1)+struct.pack('!i',6)+b'public'+struct.pack('!h',0))
            wire.send(b'D',b'Pns_portal\0')
            wire.send(b'H')
            described=wire.described()
            body=bytes.fromhex(described[-1][1])
            assert struct.unpack('!h',body[:2])[0]==2
            fields=[]
            offset=2
            for _ in range(2):
                end=body.index(0,offset)
                name=body[offset:end].decode()
                table,attribute,oid,size,mod,fmt=struct.unpack('!IhIhih',body[end+1:end+19])
                fields.append(dict(name=name,table=table,attribute=attribute,oid=oid,size=size,mod=mod,format=fmt))
                offset=end+19
            assert [(f['oid'],f['size'],f['mod']) for f in fields]==desc
            assert [(f['table'],f['attribute']) for f in fields]==[(2615,1),(2615,2)]
            wire.send(b'E',b'ns_portal\0'+struct.pack('!I',0))
            wire.send(b'S')
            messages=wire.ready()
            assert ''.join(tag for tag,_ in messages)=='DCZ',messages
            record.append(dict(side=side,sql=parameter_sql,parameters=['public'],phase='Describe portal / Execute / Sync',fields=fields,result=messages))
            wire.send(b'C',b'Pns_portal\0')
            wire.send(b'C',b'Sparam_ns\0')
            wire.send(b'S')
            assert ''.join(tag for tag,_ in wire.ready())=='33Z'
        finally:
            wire.close()
    count+=2
    # Parse before the second connection creates the schema. Bind a fresh portal each time.
    for side,conn in enumerate(admins):
        assert observe(prepare(conn,b'fresh_ns',sql.encode(),0,None))['status']==1
        run(others[side],f'CREATE SCHEMA ns_user AUTHORIZATION {owner}',1)
        run(cross[side],f'CREATE SCHEMA ns_user AUTHORIZATION {owner}',1)
        out=observe(prepared(conn,b'fresh_ns',0,None,None,None,0))
        record.append(dict(side=side,phase='Parse before CREATE / new Execute',sql=sql,result=out))
        assert len(out['rows'])==3,out
    created=contract_pair(sql,desc,False)
    ids=[]
    for side,out in enumerate(created):
        ids.append(next(text(r)[0] for r in out['rows'] if text(r)[1]=='ns_user'))
    for side,conn in enumerate(admins):
        run(others[side],'ALTER SCHEMA ns_user RENAME TO ns_renamed',1)
        out=observe(prepared(conn,b'fresh_ns',0,None,None,None,0))
        record.append(dict(side=side,phase='new Execute after RENAME',result=out))
        assert next(text(r)[0] for r in out['rows'] if text(r)[1]=='ns_renamed')==ids[side]
        assert any(text(r)[1]=='ns_user' for r in run(cross[side],sql)['rows'])
        run(others[side],'DROP SCHEMA ns_renamed',1)
        assert run(conn,sql)['rows']==initial[side]['rows']
        run(others[side],f'CREATE SCHEMA ns_user AUTHORIZATION {owner}',1)
        new=run(conn,"SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='ns_user'")
        assert text(new['rows'][0])[0]!=ids[side]
        run(conn,'CREATE TABLE public.ns_same(id integer)',1)
        run(conn,'CREATE TABLE ns_user.ns_same(id integer)',1)
        duplicate=run(conn,f'CREATE SCHEMA ns_user AUTHORIZATION {owner}',7)
        assert duplicate['errors']['C']=='42P06',duplicate
    join="SELECT n.oid AS namespace_oid,n.nspname,c.relname,c.relnamespace FROM pg_catalog.pg_namespace n JOIN pg_catalog.pg_class c ON c.relnamespace=n.oid WHERE c.relname='ns_same' ORDER BY n.nspname"
    pair=contract_pair(join,[(26,4,-1),(19,64,-1),(19,64,-1),(26,4,-1)],False)
    for out in pair:
        assert len(out['rows'])==2 and all(text(r)[0]==text(r)[3] for r in out['rows'])
    owner_sql="SELECT oid,nspowner FROM pg_catalog.pg_namespace WHERE nspname='ns_user'"
    pair=contract_pair(owner_sql,[(26,4,-1),(26,4,-1)],False)
    for side,conn in enumerate(admins):
        assert run(readers[side],owner_sql)==pair[side]
        run(conn,f'ALTER SCHEMA ns_user OWNER TO {reader}',1)
        updated=run(conn,owner_sql)
        assert text(updated['rows'][0])[0]==text(pair[side]['rows'][0])[0]
        assert text(updated['rows'][0])[1]!=text(pair[side]['rows'][0])[1]
        assert run(readers[side],owner_sql)==updated
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
    for side,conn in enumerate(admins):
        begin=observe(execute(conn,b'BEGIN'))
        assert begin['status']==1 and transaction(conn)==2
        statement='ALTER SCHEMA ns_user RENAME TO ns_transaction_name'
        out=observe(execute(conn,statement.encode()))
        record.append(dict(side=side,sql=statement,phase='transaction DDL policy',result=out,transaction=transaction(conn)))
        assert out['status']==(1 if side==0 else 7),out
        if side==1:
            assert out['errors']['C']=='0A000' and transaction(conn)==3,out
        rollback=observe(execute(conn,b'ROLLBACK'))
        assert rollback['status']==1 and transaction(conn)==0
        assert len(run(conn,"SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='ns_user'")['rows'])==1
    count+=1
    # ACL arrays are compared semantically: quoting and element order are not identity.
    def acl_entries(out):
        import csv
        raw=text(out['rows'][0])[0]
        return sorted(next(csv.reader([raw[1:-1]],escapechar='\\')))
    acls=[run(conn,acl_sql) for conn in admins]
    # Identifier quoting may differ; remove identifier quotes after array parsing.
    assert [x.replace('"','') for x in acl_entries(acls[0])]==[x.replace('"','') for x in acl_entries(acls[1])],acls
    count+=7
    for side,conn in enumerate(admins):
        for statement,expected_status,txstate in [('BEGIN',1,2),('SELECT unknown FROM pg_catalog.pg_namespace',7,3),('SELECT 1 AS ok',7,3),('ROLLBACK',1,0)]:
            out=observe(execute(conn,statement.encode()))
            record.append(dict(side=side,sql=statement,result=out,transaction=transaction(conn)))
            assert out['status']==expected_status and transaction(conn)==txstate,(statement,out,transaction(conn))
        for statement in ('DROP TABLE public.ns_same','DROP TABLE ns_user.ns_same','DROP TABLE ns_private.ns_secret','DROP SCHEMA ns_user','DROP SCHEMA ns_private'):
            run(conn,statement,1)
        run(cross[side],'DROP SCHEMA ns_user',1)
        assert run(conn,sql)['rows']==initial[side]['rows']
    print(f'pg13_003 differential: scenarios={count} PostgreSQL=180006 libpq={libversion()} BASE EMPTY JOIN FRESH DB OWNER ACL recovery cleanup',flush=True)

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
