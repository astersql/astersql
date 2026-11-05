# Copyright 2026 AsterSQL.
"""Sequence OWNED BY dependency and lifecycle contract; independent libpq against fixed PostgreSQL 18.6."""
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
    if expected==1:
        words=sql.upper().split()
        expected_tag=' '.join(words[:2]) if words[0] in ('CREATE','ALTER','DROP') else words[0]
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
        return [dict(f,table=f['table'] if f['table'] in (0,1249,2604,1259,2615,2224,2608) else origin_identities[side][f['table']]) for f in fields]
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

projection='classid,objid,objsubid,refclassid,refobjid,refobjsubid,deptype'
dep_fields=[(26,4,-1),(26,4,-1),(23,4,-1),(26,4,-1),(26,4,-1),(23,4,-1),(18,1,-1)]
dep_query="SELECT "+projection+" FROM pg_catalog.pg_depend WHERE objid='own.s'::regclass::oid ORDER BY refclassid,refobjid,refobjsubid"

def dependencies(conn, name='own.s'):
    out=run(conn,"SELECT "+projection+f" FROM pg_catalog.pg_depend WHERE objid='{name}'::regclass::oid ORDER BY refclassid,refobjid,refobjsubid")
    check_metadata(out,dep_fields)
    return [text(row) for row in out['rows']]

