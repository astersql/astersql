# Copyright 2026 AsterSQL.
"""Index identity contract; independent libpq against fixed PostgreSQL 18.6."""
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


try:
    ref_admin=connect(reference_dsn.encode())
    assert ref_admin and connected(ref_admin)==0, 'reference unavailable'
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
    for side,conn in enumerate(admins):
        record.append(dict(side=side,full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
        run(conn,'CREATE TABLE public.idx_contract (id integer, a integer, b integer, CONSTRAINT idx_contract_pk PRIMARY KEY (id))',1)
        run(conn,'CREATE INDEX idx_contract_ba ON public.idx_contract (b,a)',1)
        run(conn,'CREATE UNIQUE INDEX idx_contract_uq ON public.idx_contract (a)',1)
    base_query="SELECT c.relname,i.indexrelid,i.indrelid,i.indnatts,i.indnkeyatts,i.indisunique,i.indisprimary,i.indnullsnotdistinct,i.indkey,i.indisvalid,i.indisready FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE i.indrelid='public.idx_contract'::regclass::oid ORDER BY c.relname"
    types=[(19,64,-1),(26,4,-1),(26,4,-1),(21,2,-1),(21,2,-1),(16,1,-1),(16,1,-1),(16,1,-1),(22,-1,-1),(16,1,-1),(16,1,-1)]
    pair=contract_pair(base_query,types,False)
    saved=[]
    for side,out in enumerate(pair):
        data=[text(r) for r in out['rows']];assert len(data)==3,data
        saved.append({r[0]:r[1] for r in data})
        assert sorted((r[3:] for r in data))==sorted([['2','2','f','f','f','3 2','t','t'],['1','1','t','t','f','1','t','t'],['1','1','t','f','f','2','t','t']]),data
        for row_ in data:
            name,oid,table_oid=row_[:3]
            assert text(run(admins[side],f"SELECT c.oid FROM pg_catalog.pg_class c WHERE c.oid={oid}")['rows'][0])==[oid]
            assert text(run(admins[side],f"SELECT '{name}'::regclass::oid AS oid")['rows'][0])==[oid]
            assert text(run(admins[side],"SELECT 'public.idx_contract'::regclass::oid AS oid")['rows'][0])==[table_oid]
    unique="SELECT conrelid FROM pg_catalog.pg_constraint WHERE conrelid='public.idx_contract'::regclass::oid AND contype='u'"
    assert all(not out['rows'] for out in contract_pair(unique,[(26,4,-1)]))
    for conn in admins:
        run(conn,'CREATE TABLE public.idx_handle (id integer PRIMARY KEY)',1)
        run(conn,'CREATE TABLE public.idx_empty (n integer)',1)
    handle=base_query.replace('idx_contract','idx_handle')
    for out in contract_pair(handle,types,False):
        assert len(out['rows'])==1 and text(out['rows'][0])[3:]==['1','1','t','t','f','1','t','t'],out
    assert all(not out['rows'] for out in contract_pair(base_query.replace('idx_contract','idx_empty'),types))
    lookup='SELECT i.indexrelid,i.indrelid,i.indkey FROM pg_catalog.pg_index i WHERE i.indexrelid=$1'
    desc_lookup=[]
    for side,conn in enumerate(others):
        assert observe(prepare(conn,b'life',lookup.encode(),1,(c.c_uint*1)(26)))['status']==1
        desc=observe(describe(conn,b'life'));check_metadata(desc,[(26,4,-1),(26,4,-1),(22,-1,-1)]);desc_lookup.append(desc)
        record.append(dict(side=side,sql=lookup,phase='Parse / Describe before DDL',result=desc))
    for side,conn in enumerate(admins):
        run(conn,f'ALTER TABLE public.idx_contract OWNER TO {owner}',1)
        run(conn,f'ALTER SCHEMA public OWNER TO {owner}',1)
        conn=open_conn(side,a,owner)
        low=open_conn(side,a,reader)
        run(low,'SET search_path=pg_catalog',1)
        assert run(low,base_query)['rows']==pair[side]['rows']
        denied=run(low,'SELECT * FROM public.idx_contract',7);assert denied['errors']['C']=='42501',denied
        run(low,'SELECT 1')
        run(cross[side],'CREATE TABLE public.idx_contract (other integer)',1)
        assert not run(cross[side],base_query)['rows']
        collision=run(conn,'ALTER INDEX public.idx_contract_ba RENAME TO idx_contract_uq',7)
        assert collision['errors']['C']=='42P07',collision
        denied=run(low,'ALTER INDEX public.idx_contract_ba RENAME TO idx_denied',7)
        assert denied['errors']['C']=='42501',denied
        run(low,'SELECT 1')
        run(conn,'SET search_path=pg_catalog',1)
        hidden=run(conn,'ALTER INDEX idx_contract_ba RENAME TO idx_hidden',7)
        assert hidden['errors']['C']=='42P01',hidden
        rename_sql='ALTER INDEX public.idx_contract_ba RENAME TO idx_contract_ba_new'
        parsed=observe(prepare(conn,b'rename_index',rename_sql.encode(),0,None))
        record.append(dict(side=side,sql=rename_sql,phase='Parse DDL',result=parsed))
        assert parsed['status']==1,parsed
        desc=observe(describe(conn,b'rename_index'))
        record.append(dict(side=side,sql=rename_sql,phase='Describe DDL statement',result=desc))
        assert desc['status']==1 and not desc['fields'],desc
        before=run(others[side],base_query)
        assert any(text(r)[0]=='idx_contract_ba' and text(r)[1]==saved[side]['idx_contract_ba'] for r in before['rows']),before
        changed_ddl=observe(prepared(conn,b'rename_index',0,None,None,None,0))
        record.append(dict(side=side,sql=rename_sql,phase='Bind / Execute DDL / Sync',result=changed_ddl,transaction=transaction(conn)))
        assert changed_ddl['status']==1 and changed_ddl['command']=='ALTER INDEX' and transaction(conn)==0,changed_ddl
        run(conn,'SET search_path=public,pg_catalog',1)
        run(conn,'ALTER TABLE public.idx_contract RENAME COLUMN b TO b_new',1)
        changed=run(others[side],base_query)
        rows_=[text(r) for r in changed['rows']]
        assert any(r[0]=='idx_contract_ba_new' and r[1]==saved[side]['idx_contract_ba'] and r[8]=='3 2' for r in rows_),rows_
        for fmt in (0,1):
            out=observe(prepared(others[side],b'life',1,(s*1)(saved[side]['idx_contract_ba'].encode()),None,None,fmt))
            record.append(dict(side=side,sql=lookup,parameters=[saved[side]['idx_contract_ba']],phase='new Execute after rename',format=fmt,result=out))
            assert out['status']==2 and len(out['rows'])==1,out
            assert [dict(f,format=0) for f in out['fields']]==desc_lookup[side]['fields'],out
            if fmt==0:
                assert text(out['rows'][0])==[saved[side]['idx_contract_ba'],text(pair[side]['rows'][0])[2],'3 2'],out
            else:
                assert int(out['rows'][0][0],16)==int(saved[side]['idx_contract_ba']),out
                raw=bytes.fromhex(out['rows'][0][2]);assert raw==struct.pack('!iiIii',1,0,21,2,0)+struct.pack('!ihih',2,3,2,2),out
        definition=run(others[side],f"SELECT pg_catalog.pg_get_indexdef({saved[side]['idx_contract_ba']}) AS definition")
        definition=text(definition['rows'][0])[0]
        assert 'idx_contract_ba_new' in definition and 'b_new' in definition,definition
        for value_,state in [(b'0',None),(None,None),(b'bad','22P02'),(b'4294967296','22003')]:
            out=observe(prepared(others[side],b'life',1,(s*1)(value_),None,None,0))
            record.append(dict(side=side,sql=lookup,parameters=[None if value_ is None else value_.decode()],phase='Bind / Execute boundary',result=out,transaction=transaction(others[side])))
            if state:assert out['status']==7 and out['errors']['C']==state,out
            else:assert out['status']==2 and not out['rows'],out
            assert transaction(others[side])==0
        run(conn,'DROP INDEX public.idx_contract_ba_new',1)
        out=observe(prepared(others[side],b'life',1,(s*1)(saved[side]['idx_contract_ba'].encode()),None,None,0))
        record.append(dict(side=side,sql=lookup,parameters=[saved[side]['idx_contract_ba']],phase='new Execute after drop',result=out))
        assert out['status']==2 and not out['rows'];check_metadata(out,[(26,4,-1),(26,4,-1),(22,-1,-1)])
        null_def=run(conn,f"SELECT pg_catalog.pg_get_indexdef({saved[side]['idx_contract_ba']}) AS definition")
        assert [text(r) for r in null_def['rows']]==[[None]],null_def
        run(conn,'CREATE INDEX idx_contract_ba_new ON public.idx_contract (b_new,a)',1)
        rebuilt=run(others[side],base_query)
        assert all(text(r)[1]!=saved[side]['idx_contract_ba'] for r in rebuilt['rows']),rebuilt
        for sql in ['INSERT INTO public.idx_contract(id,a) VALUES (1,NULL),(2,NULL)','INSERT INTO public.idx_contract(id,a) VALUES (3,7)']:
            run(conn,sql,1)
        duplicate=run(conn,'INSERT INTO public.idx_contract(id,a) VALUES (4,7)',7)
        assert duplicate['errors']['C']=='23505',duplicate
        run(conn,'SELECT 1')
        for q,st,tx in [('BEGIN',1,2),('SELECT * FROM public.idx_missing',7,3),('ROLLBACK',1,0)]:
            out=observe(execute(conn,q.encode()));record.append(dict(side=side,sql=q,result=out,transaction=transaction(conn),phase='error / recovery'))
            assert out['status']==st and transaction(conn)==tx,out
        run(cross[side],'DROP TABLE public.idx_contract',1)
    count=sum('result' in observation for observation in record)
    print(f'pg13_014 differential: scenarios={count} PostgreSQL=180006 libpq={libversion()} IDX-BASE/HANDLE/LIFE recovery cleanup',flush=True)

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
    (evidence/'index-identity.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
