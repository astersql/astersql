# Copyright 2026 AsterSQL.
"""Real libpq named prepared lifecycle, also runnable against PostgreSQL 18."""
import ctypes as c
import ctypes.util
import os
import sys

library = os.environ.get('PG_LIBPQ_LIBRARY') or ('/opt/homebrew/opt/libpq/lib/libpq.dylib' if sys.platform == 'darwin' else ctypes.util.find_library('pq'))
pq = c.CDLL(library)
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
p, s, i = c.c_void_p, c.c_char_p, c.c_int
connect = api('PQconnectdb', p, s)
status = api('PQstatus', i, p)
finish = api('PQfinish', None, p)
prepare = api('PQprepare', p, p, s, s, i, c.POINTER(c.c_uint))
execute = api('PQexecPrepared', p, p, s, i, c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
query = api('PQexec', p, p, s)
close = api('PQclosePrepared', p, p, s)
rs = api('PQresultStatus', i, p)
value = api('PQgetvalue', s, p, i, i)
rows = api('PQntuples', i, p)
columns = api('PQnfields', i, p)
column_type = api('PQftype', c.c_uint, p, i)
clear = api('PQclear', None, p)
error = api('PQresultErrorMessage', s, p)
field = api('PQresultErrorField', s, p, i)
version = api('PQlibVersion', i)
assert version() >= 180000
print('libpq', version(), flush=True)
def check(result, expected=None, state=None):
    assert result
    try:
        assert rs(result) == (7 if state else 2 if expected is not None else 1), error(result)
        if state:
            assert field(result, ord('C')) == state.encode(), error(result)
        if expected is not None:
            assert (rows(result), columns(result)) == (1, 1)
            assert column_type(result, 0) == 20, "BIGINT table projection must describe int8"
            assert value(result, 0, 0).decode() == expected
    finally:
        clear(result)
def run(conn, name, number):
    values = (s * 1)(str(number).encode())
    check(execute(conn, name.encode(), 1, values, None, None, 0), str(number))
connections = []
try:
    for _ in range(2):
        conn = connect(f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5'.encode())
        assert conn
        connections.append(conn)
        assert status(conn) == 0
        if len(connections) == 1:
            check(query(conn, b'CREATE TABLE public.pg_prepared_identity (id BIGINT PRIMARY KEY)'))
            check(query(conn, b'INSERT INTO public.pg_prepared_identity VALUES (17),(31),(43),(59),(2147483648),(9223372036854775807)'))
        for name, oid in [('identity_a', 23), ('identity_b', 20)]:
            check(prepare(conn, name.encode(), b'SELECT id FROM public.pg_prepared_identity WHERE id = $1', 1, (c.c_uint * 1)(oid)))
    for conn in connections:
        for name, number in [('identity_a', 17), ('identity_b', 2147483648), ('identity_a', 31), ('identity_b', 9223372036854775807)]:
            run(conn, name, number)
    check(close(connections[0], b'identity_a'))
    check(execute(connections[0], b'identity_a', 0, None, None, None, 0), state='26000')
    run(connections[1], 'identity_a', 43)
    for conn in connections:
        check(prepare(conn, b'bad', b'SELECT FROM', 0, None), state='42601')
        check(execute(conn, b'identity_b', 0, None, None, None, 0), state='08P01')
        run(conn, 'identity_b', 59)
        check(close(conn, b'identity_b'))
    check(close(connections[1], b'identity_a'))
finally:
    if connections and status(connections[0]) == 0:
        result = query(connections[0], b'DROP TABLE IF EXISTS public.pg_prepared_identity')
        if result:
            clear(result)
    for conn in connections:
        finish(conn)
print('two connections, named statements, repeated values, Parse/Bind errors, Sync recovery, Close and PQfinish passed', flush=True)
