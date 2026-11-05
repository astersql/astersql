# Copyright 2026 AsterSQL.
"""Index definition contract; independent libpq against fixed PostgreSQL 18.6."""
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
                            elif field['oid'] in (20, 21, 23, 26, 2205):
                                raw = bytes.fromhex(cell)
                                assert len(raw) == (8 if field['oid'] == 20 else 2 if field['oid'] == 21 else 4)
                                decoded_row.append(str(int.from_bytes(raw,'big',signed=field['oid'] in (20,21,23))))
                            elif field['oid'] == 22:
                                raw=bytes.fromhex(cell)
                                ndim,hasnull,element,n,lower=struct.unpack('!iiIii',raw[:20])
                                assert (ndim,hasnull,element,lower)==(1,0,21,0)
                                offset=20;items=[]
                                for _ in range(n):
                                    size=struct.unpack('!i',raw[offset:offset+4])[0];offset+=4
                                    assert size==2
                                    items.append(str(struct.unpack('!h',raw[offset:offset+2])[0]));offset+=2
                                assert offset==len(raw)
                                decoded_row.append(' '.join(items))
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


def canonical(value):
    # Unquoted lowercase identifiers and their quoted spelling bind identically.
    return re.sub(r'(?<!")"([a-z_][a-z_0-9]*)"(?!")',r'\1',value)

def definitions(sql, expected):
    pair=contract_pair(sql,[(25,-1,-1)]*len(expected),False)
    for out in pair:
        assert [[None if x is None else canonical(x) for x in text(r)] for r in out['rows']]==[expected],out
    return pair

