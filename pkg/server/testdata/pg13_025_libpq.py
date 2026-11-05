# Copyright 2026 AsterSQL.
"""Bound view introspection contract; independent libpq against fixed PostgreSQL 18.6."""
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
a, b = base, base + '_b'
reader = base + '_reader'
native_dsn = f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname={a}'
reference_dsn = os.environ['PG18_REFERENCE_DSN']
passwords = {reader: secrets.token_hex(24)}

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


def remember(name):
    for side,conn in enumerate(admins):
        oid=int(text(run(conn,f"SELECT oid FROM pg_class WHERE relname='{name}'")['rows'][0])[0])
        origin_identities[side][oid]=name


def definition(conn,name,pretty=False):
    sql=f"SELECT pg_get_viewdef(c.oid,{str(pretty).lower()}) AS definition FROM pg_class c WHERE c.relname='{name}'"
    out=run(conn,sql)
    check_metadata(out,[(25,-1,-1)])
    assert len(out['rows'])==1 and out['rows'][0][0] is not None,out
    return text(out['rows'][0])[0]


def oid_parameters(name):
    global count
    sql="SELECT pg_get_viewdef(c.oid) AS definition FROM pg_class c WHERE c.oid=$1"
    for side,conn in enumerate(admins):
        oid=int(text(run(conn,f"SELECT oid FROM pg_class WHERE relname='{name}'")['rows'][0])[0])
        for fmt in (0,1):
            statement=f'view_oid_{count}_{fmt}'.encode()
            types=(c.c_uint*1)(26)
            got=observe(prepare(conn,statement,sql.encode(),1,types))
            assert got['status']==1,got
            desc=observe(describe(conn,statement))
            record.append(dict(side=side,sql=sql,phase='Describe statement',result=desc))
            check_metadata(desc,[(25,-1,-1)])
            assert desc['parameter_oids']==[26],desc
            for argument in (oid,0,None):
                data=(str(argument).encode() if fmt==0 else struct.pack('!I',argument)) if argument is not None else None
                values=(s*1)(data); sizes=(i*1)(len(data) if data is not None else 0); formats=(i*1)(fmt)
                out=observe(prepared(conn,statement,1,values,sizes,formats,fmt))
                record.append(dict(side=side,sql=sql,phase='Bind Execute Sync',parameter_oids=[26],parameters=[None if data is None else data.hex()],parameter_format=fmt,result_format=fmt,result=out,transaction=transaction(conn)))
                assert out['status']==2 and transaction(conn)==0,out
                check_metadata(out,[(25,-1,-1)])
                assert len(out['rows'])==(1 if argument==oid else 0),out
                if argument==oid:
                    assert text(out['rows'][0])[0]==definition(conn,name),out
                count+=1

