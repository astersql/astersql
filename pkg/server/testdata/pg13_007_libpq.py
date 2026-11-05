# Copyright 2026 AsterSQL.
"""Relation kinds, persistent owners and unsupported ancillary projections contract; independent libpq against fixed PostgreSQL 18.6."""
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
        for fmt in ((0,) if any(t[0] in (1034,2205) for t in expected) else (0, 1)):
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
                            elif field['oid'] in (20, 26, 2205):
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
    ref_admin=connect(reference_dsn.encode())
    assert ref_admin and connected(ref_admin)==0, 'reference unavailable'
    connections.append(ref_admin)
    assert serverversion(ref_admin)==libversion()==180006
    record.append(dict(version=run(ref_admin,'SELECT version()'),server_version_num=180006,libpq=libversion()))
    for role in (owner,reader):
        run(ref_admin,f"CREATE ROLE {role} LOGIN PASSWORD '{passwords[role]}'",1,log=False)
        created_roles.append(role)
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database}',1);created_dbs[0].append(database)
    admins=[open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'CREATE DATABASE {b}',1);created_dbs[1].append(b)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]
    for conn in admins:
        for sql in ('CREATE TABLE public.rel_t(id integer)', 'CREATE INDEX rel_ix ON public.rel_t(id)',
                    'CREATE VIEW public.rel_v AS SELECT id FROM public.rel_t',
                    'CREATE SEQUENCE public.rel_s START WITH 10'):
            run(conn,sql,1)
    qr="SELECT c.oid,c.relname,c.relkind,c.relowner,pg_catalog.pg_get_userbyid(c.relowner) AS owner FROM pg_catalog.pg_class c WHERE c.relname IN ('rel_t','rel_ix','rel_v','rel_s') ORDER BY c.relname"
    expected=[(26,4,-1),(19,64,-1),(18,1,-1),(26,4,-1),(19,64,-1)]
    before=contract_pair(qr,expected,False)
    for side,out in enumerate(before):
        assert [(text(r)[1],text(r)[2]) for r in out['rows']]==[('rel_ix','i'),('rel_s','S'),('rel_t','r'),('rel_v','v')],out
        assert all(text(r)[3] is not None and text(r)[4]=='root' for r in out['rows']),out
        assert run(others[side],qr)['rows']==out['rows']
        assert run(cross[side],qr)['rows']==[]
    contract_pair(qr+' LIMIT 0',expected)
    contract_pair("SELECT NULL::oid AS owner",[(26,4,-1)])
    owner_sql="SELECT c.relowner FROM pg_catalog.pg_class c WHERE c.relname='rel_t'"
    for conn in others:
        assert observe(prepare(conn,b'owner_before',owner_sql.encode(),0,None))['status']==1
    for conn in admins:run(conn,f'ALTER TABLE public.rel_t OWNER TO {owner}',1)
    after=contract_pair(qr,expected,False)
    for side,out in enumerate(after):
        assert [text(r)[0] for r in out['rows']]==[text(r)[0] for r in before[side]['rows']]
        assert [text(r)[4] for r in out['rows']]==[owner,'root',owner,'root'],out
        assert run(others[side],qr)['rows']==out['rows']
        refreshed=observe(prepared(others[side],b'owner_before',0,None,None,None,0))
        record.append(dict(side=side,sql=owner_sql,phase='Parse before ALTER / new Execute after ALTER',result=refreshed,transaction=transaction(others[side])))
        assert refreshed['status']==2 and refreshed['rows']==[[out['rows'][2][3]]],refreshed
        low=open_conn(side,a,reader)
        assert run(low,qr)['rows']==out['rows']
        denied=run(low,'SELECT id FROM public.rel_t',7)
        assert denied['errors']['C']=='42501',denied
        denied=run(low,f'ALTER TABLE public.rel_t OWNER TO {reader}',7)
        assert denied['errors']['C']=='42501',denied
        invalid=run(admins[side],f'ALTER TABLE public.rel_t OWNER TO {base}_absent',7)
        assert invalid['errors']['C']=='42704',invalid
        run(low,'SELECT 1259::oid AS recovered')
    count+=9
    # Product deliberately does not open these fields. The reference observation
    # remains raw; its successful projection is never relabelled as rejection.
    for field in ('xmin','relam','relpartbound','reltablespace','relacl','reloptions','relpersistence','relispartition'):
        sql=f"SELECT c.{field} FROM pg_catalog.pg_class c WHERE c.relname='rel_t'"
        run(admins[0],sql)
        err=run(admins[1],sql,7);assert err['errors']['C']=='0A000',err
        err=observe(prepare(admins[1],f'gate_{field}'.encode(),sql.encode(),0,None))
        record.append(dict(side=1,sql=sql,phase='Parse/Sync capability gate',result=err,transaction=transaction(admins[1])))
        assert err['status']==7 and err['errors']['C']=='0A000' and transaction(admins[1])==0,err
        run(admins[1],"SELECT c.relname FROM pg_catalog.pg_class c WHERE c.relname='rel_t'")
        err=run(admins[1],sql+' LIMIT 0',7);assert err['errors']['C']=='0A000'
        count+=1
    for sql,expected_status,tx in [('BEGIN',1,2),("SELECT relam FROM pg_catalog.pg_class",7,3),('SELECT 1259::oid',7,3),('ROLLBACK',1,0)]:
        out=observe(execute(admins[1],sql.encode()))
        record.append(dict(side=1,sql=sql,phase='Q transaction capability gate/recovery',result=out,transaction=transaction(admins[1])))
        assert out['status']==expected_status and transaction(admins[1])==tx,(sql,out,transaction(admins[1]))
    for side,conn in enumerate(admins):
        sql="SELECT c.relname,c.relkind,c.relowner FROM pg_catalog.pg_class c WHERE c.relname=$1::text"
        assert observe(prepare(conn,b'parameter_kind',sql.encode(),1,(c.c_uint*1)(25)))['status']==1
        for param in ('rel_t','rel_missing'):
            out=observe(prepared(conn,b'parameter_kind',1,(s*1)(param.encode()),None,None,0))
            record.append(dict(side=side,sql=sql,parameters=[param],phase='Bind/Execute/Sync',result=out,transaction=transaction(conn)))
            assert out['status']==2 and transaction(conn)==0,out
            assert len(out['rows'])==(0 if param=='rel_missing' else 1),out
    count+=2
    # PG partition DDL remains a closed product branch, before metadata effects.
    for sql in ('CREATE TABLE public.rel_p(id integer) PARTITION BY RANGE(id)',
                'CREATE TABLE public.rel_p0 PARTITION OF public.rel_p FOR VALUES FROM (0) TO (10)'):
        run(admins[0],sql,1)
        err=run(admins[1],sql,7);assert err['errors']['C']=='0A000',err
        absent=run(admins[1],"SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relname IN ('rel_p','rel_p0')")
        assert absent['rows']==[],absent
        count+=1
    run(admins[0],'CREATE INDEX rel_p_ix ON public.rel_p(id)',1)
    raw=run(admins[0],"SELECT c.relname,c.relkind FROM pg_catalog.pg_class c WHERE c.relname IN ('rel_p','rel_p0','rel_p_ix') ORDER BY c.relname")
    assert [text(r) for r in raw['rows']]==[['rel_p','p'],['rel_p0','r'],['rel_p_ix','I']],raw
    count+=1
    print(f'pg13_007 differential: scenarios={count} PostgreSQL=180006 libpq={libversion()} KIND OWNER ancillary gates recovery cleanup',flush=True)

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
