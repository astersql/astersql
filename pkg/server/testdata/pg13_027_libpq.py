# Copyright 2026 AsterSQL.
"""Sequence declaration and parameter contract; independent libpq against fixed PostgreSQL 18.6."""
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
        return [dict(f,table=f['table'] if f['table'] in (0,1249,2604,1259,2615,2224) else origin_identities[side][f['table']]) for f in fields]
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


sequence_fields=[(26,4,-1),(26,4,-1)]+[(20,8,-1)]*5+[(16,1,-1)]
param_fields=sequence_fields[1:]
projection='seqtypid,seqstart,seqincrement,seqmin,seqmax,seqcache,seqcycle'

def params(conn,name):
    out=run(conn,f"SELECT {projection} FROM pg_catalog.pg_sequence WHERE seqrelid='public.{name}'::regclass::oid")
    check_metadata(out,param_fields)
    assert len(out['rows'])==1,out
    return text(out['rows'][0])

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
    for database in (a,b):
        run(ref_admin,f'CREATE DATABASE {database}',1);created_dbs[0].append(database)
    admins=[open_conn(0,a),open_conn(1,a)]
    run(admins[1],f'CREATE DATABASE {b}',1);created_dbs[1].append(b)
    others=[open_conn(side,a) for side in (0,1)]
    cross=[open_conn(side,b) for side in (0,1)]
    fixtures=[
        ('s','bigint','START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE',['20','7','3','1','100','1','f']),
        ('i','integer','START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 2 CYCLE',['23','7','3','1','100','2','t']),
        ('h','smallint','START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE',['21','7','3','1','100','1','f']),
        ('default_s','','',['20','1','1','1',str(2**63-1),'1','f']),
        ('down','smallint','INCREMENT BY -2',['21','-1','-2',str(-2**15),'-1','1','f']),
        ('edge','bigint',f'START WITH {2**63-1} MINVALUE {2**63-3} MAXVALUE {2**63-1} CACHE 1',['20',str(2**63-1),'1',str(2**63-3),str(2**63-1),'1','f']),
        ('edge_down','bigint',f'START WITH {-2**63} INCREMENT BY -1 MINVALUE {-2**63} MAXVALUE {-2**63+2} CACHE 1',['20',str(-2**63),'-1',str(-2**63),str(-2**63+2),'1','f']),
    ]
    for side,conn in enumerate(admins):
        record.append(dict(side=side,version=run(conn,'SELECT version()'),server_version_num=serverversion(conn),path=run(conn,'SHOW search_path')))
        for name,kind,options,expected in fixtures:
            run(conn,f'CREATE SEQUENCE public.{name} '+(f'AS {kind} ' if kind else '')+options,1)
        run(conn,'CREATE TABLE public.ia (id integer GENERATED ALWAYS AS IDENTITY)',1)
        run(conn,'CREATE TABLE public.ib (id bigint GENERATED BY DEFAULT AS IDENTITY)',1)
    for name,kind,options,expected in fixtures:
        pair=contract_pair(f"SELECT {projection} FROM pg_catalog.pg_sequence WHERE seqrelid='public.{name}'::regclass::oid",param_fields)
        assert all(text(out['rows'][0])==expected for out in pair),(name,pair,expected)
        for side,conn in enumerate(admins):
            assert params(others[side],name)==expected
        count+=1
    # Relation IDs are different across servers, but each must match pg_class.
    for side,conn in enumerate(admins):
        oid=int(scalar(conn,"SELECT oid FROM pg_class WHERE relname='s'"))
        statement=b'parameters';sql=f'SELECT seqrelid,{projection} FROM pg_catalog.pg_sequence WHERE seqrelid=$1'
        result=observe(prepare(conn,statement,sql.encode(),1,(c.c_uint*1)(26)))
        record.append(dict(side=side,sql=sql,phase='Parse',result=result));assert result['status']==1,result
        desc=observe(describe(conn,statement));check_metadata(desc,sequence_fields);assert desc['parameter_oids']==[26],desc
        record.append(dict(side=side,sql=sql,phase='Describe statement',result=desc))
        for fmt in (0,1):
            for argument in (oid,0,None):
                data=None if argument is None else (str(argument).encode() if fmt==0 else struct.pack('!I',argument))
                out=observe(prepared(conn,statement,1,(s*1)(data),(i*1)(len(data) if data else 0),(i*1)(fmt),fmt))
                record.append(dict(side=side,sql=sql,phase='Bind Execute Sync',parameters=[None if data is None else data.hex()],parameter_oids=[26],format=fmt,result=out,transaction=transaction(conn)))
                assert out['status']==2 and transaction(conn)==0,out
                check_metadata(out,sequence_fields)
                assert [dict(f,format=0) for f in out['fields']]==desc['fields'],out
                assert len(out['rows'])==(1 if argument==oid else 0),out
                if argument==oid:
                    cell=bytes.fromhex(out['rows'][0][0]);assert (int(cell) if fmt==0 else int.from_bytes(cell,'big'))==oid
                count+=1
        # Identity mapping consumes a real association, independent of generated names.
        got=run(conn,'SELECT seqtypid,seqmax FROM pg_catalog.pg_sequence ORDER BY seqrelid')
        values=[text(row) for row in got['rows']]
        assert ['23',str(2**31-1)] in values and ['20',str(2**63-1)] in values,values
        assert run(cross[side],f'SELECT seqrelid FROM pg_catalog.pg_sequence WHERE seqrelid={oid}')['rows']==[]
        # A fresh connection stays uninitialized after all catalog operations.
        assert params(others[side],'s')==fixtures[0][3]
        step(others[side],"SELECT currval('public.s'::regclass)",7,0,'55000')
        assert scalar(conn,"SELECT nextval('public.s'::regclass)")=='7'
        for _ in range(2):assert params(conn,'s')==fixtures[0][3]
        assert scalar(conn,"SELECT currval('public.s'::regclass)")=='7'
        assert scalar(conn,"SELECT nextval('public.s'::regclass)")=='10'
        # Full bigint endpoints are consumed by the existing allocator without wrap.
        for name,expected in [('edge',str(2**63-1)),('edge_down',str(-2**63))]:
            assert scalar(conn,f"SELECT nextval('public.{name}'::regclass)")==expected
            step(conn,f"SELECT nextval('public.{name}'::regclass)",7,0,'2200H')
        count+=7
        run(conn,f'GRANT USAGE ON SCHEMA public TO {reader}',1)
        low=open_conn(side,a,reader)
        assert params(low,'s')==fixtures[0][3]
        step(low,"SELECT nextval('public.s'::regclass)",7,0,'42501')
        low_sql="SELECT nextval('public.s'::regclass)"
        parsed=observe(prepare(low,b'low_call',low_sql.encode(),0,None))
        record.append(dict(side=side,sql=low_sql,phase='Parse authenticated permission probe',result=parsed))
        assert parsed['status']==1,parsed
        denied=observe(prepared(low,b'low_call',0,None,None,None,0))
        record.append(dict(side=side,sql=low_sql,phase='Execute authenticated permission probe',result=denied,transaction=transaction(low)))
        assert denied['status']==7 and denied['errors']['C']=='42501' and transaction(low)==0,denied
        assert scalar(low,'SELECT 1')=='1'
        count+=2
        for name,options in [('bad_bounds','AS integer MINVALUE 10 MAXVALUE 1'),('bad_start','AS integer START WITH 11 MINVALUE 1 MAXVALUE 10'),('bad_type','AS smallint MAXVALUE 32768'),('bad_decl','AS text'),('bad_inc','AS integer INCREMENT BY 0'),('bad_cache','AS bigint CACHE 0')]:
            step(conn,f'CREATE SEQUENCE public.{name} {options}',7,0,'22023')
            assert run(conn,f"SELECT oid FROM pg_class WHERE relname='{name}'")['rows']==[]
            assert scalar(conn,'SELECT 1')=='1';count+=1
        # Native AUTO_INCREMENT is not a sequence; compare only on the native side.
        if side==1:
            assert run(conn,"SELECT oid FROM pg_class WHERE relname='native_auto' AND relkind='S'")['rows']==[]
            assert run(conn,"SELECT s.seqrelid FROM pg_sequence s JOIN pg_class c ON s.seqrelid=c.oid WHERE c.relname='native_auto'")['rows']==[]
        # Refuse unsupported catalog execution at its observed stage, retaining the reference error.
        step(conn,'SELECT unsupported_sequence(seqrelid) FROM pg_catalog.pg_sequence',7,0,'42883' if side==0 else '0A000')
        step(conn,'DELETE FROM pg_catalog.pg_sequence WHERE false',1 if side==0 else 7,0,None if side==0 else '0A000')
        count+=2
        bad_sql='SELECT missing_field FROM pg_catalog.pg_sequence'
        rejected=observe(prepare(conn,b'bad_catalog',bad_sql.encode(),0,None))
        record.append(dict(side=side,sql=bad_sql,phase='Parse reject / Sync',result=rejected,transaction=transaction(conn)))
        assert rejected['status']==7 and rejected['errors']['C']==('42703' if side==0 else '0A000') and transaction(conn)==0,rejected
        assert scalar(conn,'SELECT 1')=='1';count+=1
        step(conn,'BEGIN',1,2)
        step(conn,'SELECT missing_field FROM pg_catalog.pg_sequence',7,3,'42703' if side==0 else '0A000')
        step(conn,'ROLLBACK',1,0)
        assert params(conn,'s')==fixtures[0][3]
        assert scalar(conn,"SELECT currval('public.s'::regclass)")=='10';count+=1
    print(f'pg13_027 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} types parameters origins text binary zero_rows identity no_allocation endpoints authentication isolation recovery',flush=True)

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
    (evidence/'sequence-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not failed,'fixture cleanup failed; see evidence'
