# Copyright 2026 AsterSQL.
"""CHECK and NOT NULL contract; independent libpq against fixed PostgreSQL 18.6."""
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
    for conn in cross: run(conn,'CREATE TABLE public.check_doc (code integer)',1)
    for conn in admins:
        record.append(dict(side=connection_sides[conn],full_version=run(conn,'SELECT version()'),path=run(conn,'SHOW search_path')))
        run(conn,'CREATE TABLE public.check_doc (v integer NOT NULL, x integer, CONSTRAINT positive CHECK (v > 0), CONSTRAINT constant_check CHECK (1=1), CONSTRAINT multi_check CHECK (x < v AND v > 0))',1)
    sql="SELECT conname,contype,conkey FROM pg_catalog.pg_constraint WHERE conrelid='public.check_doc'::regclass::oid ORDER BY conname"
    types=[(19,64,-1),(18,1,-1),(1005,-1,-1)]
    got=contract_pair(sql,types)
    expected=[['check_doc_v_not_null','n','{1}'],['constant_check','c',None],['multi_check','c','{2,1}'],['positive','c','{1}']]
    assert [text(r) for r in got[0]['rows']]==expected,got
    assert all(not r['rows'] for r in contract_pair(sql.replace("conrelid='public.check_doc'::regclass::oid",'oid=0'),types))
    defs=[]
    for side,conn in enumerate(admins):
        assert run(others[side],sql)['rows']==run(conn,sql)['rows']
        identity=run(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE conname='positive'")
        oid=text(identity['rows'][0])[0]
        # Real OID parameters, NULL and unknown; full statement metadata per side.
        q='SELECT pg_catalog.pg_get_constraintdef($1),pg_catalog.pg_get_constraintdef($1,false),pg_catalog.pg_get_constraintdef($1,true)'
        prepared_def=observe(prepare(conn,b'ck_def',q.encode(),1,(c.c_uint*1)(26)))
        record.append(dict(side=side,phase='Parse',sql=q,result=prepared_def))
        assert prepared_def['status']==1,prepared_def
        desc=observe(describe(conn,b'ck_def'));check_metadata(desc,[(25,-1,-1)]*3)
        record.append(dict(side=side,phase='Describe statement',sql=q,result=desc))
        for arg in (oid,'0',None):
            for fmt in (0,1):
                params=(s*1)(arg.encode() if arg is not None else None)
                out=observe(prepared(conn,b'ck_def',1,params,None,None,fmt))
                record.append(dict(side=side,phase='Execute/Sync',sql=q,parameters=[arg],format=fmt,result=out,transaction=transaction(conn)))
                assert out['status']==2 and transaction(conn)==0,out
                check_metadata(out,[(25,-1,-1)]*3)
                if arg!=oid: assert out['rows']==[[None,None,None]],out
                else:
                    values=text(out['rows'][0]);assert all(values),out
                    if fmt==0: defs.append((side,values))
        wire=Wire(conn)
        try:
            wire.send(b'P',b'def_portal_stmt\0'+q.encode()+b'\0'+struct.pack('!hI',1,26))
            wire.send(b'B',b'def_portal\0def_portal_stmt\0'+struct.pack('!hhi',0,1,len(oid))+oid.encode()+struct.pack('!h',0))
            wire.send(b'D',b'Pdef_portal\0');wire.send(b'H')
            portal=wire.described()
            body=bytes.fromhex(portal[-1][1]);offset=2;fields=[]
            for _ in range(struct.unpack('!h',body[:2])[0]):
                end=body.index(0,offset)
                table,attribute,foid,size,mod,fmt=struct.unpack('!IhIhih',body[end+1:end+19])
                fields.append(dict(name=body[offset:end].decode(),table=table,attribute=attribute,oid=foid,size=size,mod=mod,format=fmt));offset=end+19
            assert fields==desc['fields'],(fields,desc)
            wire.send(b'E',b'def_portal\0'+struct.pack('!I',0));wire.send(b'S')
            result=wire.ready();assert sum(t=='D' for t,_ in result)==1 and all(t!='E' for t,_ in result),result
            record.append(dict(side=side,sql=q,parameters=[oid],phase='Describe portal / Execute / Sync',fields=fields,result=result))
            wire.send(b'C',b'Pdef_portal\0');wire.send(b'C',b'Sdef_portal_stmt\0');wire.send(b'S')
            assert ''.join(t for t,_ in wire.ready())=='33Z'
        finally:
            wire.close()
        nn=text(run(conn,"SELECT pg_catalog.pg_get_constraintdef(oid) FROM pg_catalog.pg_constraint WHERE contype='n' AND conrelid='public.check_doc'::regclass::oid")['rows'][0])[0]
        assert nn.replace('"','')=='NOT NULL v',nn
        bad=run(conn,'SELECT pg_catalog.pg_get_constraintdef(0,false,0)',7)
        assert bad['errors']['C']==('42883' if side==0 else '0A000'),bad
        run(conn,'SELECT 1')
        # Every returned CHECK fragment must reconstruct executable behavior.
        for n,fragment in enumerate(defs[-1][1]):
            run(conn,f'CREATE TABLE public.clone_{n} (v integer, CONSTRAINT clone_ck {fragment})',1)
            bad=run(conn,f'INSERT INTO public.clone_{n} VALUES (0)',7)
            assert bad['errors']['C']=='23514',bad
            run(conn,f'INSERT INTO public.clone_{n} VALUES (NULL)',1)
            run(conn,f'INSERT INTO public.clone_{n} VALUES (1)',1)
            run(conn,f'DROP TABLE public.clone_{n}',1)
        run(conn,'INSERT INTO public.check_doc VALUES (2,1)',1)
        bad=run(conn,'INSERT INTO public.check_doc VALUES (0,1)',7);assert bad['errors']['C']=='23514',bad
        bad=run(conn,'INSERT INTO public.check_doc VALUES (NULL,1)',7);assert bad['errors']['C']=='23502',bad
        # Reference has a genuine node tree. Product rejects the unavailable tree.
        tree=run(conn,"SELECT conbin FROM pg_catalog.pg_constraint WHERE conname='positive'",2 if side==0 else 7)
        if side==0: assert tree['rows'][0][0] is not None and tree['fields'][0]['oid']==194,tree
        else: assert tree['errors']['C']=='0A000',tree
        run(conn,'SELECT 1')
        run(conn,'ALTER TABLE public.check_doc RENAME COLUMN v TO renamed',1)
        assert text(run(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE conname='positive'")['rows'][0])[0]==oid
        definition=text(run(others[side],f'SELECT pg_catalog.pg_get_constraintdef({oid}::oid)')['rows'][0])[0]
        assert 'renamed' in definition,definition
        bad=run(conn,'INSERT INTO public.check_doc VALUES (0,1)',7);assert bad['errors']['C']=='23514',bad
        bad=run(conn,'UPDATE public.check_doc SET renamed=0',7);assert bad['errors']['C']=='23514',bad
        nn_oid=text(run(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE contype='n' AND conrelid='public.check_doc'::regclass::oid")['rows'][0])[0]
        run(conn,'ALTER TABLE public.check_doc ALTER COLUMN renamed DROP NOT NULL',1)
        assert run(conn,f'SELECT pg_catalog.pg_get_constraintdef({nn_oid}::oid)')['rows']==[[None]]
        run(conn,'ALTER TABLE public.check_doc ALTER COLUMN renamed SET NOT NULL',1)
        new_nn=text(run(conn,"SELECT oid FROM pg_catalog.pg_constraint WHERE contype='n' AND conrelid='public.check_doc'::regclass::oid")['rows'][0])[0]
        assert new_nn!=nn_oid,(new_nn,nn_oid)
        # Explicit privilege facts and real authenticated connections.
        run(conn,f'GRANT CONNECT ON DATABASE {a} TO {reader}',1)
        run(conn,f'GRANT USAGE ON SCHEMA public TO {reader}',1)
        low=open_conn(side,a,reader)
        assert run(low,sql)['rows']==run(conn,sql)['rows']
        denied=run(low,'SELECT * FROM public.check_doc',7);assert denied['errors']['C']=='42501',denied
        assert not run(cross[side],sql)['rows']
        for query,expected_status,expected_tx,expected_state in [
            ('BEGIN',1,2,None),
            ('INSERT INTO public.check_doc VALUES (0,1)',7,3,'23514'),
            ('SELECT 1',7,3,'25P02'),
            ('ROLLBACK',1,0,None),
            ('SELECT 1',2,0,None),
        ]:
            out=observe(execute(conn,query.encode()))
            record.append(dict(side=side,sql=query,phase='transaction error and recovery',result=out,transaction=transaction(conn)))
            assert out['status']==expected_status and transaction(conn)==expected_tx,out
            if expected_state: assert out['errors']['C']==expected_state,out
        run(conn,'DROP TABLE public.check_doc',1)
        gone=run(conn,f'SELECT pg_catalog.pg_get_constraintdef({oid}::oid)');assert gone['rows']==[[None]],gone
        count+=10
    # Quoted column names are rendered from bound identities, never the native text.
    for conn in admins:
        run(conn,'CREATE TABLE public."Con Mixed" ("a""b" integer, CONSTRAINT "ck""q" CHECK ("a""b">0))',1)
        d=text(run(conn,"SELECT pg_catalog.pg_get_constraintdef(oid) FROM pg_catalog.pg_constraint WHERE conname='ck\"q'")['rows'][0])[0]
        assert '"a""b"' in d,d
        run(conn,f'CREATE TABLE public.quoted_clone ("a""b" integer, CONSTRAINT copy_ck {d})',1)
        bad=run(conn,'INSERT INTO public.quoted_clone VALUES (0)',7);assert bad['errors']['C']=='23514',bad
        run(conn,'DROP TABLE public.quoted_clone',1);run(conn,'DROP TABLE public."Con Mixed"',1)
    count+=1
    print(f'pg13_020 differential: scenarios={count} observations={len(record)} PostgreSQL=180006 libpq={libversion()} CHECK NOTNULL DEF UNKNOWN QUOTE ACL isolation recovery cleanup',flush=True)

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
    (evidence/'check-contract.json').write_text(json.dumps(record,ensure_ascii=False,indent=2))
    assert not cleanup_failed, 'fixture cleanup failed; see evidence'
