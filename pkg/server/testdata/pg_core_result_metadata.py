# Copyright 2026 AsterSQL.
"""Compare integer result metadata across real TCP stages and libpq execution."""
import ctypes as c
import ctypes.util
import os
import socket
import struct
import sys

port = int(sys.argv[1])
pack = struct.pack
expected = [('v4', 0, 0, 23, 4, -1, 0), ('v8', 0, 0, 20, 8, -1, 0)]
count = 0

def exact(sock, size):
    result = b''
    while len(result) < size:
        part = sock.recv(size - len(result))
        assert part, 'unexpected EOF'
        result += part
    return result

def read(sock):
    tag, size = struct.unpack('!cI', exact(sock, 5))
    assert 4 <= size < 1 << 20
    return tag, exact(sock, size - 4)

def send(sock, tag, body=b''):
    sock.sendall(tag + pack('!I', len(body) + 4) + body)
    if tag in (b'P', b'B', b'D', b'E'):
        sock.sendall(b'H' + pack('!I', 4))

def until(sock, tag):
    result = []
    while True:
        message = read(sock)
        result.append(message)
        if message[0] == tag:
            return result

def query(sock, text):
    send(sock, b'Q', text.encode() + b'\0')
    result = until(sock, b'Z')
    assert all(tag != b'E' for tag, _ in result), result
    return result

def fields(message):
    assert message[0] == b'T', message
    body = message[1]
    length, = struct.unpack_from('!h', body)
    offset = 2
    result = []
    for _ in range(length):
        end = body.index(0, offset)
        name = body[offset:end].decode()
        offset = end + 1
        result.append((name, *struct.unpack_from('!IhIhih', body, offset)))
        offset += 18
    assert offset == len(body)
    return result

def row(values):
    body = pack('!h', len(values))
    for value in values:
        body += pack('!i', -1) if value is None else pack('!i', len(value.encode())) + value.encode()
    return b'D', body

def sync(sock):
    send(sock, b'S')
    assert read(sock) == (b'Z', b'I')

def describe(sock, target, name, wanted=expected):
    send(sock, b'D', target + name + b'\0')
    if target == b'S':
        assert read(sock)[0] == b't'
    result = read(sock)
    assert fields(result) == wanted, fields(result)
    return result

with socket.create_connection(('127.0.0.1', port), timeout=10) as sock:
    body = pack('!I', 196608) + b'user\0root\0database\0test\0\0'
    sock.sendall(pack('!I', len(body) + 4) + body)
    startup = until(sock, b'Z')
    assert not any(tag == b'E' for tag, _ in startup), startup
    query(sock, 'CREATE TABLE public.pg_result_metadata_9 (id INT)')
    try:
        query(sock, 'INSERT INTO public.pg_result_metadata_9 VALUES (-17), (31)')
        cases = [
            ('SELECT NULL::int4 AS v4, NULL::int8 AS v8', [row([None, None])]),
            ('SELECT id::int4 AS v4, id::int8 AS v8 FROM public.pg_result_metadata_9 WHERE 1=0', []),
            ('SELECT id::int4 AS v4, id::int8 AS v8 FROM public.pg_result_metadata_9 ORDER BY id', [row(['-17', '-17']), row(['31', '31'])]),
            ('SELECT CAST(NULL AS integer) AS v4, CAST(NULL AS bigint) AS v8', [row([None, None])]),
        ]
        for index, (sql, rows) in enumerate(cases):
            simple = query(sock, sql)
            wanted = expected
            if os.environ.get('PG_METADATA_REFERENCE') and ' FROM ' in sql:
                # PostgreSQL removes an int4-to-int4 no-op cast and retains Var origin.
                actual = fields(simple[0])
                assert actual[0][1] > 0 and actual[0][2] == 1, actual
                wanted = [(expected[0][0], actual[0][1], 1, *expected[0][3:]), expected[1]]
            assert fields(simple[0]) == wanted, (sql, fields(simple[0]))
            assert [m for m in simple if m[0] == b'D'] == rows
            name = f'metadata_{index}'.encode()
            send(sock, b'P', name + b'\0' + sql.encode() + b'\0' + pack('!h', 0))
            assert read(sock) == (b'1', b'')
            assert describe(sock, b'S', name, wanted) == simple[0]
            send(sock, b'B', b'\0' + name + b'\0' + pack('!hhh', 0, 0, 0))
            assert read(sock) == (b'2', b'')
            assert describe(sock, b'P', b'', wanted) == simple[0]
            send(sock, b'E', b'\0' + pack('!I', 0))
            execution = until(sock, b'C')
            assert execution == rows + [(b'C', f'SELECT {len(rows)}\0'.encode())], execution
            sync(sock)
            count += 1
        sql = b'SELECT $1::int4 AS v4, $2::int8 AS v8'
        # An invalid int4 Bind must recover without changing the prepared metadata.
        send(sock, b'P', b'repeated\0' + sql + b'\0' + pack('!hII', 2, 23, 20))
        assert read(sock) == (b'1', b'')
        baseline = describe(sock, b'S', b'repeated')
        for values in [(None, None), ('17', '9007199254740993'), ('-17', '-9223372036854775808'), (None, '31'), ('2147483648', '1'), ('31', None)]:
            assert describe(sock, b'S', b'repeated') == baseline
            params = b''.join(pack('!i', -1) if v is None else pack('!i', len(v)) + v.encode() for v in values)
            send(sock, b'B', b'\0repeated\0' + pack('!hh', 0, 2) + params + pack('!h', 0))
            bound = read(sock)
            if values[0] == '2147483648':
                assert bound[0] == b'E' and b'C22003\0' in bound[1], bound
            else:
                assert bound == (b'2', b''), bound
                assert describe(sock, b'P', b'') == baseline
                send(sock, b'E', b'\0' + pack('!I', 0))
                actual_row = read(sock)
                assert actual_row == row(values), (values, actual_row)
                assert read(sock) == (b'C', b'SELECT 1\0')
            sync(sock)
            count += 1
        # NULL is encoded as -1, while an empty text value has length zero.
        assert query(sock, "SELECT NULL::int4 AS v4, '' AS empty")[1] == row([None, ''])
    finally:
        sync(sock)
        query(sock, 'DROP TABLE public.pg_result_metadata_9')
        send(sock, b'X')