def endpoints(conn, owned=True, table='t', column='id', seq='s', number='2'):
    global count
    sid=scalar(conn,f"SELECT oid FROM pg_catalog.pg_class WHERE relname='{seq}' AND relkind='S'")
    tid=scalar(conn,f"SELECT oid FROM pg_catalog.pg_class WHERE relname='{table}' AND relkind='r'")
    ns=scalar(conn,"SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='own'")
    got=dependencies(conn,'own.'+seq)
    assert ['1259',sid,'0','2615',ns,'0','n'] in got,got
    edge=['1259',sid,'0','1259',tid,number,'a']
    assert (edge in got)==owned,got
    if owned:
        assert scalar(conn,f"SELECT attnum FROM pg_catalog.pg_attribute WHERE attrelid='{('own.'+table)}'::regclass::oid AND attname='{column}'")==number
    count+=1
    return sid,tid

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
    identities=[]
    for side,conn in enumerate(admins):
        record.append(dict(side=side,version=run(conn,'SELECT version()'),server_version_num=serverversion(conn),path=run(conn,'SHOW search_path')))
        for sql in [
            'CREATE SCHEMA own',
            'CREATE TABLE own.t (gone integer, id bigint, plain bigint)',
            'CREATE SEQUENCE own.s START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE OWNED BY own.t.id',
            'CREATE SEQUENCE own.unowned',
            'CREATE TABLE own.identity_table (id integer GENERATED ALWAYS AS IDENTITY, plain integer)',
        ]: run(conn,sql,1)
        identities.append(endpoints(conn))
        for invalid in [
            'CREATE SEQUENCE own.bad_owned OWNED BY own.t.missing',
            'ALTER SEQUENCE own.s OWNED BY own.t.missing',
        ]:
            assert run(conn,invalid,7)['errors']['C']=='42703'
            assert run(conn,"SELECT oid FROM pg_catalog.pg_class WHERE relname='bad_owned'")['rows']==[]
            count+=1
        run(conn,'CREATE SCHEMA other_owner',1)
        run(conn,'CREATE TABLE other_owner.t (id bigint)',1)
        for invalid in [
            'CREATE SEQUENCE own.bad_owned OWNED BY other_owner.t.id',
            'ALTER SEQUENCE own.s OWNED BY other_owner.t.id',
        ]:
            assert run(conn,invalid,7)['errors']['C']=='55000'
            assert run(conn,"SELECT oid FROM pg_catalog.pg_class WHERE relname='bad_owned'")['rows']==[]
            count+=1
        run(conn,'DROP TABLE other_owner.t',1)
        run(conn,'DROP SCHEMA other_owner',1)
        # Default nextval reference creates no ownership edge. This product branch is explicitly gated.
        out=run(conn,"CREATE TABLE own.default_only (id bigint DEFAULT nextval('own.unowned'::regclass))",1 if side==0 else 7)
        if side==1: assert out['errors']['C']=='0A000',out
        assert not any(row[-1] in ('a','i') for row in dependencies(conn,'own.unowned'))
        count+=1
    pair=contract_pair(dep_query,dep_fields,False)
    for side,conn in enumerate(admins):
        sql='SELECT '+projection+' FROM pg_catalog.pg_depend WHERE objid=$1 ORDER BY refclassid,refobjid,refobjsubid'
        stmt=b'owned_parameters'
        out=observe(prepare(conn,stmt,sql.encode(),1,(c.c_uint*1)(26)))
        record.append(dict(side=side,sql=sql,phase='Parse',result=out));assert out['status']==1,out
        out=observe(describe(conn,stmt));assert out['parameter_oids']==[26]
        assert out['fields']==pair[side]['fields'],out
        record.append(dict(side=side,sql=sql,phase='Describe statement',result=out))
        for value_ in (identities[side][0],None,'4294967295'):
            for fmt in (0,1):
                values=(s*1)(None if value_ is None else value_.encode())
                out=observe(prepared(conn,stmt,1,values,None,None,fmt))
                record.append(dict(side=side,sql=sql,parameters=[value_],parameter_oids=[26],phase='Bind Execute Sync',format=fmt,result=out,transaction=transaction(conn)))
                assert out['status']==2 and transaction(conn)==0,out
                assert [dict(f,format=0) for f in out['fields']]==pair[side]['fields'],out
                assert len(out['rows'])==(2 if value_==identities[side][0] else 0),out
                if value_==identities[side][0] and fmt==0: assert out==pair[side],out
                count+=1
        step(conn,'BEGIN',1,2)
        step(conn,dep_query,2,2)
        step(conn,'SELECT missing_field FROM pg_catalog.pg_depend',7,3)
        step(conn,'SELECT 1',7,3,'25P02')
        step(conn,'ROLLBACK',1,0)
        assert scalar(conn,'SELECT 1')=='1'
        count+=1
    for side,conn in enumerate(admins):
        assert scalar(conn,"SELECT nextval('own.s'::regclass)")=='7'
        endpoints(others[side])
        assert scalar(conn,"SELECT currval('own.s'::regclass)")=='7'
        assert run(others[side],"SELECT currval('own.s'::regclass)",7)['errors']['C']=='55000'
        assert scalar(conn,"SELECT nextval('own.s'::regclass)")=='10'
        count+=1
        # Re-execute the same prepared dependency query on a second connection.
        stmt=b'live_dependencies'
        assert observe(prepare(others[side],stmt,dep_query.encode(),0,None))['status']==1
        run(conn,'ALTER SEQUENCE own.s OWNED BY NONE',1)
        endpoints(others[side],False)
        out=observe(prepared(others[side],stmt,0,None,None,None,0))
        record.append(dict(side=side,sql=dep_query,phase='new Execute after OWNED BY NONE',result=out))
        assert out['status']==2 and all(text(row)[-1]!='a' for row in out['rows']),out
        run(conn,'ALTER SEQUENCE own.s OWNED BY own.t.id',1)
        endpoints(others[side])
        run(conn,'ALTER TABLE own.t DROP COLUMN gone',1)
        endpoints(conn)
        run(conn,'ALTER TABLE own.t RENAME COLUMN id TO renamed',1)
        run(conn,'ALTER TABLE own.t RENAME TO renamed_t',1)
        run(conn,'ALTER SEQUENCE own.s RENAME TO renamed_s',1)
        assert endpoints(others[side],table='renamed_t',column='renamed',seq='renamed_s')==identities[side]
        # Detach/rebind validates endpoints without corrupting the existing edge.
        out=run(conn,'ALTER SEQUENCE own.renamed_s OWNED BY own.renamed_t.missing',7)
        assert out['errors']['C']=='42703',out
        endpoints(conn,table='renamed_t',column='renamed',seq='renamed_s')
        run(conn,'ALTER TABLE own.renamed_t DROP COLUMN renamed',1)
        assert run(conn,f"SELECT objid FROM pg_catalog.pg_depend WHERE objid={identities[side][0]}")['rows']==[]
        run(conn,'CREATE SEQUENCE own.renamed_s OWNED BY own.renamed_t.plain',1)
        fresh=scalar(conn,"SELECT oid FROM pg_catalog.pg_class WHERE relname='renamed_s'")
        assert fresh!=identities[side][0]
        run(conn,'DROP SEQUENCE own.renamed_s',1)
        run(conn,'CREATE SEQUENCE own.drop_table OWNED BY own.renamed_t.plain',1)
        dropped=scalar(conn,"SELECT oid FROM pg_catalog.pg_class WHERE relname='drop_table'")
        run(conn,'DROP TABLE own.renamed_t',1)
        assert run(conn,f'SELECT objid FROM pg_catalog.pg_depend WHERE objid={dropped}')['rows']==[]
        count+=5
        # Internal identity edge and direct-drop protection consume durable SequenceID.
        tid=scalar(conn,"SELECT oid FROM pg_catalog.pg_class WHERE relname='identity_table'")
        edges=run(conn,f"SELECT objid FROM pg_catalog.pg_depend WHERE classid=1259 AND refclassid=1259 AND refobjid={tid} AND refobjsubid=1 AND deptype='i'")
        assert len(edges['rows'])==1,edges
        internal=text(edges['rows'][0])[0]
        name=scalar(conn,f'SELECT relname FROM pg_catalog.pg_class WHERE oid={internal}')
        out=run(conn,f'DROP SEQUENCE own."{name}"',7)
        assert out['errors']['C']=='2BP01',out
        run(conn,'ALTER TABLE own.identity_table DROP COLUMN id',1)
        assert run(conn,f'SELECT objid FROM pg_catalog.pg_depend WHERE objid={internal}')['rows']==[]
        count+=1
        # Real authenticated reader can enumerate dependency facts but cannot allocate.
        run(conn,f'GRANT USAGE ON SCHEMA own TO {reader}',1)
        run(conn,'CREATE TABLE own.reader_table (id bigint)',1)
        run(conn,'CREATE SEQUENCE own.reader_seq OWNED BY own.reader_table.id',1)
        low=open_conn(side,a,reader)
        assert len(dependencies(low,'own.reader_seq'))>=2
        assert run(low,"SELECT nextval('own.reader_seq'::regclass)",7)['errors']['C']=='42501'
        assert run(cross[side],"SELECT oid FROM pg_catalog.pg_class WHERE relname='reader_seq'")['rows']==[]
        count+=2
    for sql in [
        'SELECT '+projection+' FROM pg_catalog.pg_depend WHERE objid=NULL',
        'SELECT '+projection+' FROM pg_catalog.pg_depend WHERE objid=4294967295',
    ]: contract_pair(sql,dep_fields)
    for sql in ['SELECT missing_field FROM pg_catalog.pg_depend',
                'SELECT unsupported_dependency(objid) FROM pg_catalog.pg_depend',
                'DELETE FROM pg_catalog.pg_depend']:
        for side,conn in enumerate(admins):
            if sql.startswith('DELETE'):
                # A PostgreSQL superuser may modify this per-database catalog.
                # Record that result inside a transaction and roll it back.
                step(conn,'BEGIN',1,2)
                step(conn,sql,1 if side==0 else 7,2 if side==0 else 3,None if side==0 else '0A000')
                step(conn,'ROLLBACK',1,0)
            else:
                out=run(conn,sql,7)
                if side==1: assert out['errors']['C']=='0A000',out
            assert scalar(conn,'SELECT 1')=='1'
            count+=1
    for side,conn in enumerate(admins):
        if side==0:
            run(conn,'DROP SCHEMA own CASCADE',1)
        else:
            # Schema CASCADE is an existing capability gate, independent of owned-object drops.
            before=dependencies(conn,'own.reader_seq')
            assert run(conn,'DROP SCHEMA own CASCADE',7)['errors']['C']=='0A000'
            assert dependencies(conn,'own.reader_seq')==before
            for sql in ['DROP TABLE own.reader_table','DROP TABLE own.identity_table','DROP SEQUENCE own.unowned','DROP SCHEMA own']:
                run(conn,sql,1)
        assert run(conn,"SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='own'")['rows']==[]
    print(f'pg13_028 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} ownership endpoints lifecycle stable_slots text binary descriptions fresh_execute authentication isolation no_allocation recovery',flush=True)

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
    (evidence/'sequence-owned-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not failed,'fixture cleanup failed; see evidence'
