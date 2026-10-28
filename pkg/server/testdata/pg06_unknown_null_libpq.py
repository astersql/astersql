# Copyright 2026 AsterSQL.
"""Task 85 unknown NULL typing differential against the fixed PostgreSQL 18.6 reference."""
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
nparams = api('PQnparams', i, p)
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
                    rows=sorted([[None if null(result,r,n) else c.string_at(value(result,r,n), length(result,r,n)).hex() for n in range(cols(result))] for r in range(rows(result))],key=repr))
    finally:
        clear(result)

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
    for sql, oid, width, expected in [
        ('SELECT NULL::int4',23,4,[[None]]),
        ('SELECT NULL',25,-1,[[None]]),
        ('SELECT NULL UNION SELECT 1::int4',23,4,[[None],['31']]),
        ('SELECT NULL UNION SELECT NULL',25,-1,[[None]]),
        ('SELECT NULL::int4 UNION SELECT 2147483647::int4',23,4,[[None],['32313437343833363437']]),
        ('SELECT NULL::int4 UNION SELECT (-2147483648)::int4',23,4,[[None],['2d32313437343833363438']]),
        ('SELECT (NULL::text)::int4',23,4,[[None]]),
        ('SELECT NULL::int4 WHERE false',23,4,[]),
    ]:
        result = compare(sql)
        assert result['metadata'] == [[oid,width,-1,0]], (sql,result)
        assert sorted(result['rows'], key=repr) == sorted(expected,key=repr), (sql,result)
    # A NULL value cannot erase the source type's cast/operator eligibility.
    errors = [("SELECT (NULL::bytea)::bool", '42846'),
              ("SELECT (NULL::bytea)::bool WHERE false", '42846'),
              ("SELECT (NULL::text)+1::int4", '42883')]
    for sql, state in errors:
        compare(sql,state)
        compare('SELECT 1::int4')
    # Describe resolves bare unknown output to text; explicit casts choose
    # their own identity. Execute in both formats preserves this description.
    for name, sql, oid, width, typmod in [
        (b'bare85', b"SELECT NULL AS v",25,-1,-1),
        (b'text85', b"SELECT (NULL::text) AS v",25,-1,-1),
        (b'int85', b"SELECT NULL::int4 AS v",23,4,-1),
        (b'varchar85', b"SELECT NULL::varchar(3) AS v",1043,-1,7),
        (b'union85', b"SELECT NULL UNION SELECT 1::int4",23,4,-1)]:
        for conn in connections:
            assert observe(prepare(conn,name,sql,0,None))['status'] == 1
            result = describe(conn,name)
            assert nparams(result) == 0
            assert observe(result)['metadata'] == [[oid,width,typmod,0]]
        for outfmt in [0,1]:
            results = [observe(prepared(conn,name,0,None,None,None,outfmt)) for conn in connections]
            assert results[0] == results[1] and results[0]['status'] == 2, (sql,results)
            expected_rows = [[None]]
            if name == b'union85':
                expected_rows.append(['00000001' if outfmt else '31'])
            assert results[0]['rows'] == sorted(expected_rows,key=repr), (sql,results)
            assert results[0]['metadata'] == [[oid,width,typmod,outfmt]]
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
            for sql, state in errors:
                sql = sql.encode()
                # Parse+Flush must reject before Bind, Describe or Execute;
                # ErrorResponse is the first response, hence no partial result.
                oids = struct.pack('!h',0)
                send_message(wire,b'P',b'phase85\0'+sql+b'\0'+oids)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (sql,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state.encode(), (sql,fields)
                recover(wire)
                count += 1
    print(f'unknown_null_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
