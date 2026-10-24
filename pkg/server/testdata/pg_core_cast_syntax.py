# Copyright 2026 AsterSQL.
"""Real libpq regression for the integer cast parser boundary."""
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
execute = api('PQexec', p, p, s)
rs = api('PQresultStatus', i, p)
value = api('PQgetvalue', s, p, i, i)
clear = api('PQclear', None, p)
error = api('PQresultErrorMessage', s, p)
field = api('PQresultErrorField', s, p, i)
version = api('PQlibVersion', i)
assert version() >= 180000
print('libpq', version(), flush=True)
def query(conn, sql, expected=None, state=None):
    result = execute(conn, sql.encode())
    assert result
    try:
        assert rs(result) == (7 if state else 2 if expected is not None else 1), (sql, error(result))
        if state:
            assert field(result, ord('C')) == state.encode(), (sql, error(result))
            position = field(result, ord('P'))
            if position and sql.endswith('::'):
                assert int(position) == len(sql) + 1, (sql, position)
        if expected is not None:
            assert value(result, 0, 0).decode() == expected, sql
    finally:
        clear(result)

conn = connect(f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5'.encode())
assert conn
try:
    assert status(conn) == 0
    for sql in ['SELECT 1::int4::int8', 'SELECT ((1)::int4)::int8', 'SELECT (1+2)::int4', 'SELECT 1+2::int4', 'SELECT CAST(1 AS int4)', 'SELECT CAST(1 AS integer)', 'SELECT CAST(1 AS int)', 'SELECT CAST(1 AS int8)', 'SELECT CAST(1 AS bigint)']:
        query(conn, sql, '3' if '1+2' in sql else '1')
    query(conn, "SELECT '::int4' /* ::int8 */", '::int4')
    invalid = ['SELECT 1::', "SELECT '中', 1::"]
    if not os.environ.get('PG_CAST_REFERENCE'):
        invalid += ['SELECT 1::text', 'SELECT CAST(1 AS int2)']
    for sql in invalid:
        query(conn, sql, state='42601')
        query(conn, 'SELECT 7', '7')
finally:
    finish(conn)
print('integer cast grammar, command gate, errors, recovery and PQfinish passed', flush=True)
