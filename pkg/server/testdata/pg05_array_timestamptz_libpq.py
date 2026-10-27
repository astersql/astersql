# Copyright 2026 AsterSQL.
"""timestamptz array libpq differential; owns only its temporary PostgreSQL reference cluster."""
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
    raw = execute(conn, sql.encode())
    diagnostic = (ef(raw,ord('M')) or b'').decode(errors='replace') if status(raw) == 7 else ''
    result = observe(raw)
    assert result['status'] in [1, 2], (sql, result, diagnostic)
    return result

def bind(conn, name, data, infmt, outfmt):
    # c_char_p preserves the pointer, including NULs inside binary payloads;
    # explicit lengths are authoritative only in binary format.
    return observe(prepared(conn, name, 1, (s*1)(data),
                            (i*1)(len(data) if data is not None else 0), (i*1)(infmt), outfmt))

def set_zone(conn, zone):
    # Existing scalar contract: native settings use the engine's time_zone name.
    return query(conn, f"SET TimeZone TO '{zone}'" if conn == connections[0] else f"SET time_zone = '{zone}'")

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
    with tempfile.TemporaryDirectory(prefix='pg05-tz-array-', dir='/tmp') as root:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        started = False
        try:
            subprocess.run([str(binpath/'initdb'), '-D', root+'/data', '-U', 'root', '-A', 'trust', '--encoding=UTF8', '--no-locale'], check=True, stdout=subprocess.DEVNULL)
            subprocess.run([str(binpath/'pg_ctl'), '-D', root+'/data', '-l', root+'/log', '-o', f'-h 127.0.0.1 -p {port} -k {root}', '-w', 'start'], check=True, stdout=subprocess.DEVNULL)
            started = True
            endpoints = [f'port={port} dbname=postgres']
            if not reference_only:
                endpoints.append(f'port={int(sys.argv[1])} dbname={sys.argv[2]}')
            for endpoint in endpoints:
                conn = connect(f'host=127.0.0.1 user=root connect_timeout=10 {endpoint}'.encode())
                assert conn
                connections.append(conn)
                assert connected(conn) == 0, 'client connection failed'
            for conn in connections:
                set_zone(conn, 'UTC')
            identity = {key: query(connections[0], sql) for key, sql in [
                ('version', 'SELECT version()'), ('server_version_num', 'SHOW server_version_num'),
                ('catalog_types', 'SELECT oid,typelem,typlen FROM pg_type WHERE oid IN (1184,1185) ORDER BY oid'),
                ('server_encoding', 'SHOW server_encoding'), ('client_encoding', 'SHOW client_encoding'),
                ('standard_conforming_strings', 'SHOW standard_conforming_strings'),
                ('array_nulls', 'SHOW array_nulls'),
                ('DateStyle', 'SHOW DateStyle'), ('TimeZone', 'SHOW TimeZone')]}
            actual_version = int(bytes.fromhex(identity['server_version_num']['rows'][0][0]))
            summary.update(reference=identity, reference_version_num=actual_version,
                           reference_build=subprocess.check_output([str(binpath/'pg_config'), '--configure'], text=True).strip())
            (evidence/'identity.json').write_text(json.dumps(summary, indent=2))
            print(f'reference_version_num={actual_version}', flush=True)
            assert actual_version == 180006, 'exact PostgreSQL 18.6 required'
            assert identity['catalog_types']['rows'] == [[b'1184'.hex(), b'0'.hex(), b'8'.hex()], [b'1185'.hex(), b'1184'.hex(), b'-1'.hex()]]
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
                    return words(len(shape), int(any(v is None for v in values)) if flags is None else flags, 1184) + b''.join(words(n,l) for n,l in zip(shape,lower)) + b''.join(words(-1) if v is None else words(len(v))+v for v in values)
                parameter_oids = {}
                statements = [
                    (b'declared', b'SELECT $1 AS v', [1185]),
                    (b'inferred', b'SELECT $1::timestamptz[] AS v', []),
                    (b'zero', b'SELECT $1::timestamptz[] AS v', [0]),
                    (b'empty', b'SELECT $1::timestamptz[] AS v WHERE false', [1185]),
                    (b'first_null', b'SELECT NULL::timestamptz[] AS v UNION ALL SELECT $1::timestamptz[] AS v', [1185])]
                for name, sql, oids in statements:
                    parameter_oids[name] = 1185
                    for index, conn in enumerate(connections):
                        result = observe(prepare(conn, name, sql, len(oids), (c.c_uint*len(oids))(*oids)))
                        assert result == dict(status=1, state='N/A', metadata=[], rows=[]), result
                        result = describe(conn, name)
                        try:
                            observed = dict(status=status(result), parameter_oids=[paramtype(result,n) for n in range(nparams(result))], metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))])
                            records.write(json.dumps(dict(phase='Parse/statement Describe', endpoint='reference' if index == 0 else 'aster', statement=name.decode(), observed=observed))+'\n')
                            assert observed == dict(status=1, parameter_oids=[1185], metadata=[[1185,-1,-1,0]]), observed
                        finally:
                            clear(result)
                times = [(b'2000-01-01 00:00:00+00',0),(b'2000-01-02 00:00:00+00',86400000000),(b'2000-01-01 12:34:56.123456+00',45296123456),(b'2000-01-01 23:59:59.999999+00',86399999999),(b'2000-01-01 00:00:00.000001+00',1),(b'1999-12-31 23:59:59.999999+00',-1),(b'infinity',9223372036854775807),(b'-infinity',-9223372036854775808),(b'0001-01-01 00:00:00+00 BC',-63113904000000000),(b'12000-01-01 00:00:00+00',315569520000000000),(b'4714-11-24 00:00:00+00 BC',-211813488000000000),(b'294276-12-31 23:59:59.999999+00',9223371331199999999)]
                def scalar(n):
                    return struct.pack('!q',n)
                def quote(text):
                    return b'"'+text+b'"' if b' ' in text else text
                payloads = [
                    (b'{'+quote(text)+b',NULL}',array([2],[1],[scalar(day),None])) for text,day in times]
                payloads += [
                    (b'{{"2000-01-01 00:00:00+00",NULL},{"2000-01-01 00:00:03+00","2000-01-01 00:00:02+00"}}',array([2,2],[1,1],[scalar(0),None,scalar(3000000),scalar(2000000)])),
                    (b'[0:1][-2:-1]={{"2000-01-01 00:00:00+00",NULL},{"2000-01-01 00:00:03+00","2000-01-01 00:00:02+00"}}',array([2,2],[0,-2],[scalar(0),None,scalar(3000000),scalar(2000000)])),
                    (b'[-3:-2]={"2000-01-01 00:00:01+00","2000-01-01 00:00:01+00"}',array([2],[-3],[scalar(1000000),scalar(1000000)])),
                    (b'{}',array([],[],[])),(b'{NULL}',array([1],[1],[None])),(b'{NULL,NULL}',array([2],[1],[None,None])),
                    (b'{{{{{{"2000-01-01 00:00:00+00"}}}}}}',array([1]*6,[1]*6,[scalar(0)])),(None,None)]
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        for name, _, _ in statements:
                            for text, binary in payloads:
                                data = binary if infmt else text
                                outgoing = binary if outfmt else text
                                result_rows = [] if name == b'empty' else ([[None]] if name == b'first_null' else []) + [[None if outgoing is None else outgoing.hex()]]
                                record('Bind/Execute/rebind/shape-null', name, data, infmt, outfmt,
                                       dict(status=2, state='N/A', metadata=[[1185,-1,-1,outfmt]], rows=result_rows))
                        # Input flags are not a promise about element NULLs; output is canonical.
                        if infmt:
                            for data, canonical in [(array([1],[1],[None],0),(b'{NULL}',array([1],[1],[None]))), (array([1],[1],[scalar(0)],1),(b'{"2000-01-01 00:00:00+00"}',array([1],[1],[scalar(0)]))), (words(1,0,1184,0,-7),(b'{}',array([],[],[])))]:
                                outgoing = canonical[outfmt]
                                record('Bind/Execute/noncanonical-header', b'declared',data,infmt,outfmt,
                                       dict(status=2,state='N/A',metadata=[[1185,-1,-1,outfmt]],rows=[[outgoing.hex()]]))
                # Canonical explicit UTC input retains the same instant.
                # Also cover 24:00 and excess fractional precision canonicalization.
                for token,canonical,micros in [
                    (b'2000-01-01 00:00:00+00',b'2000-01-01 00:00:00+00',0),
                    (b'2000-01-01 24:00:00+00',b'2000-01-02 00:00:00+00',86400000000),
                    (b'2000-01-01 00:00:00.0000004+00',b'2000-01-01 00:00:00+00',0)]:
                    binary=array([2],[-2],[scalar(micros),None])
                    for infmt in [0,1]:
                        for outfmt in [0,1]:
                            data=binary if infmt else b'[-2:-1]={'+quote(token)+b',NULL}'
                            outgoing=binary if outfmt else b'[-2:-1]={'+quote(canonical)+b',NULL}'
                            record('Bind/Execute/wall-clock-canonicalization',b'declared',data,infmt,outfmt,
                                   dict(status=2,state='N/A',metadata=[[1185,-1,-1,outfmt]],rows=[[outgoing.hex()]]))
                timezone_cases=0
                # Rebind in each session zone. Derive canonical UTC bytes once from the
                # exact reference and demand that binary input never shifts them again.
                for zone in ['UTC','Asia/Shanghai','America/New_York','Australia/Lord_Howe']:
                    for conn in connections:
                        set_zone(conn, zone)
                    for token in [b'2000-01-01 00:00:00+00',b'2000-01-01 08:00:00+08',
                                  b'2024-03-10 02:30:00',b'2024-11-03 01:30:00',
                                  b'2024-10-06 02:15:00',b'2024-04-07 01:45:00']:
                        text=b'[-2:-1]={'+quote(token)+b',NULL}'
                        canonical=bind(connections[0],b'declared',text,0,1)
                        assert canonical['status']==2 and canonical['metadata']==[[1185,-1,-1,1]] and len(canonical['rows'])==1
                        binary=bytes.fromhex(canonical['rows'][0][0])
                        if token in [b'2000-01-01 00:00:00+00',b'2000-01-01 08:00:00+08']:
                            assert binary==array([2],[-2],[scalar(0),None]),canonical
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                data=binary if infmt else text
                                observed=[bind(conn,b'declared',data,infmt,outfmt) for conn in connections]
                                entry=dict(phase='TimeZone/DST/four-directions/no-double-offset',zone=zone,parameter_oid=1185,input=data.hex(),input_format=infmt,output_format=outfmt,observed=observed)
                                records.write(json.dumps(entry)+'\n');records.flush()
                                assert observed[0]['status']==2 and observed[0]['metadata']==[[1185,-1,-1,outfmt]],entry
                                assert all(v==observed[0] for v in observed),entry
                                if outfmt: assert observed[0]['rows']==canonical['rows'],entry
                                timezone_cases+=1
                    for sql in ["SELECT ARRAY['2024-03-10 02:30:00'::timestamptz,NULL::timestamptz] AS v",
                                "SELECT ARRAY[['2024-11-03 01:30:00',NULL]]::timestamptz[] AS v",
                                "SELECT '{2024-10-06 02:15:00,NULL}'::timestamptz[] AS v"]:
                        observed=[query(conn,sql) for conn in connections]
                        entry=dict(phase='TimeZone/DST/SQL constructors',zone=zone,sql=sql,observed=observed)
                        records.write(json.dumps(entry)+'\n');records.flush()
                        assert observed[0]['metadata']==[[1185,-1,-1,0]] and all(v==observed[0] for v in observed),entry
                        timezone_cases+=1
                    for precision in range(7):
                        name=b'tz_precision'
                        for conn in connections:
                            assert observe(prepare(conn,name,f'SELECT $1::timestamptz({precision})[] AS v'.encode(),1,(c.c_uint*1)(1185)))['status']==1
                        text=b'{"1999-12-31 23:59:59.999999",NULL}'
                        canonical=bind(connections[0],b'declared',text,0,1)
                        assert canonical['status']==2
                        binary=bytes.fromhex(canonical['rows'][0][0])
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                data=binary if infmt else text
                                observed=[bind(conn,name,data,infmt,outfmt) for conn in connections]
                                entry=dict(phase='TimeZone/precision/four-directions',zone=zone,precision=precision,parameter_oid=1185,input=data.hex(),input_format=infmt,output_format=outfmt,observed=observed)
                                records.write(json.dumps(entry)+'\n');records.flush()
                                assert observed[0]['status']==2 and observed[0]['metadata']==[[1185,-1,precision,outfmt]] and all(v==observed[0] for v in observed),entry
                                timezone_cases+=1
                        for conn in connections: assert observe(close_prepared(conn,name))['status']==1
                # Input interpretation is frozen at Parse, output uses Execute TimeZone.
                for conn in connections:
                    set_zone(conn,'America/New_York')
                    assert observe(prepare(conn,b'frozen',b"SELECT '{2024-03-10 02:30:00,NULL}'::timestamptz[] AS v",0,None))['status']==1
                    set_zone(conn,'Asia/Shanghai')
                for outfmt in [0,1]:
                    observed=[observe(prepared(conn,b'frozen',0,None,None,None,outfmt)) for conn in connections]
                    entry=dict(phase='Parse input frozen/Execute TimeZone display',output_format=outfmt,observed=observed)
                    records.write(json.dumps(entry)+'\n');records.flush()
                    assert observed[0]['status']==2 and observed[0]['metadata']==[[1185,-1,-1,outfmt]] and all(v==observed[0] for v in observed),entry
                    timezone_cases+=1
                for conn in connections:
                    assert observe(close_prepared(conn,b'frozen'))['status']==1
                    set_zone(conn,'UTC')
                summary['timezone_cases']=timezone_cases
                constructor_cases=0
                for conn in connections:
                    assert observe(prepare(conn,b'timestamptz_constructor',b'SELECT ARRAY[$1::timestamptz,NULL::timestamptz] AS v',1,(c.c_uint*1)(1184)))['status']==1
                for text,day in times:
                    for infmt in [0,1]:
                        for outfmt in [0,1]:
                            data=scalar(day) if infmt else text
                            expected=(array([2],[1],[scalar(day),None]) if outfmt else b'{'+quote(text)+b',NULL}').hex()
                            observed=[bind(conn,b'timestamptz_constructor',data,infmt,outfmt) for conn in connections]
                            entry=dict(phase='scalar parameter/ARRAY constructor',parameter_oid=1184,input=data.hex(),input_format=infmt,output_format=outfmt,observed=observed)
                            records.write(json.dumps(entry)+'\n');records.flush()
                            assert all(v==dict(status=2,state='N/A',metadata=[[1185,-1,-1,outfmt]],rows=[[expected]]) for v in observed),entry
                            constructor_cases+=1
                for conn in connections:
                    assert observe(close_prepared(conn,b'timestamptz_constructor'))['status']==1
                summary['constructor_cases']=constructor_cases
                sql_cases=0
                for sql in ["SELECT ARRAY[(\'2000-01-02 00:00:00+00\'::timestamptz),(\'2000-01-01 00:00:00.000001+00\'::timestamptz)] AS v", "SELECT ARRAY[(\'2000-01-02 00:00:00+00\'::timestamptz)::timestamptz,NULL::timestamptz] AS v", "SELECT ARRAY['2000-01-02 00:00:00+00'::timestamptz,'2000-01-01 00:00:00.000001+00'::timestamptz,NULL::timestamptz] AS v", "SELECT ARRAY[['2000-01-01 00:00:00+00','2000-01-01 00:00:03+00'],['2000-01-01 00:00:02+00',NULL]]::timestamptz[] AS v", "SELECT ARRAY[]::timestamptz[] AS v", "SELECT NULL::timestamptz[] AS v", "SELECT CAST('{2000-01-01 00:00:00+00,NULL}' AS timestamptz[]) AS v", "SELECT ('[0:1]={2000-01-01 00:00:00+00,NULL}'::timestamptz[])::timestamptz[] AS v"]:
                    observed=[query(conn,sql) for conn in connections]
                    records.write(json.dumps(dict(phase='SQL constructor/cast',sql=sql,observed=observed))+'\n');records.flush()
                    assert all(v==observed[0] for v in observed), (sql,observed)
                    assert observed[0]['metadata']==[[1185,-1,-1,0]]
                    sql_cases+=1
                summary['sql_cases']=sql_cases
                precision_cases=0
                for precision in range(7):
                    name=f'p{precision}'.encode()
                    sql=f'SELECT $1::timestamptz({precision})[] AS v'.encode()
                    for conn in connections:
                        assert observe(prepare(conn,name,sql,1,(c.c_uint*1)(1185)))['status']==1
                        described=describe(conn,name)
                        try:
                            observed=dict(parameter_oids=[paramtype(described,n) for n in range(nparams(described))],metadata=[[ftype(described,n),fsize(described,n),fmod(described,n),fformat(described,n)] for n in range(cols(described))])
                            records.write(json.dumps(dict(phase='timestamptz(p) statement Describe',precision=precision,observed=observed))+'\n')
                            assert observed==dict(parameter_oids=[1185],metadata=[[1185,-1,precision,0]]),observed
                        finally:
                            clear(described)
                    for token,micros in [(b'2000-01-01 23:59:59.999999+00',86399999999),(b'2000-01-01 12:34:56.123456+00',45296123456),(b'2000-01-01 00:00:00.999999+00',999999),(b'1999-12-31 23:59:59.999999+00',-1),(b'1999-12-31 23:59:59.5+00',-500000),(b'2000-01-01 00:00:00.5+00',500000)]:
                        scale=10**(6-precision)
                        rounded=(1 if micros>=0 else -1)*((abs(micros)+scale//2)//scale)*scale if precision<6 else micros
                        import datetime
                        dt=datetime.datetime(2000,1,1)+datetime.timedelta(microseconds=rounded)
                        canonical=dt.strftime('%Y-%m-%d %H:%M:%S').encode()+(('.'+f'{dt.microsecond:06}'.rstrip('0')).encode() if dt.microsecond else b'')+b'+00'
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                data=array([2],[-2],[scalar(micros),None]) if infmt else b'[-2:-1]={'+quote(token)+b',NULL}'
                                outgoing=array([2],[-2],[scalar(rounded),None]) if outfmt else b'[-2:-1]={'+quote(canonical)+b',NULL}'
                                observed=[bind(conn,name,data,infmt,outfmt) for conn in connections]
                                expected=dict(status=2,state='N/A',metadata=[[1185,-1,precision,outfmt]],rows=[[outgoing.hex()]])
                                entry=dict(phase='timestamptz(p) array rounding/carry',precision=precision,parameter_oid=1185,input=data.hex(),input_format=infmt,output_format=outfmt,expected=expected,observed=observed)
                                records.write(json.dumps(entry)+'\n');records.flush()
                                assert all(v==expected for v in observed),entry
                                precision_cases+=1
                    for text,binary in [(None,None),(b'{NULL,NULL}',array([2],[1],[None,None])),(b'{}',array([],[],[])),(b'{infinity}',array([1],[1],[scalar(9223372036854775807)])),(b'{-infinity}',array([1],[1],[scalar(-9223372036854775808)]))]:
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                data=binary if infmt else text
                                outgoing=binary if outfmt else text
                                observed=[bind(conn,name,data,infmt,outfmt) for conn in connections]
                                expected=dict(status=2,state='N/A',metadata=[[1185,-1,precision,outfmt]],rows=[[None if outgoing is None else outgoing.hex()]])
                                entry=dict(phase='timestamptz(p) typed NULL/empty',precision=precision,parameter_oid=1185,input=None if data is None else data.hex(),input_format=infmt,output_format=outfmt,expected=expected,observed=observed)
                                records.write(json.dumps(entry)+'\n');records.flush()
                                assert all(v==expected for v in observed),entry
                                precision_cases+=1
                    for conn in connections:
                        assert observe(close_prepared(conn,name))['status']==1
                summary['precision_cases']=precision_cases

                if not reference_only:
                    print('PG05_ARRAY_MID', flush=True)
                    assert sys.stdin.readline().strip() == 'continue'
                # Classify each rejection using the actual 18.6 reference, retaining
                # full observations before asserting equality. The TCP pass below
                # separately proves Bind phase, discard-until-Sync and no DataRow.
                # Both lexical/container errors and a bad element after a valid
                # element must reject the entire value.
                invalid_text = [b'',b' ',b'{',b'{"2000-01-01 00:00:00+00",}',b'{,"2000-01-01 00:00:00+00"}',b'{{"2000-01-01 00:00:00+00",NULL},{NULL}}',b'[0:2]={"2000-01-01 00:00:00+00",NULL}',b'{"t}',b'{t\\}',b'{\xff}',b'{{{{{{{"2000-01-01 00:00:00+00"}}}}}}}',b'[2147483647:2147483647]={"2000-01-01 00:00:00+00"}']
                invalid_text += [b'{"2000-01-01 00:00:00+00",'+bad+b'}' for bad in [b'garbage',b'""',b'"NULL"',b'\xc3',b'"2000-13-01 00:00:00+00"',b'"2000-02-30 00:00:00+00"',b'"2000-01-01 25:00:00+00"',b'"294277-01-01 00:00:00+00"',b'"4714-11-23 00:00:00+00 BC"']]

                header = words(1,0,1184,1,1)
                invalid_binary = [words(-1),words(7),words(0,-1),words(0,2),words(0,0,23),words(0,0,9999),
                    words(1,0,1184,-1,1),words(2,0,1184,50000,1,50000,1),words(1,0,1184,134217728,1),
                    words(1,0,1184,1,2147483647),words(2,0,1184,0,1,-1,1),words(2,0,1184,0,1,1,2147483647),
                    words(1,0,1184,134217727,1),words(0,0,1184,0),array([1],[1],[scalar(0)])+b'\0']
                invalid_binary += [header[:n] for n in range(len(header))]
                invalid_binary += [header+words(1)[:n] for n in range(4)]
                invalid_binary += [header+words(n) for n in [-2,-2147483648,1,4,2147483647]]
                invalid_binary += [array([2],[1],[scalar(0),bad]) for bad in [bytes(n) for n in range(8)]+[bytes(9),scalar(-211813488000000001),scalar(9223371331200000000)]]
                diagnostics = []
                mismatches = []
                for infmt, cases in [(0,invalid_text),(1,invalid_binary)]:
                    for outfmt in [0,1]:
                        for data in cases:
                            observed = [bind(conn,b'declared',data,infmt,outfmt) for conn in connections]
                            entry = dict(phase='libpq Bind/Execute rejection (TCP phase below)',input=data.hex(),input_format=infmt,output_format=outfmt,parameter_oid=1185,reference=observed[0])
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
                def raw_bind(sock, data, infmt, outfmt, statement=b'a'):
                    body=b'\0'+statement+b'\0'+struct.pack('!hhh',1,infmt,1)+words(-1 if data is None else len(data))+(data or b'')+struct.pack('!hh',1,outfmt)
                    send(sock,b'B',body)
                def raw_log(phase, messages, **extra):
                    records.write(json.dumps(dict(phase=phase,messages=[[t.decode(),b.hex()] for t,b in messages],**extra))+'\n'); records.flush()
                raw_sockets=[]
                tcp_cases=0
                try:
                    for endpoint in [port]+([] if reference_only else [int(sys.argv[1])]):
                        sock=raw_connect(endpoint)
                        send(sock,b'Q',b"SET TimeZone TO 'UTC'\0" if endpoint == port else b"SET time_zone = 'UTC'\0")
                        assert not any(t==b'E' for t,b in until_ready(sock))
                        send(sock,b'P',b'a\0SELECT $1 AS v\0'+struct.pack('!hI',1,1185))
                        send(sock,b'D',b'Sa\0'); send(sock,b'S')
                        messages=until_ready(sock)
                        expected_desc=struct.pack('!h',1)+b'v\0'+struct.pack('!IhIhih',0,0,1185,-1,-1,0)
                        assert messages == [(b'1',b''),(b't',struct.pack('!hI',1,1185)),(b'T',expected_desc),(b'Z',b'I')], messages
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
                        for precision in range(7):
                            statement=f'p{precision}'.encode()
                            send(sock,b'P',statement+b'\0'+f'SELECT $1::timestamptz({precision})[] AS v'.encode()+b'\0'+struct.pack('!hI',1,1185))
                            send(sock,b'D',b'S'+statement+b'\0');send(sock,b'S')
                            description=struct.pack('!h',1)+b'v\0'+struct.pack('!IhIhih',0,0,1185,-1,precision,0)
                            messages=until_ready(sock)
                            assert messages==[(b'1',b''),(b't',struct.pack('!hI',1,1185)),(b'T',description),(b'Z',b'I')],messages
                            raw_log('TCP timestamptz(p) statement Describe',messages,precision=precision)
                            for text,binary in [(None,None),(b'{NULL}',array([1],[1],[None])),(b'{}',array([],[],[])),(b'{"2000-01-02 00:00:00+00"}',array([1],[1],[scalar(86400000000)]))]:
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
                                        raw_log('TCP timestamptz(p) portal Describe/typed NULL/empty/day carry',messages,precision=precision,input=None if data is None else data.hex(),input_format=infmt,output_format=outfmt)
                                        tcp_cases+=1
                            send(sock,b'C',b'S'+statement+b'\0');send(sock,b'S')
                            assert until_ready(sock)==[(b'3',b''),(b'Z',b'I')]
                        for infmt,outfmt,data,state in diagnostics:
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
            if started or Path(root+'/data/postmaster.pid').exists():
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
