# Copyright 2026 AsterSQL.
"""Task 81 jsonb-to-json differential against the fixed PostgreSQL 18.6 reference."""
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
    if data is not None and infmt == 1:
        data = b'\x01' + data
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
    # Keep planning deterministic on the reference endpoint.
    query(ref, 'SET plan_cache_mode = force_generic_plan')
    assert version() == server_version(ref) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
    assert query(ref, 'SELECT castcontext FROM pg_cast WHERE castsource=3802 AND casttarget=114')['rows'] == [[b'a'.hex()]]
    # Shared parser literals require doubled backslashes; bound jsonb bytes below
    # are identical on both endpoints and are never adapted.
    def compare(sql, expected_status=2, state='N/A'):
        global count
        results = [observe(execute(conn, (sql.replace('pg06_081', 'public.pg06_081').replace('\\', '\\\\') if conn == native else sql).encode())) for conn in connections]
        assert results[0] == results[1], (sql, results)
        assert results[0]['status'] == expected_status and results[0]['state'] == state, (sql, results)
        count += 1
        return results[0]
    fixtures = [(b'{"a":1}',b'{"a": 1}'), (b' {"aa":1,"b":2,"aa":3.00} ',b'{"b": 2, "aa": 3.00}'),(br'"\u4e2d\ud83d\ude00"','"中😀"'.encode()),(b'null',b'null'),(None,None)]
    for conn in connections:
        result = observe(prepare(conn,b'explicit',b'SELECT $1::json AS v',1,(c.c_uint*1)(3802)))
        assert result['status'] == 1, result
        description = describe(conn,b'explicit')
        assert nparams(description) == 1 and paramtype(description,0) == 3802
        observed = observe(description)
        assert observed['metadata'] == [[114,-1,-1,0]], observed
    for text,expected in fixtures:
        for infmt in [0,1]:
            for outfmt in [0,1]:
                results = [bind(conn,b'explicit',text,infmt,outfmt) for conn in connections]
                assert results[0] == results[1] and results[0]['status'] == 2, (text,infmt,outfmt,results)
                encoded = None if expected is None else expected.hex()
                assert results[0]['rows'] == [[encoded]], results
                assert results[0]['metadata'] == [[114,-1,-1,outfmt]], results
                count += 1
        literal = 'NULL' if text is None else "'"+text.decode()+"'"
        compare(f'SELECT ({literal}::jsonb)::json AS v')
    compare('SELECT (NULL::jsonb)::json AS v LIMIT 0')
    # Largest exponent inside the source jsonb numeric domain.
    compare("SELECT ('1e131071'::jsonb)::json AS v")
    for text,state in [(b'garbage','22P02'),(br'"\u0000"','22P05'),(br'"\ud800"','22P02'),(b'1e200000','22003')]:
        for infmt in [0,1]:
            results = [bind(conn,b'explicit',text,infmt,0) for conn in connections]
            assert results[0] == results[1] and results[0]['state'] == state, (text,infmt,results)
            assert results[0]['status'] == 7 and not results[0]['rows'], results
            count += 1
            compare('SELECT 1::int4')
    query(ref,'CREATE TEMP TABLE pg06_081 (id int, v json)')
    for text,_ in fixtures:
        literal = 'NULL' if text is None else "'"+text.decode()+"'"
        compare(f'INSERT INTO pg06_081 VALUES (1,{literal}::jsonb)',1)
        compare(f'INSERT INTO pg06_081 SELECT 2,{literal}::jsonb',1)
        compare(f'UPDATE pg06_081 SET v={literal}::jsonb WHERE id=1',1)
        compare('SELECT v FROM pg06_081 ORDER BY id')
        compare('DELETE FROM pg06_081',1)
    compare('INSERT INTO pg06_081 VALUES (1,NULL)',1)
    original = compare('SELECT v FROM pg06_081')
    for literal,state in [("'garbage'",'22P02'),("'\"\\u0000\"'",'22P05'),("'\"\\ud800\"'",'22P02'),("'1e200000'",'22003')]:
        for sql in [f'SELECT ({literal}::jsonb)::json',f'INSERT INTO pg06_081 VALUES (2,{literal}::jsonb)',f'UPDATE pg06_081 SET v={literal}::jsonb WHERE id=1']:
            compare(sql,7,state)
            compare('SELECT 1::int4')
            assert compare('SELECT v FROM pg06_081') == original
    compare('SELECT coalesce(NULL::json,NULL::jsonb)',7,'42846')
    compare('SELECT 1::int4')
    # Inspect the actual protocol boundary on both authenticated libpq sockets.
    # No libpq operations run until these exchanges have drained ReadyForQuery.
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
            send_message(wire,b'P',b'implicit\0SELECT coalesce(NULL::json,$1)\0'+struct.pack('!hI',1,3802))
            send_message(wire,b'H')
            error = receive(wire)
            assert error[0] == b'E', error
            assert b'C42846\0' in error[1], error
            recover(wire)
            count += 1
            send_message(wire,b'P',b'phase\0SELECT $1::json AS v\0'+struct.pack('!hI',1,3802))
            send_message(wire,b'H')
            assert receive(wire) == (b'1',b'')
            recover(wire)
            for text,state,phase in [(b'garbage',b'22P02',b'B'),(br'"\u0000"',b'22P05',b'B'),(br'"\ud800"',b'22P02',b'B'),(b'1e200000',b'22003',b'B')]:
                body = b'\0phase\0'+struct.pack('!hhi',0,1,len(text))+text+struct.pack('!h',0)
                send_message(wire,b'B',body)
                send_message(wire,b'H')
                if phase == b'E':
                    response = receive(wire)
                    assert response == (b'2',b''), response
                    send_message(wire,b'E',b'\0'+struct.pack('!i',0))
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (phase,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state, fields
                recover(wire)
                count += 1
    print(f'jsonb_json_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
