# Copyright 2026 AsterSQL.
"""Real libpq int8 constant/column, SQLSTATE and transaction regression."""
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
reference = bool(os.environ.get('PG_INT8_REFERENCE'))
conn = connect(os.environ.get('PG_INT8_CONNINFO', f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5').encode())
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
            assert oid(result, 0) == typed, (sql, oid(result, 0))
        count += 1
        print(sql, state or expected, flush=True)
    finally:
        clear(result)
try:
    if reference:
        query('SELECT version()', expected=[os.environ['PG_INT8_VERSION']])
    for syntax in ['{}::int8', 'CAST({} AS bigint)', 'CAST({} AS int8)']:
        for source, expected in [('(-9223372036854775808)', '-9223372036854775808'), ('9223372036854775807', '9223372036854775807'), ('9007199254740993', '9007199254740993'), ("'-9223372036854775808'", '-9223372036854775808'), ("'9223372036854775807'", '9223372036854775807'), ("' +09007199254740993 '", '9007199254740993'), ('NULL', None), ('2147483647::int4', '2147483647')]:
            query('SELECT ' + syntax.format(source), [expected], typed=20)
        for source, state in [('9223372036854775808','22003'), ('(-9223372036854775809)','22003'), ("'9223372036854775808'",'22003'), ("'-9223372036854775809'",'22003'), ("'18446744073709551615'",'22003'), ("'12x'",'22P02'), ("''",'22P02'), ("'1.0'",'22P02')]:
            query('SELECT ' + syntax.format(source), state=state)
            assert tx(conn) == 0
            query('SELECT 7', ['7'])
    for source in ['2147483648::int8::int4', '2147483648::int4::int8', "CAST(CAST('2147483648' AS int8) AS int4)"]:
        query('SELECT ' + source, state='22003')
        query('SELECT 7', ['7'])
    query('SELECT 2147483647::int8::int4', ['2147483647'], typed=23)
    table = f'public.pg_core_int8_{os.getpid()}'
    query(f'CREATE TABLE {table} (id INT PRIMARY KEY, n BIGINT, t TEXT)')
    try:
        for key, n in enumerate([-9223372036854775808, 9223372036854775807, 9007199254740993], 1):
            query(f"INSERT INTO {table} VALUES ({key}, {n}, '{n}')")
        for expr in ['n::int8', 'CAST(t AS bigint)']:
            query(f'SELECT {expr} FROM {table} ORDER BY id', ['-9223372036854775808', '9223372036854775807', '9007199254740993'], typed=20)
            query(f'SELECT {expr} FROM {table} WHERE id = 0', [], typed=20)
        query(f'INSERT INTO {table} VALUES (4, NULL, NULL)')
        query(f'SELECT n::int8 FROM {table} WHERE id = 4', [None], typed=20)
        query(f'SELECT t::int8 FROM {table} WHERE id = 4', [None], typed=20)
        for source, state in [("'9223372036854775808'", '22003'), ("'-9223372036854775809'", '22003'), ("'18446744073709551615'", '22003'), ("'12x'", '22P02')]:
            query(f'UPDATE {table} SET t = {source} WHERE id = 4')
            query(f'SELECT t::int8 FROM {table} WHERE id = 4', state=state)
            query('SELECT 7', ['7'])
        query(f'SELECT n::int8::int4 FROM {table} WHERE id = 3', state='22003')
        query(f'SELECT n::int4::int8 FROM {table} WHERE id = 3', state='22003')
    finally:
        query(f'DROP TABLE {table}')
    params = api('PQexecParams', p, p, s, i, c.POINTER(c.c_uint), c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
    for text, state in [('9007199254740993', None), ('-9223372036854775808', None), ('9223372036854775807', None), ('9223372036854775808', '22003')]:
        result = params(conn, b'SELECT $1::int8 AS pg_core_int8_result', 1, (c.c_uint * 1)(25), (s * 1)(text.encode()), None, None, 0)
        try:
            assert rs(result) == (7 if state else 2), error(result)
            if state:
                assert field(result, ord('C')) == state.encode()
            else:
                assert value(result, 0, 0).decode() == text and oid(result, 0) == 20, (text, value(result, 0, 0), oid(result, 0))
            count += 1
        finally:
            clear(result)
        query('SELECT 7', ['7'])
    for sql, state in [("SELECT '12x'::int8", '22P02'), ('SELECT 9223372036854775808::int8', '22003')]:
        query('BEGIN')
        query(sql, state=state)
        assert tx(conn) == 3
        query('SELECT 7', state='25P02')
        query('ROLLBACK')
        assert tx(conn) == 0
        query('SELECT 7', ['7'])
    print('libpq', version(), 'effective SQL cases', count, 'passed', flush=True)
finally:
    finish(conn)
