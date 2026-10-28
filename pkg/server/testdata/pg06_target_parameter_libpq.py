# Copyright 2026 AsterSQL.
"""Task 93 target-column parameter typing differential against the fixed PostgreSQL 18.6 reference."""
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
paramtype = api('PQparamtype', c.c_uint, p, i)
connections = []
count = 0
table = f'public.pg06_target93_{os.getpid()}'
try:
    for dsn in [os.environ['PG18_REFERENCE_DSN'], f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test']:
        conn = connect(dsn.encode())
        assert conn and connected(conn) == 0, 'connection failed'
        connections.append(conn)
    assert version() == server_version(connections[0]) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
    def compare(sql, expected_status=1):
        results = [observe(execute(conn, sql.encode())) for conn in connections]
        assert results[0] == results[1], (sql,results)
        assert results[0]['status'] == expected_status, (sql,results)
        return results[0]
    compare(f'CREATE TABLE {table} (v INT)')
    compare(f'INSERT INTO {table} VALUES (7)')
    oids = (c.c_uint * 1)(0)
    for kind,sql in [('insert',f'INSERT INTO {table}(v) VALUES ($1)'),
                     ('update',f'UPDATE {table} SET v=$1')]:
        name = f'target93_{kind}'.encode()
        for conn in connections:
            result = observe(prepare(conn,name,sql.encode(),1,oids))
            assert result['status'] == 1, (kind,result)
            result = describe(conn,name)
            assert nparams(result) == 1
            identity = paramtype(result,0)
            description = observe(result)
            assert identity == 23, (kind,identity,description)
            # DML without RETURNING has NoData; typlen/typmod/format do not apply.
            assert description == dict(status=1,state='N/A',metadata=[],rows=[]), description
        assert compare(f'SELECT v FROM {table}',2)['rows'] == [['37']]
        for infmt in [0,1]:
            for val in [42,-2147483648,2147483647,None]:
                import struct
                data = None if val is None else (str(val).encode() if infmt == 0 else struct.pack('!i',val))
                params = (s * 1)(data)
                lengths = (i * 1)(0 if data is None else len(data))
                formats = (i * 1)(infmt)
                for outfmt in [0,1]:
                    if kind == 'insert':
                        compare(f'DELETE FROM {table}')
                    results = [observe(prepared(conn,name,1,params,lengths,formats,outfmt)) for conn in connections]
                    assert results[0] == results[1] == dict(status=1,state='N/A',metadata=[],rows=[]), results
                    result = compare(f'SELECT v FROM {table}',2)
                    expected = None if val is None else str(val).encode().hex()
                    assert result['rows'] == [[expected]], result
                    assert result['metadata'] == [[23,4,-1,0]], result
                    count += 1
                    compare(f'DELETE FROM {table}')
                    compare(f'INSERT INTO {table} VALUES (7)')
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
            for kind in ['insert','update']:
                for data,state in [(b'bad','22P02'),(b'2147483648','22003'),(b'-2147483649','22003')]:
                    # Only Bind+Flush is sent: an error here proves input decoding
                    # uses Parse's int4 identity before any Execute side effect.
                    body = b'\0'+f'target93_{kind}'.encode()+b'\0'+struct.pack('!hh',0,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0)
                    send_message(wire,b'B',body)
                    send_message(wire,b'H')
                    message = receive(wire)
                    assert message[0] == b'E', (kind,data,message)
                    fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                    assert fields[b'C'] == state.encode(), (kind,data,fields)
                    recover(wire)
                    result = observe(execute(conn,f'SELECT v FROM {table}'.encode()))
                    assert result['rows'] == [['37']], result
                    count += 1
    print(f'target_parameter_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()))
        finish(conn)