try:
    ref_admin=connect(reference_dsn.encode())
    assert ref_admin and connected(ref_admin)==0,'reference unavailable'
    connections.append(ref_admin)
    assert serverversion(ref_admin)==libversion()==180006
    record.append(dict(version=run(ref_admin,'SELECT version()'),server_version_num=180006,libpq=libversion()))
    for role in (owner,reader):
        run(ref_admin,f"CREATE ROLE {role} LOGIN PASSWORD '{passwords[role]}'",1,log=False);created_roles.append(role)
    for database in (a,b,fresh):
        run(ref_admin,f'CREATE DATABASE {database}',1);created_dbs[0].append(database)
    admins=[open_conn(0,a),open_conn(1,a)]
    for database in (b,fresh):
        run(admins[1],f'CREATE DATABASE {database}',1);created_dbs[1].append(database)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]
    clones=[open_conn(side,fresh) for side in (0,1)]
    for side,conn in enumerate(admins):
        record.append(dict(side=side,full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
        for target in (conn,clones[side]):
            run(target,'CREATE TABLE public.idx_contract (id integer, obsolete integer, a integer, b integer)',1)
            run(target,'ALTER TABLE public.idx_contract DROP COLUMN obsolete',1)
        run(conn,'CREATE INDEX idx_contract_ba ON public.idx_contract (b,a)',1)
        run(conn,'CREATE UNIQUE INDEX idx_contract_uq ON public.idx_contract (a)',1)
    sql="SELECT pg_catalog.pg_get_indexdef(i.indexrelid),pg_catalog.pg_get_indexdef(i.indexrelid,0,false),pg_catalog.pg_get_indexdef(i.indexrelid,1,true),pg_catalog.pg_get_indexdef(i.indexrelid,2,true),pg_catalog.pg_get_indexdef(i.indexrelid,99,false),pg_catalog.pg_get_indexdef(i.indexrelid,-1,true) FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE c.relname='idx_contract_ba'"
    definition='CREATE INDEX idx_contract_ba ON public.idx_contract USING btree (b, a)'
    pair=definitions(sql,[definition,definition,'b','a','',''])
    uq="SELECT pg_catalog.pg_get_indexdef(i.indexrelid) FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE c.relname='idx_contract_uq'"
    unique=definitions(uq,['CREATE UNIQUE INDEX idx_contract_uq ON public.idx_contract USING btree (a)'])
    for side,conn in enumerate(admins):
        # Execute returned definitions verbatim in an isolated database.
        for result in (pair[side],unique[side]):
            run(clones[side],text(result['rows'][0])[0],1)
        facts="SELECT i.indkey,i.indisunique,i.indnatts,i.indnkeyatts FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid ORDER BY c.relname"
        assert run(conn,facts)['rows']==run(clones[side],facts)['rows']
        for target in (conn,clones[side]):
            run(target,'INSERT INTO public.idx_contract(id,a,b) VALUES (1,NULL,2),(2,NULL,3),(3,7,4)',1)
            denied=run(target,'INSERT INTO public.idx_contract(id,a,b) VALUES (4,7,5)',7)
            assert denied['errors']['C']=='23505',denied
            run(target,'SELECT 1')
        assert run(others[side],sql)['rows']==pair[side]['rows']
        assert not run(cross[side],sql)['rows']
        run(conn,f'ALTER TABLE public.idx_contract OWNER TO {owner}',1)
        run(conn,f'ALTER SCHEMA public OWNER TO {owner}',1)
        low=open_conn(side,a,reader)
        assert run(low,sql)['rows']==pair[side]['rows']
        assert run(low,'SELECT * FROM public.idx_contract',7)['errors']['C']=='42501'
    definitions('SELECT pg_catalog.pg_get_indexdef(0),pg_catalog.pg_get_indexdef(NULL),pg_catalog.pg_get_indexdef(0,99,true),pg_catalog.pg_get_indexdef(NULL,1,true)',[None]*4)
    # OID-bound query with no FROM: statement/portal metadata and text/binary values.
    lookup='SELECT pg_catalog.pg_get_indexdef($1),pg_catalog.pg_get_indexdef($1,1,true),pg_catalog.pg_get_indexdef($1,99,false)'
    for side,conn in enumerate(others):
        oid=text(run(conn,"SELECT i.indexrelid FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE c.relname='idx_contract_ba'")['rows'][0])[0]
        assert observe(prepare(conn,b'def_oid',lookup.encode(),1,(c.c_uint*1)(26)))['status']==1
        desc=observe(describe(conn,b'def_oid'));check_metadata(desc,[(25,-1,-1)]*3)
        record.append(dict(side=side,sql=lookup,parameter_oids=[26],phase='Parse / Describe statement',result=desc))
        wire=Wire(conn)
        try:
            wire.send(b'P',b'oid_portal_stmt\0'+lookup.encode()+b'\0'+struct.pack('!hI',1,26))
            wire.send(b'B',b'oid_portal\0oid_portal_stmt\0'+struct.pack('!hhi',0,1,len(oid))+oid.encode()+struct.pack('!h',0))
            wire.send(b'D',b'Poid_portal\0');wire.send(b'H')
            portal=wire.described()
            assert portal[-1][0]=='T'
            body=bytes.fromhex(portal[-1][1]);offset=2;fields=[]
            for _ in range(struct.unpack('!h',body[:2])[0]):
                end=body.index(0,offset)
                table,attribute,oid_type,size,mod,fmt=struct.unpack('!IhIhih',body[end+1:end+19])
                fields.append(dict(name=body[offset:end].decode(),table=table,attribute=attribute,oid=oid_type,size=size,mod=mod,format=fmt))
                offset=end+19
            assert fields==desc['fields'],(fields,desc)
            wire.send(b'E',b'oid_portal\0'+struct.pack('!I',0));wire.send(b'S')
            result=wire.ready()
            assert sum(t=='D' for t,_ in result)==1 and all(t!='E' for t,_ in result),result
            record.append(dict(side=side,sql=lookup,parameters=[oid],phase='parameter Describe portal / Execute / Sync',describe=portal,result=result))
            wire.send(b'C',b'Poid_portal\0');wire.send(b'C',b'Soid_portal_stmt\0');wire.send(b'S')
            assert ''.join(t for t,_ in wire.ready())=='33Z'
        finally: wire.close()
        for parameter,state in [(oid.encode(),None),(b'0',None),(None,None),(b'bad','22P02'),(b'4294967296','22003')]:
            for fmt in (0,1):
                out=observe(prepared(conn,b'def_oid',1,(s*1)(parameter),None,None,fmt))
                record.append(dict(side=side,sql=lookup,parameters=[None if parameter is None else parameter.decode()],format=fmt,phase='Bind / Execute / Sync',result=out,transaction=transaction(conn)))
                if state: assert out['status']==7 and out['errors']['C']==state,out
                else:
                    assert out['status']==2 and len(out['rows'])==1,out
                    assert [dict(f,format=0) for f in out['fields']]==desc['fields']
                    expected=[definition,'b',''] if parameter==oid.encode() else [None]*3
                    assert [None if v is None else canonical(v) for v in text(out['rows'][0])]==expected,out
                assert transaction(conn)==0
        run(admins[side],'ALTER INDEX public.idx_contract_ba RENAME TO idx_contract_ba_new',1)
        run(admins[side],'ALTER TABLE public.idx_contract RENAME COLUMN b TO b_new',1)
        out=observe(prepared(conn,b'def_oid',1,(s*1)(oid.encode()),None,None,0))
        record.append(dict(side=side,sql=lookup,parameters=[oid],phase='new Bind after DDL',result=out))
        assert [canonical(x) for x in text(out['rows'][0])]==['CREATE INDEX idx_contract_ba_new ON public.idx_contract USING btree (b_new, a)','b_new',''],out
        run(admins[side],'DROP INDEX public.idx_contract_ba_new',1)
        out=observe(prepared(conn,b'def_oid',1,(s*1)(oid.encode()),None,None,0))
        assert text(out['rows'][0])==[None]*3,out
        record.append(dict(side=side,sql=lookup,parameters=[oid],phase='new Bind after DROP',result=out))
    for conn in admins:
        run(conn,'CREATE TABLE public."Idx Mixed" ("a""b" integer)',1)
        run(conn,'CREATE INDEX "ix""q" ON public."Idx Mixed" ("a""b")',1)
    quote="SELECT pg_catalog.pg_get_indexdef(i.indexrelid),pg_catalog.pg_get_indexdef(i.indexrelid,1,true) FROM pg_catalog.pg_index i WHERE i.indrelid='public.\"Idx Mixed\"'::regclass::oid"
    expected=['CREATE INDEX "ix""q" ON public."Idx Mixed" USING btree ("a""b")','"a""b"']
    definitions(quote,expected)
    for conn in admins: run(conn,'SET search_path=pg_catalog',1)
    definitions(quote,expected)
    for conn in admins: run(conn,'RESET search_path',1)
    for conn in admins:
        run(conn,'CREATE SCHEMA "Index Space"',1)
        run(conn,'CREATE TABLE "Index Space".idx_custom (a integer)',1)
        run(conn,'CREATE INDEX idx_custom_ix ON "Index Space".idx_custom (a)',1)
    definitions("SELECT pg_catalog.pg_get_indexdef(i.indexrelid) FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE c.relname='idx_custom_ix'",['CREATE INDEX idx_custom_ix ON "Index Space".idx_custom USING btree (a)'])
    empty=quote.replace("i.indrelid='public.\"Idx Mixed\"'::regclass::oid",'i.indexrelid=0')
    assert all(not out['rows'] for out in contract_pair(empty,[(25,-1,-1)]*2))
    # Unsupported sources must never produce a simplified executable definition.
    for suffix in ['USING hash (a)','(a) INCLUDE (b_new)','((a+1))','(a) WHERE b_new>0','(a DESC)']:
        ddl='CREATE INDEX idx_unsupported ON public.idx_contract '+suffix
        run(admins[0],ddl,1)
        reference=run(admins[0],"SELECT pg_catalog.pg_get_indexdef(i.indexrelid) FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE c.relname='idx_unsupported'")
        assert reference['rows']
        run(admins[0],'DROP INDEX public.idx_unsupported',1)
        denied=run(admins[1],ddl,7)
        assert denied['errors']['C']=='0A000',denied
        rejected=observe(prepare(admins[1],b'unsupported',ddl.encode(),0,None))
        record.append(dict(side=1,sql=ddl,phase='Parse rejection',result=rejected,transaction=transaction(admins[1])))
        assert rejected['status']==7 and rejected['errors']['C']=='0A000',rejected
        assert not run(admins[1],"SELECT c.oid FROM pg_catalog.pg_class c WHERE c.relname='idx_unsupported'")['rows']
        run(admins[1],'SELECT 1')
    for conn in admins:
        bad=run(conn,'SELECT pg_catalog.pg_get_indexdef(0,1,true,0)',7)
        record.append(dict(phase='wrong signature',result=bad))
        run(conn,'SELECT 1')
        for q,st,tx in [('BEGIN',1,2),('SELECT * FROM public.idx_missing',7,3),('ROLLBACK',1,0)]:
            out=observe(execute(conn,q.encode()));record.append(dict(sql=q,result=out,transaction=transaction(conn),phase='error / recovery'))
            assert out['status']==st and transaction(conn)==tx,out
    count=sum('result' in observation for observation in record)
    print(f'pg13_016 differential: observations={count} PostgreSQL=180006 libpq={libversion()} IDX-DEF/QUOTE/UNKNOWN clone_rebuild=4 uniqueness=4 authenticated_reader=2 recovery cleanup',flush=True)

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
    (evidence/'index-definition.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
