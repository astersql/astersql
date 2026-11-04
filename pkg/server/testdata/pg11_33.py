# Copyright 2026 AsterSQL.
"""Sequence differential over real libpq connections and shared server listeners.
Q uses PQexec; extended uses PQexecParams and named prepared statements.
Only Close uses the authenticated idle socket, drained through Sync.
Arguments: target TCP port, target database.
"""
import copy
import ctypes as c
import ctypes.util
import json
import os
import socket
import struct
import sys
from pathlib import Path

archive = Path(os.environ["PG11_33_EVIDENCE_DIR"])
archive.mkdir(parents=True, exist_ok=False)
output = (archive / "observations.jsonl").open("w")
def record(data):
    print(json.dumps(data), file=output, flush=True)
    print(json.dumps(data), flush=True)

library = os.environ.get('PG_LIBPQ_LIBRARY') or ctypes.util.find_library('pq')
assert library, 'PG_LIBPQ_LIBRARY/libpq required (no skip)'
pq = c.CDLL(library)
def api(name, result, *args):
    f = getattr(pq, name)
    f.restype, f.argtypes = result, args
    return f
connect = api('PQconnectdb', c.c_void_p, c.c_char_p)
status = api('PQstatus', c.c_int, c.c_void_p)
finish = api('PQfinish', None, c.c_void_p)
fd = api('PQsocket', c.c_int, c.c_void_p)
assert api('PQlibVersion', c.c_int)() >= 180000, 'libpq 18 required'
exec_query = api('PQexec', c.c_void_p, c.c_void_p, c.c_char_p)
exec_params = api('PQexecParams', c.c_void_p, c.c_void_p, c.c_char_p,
                  c.c_int, c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p, c.c_int)
prepare_query = api('PQprepare', c.c_void_p, c.c_void_p, c.c_char_p, c.c_char_p, c.c_int, c.c_void_p)
describe_query = api('PQdescribePrepared', c.c_void_p, c.c_void_p, c.c_char_p)
execute_prepared = api('PQexecPrepared', c.c_void_p, c.c_void_p, c.c_char_p, c.c_int, c.c_void_p, c.c_void_p, c.c_void_p, c.c_int)
result_status = api('PQresultStatus', c.c_int, c.c_void_p)
ntuples = api('PQntuples', c.c_int, c.c_void_p)
nfields = api('PQnfields', c.c_int, c.c_void_p)
fname = api('PQfname', c.c_char_p, c.c_void_p, c.c_int)
ftype = api('PQftype', c.c_uint, c.c_void_p, c.c_int)
fsize = api('PQfsize', c.c_int, c.c_void_p, c.c_int)
fmod = api('PQfmod', c.c_int, c.c_void_p, c.c_int)
fformat = api('PQfformat', c.c_int, c.c_void_p, c.c_int)
ftable = api('PQftable', c.c_uint, c.c_void_p, c.c_int)
ftablecol = api('PQftablecol', c.c_int, c.c_void_p, c.c_int)
getvalue = api('PQgetvalue', c.c_void_p, c.c_void_p, c.c_int, c.c_int)
getlength = api('PQgetlength', c.c_int, c.c_void_p, c.c_int, c.c_int)
getisnull = api('PQgetisnull', c.c_int, c.c_void_p, c.c_int, c.c_int)
cmdstatus = api('PQcmdStatus', c.c_char_p, c.c_void_p)
error_field = api('PQresultErrorField', c.c_char_p, c.c_void_p, c.c_int)
transaction_status = api('PQtransactionStatus', c.c_int, c.c_void_p)
nparams = api('PQnparams', c.c_int, c.c_void_p)
paramtype = api('PQparamtype', c.c_uint, c.c_void_p, c.c_int)
clear = api('PQclear', None, c.c_void_p)
pq_trace = api('PQtrace', None, c.c_void_p, c.c_void_p)
pq_untrace = api('PQuntrace', None, c.c_void_p)
trace_flags = api('PQsetTraceFlags', None, c.c_void_p, c.c_int)
notice_receiver = api('PQsetNoticeReceiver', c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p)
libc = c.CDLL(None)
libc.fopen.restype, libc.fopen.argtypes = c.c_void_p, [c.c_char_p,c.c_char_p]
libc.fclose.restype, libc.fclose.argtypes = c.c_int, [c.c_void_p]
libc.fflush.restype, libc.fflush.argtypes = c.c_int, [c.c_void_p]
fflush = libc.fflush
connection_id = 0