pq = c.CDLL(os.environ.get('PG_LIBPQ_LIBRARY') or ('/opt/homebrew/opt/libpq/lib/libpq.dylib' if sys.platform == 'darwin' else ctypes.util.find_library('pq')))
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
p, s, i, u = c.c_void_p, c.c_char_p, c.c_int, c.c_uint
connect = api('PQconnectdb', p, s)
status = api('PQstatus', i, p)
finish = api('PQfinish', None, p)
clear = api('PQclear', None, p)
rs = api('PQresultStatus', i, p)
error = api('PQresultErrorMessage', s, p)
execute = api('PQexec', p, p, s)
prepare = api('PQprepare', p, p, s, s, i, c.POINTER(u))
describe_prepared = api('PQdescribePrepared', p, p, s)
bind = api('PQexecPrepared', p, p, s, i, c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
cols = api('PQnfields', i, p)
rows = api('PQntuples', i, p)
value = api('PQgetvalue', s, p, i, i)
null = api('PQgetisnull', i, p, i, i)
get_length = api('PQgetlength', i, p, i, i)
field_functions = [api(name, result, p, i) for name, result in [('PQfname', s), ('PQftable', u), ('PQftablecol', i), ('PQftype', u), ('PQfsize', i), ('PQfmod', i), ('PQfformat', i)]]
version = api('PQlibVersion', i)
conn = connect(f'host=127.0.0.1 port={port} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5'.encode())
assert conn and status(conn) == 0

def check(result, result_status, metadata=True):
    assert result and rs(result) == result_status, error(result)
    if metadata:
        actual = [tuple(fn(result, j).decode() if k == 0 else fn(result, j) for k, fn in enumerate(field_functions)) for j in range(cols(result))]
        assert actual == expected, actual

try:
    if os.environ.get('PG_METADATA_REFERENCE'):
        result = execute(conn, b'SELECT version()')
        try:
            check(result, 2, False)
            print(value(result, 0, 0).decode(), flush=True)
        finally:
            clear(result)
    result = prepare(conn, b'libpq_metadata_9', b'SELECT $1::int4 AS v4, $2::int8 AS v8', 0, None)
    try:
        check(result, 1, False)
    finally:
        clear(result)
    for inputs in [(None, None), ('17', '9007199254740993'), ('-17', '-9223372036854775808'), (None, '31'), ('31', None)]:
        result = describe_prepared(conn, b'libpq_metadata_9')
        try:
            check(result, 1)
        finally:
            clear(result)
        result = bind(conn, b'libpq_metadata_9', 2, (s * 2)(*[None if v is None else v.encode() for v in inputs]), None, None, 0)
        try:
            check(result, 2)
            assert rows(result) == 1
            actual = [None if null(result, 0, j) else value(result, 0, j).decode() for j in range(2)]
            assert actual == list(inputs), actual
            for j, text in enumerate(inputs):
                assert get_length(result, 0, j) == (0 if text is None else len(text))
            count += 1
        finally:
            clear(result)
    print('libpq', version(), 'effective metadata scenarios', count, 'passed', flush=True)
finally:
    finish(conn)
