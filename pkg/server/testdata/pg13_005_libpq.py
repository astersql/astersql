# Copyright 2026 AsterSQL.
"""Relation identity and foreign-key contract; independent libpq against fixed PostgreSQL 18.6."""
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
        for target in (conn,cross[side]):
            for sql in ('CREATE TABLE public.rel_t(id integer)', 'CREATE INDEX rel_ix ON public.rel_t(id)',
                        'CREATE VIEW public.rel_v AS SELECT id FROM public.rel_t','CREATE SEQUENCE public.rel_s START WITH 10'):
                run(target,sql,1)
    qr="SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='public' AND c.relname IN ('rel_t','rel_ix','rel_v','rel_s','rel_renamed') ORDER BY c.relname"
    desc=[(26,4,-1),(19,64,-1),(26,4,-1),(18,1,-1)]
    original=contract_pair(qr,desc,False)
    for side,conn in enumerate(admins):
        assert len(original[side]['rows'])==4,original[side]
        assert [text(r)[1::2] for r in original[side]['rows']]==[['rel_ix','i'],['rel_s','S'],['rel_t','r'],['rel_v','v']]
        assert run(others[side],qr)==original[side]
        assert run(readers[side],qr)==original[side]
    isolated=[run(conn,qr) for conn in cross]
    contract_pair("SELECT c.oid,c.relnamespace FROM pg_catalog.pg_class c WHERE c.oid='pg_catalog.pg_class'::regclass::oid",[(26,4,-1)]*2)
    contract_pair("SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c WHERE c.relname='rel_missing'",desc)
    contract_pair("SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c LIMIT 0",desc)
    for sql,n in [
        ("SELECT i.indrelid,i.indexrelid,t.oid,x.oid FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class t ON t.oid=i.indrelid JOIN pg_catalog.pg_class x ON x.oid=i.indexrelid WHERE t.relname='rel_t' AND x.relname='rel_ix'",4),
        ("SELECT s.seqrelid,c.oid FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid=s.seqrelid WHERE c.relname='rel_s'",2),
        ("SELECT a.attrelid,c.oid FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid WHERE c.relname='rel_t' AND a.attname='id'",2)]:
        pair=contract_pair(sql,[(26,4,-1)]*n,False)
        for out in pair:
            assert len(out['rows'])==1,out
            vals=text(out['rows'][0]);assert vals[:n//2]==vals[n//2:],out
    def identities(out):
        return {text(row)[1]:text(row)[0] for row in out['rows']}
    contract_pair("SELECT '4294967295'::oid AS maximum,NULL::oid AS absent",[(26,4,-1)]*2)
    lifecycle_sql="SELECT oid FROM pg_catalog.pg_class WHERE relname=$1::text"
    for side,conn in enumerate(admins):
        assert observe(prepare(conn,b'life',lifecycle_sql.encode(),1,(c.c_uint*1)(25)))['status']==1
        statement_desc=observe(describe(conn,b'life'))
        check_metadata(statement_desc,[(26,4,-1)])
        def life(name):
            got=observe(prepared(conn,b'life',1,(s*1)(name.encode()),None,None,0))
            record.append(dict(side=side,sql=lifecycle_sql,parameters=[name],phase='new Bind/Execute/Sync after DDL',result=got,transaction=transaction(conn)))
            assert got['status']==2 and transaction(conn)==0,got
            return got
        run(conn,'ALTER TABLE public.rel_t RENAME TO rel_renamed',1)
        renamed=run(conn,qr);old=identities(original[side]);new=identities(renamed)
        assert new['rel_renamed']==old['rel_t'] and new['rel_ix']==old['rel_ix']
        assert run(others[side],qr)==renamed
        assert text(life('rel_renamed')['rows'][0])==[old['rel_t']]
        run(conn,'DROP VIEW public.rel_v',1);run(conn,'DROP TABLE public.rel_renamed',1)
        remaining=run(conn,qr);assert identities(remaining)=={'rel_s':old['rel_s']}
        assert run(others[side],qr)==remaining
        assert life('rel_renamed')['rows']==[]
        run(conn,'CREATE TABLE public.rel_t(id integer)',1);run(conn,'CREATE INDEX rel_ix ON public.rel_t(id)',1)
        rebuilt=run(conn,qr);new=identities(rebuilt)
        assert new['rel_s']==old['rel_s'] and new['rel_t']!=old['rel_t'] and new['rel_ix']!=old['rel_ix']
        assert run(others[side],qr)==rebuilt
        assert text(life('rel_t')['rows'][0])==[new['rel_t']]
        assert run(cross[side],qr)==isolated[side]
        bad=run(conn,"SELECT '4294967296'::oid",7);assert bad['errors']['C']=='22003',bad
        for sql,expected,state in [('BEGIN',1,2),("SELECT '4294967296'::oid",7,3),('SELECT 1 AS blocked',7,3),('ROLLBACK',1,0)]:
            got=observe(execute(conn,sql.encode()))
            record.append(dict(side=side,sql=sql,phase='Q',result=got,transaction=transaction(conn)))
            assert got['status']==expected and transaction(conn)==state,(sql,got,transaction(conn))
        run(conn,'SELECT 1 AS recovered')
    count+=9
    print(f'pg13_005 differential: scenarios={count} PostgreSQL=180006 libpq={libversion()} BASE KEY LIFE RANGE recovery cleanup',flush=True)

finally:
    cleanup=[]
    for conn in reversed(connections[1:]): finish(conn)
    if connections:
        for database in reversed(created_dbs[0]):
            out=observe(execute(connections[0],f'DROP DATABASE {database}'.encode()))
            cleanup.append((database,out))
        for role in reversed(created_roles):
            cleanup.append((role,observe(execute(connections[0],f'DROP ROLE {role}'.encode()))))
        finish(connections[0])
    # Native root fixture database is owned and removed by the Rust guard.
    if created_dbs[1]:
        conn=connect(native_dsn.encode())
        if conn and connected(conn)==0:
            for database in reversed(created_dbs[1]):
                cleanup.append((database,observe(execute(conn,f'DROP DATABASE {database}'.encode()))))
            finish(conn)
    cleanup_failed=any(out['status']!=1 for _,out in cleanup)
    record.append(dict(cleanup=cleanup,connections_closed=True,cleanup_failed=cleanup_failed))
    evidence=Path(os.environ['PG13_EVIDENCE_DIR']);evidence.mkdir(parents=True,exist_ok=True)
    (evidence/'relations.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