try:
    ref_admin=connect(reference_dsn.encode())
    assert ref_admin and connected(ref_admin)==0,'reference unavailable'
    connections.append(ref_admin)
    assert serverversion(ref_admin)==libversion()==180006
    record.append(dict(version=run(ref_admin,'SELECT version()'),server_version_num=180006,libpq=libversion()))
    for role in (reader,):
        run(ref_admin,f"CREATE ROLE {role} LOGIN PASSWORD '{passwords[role]}'",1,log=False);created_roles.append(role)
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database}',1);created_dbs[0].append(database)
    admins=[open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'CREATE DATABASE {b}',1);created_dbs[1].append(b)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]
    for conn in admins:
        record.append(dict(side=connection_sides[conn],full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
        run(conn,'CREATE TABLE public.view_base(id integer,label text)',1)
        run(conn,"INSERT INTO public.view_base VALUES(1,'AS WITH LOCAL CHECK OPTION'),(2,NULL)",1)
    cases=[('ViewAS','CREATE VIEW public."ViewAS" ("ID","Label") AS /* AS ignored */ SELECT id AS "ID",label AS "Label" FROM public.view_base WHERE id>0 WITH LOCAL CHECK OPTION',[(23,4,-1),(25,-1,-1)]),
           ('as',"CREATE VIEW public.\"as\" AS SELECT 'AS WITH CASCADED CHECK OPTION' AS \"aS\"",[(25,-1,-1)]),
           ('view_cascade','CREATE VIEW public.view_cascade AS SELECT id FROM public.view_base WHERE id>0 WITH CASCADED CHECK OPTION',[(23,4,-1)]),
           ('view_default_check','CREATE VIEW public.view_default_check AS SELECT id FROM public.view_base WHERE id>0 WITH CHECK OPTION',[(23,4,-1)]),
           ('view_empty','CREATE VIEW public.view_empty AS SELECT id,label FROM public.view_base WHERE id<0',[(23,4,-1),(25,-1,-1)]),
           ('view_null','CREATE VIEW public.view_null AS SELECT NULL AS n',[(25,-1,-1)]),
           ('view_quoted',"CREATE VIEW public.view_quoted AS SELECT 'literal ''AS''' AS \"AS\"\"Q\"",[(25,-1,-1)])]
    for number,(name,ddl,metadata) in enumerate(cases):
        for conn in admins: run(conn,ddl,1)
        remember(name)
        pair=contract_pair(f'SELECT * FROM public."{name}"',metadata)
        for side,conn in enumerate(admins):
            # Each endpoint renders its own definition; equivalence is execution,
            # full column descriptions and values, never deparser whitespace.
            for pretty in (False,True):
                source=definition(conn,name,pretty)
                clone=f'clone_{number}_{int(pretty)}'
                run(conn,f'CREATE VIEW public.{clone} AS '+source,1)
                cloned=run(conn,f'SELECT * FROM public.{clone}')
                check_metadata(cloned,metadata)
                assert cloned['rows']==pair[side]['rows'],(source,cloned,pair[side])
                assert [f['name'] for f in cloned['fields']]==[f['name'] for f in pair[side]['fields']],cloned
                assert run(others[side],f'SELECT * FROM public.{clone}')['rows']==cloned['rows']
                run(conn,f'DROP VIEW public.{clone}',1)
            assert definition(conn,name)==definition(others[side],name)
            count+=2
    for side,conn in enumerate(others):
        sql="SELECT pg_get_viewdef(c.oid) FROM pg_class c WHERE c.relname='ViewAS'"
        assert observe(prepare(conn,b'before_rename',sql.encode(),0,None))['status']==1
    oid_parameters('ViewAS')
    contract_pair('SELECT pg_get_viewdef(NULL::oid),pg_get_viewdef(0::oid)',[(25,-1,-1)]*2)
    contract_pair("SELECT pg_get_viewdef(c.oid) FROM pg_class c WHERE c.oid=0",[(25,-1,-1)])
    # True schema shadowing, including a clone made while the hostile path is active.
    for side,conn in enumerate(admins):
        run(conn,'CREATE SCHEMA view_shadow',1)
        run(conn,'CREATE TABLE view_shadow.view_base(id integer,label text)',1)
        run(conn,"INSERT INTO view_shadow.view_base VALUES(99,'shadow')",1)
        run(conn,'SET search_path TO view_shadow,public,pg_catalog',1)
        source=definition(conn,'ViewAS')
        run(conn,'CREATE VIEW public.path_clone AS '+source,1)
        original=run(conn,'SELECT * FROM public."ViewAS"')
        cloned=run(conn,'SELECT * FROM public.path_clone')
        assert cloned['rows']==original['rows'] and text(cloned['rows'][0])==['1','AS WITH LOCAL CHECK OPTION'],(source,cloned)
        assert cloned['fields'][0]['oid']==23 and len(cloned['rows'])==2,cloned
        run(conn,'SET search_path TO public,pg_catalog',1)
        # Binding retains the original column identity when its spelling changes.
        run(conn,'ALTER TABLE public.view_base RENAME COLUMN label TO renamed_label',1)
        after=definition(conn,'ViewAS')
        assert 'renamed_label' in after,after
        assert run(conn,'SELECT * FROM public."ViewAS"')['rows']==original['rows']
        assert run(others[side],'SELECT * FROM public."ViewAS"')['rows']==original['rows']
        got=observe(prepared(others[side],b'before_rename',0,None,None,None,0))
        record.append(dict(side=side,phase='Parse before rename Execute after rename',result=got))
        assert got['status']==2 and text(got['rows'][0])[0]==after,got
        # A real low-privilege authenticated connection can introspect but not read.
        run(conn,f'GRANT CONNECT ON DATABASE {a} TO {reader}',1)
        low=open_conn(side,a,reader)
        assert definition(low,'ViewAS')==after
        assert run(low,'SELECT * FROM public."ViewAS"',7)['errors']['C']=='42501'
        run(cross[side],'CREATE VIEW public."ViewAS" AS SELECT 9 AS "ID"',1)
        assert definition(cross[side],'ViewAS')!=after
        step(conn,'BEGIN',1,2)
        failed=step(conn,'SELECT pg_get_viewdef(0,true,false)',7,3)
        assert not failed['rows']
        step(conn,'ROLLBACK',1,0)
        assert run(conn,'SELECT 1')['rows']==[['31']]
        count+=5
    # PG rejects an unknown function; an existing native definition must not leak
    # unvalidated dialect SQL through introspection.
    reference=run(admins[0],"CREATE VIEW public.bad_native AS SELECT IFNULL(renamed_label,'x') FROM public.view_base",7)
    native=run(admins[1],"SELECT pg_get_viewdef(c.oid) FROM pg_class c WHERE c.relname='view_native'",7)
    assert native['errors']['C']=='0A000' and not native['rows'],native
    assert reference['errors']['C']=='42883',reference
    for conn in admins: assert run(conn,'SELECT 1')['rows']==[['31']]
    count+=1
    print(f'pg13_025 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} BASE LEX PATH REJECT NULL text binary portals authentication isolation recovery cleanup',flush=True)

finally:
    cleanup=[]
    for conn in reversed(connections[1:]): finish(conn)
    if connections:
        for database in reversed(created_dbs[0]):
            cleanup.append((database,observe(execute(connections[0],f'DROP DATABASE {database}'.encode()))))
        for role in reversed(created_roles):
            cleanup.append((role,observe(execute(connections[0],f'DROP ROLE {role}'.encode()))))
        finish(connections[0])
    if created_dbs[1]:
        conn=connect(native_dsn.encode())
        if conn and connected(conn)==0:
            for database in reversed(created_dbs[1]):
                cleanup.append((database,observe(execute(conn,f'DROP DATABASE {database}'.encode()))))
            finish(conn)
    cleanup_failed=any(out['status']!=1 for _,out in cleanup)
    record.append(dict(cleanup=cleanup,connections_closed=True,cleanup_failed=cleanup_failed))
    evidence=Path(os.environ['PG13_EVIDENCE_DIR']);evidence.mkdir(parents=True,exist_ok=True)
    (evidence/'view-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed,'fixture cleanup failed; see evidence'
