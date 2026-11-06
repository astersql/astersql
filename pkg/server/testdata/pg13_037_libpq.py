# Copyright 2026 AsterSQL.
"""Standard table constraints contract; independent libpq against fixed PostgreSQL 18.6."""
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
base = sys.argv[2]
assert base.replace('_', '').isalnum()
a, b = base, base + '_b'
reader = base + '_reader'
owner = base + '_owner'
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
    if expected==1:
        words=sql.upper().split()
        expected_tag='UPDATE 1' if words[0]=='UPDATE' else 'INSERT 0 1' if words[0]=='INSERT' else ' '.join(words[:2]) if words[0] in ('CREATE','ALTER','DROP') else words[0]
        if words[:2]==['CREATE','TEMPORARY']: expected_tag='CREATE TABLE'
        if words[:3]==['CREATE','UNIQUE','INDEX']: expected_tag='CREATE INDEX'
        if words[0] in ('GRANT','REVOKE') and 'ON' not in words: expected_tag += ' ROLE'
        assert out['command']==expected_tag,(sql if log else 'credential setup',out)
    assert transaction(conn) == 0, (sql if log else "credential setup", transaction(conn))
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
        return [dict(f,table=('table_constraints' if f['table']==schemata_ids[side] else f['table']) if f['table'] in (0,1247,1249,2604,1259,2615,2224,2608,schemata_ids[side]) else ('unmapped',side,f['table'])) for f in fields]
    assert origins(pair[0]['fields'],0) == origins(pair[1]['fields'],1), (sql, pair)
    assert pair[0]['command'] == pair[1]['command'], (sql, pair)
    if compare_rows:
        assert [text(r) for r in pair[0]['rows']] == [text(r) for r in pair[1]['rows']], (sql, pair)
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


def scalar(conn,sql):
    out=run(conn,sql)
    assert len(out['rows'])==1 and len(out['rows'][0])==1,out
    return text(out['rows'][0])[0]

