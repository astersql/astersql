# Copyright 2026 AsterSQL.
"""Default object identity and normalized presence contract; independent libpq against fixed PostgreSQL 18.6."""
import ctypes as c
import json
import re
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
    if expected==1 and sql.startswith(('ALTER INDEX ','DROP INDEX ')):
        assert out['command']==' '.join(sql.split()[:2]),out
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
                            elif field['oid'] in (20, 21, 23, 26, 2205):
                                raw = bytes.fromhex(cell)
                                assert len(raw) == (8 if field['oid'] == 20 else 2 if field['oid'] == 21 else 4)
                                decoded_row.append(str(int.from_bytes(raw,'big',signed=field['oid'] in (20,21,23))))
                            elif field['oid'] in (22,30,1005,1028):
                                raw=bytes.fromhex(cell)
                                ndim,hasnull,element,n,lower=struct.unpack('!iiIii',raw[:20])
                                assert (ndim,hasnull,element,lower)==(1,0,21 if field['oid'] in (22,1005) else 26,1 if field['oid'] in (1005,1028) else 0)
                                offset=20;items=[]
                                for _ in range(n):
                                    size=struct.unpack('!i',raw[offset:offset+4])[0];offset+=4
                                    assert size==(2 if field['oid'] in (22,1005) else 4)
                                    items.append(str(int.from_bytes(raw[offset:offset+size],'big',signed=field['oid'] in (22,1005))));offset+=size
                                assert offset==len(raw)
                                decoded_row.append('{'+','.join(items)+'}' if field['oid'] in (1005,1028) else ' '.join(items))
                            elif field['oid'] == 18 and cell=='00':
                                decoded_row.append('')
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
            wire.send(b'B',b'decl_portal\0'+statement+b'\0'+struct.pack('!hhh',0,0,0))
            wire.send(b'D',b'Pdecl_portal\0')
            wire.send(b'H')
            messages=wire.described()
            body=bytes.fromhex(messages[-1][1]); offset=2; fields=[]
            for _ in range(struct.unpack('!h',body[:2])[0]):
                end=body.index(0,offset)
                table,attribute,oid,size,mod,fmt=struct.unpack('!IhIhih',body[end+1:end+19])
                fields.append(dict(name=body[offset:end].decode(),table=table,attribute=attribute,oid=oid,size=size,mod=mod,format=fmt))
                offset=end+19
            assert fields==pair[side]['fields'],(fields,pair[side])
            wire.send(b'E',b'decl_portal\0'+struct.pack('!I',0))
            wire.send(b'S')
            result=wire.ready()
            assert sum(t=='D' for t,_ in result)==len(pair[side]['rows']),result
            assert all(t!='E' for t,_ in result),result
            record.append(dict(side=side,sql=sql,parameters=[],phase='Describe portal / Execute / Sync',fields=fields,result=result))
            wire.send(b'C',b'Pdecl_portal\0'); wire.send(b'C',b'S'+statement+b'\0');wire.send(b'S')
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


def step(conn, sql, expected_status, tx, state=None):
    out=observe(execute(conn,sql.encode()))
    record.append(dict(side=connection_sides[conn],sql=sql,parameters=[],phase='Q execute/commit/recovery',result=out,transaction=transaction(conn)))
    assert out['status']==expected_status and transaction(conn)==tx,(sql,out,transaction(conn))
    if state: assert out['errors']['C']==state,(sql,out)
    return out

