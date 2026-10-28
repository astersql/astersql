# Copyright 2026 AsterSQL.
"""Task 83 forbidden bytea/boolean casts differential against the fixed PostgreSQL 18.6 reference."""
import ctypes as c
import os
import sys

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
    message = ef(raw, ord('M'))
    result = observe(raw)
    assert result['status'] in [1, 2], (sql, result, message)
    return result

def bind(conn, name, data, infmt, outfmt):
    # c_char_p preserves the pointer, including NULs inside binary payloads;
    # explicit lengths are authoritative only in binary format.
    return observe(prepared(conn, name, 1, (s*1)(data),
                            (i*1)(len(data) if data is not None else 0), (i*1)(infmt), outfmt))


server_version = api('PQserverVersion', i, p)
connections = []
count = 0
try:
    for dsn in [os.environ['PG18_REFERENCE_DSN'], f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test']:
        conn = connect(dsn.encode())
        assert conn and connected(conn) == 0, 'connection failed'
        connections.append(conn)
    ref, native = connections
    assert version() == server_version(ref) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
    # Neither an existing catalog identity nor equal storage length gives
    # bytea/boolean a conversion path. pg_cast absence alone is not proof.
    assert query(ref, 'SELECT castsource FROM pg_cast WHERE (castsource=17 AND casttarget=16) OR (castsource=16 AND casttarget=17)')['rows'] == []
    def compare(sql, state='N/A'):
        global count
        results = [observe(execute(conn, sql.encode())) for conn in connections]
        assert results[0] == results[1], (sql, results)
        assert results[0]['state'] == state, (sql, results)
        assert results[0]['status'] == (2 if state == 'N/A' else 7), results
        if state != 'N/A':
            assert results[0]['rows'] == results[0]['metadata'] == [], results
        count += 1
        return results[0]
    for expr in ["'true'::bytea", "''::bytea", "'\\x00'::bytea", 'NULL::bytea']:
        for suffix in ['', ' LIMIT 0', ' WHERE false']:
            compare(f'SELECT ({expr})::bool{suffix}', '42846')
            compare('SELECT 1::int4')
    for expr in ['true::bool', 'false::bool', 'NULL::bool']:
        compare(f'SELECT ({expr})::bytea', '42846')
        compare('SELECT 1::int4')
    # Identity casts still preserve typed NULL and metadata in both formats.
    for name, sql, oid, width, data, binary in [
        (b'bytes', b'SELECT $1::bytea AS v',17,-1,b'\\x00ff',b'\x00\xff'),
        (b'boolean', b'SELECT $1::bool AS v',16,1,b'true',b'\x01')]:
        for conn in connections:
            assert observe(prepare(conn,name,sql,1,(c.c_uint*1)(oid)))['status'] == 1
            result = describe(conn,name)
            assert nparams(result) == 1 and paramtype(result,0) == oid
            assert observe(result)['metadata'] == [[oid,width,-1,0]]
        for infmt in [0,1]:
            for outfmt in [0,1]:
                for payload in [None, data if infmt == 0 else binary]:
                    results = [bind(conn,name,payload,infmt,outfmt) for conn in connections]
                    assert results[0] == results[1] and results[0]['status'] == 2, (name,results)
                    expected = None if payload is None else binary if outfmt == 1 else b't' if oid == 16 else data
                    assert results[0]['rows'] == [[None if expected is None else expected.hex()]], results
                    assert results[0]['metadata'] == [[oid,width,-1,outfmt]], results
                    count += 1
    import socket
    import struct
    socket_fd = api('PQsocket', i, p)
    def receive(wire):
        def exact(size):
            data = b''
            while len(data) < size:
                part = wire.recv(size-len(data))
                assert part, 'unexpected EOF'
                data += part
            return data
        tag = exact(1)
        size = struct.unpack('!I',exact(4))[0]
        return tag,exact(size-4)
    def send_message(wire,tag,body=b''):
        wire.sendall(tag+struct.pack('!I',len(body)+4)+body)
    def recover(wire):
        send_message(wire,b'S')
        assert receive(wire) == (b'Z',b'I')
        send_message(wire,b'Q',b'SELECT 1::int4\0')
        messages = []
        while True:
            message = receive(wire)
            messages.append(message)
            if message[0] == b'Z':
                break
        assert [m[0] for m in messages] == [b'T',b'D',b'C',b'Z']
        assert messages[1][1] == struct.pack('!hi',1,1)+b'1'
    for conn in connections:
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            for sql, oid in [(b'SELECT (NULL::bytea)::bool',0),
                             (b'SELECT (true::bool)::bytea',0),
                             (b'SELECT $1::bool',17),
                             (b'SELECT $1::bytea',16),
                             (b'SELECT $1::bytea::bool',0)]:
                # Parse+Flush must reject before Bind, Describe or Execute;
                # ErrorResponse is the first response, hence no partial result.
                oids = struct.pack('!hI',1,oid) if b'$1' in sql else struct.pack('!h',0)
                send_message(wire,b'P',b'phase83\0'+sql+b'\0'+oids)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (sql,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == b'42846', (sql,fields)
                recover(wire)
                count += 1
    # A legal PG cast that is still unavailable locally retains the separate
    # capability error, instead of being relabelled as a nonexistent path.
    # name (OID 19) -> text is legal in PG, but the native generic
    # parameter codec has not opened that source identity.
    for conn in connections:
        result = observe(prepare(conn,b'legal83',b'SELECT $1::text',1,(c.c_uint*1)(19)))
        assert result['status'] == (1 if conn == ref else 7), result
        assert result['state'] == ('N/A' if conn == ref else '0A000'), result
        assert result['rows'] == result['metadata'] == [], result
    assert bind(ref,b'legal83',b'abc',0,0)['rows'] == [[b'abc'.hex()]]
    query(native,'SELECT 1::int4')
    print(f'bytea_bool_differential_cases={count} passed; legal_unimplemented=0A000',flush=True)
finally:
    for conn in connections:
        finish(conn)