try:
    ref_admin=connect(reference_dsn.encode())
    assert ref_admin and connected(ref_admin)==0,'reference unavailable'
    connections.append(ref_admin)
    assert serverversion(ref_admin)==180006 and libversion()==180006
    record.append(dict(version=run(ref_admin,'SELECT version()'),server_version_num=180006,libpq=libversion()))
    run(ref_admin,f"CREATE ROLE {reader} LOGIN PASSWORD '{passwords[reader]}'",1,log=False);created_roles.append(reader)
    run(ref_admin,f'CREATE ROLE {owner}',1);created_roles.append(owner)
    run(ref_admin,f'GRANT {owner} TO {reader}',1)
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database}',1);created_dbs[0].append(database)
    admins=[open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'CREATE DATABASE {b}',1);created_dbs[1].append(b)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]
    sql="SELECT constraint_catalog,constraint_schema,constraint_name,table_catalog,table_schema,table_name,constraint_type,is_deferrable,initially_deferred,enforced,nulls_distinct FROM information_schema.table_constraints WHERE table_schema='public' AND table_name='tc_c' ORDER BY constraint_name"
    types=[(19,64,-1)]*6+[(1043,-1,-1)]+[(1043,-1,7)]*4
    for side,conn in enumerate(admins):
        record.append(dict(side=side,version=run(conn,'SELECT version()'),server_version_num=serverversion(conn),path=run(conn,'SHOW search_path')))
        run(conn,'CREATE TABLE public.tc_p(a integer, b integer, CONSTRAINT tc_pk PRIMARY KEY(a,b), CONSTRAINT tc_u UNIQUE(b))',1)
        run(conn,'CREATE TABLE public.tc_c(x integer NOT NULL, v integer, CONSTRAINT tc_f FOREIGN KEY(x) REFERENCES public.tc_p(b), CONSTRAINT tc_ck CHECK(v > 0))',1)
    schemata_ids=[int(scalar(conn,"SELECT c.oid FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='information_schema' AND c.relname='table_constraints'")) for conn in admins]
    pair=contract_pair(sql,types)
    assert len(pair[0]['rows'])==3,pair
    for row in map(text,pair[0]['rows']): assert row[7:]==['NO','NO','YES',None],row
    contract_pair(sql.replace('tc_c','missing'),types)
    parent=contract_pair(sql.replace('tc_c','tc_p'),types)
    assert any(text(r)[2:3]==['tc_u'] and text(r)[6:]==['UNIQUE','NO','NO','YES','YES'] for r in parent[0]['rows']),parent
    for conn in admins:
        run(conn,'CREATE TABLE public.tc_off(v integer,CONSTRAINT tc_off_ck CHECK(v>0) NOT ENFORCED)',1)
        run(conn,'INSERT INTO public.tc_off VALUES(-1)',1)
    off=contract_pair(sql.replace('tc_c','tc_off'),types)
    assert text(off[0]['rows'][0])[6:]==['CHECK','NO','NO','NO',None],off
    readers=[open_conn(side,a,reader) for side in (0,1)]
    parameter_sql=sql.replace("table_schema='public' AND table_name='tc_c'",'table_schema=$1 AND table_name=$2')
    for side,conn in enumerate(admins):
        low=readers[side]
        assert run(low,sql)['rows']==[]
        run(conn,f'GRANT USAGE ON SCHEMA public TO {reader}',1)
        for privilege in ('SELECT','SELECT(x)'):
            run(conn,f'GRANT {privilege} ON public.tc_c TO {reader}',1)
            assert run(low,sql)['rows']==[], 'SELECT must not expose constraints'
            run(conn,f'REVOKE {privilege} ON public.tc_c FROM {reader}',1)
            count+=1
        for privilege in ('UPDATE(v)','INSERT(x)','REFERENCES(x)','INSERT','UPDATE','DELETE','REFERENCES','TRIGGER'):
            run(conn,f'GRANT {privilege} ON public.tc_c TO {reader}',1)
            assert len(run(low,sql)['rows'])==3
            run(conn,f'REVOKE {privilege} ON public.tc_c FROM {reader}',1)
            assert run(low,sql)['rows']==[]
            count+=1
        assert run(conn,sql)==pair[side]
        assert run(cross[side],sql)['rows']==[]
        run(cross[side],'CREATE TABLE public.tc_c(z integer,CONSTRAINT cross_ck CHECK(z>0))',1)
        cross_result=run(cross[side],sql)
        assert len(cross_result['rows'])==1 and text(cross_result['rows'][0])[0]==b
        assert run(conn,sql)==pair[side]
        count+=1
        # Same names must remain distinct across tables and schemas, independent of path.
        run(conn,'CREATE SCHEMA tc_s',1)
        run(conn,'CREATE TABLE tc_s.other(a integer,CONSTRAINT tc_ck CHECK(a>0))',1)
        run(conn,'CREATE TABLE public.tc_other(a integer,CONSTRAINT tc_ck CHECK(a>0))',1)
        names=sql[:sql.index(' WHERE')]+" WHERE constraint_name='tc_ck' ORDER BY table_schema,table_name"
        assert len(run(conn,names)['rows'])==3
        run(conn,'SET search_path TO pg_catalog',1)
        assert len(run(conn,names)['rows'])==3
        run(conn,'RESET search_path',1)
        count+=1
        run(conn,'CREATE TABLE public.tc_i(a integer)',1)
        run(conn,'CREATE UNIQUE INDEX tc_ix ON public.tc_i(a)',1)
        assert run(conn,sql.replace('tc_c','tc_i'))['rows']==[]
        count+=1
        run(conn,'CREATE TEMPORARY TABLE tc_local(a integer NOT NULL)',1)
        local_sql=sql[:sql.index(' WHERE')]+" WHERE table_name='tc_local' ORDER BY constraint_name"
        local=run(conn,local_sql)
        assert len(local['rows'])==1 and text(local['rows'][0])[6]=='CHECK',local
        assert run(others[side],local_sql)['rows']==[]
        count+=1
        # Constraint OIDs and state are checked within each endpoint.
        pg=run(conn,"SELECT conname,contype,condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE conrelid='public.tc_c'::regclass::oid ORDER BY conname")
        catalog={text(r)[0]:text(r)[1:] for r in pg['rows']}
        for row in map(text,run(conn,sql)['rows']):
            source=catalog[row[2]]
            assert row[6]=={'p':'PRIMARY KEY','u':'UNIQUE','f':'FOREIGN KEY','c':'CHECK','n':'CHECK'}[source[0]]
            assert row[7:10]==['YES' if x=='t' else 'NO' for x in (source[1],source[2],source[4])]
        count+=1
        assert observe(prepare(others[side],b'fresh',sql.encode(),0,None))['status']==1
        run(conn,'ALTER TABLE public.tc_c RENAME COLUMN x TO xx',1)
        run(conn,'ALTER TABLE public.tc_p RENAME COLUMN b TO bb',1)
        fresh=observe(prepared(others[side],b'fresh',0,None,None,None,0))
        record.append(dict(side=side,phase='fresh Execute after rename',result=fresh))
        assert fresh==run(conn,sql) and len(fresh['rows'])==3
        run(conn,'ALTER TABLE public.tc_c DROP CONSTRAINT tc_f',1)
        fresh=observe(prepared(others[side],b'fresh',0,None,None,None,0))
        record.append(dict(side=side,phase='fresh Execute after drop',result=fresh))
        assert fresh==run(conn,sql) and len(fresh['rows'])==2
        count+=1
        assert observe(prepare(conn,b'param',parameter_sql.encode(),2,(c.c_uint*2)(25,25)))['status']==1
        pd=observe(describe(conn,b'param')); record.append(dict(side=side,phase='parameter Describe',result=pd))
        assert pd['parameter_oids']==[25,25],pd
        for name in (b'tc_c',b'missing',None):
            for fmt in (0,1):
                out=observe(prepared(conn,b'param',2,(s*2)(b'public',name),None,None,fmt))
                record.append(dict(side=side,sql=parameter_sql,parameters=['public',None if name is None else name.decode()],format=fmt,phase='Bind Execute Sync',result=out,transaction=transaction(conn)))
                assert out['status']==2 and transaction(conn)==0 and len(out['rows'])==(2 if name==b'tc_c' else 0),out
                check_metadata(out,types)
                count+=1
        bad=observe(prepare(conn,b'bad',b'SELECT absent FROM information_schema.table_constraints',0,None))
        record.append(dict(side=side,phase='Parse error',result=bad));assert bad['status']==7 and bad['errors']['C']=='42703',bad
        step(conn,'BEGIN',1,2);step(conn,'SELECT absent FROM information_schema.table_constraints',7,3,'42703');step(conn,'SELECT 1',7,3,'25P02');step(conn,'ROLLBACK',1,0)
        count+=1
        not_valid='ALTER TABLE public.tc_c ADD CONSTRAINT tc_nv CHECK(v>0) NOT VALID'
        out=run(conn,not_valid,7 if side else 1)
        if side:
            assert out['errors']['C']=='0A000',out
            assert len(run(conn,sql)['rows'])==2
        else:
            nv=run(conn,sql.replace("ORDER BY constraint_name","AND constraint_name='tc_nv' ORDER BY constraint_name"))
            assert text(nv['rows'][0])[9]=='YES',nv
            # Remove through reference DDL only; no unsupported product mutation.
            run(conn,'ALTER TABLE public.tc_c DROP CONSTRAINT tc_nv',1)
        count+=1
        # Existing deferred/NULLS-NOT-DISTINCT execution remains an explicit product gate.
        unsupported='CREATE TABLE public.tc_deferred(a integer,CONSTRAINT tc_d UNIQUE NULLS NOT DISTINCT(a) DEFERRABLE INITIALLY DEFERRED)'
        out=run(conn,unsupported,7 if side else 1)
        if side:
            assert out['errors']['C']=='0A000',out
            assert run(conn,"SELECT relname FROM pg_catalog.pg_class WHERE relname='tc_deferred'")['rows']==[]
        else:
            deferred=run(conn,sql.replace('tc_c','tc_deferred'))
            assert text(deferred['rows'][0])[6:]==['UNIQUE','YES','YES','YES','NO'],deferred
            run(conn,'DROP TABLE public.tc_deferred',1)
        count+=1
        # Owner inheritance is established through authenticated membership, not a name override.
        run(conn,f'ALTER TABLE public.tc_c OWNER TO {owner}',1)
        assert len(run(low,sql)['rows'])==2
        run(conn,'ALTER TABLE public.tc_c OWNER TO root',1)
        assert run(low,sql)['rows']==[]
        count+=1
        # Local temporary models are connection-owned and released by finally/PQfinish.
        for table in ('tc_c','tc_p','tc_i','tc_other','tc_off'): run(conn,f'DROP TABLE public.{table}',1)
        run(conn,'DROP TABLE tc_s.other',1);run(conn,'DROP SCHEMA tc_s',1)
        assert run(others[side],sql)['rows']==[]
        count+=1
    print(f'pg13_037 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} p_u_f_c_n state independent_index name ACL authentication owner_inheritance temporary cross_database fresh_execute text binary null zero_describe error recovery cleanup',flush=True)

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
    failed=any(out['status']!=1 for _,out in cleanup)
    record.append(dict(cleanup=cleanup,connections_closed=True,cleanup_failed=failed))
    evidence=Path(os.environ['PG13_EVIDENCE_DIR']);evidence.mkdir(parents=True,exist_ok=True)
    (evidence/'table-constraints-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not failed,'fixture cleanup failed; see evidence'
