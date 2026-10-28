# Copyright 2026 AsterSQL.
"""Task 88 pairwise INTERSECT common typing differential against the fixed PostgreSQL 18.6 reference."""
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
        ('SELECT 1::int4 INTERSECT SELECT 1::int8',20,8,[['31']]),
        ('SELECT 1::int8 INTERSECT SELECT 1::int4',20,8,[['31']]),
        ('SELECT 1::int4 WHERE false INTERSECT SELECT 1::int8',20,8,[]),
        ('SELECT 1::int4 INTERSECT SELECT 2::int8',20,8,[]),
        ("SELECT 'a'::text INTERSECT SELECT 'a'::varchar(3)",25,-1,[['61']]),
        ('SELECT NULL INTERSECT SELECT NULL',25,-1,[[None]]),
        ('SELECT true INTERSECT SELECT true::bool',16,1,[['74']]),
        ('SELECT NULL::date INTERSECT SELECT NULL::timestamp',1114,8,[[None]]),
        ('SELECT NULL::int4[] INTERSECT SELECT NULL::int8[]',1016,-1,[[None]]),
        ("SELECT '{1}'::int4[] INTERSECT SELECT '{1}'::int8[]",1016,-1,[['7b317d']]),
        ("SELECT '2000-01-01'::date INTERSECT SELECT '2000-01-01 00:00:00'::timestamp",1114,8,
         [[b'2000-01-01 00:00:00'.hex()]]),
        ("SELECT '01' INTERSECT SELECT 1::int4",23,4,[['31']]),
        ("SELECT 'a' INTERSECT SELECT 'a'",25,-1,[['61']]),
        ('SELECT 1::int2 INTERSECT SELECT 1::float4',700,4,[['31']]),
        ('SELECT 1::numeric INTERSECT SELECT 1::float8',701,8,[['31']]),
        ("SELECT 'a'::varchar(3) INTERSECT SELECT 'a'::text",1043,-1,[['61']]),
        ('SELECT NULL INTERSECT (SELECT NULL INTERSECT SELECT 1::int4)',23,4,[]),
        ('(SELECT NULL INTERSECT SELECT 1::int4) INTERSECT SELECT NULL',23,4,[]),
        ('SELECT 2147483647::int4 INTERSECT SELECT 2147483647::int8',20,8,[[b'2147483647'.hex()]]),
        ('SELECT NULL::int4 INTERSECT SELECT NULL::int8',20,8,[[None]]),
        ('SELECT 1::int4 INTERSECT ALL SELECT 1::int8',20,8,[['31']]),
        ("SELECT 'a'::varchar(3) INTERSECT SELECT 'a'::varchar(4)",1043,-1,[['61']]),
        ("SELECT 'a'::varchar(3) INTERSECT ALL SELECT 'a'::text",1043,-1,[['61']]),
        ('SELECT NULL::varchar(3) INTERSECT SELECT NULL',1043,-1,[[None]]),
        ('(SELECT 1::int4 UNION ALL SELECT 1::int4) INTERSECT SELECT 1::int8',20,8,[['31']]),
        ('(SELECT 1::int4 UNION ALL SELECT 1::int4) INTERSECT ALL (SELECT 1::int8 UNION ALL SELECT 1::int8)',20,8,[['31'],['31']]),
    ]:
        result = compare(sql)
        assert result['metadata'] == [[oid,width,7 if 'varchar(3)' in sql and oid == 1043 else -1,0]], (sql,result)
        assert sorted(result['rows'], key=repr) == sorted(expected,key=repr), (sql,result)
    # A NULL value cannot erase the source type's cast/operator eligibility.
    multi = compare('SELECT 1::int2, NULL INTERSECT SELECT 1::int8, NULL')
    assert multi['metadata'] == [[20,8,-1,0],[25,-1,-1,0]], multi
    assert multi['rows'] == [['31',None]], multi
    errors = [("SELECT NULL INTERSECT SELECT NULL INTERSECT SELECT 1::int4", '42804'),
              ("(SELECT NULL INTERSECT SELECT NULL) INTERSECT SELECT 1::int4", '42804'),
              ("SELECT 1::int4 INTERSECT SELECT 'x'::text", '42804'),
              ("SELECT true INTERSECT SELECT 1::int4", '42804'),
              ("SELECT '1'::text INTERSECT SELECT 1::int4", '42804'),
              ("SELECT NULL::text INTERSECT SELECT NULL::int4 WHERE false", '42804'),
              ("SELECT NULL::json INTERSECT SELECT NULL::jsonb", '42846'),
              ("SELECT 'x' INTERSECT SELECT 1::int4", '22P02'),
              ("SELECT 1::int4 INTERSECT SELECT 'x'", '22P02')]
    for sql, state in errors:
        compare(sql,state)
        compare('SELECT 1::int4')
    # Statement Describe reports the common modifier; the planned portal
    # retains the uncoerced left projection modifier in both result formats.
    for name, sql, oid, width, typmod in [
        (b'null88', b"SELECT NULL INTERSECT SELECT NULL",25,-1,-1),
        (b'intersect88', b"SELECT NULL::int4 INTERSECT SELECT NULL::int8",20,8,-1),
        (b'wide88', b"SELECT 1::int4 INTERSECT SELECT 1::int8",20,8,-1),
        (b'varchar88', b"SELECT 'a'::varchar(3) INTERSECT SELECT 'a'::text",1043,-1,-1)]:
        for conn in connections:
            assert observe(prepare(conn,name,sql,0,None))['status'] == 1
            result = describe(conn,name)
            assert nparams(result) == 0
            assert observe(result)['metadata'] == [[oid,width,typmod,0]]
        for outfmt in [0,1]:
            results = [observe(prepared(conn,name,0,None,None,None,outfmt)) for conn in connections]
            assert results[0] == results[1] and results[0]['status'] == 2, (sql,results)
            expected_rows = [[None]]
            if name == b'varchar88':
                expected_rows = [['61']]
            if name == b'wide88':
                expected_rows = [['0000000000000001']] if outfmt else [['31']]
            assert results[0]['rows'] == sorted(expected_rows,key=repr), (sql,results)
            execution_typmod = 7 if name == b'varchar88' else typmod
            assert results[0]['metadata'] == [[oid,width,execution_typmod,outfmt]]
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
                send_message(wire,b'P',b'phase88\0'+sql+b'\0'+oids)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (sql,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state.encode(), (sql,fields)
                recover(wire)
                count += 1
    print(f'intersect_common_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
