# Copyright 2026 AsterSQL.
"""Generated expression catalog contract; independent libpq against fixed PostgreSQL 18.6."""
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
import time

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
num_params = api('PQnparams', i, p)
param_type = api('PQparamtype', c.c_uint, p, i)
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
origin_identities = [{},{}]
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
        return dict(status=status(result), parameter_oids=[param_type(result,n) for n in range(num_params(result))], errors={chr(k): (ef(result, k) or b'').decode()
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
    def origins(fields, side):
        return [dict(f,table=f['table'] if f['table'] in (0,1249,2604) else origin_identities[side][f['table']]) for f in fields]
    assert origins(pair[0]['fields'],0) == origins(pair[1]['fields'],1), (sql, pair)
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
    for conn in admins:
        record.append(dict(side=connection_sides[conn],full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
    for mode,name,flag in [('STORED','def_doc','s'),('VIRTUAL','def_virtual','v')]:
        for conn in admins:
            run(conn,f'CREATE TABLE public.{name} (x integer,y integer GENERATED ALWAYS AS (x+1) {mode},d integer DEFAULT 7)',1)
            run(conn,f'INSERT INTO public.{name}(x) VALUES (2),(NULL)',1)
        for side,conn in enumerate(admins):
            relid=int(text(run(conn,f"SELECT oid FROM pg_class WHERE relname='{name}'")['rows'][0])[0])
            origin_identities[side][relid]=name
        contract_pair(f'SELECT x,y,d FROM public.{name} ORDER BY x',[(23,4,-1)]*3)
        sql=f"SELECT a.attnum,a.attname,a.attgenerated,a.atthasdef,d.oid,d.adrelid,d.adnum,d.adbin,pg_get_expr(d.adbin,d.adrelid) AS definition FROM pg_attribute a LEFT JOIN pg_attrdef d ON a.attrelid=d.adrelid AND a.attnum=d.adnum WHERE a.attrelid='public.{name}'::regclass::oid AND a.attnum>0 ORDER BY a.attnum"
        pair=contract_pair(sql,[(21,2,-1),(19,64,-1),(18,1,-1),(16,1,-1),(26,4,-1),(26,4,-1),(21,2,-1),(194,-1,-1),(25,-1,-1)],False)
        initial=[]
        for side,out in enumerate(pair):
            data=[text(row) for row in out['rows']]
            assert len(data)==3 and data[0][2:]==['','f']+[None]*5,data
            assert data[1][0:4]==['2','y',flag,'t'] and data[1][6]=='2',data
            assert data[2][2:4]==['','t'] and data[2][8]=='7',data
            assert int(data[1][4])>0 and data[1][4]!=data[2][4] and data[1][5]==data[2][5],data
            assert data[1][7].startswith('{OPEXPR ') and ':varattno 1 ' in data[1][7],data
            assert data[1][8].replace('"','')=='(x + 1)',data
            initial.append(data[1][4:8])
            assert run(others[side],sql)['rows']==out['rows']
            assert observe(prepare(others[side],(name+'_prepared').encode(),sql.encode(),0,None))['status']==1
            run(cross[side],f'CREATE TABLE public.{name} (z integer)',1)
            assert len(run(cross[side],sql)['rows'])==1
            assert run(cross[side],f"SELECT oid FROM pg_attrdef WHERE adrelid='public.{name}'::regclass::oid")['rows']==[]
            run(admins[side],f'GRANT CONNECT ON DATABASE {a} TO {reader}',1)
            low=open_conn(side,a,reader)
            assert run(low,sql)['rows']==out['rows']
            assert run(low,f'SELECT * FROM public.{name}',7)['errors']['C']=='42501'
        # Actual PG nodeToString must agree, including the typed VAR and operator.
        assert text(pair[0]['rows'][1])[7]==text(pair[1]['rows'][1])[7],pair
        for side,conn in enumerate(admins):
            run(conn,f'UPDATE public.{name} SET x=4 WHERE x=2',1)
            run(conn,f'ALTER TABLE public.{name} RENAME COLUMN x TO renamed_x',1)
            run(conn,f'INSERT INTO public.{name}(renamed_x) VALUES (6)',1)
            got=observe(prepared(others[side],(name+'_prepared').encode(),0,None,None,None,0))
            record.append(dict(side=side,sql=sql,phase='Parse before rename Execute after rename',result=got))
            assert got['status']==2,got
            data=[text(row) for row in got['rows']]
            assert data[1][4:8]==initial[side],data
            assert data[1][8].replace('"','')=='(renamed_x + 1)',data
            # A tree with Vars must have a real relation context.
            bad=run(conn,f"SELECT pg_get_expr(adbin,0) FROM pg_attrdef WHERE adrelid='public.{name}'::regclass::oid AND adnum=2",7)
            assert not bad['rows'] and transaction(conn)==0,bad
            assert run(conn,'SELECT 1')['rows']==[['31']]
            if mode=='STORED':
                bad=run(conn,f'INSERT INTO public.{name}(renamed_x) VALUES (2147483647)',7)
                assert bad['errors']['C']=='22003' and not bad['rows'],bad
                assert not run(conn,f'SELECT renamed_x FROM public.{name} WHERE renamed_x=2147483647')['rows']
            else:
                run(conn,f'INSERT INTO public.{name}(renamed_x) VALUES (2147483647)',1)
                assert [text(r) for r in run(conn,f'SELECT renamed_x FROM public.{name} WHERE renamed_x=2147483647')['rows']]==[['2147483647']]
                bad=run(conn,f'SELECT y FROM public.{name} WHERE renamed_x=2147483647',7)
                assert bad['errors']['C']=='22003' and not bad['rows'],bad
                run(conn,f'DELETE FROM public.{name} WHERE renamed_x=2147483647',1)
            step(conn,'BEGIN',1,2)
            step(conn,f'UPDATE public.{name} SET renamed_x=8 WHERE renamed_x=6',1,2)
            step(conn,'ROLLBACK',1,0)
            count+=3
        contract_pair(f'SELECT renamed_x,y FROM public.{name} ORDER BY renamed_x',[(23,4,-1)]*2)
        contract_pair(f"SELECT pg_get_expr(adbin,adrelid,true) FROM pg_attrdef WHERE adrelid='public.{name}'::regclass::oid AND adnum=2",[(25,-1,-1)],False)
    contract_pair("SELECT oid,adrelid,adnum,adbin FROM pg_attrdef WHERE adrelid=0",[(26,4,-1),(26,4,-1),(21,2,-1),(194,-1,-1)])
    contract_pair('SELECT pg_get_expr(NULL,0),pg_get_expr(NULL,0,true)',[(25,-1,-1)]*2)
    for side,conn in enumerate(admins):
        bad=run(conn,'CREATE TABLE public.bad_generated (x integer,y integer GENERATED ALWAYS AS (IFNULL(x,0)) STORED)',7)
        assert not bad['rows']
        assert bad['errors']['C']=='42883',bad
        assert not run(conn,"SELECT oid FROM pg_class WHERE relname='bad_generated'")['rows']
        assert run(conn,'SELECT 1')['rows']==[['31']]
        step(conn,'BEGIN',1,2)
        failed=step(conn,"SELECT pg_get_expr('fake',0)",7,3)
        assert not failed['rows']
        step(conn,'ROLLBACK',1,0)
        assert run(conn,'SELECT 1')['rows']==[['31']]
        count+=2
    print(f'pg13_024 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} modes=2 rename identity NULL text binary portals authentication isolation rollback cleanup',flush=True)

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
    (evidence/'generated-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
