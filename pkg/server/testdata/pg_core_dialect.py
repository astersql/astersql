# Copyright 2026 AsterSQL.
"""Real libpq simple-query lifecycle; stdin barriers interleave MySQL SQL."""
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
        if expected is not None:
            assert value(result, 0, 0).decode() == expected, sql
    finally:
        clear(result)

def barrier(n):
    print('stage', n, flush=True)
    assert sys.stdin.readline() == 'continue\n'
for generation in range(2):
    conn = connect(f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5'.encode())
    assert conn
    try:
        assert status(conn) == 0
        query(conn, 'SELECT current_schema()', 'public')
        for n, command in enumerate(['SET search_path TO pg_catalog', 'RESET search_path', 'BEGIN', 'ROLLBACK']):
            query(conn, command)
            if n == 0:
                query(conn, 'SHOW search_path', 'pg_catalog')
            if n == 1:
                query(conn, 'SELECT current_schema()', 'public')
            query(conn, 'SELECT 271828 AS pg_core_pg_identity', '271828')
            barrier(generation * 4 + n)
        query(conn, 'SELECT * FROM public.pg_core_dialect_missing', state='42P01')
        query(conn, 'SELECT 271828 AS pg_core_pg_identity', '271828')
    finally:
        finish(conn)
print('two connections, SET/RESET, transactions, error recovery and PQfinish passed', flush=True)
