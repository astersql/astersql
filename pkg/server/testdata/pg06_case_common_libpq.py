# Copyright 2026 AsterSQL.
"""Task 90 CASE common typing differential against the fixed PostgreSQL 18.6 reference."""
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
    cases = [
        ('SELECT CASE WHEN true THEN 1::int4 ELSE 2::int8 END',20,8,-1,'1'),
        ('SELECT CASE WHEN false THEN 1::int8 ELSE 2::int4 END',20,8,-1,'2'),
        ('SELECT CASE WHEN true THEN 2147483647::int4 ELSE 2::int8 END',20,8,-1,'2147483647'),
        ("SELECT CASE WHEN true THEN 'a' ELSE 'b' END",25,-1,-1,'a'),
        ('SELECT CASE WHEN true THEN NULL ELSE NULL END',25,-1,-1,None),
        ('SELECT CASE WHEN false THEN 1::int4 END',23,4,-1,None),
        ("SELECT CASE WHEN true THEN '01' ELSE 2::int4 END",23,4,-1,'1'),
        ("SELECT CASE WHEN true THEN 'a'::text ELSE 'b'::varchar(3) END",1043,-1,-1,'a'),
        ("SELECT CASE WHEN false THEN 'a'::varchar(3) ELSE 'b'::text END",25,-1,-1,'b'),
        ("SELECT CASE WHEN true THEN 'a'::varchar(3) ELSE 'b'::varchar(3) END",1043,-1,7,'a'),
        ('SELECT CASE WHEN false THEN CAST(2147483648::int8 AS int4) ELSE 2::int4 END',23,4,-1,'2'),
        ('SELECT CASE WHEN true THEN 1::int4 ELSE CASE WHEN false THEN 2::int8 ELSE 3::int4 END END',20,8,-1,'1'),
        ('SELECT CASE WHEN true THEN NULL WHEN false THEN NULL ELSE 2::int4 END',23,4,-1,None),
        ('SELECT CASE WHEN true THEN x ELSE 2::int8 END FROM (SELECT 1::int4 AS x) AS t',20,8,-1,'1'),
    ]
    for idx,(sql,oid,width,typmod,expected) in enumerate(cases):
        result = compare(sql)
        assert result['metadata'] == [[oid,width,typmod,0]], (sql,result)
        assert result['rows'] == [[None if expected is None else expected.encode().hex()]], (sql,result)
        name = f'case90_{idx}'.encode()
        for conn in connections:
            assert observe(prepare(conn,name,sql.encode(),0,None))['status'] == 1
            result = describe(conn,name)
            assert nparams(result) == 0
            assert observe(result)['metadata'] == [[oid,width,typmod,0]]
        for outfmt in [0,1]:
            results = [observe(prepared(conn,name,0,None,None,None,outfmt)) for conn in connections]
            assert results[0] == results[1] and results[0]['status'] == 2, (sql,results)
            assert results[0]['metadata'] == [[oid,width,typmod,outfmt]], (sql,results)
            count += 1
    empty = compare('SELECT CASE WHEN true THEN 1::int4 ELSE 2::int8 END WHERE false')
    assert empty['metadata'] == [[20,8,-1,0]] and empty['rows'] == []
    errors = [
        ("SELECT CASE WHEN true THEN 1::int4 ELSE 'x'::text END",'42804'),
        ("SELECT CASE WHEN false THEN true ELSE 1::int4 END",'42804'),
        ("SELECT CASE WHEN true THEN NULL::text ELSE NULL::int4 END",'42804'),
        ("SELECT CASE WHEN true THEN NULL::json ELSE NULL::jsonb END",'42846'),
        ("SELECT CASE WHEN true THEN 1::int4 ELSE 'x' END",'22P02'),
        ("SELECT CASE WHEN false THEN 'x' ELSE 1::int4 END",'22P02'),
    ]
    for sql,state in errors:
        compare(sql,state)
        compare('SELECT 1::int4')
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
                send_message(wire,b'P',b'phase90\0'+sql+b'\0'+oids)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (sql,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state.encode(), (sql,fields)
                recover(wire)
                count += 1
    print(f'case_common_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