try:
    ref_admin=connect(reference_dsn.encode())
    assert ref_admin and connected(ref_admin)==0,'reference unavailable'
    connections.append(ref_admin)
    assert serverversion(ref_admin)==libversion()==180006
    record.append(dict(version=run(ref_admin,'SELECT version()'),server_version_num=180006,libpq=libversion()))
    for role in (owner,reader):
        run(ref_admin,f"CREATE ROLE {role} LOGIN PASSWORD '{passwords[role]}'",1,log=False);created_roles.append(role)
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database}',1);created_dbs[0].append(database)
    admins=[open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'CREATE DATABASE {b}',1);created_dbs[1].append(b)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]

    for conn in cross: run(conn,"CREATE TABLE public.def_doc (qty integer DEFAULT 99)",1)
    for conn in admins:
        record.append(dict(side=connection_sides[conn],full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
        run(conn,"CREATE TABLE public.def_doc (plain integer,n integer DEFAULT NULL,literal text DEFAULT 'NULL',qty integer DEFAULT 7,label text DEFAULT 'O''Reilly',flag boolean DEFAULT true)",1)
    sql="SELECT a.attnum,a.attname,a.atthasdef,d.oid,d.adrelid,d.adnum FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid='public.def_doc'::regclass::oid AND a.attnum>0 ORDER BY a.attnum"
    types=[(21,2,-1),(19,64,-1),(16,1,-1),(26,4,-1),(26,4,-1),(21,2,-1)]
    pair=contract_pair(sql,types,False)
    baseline=[]
    for side,out in enumerate(pair):
        data=[text(r) for r in out['rows']]
        assert len(data)==6 and data[0][2:]==['f',None,None,None] and data[1][2:]==['f',None,None,None],data
        ids=[r[3] for r in data[2:]]
        assert len(set(ids))==4 and all(int(v)>0 for v in ids),data
        relation=text(run(admins[side],"SELECT oid FROM pg_catalog.pg_class WHERE relname='def_doc'")['rows'][0])[0]
        assert all(r[4]==relation and r[5]==r[0] and r[3]!=relation and r[2]=='t' for r in data[2:]),data
        assert run(others[side],sql)['rows']==out['rows']
        run(admins[side],'INSERT INTO public.def_doc DEFAULT VALUES',1)
        initial=run(admins[side],'SELECT plain,n,literal,qty,label,flag FROM public.def_doc')
        assert [text(r) for r in initial['rows']]==[[None,None,'NULL','7',"O'Reilly",'t']],initial
        run(admins[side],'DELETE FROM public.def_doc',1)
        parameter_sql='SELECT oid,adrelid,adnum FROM pg_catalog.pg_attrdef WHERE oid=$1'
        assert observe(prepare(admins[side],b'default_oid',parameter_sql.encode(),1,(c.c_uint*1)(26)))['status']==1
        parameter_desc=observe(describe(admins[side],b'default_oid'))
        check_metadata(parameter_desc,[(26,4,-1),(26,4,-1),(21,2,-1)])
        record.append(dict(side=side,sql=parameter_sql,phase='Describe statement',result=parameter_desc))
        for input_format in (0,1):
            for arg in (data[3][3],'0',None):
                raw=(arg.encode() if input_format==0 else struct.pack('!I',int(arg))) if arg is not None else None
                for fmt in (0,1):
                    result=observe(prepared(admins[side],b'default_oid',1,(s*1)(raw),(i*1)(len(raw) if raw is not None else 0),(i*1)(input_format),fmt))
                    record.append(dict(side=side,sql=parameter_sql,phase='Bind Execute Sync',parameters=[dict(oid=26,format=input_format,value=arg)],format=fmt,result=result,transaction=transaction(admins[side])))
                    assert result['status']==2 and transaction(admins[side])==0,result
                    assert len(result['rows'])==(1 if arg==data[3][3] else 0),result
                    if result['rows']:
                        got=[str(int.from_bytes(bytes.fromhex(v),'big')) for v in result['rows'][0]] if fmt else text(result['rows'][0])
                        assert got==data[3][3:],(got,data[3])
        count+=14
        baseline.append(data)
        # Prepared before DDL; first execution must see the new definition.
        assert observe(prepare(others[side],b'before_ddl',sql.encode(),0,None))['status']==1
    empty="SELECT oid,adrelid,adnum FROM pg_catalog.pg_attrdef WHERE adrelid=0"
    contract_pair(empty,[(26,4,-1),(26,4,-1),(21,2,-1)])
    for side,conn in enumerate(admins):
        run(conn,'ALTER TABLE public.def_doc ALTER COLUMN qty SET DEFAULT 9',1)
        out=observe(prepared(others[side],b'before_ddl',0,None,None,None,0))
        record.append(dict(side=side,phase='Parse before DDL Execute after DDL',sql=sql,result=out))
        assert out['status']==2 and text(out['rows'][3])[3]!=baseline[side][3][3],out
        run(conn,'ALTER TABLE public.def_doc ALTER COLUMN label DROP DEFAULT',1)
        run(conn,'ALTER TABLE public.def_doc RENAME COLUMN qty TO quantity',1)
        now=[text(r) for r in run(others[side],sql)['rows']]
        assert now[3][1]=='quantity' and now[3][3]==text(out['rows'][3])[3],now
        assert now[4][2:]==['f',None,None,None] and now[2][3]==baseline[side][2][3] and now[5][3]==baseline[side][5][3],now
        run(conn,'INSERT INTO public.def_doc DEFAULT VALUES',1)
        vals=run(conn,'SELECT plain,n,literal,quantity,label,flag FROM public.def_doc')
        assert [text(r) for r in vals['rows']]==[[None,None,'NULL','9',None,'t']],vals
        run(conn,f'GRANT CONNECT ON DATABASE {a} TO {reader}',1)
        run(conn,'GRANT USAGE ON SCHEMA public TO '+reader,1)
        low=open_conn(side,a,reader)
        assert run(low,sql)['rows']==run(conn,sql)['rows']
        assert run(low,'SELECT * FROM public.def_doc',7)['errors']['C']=='42501'
        assert len(run(cross[side],sql)['rows'])==1
        run(conn,'ALTER TABLE public.def_doc ALTER COLUMN quantity SET DEFAULT 9',1)
        replacement=[text(r) for r in run(conn,sql)['rows']]
        assert replacement[3][3]!=now[3][3],replacement
        # Reads are side-effect free and object identity survives repeated reads.
        assert run(conn,sql)['rows']==run(others[side],sql)['rows']
        old_relation=now[3][4]
        run(conn,'DROP TABLE public.def_doc',1)
        run(conn,'CREATE TABLE public.def_doc (qty integer DEFAULT 7)',1)
        rebuilt=[text(r) for r in run(conn,sql)['rows']]
        assert len(rebuilt)==1 and rebuilt[0][4]!=old_relation and rebuilt[0][3] not in [r[3] for r in now],rebuilt
        assert not run(conn,'SELECT oid FROM pg_catalog.pg_attrdef WHERE adrelid='+old_relation)['rows']
        count+=12
    print(f'pg13_022 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} BASE META authenticated-reader isolation prepared-refresh cleanup',flush=True)

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
    (evidence/'default-identity-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
