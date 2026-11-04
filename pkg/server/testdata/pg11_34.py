# Copyright 2026 AsterSQL.
"""Identity differential over real libpq connections and shared server listeners.
Q uses PQexec; extended uses PQexecParams and named prepared statements.
Named portals/max_rows and Close use the authenticated socket; responses
are drained at each requested Flush/Sync boundary.
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

archive = Path(os.environ["PG11_34_EVIDENCE_DIR"])
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
libpq_version=api('PQlibVersion', c.c_int)()
assert libpq_version >= 180000, 'libpq 18 required'
record(dict(case_id='client_version',libpq_version=libpq_version))
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
def wire(client, messages, stop=None):
    for tag,body in messages: client.send(tag,body)
    client.send(b'H' if stop else b'S',b'')
    o=dict(rows=[],metadata=[],origins=[],errors=[],warnings=[],command=[],ready='',phase={b'P':'parse',b'B':'bind',b'D':'describe-portal',b'E':'execute',b'C':'close'}.get(messages[0][0] if messages else b'S','sync'),wire=[],frontend=[[tag.decode(),body.hex()] for tag,body in messages]+[['H' if stop else 'S','']],completion=[])
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
        if tag==stop: o['ready']=None; return o

def bind(name, statement='s', value='1'):
    data=None if value is None else value.encode()
    body=name.encode()+b'\0'+statement.encode()+b'\0'+struct.pack('!HH',0,1)
    body+=struct.pack('!i',-1 if data is None else len(data))+(data or b'')+struct.pack('!H',0)
    return (b'B',body)
def portal(name): return (b'D',b'P'+name.encode()+b'\0')
def fetch(name, limit=0): return (b'E',name.encode()+b'\0'+struct.pack('!I',limit))
def close_statement(name): return (b'C',b'S'+name.encode()+b'\0')

admin=Client(reference)
connections=[]; created=False; count=0; negative=0; scenes=0
try:
    for sql in ('SELECT version()','SHOW server_version_num'):
        o=admin.run(sql); record(dict(case_id='version',sql=sql,observation=o))
        if sql.startswith('SHOW'): assert o['rows']==[['180006']]
    database=f'pg1134_{os.getpid()}_{sys.argv[1]}'
    assert not admin.run('CREATE DATABASE '+database)['errors']; created=True
    ref=reference+' dbname='+database
    for ext in (False,True):
        def pair():
            global a,b,r,s
            a,b,r,s=[Client(dsn) for dsn in (target,target,ref,ref)]
            connections.extend((a,b,r,s))
        def compare(l,rr,sql):
            global count
            record(dict(case_id=f'{ext}.{scenes}.{count}',sql=sql,target=l,reference=rr)); count+=1
            equal(l,rr)
            return l
        # Origin OIDs are resolved at each endpoint, then compared by relation
        # identity and attribute number. Their numeric values are never equated.
        def q(sql,who='a',rows=None,error=None,state=None):
            x,y=(a,r) if who=='a' else (b,s)
            l,rr=x.run(sql,ext),y.run(sql,ext)
            compare(l,rr,sql)
            for client,o in ((x,l),(y,rr)):
                for oid,col in o['origins']:
                    if not oid or oid in client.linked: continue
                    cat=client.run('SELECT c.oid,c.relname,n.nspname FROM pg_class c JOIN pg_namespace n ON c.relnamespace=n.oid WHERE c.oid='+str(oid),ext)
                    record(dict(case_id=f'{ext}.{count}.origin',oid=oid,column=col,observation=cat))
                    assert not cat['errors'] and len(cat['rows'])==1 and cat['rows'][0][0]==str(oid)
                    client.linked[oid]=cat['rows'][0][1:]
            for (lo,lc),(ro,rc) in zip(l['origins'],rr['origins']):
                if lo and ro: assert x.linked[lo]==y.linked[ro] and lc==rc
            assert [e['C'] for e in l['errors']]==([error] if error else []),(sql,l)
            if rows is not None: assert l['rows']==rows,(sql,l['rows'],rows)
            if state: assert l['ready']==state
            return l
        def init(mode='ALWAYS',typ='bigint',options='START 10',unique=''):
            global scenes
            scenes+=1; pair()
            q(f'CREATE TABLE public.identity_test (id {typ} GENERATED {mode} AS IDENTITY ({options}), note text{unique})')
        def insert(tail,rows=None,error=None,who='a',state='I'):
            return q('INSERT INTO public.identity_test'+tail+' RETURNING id,note',who,rows,error,state)
        def snapshot(rows):
            return q('SELECT id,note FROM public.identity_test ORDER BY id','b',rows=rows,state='I')
        def current(who,value=None,error=None):
            clients=(a,r) if who=='a' else (b,s)
            observations=[]
            for endpoint,client in zip(('target','reference'),clients):
                seq=client.run("SELECT pg_get_serial_sequence('public.identity_test','id')",ext)
                assert not seq['errors']; name=seq['rows'][0][0]
                o=client.run(f"SELECT currval('{name}') AS v",ext)
                record(dict(case_id=f'{ext}.{scenes}.currval.{who}',endpoint=endpoint,sequence=seq,observation=o))
                assert [e['C'] for e in o['errors']]==([error] if error else [])
                if not error: assert o['rows']==[[str(value)]]
                observations.append(o)
            compare(*observations,'identity connection currval')
        def recover(transaction=False):
            if transaction:
                q('SELECT 1 AS ok',error='25P02',state='E'); q('ROLLBACK',state='I')
            q('SELECT 1 AS ok',rows=[['1']],state='I')
        def done():
            # Capture the actual owned sequence, then prove DROP removes it.
            names=[]
            for endpoint,client in (('target',a),('reference',r)):
                o=client.run("SELECT pg_get_serial_sequence('public.identity_test','id')",ext)
                record(dict(case_id=f'{ext}.{scenes}.owned',endpoint=endpoint,observation=o))
                assert not o['errors'] and o['rows'][0][0]; names.append(o['rows'][0][0])
            q('DROP TABLE public.identity_test')
            drops=[]
            for endpoint,client,name in zip(('target','reference'),(a,r),names):
                o=client.run("SELECT relname FROM pg_class WHERE oid='"+name+"'::regclass",ext)
                record(dict(case_id=f'{ext}.{scenes}.drop',endpoint=endpoint,observation=o))
                assert [e['C'] for e in o['errors']]==['42P01']
                drops.append(o)
            compare(*drops,'owned sequence absence after DROP')
            for client in (a,b,r,s): client.close()
        for mode in ('ALWAYS','BY DEFAULT'):
            for typ,oid in (('smallint',21),('integer',23),('bigint',20)):
                init(mode,typ)
                o=insert("(note) VALUES ('omit')",[['10','omit']]); assert o['metadata'][0][1]==oid
                insert(" VALUES (DEFAULT,'default')",[['11','default']]); snapshot([['10','omit'],['11','default']]); done()
                init(mode,typ); insert(' DEFAULT VALUES',[['10',None]]); snapshot([['10',None]]); done()
            for override in ('',' OVERRIDING SYSTEM VALUE',' OVERRIDING USER VALUE'):
                for value in ('900','NULL','DEFAULT'):
                    init(mode)
                    error='428C9' if mode=='ALWAYS' and not override and value!='DEFAULT' else ('23502' if value=='NULL' and 'USER' not in override else None)
                    generated=value=='DEFAULT' or 'USER' in override
                    first='10' if generated else '900'
                    err=insert(override+f" VALUES ({value},'input')",None if error else [[first,'input']],error)
                    recover(); effects=snapshot([] if error else [[first,'input']])
                    next_value='11' if generated and not error else '10'
                    observed=insert("(note) VALUES ('after')",[[next_value,'after']])
                    snapshot(sorted(([] if error else [[first,'input']])+[[next_value,'after']],key=lambda row:int(row[0])))
                    if mode=='ALWAYS' and not override and value=='900':
                        for field,original in (('value',observed),('sqlstate',err),('side_effect',effects)):
                            changed=copy.deepcopy(original)
                            if field=='sqlstate': changed['errors'][0]['C']='XX000'
                            else: changed['rows']=[['999']]
                            try: equal(original,changed)
                            except AssertionError: negative+=1
                            else: raise AssertionError('negative comparator check failed '+field)
                    done()
            for override in ('SYSTEM','USER'):
                for columns,values in (('(id,note)',"(-1,'negative')"),('(id,note)',"(9223372036854775807,'max')"),('(note)',"('omit')")):
                    init(mode)
                    generated=override=='USER' or columns=='(note)'
                    value='10' if generated else values.split(',')[0][1:]
                    note=values.split("'")[1]
                    insert(columns+f' OVERRIDING {override} VALUE VALUES '+values,[[value,note]])
                    insert("(note) VALUES ('next')",[['11' if generated else '10','next']]); done()
                init(mode)
                q(f'INSERT INTO identity_test OVERRIDING {override} VALUE DEFAULT VALUES',error='42601')
                recover(); snapshot([]); insert(' DEFAULT VALUES',[['10',None]]); done()
            for value,error in (("'invalid'",'22P02'),):
                init(mode)
                insert(' OVERRIDING USER VALUE VALUES ('+value+",'bad')",error=error)
                recover(); snapshot([]); insert(' DEFAULT VALUES',[['10',None]]); done()
            init(mode)
            insert(" OVERRIDING USER VALUE VALUES (1/0,'ignored')",[['10','ignored']])
            snapshot([['10','ignored']]); done()
            init(mode)
            q("INSERT INTO identity_test(note) VALUES ('bad') RETURNING",error='42601')
            recover(); snapshot([]); insert(' DEFAULT VALUES',[['10',None]]); done()
            init(mode)
            insert(" OVERRIDING SYSTEM VALUE VALUES (10,'manual')",[['10','manual']])
            insert(" DEFAULT VALUES",[['10',None]])
            q('SELECT id FROM identity_test ORDER BY id','b',rows=[['10'],['10']]); done()
            # Row rollback and allocation rollback are independent facts.
            init(mode); q('BEGIN',state='T'); insert("(note) VALUES ('undone')",[['10','undone']],state='T')
            q('SELECT id,note FROM identity_test',rows=[['10','undone']],state='T'); snapshot([])
            current('a',10); current('b',error='55000')
            q('ROLLBACK',state='I'); snapshot([]); current('a',10)
            insert("(note) VALUES ('kept')",[['11','kept']],who='b'); current('a',10); current('b',11)
            snapshot([['11','kept']]); done()
            for transaction in (False,True):
                init(mode,unique=' UNIQUE'); insert("(note) VALUES ('seed')",[['10','seed']])
                if transaction: q('BEGIN',state='T')
                insert("(note) VALUES ('seed')",error='23505',state='E' if transaction else 'I')
                recover(transaction); current('a',11); snapshot([['10','seed']]); insert("(note) VALUES ('after')",[['12','after']]); snapshot([['10','seed'],['12','after']])
                insert(' DEFAULT VALUES',[['13',None]]); insert(' DEFAULT VALUES',[['14',None]])
                snapshot([['10','seed'],['12','after'],['13',None],['14',None]]); done()
            # Catalog probes must not consume values; all numeric identities are
            # checked through true links inside each endpoint.
            init(mode,options='START 10 MINVALUE 1 MAXVALUE 1000 CACHE 1 NO CYCLE')
            q("SELECT attidentity,attnotnull,atthasdef,atttypid FROM pg_attribute WHERE attrelid='public.identity_test'::regclass AND attname='id'",rows=[['a' if mode=='ALWAYS' else 'd','t','f','20']])
            q("SELECT adnum FROM pg_attrdef WHERE adrelid='public.identity_test'::regclass",rows=[])
            catalog_observations=[]
            for endpoint,client in (('target',a),('reference',r)):
                endpoint_observations=[]
                seq=client.run("SELECT pg_get_serial_sequence('public.identity_test','id')",ext)
                assert not seq['errors']; name=seq['rows'][0][0]
                for sql,rows in (
                    (f"SELECT seqstart,seqincrement,seqmin,seqmax,seqcache,seqcycle FROM pg_sequence WHERE seqrelid='{name}'::regclass",[['10','1','1','1000','1','f']]),
                    (f"SELECT deptype,refobjsubid FROM pg_depend WHERE objid='{name}'::regclass AND refobjid='public.identity_test'::regclass",[['i','1']]),
                    (f"SELECT relkind FROM pg_class WHERE oid='{name}'::regclass",[['S']]),
                ):
                    o=client.run(sql,ext); record(dict(case_id=f'{ext}.{scenes}.catalog',endpoint=endpoint,sql=sql,observation=o))
                    assert not o['errors'] and o['rows']==rows
                    endpoint_observations.append(o)
                catalog_observations.append(endpoint_observations)
            for l,rr in zip(*catalog_observations): compare(l,rr,'owned catalog relation')
            q("SELECT pg_get_serial_sequence('public.identity_test','note'),pg_get_serial_sequence(NULL,'id')",rows=[[None,None]])
            q("SELECT pg_get_serial_sequence('public.identity_test','missing')",error='42703'); recover()
            q("SELECT pg_get_serial_sequence('public.identity_missing','id')",error='42P01'); recover()
            insert(' DEFAULT VALUES',[['10',None]]); current('a',10)
            a.close(); r.close(); a,r=Client(target),Client(ref); connections.extend((a,r))
            snapshot([['10',None]]); current('a',error='55000'); insert(' DEFAULT VALUES',[['11',None]]); done()
        # Explicit failure transaction and disconnect rollback use fresh objects.
        init(); q('BEGIN',state='T')
        insert(" VALUES (900,'bad')",error='428C9',state='E'); recover(True)
        snapshot([]); insert(' DEFAULT VALUES',[['10',None]]); done()
        init(); q('BEGIN',state='T'); insert(' DEFAULT VALUES',[['10',None]],state='T')
        snapshot([]); a.close(); r.close()
        a,r=Client(target),Client(ref); connections.extend((a,r))
        snapshot([]); insert(' DEFAULT VALUES',[['11',None]],who='b'); done()
        for typ,options,error in (('smallint','START 32768','22023'),('text','START 10','22023'),('bigint','CACHE 0','22023')):
            scenes+=1; pair()
            q(f'CREATE TABLE identity_test(id {typ} GENERATED ALWAYS AS IDENTITY ({options}))',error=error)
            recover()
            q("SELECT relname FROM pg_class WHERE relname='identity_test'",rows=[])
            for client in (a,b,r,s): client.close()
        # A declaration bound and a post-allocation exhaustion are distinct.
        for typ,maximum in (('smallint','32767'),('integer','2147483647'),('bigint','9223372036854775807')):
            init(typ=typ,options='START '+maximum)
            insert(" OVERRIDING SYSTEM VALUE VALUES (NULL,'bad')",error='23502')
            recover(); snapshot([])
            insert(' DEFAULT VALUES',[[maximum,None]])
            insert(' DEFAULT VALUES',error='2200H'); recover(); snapshot([[maximum,None]]); done()
        # Prepared Parse/Describe allocate nothing; execution returns the actual
        # rows, and closing the statement does not affect the owned sequence.
        init()
        for label,operation in (
            ('Parse',lambda c:prepare(c,'identity_prepared',"INSERT INTO identity_test(note) VALUES ('prepared') RETURNING id,note",())),
            ('Describe',lambda c:describe(c,'identity_prepared')),
        ):
            o=compare(operation(a),operation(r),label); assert not o['errors']
        insert("(note) VALUES ('observer')",[['10','observer']],who='b')
        o=compare(execute(a,'identity_prepared',()),execute(r,'identity_prepared',()),'Execute'); assert o['rows']==[['11','prepared']]
        compare(wire(a,[close_statement('identity_prepared')]),wire(r,[close_statement('identity_prepared')]),'Close')
        snapshot([['10','observer'],['11','prepared']]); done()
        init()
        sql="INSERT INTO identity_test(note) VALUES ('first'),('second') RETURNING id,note"
        compare(prepare(a,'identity_portal',sql,()),prepare(r,'identity_portal',sql,()),'portal Parse')
        compare(describe(a,'identity_portal'),describe(r,'identity_portal'),'portal statement Describe')
        messages=[(b'B',b'ip\0identity_portal\0'+struct.pack('!HHH',0,0,0)),portal('ip')]
        compare(wire(a,messages,b'T'),wire(r,messages,b'T'),'portal Bind/Describe before execution')
        insert("(note) VALUES ('observer')",[['10','observer']],who='b')
        l=compare(wire(a,[fetch('ip',1)],b's'),wire(r,[fetch('ip',1)],b's'),'first Execute max_rows=1')
        assert l['rows']==[['11','first']] and l['completion']==['PortalSuspended']
        # A has not sent Sync. Compare the real independently visible effects;
        # the returning row alone never establishes a commit.
        snapshot([['10','observer']])
        l=compare(wire(a,[fetch('ip')],b'C'),wire(r,[fetch('ip')],b'C'),'resume Execute without Sync')
        assert l['rows']==[['12','second']] and l['command']==['INSERT 0 1']
        l=compare(wire(a,[fetch('ip')],b'C'),wire(r,[fetch('ip')],b'C'),'completed portal Execute without Sync')
        assert l['rows']==[] and l['command']==['INSERT 0 0']
        snapshot([['10','observer']])
        closes=[(b'C',b'Pip\0'),close_statement('identity_portal')]
        l=compare(wire(a,closes),wire(r,closes),'Close portal and statement then Sync')
        assert l['ready']=='I' and l['completion']==['CloseComplete','CloseComplete']
        snapshot([['10','observer'],['11','first'],['12','second']])
        insert("(note) VALUES ('after')",[['13','after']]); done()
        # A whole unsynchronized DML segment owns one shared transaction.
        # Observe from B before Sync, after error Sync, and after disconnect.
        def raw_statement(name,sql):
            return [(b'P',name.encode()+b'\0'+sql.encode()+b'\0'+struct.pack('!H',0)),
                    (b'B',name.encode()+b'\0'+name.encode()+b'\0'+struct.pack('!HHH',0,0,0)),
                    portal(name),fetch(name)]
        for scenario in ('execute_error','parse_error','disconnect','explicit_commit','explicit_error'):
            init(unique=' UNIQUE')
            explicit=scenario.startswith('explicit')
            if explicit: q('BEGIN',state='T')
            sql="INSERT INTO identity_test(note) VALUES ('seed') RETURNING id,note"
            o=compare(wire(a,raw_statement('segment_first',sql),b'C'),wire(r,raw_statement('segment_first',sql),b'C'),scenario+' first Execute without Sync')
            assert o['rows']==[['10','seed']] and not o['errors']
            snapshot([])
            failed=scenario.endswith('error')
            if failed:
                if scenario=='parse_error':
                    messages=[(b'P',b'segment_bad\0SELECT missing_identity_column\0\0\0')]
                    code='42703'
                else:
                    messages=raw_statement('segment_second',sql); code='23505'
                o=compare(wire(a,messages,b'E'),wire(r,messages,b'E'),scenario+' error before Sync')
                assert [e['C'] for e in o['errors']]==[code]
                snapshot([])
            if scenario=='disconnect':
                a.close(); r.close(); a,r=Client(target),Client(ref); connections.extend((a,r))
            else:
                o=compare(wire(a,[]),wire(r,[]),scenario+' Sync')
                expected_state=('E' if failed else 'T') if explicit else 'I'
                assert o['ready']==expected_state
            if explicit:
                snapshot([])
                if failed:
                    q('SELECT 1',error='25P02',state='E'); q('ROLLBACK',state='I')
                else: q('COMMIT',state='I')
            snapshot([['10','seed']] if scenario=='explicit_commit' else [])
            value='12' if scenario in ('execute_error','explicit_error') else '11'
            insert("(note) VALUES ('after')",[[value,'after']])
            snapshot(([['10','seed']] if scenario=='explicit_commit' else [])+[[value,'after']]); done()
        # These narrow declaration options remain explicitly unsupported. Observe
        # reference success and target refusal separately, never mark them equal.
        for options in ('START 10 CACHE 2','START 3 INCREMENT -1 MINVALUE 1 MAXVALUE 3'):
            scenes+=1; pair()
            sql=f'CREATE TABLE identity_test(id bigint GENERATED ALWAYS AS IDENTITY ({options}))'
            l,rr=a.run(sql,ext),r.run(sql,ext)
            record(dict(case_id=f'{ext}.{scenes}.capability_gate',sql=sql,target=l,reference=rr))
            assert [e['C'] for e in l['errors']]==['0A000'] and not rr['errors']
            assert a.run("SELECT relname FROM pg_class WHERE relname='identity_test'",ext)['rows']==[]
            assert not r.run('DROP TABLE identity_test',ext)['errors']
            for client in (a,b,r,s): client.close()
        print('PG11_34_MID',flush=True); assert sys.stdin.readline().strip()=='continue'
finally:
    for client in connections:
        if client.conn: client.run('ROLLBACK'); client.close()
    if created: assert not admin.run('DROP DATABASE '+database)['errors']
    admin.close(); output.close()
(archive/'summary.json').write_text(json.dumps(dict(passed=True,paired_inputs=count,scenarios=scenes,protocols=2,negative_checks=negative,unsupported_identity_options=['CACHE > 1','negative INCREMENT'],cleanup=dict(reference_database=True,pg_connections=True))))
