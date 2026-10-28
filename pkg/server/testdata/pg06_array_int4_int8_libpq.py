# Copyright 2026 AsterSQL.
"""Task 82 int4[]-to-int8[] differential against the fixed PostgreSQL 18.6 reference."""
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
    query(ref, 'SET plan_cache_mode = force_generic_plan')
    assert version() == server_version(ref) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
    assert query(ref, 'SELECT castcontext FROM pg_cast WHERE castsource=23 AND casttarget=20')['rows'] == [[b'i'.hex()]]
    def compare(sql, expected_status=2, state='N/A'):
        global count
        results = [observe(execute(conn, (sql.replace('pg06_082', 'public.pg06_082') if conn == native else sql).encode())) for conn in connections]
        assert results[0] == results[1], (sql, results)
        assert results[0]['status'] == expected_status and results[0]['state'] == state, (sql, results)
        count += 1
        return results[0]
    compare('SELECT ARRAY[1::int4,NULL::int4]::int8[] AS v')
    for name,sql,oid in [(b'explicit',b'SELECT $1::int8[] AS v',1007),
                         (b'inferred',b'SELECT $1::int4[]::int8[] AS v',0),
                         (b'implicit',b'SELECT coalesce(NULL::int8[],$1) AS v',1007)]:
        for conn in connections:
            result = observe(prepare(conn,name,sql,1,(c.c_uint*1)(oid)))
            assert result['status'] == 1, (name,result)
            description = describe(conn,name)
            assert nparams(description) == 1 and paramtype(description,0) == 1007
            observed = observe(description)
            assert observed['metadata'] == [[1016,-1,-1,0]], observed
    import struct
    def binary(shape, values, width):
        oid = 23 if width == 4 else 20
        data = struct.pack('!iii',len(shape),int(None in values),oid)
        data += b''.join(struct.pack('!ii',*axis) for axis in shape)
        for value in values:
            data += struct.pack('!i',-1 if value is None else width)
            if value is not None:
                data += struct.pack('!i' if width == 4 else '!q',value)
        return data
    fixtures = [(b'{1,NULL}',[(2,1)],[1,None]),
                (b'[0:1][-2:-1]={{-2147483648,NULL},{0,2147483647}}',[(2,0),(2,-2)],[-2147483648,None,0,2147483647]),
                (b'{}',[],[]),(b'{NULL}',[(1,1)],[None]),(None,[],[])]
    for text,shape,values in fixtures:
        for name in [b'explicit',b'inferred',b'implicit']:
            for infmt in [0,1]:
                payload = None if text is None else text if infmt == 0 else binary(shape,values,4)
                for outfmt in [0,1]:
                    results = [bind(conn,name,payload,infmt,outfmt) for conn in connections]
                    assert results[0] == results[1] and results[0]['status'] == 2, (name,text,infmt,outfmt,results)
                    expected = None if text is None else text if outfmt == 0 else binary(shape,values,8)
                    assert results[0]['rows'] == [[None if expected is None else expected.hex()]], results
                    assert results[0]['metadata'] == [[1016,-1,-1,outfmt]], results
                    count += 1
        literal = 'NULL' if text is None else "'"+text.decode()+"'"
        compare(f'SELECT ({literal}::int4[])::int8[] AS v')
    compare('SELECT (NULL::int4[])::int8[] AS v LIMIT 0')
    query(ref,'CREATE TEMP TABLE pg06_082 (id int, v int8[])')
    for text,_,_ in fixtures:
        literal = 'NULL' if text is None else "'"+text.decode()+"'"
        compare(f'INSERT INTO pg06_082 VALUES (1,{literal}::int4[])',1)
        compare(f'INSERT INTO pg06_082 SELECT 2,{literal}::int4[]',1)
        compare(f'UPDATE pg06_082 SET v={literal}::int4[] WHERE id=1',1)
        compare('SELECT v FROM pg06_082 ORDER BY id')
        compare('DELETE FROM pg06_082',1)
    compare('INSERT INTO pg06_082 VALUES (1,NULL)',1)
    original = compare('SELECT v FROM pg06_082')
    # No int4 value overflows int8. Invalid elements must fail while decoding
    # the source array, before any partial row or assignment is visible.
    for literal,state in [("'{1,garbage,2}'",'22P02'),("'{1,2147483648,2}'",'22003')]:
        for sql in [f'SELECT ({literal}::int4[])::int8[]',f'INSERT INTO pg06_082 VALUES (2,{literal}::int4[])',f'UPDATE pg06_082 SET v={literal}::int4[] WHERE id=1']:
            compare(sql,7,state)
            compare('SELECT 1::int4')
            assert compare('SELECT v FROM pg06_082') == original
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
            send_message(wire,b'P',b'phase\0SELECT $1::int8[] AS v\0'+struct.pack('!hI',1,1007))
            send_message(wire,b'H')
            assert receive(wire) == (b'1',b'')
            recover(wire)
            for text,state in [(b'{1,garbage,2}',b'22P02'),(b'{1,2147483648,2}',b'22003')]:
                body = b'\0phase\0'+struct.pack('!hhi',0,1,len(text))+text+struct.pack('!h',0)
                send_message(wire,b'B',body)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', message
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state, fields
                recover(wire)
                count += 1
    print(f'array_int4_int8_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