def diagnostics(result):
    return {field: value.decode() for field in 'SVCMPDHFLR'
            if (value := error_field(result,ord(field))) is not None}
mysql_port = int(os.environ['PG11_MYSQL_PORT'])
assert 0 < mysql_port < 65536
reference = os.environ.get('PG18_REFERENCE_DSN')
assert reference, 'PG18_REFERENCE_DSN required (no skip)'
target = os.environ.get('PG11_TARGET_DSN') or (
    f'host=127.0.0.1 port={int(sys.argv[1])} dbname={sys.argv[2]} user=root sslmode=disable')

class Client:
    def __init__(self, dsn):
        self.conn = connect(dsn.encode())
        assert self.conn and status(self.conn) == 0, 'libpq connection failed (DSN redacted)'
        self.blocking = os.get_blocking(fd(self.conn))
        self.sock = socket.socket(fileno=os.dup(fd(self.conn)))
        self.sock.settimeout(10)
        global connection_id
        connection_id += 1
        self.trace_path = archive / f'libpq-{connection_id}.trace'
        self.trace = libc.fopen(os.fsencode(self.trace_path),b'w')
        assert self.trace, 'cannot open libpq trace'
        self.trace_offset = 0
        pq_trace(self.conn,self.trace)
        trace_flags(self.conn,1)  # PQTRACE_SUPPRESS_TIMESTAMPS
        self.linked = {}
        self.notices = []
        @c.CFUNCTYPE(None,c.c_void_p,c.c_void_p)
        def receive_notice(arg,result):
            self.notices.append(diagnostics(result))
        self.receive_notice = receive_notice
        notice_receiver(self.conn,receive_notice,None)
    def close(self):
        if self.conn:
            pq_untrace(self.conn)
            assert libc.fclose(self.trace) == 0
            self.sock.close()
            os.set_blocking(fd(self.conn), self.blocking)
            finish(self.conn)
            self.conn = None
    def send(self, tag, body):
        self.sock.sendall(tag + struct.pack('!I', len(body) + 4) + body)
    def exact(self, n):
        out = b''
        while len(out) < n:
            part = self.sock.recv(n-len(out))
            assert part, 'unexpected EOF'
            out += part
        return out
    def run(self, sql, ext=False, operation=None):
        self.notices.clear()
        result = operation() if operation else (exec_params(self.conn, sql.encode(), 0, None, None, None, None, 0)
                  if ext else exec_query(self.conn, sql.encode()))
        assert result, 'libpq returned no result'
        try:
            code = result_status(result)
            assert code in (1, 2, 7), f'unexpected libpq result status {code}'
            o = dict(rows=[], metadata=[], origins=[], errors=[],
                     warnings=list(self.notices), command=[], ready='',
                     phase='parse' if ext else 'query', parameters=[paramtype(result,i) for i in range(nparams(result))])
            for col in range(nfields(result)):
                o['metadata'].append([fname(result,col).decode(), ftype(result,col),
                                      fsize(result,col), fmod(result,col), fformat(result,col)])
                o['origins'].append([ftable(result,col), ftablecol(result,col)])
            for row in range(ntuples(result)):
                o['rows'].append([
                    None if getisnull(result,row,col) else
                    c.string_at(getvalue(result,row,col), getlength(result,row,col)).decode()
                    for col in range(nfields(result))])
            if code == 7:
                o['errors'].append(diagnostics(result))
            else:
                command = cmdstatus(result).decode()
                if command: o['command'].append(command)
            fflush(self.trace)
            with self.trace_path.open() as stream:
                stream.seek(self.trace_offset)
                trace = stream.read()
                self.trace_offset = stream.tell()
            # Trace is produced by libpq itself, not inferred by replaying failed
            # SQL in a different transaction. Preserve it alongside PQresult fields.
            o['wire_trace'] = trace
            ready = None
            pending = 'query'
            description = None
            for line in trace.splitlines():
                fields = line.split('\t')
                if len(fields) < 3: continue
                tag = fields[2]
                if fields[0] == 'F':
                    # Frontend pipelines contain later messages before the first
                    # response. First request establishes the initial phase.
                    if pending == 'query':
                        pending = {'Parse':'parse','Bind':'bind','Describe':'describe-statement'}.get(tag,'query')
                        o['phase'] = pending
                    if tag == 'Describe':
                        description = 'Describe(statement)' if fields[3].strip().startswith('S ') else 'Describe(portal)'
                elif fields[0] == 'B':
                    if tag == 'ParseComplete': o['phase'] = 'bind'
                    elif tag == 'BindComplete': o['phase'] = 'describe-portal'
                    elif tag in ('RowDescription','NoData'):
                        if tag == 'RowDescription': o['description_source'] = description or 'Query'
                        o['phase'] = 'execute' if description == 'Describe(portal)' else ('describe-statement' if description else 'query')
                    elif tag == 'ReadyForQuery': ready = fields[3].strip()
            o['ready'] = {0:'I',2:'T',3:'E'}[transaction_status(self.conn)]
            assert ready == o['ready'], 'trace/libpq transaction status disagree'
            return o
        finally:
            clear(result)

