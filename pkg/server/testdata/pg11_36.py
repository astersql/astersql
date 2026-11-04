# Copyright 2026 AsterSQL.
"""RealTiKV identity acceptance. The caller owns the isolated server lifecycle.
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

archive = Path(os.environ["PG11_36_EVIDENCE_DIR"])
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


import signal
signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
admin=Client(reference)
connections=[]
created=False
checks=scenarios=negative=0
persistent=[]
refdb=f'pg1136_{os.getpid()}_{sys.argv[1]}'
try:
    for sql in ('SELECT version()','SHOW server_version_num'):
        o=admin.run(sql); record(dict(case_id='version',sql=sql,observation=o))
        assert not o['errors']
        if sql.startswith('SHOW'): assert o['rows']==[['180006']]
    assert not admin.run('CREATE DATABASE '+refdb)['errors']; created=True
    ref=reference+' dbname='+refdb
    for ext in (False,True):
        a,b,r,s=[Client(d) for d in (target,target,ref,ref)]; connections.extend((a,b,r,s))
        suffix='e' if ext else 'q'
        def q(sql,rows=None,error=None,state='I',observer=False):
            global checks
            x,y=(b,s) if observer else (a,r)
            l,rr=x.run(sql,ext),y.run(sql,ext)
            record(dict(case_id=f'{suffix}.{checks}',sql=sql,target=l,reference=rr)); equal(l,rr)
            assert [e['C'] for e in l['errors']]==([error] if error else [])
            assert l['ready']==state
            if sql.startswith('INSERT') and not l['errors']:
                for observation,identity in zip((l,rr),identities):
                    assert observation['origins']==[[int(identity[2]),1],[int(identity[2]),2]]
            if rows is not None: assert l['rows']==rows,(sql,l['rows'],rows)
            checks+=1
            return l
        for mode in ('ALWAYS','BY DEFAULT'):
            for case in ('generate','explicit','rollback','failure'):
                name=f'id_{suffix}_{mode[0].lower()}_{case}'; table='public.'+name
                q(f'CREATE TABLE {table}(id bigint GENERATED {mode} AS IDENTITY (START WITH 10 MINVALUE 1 MAXVALUE 1000 CACHE 1) PRIMARY KEY,note text UNIQUE)')
                identities=[]
                for client in (a,r):
                    seqobs=client.run(f"SELECT pg_get_serial_sequence('{table}','id')",ext)
                    assert not seqobs['errors']; seq=seqobs['rows'][0][0]
                    oidobs=client.run(f"SELECT oid,relkind FROM pg_class WHERE oid='{seq}'::regclass",ext)
                    tidobs=client.run(f"SELECT oid FROM pg_class WHERE relname='{name}'",ext)
                    assert not oidobs['errors'] and oidobs['rows'][0][1]=='S'
                    identities.append((seq,oidobs['rows'][0][0],tidobs['rows'][0][0]))
                    record(dict(case_id=name+'.identity',sequence=seqobs,identity=oidobs,table=tidobs))
                q(f"SELECT attidentity,attnotnull,atthasdef,atttypid FROM pg_attribute WHERE attrelid='{table}'::regclass AND attname='id'",[['a' if mode=='ALWAYS' else 'd','t','f','20']])
                def ins(values,rows=None,error=None,state='I',override=''):
                    return q(f'INSERT INTO {table}(id,note) {override} VALUES {values} RETURNING id,note',rows,error,state)
                if case=='generate':
                    baseline=q(f"INSERT INTO {table}(note) VALUES('omit') RETURNING id,note",[['10','omit']])
                    ins("(DEFAULT,'default')",[['11','default']])
                    q(f'INSERT INTO {table} DEFAULT VALUES RETURNING id,note',[['12',None]])
                    expected=[['10','omit'],['11','default'],['12',None]]; nextid=13
                elif case=='explicit':
                    ins("(900,'manual')",[['900','manual']],override='OVERRIDING SYSTEM VALUE' if mode=='ALWAYS' else '')
                    ins("(NULL,'null')",error='23502',override='OVERRIDING SYSTEM VALUE')
                    q('SELECT 1 AS ok',[['1']])
                    q(f"INSERT INTO {table}(note) VALUES('next') RETURNING id,note",[['10','next']])
                    expected=[['10','next'],['900','manual']]; nextid=11
                elif case=='rollback':
                    q('BEGIN',state='T'); ins("(DEFAULT,'undone')",[['10','undone']],state='T')
                    q(f'SELECT id,note FROM {table} ORDER BY id',[],observer=True)
                    q('ROLLBACK'); expected=[]; nextid=11
                else:
                    ins("(DEFAULT,'seed')",[['10','seed']]); q('BEGIN',state='T')
                    ins("(DEFAULT,'seed')",error='23505',state='E'); q('SELECT 1',error='25P02',state='E'); q('ROLLBACK')
                    expected=[['10','seed']]; nextid=12
                snapshot=q(f'SELECT id,note FROM {table} ORDER BY id',expected,observer=True)
                persistent.append((ext,name,mode,identities,expected,nextid)); scenarios+=1
        for field in ('rows','errors'):
            changed=copy.deepcopy(baseline); changed[field]=[['bad']] if field=='rows' else [{'C':'99999'}]
            try: equal(changed,baseline)
            except AssertionError: negative+=1
            else: raise AssertionError('negative comparison accepted '+field)
        changed=copy.deepcopy(snapshot); changed['rows'].append(['999','orphan'])
        try: equal(changed,snapshot)
        except AssertionError: negative+=1
        else: raise AssertionError('negative side effect accepted')
        for client in (a,b,r,s): client.close()
    print('PG11_36_RESTART',flush=True)
    assert sys.stdin.readline().strip()=='restarted'
    for ext,name,mode,identities,expected,nextid in persistent:
        a,r=Client(target),Client(ref); connections.extend((a,r)); table='public.'+name
        for endpoint,client,(seq,oid,tid) in zip(('target','reference'),(a,r),identities):
            for sql,rows in (
                (f"SELECT oid,relkind FROM pg_class WHERE oid='{seq}'::regclass",[[oid,'S']]),
                (f"SELECT oid FROM pg_class WHERE relname='{name}'",[[tid]]),
                (f"SELECT pg_get_serial_sequence('{table}','id')",[[seq]]),
                (f"SELECT deptype,refobjsubid FROM pg_depend WHERE objid='{seq}'::regclass AND refobjid='{table}'::regclass",[['i','1']])):
                o=client.run(sql,ext); record(dict(case_id=name+'.restart.identity',endpoint=endpoint,sql=sql,observation=o))
                assert not o['errors'] and o['rows']==rows
        observations=[client.run(f"SELECT currval('{identity[0]}'::regclass) AS v",ext) for client,identity in zip((a,r),identities)]
        equal(*observations); assert [e['C'] for e in observations[0]['errors']]==['55000']
        record(dict(case_id=name+'.restart.currval',target=observations[0],reference=observations[1])); checks+=1
        for sql,rows,error in (
            (f"SELECT attidentity,attnotnull,atthasdef,atttypid FROM pg_attribute WHERE attrelid='{table}'::regclass AND attname='id'",[['a' if mode=='ALWAYS' else 'd','t','f','20']],None),
            (f'SELECT id,note FROM {table} ORDER BY id',expected,None),
            (f"INSERT INTO {table}(note) VALUES('restarted') RETURNING id,note",[[str(nextid),'restarted']],None),
            (f"INSERT INTO {table}(id,note) VALUES(950,'mode-check') RETURNING id,note",None if mode=='ALWAYS' else [['950','mode-check']],'428C9' if mode=='ALWAYS' else None)):
            l,rr=a.run(sql,ext),r.run(sql,ext)
            record(dict(case_id=name+'.restart.value',sql=sql,target=l,reference=rr)); equal(l,rr)
            assert [e['C'] for e in l['errors']]==([error] if error else [])
            if sql.startswith('INSERT') and not error:
                for observation,identity in zip((l,rr),identities):
                    assert observation['origins']==[[int(identity[2]),1],[int(identity[2]),2]]
            if rows is not None: assert l['rows']==rows
            checks+=1
        expected=expected+[[str(nextid),'restarted']]+([['950','mode-check']] if mode=='BY DEFAULT' else [])
        expected.sort(key=lambda row:int(row[0]))
        sql=f'SELECT id,note FROM {table} ORDER BY id'
        l,rr=a.run(sql,ext),r.run(sql,ext); equal(l,rr); assert l['rows']==expected
        record(dict(case_id=name+'.restart.effects',sql=sql,target=l,reference=rr)); checks+=1
        for endpoint,client,(seq,oid,tid) in zip(('target','reference'),(a,r),identities):
            assert not client.run('DROP TABLE '+table,ext)['errors']
            for sql in (f'SELECT oid FROM pg_class WHERE oid={oid}',f'SELECT objid FROM pg_depend WHERE objid={oid}'):
                o=client.run(sql,ext)
                record(dict(case_id=name+'.drop',endpoint=endpoint,sql=sql,observation=o))
                assert not o['errors'] and o['rows']==[],o
            client.close()
        scenarios+=1
    record(dict(summary=True,scenarios=scenarios,comparisons=checks,negative_checks=negative))
    assert scenarios==32 and negative==6
finally:
    for client in connections:
        if client.conn: client.run('ROLLBACK'); client.close()
    if created: assert not admin.run('DROP DATABASE '+refdb)['errors']
    admin.close(); output.close()
