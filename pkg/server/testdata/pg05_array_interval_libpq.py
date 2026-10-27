# Copyright 2026 AsterSQL.
"""interval array libpq differential; owns only its temporary PostgreSQL reference cluster."""
import ctypes as c
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile

pq = c.CDLL(os.environ.get('PG_LIBPQ_LIBRARY', '/opt/homebrew/opt/libpq/lib/libpq.dylib'))
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
p, s, i = c.c_void_p, c.c_char_p, c.c_int
connect = api('PQconnectdb', p, s)
connected = api('PQstatus', i, p)
finish = api('PQfinish', None, p)
execute = api('PQexec', p, p, s)
prepare = api('PQprepare', p, p, s, s, i, c.POINTER(c.c_uint))
describe = api('PQdescribePrepared', p, p, s)
close_prepared = api('PQclosePrepared', p, p, s)
nparams = api('PQnparams', i, p)
paramtype = api('PQparamtype', c.c_uint, p, i)
prepared = api('PQexecPrepared', p, p, s, i, c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
status = api('PQresultStatus', i, p)
ef = api('PQresultErrorField', s, p, i)
rows = api('PQntuples', i, p)
cols = api('PQnfields', i, p)
value = api('PQgetvalue', p, p, i, i)
length = api('PQgetlength', i, p, i, i)
null = api('PQgetisnull', i, p, i, i)
ftype = api('PQftype', c.c_uint, p, i)
fsize = api('PQfsize', i, p, i)
fmod = api('PQfmod', i, p, i)
fformat = api('PQfformat', i, p, i)
clear = api('PQclear', None, p)
version = api('PQlibVersion', i)
assert version() >= 180000

def observe(result):
    assert result, 'libpq returned no result'
    try:
        return dict(status=status(result), state=(ef(result, ord('C')) or b'').decode() or 'N/A',
                    metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))],
                    rows=[[None if null(result,r,n) else c.string_at(value(result,r,n), length(result,r,n)).hex() for n in range(cols(result))] for r in range(rows(result))])
    finally:
        clear(result)

def query(conn, sql):
    result = observe(execute(conn, sql.encode()))
    assert result['status'] in [1, 2], (sql, result)
    return result

def bind(conn, name, data, infmt, outfmt):
    # c_char_p preserves the pointer, including NULs inside binary payloads;
    # explicit lengths are authoritative only in binary format.
    return observe(prepared(conn, name, 1, (s*1)(data),
                            (i*1)(len(data) if data is not None else 0), (i*1)(infmt), outfmt))

def interrupted(signum, frame):
    raise KeyboardInterrupt(f'signal {signum}')
signal.signal(signal.SIGTERM, interrupted)

binpath = Path(os.environ.get('PG18_BIN', '/opt/homebrew/opt/postgresql@18/bin'))
# Reference-only is a diagnostic mode. It never reports differential acceptance.
reference_only = sys.argv[1] == '--reference-only'
evidence = Path(sys.argv[3])
evidence.mkdir(parents=True, exist_ok=True)
summary = dict(passed=False, mode='reference-only' if reference_only else 'differential',
               libpq=version(), cases=0)
