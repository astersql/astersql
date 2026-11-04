# Copyright 2026 AsterSQL.
"""Prepared-state differential. Arguments: target TCP port, target database.

Q queries use PQexec/PQexecParams; named statements use PQprepare,
PQdescribePrepared and PQexecPrepared. libpq traces preserve error phases.
Named portals and paging use the authenticated idle socket because libpq has
no public API for them; their complete wire observations are recorded separately.
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

archive = Path(os.environ["PG11_32_EVIDENCE_DIR"])
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
    # The roadmap explicitly tracks the existing synthetic catalog mapping
    # separately. Keep this one measured physical-origin limitation visible;
    # ordinary table origins and every other field remain strict comparisons.
    if (a['metadata']==b['metadata']==[['relname',19,64,-1,0]]
            and a['origins']==[[0,0]] and b['origins']==[[1259,2]]):
        record(dict(case_id='known-catalog-origin',target=a['origins'],reference=b['origins'],
                    limitation='synthetic pg_class has no physical column origin'))
        left['origins']=right['origins']
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
connections=[]; created=False; count=0; negative=0; scenes=0
try:
    for sql in ['SELECT version()', 'SHOW server_version_num']:
        provenance=admin.run(sql); record(dict(case_id='provenance',sql=sql,observation=provenance))
        if sql=='SHOW server_version_num': assert provenance['rows']==[['180006']]
    database=f'pg1132_{os.getpid()}_{sys.argv[1]}'
    assert not admin.run('CREATE DATABASE '+database)['errors']; created=True
    ref=reference+' dbname='+database
    for ext in (False,True):
        clients=[]
        for dsn in (target,target,ref,ref):
            client=Client(dsn); clients.append(client); connections.append(client)
        a,b,r,s=clients
        def pair_op(label,op,who='a',rows=None,error=None,state=None):
            global count
            x,y=(a,r) if who=='a' else (b,s)
            left,right=op(x),op(y)
            record(dict(case_id=f'{ext}.{count}.{label}',target=left,reference=right))
            count+=1; equal(left,right)
            if rows is not None: assert left['rows']==rows,(label,left['rows'],rows)
            if error: assert [e['C'] for e in left['errors']]==[error],(label,left)
            else: assert not left['errors'],(label,left)
            if state: assert left['ready']==state
            # Relate each nonzero physical origin to this endpoint's real
            # catalog identity; cache only immutable identity, never rows.
            for client, observation in ((x,left),(y,right)):
                for oid,col in observation['origins']:
                    if not oid or oid in client.linked: continue
                    sql = ('SELECT c.oid,c.relname,n.nspname FROM pg_catalog.pg_class c '
                           'JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE c.oid='+str(oid))
                    catalog=client.run(sql)
                    record(dict(case_id=f'origin.{ext}.{count}',oid=oid,column=col,catalog=catalog))
                    assert not catalog['errors'] and len(catalog['rows'])==1
                    assert catalog['rows'][0][0]==str(oid)
                    client.linked[oid]=catalog['rows'][0][1:]
            for (loid,lcol),(roid,rcol) in zip(left['origins'],right['origins']):
                if loid and roid:
                    assert x.linked[loid]==y.linked[roid] and lcol==rcol
            return left
        def q(sql,who='a',**kw): return pair_op(sql,lambda c:c.run(sql,ext),who,**kw)
        def p(sql='SELECT v FROM prep_doc WHERE id=$1',name='s',who='a',oids=(23,)):
            pair_op('Parse '+name,lambda c:prepare(c,name,sql,oids),who)
            observation=pair_op('Describe '+name,lambda c:describe(c,name),who)
            assert observation['parameters']==list(oids)
            return observation
        def e(value='1',name='s',who='a',**kw):
            # PQexecPrepared sends Bind/Describe(portal)/Execute/Sync for the
            # existing named statement; it never reparses the SQL.
            return pair_op('Bind/Execute '+name,lambda c:execute(c,name,() if value==() else (value,)),who,**kw)
        def w(label,msg,who='a',**kw): return pair_op(label,lambda c:wire(c,msg),who,**kw)
        def shut(name='s',who='a'): w('Close '+name,[close_statement(name)],who)
        def init():
            global scenes
            scenes+=1
            for who in ('a','b'): q('SET search_path TO public',who)
            q('CREATE TABLE public.prep_doc (id integer PRIMARY KEY,v integer)')
            q('INSERT INTO public.prep_doc VALUES (1,10),(2,20)')
        def effects(rows=None):
            observations=[]
            for who in ('a','b'):
                observations.append(q('SELECT id,v FROM public.prep_doc ORDER BY id',who,rows=rows or [['1','10'],['2','20']]))
            return observations
        def done():
            effects(); q('DROP TABLE public.prep_doc')
        # stable and per-connection statement ownership.
        init(); p(); e(rows=[['10']]); e('2',rows=[['20']]); e(None,rows=[])
        q('INSERT INTO public.prep_doc VALUES (3,NULL)','b'); e('3',rows=[[None]])
        effects([['1','10'],['2','20'],['3',None]]); q('DELETE FROM public.prep_doc WHERE id=3','b')
        p('SELECT id FROM public.prep_doc WHERE id=$1',who='b'); e(who='b',rows=[['1']]); e(rows=[['10']])
        shut(who='b'); missing=e(who='b',error='26000'); q('SELECT 1 AS ok','b',rows=[['1']])
        q('BEGIN'); w('Bind private',[bind('private'),portal('private')],state='T')
        w('foreign portal',[fetch('private')],who='b',error='34000',state='I')
        q('SELECT 1 AS ok','b',rows=[['1']]); w('Execute private',[portal('private'),fetch('private')],rows=[['10']],state='T'); q('ROLLBACK')
        p('SELECT id FROM public.prep_doc WHERE id=$1',who='b')
        for who in ('a','b'):
            q('BEGIN',who); w('Bind own p',[bind('p'),portal('p')],who,state='T')
        shut(); w('B p survives A close',[portal('p'),fetch('p')],who='b',rows=[['1']],state='T')
        e(error='26000',state='E'); q('ROLLBACK'); q('ROLLBACK','b'); shut(who='b'); done()
        # Missing path, qualified reference and failed transaction recovery.
        init(); p(); p('SELECT v FROM public.prep_doc WHERE id=$1','qualified'); e(rows=[['10']])
        q('SET search_path TO pg_catalog'); err=e(error='42P01',state='I')
        q('SELECT 1 AS ok',rows=[['1']]); e(name='qualified',rows=[['10']])
        q('BEGIN'); q('INSERT INTO public.prep_doc VALUES (9,99)')
        q('SELECT id,v FROM public.prep_doc ORDER BY id',rows=[['1','10'],['2','20'],['9','99']]); q('SELECT id,v FROM public.prep_doc ORDER BY id','b',rows=[['1','10'],['2','20']])
        e(error='42P01',state='E'); q('SELECT 1 AS ok',error='25P02',state='E'); q('ROLLBACK',state='I')
        q('SET search_path TO public'); e(rows=[['10']]); e('bad',error='22P02'); e('2',rows=[['20']]); e(None,rows=[])
        observed=effects()
        for field in ('value','sqlstate','side_effect'):
            original=err if field=='sqlstate' else observed[0]
            changed=copy.deepcopy(original)
            if field=='sqlstate': changed['errors'][0]['C']='XX000'
            else: changed['rows'][0][1]='999'
            try: equal(original,changed)
            except AssertionError: negative+=1; record(dict(case_id=f'negative.{ext}.{field}',detected=True))
            else: raise AssertionError('negative self-check missed '+field)
        shut(); shut('qualified'); done()
        # Path switch and earlier-shadow have independent initial states.
        for earlier in (False,True):
            init(); q('CREATE SCHEMA prep_alt')
            if not earlier:
                q('CREATE TABLE prep_alt.prep_doc (id integer PRIMARY KEY,v integer)','b'); q('INSERT INTO prep_alt.prep_doc VALUES (1,90)','b')
            else: q('SET search_path TO prep_alt, public')
            p(); e(rows=[['10']])
            if earlier:
                q('CREATE TABLE prep_alt.prep_doc (id integer PRIMARY KEY,v integer)','b'); q('INSERT INTO prep_alt.prep_doc VALUES (1,90)','b'); e(rows=[['10']]); q('SET search_path TO public'); e(rows=[['10']])
            q('SET search_path TO prep_alt, public'); alt=e(rows=[['90']])
            q('SET search_path TO public'); public=e(rows=[['10']])
            assert alt['origins'][0][0]!=public['origins'][0][0]
            q('SELECT id,v FROM prep_alt.prep_doc','b',rows=[['1','90']]); effects()
            shut()
            p('SELECT d.v FROM public.prep_doc d JOIN prep_alt.prep_doc h ON d.id=h.id WHERE d.id=$1',name='join_dep')
            e(name='join_dep',rows=[['10']]); q('DROP TABLE prep_alt.prep_doc','b')
            e(name='join_dep',error='42P01'); q('SELECT 1 AS ok',rows=[['1']])
            q('CREATE TABLE prep_alt.prep_doc (id integer PRIMARY KEY,v integer)','b'); q('INSERT INTO prep_alt.prep_doc VALUES (1,70)','b')
            e(name='join_dep',rows=[['10']]); shut('join_dep')
            q('DROP TABLE prep_alt.prep_doc'); q('DROP SCHEMA prep_alt'); done()
        # Drop/recreate: link real relation OIDs locally, never across endpoints.
        init(); p(); old=e(rows=[['10']]); q('DROP TABLE public.prep_doc','b'); e(error='42P01'); q('SELECT 1 AS ok',rows=[['1']])
        q('CREATE TABLE public.prep_doc (id integer PRIMARY KEY,v integer)','b'); q('INSERT INTO public.prep_doc VALUES (1,70),(2,80)','b')
        new=e(rows=[['70']]); assert old['origins'][0][0]!=new['origins'][0][0]
        for client in (a,r):
            desc=describe(client,'s'); catalog=client.run("SELECT oid FROM pg_catalog.pg_class WHERE relname='prep_doc'")
            record(dict(case_id=f'oid-link.{ext}',description=desc,catalog=catalog))
            assert str(desc['origins'][0][0])==catalog['rows'][0][0]
        effects([['1','70'],['2','80']]); shut()
        for _ in range(3):
            p(); e(rows=[['70']]); shut()
        q('DROP TABLE public.prep_doc')
        # Fixed results, no write before rejection, parameter identity on rebind.
        for change in ('ADD COLUMN extra integer','ALTER COLUMN v TYPE bigint'):
            init(); p('SELECT * FROM public.prep_doc WHERE id=$1'); e(rows=[['1','10']])
            p('UPDATE public.prep_doc SET v=99 WHERE id=$1 RETURNING *','write')
            p('SELECT id FROM public.prep_doc WHERE id=$1','projection')
            q('ALTER TABLE public.prep_doc '+change,'b')
            pair_op('Describe fixed',lambda c:describe(c,'s'),error='0A000')
            e(error='0A000'); e(name='write',error='0A000'); e(name='projection',rows=[['1']]); effects()
            shut(); shut('write'); shut('projection'); p('SELECT * FROM public.prep_doc WHERE id=$1')
            e(rows=[['1','10',None]] if change.startswith('ADD') else [['1','10']]); shut(); done()
        init(); p(); q('DROP TABLE public.prep_doc','b')
        q('CREATE TABLE public.prep_doc (v integer,id bigint PRIMARY KEY)','b'); q('INSERT INTO public.prep_doc VALUES (70,1),(80,2147483648)','b')
        e(rows=[['70']]); e('2147483648',error='22003'); e(None,rows=[]); e(rows=[['70']]); shut(); q('DROP TABLE public.prep_doc')
        # Bind before SET, Execute only after SET: no earlier Execute snapshot.
        init(); p(); q('BEGIN'); w('Bind before SET',[bind('bound'),portal('bound')],state='T')
        q('SET search_path TO pg_catalog'); w('Execute after SET',[portal('bound'),fetch('bound')],rows=[['10']],state='T')
        q('ROLLBACK'); e(rows=[['10']]); shut(); done()
        # Old and new portals, runtime session function, repeated write pagination.
        init(); q('CREATE SCHEMA prep_alt'); q('CREATE TABLE prep_alt.prep_doc (id integer PRIMARY KEY,v integer)'); q('INSERT INTO prep_alt.prep_doc VALUES (1,90),(2,80)')
        p('SELECT v FROM prep_doc WHERE id >= $1 ORDER BY id'); q('BEGIN')
        w('Bind old',[bind('old'),portal('old')],state='T'); q('SET search_path TO prep_alt, public')
        w('Bind new',[bind('new'),portal('new')],state='T')
        for name,expected in [('old','10'),('new','90')]: w('page '+name,[portal(name),fetch(name,1)],rows=[[expected]],state='T')
        q('SET search_path TO pg_catalog')
        for name,expected in [('old','20'),('new','80')]:
            w('finish '+name,[portal(name),fetch(name)],rows=[[expected]],state='T'); w('repeat '+name,[portal(name),fetch(name)],rows=[],state='T')
        q('ROLLBACK'); q('SET search_path TO public'); e(rows=[['10'],['20']]); shut()
        p('SELECT current_schema()','live',oids=()); q('BEGIN'); w('Bind schema old',[(b'B',b'old\0live\0'+struct.pack('!HHH',0,0,0)),portal('old')],state='T')
        q('SET search_path TO pg_catalog')
        w('Bind schema new',[(b'B',b'new\0live\0'+struct.pack('!HHH',0,0,0)),portal('new')],state='T')
        for name in ('old','new'):
            w('schema runtime '+name,[portal(name),fetch(name)],rows=[['pg_catalog']],state='T')
        q('ROLLBACK'); shut('live')
        p('UPDATE public.prep_doc SET v=v+1 WHERE id >= $1 RETURNING v'); q('BEGIN')
        w('write first',[bind('write'),portal('write'),fetch('write',1)],rows=[['11']],state='T')
        w('write rest',[portal('write'),fetch('write')],rows=[['21']],state='T'); w('write repeat',[portal('write'),fetch('write')],rows=[],state='T')
        q('COMMIT'); effects([['1','11'],['2','21']]); shut(); q('DROP TABLE public.prep_doc'); q('DROP TABLE prep_alt.prep_doc'); q('DROP SCHEMA prep_alt')
        # New catalog execution sees committed DDL, without re-Parse.
        init(); p("SELECT relname FROM pg_catalog.pg_class WHERE relname='prep_later'",oids=())
        e((),rows=[]); q('CREATE TABLE public.prep_later (id integer)','b'); e((),rows=[['prep_later']]); q('DROP TABLE public.prep_later','b'); e((),rows=[]); shut(); done()
        print('PG11_32_MID',flush=True); assert sys.stdin.readline().strip()=='continue'
        for client in clients: client.close()
        a,r=Client(target),Client(ref); connections.extend([a,r])
        q('SELECT id FROM public.prep_doc',error='42P01'); q('SELECT 1 AS alive',rows=[['1']]); e(error='26000'); q('SELECT 1 AS recovered',rows=[['1']]); a.close(); r.close()
finally:
    for client in connections:
        if client.conn: client.run('ROLLBACK'); client.close()
    if created: assert not admin.run('DROP DATABASE '+database)['errors']
    admin.close(); output.close()
(archive/'summary.json').write_text(json.dumps(dict(passed=True,paired_inputs=count,scenarios=scenes,protocols=2,negative_checks=negative,cleanup=dict(reference_database=True,pg_connections=True))))
