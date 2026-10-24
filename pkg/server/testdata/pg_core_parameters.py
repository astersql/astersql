# Copyright 2026 AsterSQL.
"""Exercise inferred integer parameter types with real libpq and SQL execution."""
import ctypes as c
import ctypes.util
import os
import sys

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
field = api('PQresultErrorField', s, p, i)
value = api('PQgetvalue', s, p, i, i)
null = api('PQgetisnull', i, p, i, i)
rows = api('PQntuples', i, p)
cols = api('PQnfields', i, p)
oid = api('PQftype', u, p, i)
execute = api('PQexec', p, p, s)
prepare = api('PQprepare', p, p, s, s, i, c.POINTER(u))
describe = api('PQdescribePrepared', p, p, s)
close_prepared = api('PQclosePrepared', p, p, s)
nparams = api('PQnparams', i, p)
ptype = api('PQparamtype', u, p, i)
bind = api('PQexecPrepared', p, p, s, i, c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
version = api('PQlibVersion', i)
assert version() >= 180000
conn = connect(f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5'.encode())
assert conn and status(conn) == 0
count = 0
def check(result, expected_status, state=None):
    assert result
    assert rs(result) == expected_status, error(result)
    if state:
        assert field(result, ord('C')) == state.encode(), (state, error(result))
def sql(text, expected_status=1):
    result = execute(conn, text.encode())
    try:
        check(result, expected_status)
        if expected_status == 2:
            print(text, value(result, 0, 0).decode(), flush=True)
    finally:
        clear(result)
try:
    if os.environ.get('PG_PARAMETERS_REFERENCE'):
        sql('SELECT version()', 2)
    cases = [
        ('SELECT ($1)::int4', [], [23], [23], [([' +017 '], ['17']), (['-2147483648'], ['-2147483648']), (['2147483647'], ['2147483647']), (['-2147483649'], '22003'), ([None], [None]), (['12x'], '22P02'), (['2147483648'], '22003'), (['999999999999999999999x'], '22003')]),
        ('SELECT CAST($1 AS bigint)', [0], [20], [20], [(['9007199254740993'], ['9007199254740993']), (['-9223372036854775808'], ['-9223372036854775808']), (['9223372036854775807'], ['9223372036854775807']), ([None], [None]), (['9223372036854775808'], '22003'), (['-9223372036854775809'], '22003'), (['12x'], '22P02')]),
        ('SELECT $2::int4 AS b, $1::int8 AS a, $2::int4 AS c', [], [20, 23], [23, 20, 23], [(['9007199254740993', '17'], ['17', '9007199254740993', '17']), ([None, '19'], ['19', None, '19']), (['31', None], [None, '31', None]), ([None, None], [None, None, None])]),
        ('SELECT $1::int4', [20], [20], [23], [(['17'], ['17']), (['2147483648'], '22003'), ([None], [None])]),
        ('SELECT $1::int4', [25], [25], [23], [(['17'], ['17']), (['12x'], '22P02'), (['2147483648'], '22003')]),
        ('SELECT $1::int4, $1::int8', [], [23], [23, 20], [(['17'], ['17', '17']), (['2147483648'], '22003')]),
        ('SELECT $1::int8, $1::int4', [], [20], [20, 23], [(['17'], ['17', '17']), (['2147483648'], '22003')]),
        ('SELECT $1::int8::int4', [], [20], [23], [(['17'], ['17']), (['2147483648'], '22003')]),
    ]
    text, _, inferred, outputs, executions = cases[2]
    cases.append((text, [20, 0], inferred, outputs, executions))
    for index, (text, supplied, inferred, outputs, executions) in enumerate(cases):
        name = f'pg_core_parameter_{index}'.encode()
        result = prepare(conn, name, text.encode(), len(supplied), (u * len(supplied))(*supplied) if supplied else None)
        try:
            check(result, 1)
        finally:
            clear(result)
        result = describe(conn, name)
        try:
            check(result, 1)
            assert nparams(result) == len(inferred)
            assert [ptype(result, j) for j in range(nparams(result))] == inferred
            assert [oid(result, j) for j in range(cols(result))] == outputs
        finally:
            clear(result)
        for inputs, expected in executions:
            result = bind(conn, name, len(inputs), (s * len(inputs))(*[None if v is None else v.encode() for v in inputs]), None, None, 0)
            try:
                check(result, 7 if isinstance(expected, str) else 2, expected if isinstance(expected, str) else None)
                if not isinstance(expected, str):
                    assert rows(result) == 1
                    assert [None if null(result, 0, j) else value(result, 0, j).decode() for j in range(cols(result))] == expected
                    assert [oid(result, j) for j in range(cols(result))] == outputs
                count += 1
                print(text, inputs, expected, flush=True)
            finally:
                clear(result)
            sql('SELECT 7', 2)
        result = close_prepared(conn, name)
        try:
            check(result, 1)
        finally:
            clear(result)
    print('libpq', version(), 'effective parameter executions', count, 'passed', flush=True)
finally:
    finish(conn)
