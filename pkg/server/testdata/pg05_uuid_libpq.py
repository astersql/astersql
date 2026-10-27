# Copyright 2026 AsterSQL.
"""UUID libpq differential; owns only its temporary PostgreSQL reference cluster."""
import ctypes as c
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import uuid

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
evidence = Path(sys.argv[3])
evidence.mkdir(parents=True, exist_ok=True)
count = 0
connections = []
started = False
summary = dict(passed=False, libpq=version(), cases=0)
with tempfile.TemporaryDirectory(prefix='pg05-uuid-reference-') as root:
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    try:
        subprocess.run([str(binpath/'initdb'), '-D', root+'/data', '-U', 'root', '-A', 'trust', '--encoding=UTF8', '--no-locale'], check=True, stdout=subprocess.DEVNULL)
        # Mark ownership before startup so even an interrupted pg_ctl is stopped.
        started = True
        subprocess.run([str(binpath/'pg_ctl'), '-D', root+'/data', '-l', root+'/log', '-o', f'-h 127.0.0.1 -p {port} -k {root}', '-w', 'start'], check=True, stdout=subprocess.DEVNULL)
        for endpoint in [f'port={port} dbname=postgres', f'port={sys.argv[1]} dbname={sys.argv[2]}']:
            conn = connect(f'host=127.0.0.1 user=root connect_timeout=10 {endpoint}'.encode())
            assert conn
            connections.append(conn)
            assert connected(conn) == 0, 'client connection failed'
        ref, native = connections
        identity = {key: query(ref, sql) for key, sql in [
            ('version', 'SELECT version()'), ('server_version_num', 'SHOW server_version_num'),
            ('server_encoding', 'SHOW server_encoding'), ('client_encoding', 'SHOW client_encoding'),
            ('DateStyle', 'SHOW DateStyle'), ('TimeZone', 'SHOW TimeZone')]}
        actual_version = int(bytes.fromhex(identity['server_version_num']['rows'][0][0]))
        summary.update(reference=identity, reference_version_num=actual_version,
                       reference_build=subprocess.check_output([str(binpath/'pg_config'), '--configure'], text=True).strip())
        (evidence/'identity.json').write_text(json.dumps(summary, indent=2))
        print(f'reference_version_num={actual_version}', flush=True)
        assert actual_version == 180006 or os.environ.get('PG05_UUID_SUPPLEMENTAL') == '1', 'exact PostgreSQL 18.6 required; supplemental runs must opt in'
        with (evidence/'cases.jsonl').open('w') as records:
            def record(phase, name, data, infmt, outfmt, expected):
                global count
                r, a = [bind(conn, name, data, infmt, outfmt) for conn in connections]
                entry = dict(phase=phase, statement=name.decode(), input_format=infmt, output_format=outfmt,
                             parameter_oid=2950, input=None if data is None else data.hex(), reference=r, aster=a)
                records.write(json.dumps(entry)+'\n')
                records.flush()
                assert r == a, entry
                assert a == expected, entry
                count += 1
            for name, sql, oids in [
                (b'declared', b'SELECT $1 AS v', [2950]),
                (b'inferred', b'SELECT $1::uuid AS v', []),
                (b'zero', b'SELECT $1::uuid AS v', [0]),
                (b'empty', b'SELECT $1::uuid AS v WHERE false', [2950]),
                (b'first_null', b'SELECT NULL::uuid AS v UNION ALL SELECT $1::uuid AS v', [2950])]:
                for conn in connections:
                    result = observe(prepare(conn, name, sql, len(oids), (c.c_uint*len(oids))(*oids)))
                    assert result['status'] == 1, result
                    result = describe(conn, name)
                    try:
                        assert status(result) == 1
                        assert nparams(result) == 1 and paramtype(result, 0) == 2950
                        assert [ftype(result, 0), fsize(result, 0), fmod(result, 0), fformat(result, 0)] == [2950, 16, -1, 0]
                    finally:
                        clear(result)
                records.write(json.dumps(dict(phase='Parse/statement Describe', statement=name.decode(), parameter_oids=[2950], metadata=[[2950,16,-1,0]]))+'\n')
            payloads = [bytes(16), bytes([255])*16, bytes.fromhex('00112233445566778899aabbccddeeff'), bytes(range(16))]
            payloads += [(1 << bit).to_bytes(16, 'big') for bit in range(128)]
            payloads += [((1 << 128)-1 ^ (1 << bit)).to_bytes(16, 'big') for bit in range(128)]
            spellings = [b'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', b'A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11',
                         b'{a0eebc999c0b4ef8bb6d6bb9bd380a11}', b'a0ee-bc99-9c0b-4ef8-bb6d-6bb9-bd38-0a11']
            for infmt in [0,1]:
                for outfmt in [0,1]:
                    def expected(raw):
                        return None if raw is None else (raw if outfmt else str(uuid.UUID(bytes=raw)).encode()).hex()
                    for raw in payloads + [None]:
                        data = None if raw is None else (raw if infmt else str(uuid.UUID(bytes=raw)).encode())
                        record('Bind/Execute/rebind', b'declared', data, infmt, outfmt,
                               dict(status=2,state='N/A',metadata=[[2950,16,-1,outfmt]],rows=[[expected(raw)]]))
                    for name in [b'inferred', b'zero', b'empty', b'first_null']:
                        for raw in [payloads[2], None]:
                            data = None if raw is None else (raw if infmt else str(uuid.UUID(bytes=raw)).encode())
                            result_rows = [] if name == b'empty' else ([[None]] if name == b'first_null' else []) + [[expected(raw)]]
                            record('Bind/Execute/null-shape', name, data, infmt, outfmt,
                                   dict(status=2,state='N/A',metadata=[[2950,16,-1,outfmt]],rows=result_rows))
                    if infmt == 0:
                        for text in spellings:
                            record('Bind/Execute/spelling', b'declared', text, infmt, outfmt,
                                   dict(status=2,state='N/A',metadata=[[2950,16,-1,outfmt]],rows=[[(bytes.fromhex('a0eebc999c0b4ef8bb6d6bb9bd380a11') if outfmt else b'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11').hex()]]))
            print('PG05_UUID_MID', flush=True)
            assert sys.stdin.readline().strip() == 'continue'
            invalid = [(0, x, '22P02') for x in [b'', b'bad', b' 00112233445566778899aabbccddeeff',
                b'00112233445566778899aabbccddeeff ', b'urn:uuid:00112233-4455-6677-8899-aabbccddeeff',
                b'00112233445566778899aabbccddeef', b'00112233445566778899aabbccddeeff0',
                b'00112233445566778899aabbccddeefg', b'00-112233445566778899aabbccddeeff',
                b'00112233445566778899aabbccddeeff-', b'{00112233445566778899aabbccddeeff',
                b'00112233445566778899aabbccddeeff}']]
            invalid += [(0,b'\xff','22021'),(0,b'\xc3(','22021')]
            invalid += [(1,bytes(n),'08P01' if n < 16 else '22P03') for n in list(range(16))+[17,32]]
            for outfmt in [0,1]:
                for infmt, data, state in invalid:
                    record('Bind/reject/no-DataRow', b'declared', data, infmt, outfmt,
                           dict(status=7,state=state,metadata=[],rows=[]))
                    # PQexecPrepared sends Sync; this probes reuse of that statement
                    # and connection after every rejection, with a different value.
                    record('Sync/recovery', b'declared', payloads[3], 1, outfmt,
                           dict(status=2,state='N/A',metadata=[[2950,16,-1,outfmt]],rows=[[(payloads[3] if outfmt else str(uuid.UUID(bytes=payloads[3])).encode()).hex()]]))
            for conn in connections:
                for name in [b'declared', b'inferred', b'zero', b'empty', b'first_null']:
                    result = observe(close_prepared(conn, name))
                    assert result['status'] == 1, result
        summary.update(cases=count, passed=actual_version == 180006, supplemental=actual_version != 180006)
    finally:
        for conn in connections:
            finish(conn)
        if started:
            subprocess.run([str(binpath/'pg_ctl'), '-D', root+'/data', '-m', 'immediate', '-w', 'stop'], check=True, stdout=subprocess.DEVNULL)
            with socket.socket() as sock:
                assert sock.connect_ex(('127.0.0.1',port)) != 0
        summary.update(cases=count, reference_port_closed=True, clients_closed=True)
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
# TemporaryDirectory has now removed the owned cluster, including after errors.
summary['reference_directory_removed'] = not Path(root).exists()
(evidence/'summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary), flush=True)
