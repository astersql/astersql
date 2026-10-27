# Copyright 2026 AsterSQL.
"""JSONB libpq differential; owns only its temporary PostgreSQL reference cluster."""
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
    with tempfile.TemporaryDirectory(prefix='pg05-jsonb-reference-') as root:
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
                ('server_encoding', 'SHOW server_encoding'), ('client_encoding', 'SHOW client_encoding'),
                ('standard_conforming_strings', 'SHOW standard_conforming_strings'),
                ('DateStyle', 'SHOW DateStyle'), ('TimeZone', 'SHOW TimeZone')]}
            actual_version = int(bytes.fromhex(identity['server_version_num']['rows'][0][0]))
            summary.update(reference=identity, reference_version_num=actual_version,
                           reference_build=subprocess.check_output([str(binpath/'pg_config'), '--configure'], text=True).strip())
            (evidence/'identity.json').write_text(json.dumps(summary, indent=2))
            print(f'reference_version_num={actual_version}', flush=True)
            assert actual_version == 180006, 'exact PostgreSQL 18.6 required'
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

                parameter_oids = {}
                statements = [
                    (b'declared', b'SELECT $1 AS v', [3802]),
                    (b'inferred', b'SELECT $1::jsonb AS v', []),
                    (b'zero', b'SELECT $1::jsonb AS v', [0]),
                    (b'empty', b'SELECT $1::jsonb AS v WHERE false', [3802]),
                    (b'first_null', b'SELECT NULL::jsonb AS v UNION ALL SELECT $1::jsonb AS v', [3802]),
                    (b'text_to', b'SELECT $1::jsonb AS v', [25]),
                    (b'json_to', b'SELECT $1::jsonb AS v', [114]),
                    (b'to_json', b'SELECT $1::json AS v', [3802]),
                    (b'to_text', b'SELECT $1::text AS v', [3802]),
                    (b'original_json', b'SELECT $1 AS v', [114])]
                for name, sql, oids in statements:
                    parameter_oids[name] = oids[0] if oids and oids[0] else 3802
                    result_oid = {b'to_json':114,b'to_text':25,b'original_json':114}.get(name,3802)
                    for index, conn in enumerate(connections):
                        result = observe(prepare(conn, name, sql, len(oids), (c.c_uint*len(oids))(*oids)))
                        assert result == dict(status=1, state='N/A', metadata=[], rows=[]), result
                        result = describe(conn, name)
                        try:
                            observed = dict(status=status(result), parameter_oids=[paramtype(result,n) for n in range(nparams(result))],
                                            metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))])
                            records.write(json.dumps(dict(phase='Parse/statement Describe', endpoint='reference' if index == 0 else 'aster', statement=name.decode(), observed=observed))+'\n')
                            assert observed == dict(status=1, parameter_oids=[parameter_oids[name]], metadata=[[result_oid,-1,-1,0]]), observed
                        finally:
                            clear(result)
                payloads = [
                    (b'null', b'null'), (b'"null"', b'"null"'),
                    (b'true', b'true'), (b'false', b'false'),
                    (b'0', b'0'), (b'-0.00', b'0.00'),
                    (b'1.00', b'1.00'), (b'1.230e-5', b'0.00001230'),
                    (b'18446744073709551617', b'18446744073709551617'),
                    (b'12345678901234567890.12345678901234567890', b'12345678901234567890.12345678901234567890'),
                    (b'{}', b'{}'), (b'[]', b'[]'), (b'[2,1,2]', b'[2, 1, 2]'),
                    (b' { "aa":1, "b":2, "aa":3 } ', b'{"b": 2, "aa": 3}'),
                    (b'{"a":1,"\\u0061":2,"o":{"z":1,"z":3}}', b'{"a": 2, "o": {"z": 3}}'),
                    (b'[null,{"x":[true,"\\u4e2d",1]}]', '[null, {"x": [true, "中", 1]}]'.encode()),
                    (b'"\\ud83d\\ude00"', '"😀"'.encode()),
                    (b'"\\u4e2d"', '"中"'.encode()), ('"中文😀"'.encode(), '"中文😀"'.encode()),
                    (b'1e131071', b'1'+b'0'*131071),
                    (b'1e-16383', b'0.'+b'0'*16382+b'1'),
                    (b'['*256+b'null'+b']'*256, b'['*256+b'null'+b']'*256),
                    (b'{"b":2,"aa":3}', b'{"b": 2, "aa": 3}'),
                    (b'1', b'1'), (b'"x"', b'"x"')]
                def wire(data, fmt):
                    return None if data is None else (b'\x01' if fmt else b'') + data
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        def expected(data, result_rows=None):
                            if result_rows is None:
                                result_rows = [[None if data is None else wire(data,outfmt).hex()]]
                            return dict(status=2, state='N/A', metadata=[[3802,-1,-1,outfmt]], rows=result_rows)
                        for data, canonical in payloads + [(None,None)]:
                            record('Bind/Execute/rebind', b'declared', wire(data,infmt), infmt, outfmt, expected(canonical))
                        for name in [b'inferred', b'zero', b'empty', b'first_null']:
                            for data, canonical in [payloads[13], (None,None)]:
                                result_rows = [] if name == b'empty' else ([[None]] if name == b'first_null' else []) + [[None if canonical is None else wire(canonical,outfmt).hex()]]
                                record('Bind/Execute/null-shape', name, wire(data,infmt), infmt, outfmt, expected(canonical, result_rows))
                # Explicit bridges preserve their target identity and revalidate
                # JSON text rather than reusing its looser Unicode/numeric domain.
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        for name in [b'text_to',b'json_to',b'to_json',b'to_text',b'original_json']:
                            oid = {b'to_json':114,b'to_text':25,b'original_json':114}.get(name,3802)
                            for data, canonical in [payloads[13],(None,None)]:
                                result_data = data if name == b'original_json' else canonical
                                incoming = wire(data,infmt) if parameter_oids[name] == 3802 else data
                                outgoing = wire(result_data,outfmt) if oid == 3802 else result_data
                                record('Bind/Execute/explicit-bridge',name,incoming,infmt,outfmt,
                                       dict(status=2,state='N/A',metadata=[[oid,-1,-1,outfmt]],
                                            rows=[[None if outgoing is None else outgoing.hex()]]))
                        for name in [b'text_to',b'json_to']:
                            for data,state in [(b'"\\u0000"','22P05'),(b'"\\ud800"','22P02'),(b'1e200000','22003')]:
                                record('Execute/cast-reject/no-DataRow',name,data,infmt,outfmt,
                                       dict(status=7,state=state,metadata=[],rows=[]))
                                record('Sync/cast-recovery',name,b'null',infmt,outfmt,
                                       dict(status=2,state='N/A',metadata=[[3802,-1,-1,outfmt]],rows=[[wire(b'null',outfmt).hex()]]))
                if not reference_only:
                    print('PG05_JSONB_MID', flush=True)
                    assert sys.stdin.readline().strip() == 'continue'
                invalid = [(x, '22P02') for x in [b'', b' ', b'bad', b'NULL', b'NaN', b'Infinity',
                    b'01', b'+1', b'1.', b'1e', b'[1,]', b'{"a":}', b'{"a":1,}',
                    b'"unterminated', b'"\\x00"', b'"\\u12"', b'"\\uZZZZ"', b'{} trailing']]
                invalid += [(b'"\\u0000"', '22P05')]
                invalid += [(x, '22P02') for x in [b'"\\ud800"', b'"\\udc00"', b'"\\ud800x"']]
                invalid += [(b'1e200000','22003'), (b'1e-200000','22003'), (b'1e131072','22003'), (b'1e-16384','22003'), (b'\xff','22021'), (b'\xc3(','22021')]
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        cases = [(wire(data,infmt),state) for data,state in invalid]
                        if infmt:
                            cases += [(b'', '08P01'), (b'\x00{}','XX000'), (b'\x02{}','XX000'), (b'\xff{}','XX000'),
                                      (b'\x01{}\x00','22021'), (b'\x01"a\x00b"','22021')]
                        for data, state in cases:
                            record('Bind/reject/no-DataRow', b'declared', data, infmt, outfmt,
                                   dict(status=7, state=state, metadata=[], rows=[]))
                            record('Sync/recovery', b'declared', wire(b' {"recovered":true} ',infmt), infmt, outfmt,
                                   dict(status=2, state='N/A', metadata=[[3802,-1,-1,outfmt]], rows=[[wire(b'{"recovered": true}',outfmt).hex()]]))
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