# Compare semantic observations, retaining full diagnostics and object identities
# separately. Local table OIDs must be linked within each endpoint, never equated.
def semantic(o):
    return {k: o[k] for k in ('rows', 'metadata', 'command', 'ready')} | {
        'errors': [e['C'] for e in o['errors']],
        'warnings': [e['C'] for e in o['warnings']],
        'description_source': o.get('description_source'),
        'parameters': o.get('parameters',[]),
        'origins': [[bool(oid),col] for oid,col in o['origins']],
        'phase': o['phase'] if o['errors'] else None,
        'completion': o.get('completion',[])}
def equal(a, b):
    left,right=semantic(a),semantic(b)
    assert left == right, (left,right)


def prepare(client, name, sql, oids=(23,)):
    types = (c.c_uint * len(oids))(*oids)
    return client.run(sql, True, lambda: prepare_query(client.conn,name.encode(),sql.encode(),len(oids),types))
def describe(client, name):
    return client.run('Describe '+name, True, lambda: describe_query(client.conn,name.encode()))
def execute(client, name, values=('1',)):
    args = (c.c_char_p * len(values))(*[None if v is None else v.encode() for v in values])
    return client.run('Execute '+name, True, lambda: execute_prepared(client.conn,name.encode(),len(values),args,None,None,0))

# libpq has no public named-portal/max_rows API. Use its authenticated idle
# socket for those operations only; fully drain Sync before returning to libpq.
def wire(client, messages):
    for tag,body in messages: client.send(tag,body)
    client.send(b'S',b'')
    o=dict(rows=[],metadata=[],origins=[],errors=[],warnings=[],command=[],ready='',phase={b'B':'bind',b'D':'describe-portal',b'E':'execute',b'C':'close'}[messages[0][0]],wire=[],frontend=[[tag.decode(),body.hex()] for tag,body in messages]+[['S','']],completion=[])
    while True:
        tag=client.exact(1); length=struct.unpack('!I',client.exact(4))[0]
        body=client.exact(length-4)
        o['wire'].append([tag.decode(),body.hex()])
        if tag==b'2': o['phase']='describe-portal'
        elif tag==b'T':
            off=2
            for _ in range(struct.unpack_from('!H',body)[0]):
                end=body.index(0,off); name=body[off:end].decode(); off=end+1
                table,col,oid,size,mod,fmt=struct.unpack_from('!IhIhih',body,off); off+=18
                o['metadata'].append([name,oid,size,mod,fmt]); o['origins'].append([table,col])
            o['description_source']='Describe(portal)'; o['phase']='execute'
        elif tag==b'D':
            off=2; row=[]
            for _ in range(struct.unpack_from('!H',body)[0]):
                n=struct.unpack_from('!i',body,off)[0]; off+=4
                row.append(None if n<0 else body[off:off+n].decode())
                if n>=0: off+=n
            o['rows'].append(row)
        elif tag==b'C': o['command'].append(body[:-1].decode()); o['completion'].append('CommandComplete')
        elif tag==b's': o['completion'].append('PortalSuspended')
        elif tag==b'3': o['completion'].append('CloseComplete')
        elif tag==b'E':
            fields={}; off=0
            while body[off]:
                key=chr(body[off]); end=body.index(0,off+1)
                fields[key]=body[off+1:end].decode(); off=end+1
            o['errors'].append(fields)
        elif tag==b'Z': o['ready']=body.decode(); return o