connections = []
count = 0
try:
    with tempfile.TemporaryDirectory(prefix='pg05-iv-', dir='/tmp') as root:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        started = False
        try:
            subprocess.run([str(binpath/'initdb'), '-D', root+'/data', '-U', 'root', '-A', 'trust', '--encoding=UTF8', '--no-locale'], check=True, stdout=subprocess.DEVNULL)
            started = True
            subprocess.run([str(binpath/'pg_ctl'), '-D', root+'/data', '-l', root+'/log', '-o', f'-h 127.0.0.1 -p {port} -k {root}', '-w', 'start'], check=True, stdout=subprocess.DEVNULL)
            endpoints = [f'port={port} dbname=postgres']
            if not reference_only:
                endpoints.append(f'port={int(sys.argv[1])} dbname={sys.argv[2]}')
            for endpoint in endpoints:
                conn = connect(f'host=127.0.0.1 user=root connect_timeout=10 {endpoint}'.encode())
                assert conn
                connections.append(conn)
                assert connected(conn) == 0, 'client connection failed'
            identity = {key: query(connections[0], sql) for key, sql in [
                ('version', 'SELECT version()'), ('server_version_num', 'SHOW server_version_num'),
                ('catalog_types', 'SELECT oid,typelem,typlen FROM pg_type WHERE oid IN (1186,1187) ORDER BY oid'),
                ('server_encoding', 'SHOW server_encoding'), ('client_encoding', 'SHOW client_encoding'),
                ('standard_conforming_strings', 'SHOW standard_conforming_strings'),
                ('array_nulls', 'SHOW array_nulls'),
                ('IntervalStyle', 'SHOW IntervalStyle'), ('DateStyle', 'SHOW DateStyle'), ('TimeZone', 'SHOW TimeZone')]}
            actual_version = int(bytes.fromhex(identity['server_version_num']['rows'][0][0]))
            summary.update(reference=identity, reference_version_num=actual_version,
                           reference_build=subprocess.check_output([str(binpath/'pg_config'), '--configure'], text=True).strip())
            (evidence/'identity.json').write_text(json.dumps(summary, indent=2))
            print(f'reference_version_num={actual_version}', flush=True)
            assert actual_version == 180006, 'exact PostgreSQL 18.6 required'
            assert identity['catalog_types']['rows'] == [[b'1186'.hex(), b'0'.hex(), b'16'.hex()], [b'1187'.hex(), b'1186'.hex(), b'-1'.hex()]]
            assert identity['array_nulls']['rows'] == [[b'on'.hex()]], 'array_nulls=on required'
            with (evidence/'cases.jsonl').open('w') as records:
                def record(phase, name, data, infmt, outfmt, expected):
                    global count
                    observed = [bind(conn, name, data, infmt, outfmt) for conn in connections]
                    entry = dict(phase=phase, statement=name.decode(), input_format=infmt,
                                 output_format=outfmt, parameter_oid=parameter_oids[name],
                                 input=None if data is None else data.hex(), reference=observed[0])
                    if not reference_only:
                        entry['aster'] = observed[1]
                    records.write(json.dumps(entry)+'\n')
                    records.flush()
                    for result in observed:
                        assert result == expected, entry
                    count += 1

                import struct
                def words(*values):
                    return struct.pack('!'+'i'*len(values), *values)
                def array(shape, lower, values, flags=None):
                    return words(len(shape), int(any(v is None for v in values)) if flags is None else flags, 1186) + b''.join(words(n,l) for n,l in zip(shape,lower)) + b''.join(words(-1) if v is None else words(len(v))+v for v in values)
                parameter_oids = {}
                statements = [
                    (b'declared', b'SELECT $1 AS v', [1187]),
                    (b'inferred', b'SELECT $1::interval[] AS v', []),
                    (b'zero', b'SELECT $1::interval[] AS v', [0]),
                    (b'empty', b'SELECT $1::interval[] AS v WHERE false', [1187]),
                    (b'first_null', b'SELECT NULL::interval[] AS v UNION ALL SELECT $1::interval[] AS v', [1187])]
                for name, sql, oids in statements:
                    parameter_oids[name] = 1187
                    for index, conn in enumerate(connections):
                        result = observe(prepare(conn, name, sql, len(oids), (c.c_uint*len(oids))(*oids)))
                        assert result == dict(status=1, state='N/A', metadata=[], rows=[]), result
                        result = describe(conn, name)
                        try:
                            observed = dict(status=status(result), parameter_oids=[paramtype(result,n) for n in range(nparams(result))], metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))])
                            records.write(json.dumps(dict(phase='Parse/statement Describe', endpoint='reference' if index == 0 else 'aster', statement=name.decode(), observed=observed))+'\n')
                            assert observed == dict(status=1, parameter_oids=[1187], metadata=[[1187,-1,-1,0]]), observed
                        finally:
                            clear(result)
                times = [(b'1 mon',(1,0,0)),(b'30 days',(0,30,0)),(b'1 day',(0,1,0)),(b'24:00:00',(0,0,86400000000)),(b'-1 mons +2 days -03:04:05.123456',(-1,2,-11045123456)),(b'00:00:00',(0,0,0)),(b'infinity',(2147483647,2147483647,9223372036854775807)),(b'-infinity',(-2147483648,-2147483648,-9223372036854775808)),(b'178956970 years 7 mons',(2147483647,0,0)),(b'-178956970 years -8 mons',(-2147483648,0,0)),(b'2147483647 days',(0,2147483647,0)),(b'-2147483648 days',(0,-2147483648,0))]
                def scalar(parts):
                    months,days,micros=parts if isinstance(parts,tuple) else (0,0,parts)
                    return struct.pack('!qii',micros,days,months)
                def quote(text):
                    return b'"'+text.replace(b'\\',b'\\\\').replace(b'"',b'\\"')+b'"' if any(x in text for x in [b' ',b'"',b'\\',b',']) else text
                payloads = [(b'{'+quote(text)+b',NULL}',array([2],[1],[scalar(parts),None])) for text,parts in times]
                payloads += [
                    (b'{{"1 mon",NULL},{"1 day",24:00:00}}',array([2,2],[1,1],[scalar((1,0,0)),None,scalar((0,1,0)),scalar((0,0,86400000000))])),
                    (b'[0:1][-2:-1]={{"1 mon",NULL},{"1 day",24:00:00}}',array([2,2],[0,-2],[scalar((1,0,0)),None,scalar((0,1,0)),scalar((0,0,86400000000))])),
                    (b'[-3:-2]={00:00:00,00:00:00}',array([2],[-3],[scalar(0),scalar(0)])),
                    (b'{}',array([],[],[])),(b'{NULL}',array([1],[1],[None])),(b'{NULL,NULL}',array([2],[1],[None,None])),
                    (b'{{{{{{"1 mon"}}}}}}',array([1]*6,[1]*6,[scalar((1,0,0))])),(None,None)]
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        for name, _, _ in statements:
                            for text, binary in payloads:
                                data = binary if infmt else text
                                outgoing = binary if outfmt else text
                                result_rows = [] if name == b'empty' else ([[None]] if name == b'first_null' else []) + [[None if outgoing is None else outgoing.hex()]]
                                record('Bind/Execute/rebind/shape-null', name, data, infmt, outfmt,
                                       dict(status=2, state='N/A', metadata=[[1187,-1,-1,outfmt]], rows=result_rows))
                        # Input flags are not a promise about element NULLs; output is canonical.
                        if infmt:
                            for data, canonical in [(array([1],[1],[None],0),(b'{NULL}',array([1],[1],[None]))), (array([1],[1],[scalar(0)],1),(b'{00:00:00}',array([1],[1],[scalar(0)]))), (words(1,0,1186,0,-7),(b'{}',array([],[],[])))]:
                                outgoing = canonical[outfmt]
                                record('Bind/Execute/noncanonical-header', b'declared',data,infmt,outfmt,
                                       dict(status=2,state='N/A',metadata=[[1187,-1,-1,outfmt]],rows=[[outgoing.hex()]]))
                boundary_cases=0
                for token,parts in [(b'9223372036854.775807 seconds',(0,0,9223372036854775807)),(b'-9223372036854.775808 seconds',(0,0,-9223372036854775808)),(b'-2147483648 months 2147483647 days -9223372036854.775808 seconds',(-2147483648,2147483647,-9223372036854775808)),(b'1.5 months',(1,15,0)),(b'-2147483648 months 2147483647 days 9223372036854.775807 seconds',(-2147483648,2147483647,9223372036854775807))]:
                    ref=query(connections[0],"SELECT '"+token.decode()+"'::interval AS v")
                    displayed=bytes.fromhex(ref['rows'][0][0])
                    for infmt in [0,1]:
                        for outfmt in [0,1]:
                            data=array([2],[-2],[scalar(parts),None]) if infmt else b'[-2:-1]={'+quote(token)+b',NULL}'
                            outgoing=array([2],[-2],[scalar(parts),None]) if outfmt else b'[-2:-1]={'+quote(displayed)+b',NULL}'
                            record('Bind/Execute/component-extremes-fractional-month',b'declared',data,infmt,outfmt,dict(status=2,state='N/A',metadata=[[1187,-1,-1,outfmt]],rows=[[outgoing.hex()]]))
                            boundary_cases+=1
                summary['boundary_cases']=boundary_cases
                constructor_cases=0
                for conn in connections:
                    assert observe(prepare(conn,b'interval_constructor',b'SELECT ARRAY[$1::interval,NULL::interval] AS v',1,(c.c_uint*1)(1186)))['status']==1
                for text,day in times:
                    for infmt in [0,1]:
                        for outfmt in [0,1]:
                            data=scalar(day) if infmt else text
                            expected=(array([2],[1],[scalar(day),None]) if outfmt else b'{'+quote(text)+b',NULL}').hex()
                            observed=[bind(conn,b'interval_constructor',data,infmt,outfmt) for conn in connections]
                            entry=dict(phase='scalar parameter/ARRAY constructor',parameter_oid=1186,input=data.hex(),input_format=infmt,output_format=outfmt,observed=observed)
                            records.write(json.dumps(entry)+'\n');records.flush()
                            assert all(v==dict(status=2,state='N/A',metadata=[[1187,-1,-1,outfmt]],rows=[[expected]]) for v in observed),entry
                            constructor_cases+=1
                for conn in connections:
                    assert observe(close_prepared(conn,b'interval_constructor'))['status']==1
                summary['constructor_cases']=constructor_cases
                sql_cases=0
                for sql in ["SELECT ARRAY['1 month'::interval,'24 hours'::interval] AS v", "SELECT ARRAY['1.5 months'::interval,NULL::interval] AS v", "SELECT ARRAY[['1 month','1 day'],['24 hours',NULL]]::interval[] AS v", "SELECT ARRAY[]::interval[] AS v", "SELECT NULL::interval[] AS v", "SELECT CAST('{1 month,NULL}' AS interval[]) AS v", "SELECT ('[0:1]={1 month,NULL}'::interval[])::interval[] AS v", "SELECT NULL::interval[] AS v UNION ALL SELECT '{1 month}'::interval[] AS v"]:
                    observed=[query(conn,sql) for conn in connections]
                    records.write(json.dumps(dict(phase='SQL constructor/cast',sql=sql,observed=observed))+'\n');records.flush()
                    assert all(v==observed[0] for v in observed), (sql,observed)
                    assert observed[0]['metadata']==[[1187,-1,-1,0]]
                    sql_cases+=1
                summary['sql_cases']=sql_cases
                # Scalar prerequisite is measured on this exact 18.6 reference and native server.
                scalar_cases=0
                style_cases=0
                for style in ['postgres','postgres_verbose','sql_standard','iso_8601']:
                    for conn in connections:
                        query(conn, f"SET IntervalStyle = '{style}'")
                        assert observe(prepare(conn,b'ivscalar',b'SELECT $1::interval AS v',1,(c.c_uint*1)(1186)))['status']==1
                    for token,parts in times[:8]+[(f'{m:+d} months {d:+d} days {t:+d} microseconds'.encode(),(m,d,t)) for m in [-14,0,14] for d in [-2,0,2] for t in [-7384000000,0,7384000000]]:
                        # Canonical postgres text is explicitly signed; render per style with the reference scalar.
                        ref=query(connections[0], "SELECT '"+token.decode()+"'::interval AS v")
                        displayed=bytes.fromhex(ref['rows'][0][0])
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                data=scalar(parts) if infmt else displayed
                                observed=[bind(conn,b'ivscalar',data,infmt,outfmt) for conn in connections]
                                expected=dict(status=2,state='N/A',metadata=[[1186,16,-1,outfmt]],rows=[[(scalar(parts) if outfmt else displayed).hex()]])
                                entry=dict(phase='18.6 scalar prerequisite/style',style=style,parameter_oid=1186,input=data.hex(),input_format=infmt,output_format=outfmt,expected=expected,observed=observed)
                                records.write(json.dumps(entry)+'\n');records.flush()
                                assert all(v==expected for v in observed),entry
                                scalar_cases+=1
                                data=array([2],[-2],[scalar(parts),None]) if infmt else b'[-2:-1]={'+quote(displayed)+b',NULL}'
                                outgoing=array([2],[-2],[scalar(parts),None]) if outfmt else b'[-2:-1]={'+quote(displayed)+b',NULL}'
                                observed=[bind(conn,b'declared',data,infmt,outfmt) for conn in connections]
                                expected=dict(status=2,state='N/A',metadata=[[1187,-1,-1,outfmt]],rows=[[outgoing.hex()]])
                                entry=dict(phase='array IntervalStyle/four directions',style=style,parameter_oid=1187,input=data.hex(),input_format=infmt,output_format=outfmt,expected=expected,observed=observed)
                                records.write(json.dumps(entry)+'\n');records.flush()
                                assert all(v==expected for v in observed),entry
                                style_cases+=1
                    for conn in connections:
                        assert observe(close_prepared(conn,b'ivscalar'))['status']==1
                for conn in connections: query(conn,"SET IntervalStyle = 'postgres'")
                summary.update(scalar_cases=scalar_cases,style_cases=style_cases)
                precision_cases=0
                modifiers=[(f'interval({p})',32767,p) for p in range(7)] + [(f'interval {field}',mask,-1) for field,mask in [('year',4),('month',2),('year to month',6),('day',8),('hour',1024),('minute',2048),('second',4096),('day to hour',1032),('day to minute',3080),('day to second',7176),('hour to minute',3072),('hour to second',7168),('minute to second',6144)]] + [('interval second(3)',4096,3)]
                for ordinal,(target,fields,precision) in enumerate(modifiers):
                    name=f'm{ordinal}'.encode()
                    typmod=(fields<<16)|(65535 if precision<0 else precision)
                    for conn in connections:
                        assert observe(prepare(conn,name,f'SELECT $1::{target}[] AS v'.encode(),1,(c.c_uint*1)(1187)))['status']==1
                        described=describe(conn,name)
                        try:
                            observed=dict(parameter_oids=[paramtype(described,n) for n in range(nparams(described))],metadata=[[ftype(described,n),fsize(described,n),fmod(described,n),fformat(described,n)] for n in range(cols(described))])
                            records.write(json.dumps(dict(phase='interval fields/p statement Describe',target=target,observed=observed))+'\n')
                            assert observed==dict(parameter_oids=[1187],metadata=[[1187,-1,typmod,0]]),observed
                        finally: clear(described)
                    for token,parts in [(b'1 year 2 mons 1 day 02:03:04.123456',(14,1,7384123456)),(b'-1 year -2 mons -1 days -02:03:04.123456',(-14,-1,-7384123456)),(b'00:00:00.5',(0,0,500000)),(b'-00:00:00.5',(0,0,-500000)),(b'23:59:59.999999',(0,0,86399999999))]:
                        months,days,micros=parts
                        if fields==4: months=int(months/12)*12; days=0; micros=0
                        elif fields in [2,6]: days=0; micros=0
                        elif fields==8: micros=0
                        else:
                            unit=3600000000 if fields in [1024,1032] else 60000000 if fields in [2048,3072,3080] else 1
                            micros=(1 if micros>=0 else -1)*(abs(micros)//unit)*unit
                        if 0<=precision<6:
                            scale=10**(6-precision)
                            micros=(1 if micros>=0 else -1)*((abs(micros)+scale//2)//scale)*scale
                        canonical=query(connections[0],"SELECT '"+token.decode()+"'::"+target+" AS v")
                        displayed=bytes.fromhex(canonical['rows'][0][0])
                        for text,binary,expected_text,expected_binary in [(b'[-2:-1]={'+quote(token)+b',NULL}',array([2],[-2],[scalar(parts),None]),b'[-2:-1]={'+quote(displayed)+b',NULL}',array([2],[-2],[scalar((months,days,micros)),None])),(None,None,None,None),(b'{NULL,NULL}',array([2],[1],[None,None]),b'{NULL,NULL}',array([2],[1],[None,None])),(b'{}',array([],[],[]),b'{}',array([],[],[]))]+[(b'{'+quote(token)+b'}',array([1],[1],[scalar(parts)]),b'{'+quote(token)+b'}',array([1],[1],[scalar(parts)])) for token,parts in times[6:8]]:
                            for infmt in [0,1]:
                                for outfmt in [0,1]:
                                    data=binary if infmt else text
                                    outgoing=expected_binary if outfmt else expected_text
                                    observed=[bind(conn,name,data,infmt,outfmt) for conn in connections]
                                    expected=dict(status=2,state='N/A',metadata=[[1187,-1,typmod,outfmt]],rows=[[None if outgoing is None else outgoing.hex()]])
                                    entry=dict(phase='interval fields/p rounding/null/infinity',target=target,parameter_oid=1187,input=None if data is None else data.hex(),input_format=infmt,output_format=outfmt,expected=expected,observed=observed)
                                    records.write(json.dumps(entry)+'\n');records.flush()
                                    assert all(v==expected for v in observed),entry
                                    precision_cases+=1
                    for conn in connections: assert observe(close_prepared(conn,name))['status']==1
                summary['precision_cases']=precision_cases

                if not reference_only:
                    print('PG05_ARRAY_MID', flush=True)
                    assert sys.stdin.readline().strip() == 'continue'
                # Classify each rejection using the actual 18.6 reference, retaining
                # full observations before asserting equality. The TCP pass below
                # separately proves Bind phase, discard-until-Sync and no DataRow.
                # Both lexical/container errors and a bad element after a valid
                # element must reject the entire value.
                invalid_text = [b'',b' ',b'{',b'{"1 mon",}',b'{,"1 mon"}',b'{{"1 mon",NULL},{NULL}}',b'[0:2]={"1 mon",NULL}',b'{"t}',b'{t\\}',b'{\xff}',b'{{{{{{{"1 mon"}}}}}}}',b'[2147483647:2147483647]={"1 mon"}']
                invalid_text += [b'{"1 mon",'+bad+b'}' for bad in [b'garbage',b'""',b'"NULL"',b'\xc3',b'"2147483648 months"',b'"2147483647 months 1 month"',b'"2147483648 days"',b'"9223372036854.775808 seconds"',b'now']]
                header = words(1,0,1186,1,1)
                invalid_binary = [words(-1),words(7),words(0,-1),words(0,2),words(0,0,23),words(0,0,9999),
                    words(1,0,1186,-1,1),words(2,0,1186,50000,1,50000,1),words(1,0,1186,134217728,1),
                    words(1,0,1186,1,2147483647),words(2,0,1186,0,1,-1,1),words(2,0,1186,0,1,1,2147483647),
                    words(1,0,1186,134217727,1),words(0,0,1186,0),array([1],[1],[scalar(0)])+b'\0']
                invalid_binary += [header[:n] for n in range(len(header))]
                invalid_binary += [header+words(1)[:n] for n in range(4)]
                invalid_binary += [header+words(n) for n in [-2,-2147483648,1,4,2147483647]]
                invalid_binary += [array([2],[1],[scalar(0),bad]) for bad in [bytes(n) for n in range(16)]+[bytes(17)]]
                diagnostics = []
                mismatches = []
                for infmt, cases in [(0,invalid_text),(1,invalid_binary)]:
                    for outfmt in [0,1]:
                        for data in cases:
                            observed = [bind(conn,b'declared',data,infmt,outfmt) for conn in connections]
                            entry = dict(phase='libpq Bind/Execute rejection (TCP phase below)',input=data.hex(),input_format=infmt,output_format=outfmt,parameter_oid=1187,reference=observed[0])
                            if not reference_only:
                                entry['aster'] = observed[1]
                            records.write(json.dumps(entry)+'\n'); records.flush()
                            assert observed[0]['status'] == 7 and not observed[0]['rows'], entry
                            diagnostics.append((infmt,outfmt,data,observed[0]['state']))
                            if any(result != observed[0] for result in observed[1:]):
                                mismatches.append(entry)
                            recoveries = []
                            for conn in connections:
                                recovered = query(conn,'SELECT 1::int4 AS v')
                                recoveries.append(recovered)
                                assert recovered == dict(status=2,state='N/A',metadata=[[23,4,-1,0]],rows=[['31']]), recovered
                            records.write(json.dumps(dict(phase='libpq Sync/recovery',query='SELECT 1::int4 AS v',observed=recoveries))+'\n')
                            count += 1

                # libpq hides portal RowDescription and the discarded messages.
                # Use the same real endpoints with startup/authentication payloads
                # deliberately excluded from evidence.
                def recv_exact(sock, n):
                    data = b''
                    while len(data) < n:
                        part = sock.recv(n-len(data))
                        assert part, 'unexpected EOF'
                        data += part
                    return data
                def send(sock, tag, body=b''):
                    sock.sendall(tag+struct.pack('!I',len(body)+4)+body)
                def read(sock):
                    tag = recv_exact(sock,1)
                    size = struct.unpack('!I',recv_exact(sock,4))[0]
                    return tag, recv_exact(sock,size-4)
                def until_ready(sock):
                    result=[]
                    while True:
                        msg=read(sock); result.append(msg)
                        if msg[0] == b'Z':
                            assert msg[1] == b'I'
                            return result
                def raw_connect(endpoint):
                    sock=socket.create_connection(('127.0.0.1',endpoint),timeout=10)
                    raw_sockets.append(sock)
                    body=words(196608)+b'user\0root\0database\0'+(b'postgres' if endpoint == port else sys.argv[2].encode())+b'\0\0'
                    sock.sendall(words(len(body)+4)+body)
                    messages=until_ready(sock)
                    assert not any(t==b'E' for t,b in messages)
                    return sock
                def raw_bind(sock, data, infmt, outfmt, statement=b'a', portal=b''):
                    body=portal+b'\0'+statement+b'\0'+struct.pack('!hhh',1,infmt,1)+words(-1 if data is None else len(data))+(data or b'')+struct.pack('!hh',1,outfmt)
                    send(sock,b'B',body)
                def raw_log(phase, messages, **extra):
                    records.write(json.dumps(dict(phase=phase,messages=[[t.decode(),b.hex()] for t,b in messages],**extra))+'\n'); records.flush()
                raw_sockets=[]
                tcp_cases=0
                try:
                    for endpoint in [port]+([] if reference_only else [int(sys.argv[1])]):
                        sock=raw_connect(endpoint)
                        send(sock,b'P',b'a\0SELECT $1 AS v\0'+struct.pack('!hI',1,1187))
                        send(sock,b'D',b'Sa\0'); send(sock,b'S')
                        messages=until_ready(sock)
                        expected_desc=struct.pack('!h',1)+b'v\0'+struct.pack('!IhIhih',0,0,1187,-1,-1,0)
                        assert messages == [(b'1',b''),(b't',struct.pack('!hI',1,1187)),(b'T',expected_desc),(b'Z',b'I')], messages
                        raw_log('TCP Parse/statement Describe',messages,endpoint='reference' if endpoint==port else 'aster')
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                for text,binary in payloads:
                                    data = binary if infmt else text
                                    outgoing = binary if outfmt else text
                                    raw_bind(sock,data,infmt,outfmt)
                                    send(sock,b'D',b'P\0'); send(sock,b'E',b'\0'+words(0)); send(sock,b'S')
                                    messages=until_ready(sock)
                                    expected_desc=expected_desc[:-2]+struct.pack('!h',outfmt)
                                    row=struct.pack('!h',1)+words(-1 if outgoing is None else len(outgoing))+(outgoing or b'')
                                    assert messages == [(b'2',b''),(b'T',expected_desc),(b'D',row),(b'C',b'SELECT 1\0'),(b'Z',b'I')], messages
                                    raw_log('TCP Bind/portal Describe/Execute',messages,input=None if data is None else data.hex(),input_format=infmt,output_format=outfmt,endpoint='reference' if endpoint==port else 'aster')
                                    tcp_cases+=1
                        for inputstyle in ['postgres','sql_standard']:
                            for outputstyle in ['postgres','postgres_verbose','sql_standard','iso_8601']:
                                for literal in [False,True]:
                                    for outfmt in [0,1]:
                                        send(sock,b'Q',f"SET IntervalStyle = '{inputstyle}'".encode()+b'\0')
                                        assert [t for t,b in until_ready(sock) if t!=b'S']==[b'C',b'Z']
                                        statement=b'frozen'
                                        sql=b"SELECT '[-2:-1]={-1 2:03:04,NULL}'::interval[] AS v" if literal else b'SELECT $1::interval[] AS v'
                                        send(sock,b'P',statement+b'\0'+sql+b'\0'+(struct.pack('!h',0) if literal else struct.pack('!hI',1,1187)))
                                        send(sock,b'S');assert until_ready(sock)==[(b'1',b''),(b'Z',b'I')]
                                        if literal:
                                            send(sock,b'B',b'f\0'+statement+b'\0'+struct.pack('!hhhh',0,0,1,outfmt))
                                        else: raw_bind(sock,b'[-2:-1]={-1 2:03:04,NULL}',0,outfmt,statement,b'f')
                                        send(sock,b'H');assert read(sock)==(b'2',b'')
                                        send(sock,b'P',b'setting\0'+f"SET IntervalStyle = '{outputstyle}'".encode()+b'\0'+struct.pack('!h',0))
                                        send(sock,b'B',b'setting\0setting\0'+struct.pack('!hhh',0,0,0))
                                        send(sock,b'E',b'setting\0'+words(0));send(sock,b'H')
                                        assert [read(sock)[0] for _ in range(3)]==[b'1',b'2',b'C']
                                        micros=-7384000000 if inputstyle=='sql_standard' else 7384000000
                                        query(connections[0],f"SET IntervalStyle = '{outputstyle}'")
                                        canonical=query(connections[0],"SELECT '"+('-1 days -02:03:04' if micros<0 else '-1 days +02:03:04')+"'::interval AS v")
                                        text=b'[-2:-1]={'+quote(bytes.fromhex(canonical['rows'][0][0]))+b',NULL}'
                                        outgoing=array([2],[-2],[scalar((0,-1,micros)),None]) if outfmt else text
                                        send(sock,b'D',b'Pf\0');send(sock,b'E',b'f\0'+words(0));send(sock,b'S')
                                        messages=until_ready(sock)
                                        desc=struct.pack('!h',1)+b'v\0'+struct.pack('!IhIhih',0,0,1187,-1,-1,outfmt)
                                        row=struct.pack('!h',1)+words(len(outgoing))+outgoing
                                        raw_log('TCP Parse/Bind freeze and Execute IntervalStyle',messages,input_style=inputstyle,output_style=outputstyle,literal=literal,output_format=outfmt,expected_components=[0,-1,micros])
                                        assert all(b==b'IntervalStyle\0'+outputstyle.encode()+b'\0' for t,b in messages if t==b'S'),messages
                                        assert [(t,b) for t,b in messages if t!=b'S']==[(b'T',desc),(b'D',row),(b'C',b'SELECT 1\0'),(b'Z',b'I')],messages
                                        send(sock,b'C',b'S'+statement+b'\0');send(sock,b'C',b'Ssetting\0');send(sock,b'S');assert until_ready(sock)==[(b'3',b''),(b'3',b''),(b'Z',b'I')]
                                        tcp_cases+=1
                        send(sock,b'Q',b"SET IntervalStyle = 'postgres'\0");until_ready(sock)
                        query(connections[0],"SET IntervalStyle = 'postgres'")
                        for precision in range(7):
                            statement=f'p{precision}'.encode()
                            send(sock,b'P',statement+b'\0'+f'SELECT $1::interval({precision})[] AS v'.encode()+b'\0'+struct.pack('!hI',1,1187))
                            send(sock,b'D',b'S'+statement+b'\0');send(sock,b'S')
                            description=struct.pack('!h',1)+b'v\0'+struct.pack('!IhIhih',0,0,1187,-1,(32767<<16)|precision,0)
                            messages=until_ready(sock)
                            assert messages==[(b'1',b''),(b't',struct.pack('!hI',1,1187)),(b'T',description),(b'Z',b'I')],messages
                            raw_log('TCP interval(p) statement Describe',messages,precision=precision)
                            for text,binary in [(None,None),(b'{NULL}',array([1],[1],[None])),(b'{}',array([],[],[])),(b'{24:00:00}',array([1],[1],[scalar(86400000000)]))]:
                                for infmt in [0,1]:
                                    for outfmt in [0,1]:
                                        data=binary if infmt else text
                                        outgoing=binary if outfmt else text
                                        raw_bind(sock,data,infmt,outfmt,statement)
                                        send(sock,b'D',b'P\0');send(sock,b'E',b'\0'+words(0));send(sock,b'S')
                                        messages=until_ready(sock)
                                        description=description[:-2]+struct.pack('!h',outfmt)
                                        row=struct.pack('!h',1)+words(-1 if outgoing is None else len(outgoing))+(outgoing or b'')
                                        assert messages==[(b'2',b''),(b'T',description),(b'D',row),(b'C',b'SELECT 1\0'),(b'Z',b'I')],messages
                                        raw_log('TCP interval(p) portal Describe/typed NULL/empty/day carry',messages,precision=precision,input=None if data is None else data.hex(),input_format=infmt,output_format=outfmt)
                                        tcp_cases+=1
                            send(sock,b'C',b'S'+statement+b'\0');send(sock,b'S')
                            assert until_ready(sock)==[(b'3',b''),(b'Z',b'I')]
                        send(sock,b'P',b'overflow\0SELECT $1::interval(0)[] AS v\0'+struct.pack('!hI',1,1187));send(sock,b'S')
                        assert until_ready(sock)==[(b'1',b''),(b'Z',b'I')]
                        for infmt in [0,1]:
                            for micros in [-9223372036854775808,9223372036854775807]:
                                for outfmt in [0,1]:
                                    text=(b'9223372036854.775807 seconds' if micros>0 else b'-9223372036854.775808 seconds')
                                    data=array([2],[1],[scalar(0),scalar(micros)]) if infmt else b'{00:00:00,'+quote(text)+b'}'
                                    raw_bind(sock,data,infmt,outfmt,b'overflow');send(sock,b'S')
                                    messages=until_ready(sock)
                                    raw_log('TCP interval array typmod overflow at Bind',messages,input=data.hex(),input_format=infmt,output_format=outfmt)
                                    assert [t for t,b in messages]==[b'E',b'Z'],messages
                                    fields=dict((part[:1],part[1:]) for part in messages[0][1].split(b'\0') if part)
                                    assert fields[b'C']==b'22008',messages
                                    send(sock,b'Q',b'SELECT 1::int4 AS v\0');assert [t for t,b in until_ready(sock)]==[b'T',b'D',b'C',b'Z']
                                    tcp_cases+=1
                        send(sock,b'C',b'Soverflow\0');send(sock,b'S');assert until_ready(sock)==[(b'3',b''),(b'Z',b'I')]
                        # NUL text is a raw Bind test because libpq text lengths stop at NUL.
                        for infmt,outfmt,data,state in diagnostics+[(0,f,b'{00:00:00,\0}', '22021') for f in [0,1]]:

                            raw_bind(sock,data,infmt,outfmt)
                            # An otherwise valid write must be discarded after Bind error.
                            send(sock,b'P',b'should_skip\0CREATE TABLE array_error_side_effect (id integer)\0'+struct.pack('!h',0))
                            send(sock,b'B',b'\0should_skip\0'+struct.pack('!hhh',0,0,0))
                            send(sock,b'E',b'\0'+words(0)); send(sock,b'S')
                            messages=until_ready(sock)
                            raw_log('TCP Bind/reject/discard/Sync',messages,input=data.hex(),input_format=infmt,output_format=outfmt,endpoint='reference' if endpoint==port else 'aster')
                            assert [t for t,b in messages] == [b'E',b'Z'], messages
                            fields=dict((part[:1],part[1:]) for part in messages[0][1].split(b'\0') if part)
                            if fields[b'C'].decode()!=state:
                                mismatches.append(dict(phase='TCP Bind SQLSTATE',input=data.hex(),expected=state,actual=fields[b'C'].decode()))
                            send(sock,b'Q',b'SELECT 1::int4 AS v\0')
                            recovered=until_ready(sock)
                            assert [t for t,b in recovered]==[b'T',b'D',b'C',b'Z'], recovered
                            assert recovered[1] == (b'D',struct.pack('!h',1)+words(1)+b'1')
                            raw_log('TCP Sync/recovery SELECT 1::int4',recovered)
                            # The discarded CREATE must leave no catalog object.
                            send(sock,b'Q',b'SELECT * FROM array_error_side_effect\0')
                            absent=until_ready(sock)
                            assert [t for t,b in absent]==[b'E',b'Z'], absent
                            absent_fields=dict((part[:1],part[1:]) for part in absent[0][1].split(b'\0') if part)
                            assert absent_fields[b'C']==b'42P01', absent
                            raw_log('TCP no-side-effect/catalog-absence',absent)
                            tcp_cases+=1
                        send(sock,b'C',b'Sa\0'); send(sock,b'S')
                        assert until_ready(sock)==[(b'3',b''),(b'Z',b'I')]
                        send(sock,b'X')
                finally:
                    for sock in raw_sockets:
                        sock.close()
                    summary.update(tcp_cases=tcp_cases,raw_clients_closed=True,mismatches=mismatches)
                assert not mismatches, mismatches[:3]
                for conn in connections:
                    for name, _, _ in statements:
                        result = observe(close_prepared(conn, name))
                        assert result['status'] == 1, result
            summary.update(cases=count, reference_matrix_passed=True)
        finally:
            for conn in connections:
                finish(conn)
            summary['clients_closed'] = True
            if started:
                subprocess.run([str(binpath/'pg_ctl'), '-D', root+'/data', '-m', 'immediate', '-w', 'stop'], check=True, stdout=subprocess.DEVNULL)
                with socket.socket() as sock:
                    summary['reference_port_closed'] = sock.connect_ex(('127.0.0.1',port)) != 0
                assert summary['reference_port_closed']
finally:
    summary.update(cases=count, reference_directory_removed='root' in globals() and not Path(root).exists())
    (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
summary['passed'] = not reference_only
(evidence/'summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary), flush=True)
