# Copyright 2026 AsterSQL.
"""Foreign key order and action contract; independent libpq against fixed PostgreSQL 18.6."""
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
                            elif field['oid'] in (22,30,1005):
                                raw=bytes.fromhex(cell)
                                ndim,hasnull,element,n,lower=struct.unpack('!iiIii',raw[:20])
                                assert (ndim,hasnull,element,lower)==(1,0,21 if field['oid'] in (22,1005) else 26,1 if field['oid']==1005 else 0)
                                offset=20;items=[]
                                for _ in range(n):
                                    size=struct.unpack('!i',raw[offset:offset+4])[0];offset+=4
                                    assert size==(2 if field['oid'] in (22,1005) else 4)
                                    items.append(str(int.from_bytes(raw[offset:offset+size],'big',signed=field['oid'] in (22,1005))));offset+=size
                                assert offset==len(raw)
                                decoded_row.append('{'+','.join(items)+'}' if field['oid']==1005 else ' '.join(items))
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
    for conn in cross: run(conn,'CREATE TABLE public.con_parent (code integer)',1)
    for conn in admins:
        record.append(dict(side=connection_sides[conn],full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
        for ddl in [
            'CREATE TABLE public.con_parent (a integer,b integer,CONSTRAINT con_pk PRIMARY KEY(b,a))',
            'CREATE TABLE public.con_child (x integer,y integer,CONSTRAINT con_fk FOREIGN KEY(y,x) REFERENCES public.con_parent(a,b) ON UPDATE RESTRICT ON DELETE CASCADE)',
        ]: run(conn,ddl,1)
    sql="SELECT c.conkey,c.confkey,c.confupdtype,c.confdeltype,c.confmatchtype FROM pg_catalog.pg_constraint c WHERE c.conname='con_fk'"
    types=[(1005,-1,-1)]*2+[(18,1,-1)]*3
    got=contract_pair(sql,types)
    assert [text(r) for r in got[0]['rows']]==[['{2,1}','{1,2}','r','c','s']]
    nonfk=contract_pair("SELECT confupdtype,confdeltype,confmatchtype FROM pg_catalog.pg_constraint WHERE conname='con_pk'",[(18,1,-1)]*3)
    assert [text(r) for r in nonfk[0]['rows']]==[[' ',' ',' ']]
    join="SELECT i.indrelid=c.confrelid AS parent_index,t.relname,p.relname FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_index i ON i.indexrelid=c.conindid JOIN pg_catalog.pg_class t ON t.oid=c.conrelid JOIN pg_catalog.pg_class p ON p.oid=c.confrelid WHERE c.conname='con_fk'"
    got=contract_pair(join,[(16,1,-1),(19,64,-1),(19,64,-1)])
    assert [text(r) for r in got[0]['rows']]==[['t','con_child','con_parent']]
    assert all(not r['rows'] for r in contract_pair(sql.replace("c.conname='con_fk'",'c.oid=0'),types))
    for side,conn in enumerate(admins):
        assert run(others[side],sql)['rows']==run(conn,sql)['rows']
        assert not run(cross[side],sql)['rows']
        low=open_conn(side,a,reader)
        assert run(low,sql)['rows']==run(conn,sql)['rows']
        assert run(low,'SELECT * FROM public.con_child',7)['errors']['C']=='42501'
        for q in ['INSERT INTO public.con_parent VALUES (10,20)','INSERT INTO public.con_child VALUES (20,10)','INSERT INTO public.con_child VALUES (NULL,999)','INSERT INTO public.con_child VALUES (888,NULL)']:
            run(conn,q,1)
        for q in ['INSERT INTO public.con_child VALUES (777,666)','UPDATE public.con_parent SET a=11 WHERE a=10 AND b=20']:
            out=run(conn,q,7)
            assert out['errors']['C']==('23001' if q.startswith('UPDATE') else '23503'),out
        assert [text(r) for r in run(conn,'SELECT a,b FROM public.con_parent')['rows']]==[['10','20']]
        run(conn,'DELETE FROM public.con_parent WHERE a=10 AND b=20',1)
        assert [text(r) for r in run(conn,'SELECT x,y FROM public.con_child ORDER BY y,x')['rows']]==[[None,'999'],['888',None]]
        for action,code in [('NO ACTION','a'),('RESTRICT','r'),('SET NULL','n'),('CASCADE','c')]:
            run(conn,'DROP TABLE public.con_child',1)
            run(conn,f'CREATE TABLE public.con_child (x integer,y integer,CONSTRAINT con_fk FOREIGN KEY(y,x) REFERENCES public.con_parent(a,b) ON DELETE {action})',1)
            assert [text(r) for r in run(others[side],sql)['rows']]==[['{2,1}','{1,2}','a',code,'s']]
            run(conn,'INSERT INTO public.con_parent VALUES (10,20)',1)
            run(conn,'INSERT INTO public.con_child VALUES (20,10)',1)
            out=run(conn,'DELETE FROM public.con_parent WHERE a=10',7 if code in ('a','r') else 1)
            if code in ('a','r'):
                assert out['errors']['C']==('23001' if code=='r' else '23503'),out
                assert [text(r) for r in run(conn,'SELECT x,y FROM public.con_child')['rows']]==[['20','10']]
                run(conn,'DELETE FROM public.con_child',1)
                run(conn,'DELETE FROM public.con_parent',1)
            else:
                assert [text(r) for r in run(conn,'SELECT x,y FROM public.con_child')['rows']]==([[None,None]] if code=='n' else [])
        run(conn,'DROP TABLE public.con_child',1)
        run(conn,'CREATE TABLE public.con_child (x integer,y integer,CONSTRAINT con_fk FOREIGN KEY(y,x) REFERENCES public.con_parent(a,b) MATCH SIMPLE ON UPDATE CASCADE ON DELETE CASCADE)',1)
        run(conn,'INSERT INTO public.con_parent VALUES (10,20)',1)
        run(conn,'INSERT INTO public.con_child VALUES (20,10)',1)
        run(conn,'UPDATE public.con_parent SET a=11 WHERE a=10',1)
        assert [text(r) for r in run(conn,'SELECT x,y FROM public.con_child')['rows']]==[['20','11']]
        lookup="SELECT conkey,confkey,confmatchtype FROM pg_catalog.pg_constraint WHERE oid=$1"
        saved=text(run(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE conname='con_fk'")['rows'][0])[0].encode()
        assert observe(prepare(others[side],b'fk_lookup',lookup.encode(),1,(c.c_uint*1)(26)))['status']==1
        desc=observe(describe(others[side],b'fk_lookup'));check_metadata(desc,[(1005,-1,-1)]*2+[(18,1,-1)])
        record.append(dict(side=side,sql=lookup,phase='Parse / Describe',parameter_oids=[26],result=desc))
        for parameter,state in [(saved,None),(b'0',None),(None,None),(b'bad','22P02'),(b'4294967296','22003')]:
            for fmt in (0,1):
                out=observe(prepared(others[side],b'fk_lookup',1,(s*1)(parameter),None,None,fmt))
                record.append(dict(side=side,sql=lookup,parameters=[None if parameter is None else parameter.decode()],format=fmt,phase='Bind / Execute / Sync',result=out,transaction=transaction(others[side])))
                if state: assert out['status']==7 and out['errors']['C']==state,out
                else:
                    assert out['status']==2 and len(out['rows'])==(1 if parameter==saved else 0),out
                    assert [dict(f,format=0) for f in out['fields']]==desc['fields']
                assert transaction(others[side])==0
        for clause in ['MATCH FULL','MATCH PARTIAL','ON DELETE SET DEFAULT','ON UPDATE SET NULL','ON UPDATE SET DEFAULT']:
            q=f'CREATE TABLE public.closed_fk (x integer,y integer,FOREIGN KEY(y,x) REFERENCES public.con_parent(a,b) {clause})'
            out=observe(execute(conn,q.encode())); record.append(dict(side=side,sql=q,result=out,transaction=transaction(conn),phase='capability gate'))
            if side==1:
                assert out['status']==7 and out['errors']['C']=='0A000',out
                assert not run(conn,"SELECT oid FROM pg_catalog.pg_class WHERE relname='closed_fk'")['rows']
            elif out['status']==1: run(conn,'DROP TABLE public.closed_fk',1)
        for q,st,tx in [('BEGIN',1,2),('SELECT * FROM public.con_missing',7,3),('ROLLBACK',1,0)]:
            out=observe(execute(conn,q.encode())); record.append(dict(side=side,sql=q,result=out,transaction=transaction(conn),phase='recovery'))
            assert out['status']==st and transaction(conn)==tx,out
    print(f'pg13_019 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} CON-ORDER/JOIN/ACTION permutations NULL actions gates ACL isolation recovery cleanup',flush=True)

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
    (evidence/'foreign-key-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
