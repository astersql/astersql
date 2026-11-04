# Copyright 2026 AsterSQL.
"""Quoted-name differential. Arguments: target TCP port, target database.

Queries use PQexec/PQexecParams; libpq protocol traces preserve error phase,
RowDescription source, and ReadyForQuery. The idle socket is used only to probe
an extra Sync after a failed extended query.
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

archive = Path(os.environ["PG11_31_EVIDENCE_DIR"])
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
    def run(self, sql, ext=False):
        self.notices.clear()
        result = (exec_params(self.conn, sql.encode(), 0, None, None, None, None, 0)
                  if ext else exec_query(self.conn, sql.encode()))
        assert result, 'libpq returned no result'
        try:
            code = result_status(result)
            assert code in (1, 2, 7), f'unexpected libpq result status {code}'
            o = dict(rows=[], metadata=[], origins=[], errors=[],
                     warnings=list(self.notices), command=[], ready='',
                     phase='parse' if ext else 'query')
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
            for line in trace.splitlines():
                fields = line.split('\t')
                if len(fields) < 3 or fields[0] != 'B': continue
                tag = fields[2]
                if tag == 'ParseComplete': o['phase'] = 'bind'
                elif tag == 'BindComplete': o['phase'] = 'describe-portal'
                elif tag in ('RowDescription','NoData'):
                    o['phase'] = 'execute' if ext else 'query'
                    if tag == 'RowDescription':
                        o['description_source'] = 'Describe(portal)' if ext else 'Query'
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
        'description_source': o.get('description_source')}
def equal(a, b):
    assert semantic(a) == semantic(b), (semantic(a), semantic(b))

admin = Client(reference)
database = f'pg1131_{os.getpid()}_{sys.argv[1]}'
connections = []
created = False
count = 0
negative = 0
try:
    for sql in ['SELECT version()', 'SHOW server_version_num']:
        provenance = admin.run(sql)
        record(dict(case_id='provenance', sql=sql, observation=provenance))
        if sql == 'SHOW server_version_num':
            assert provenance['rows'] == [['180006']], 'fixed PostgreSQL 18.6 required'
    assert not admin.run('CREATE DATABASE '+database)['errors']
    created = True
    ref = reference + ' dbname=' + database
    for ext in (False, True):
        clients = [Client(dsn) for dsn in [target,target,ref,ref]]
        connections.extend(clients)
        a,b,r,s = clients
        def pair(sql, who='a', rows=None, error=None):
            global count
            x,y = (a,r) if who == 'a' else (b,s)
            left,right = x.run(sql,ext),y.run(sql,ext)
            record(dict(case_id=f'{ext}.{count}.{who}',sql=sql,
                        expected=dict(rows=rows,sqlstate=error),target=left,reference=right))
            count += 1
            equal(left,right)
            if left['phase'] != right['phase']:
                # Shared unknown UPDATE-target validation runs at Execute;
                # keep this measured boundary explicit, never claim phase parity.
                assert sql == 'UPDATE public.t SET "cC"=99 WHERE id=1'
                assert (left['phase'],right['phase']) == ('execute','parse')
            if rows is not None: assert left['rows'] == rows
            if error: assert [e['C'] for e in left['errors']] == [error]
            return left
        for who in ['a','b']: pair('SET search_path TO public',who)
        def setup():
            for sql in ['CREATE TABLE public.t (id integer PRIMARY KEY,c integer,"C" integer)',
                        'INSERT INTO public.t VALUES (1,10,11)',
                        'CREATE TABLE public."T" (id integer PRIMARY KEY,c integer)',
                        'INSERT INTO public."T" VALUES (2,20)',
                        'CREATE TABLE public."a.b" (id integer PRIMARY KEY)',
                        'INSERT INTO public."a.b" VALUES (3)']:
                assert not pair(sql)['errors']
        def effects(upper='20',cap='11',who='a'):
            observations=[]
            for sql,rows in [('SELECT id,c,"C" FROM public.t ORDER BY id',[['1','10',cap]]),
                             ('SELECT id,c FROM public."T" ORDER BY id',[['2',upper]]),
                             ('SELECT id FROM public."a.b" ORDER BY id',[['3']])]:
                observations.append(pair(sql,who,rows=rows))
            return observations
        def cleanup():
            for sql in ['DROP TABLE public."a.b"','DROP TABLE public."T"','DROP TABLE public.t']:
                assert not pair(sql)['errors']
        setup(); effects(); effects(who='b')
        reads = [('SELECT id FROM '+name,[['1']]) for name in
                 ['t','T','"t"','public.t','PUBLIC.T','"public"."t"']]
        reads += [('SELECT id FROM '+name,[['2']]) for name in ['"T"','public."T"']]
        reads += [('SELECT c,C,"c","C" FROM public.t',[['10','10','10','11']]),
                  ('SELECT A.c FROM public.t AS A',[['10']]),
                  ('SELECT "A".c AS "V" FROM public.t AS "A"',[['10']]),
                  ('SELECT c AS V,c AS "V" FROM public.t',[['10','10']]),
                  ('SELECT id FROM public."a.b"',[['3']]),
                  ('''SELECT 'public.T', '"T"', 'a.b' FROM public.t /* public."T" */''', [['public.T','"T"','a.b']])]
        for sql,rows in reads:
            pair(sql,rows=rows); effects(); effects(who='b')
        # Current-database qualification is substituted independently per endpoint.
        left=a.run('SELECT id FROM "'+sys.argv[2]+'".public.t',ext)
        right=r.run('SELECT id FROM "'+database+'".public.t',ext)
        record(dict(case_id=f'current-db.{ext}',target=left,reference=right,
                    target_sql='SELECT id FROM "'+sys.argv[2]+'".public.t',
                    reference_sql='SELECT id FROM "'+database+'".public.t'))
        equal(left,right); count += 1
        for sql,code in [('SELECT "cC" FROM public.t','42703'),
                         ('SELECT a.c FROM public.t AS "A"','42P01'),
                         ('SELECT t.c FROM public.t AS "A"','42P01'),
                         ('SELECT id FROM other_db.public.t','0A000'),
                         ('SELECT id FROM "PUBLIC".t','42P01'),
                         ('SELECT id FROM public.""','42601'),
                         ('SELECT id FROM public."broken','42601'),
                         ('UPDATE public.t SET "cC"=99 WHERE id=1','42703'),
                         ('UPDATE public."Missing" SET c=99 WHERE id=2','42P01')]:
            pair(sql,error=code); pair('SELECT id FROM t',rows=[['1']]); effects(); effects(who='b')
        # Link source OID/attribute numbers to each endpoint's real catalog.
        for side,x,y in [('target',a,b),('reference',r,s)]:
            for table,colnums in [('t',[1,2,3]),('T',[1,2]),('a.b',[1])]:
                cols='id,c,"C"' if table=='t' else ('id,c' if table=='T' else 'id')
                sql=f'SELECT {cols} FROM public."{table}"'
                result=x.run(sql,ext)
                oid=x.run("SELECT c.oid::bigint FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='"+table+"'",ext)
                record(dict(case_id=f'identity.{ext}.{side}.{table}',result=result,catalog=oid))
                assert len(oid['rows'])==1
                assert result['origins']==[[int(oid['rows'][0][0]),n] for n in colnums]
                assert y.run(sql,ext)['origins']==result['origins']
        cleanup()
        failures = [('SELECT "cC" FROM public.t','42703'),
                    ('UPDATE public.t SET "cC"=99 WHERE id=1','42703'),
                    ('UPDATE public."Missing" SET c=99 WHERE id=2','42P01'),
                    ('SELECT id FROM public.""','42601'),
                    ('SELECT id FROM public."broken','42601'),
                    ('SELECT a.c FROM public.t AS "A"','42P01')]
        for ending,failure,code in [('ROLLBACK',None,None),('COMMIT',None,None)] + [
                ('FAILED',sql,code) for sql,code in failures]:
            setup(); effects(); effects(who='b')
            pair('BEGIN'); pair('UPDATE public."T" SET c=21 WHERE id=2')
            pair('UPDATE public.t SET "C"=NULL WHERE id=1')
            effects('21',None); effects(who='b')
            if ending=='FAILED':
                error=pair(failure,error=code)
                pair('SELECT c FROM public."T"',error='25P02')
                if ext:
                    for x in [a,r]:
                        x.send(b'S',b''); tag=x.exact(1); length=struct.unpack('!I',x.exact(4))[0]
                        assert tag==b'Z' and x.exact(length-4)==b'E'
                pair('ROLLBACK')
                observed=effects(); effects(who='b')
                for field in (['rows','errors','side_effects'] if failure==failures[0][0] else []):
                    original=error if field=='errors' else observed[1 if field=='side_effects' else 0]
                    changed=copy.deepcopy(original)
                    if field=='errors': changed['errors'][0]['C']='XX000'
                    else: changed['rows'][0][1]='999'
                    try: equal(original,changed)
                    except AssertionError:
                        negative += 1
                        record(dict(case_id=f'negative.{ext}.{field}',detected=True))
                    else: raise AssertionError('negative comparator check missed '+field)
            else:
                pair(ending)
                effects('21' if ending=='COMMIT' else '20',None if ending=='COMMIT' else '11')
                effects('21' if ending=='COMMIT' else '20',None if ending=='COMMIT' else '11',who='b')
            cleanup()
        pair('CREATE TABLE public.t (id integer PRIMARY KEY,c integer,"C" integer)')
        pair('INSERT INTO public.t VALUES (1,10,11)')
        pair('SELECT id FROM "T"',error='42P01'); pair('SELECT id FROM t',rows=[['1']])
        pair('DROP TABLE public.t')
        print('PG11_31_MID',flush=True)
        assert sys.stdin.readline().strip()=='continue'
        for mode in ['EMPTY','ANSI']:
            print('PG11_31_MODE_'+mode,flush=True)
            assert sys.stdin.readline().strip()=='continue'
            setup(); effects(); effects(who='b')
            pair('SELECT c,C,"c","C" FROM public.t',rows=[['10','10','10','11']])
            cleanup()
            print('PG11_31_MODE_CHECK',flush=True)
            assert sys.stdin.readline().strip()=='continue'
        for client in clients: client.close()
        a,r=Client(target),Client(ref); connections.extend([a,r])
        pair('SELECT id FROM public.t',error='42P01'); pair('SELECT 1 AS alive',rows=[['1']])
        a.close(); r.close()
finally:
    for client in connections:
        if client.conn:
            client.run('ROLLBACK'); client.close()
    if created: assert not admin.run('DROP DATABASE '+database)['errors']
    admin.close(); output.close()
(archive/'summary.json').write_text(json.dumps(dict(passed=True,paired_inputs=count,
    protocols=2,negative_checks=negative,cleanup=dict(reference_database=True,
    pg_connections=True))))
