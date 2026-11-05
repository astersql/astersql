# Copyright 2026 AsterSQL.
"""Constraint validation and immediate execution contract; independent libpq against fixed PostgreSQL 18.6."""
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
    for conn in cross: run(conn,'CREATE TABLE public.state_doc (v integer)',1)
    for conn in admins:
        record.append(dict(side=connection_sides[conn],full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
        run(conn,'CREATE TABLE public.parent (v integer PRIMARY KEY)',1)
        run(conn,'CREATE TABLE public.state_doc (v integer NOT NULL, p integer, CONSTRAINT state_u UNIQUE(v), CONSTRAINT state_c CHECK (v > 0), CONSTRAINT state_fk FOREIGN KEY(p) REFERENCES public.parent(v))',1)
    sql="SELECT conname,condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE conrelid='public.state_doc'::regclass::oid ORDER BY conname"
    types=[(19,64,-1)]+[(16,1,-1)]*4
    got=contract_pair(sql,types)
    assert [text(r) for r in got[0]['rows']]==[[n,'f','f','t','t'] for n in ('state_c','state_doc_v_not_null','state_fk','state_u')],got
    contract_pair(sql.replace("conrelid='public.state_doc'::regclass::oid",'oid=0'),types)
    extra="SELECT conname,conislocal,coninhcount,conparentid,contypid,conperiod,connoinherit,conexclop,confdelsetcols,conpfeqop,conppeqop,conffeqop FROM pg_catalog.pg_constraint WHERE conrelid='public.state_doc'::regclass::oid ORDER BY conname"
    contract_pair(extra,[(19,64,-1),(16,1,-1),(21,2,-1),(26,4,-1),(26,4,-1),(16,1,-1),(16,1,-1),(1028,-1,-1),(1005,-1,-1)]+[(1028,-1,-1)]*3)
    contract_pair("SELECT oid,oprname,oprleft,oprright,oprresult,oprnamespace FROM pg_catalog.pg_operator WHERE oid IN (94,96,410) ORDER BY oid",[(26,4,-1),(19,64,-1)]+[(26,4,-1)]*4)
    contract_pair("SELECT condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE contype='p' AND conrelid='public.parent'::regclass::oid",[(16,1,-1)]*4)
    for conn in admins:
        run(conn,'CREATE TABLE public.pair_parent (a integer,b integer,CONSTRAINT pair_pk PRIMARY KEY(a,b))',1)
        run(conn,'CREATE TABLE public.pair_child (x integer,y integer,CONSTRAINT pair_fk FOREIGN KEY(y,x) REFERENCES public.pair_parent(a,b))',1)
    pair=contract_pair("SELECT conpfeqop,conppeqop,conffeqop FROM pg_catalog.pg_constraint WHERE conname='pair_fk'",[(1028,-1,-1)]*3)
    assert [text(r) for r in pair[0]['rows']]==[['{96,96}']*3],pair
    contract_pair("SELECT op.oid,op.oprleft,op.oprright FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_operator op ON op.oid=c.conpfeqop[1] WHERE c.conname='pair_fk'",[(26,4,-1)]*3)
    for side,conn in enumerate(admins):
        assert run(others[side],sql)['rows']==run(conn,sql)['rows']
        assert not run(cross[side],sql)['rows']
        # Real authentication, independent catalog visibility and relation denial.
        run(conn,f'GRANT CONNECT ON DATABASE {a} TO {reader}',1)
        run(conn,f'GRANT USAGE ON SCHEMA public TO {reader}',1)
        low=open_conn(side,a,reader)
        assert run(low,sql)['rows']==run(conn,sql)['rows']
        bad=run(low,'SELECT * FROM public.state_doc',7);assert bad['errors']['C']=='42501',bad
        run(conn,'INSERT INTO public.parent VALUES (1)',1)
        assert run(conn,'INSERT INTO public.parent VALUES (1)',7)['errors']['C']=='23505'
        step(conn,'BEGIN',1,2)
        step(conn,'INSERT INTO public.state_doc VALUES (1,1)',1,2)
        step(conn,'COMMIT',1,0)
        for q,state in [('INSERT INTO public.state_doc VALUES (1,1)','23505'),('INSERT INTO public.state_doc VALUES (0,1)','23514'),('INSERT INTO public.state_doc VALUES (NULL,1)','23502'),('INSERT INTO public.state_doc VALUES (2,999)','23503')]:
            step(conn,'BEGIN',1,2);step(conn,q,7,3,state)
            step(conn,'SELECT 1',7,3,'25P02');step(conn,'ROLLBACK',1,0)
            assert len(run(conn,'SELECT v,p FROM public.state_doc')['rows'])==1
        # NOT VALID remains enforced on the reference, including history scans.
        run(conn,'CREATE TABLE public.con_validate (n integer)',1)
        run(conn,'INSERT INTO public.con_validate VALUES (-1)',1)
        add='ALTER TABLE public.con_validate ADD CONSTRAINT con_cv CHECK (n>0) NOT VALID'
        analysis=observe(prepare(conn,b'not_valid_analysis',add.encode(),0,None))
        record.append(dict(side=side,sql=add,phase='Parse capability boundary',result=analysis,transaction=transaction(conn)))
        assert analysis['status']==(1 if side==0 else 7),analysis
        if side==1: assert analysis['errors']['C']=='0A000',analysis
        out=run(conn,add,1 if side==0 else 7)
        state_sql="SELECT condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE conrelid='public.con_validate'::regclass::oid"
        if side==0:
            assert [text(r) for r in run(conn,state_sql)['rows']]==[['f','f','f','t']]
            assert run(conn,'INSERT INTO public.con_validate VALUES (-2)',7)['errors']['C']=='23514'
            assert run(conn,'ALTER TABLE public.con_validate VALIDATE CONSTRAINT con_cv',7)['errors']['C']=='23514'
            assert [text(r) for r in run(conn,state_sql)['rows']]==[['f','f','f','t']]
            run(conn,'DELETE FROM public.con_validate WHERE n<0',1)
            run(conn,'ALTER TABLE public.con_validate VALIDATE CONSTRAINT con_cv',1)
            assert [text(r) for r in run(conn,state_sql)['rows']]==[['f','f','t','t']]
            definition=text(run(conn,"SELECT pg_catalog.pg_get_constraintdef(oid) FROM pg_catalog.pg_constraint WHERE conname='con_cv'")['rows'][0])[0]
            assert 'NOT VALID' not in definition
        else:
            assert out['errors']['C']=='0A000',out
            assert run(conn,'ALTER TABLE public.con_validate VALIDATE CONSTRAINT con_cv',7)['errors']['C']=='0A000'
            assert not run(conn,state_sql)['rows']
            assert [text(r) for r in run(conn,'SELECT n FROM public.con_validate')['rows']]==[['-1']]
        # Initial deferred mode is not a substitute for deferrability.
        deferred='CREATE TABLE public.con_defer (v integer, CONSTRAINT con_du UNIQUE(v) DEFERRABLE INITIALLY DEFERRED)'
        out=run(conn,deferred,1 if side==0 else 7)
        if side==0:
            assert [text(r) for r in run(conn,"SELECT condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE conname='con_du'")['rows']]==[['t','t','t','t']]
            step(conn,'BEGIN',1,2);step(conn,'INSERT INTO public.con_defer VALUES (1),(1)',1,2)
            step(conn,'COMMIT',7,0,'23505')
            assert not run(conn,'SELECT v FROM public.con_defer')['rows']
        else:
            assert out['errors']['C']=='0A000',out
            assert run(conn,"SELECT pg_catalog.to_regclass('public.con_defer')")['rows']==[[None]]
        invalid='CREATE TABLE public.invalid_initial (v integer, CONSTRAINT bad_du UNIQUE(v) NOT DEFERRABLE INITIALLY DEFERRED)'
        bad=run(conn,invalid,7);assert bad['errors']['C']=='42601',bad
        assert run(conn,"SELECT pg_catalog.to_regclass('public.invalid_initial')")['rows']==[[None]]
        run(conn,'SELECT 1')
        count+=12
    for conn in admins:
        run(conn,'CREATE TABLE public.unenforced (v integer, CONSTRAINT state_off CHECK(v>0) NOT ENFORCED)',1)
        run(conn,'INSERT INTO public.unenforced VALUES (-1)',1)
    off=contract_pair("SELECT condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE conname='state_off'",[(16,1,-1)]*4)
    assert [text(r) for r in off[0]['rows']]==[['f','f','f','f']],off
    # NULL-aware and OID parameter binding to the state projection, both formats.
    for side,conn in enumerate(admins):
        oid=text(run(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE conname='state_c'")['rows'][0])[0]
        q='SELECT condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE oid=$1'
        out=observe(prepare(conn,b'state_param',q.encode(),1,(c.c_uint*1)(26)));assert out['status']==1,out
        desc=observe(describe(conn,b'state_param'));check_metadata(desc,[(16,1,-1)]*4)
        record.append(dict(side=side,phase='Parse/Describe statement',sql=q,result=desc))
        for arg in (oid,'0',None):
            for input_format in (0,1):
                raw=(arg.encode() if input_format==0 else int(arg).to_bytes(4,'big')) if arg is not None else None
                for fmt in (0,1):
                    out=observe(prepared(conn,b'state_param',1,(s*1)(raw),(i*1)(len(raw) if raw is not None else 0),(i*1)(input_format),fmt))
                    record.append(dict(side=side,phase='Bind/Execute/Sync',sql=q,parameters=[dict(oid=26,format=input_format,value=arg)],format=fmt,result=out,transaction=transaction(conn)))
                    assert out['status']==2 and transaction(conn)==0,out
                    assert out['rows']==([[('66' if fmt==0 else '00')]*2+[('74' if fmt==0 else '01')]*2] if arg==oid else []),out
        count+=12
    print(f'pg13_021 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} STATE EXTRA VALIDATE-gate DEFER-gate ACL isolation recovery cleanup',flush=True)

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
    (evidence/'state-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
