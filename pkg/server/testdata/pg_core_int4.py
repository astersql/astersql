# Copyright 2026 AsterSQL.
"""Real libpq int4 constant/column, SQLSTATE and transaction regression."""
import ctypes as c
import ctypes.util
import os
import sys
pq = c.CDLL(os.environ.get('PG_LIBPQ_LIBRARY') or ('/opt/homebrew/opt/libpq/lib/libpq.dylib' if sys.platform == 'darwin' else ctypes.util.find_library('pq')))
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
null = api('PQgetisnull', i, p, i, i)
rows = api('PQntuples', i, p)
oid = api('PQftype', c.c_uint, p, i)
clear = api('PQclear', None, p)
error = api('PQresultErrorMessage', s, p)
field = api('PQresultErrorField', s, p, i)
tx = api('PQtransactionStatus', i, p)
version = api('PQlibVersion', i)
assert version() >= 180000
reference = bool(os.environ.get('PG_INT4_REFERENCE'))
conn = connect(os.environ.get('PG_INT4_CONNINFO', f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5').encode())
assert conn and status(conn) == 0
count = 0
def query(sql, expected=None, state=None, typed=False):
    global count
    result = execute(conn, sql.encode())
    assert result
    try:
        assert rs(result) == (7 if state else 2 if expected is not None else 1), (sql, error(result))
        if state:
            assert field(result, ord('C')) == state.encode(), (sql, error(result), field(result, ord('C')))
        if expected is not None:
            actual = [None if null(result, row, 0) else value(result, row, 0).decode() for row in range(rows(result))]
            assert actual == expected, (sql, actual, expected)
        if typed:
            assert oid(result, 0) == 23, (sql, oid(result, 0))
        count += 1
        print(sql, state or expected, flush=True)
    finally:
        clear(result)
try:
    if reference:
        query('SELECT version()', expected=[os.environ['PG_INT4_VERSION']])
    for syntax in ['{}::int4', 'CAST({} AS integer)', 'CAST({} AS int)']:
        for source, expected in [('0', '0'), ('(-2147483648)', '-2147483648'), ('2147483647', '2147483647'), ("' +00042 '", '42'), ("' -0000 '", '0'), ("'\t-12\n'", '-12'), ('NULL', None)]:
            query('SELECT ' + syntax.format(source), [expected], typed=True)
        for source, state in [('2147483648','22003'), ('9223372036854775808','22003'), ('(-2147483649)','22003'), ("'999999999999999999999999999999999999'",'22003'), ("''",'22P02'), ("'12x'",'22P02'), ("'1.0'",'22P02'), ("'+ 1'",'22P02'), ("'　1'",'22P02')]:
            query('SELECT ' + syntax.format(source), state=state)
            assert tx(conn) == 0
            query('SELECT 7', ['7'])
    table = f'public.pg_core_int4_{os.getpid()}'
    query(f'CREATE TABLE {table} (id INT PRIMARY KEY, n BIGINT, t TEXT, f REAL, d DECIMAL(8,2))')
    try:
        for key, n, text in [(1, -2147483648, ' +00042 '), (2, 2147483647, '2147483647'), (3, 2147483648, '12x')]:
            query(f"INSERT INTO {table} VALUES ({key}, {n}, '{text}', 1.0, 1.00)")
        for expr, expected in [('n::int4', ['-2147483648','2147483647']), ('CAST(t AS int4)', ['42','2147483647'])]:
            query(f'SELECT {expr} FROM {table} WHERE id < 3 ORDER BY id', expected, typed=True)
            query(f'SELECT {expr} FROM {table} WHERE id = 0', [], typed=True)
        query(f'INSERT INTO {table} VALUES (4, NULL, NULL, NULL, NULL)')
        query(f'SELECT CAST(n AS int4) FROM {table} WHERE id = 4', [None], typed=True)
        query(f'SELECT t::int4 FROM {table} WHERE id = 4', [None], typed=True)
        if not reference:
            for expr in ['f::int4', 'CAST(d AS int4)']:
                for key in [0, 1]:
                    query(f'SELECT {expr} FROM {table} WHERE id = {key}', state='0A000')
        for expr, state in [('n::int4','22003'), ('CAST(t AS int4)','22P02')]:
            query(f'SELECT {expr} FROM {table} WHERE id = 3', state=state)
            query('SELECT 7', ['7'])
    finally:
        query(f'DROP TABLE {table}')
    for sql, state in [("SELECT '12x'::int4", '22P02'), ('SELECT 2147483648::int4','22003')]:
        query('BEGIN')
        query(sql, state=state)
        assert tx(conn) == 3
        query('SELECT 7', state='25P02')
        query('ROLLBACK')
        assert tx(conn) == 0
        query('SELECT 7', ['7'])
    if not reference:
        for source in ['1.5', '1e0']:
            query(f'SELECT {source}::int4', state='0A000')
    print('libpq', version(), 'effective SQL cases', count, 'passed', flush=True)
finally:
    finish(conn)
