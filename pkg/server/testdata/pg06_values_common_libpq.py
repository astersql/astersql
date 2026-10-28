# Copyright 2026 AsterSQL.
"""Task 91 VALUES common typing differential against the fixed PostgreSQL 18.6 reference."""
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
                    rows=[[None if null(result,r,n) else c.string_at(value(result,r,n), length(result,r,n)).hex() for n in range(cols(result))] for r in range(rows(result))])
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
        ("VALUES (1::int4,'a'),(2::int8,NULL)", [[20,8,-1,0],[25,-1,-1,0]], [['1','a'],['2',None]]),
        ("VALUES (NULL,NULL),(NULL,NULL)", [[25,-1,-1,0],[25,-1,-1,0]], [[None,None],[None,None]]),
        ("VALUES (NULL,1::int4),(2::int8,NULL)", [[20,8,-1,0],[23,4,-1,0]], [[None,'1'],['2',None]]),
        ("VALUES (2147483647::int4,'a'),(9223372036854775807::int8,'b')", [[20,8,-1,0],[25,-1,-1,0]], [['2147483647','a'],['9223372036854775807','b']]),
        ("VALUES ('01',true),(2::int4,false)", [[23,4,-1,0],[16,1,-1,0]], [['1','t'],['2','f']]),
        ("VALUES ('a'::varchar(3)),('b'::varchar(3))", [[1043,-1,7,0]], [['a'],['b']]),
        ("VALUES ('a'::varchar(3)),('b'::varchar(4))", [[1043,-1,-1,0]], [['a'],['b']]),
        ("VALUES (NULL),(NULL),(1::int4)", [[23,4,-1,0]], [[None],[None],['1']]),
        ("VALUES (1::int4,'a'),(2::int8,NULL) LIMIT 0", [[20,8,-1,0],[25,-1,-1,0]], []),
        ("VALUES (1::int4,'a'),(2::int8,'b') ORDER BY 1 DESC LIMIT 1", [[20,8,-1,0],[25,-1,-1,0]], [['2','b']]),
        ("/* leading */ VALUES (1::int4, 'a,(b)'), /* row */ (2::int8, NULL)", [[20,8,-1,0],[25,-1,-1,0]], [['1','a,(b)'],['2',None]]),
    ]
    for idx,(sql,metadata,expected) in enumerate(cases):
        result = compare(sql)
        assert result['metadata'] == metadata, (sql,result)
        assert result['rows'] == [[None if v is None else v.encode().hex() for v in row] for row in expected], (sql,result)
        name = f'values91_{idx}'.encode()
        for conn in connections:
            assert observe(prepare(conn,name,sql.encode(),0,None))['status'] == 1
            result = describe(conn,name)
            assert nparams(result) == 0
            assert observe(result)['metadata'] == metadata
        for outfmt in [0,1]:
            results = [observe(prepared(conn,name,0,None,None,None,outfmt)) for conn in connections]
            assert results[0] == results[1] and results[0]['status'] == 2, (sql,results)
            assert results[0]['metadata'] == [field[:3]+[outfmt] for field in metadata], (sql,results)
            count += 1
    paramtype = api('PQparamtype', c.c_uint, p, i)
    parameter_sql = b"VALUES ($1::int4,$2::text),($1::int8,NULL)"
    parameter_oids = (c.c_uint * 2)(23,25)
    for conn in connections:
        result = observe(prepare(conn,b'values91_parameters',parameter_sql,2,parameter_oids))
        assert result['status'] == 1, result
        result = describe(conn,b'values91_parameters')
        assert nparams(result) == 2
        assert [paramtype(result,n) for n in range(2)] == [23,25]
        assert observe(result)['metadata'] == [[20,8,-1,0],[25,-1,-1,0]]
    parameters = (s * 2)(b'7',b'hello')
    for outfmt in [0,1]:
        results = [observe(prepared(conn,b'values91_parameters',2,parameters,None,None,outfmt)) for conn in connections]
        assert results[0] == results[1] and results[0]['status'] == 2, results
        count += 1
    errors = [
        ("VALUES (1::int4,'a'),(true,'b')",'42804'),
        ("VALUES (1::int4,'a'::text),(2::int8,3::int4)",'42804'),
        ("VALUES (1::int4),('x')",'22P02'),
        ("VALUES (NULL::text),(NULL::int4)",'42804'),
        ("VALUES (1,2),(3)",'42601'),
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
                send_message(wire,b'P',b'phase91\0'+sql+b'\0'+oids)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (sql,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state.encode(), (sql,fields)
                recover(wire)
                count += 1
    print(f'values_common_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
