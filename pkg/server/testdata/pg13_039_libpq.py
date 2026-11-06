# Copyright 2026 AsterSQL.
"""Standard referential constraint contract; independent libpq against fixed PostgreSQL 18.6."""
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
        return [dict(f,table=('referential_constraints' if f['table']==schemata_ids[side] else f['table']) if f['table'] in (0,1247,1249,2604,1259,2615,2224,2608,schemata_ids[side]) else ('unmapped',side,f['table'])) for f in fields]
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
    sql="SELECT constraint_catalog,constraint_schema,constraint_name,unique_constraint_catalog,unique_constraint_schema,unique_constraint_name,match_option,update_rule,delete_rule FROM information_schema.referential_constraints WHERE constraint_schema='public' AND constraint_name='rc_fk'"
    types=[(19,64,-1)]*6+[(1043,-1,-1)]*3
    for side,conn in enumerate(admins):
        record.append(dict(side=side,version=run(conn,'SELECT version()'),server_version_num=serverversion(conn),path=run(conn,'SHOW search_path')))
        run(conn,'CREATE TABLE public.rp(a integer,b integer,CONSTRAINT rp_pk PRIMARY KEY(a,b))',1)
        run(conn,'CREATE TABLE public.rc(x integer,y integer,v integer,CONSTRAINT rc_fk FOREIGN KEY(x,y) REFERENCES public.rp(b,a) ON UPDATE RESTRICT ON DELETE CASCADE)',1)
    schemata_ids=[int(scalar(conn,"SELECT c.oid FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='information_schema' AND c.relname='referential_constraints'")) for conn in admins]
    pair=contract_pair(sql,types)
    assert [text(r) for r in pair[0]['rows']]==[[a,'public','rc_fk',a,'public','rp_pk','NONE','RESTRICT','CASCADE']],pair
    contract_pair(sql.replace("'rc_fk'","'missing'"),types)
    for conn in admins:
        run(conn,'CREATE TABLE public.ri(a integer,b integer)',1)
        run(conn,'CREATE UNIQUE INDEX ri_ix ON public.ri(a,b)',1)
        run(conn,'CREATE TABLE public.rr(x integer,y integer,CONSTRAINT rr_fk FOREIGN KEY(x,y) REFERENCES public.ri(b,a))',1)
        run(conn,'CREATE TABLE public.ru(a integer,CONSTRAINT ru_u UNIQUE(a))',1)
        run(conn,'CREATE TABLE public.rv(x integer,CONSTRAINT rv_fk FOREIGN KEY(x) REFERENCES public.ru(a) ON DELETE SET NULL)',1)
        run(conn,'CREATE TABLE public.rw(x integer,CONSTRAINT rw_fk FOREIGN KEY(x) REFERENCES public.ru(a) ON UPDATE CASCADE ON DELETE RESTRICT)',1)
    independent=contract_pair(sql.replace("'rc_fk'","'rr_fk'"),types)
    assert [text(r)[3:] for r in independent[0]['rows']]==[[None,None,None,'NONE','NO ACTION','NO ACTION']],independent
    unique=contract_pair(sql.replace("'rc_fk'","'rv_fk'"),types)
    assert [text(r)[3:] for r in unique[0]['rows']]==[[a,'public','ru_u','NONE','NO ACTION','SET NULL']],unique
    cascade=contract_pair(sql.replace("'rc_fk'","'rw_fk'"),types)
    assert text(cascade[0]['rows'][0])[6:]==['NONE','CASCADE','RESTRICT'],cascade
    # Each relation uses its own real identities. The names deliberately collide.
    for conn in admins:
        run(conn,'CREATE SCHEMA r_s',1)
        run(conn,'CREATE TABLE r_s.rp(a integer,CONSTRAINT rp_pk UNIQUE(a))',1)
        run(conn,'CREATE TABLE r_s.rc(x integer,CONSTRAINT rc_fk FOREIGN KEY(x) REFERENCES r_s.rp(a))',1)
        run(conn,'CREATE TABLE public.cross_child(x integer,CONSTRAINT cross_fk FOREIGN KEY(x) REFERENCES r_s.rp(a))',1)
    names=sql.replace("constraint_schema='public' AND ","")+" ORDER BY constraint_schema"
    collision=contract_pair(names,types)
    assert [text(r)[1:6] for r in collision[0]['rows']]==[['public','rc_fk',a,'public','rp_pk'],['r_s','rc_fk',a,'r_s','rp_pk']],collision
    cross_schema=contract_pair(sql.replace("'rc_fk'","'cross_fk'"),types)
    assert text(cross_schema[0]['rows'][0])[4:6]==['r_s','rp_pk']
    readers=[open_conn(side,a,reader) for side in (0,1)]
    parameter_sql=sql.replace("constraint_schema='public' AND constraint_name='rc_fk'",'constraint_schema=$2 AND constraint_name=$1')
    for side,conn in enumerate(admins):
        low=readers[side]
        assert run(low,sql)['rows']==[]
        run(conn,f'GRANT USAGE ON SCHEMA public TO {reader}',1)
        for privilege,visible in [('SELECT',False),('SELECT(x)',False),('INSERT(x)',True),('UPDATE(y)',True),('REFERENCES(v)',True),('DELETE',True)]:
            run(conn,f'GRANT {privilege} ON public.rc TO {reader}',1)
            got=run(low,sql)
            assert got['rows']==(pair[side]['rows'] if visible else []),(privilege,got)
            run(conn,f'REVOKE {privilege} ON public.rc FROM {reader}',1)
            assert run(low,sql)['rows']==[]
            count+=1
        assert run(conn,sql)==pair[side]
        assert run(cross[side],sql)['rows']==[]
        run(cross[side],'CREATE TABLE public.rp(a integer,CONSTRAINT rp_other UNIQUE(a))',1)
        run(cross[side],'CREATE TABLE public.rc(x integer,CONSTRAINT rc_fk FOREIGN KEY(x) REFERENCES public.rp(a))',1)
        cross_result=run(cross[side],sql)
        assert len(cross_result['rows'])==1 and text(cross_result['rows'][0])[0]==b and text(cross_result['rows'][0])[5]=='rp_other'
        assert run(conn,sql)==pair[side]
        count+=1
        run(conn,'SET search_path TO pg_catalog',1)
        assert run(conn,names)==collision[side]
        run(conn,'RESET search_path',1)
        count+=1
        temp_sql='CREATE TEMPORARY TABLE r_local_c(x integer,CONSTRAINT local_fk FOREIGN KEY(x) REFERENCES public.ru(a))'
        temp=observe(execute(conn,temp_sql.encode()))
        record.append(dict(side=side,sql=temp_sql,phase='temporary FK capability gate',result=temp,transaction=transaction(conn)))
        if side==1:
            assert temp['status']==7 and temp['errors']['C']=='0A000',temp
        else:
            assert temp['status']==7, temp  # PostgreSQL also forbids temp-to-permanent references.
        assert run(conn,"SELECT relname FROM pg_catalog.pg_class WHERE relname='r_local_c'")['rows']==[]
        assert transaction(conn)==0
        count+=1
        pg=run(conn,"SELECT c.confupdtype,c.confdeltype,c.confmatchtype,p.conname FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_constraint p ON p.conindid=c.conindid AND p.conrelid=c.confrelid WHERE c.conrelid='public.rc'::regclass::oid AND c.contype='f'")
        assert [text(r) for r in pg['rows']]==[['r','c','s','rp_pk']],pg
        count+=1
        original_fk=scalar(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE conrelid='public.rc'::regclass::oid AND contype='f'")
        assert observe(prepare(others[side],b'fresh',sql.encode(),0,None))['status']==1
        run(conn,'ALTER TABLE public.rc RENAME COLUMN x TO xx',1)
        run(conn,'ALTER TABLE public.rp RENAME COLUMN b TO bb',1)
        fresh=observe(prepared(others[side],b'fresh',0,None,None,None,0))
        record.append(dict(side=side,phase='fresh Execute after rename',result=fresh))
        assert fresh==pair[side] and fresh==run(conn,sql),fresh
        count+=1
        assert observe(prepare(conn,b'param',parameter_sql.encode(),2,(c.c_uint*2)(25,25)))['status']==1
        pd=observe(describe(conn,b'param'));record.append(dict(side=side,phase='parameter Describe',result=pd))
        assert pd['parameter_oids']==[25,25],pd
        for name in (b'rc_fk',b'missing',None):
            for fmt in (0,1):
                out=observe(prepared(conn,b'param',2,(s*2)(name,b'public'),None,None,fmt))
                record.append(dict(side=side,sql=parameter_sql,parameters=[None if name is None else name.decode(),'public'],format=fmt,phase='Bind Execute Sync',result=out,transaction=transaction(conn)))
                assert out['status']==2 and transaction(conn)==0 and len(out['rows'])==(1 if name==b'rc_fk' else 0),out
                check_metadata(out,types)
                count+=1
        bad=observe(prepare(conn,b'bad',b'SELECT absent FROM information_schema.referential_constraints',0,None))
        record.append(dict(side=side,phase='Parse error',result=bad));assert bad['status']==7 and bad['errors']['C']=='42703',bad
        step(conn,'BEGIN',1,2);step(conn,'SELECT absent FROM information_schema.referential_constraints',7,3,'42703');step(conn,'SELECT 1',7,3,'25P02');step(conn,'ROLLBACK',1,0)
        count+=1
        # Unsupported execution branches remain explicit, with no publication.
        for clause in ('MATCH FULL','MATCH PARTIAL','ON UPDATE SET NULL','ON DELETE SET DEFAULT','DEFERRABLE INITIALLY DEFERRED'):
            ddl=f'CREATE TABLE public.unsupported_rc(x integer,y integer,FOREIGN KEY(x,y) REFERENCES public.rp(a,bb) {clause})'
            out=observe(execute(conn,ddl.encode()))
            record.append(dict(side=side,sql=ddl,phase='DDL capability gate',result=out,transaction=transaction(conn)))
            if side==1:
                assert out['status']==7 and out['errors']['C']=='0A000',out
                assert run(conn,"SELECT relname FROM pg_catalog.pg_class WHERE relname='unsupported_rc'")['rows']==[]
            elif out['status']==1:
                run(conn,'DROP TABLE public.unsupported_rc',1)
            assert transaction(conn)==0
            count+=1
        run(conn,f'ALTER TABLE public.rc OWNER TO {owner}',1)
        assert len(run(low,sql)['rows'])==1
        run(conn,'ALTER TABLE public.rc OWNER TO root',1)
        assert run(low,sql)['rows']==[]
        count+=1
        run(conn,'ALTER TABLE public.rc DROP CONSTRAINT rc_fk',1)
        fresh=observe(prepared(others[side],b'fresh',0,None,None,None,0))
        assert fresh['rows']==[],fresh
        run(conn,'DROP TABLE public.rc',1)
        run(conn,'CREATE TABLE public.rc(xx integer,y integer,CONSTRAINT rc_fk FOREIGN KEY(xx,y) REFERENCES public.rp(bb,a))',1)
        fresh=observe(prepared(others[side],b'fresh',0,None,None,None,0))
        record.append(dict(side=side,phase='fresh Execute after recreate',result=fresh))
        assert len(fresh['rows'])==1 and text(fresh['rows'][0])[5:] == ['rp_pk','NONE','NO ACTION','NO ACTION'],fresh
        assert scalar(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE conrelid='public.rc'::regclass::oid AND contype='f'")!=original_fk
        count+=1
        for table in ('cross_child','rc','rp','rr','ri','rv','rw','ru'):run(conn,f'DROP TABLE public.{table}',1)
        run(conn,'DROP TABLE r_s.rc',1);run(conn,'DROP TABLE r_s.rp',1);run(conn,'DROP SCHEMA r_s',1)
        assert run(others[side],sql)['rows']==[]
        count+=1
    print(f'pg13_039 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} RC parent_index p_u independent_null cross_schema ACL authentication owner_inheritance temporary_gate cross_database fresh_execute rename text binary null zero_describe error recovery cleanup',flush=True)

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
    (evidence/'referential-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not failed,'fixture cleanup failed; see evidence'
