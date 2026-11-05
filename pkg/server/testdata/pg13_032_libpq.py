# Copyright 2026 AsterSQL.
"""Standard table list contract; independent libpq against fixed PostgreSQL 18.6."""
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
group = base + '_group'
member = base + '_member'
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
        expected_tag='INSERT 0 1' if words[0]=='INSERT' else ' '.join(words[:2]) if words[0] in ('CREATE','ALTER','DROP') else words[0]
        if words[:2]==['CREATE','TEMPORARY']: expected_tag='CREATE TABLE'
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
        return [dict(f,table=('views' if f['table']==schemata_ids[side] else f['table']) if f['table'] in (0,1247,1249,2604,1259,2615,2224,2608,schemata_ids[side]) else ('unmapped',side,f['table'])) for f in fields]
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
    for role in (group,member):
        run(ref_admin,f'CREATE ROLE {role}',1);created_roles.append(role)
    run(ref_admin,f'GRANT {group} TO {member}',1);run(ref_admin,f'GRANT {member} TO {reader}',1)
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database}',1);created_dbs[0].append(database)
    admins=[open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'CREATE DATABASE {b}',1);created_dbs[1].append(b)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]
    for side,conn in enumerate(admins):
        record.append(dict(side=side,version=run(conn,'SELECT version()'),server_version_num=serverversion(conn),path=run(conn,'SHOW search_path')))
        for sql in ('CREATE TABLE public.is_map_t(id integer)', 'INSERT INTO public.is_map_t VALUES (32)', 'CREATE VIEW public.is_map_v AS SELECT id FROM public.is_map_t', 'CREATE VIEW public.literal_v AS SELECT 32 AS id'):
            run(conn,sql,1)
    sql="SELECT table_catalog,table_schema,table_name,view_definition,check_option,is_updatable,is_insertable_into,is_trigger_updatable,is_trigger_deletable,is_trigger_insertable_into FROM information_schema.views WHERE table_schema='public' AND table_name='is_map_v'"
    for conn in admins: run(conn,sql)
    schemata_ids=[int(scalar(conn,"SELECT c.oid FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='information_schema' AND c.relname='views'")) for conn in admins]
    types=[(19,64,-1)]*3+[(1043,-1,-1)]*2+[(1043,-1,7)]*5
    # Definitions are semantic SQL, not formatting-equal across deparsers.
    for side,conn in enumerate(admins):
        out=run(conn,sql);check_metadata(out,types)
        definition=text(out['rows'][0])[3]
        assert definition==scalar(conn,"SELECT pg_catalog.pg_get_viewdef('public.is_map_v'::regclass)"),out
        run(conn,'CREATE VIEW public.clone_v AS '+definition,1)
        assert run(conn,'SELECT id FROM public.clone_v')['rows']==run(conn,'SELECT id FROM public.is_map_v')['rows']
        run(conn,'DROP VIEW public.clone_v',1)
    contract_pair(sql,types,False)
    for side,conn in enumerate(admins):
        sid=schemata_ids[side]
        out=run(conn,sql)
        assert all(f['table']==sid and f['attribute']==n+1 for n,f in enumerate(out['fields'])),out
        attributes=run(conn,f"SELECT a.attnum,a.attname,t.typname,t.typbasetype FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type t ON a.atttypid=t.oid WHERE a.attrelid={sid} AND a.attnum>0 ORDER BY a.attnum")
        labels=['table_catalog','table_schema','table_name','view_definition','check_option','is_updatable','is_insertable_into','is_trigger_updatable','is_trigger_deletable','is_trigger_insertable_into']
        assert [text(r) for r in attributes['rows']]==[[str(n+1),label,'sql_identifier' if n<3 else 'character_data' if n<5 else 'yes_or_no','19' if n<3 else '1043'] for n,label in enumerate(labels)],attributes

    fields="SELECT table_catalog,table_schema,table_name,check_option,is_updatable,is_insertable_into,is_trigger_updatable,is_trigger_deletable,is_trigger_insertable_into FROM information_schema.views WHERE table_schema='public' AND table_name='is_map_v'"
    pair=contract_pair(fields,types[:3]+types[4:])
    assert [text(r) for r in pair[0]['rows']]==[[a,'public','is_map_v','NONE','YES','YES','NO','NO','NO']],pair
    contract_pair(sql.replace('is_map_v','missing'),types)
    literal=contract_pair(fields.replace('is_map_v','literal_v'),types[:3]+types[4:])
    assert text(literal[0]['rows'][0])[4:]==['NO','NO','NO','NO','NO'],literal
    system_names="SELECT table_name FROM information_schema.views WHERE table_schema='information_schema' AND table_name IN ('schemata','tables','views') ORDER BY table_name"
    system=contract_pair(system_names,[(19,64,-1)])
    assert [text(r) for r in system[0]['rows']]==[['schemata'],['tables'],['views']],system
    readers=[open_conn(side,a,reader) for side in (0,1)]
    for side,conn in enumerate(admins):
        low=readers[side]
        assert run(low,sql)['rows']==[]
        system_definition="SELECT view_definition FROM information_schema.views WHERE table_schema='information_schema' AND table_name='views'"
        assert text(run(low,system_definition)['rows'][0])==[None]
        system_owner=run(conn,system_definition,2 if side==0 else 7)
        if side==1: assert system_owner['errors']['C']=='0A000',system_owner

        run(conn,f'GRANT SELECT ON public.is_map_v TO {reader}',1)
        out=run(low,sql);assert len(out['rows'])==1 and text(out['rows'][0])[3] is None,out
        check_metadata(out,types)
        run(conn,f'REVOKE USAGE ON SCHEMA public FROM PUBLIC',1)
        assert len(run(low,sql)['rows'])==1
        run(conn,f'GRANT USAGE ON SCHEMA public TO PUBLIC',1)
        run(conn,f'REVOKE SELECT ON public.is_map_v FROM {reader}',1);assert run(low,sql)['rows']==[]
        run(conn,f'ALTER TABLE public.is_map_v OWNER TO {group}',1)
        assert text(run(low,sql)['rows'][0])[3] is not None
        run(low,f'SET ROLE {member}',1);assert text(run(low,sql)['rows'][0])[3] is not None
        run(low,'SET ROLE NONE',1);run(conn,'ALTER TABLE public.is_map_v OWNER TO root',1)
        assert run(cross[side],sql)['rows']==[]
        run(conn,'SET search_path TO pg_catalog',1);assert len(run(conn,sql)['rows'])==1
        run(conn,'SET search_path TO public',1)
        assert observe(prepare(others[side],b'fresh',sql.encode(),0,None))['status']==1
        run(conn,'ALTER TABLE public.is_map_t RENAME TO renamed_t',1)
        out=observe(prepared(others[side],b'fresh',0,None,None,None,0))
        assert out['status']==2 and 'renamed_t' in text(out['rows'][0])[3],out
        record.append(dict(side=side,phase='fresh Execute after base rename',result=out))
        run(conn,'CREATE VIEW public.checked_v AS SELECT id FROM public.renamed_t WHERE id>0 WITH LOCAL CHECK OPTION',1)
        assert scalar(conn,"SELECT check_option FROM information_schema.views WHERE table_name='checked_v'")=='LOCAL'
        run(conn,'CREATE VIEW public.cascaded_v AS SELECT id FROM public.renamed_t WHERE id>0 WITH CASCADED CHECK OPTION',1)
        assert scalar(conn,"SELECT check_option FROM information_schema.views WHERE table_name='cascaded_v'")=='CASCADED'
        # Bound parameters include SQL NULL and zero-row complete descriptions.
        parameter_sql="SELECT table_name,view_definition FROM information_schema.views WHERE table_schema=$1 AND table_name=$2"
        oids=(c.c_uint*2)(25,25)
        assert observe(prepare(conn,b'param',parameter_sql.encode(),2,oids))['status']==1
        for name in (b'is_map_v',b'missing',None):
            for fmt in (0,1):
                vals=(s*2)(b'public',name)
                out=observe(prepared(conn,b'param',2,vals,None,None,fmt))
                record.append(dict(side=side,sql=parameter_sql,parameters=['public',None if name is None else name.decode()],format=fmt,phase='Bind Execute Sync',result=out,transaction=transaction(conn)))
                assert out['status']==2 and transaction(conn)==0 and len(out['rows'])==(1 if name==b'is_map_v' else 0),out
                check_metadata(out,[(19,64,-1),(1043,-1,-1)])
        out=observe(prepare(conn,b'bad',b'SELECT absent FROM information_schema.views',0,None))
        record.append(dict(side=side,phase='Parse error',result=out))
        assert out['status']==7 and out['errors']['C']=='42703',out
        # Session-local table rows cannot leak into the view-only provider.
        run(conn,'CREATE TEMPORARY TABLE session_local(id integer)',1)
        run(others[side],'CREATE TEMPORARY TABLE other_local(id integer)',1)
        for observer in (conn,others[side],cross[side]):
            assert run(observer,"SELECT table_name FROM information_schema.views WHERE table_name IN ('session_local','other_local')")['rows']==[]
        count+=10
        for bad in ('SELECT absent FROM information_schema.views',):
            assert run(conn,bad,7)['errors']['C']=='42703'
        step(conn,'BEGIN',1,2);step(conn,'SELECT absent FROM information_schema.views',7,3,'42703');step(conn,'SELECT 1',7,3,'25P02');step(conn,'ROLLBACK',1,0)
        for name in ('checked_v','cascaded_v','is_map_v','literal_v'):run(conn,f'DROP VIEW public.{name}',1)
        assert run(others[side],sql)['rows']==[]
        run(conn,'DROP TABLE public.renamed_t',1)
        count+=20
    print(f'pg13_032 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} definition clone flags check_option authentication owner inherited isolation fresh_execute text binary recovery cleanup',flush=True)

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
    (evidence/'views-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not failed,'fixture cleanup failed; see evidence'
