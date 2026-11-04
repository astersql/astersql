# Copyright 2026 AsterSQL.
"""RealTiKV sequence acceptance. The caller owns the isolated server lifecycle.
Arguments: target TCP port, target database. Restart is a stdin/stdout handshake.
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

archive = Path(os.environ["PG11_35_EVIDENCE_DIR"])
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


import concurrent.futures
import signal
signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

admin = Client(reference)
connections = []
created = False
checks = 0
scenarios = 0
negative = 0
refdb = f'pg1135_{os.getpid()}_{sys.argv[1]}'
try:
    for sql in ('SELECT version()', 'SHOW server_version_num'):
        o = admin.run(sql)
        record(dict(case_id='version', sql=sql, observation=o))
        assert not o['errors']
        if sql.startswith('SHOW'):
            assert o['rows'] == [['180006']]
    assert not admin.run('CREATE DATABASE ' + refdb)['errors']
    created = True
    ref = reference + ' dbname=' + refdb
    persistent = []
    for ext in (False, True):
        a,b,r,s = [Client(dsn) for dsn in (target,target,ref,ref)]
        connections.extend((a,b,r,s))
        suffix = 'e' if ext else 'q'
        name = 'seq_' + suffix
        arg = "'public." + name + "'::regclass"
        def q(sql, who='a', rows=None, error=None, state='I'):
            global checks
            x,y = (a,r) if who == 'a' else (b,s)
            l,rr = x.run(sql,ext),y.run(sql,ext)
            record(dict(case_id=f'{suffix}.{checks}',sql=sql,target=l,reference=rr))
            equal(l,rr)
            assert [e['C'] for e in l['errors']] == ([error] if error else [])
            assert l['ready'] == state
            if rows is not None:
                assert l['rows'] == rows, (sql,l['rows'],rows)
            checks += 1
            return l
        def value(fn, expected=None, who='a', error=None, state='I'):
            return q(f'SELECT {fn}({arg}) AS v',who,
                     None if error else [[None if expected is None else str(expected)]],error,state)
        q(f'CREATE SEQUENCE public.{name} AS bigint START 10 MINVALUE 1 MAXVALUE 1000 CACHE 1 NO CYCLE')
        # Each endpoint links its own actual identity, including across restart.
        identities=[]
        for client in (a,r):
            identity=client.run(f"SELECT oid,relkind FROM pg_class WHERE relname='{name}'",ext)
            assert not identity['errors'] and len(identity['rows'])==1
            assert identity['rows'][0][1]=='S'
            identities.append(identity['rows'][0][0])
            record(dict(case_id=suffix+'.identity',observation=identity))
        value('currval',error='55000'); value('nextval',10)
        value('currval',error='55000',who='b'); value('nextval',11,who='b')
        baseline=value('currval',10)
        value('currval',11,who='b'); scenarios+=1
        q(f'SELECT setval({arg},40,true) AS v',rows=[['40']])
        value('currval',40); value('currval',11,who='b'); value('nextval',41,who='b')
        q(f'SELECT setval({arg},50,false) AS v',rows=[['50']])
        value('currval',40); value('nextval',50,who='b'); value('nextval',51)
        scenarios+=1
        q('CREATE TABLE public.rows_'+suffix+' (id integer PRIMARY KEY,v bigint)')
        q('BEGIN',state='T'); value('nextval',52,state='T')
        q('INSERT INTO public.rows_'+suffix+' VALUES(1,52)',state='T')
        q('ROLLBACK'); q('SELECT id,v FROM public.rows_'+suffix,rows=[])
        value('currval',52); value('nextval',53,who='b'); scenarios+=1
        q(f'SELECT setval({arg},0,true) AS v',error='22003')
        q('SELECT 1 AS recovered',rows=[['1']])
        q(f'SELECT setval({arg},NULL,true) AS v',rows=[[None]])
        q('BEGIN',state='T'); value('nextval',54,state='T')
        q(f'SELECT setval({arg},1001,false) AS v',error='22003',state='E')
        q('SELECT 1',error='25P02',state='E'); q('ROLLBACK')
        value('currval',54); scenarios+=1
        # Concurrent order is nondeterministic; compare the complete set and
        # every result descriptor, rather than equating cross-connection order.
        def allocate(dsn):
            client=Client(dsn)
            try:
                return [client.run(f'SELECT nextval({arg}) AS v',ext) for _ in range(50)]
            finally:
                client.close()
        all_observations=[]
        for endpoint,dsn in (('target',target),('reference',ref)):
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                batches=list(pool.map(allocate,[dsn]*4))
            observations=[o for batch in batches for o in batch]
            values=[]
            for o in observations:
                assert not o['errors'] and o['ready']=='I'
                assert o['metadata']==baseline['metadata']
                values.append(int(o['rows'][0][0]))
            assert sorted(values)==list(range(55,255)),(endpoint,values)
            record(dict(case_id=suffix+'.concurrent',endpoint=endpoint,observations=observations,values=values))
            all_observations.append(values)
        scenarios+=1
        # Negative self-checks cover values, SQLSTATE and durable side effects.
        for field in ('rows','errors'):
            mutated=copy.deepcopy(baseline)
            mutated[field]=[['bad']] if field=='rows' else [{'C':'99999'}]
            try: equal(mutated,baseline)
            except AssertionError: negative+=1
            else: raise AssertionError('differential failed negative '+field)
        values=all_observations[0].copy(); values[0]=-1
        try: assert sorted(values)==list(range(55,255))
        except AssertionError: negative+=1
        else: raise AssertionError('side-effect comparator failed negative check')
        # Persist both called states without consuming them before shutdown.
        for state_name,called in (('called','true'),('uncalled','false')):
            reset_name=name+'_'+state_name
            q(f'CREATE SEQUENCE public.{reset_name} AS bigint CACHE 1')
            q(f"SELECT setval('public.{reset_name}'::regclass,80,{called}) AS v",rows=[['80']])
        # setval also survives an explicit user rollback.
        reset_name=name+'_rollback'
        q(f'CREATE SEQUENCE public.{reset_name} AS bigint CACHE 1')
        q('BEGIN',state='T')
        q(f"SELECT setval('public.{reset_name}'::regclass,90,true) AS v",rows=[['90']],state='T')
        q('ROLLBACK')
        persistent.append((name,identities))
        for client in (a,b,r,s): client.close()
    # The supervising Rust test waits for a real normal server restart on the
    # same PD. No client remains open and no sequence state is copied by us.
    print('PG11_35_RESTART',flush=True)
    assert sys.stdin.readline().strip()=='restarted', 'normal restart acknowledgement required'
    for ext,(name,identities) in zip((False,True),persistent):
        a,r=Client(target),Client(ref); connections.extend((a,r))
        arg="'public."+name+"'::regclass"
        for endpoint,client,oid in zip(('target','reference'),(a,r),identities):
            identity=client.run(f"SELECT oid,relkind FROM pg_class WHERE relname='{name}'",ext)
            assert identity['rows']==[[oid,'S']],identity
            record(dict(case_id=name+'.restart.identity',endpoint=endpoint,observation=identity))
        for sql,rows,error in ((f'SELECT currval({arg}) AS v',None,'55000'),
                               (f'SELECT nextval({arg}) AS v',[['255']],None),
                               (f'SELECT currval({arg}) AS v',[['255']],None)):
            l,rr=a.run(sql,ext),r.run(sql,ext); equal(l,rr)
            assert [e['C'] for e in l['errors']]==([error] if error else [])
            if rows is not None: assert l['rows']==rows
            record(dict(case_id=name+'.restart.value',sql=sql,target=l,reference=rr)); checks+=1
        scenarios+=1
        for state_name,expected in (('called','81'),('uncalled','80'),('rollback','91')):
            sql=f"SELECT nextval('public.{name}_{state_name}'::regclass) AS v"
            l,rr=a.run(sql,ext),r.run(sql,ext); equal(l,rr)
            assert l['rows']==[[expected]] and not l['errors']
            record(dict(case_id=name+'.restart.'+state_name,sql=sql,target=l,reference=rr))
            checks+=1; scenarios+=1
            for client in (a,r):
                assert not client.run(f'DROP SEQUENCE public.{name}_{state_name}',ext)['errors']
        for client in (a,r):
            assert not client.run('DROP TABLE public.rows_'+('e' if ext else 'q'),ext)['errors']
            assert not client.run('DROP SEQUENCE public.'+name,ext)['errors']
            client.close()
    record(dict(summary=True,scenarios=scenarios,comparisons=checks,concurrent_values=800,negative_checks=negative))
    assert scenarios==18 and negative==6
finally:
    for client in connections:
        if client.conn:
            client.run('ROLLBACK'); client.close()
    if created: assert not admin.run('DROP DATABASE '+refdb)['errors']
    admin.close()
    output.close()