def bind(name, statement='s', value='1'):
    data=None if value is None else value.encode()
    body=name.encode()+b'\0'+statement.encode()+b'\0'+struct.pack('!HH',0,1)
    body+=struct.pack('!i',-1 if data is None else len(data))+(data or b'')+struct.pack('!H',0)
    return (b'B',body)
def portal(name): return (b'D',b'P'+name.encode()+b'\0')
def fetch(name, limit=0): return (b'E',name.encode()+b'\0'+struct.pack('!I',limit))
def close_statement(name): return (b'C',b'S'+name.encode()+b'\0')

admin=Client(reference)
connections=[]; created=False; count=0; negative=0; scenes=0; created_roles=[]
try:
    for sql in ('SELECT version()','SHOW server_version_num'):
        o=admin.run(sql); record(dict(case_id='version',sql=sql,observation=o))
        if sql.startswith('SHOW'): assert o['rows']==[['180006']]
    database=f'pg1133_{os.getpid()}_{sys.argv[1]}'
    assert not admin.run('CREATE DATABASE '+database)['errors']; created=True
    ref=reference+' dbname='+database
    for ext in (False,True):
        def connect_pair():
            global a,b,r,s
            a,b,r,s=[Client(dsn) for dsn in (target,target,ref,ref)]
            connections.extend((a,b,r,s))
        def q(sql,who='a',rows=None,error=None,state=None):
            global count
            x,y=(a,r) if who=='a' else (b,s)
            l,rr=x.run(sql,ext),y.run(sql,ext)
            record(dict(case_id=f'{ext}.{scenes}.{count}',sql=sql,target=l,reference=rr)); count+=1
            equal(l,rr)
            for client,observation in ((x,l),(y,rr)):
                for oid,col in observation['origins']:
                    if not oid or oid in client.linked: continue
                    catalog=client.run('SELECT c.oid,c.relname,n.nspname FROM pg_catalog.pg_class c '
                        'JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE c.oid='+str(oid),ext)
                    record(dict(case_id=f'{ext}.{count}.origin',oid=oid,column=col,observation=catalog))
                    assert not catalog['errors'] and len(catalog['rows'])==1 and catalog['rows'][0][0]==str(oid)
                    client.linked[oid]=catalog['rows'][0][1:]
            for (loid,lcol),(roid,rcol) in zip(l['origins'],rr['origins']):
                if loid and roid: assert x.linked[loid]==y.linked[roid] and lcol==rcol
            assert [e['C'] for e in l['errors']]==([error] if error else []),(sql,l)
            if rows is not None: assert l['rows']==rows,(sql,l['rows'],rows)
            if state: assert l['ready']==state
            return l
        def v(fn='nextval',value=None,who='a',error=None,arg="'public.seq_test'::regclass"):
            observation=q(f'SELECT {fn}({arg}) AS v',who,rows=None if error else [[None if value is None else str(value)]],error=error)
            if error: q('SELECT 1 AS recovered',who,rows=[['1']],state='I')
            return observation
        identities={}
        def init(options='START 10 MINVALUE 1 MAXVALUE 1000 CACHE 1'):
            global scenes
            scenes+=1; connect_pair(); q('CREATE SEQUENCE public.seq_test AS bigint '+options)
            for endpoint,c in (('target',a),('reference',r)):
                identity=c.run("SELECT oid,relkind FROM pg_class WHERE relname='seq_test'",ext)
                record(dict(case_id=f'{ext}.{scenes}.identity',endpoint=endpoint,observation=identity))
                assert not identity['errors'] and len(identity['rows'])==1 and identity['rows'][0][1]=='S'
                oid=identity['rows'][0][0]
                assert identities.get(endpoint)!=oid
                identities[endpoint]=oid
        def done():
            q('DROP SEQUENCE public.seq_test')
            for c in (a,b,r,s): c.close()
        init(); err=v('currval',error='55000'); q('SELECT 1 AS ok',rows=[['1']]); v(value=10); v('currval',10)
        v('currval',who='b',error='55000'); v(value=11,who='b'); v('currval',10); v(value=12); v('currval',11,'b')
        a.close(); r.close(); a,r=Client(target),Client(ref); connections.extend((a,r))
        v('currval',error='55000'); v(value=13); v('currval',11,'b'); done()
        init(); v('setval',40,arg="'public.seq_test',40"); v('currval',40); v(value=41)
        v('setval',50,arg="'public.seq_test',50,true"); v('currval',50); v(value=51,who='b'); v('currval',50); done()
        init(); v('setval',60,arg="'public.seq_test',60,false"); v('currval',error='55000'); v(value=60)
        v('setval',80,arg="'public.seq_test',80,false"); v('currval',60); v(value=80); done()
        init(); q('CREATE TABLE seq_rows(id integer PRIMARY KEY,v bigint)'); q('BEGIN',state='T'); v(value=10)
        q('INSERT INTO seq_rows VALUES(1,10)'); q('ROLLBACK',state='I'); v('currval',10)
        effects=q('SELECT id,v FROM seq_rows ORDER BY id','b',rows=[]); v(value=11,who='b'); q('BEGIN'); v('setval',40,arg="'public.seq_test',40,true")
        q('SELECT missing_column',error='42703',state='E'); q('SELECT 1',error='25P02',state='E')
        q('ROLLBACK'); v('currval',40); v(value=41,who='b'); q('DROP TABLE seq_rows'); done()
        init(); v(arg='NULL'); v('currval',arg='NULL'); v('setval',arg="'public.seq_test',NULL,true")
        v('setval',arg="'public.seq_test',20,NULL"); v('setval',arg="'public.seq_test',0,true",error='22003')
        v('setval',arg="'public.seq_test',1001,true",error='22003'); v(arg="'missing_sequence'",error='42P01')
        q('CREATE TABLE ordinary(id integer)'); v(arg="'ordinary'",error='42809'); q('DROP TABLE ordinary'); v(value=10)
        observed=v('currval',10)
        for field in ('value','sqlstate','side_effect'):
            original=err if field=='sqlstate' else (effects if field=='side_effect' else observed)
            changed=copy.deepcopy(original)
            if field=='sqlstate': changed['errors'][0]['C']='XX000'
            else: changed['rows']=[['999']]
            try: equal(original,changed)
            except AssertionError: negative+=1
            else: raise AssertionError('negative check failed '+field)
        done()
        # Each privilege combination starts from a genuinely new connection/object.
        for privilege in ('NONE','USAGE','SELECT','UPDATE'):
            init(); role=f"{os.environ['PG11_ROLE_PREFIX']}_{int(ext)}_{privilege.lower()}"
            setup=admin.run('CREATE ROLE '+role)
            record(dict(case_id='reference-role-setup',role=role,observation=setup))
            assert not setup['errors']; created_roles.append(role)
            user=admin.run('SELECT current_user')['rows'][0][0]
            assert not admin.run('GRANT '+role+' TO '+user)['errors']
            if privilege!='NONE': q('GRANT '+privilege+' ON SEQUENCE public.seq_test TO '+role)
            q('SET ROLE '+role,'b')
            v(who='b',value=10,error=None if privilege in ('USAGE','UPDATE') else '42501')
            v('currval',10,'b',error=None if privilege=='USAGE' else ('55000' if privilege=='SELECT' else '42501'))
            v('setval',40,'b',arg="'public.seq_test',40",error=None if privilege=='UPDATE' else '42501')
            q('SELECT 1 AS ok','b',rows=[['1']])
            if privilege!='NONE': q('REVOKE '+privilege+' ON SEQUENCE public.seq_test FROM '+role)
            v(who='b',error='42501'); v('currval',who='b',error='42501')
            v('setval',who='b',arg="'public.seq_test',40",error='42501')
            v(value={'NONE':10,'USAGE':11,'SELECT':10,'UPDATE':41}[privilege]); done()
            assert not admin.run('DROP ROLE '+role)['errors']; created_roles.remove(role)
        init('START 10 MINVALUE 1 MAXVALUE 209 CACHE 1')
        import threading
        from concurrent.futures import ThreadPoolExecutor
        def allocate(x,y):
            barrier=threading.Barrier(2)
            def worker(c):
                barrier.wait(); obs=[c.run("SELECT nextval('public.seq_test') AS v",ext) for _ in range(100)]
                assert all(not o['errors'] for o in obs)
                assert c.run("SELECT currval('public.seq_test') AS v",ext)['rows']==obs[-1]['rows']
                return obs
            with ThreadPoolExecutor(2) as pool:
                f=pool.submit(worker,x); g=pool.submit(worker,y); obs=f.result()+g.result()
            assert sorted(int(o['rows'][0][0]) for o in obs)==list(range(10,210))
            return sorted(obs,key=lambda o:int(o['rows'][0][0]))
        for l,rr in zip(allocate(a,b),allocate(r,s)):
            equal(l,rr); record(dict(case_id=f'{ext}.concurrent.{count}',target=l,reference=rr)); count+=1
        v(error='2200H'); done()
        init('START 10 MINVALUE 1 MAXVALUE 1000 CACHE 3')
        v(value=10); v('setval',40,'b',arg="'public.seq_test',40,true")
        for n in (11,12,41): v(value=n)
        v(value=44,who='b'); v('setval',arg="'public.seq_test',1001,false",error='22003')
        v('setval',arg="'public.seq_test',NULL,false"); v(value=42)
        v('setval',60,arg="'public.seq_test',60,false"); v('currval',42); v(value=60)
        for n in (45,46,63): v(value=n,who='b')
        v('setval',999,arg="'public.seq_test',999,false"); v(value=999); v(value=1000); v(error='2200H'); done()
        init('START 1 MINVALUE 1 MAXVALUE 3 CACHE 1 CYCLE')
        for n in (1,2,3,1): v(value=n)
        done()
        # Parse and Describe must not allocate; actual Execute allocates once.
        init()
        for label,operation in (
            ('Parse',lambda c:prepare(c,'seq_prepared',"SELECT nextval('public.seq_test') AS v",())),
            ('Describe',lambda c:describe(c,'seq_prepared')),
        ):
            l,rr=operation(a),operation(r); equal(l,rr)
            record(dict(case_id=f'{ext}.prepared.{label}',target=l,reference=rr)); count+=1
            assert not l['errors']
        v(value=10,who='b')
        for expected in ('11','12'):
            l,rr=execute(a,'seq_prepared',()),execute(r,'seq_prepared',()); equal(l,rr)
            record(dict(case_id=f'{ext}.prepared.execute.{expected}',target=l,reference=rr)); count+=1
            assert l['rows']==[[expected]]
        l,rr=wire(a,[close_statement('seq_prepared')]),wire(r,[close_statement('seq_prepared')]); equal(l,rr)
        record(dict(case_id=f'{ext}.prepared.close',target=l,reference=rr)); count+=1
        v('currval',12); done()
        print('PG11_33_MID',flush=True); assert sys.stdin.readline().strip()=='continue'
finally:
    for c in connections:
        if c.conn: c.run('ROLLBACK'); c.close()
    if created: assert not admin.run('DROP DATABASE '+database)['errors']
    for role in created_roles: assert not admin.run('DROP ROLE '+role)['errors']
    admin.close(); output.close()
(archive/'summary.json').write_text(json.dumps(dict(passed=True,paired_inputs=count,scenarios=scenes,protocols=2,negative_checks=negative,cleanup=dict(reference_database=True,pg_connections=True))))
