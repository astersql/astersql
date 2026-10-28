# Copyright 2026 AsterSQL.
"""Task 78 time-to-interval differential against the fixed PostgreSQL 18.6 reference."""
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
    assert version() == server_version(ref) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
    eligibility = query(ref, 'SELECT castcontext FROM pg_cast WHERE castsource=1083 AND casttarget=1186')
    assert eligibility['rows'] == [[b'i'.hex()]]
    print('pg_cast time->interval context=i (explicit, assignment, implicit)', flush=True)
    def compare(sql, expected_status=2, state='N/A'):
        global count
        observations = [observe(execute(conn, (sql.replace('pg06_078', 'public.pg06_078') if conn == native else sql).encode())) for conn in connections]
        assert observations[0] == observations[1], (sql, observations)
        assert observations[0]['status'] == expected_status and observations[0]['state'] == state, (sql, observations)
        count += 1
        return observations[0]
    for zone in ['UTC', 'Asia/Shanghai', 'America/New_York']:
        for conn in connections:
            query(conn, f"SET time_zone = '{zone}'" if conn == native else f"SET TimeZone TO '{zone}'")
        for target in ['interval', 'interval(3)', 'interval hour to minute']:
            for conn in connections:
                result = observe(prepare(conn, b'explicit', f'SELECT $1::{target} AS v'.encode(), 1, (c.c_uint*1)(1083)))
                assert result['status'] == 1, result
            descriptions = [observe(describe(conn, b'explicit')) for conn in connections]
            assert descriptions[0] == descriptions[1], descriptions
            assert descriptions[0]['metadata'][0][0:2] == [1186, 16]
            for conn in connections:
                result = describe(conn, b'explicit')
                assert nparams(result) == 1 and paramtype(result, 0) == 1083
                clear(result)
            for text, micros in [(b'00:00:00',0),(b'12:34:56.123456',45296123456),(b'23:59:59.999999',86399999999),(b'24:00:00',86400000000),(None,0)]:
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        data = None if text is None else text if infmt == 0 else micros.to_bytes(8,'big',signed=True)
                        results = [bind(conn,b'explicit',data,infmt,outfmt) for conn in connections]
                        assert results[0] == results[1] and results[0]['status'] == 2, (zone,target,text,infmt,outfmt,results)
                        count += 1
            for conn in connections:
                clear(close_prepared(conn,b'explicit'))
        for text in ['00:00:00','12:34:56.123456','23:59:59.999999','24:00:00']:
            compare(f"SELECT ('{text}'::time)::interval AS v")
        compare('SELECT (NULL::time)::interval AS v')
        compare("SELECT ('23:59:59.999999'::time(3))::interval AS v")
        compare('SELECT (NULL::time)::interval AS v LIMIT 0')
    query(ref, 'CREATE TEMP TABLE pg06_078 (id int PRIMARY KEY, v interval(3))')
    for conn in connections:
        query(conn, 'DELETE FROM public.pg06_078' if conn == native else 'DELETE FROM pg06_078')
    for literal in ["'00:00:00'", "'12:34:56.123456'", "'23:59:59.999999'", "'24:00:00'", 'NULL']:
        compare(f'INSERT INTO pg06_078 VALUES (1,{literal}::time)',1)
        compare(f'INSERT INTO pg06_078 SELECT 2,{literal}::time',1)
        compare(f'UPDATE pg06_078 SET v={literal}::time WHERE id=1',1)
        compare('SELECT v FROM pg06_078 ORDER BY id')
        compare('DELETE FROM pg06_078',1)
    for sql,state in [("SELECT ('garbage'::time)::interval",'22007'),("SELECT ('25:00'::time)::interval",'22008'),("SELECT ('infinity'::time)::interval",'22007'),('SELECT coalesce(NULL::interval,NULL::time)','42804')]:
        compare(sql,7,state)
        compare('SELECT 1::int4')
    print(f'time_interval_differential_cases={count} passed', flush=True)
finally:
    for conn in connections:
        finish(conn)
