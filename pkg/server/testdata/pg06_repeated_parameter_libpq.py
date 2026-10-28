# Copyright 2026 AsterSQL.
"""Task 94 repeated parameter identity differential against the fixed PostgreSQL 18.6 reference."""
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
table = f'public.pg06_repeat94_{os.getpid()}'
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
    compare(f'CREATE TABLE {table} (a INT, b BIGINT, c INT)')
    compare(f'INSERT INTO {table} VALUES (7,7,7)')
    oids = (c.c_uint * 1)(0)
    for case,(sql,identity,types) in enumerate([
        ('SELECT $1::int4,$1::int8',23,[23,20]),
        ('SELECT $1::int8,$1::int4',20,[20,23]),
        (f'UPDATE {table} SET a=$1,c=$1',23,[]),
        (f'UPDATE {table} SET a=$1,b=$1::int8',20,[]),
        (f'UPDATE {table} SET a=$1,b=$1',23,[]),
    ]):
        name = f'repeat94_{case}'.encode()
        results = [observe(prepare(conn,name,sql.encode(),1,oids)) for conn in connections]
        print('prepare',case,results,flush=True)
        assert results[0] == results[1], (sql,results)
        if case >= 3:
            assert results[0]['state'] == '42P08', results
            continue
        assert results[0]['status'] == 1, results
        for conn in connections:
            result = describe(conn,name)
            assert nparams(result) == 1 and paramtype(result,0) == identity
            description = observe(result)
            assert description == dict(status=1,state='N/A',
                                       metadata=[[oid,4 if oid == 23 else 8,-1,0] for oid in types],rows=[]), description
        import struct
        for infmt in [0,1]:
            for val in [42,-2147483648,2147483647,None]:
                data = None if val is None else (str(val).encode() if infmt == 0 else struct.pack('!i' if identity == 23 else '!q',val))
                params = (s * 1)(data)
                lengths = (i * 1)(0 if data is None else len(data))
                formats = (i * 1)(infmt)
                for outfmt in [0,1]:
                    results = [observe(prepared(conn,name,1,params,lengths,formats,outfmt)) for conn in connections]
                    assert results[0] == results[1], (sql,val,infmt,outfmt,results)
                    assert results[0]['status'] == (2 if types else 1), results
                    if types:
                        expected = [None if val is None else (str(val).encode() if outfmt == 0 else struct.pack('!i' if oid == 23 else '!q',val)).hex() for oid in types]
                        assert results[0]['rows'] == [expected], results
                        assert results[0]['metadata'] == [[oid,4 if oid == 23 else 8,-1,outfmt] for oid in types]
                    else:
                        result = compare(f'SELECT a,c FROM {table}',2)
                        expected = None if val is None else str(val).encode().hex()
                        assert result['rows'] == [[expected,expected]], result
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
    compare(f'UPDATE {table} SET a=7,b=7,c=7')
    for conn in connections:
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            # The swapped SELECT has one int8 source. Its int4 result cast
            # fails at Execute, whereas the int4-first source fails at Bind.
            data = b'2147483648'
            body = b'\0repeat94_1\0'+struct.pack('!hh',0,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0)
            send_message(wire,b'B',body)
            send_message(wire,b'H')
            assert receive(wire) == (b'2',b'')
            send_message(wire,b'E',b'\0'+struct.pack('!i',0))
            send_message(wire,b'H')
            message = receive(wire)
            assert message[0] == b'E', message
            fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
            assert fields[b'C'] == b'22003', fields
            recover(wire)
            count += 1
            for sql in [f'UPDATE {table} SET a=$1,b=$1::int8',
                        f'UPDATE {table} SET a=$1,b=$1',
                        f'INSERT INTO {table}(a,b) VALUES ($1,$1)',
                        f'INSERT INTO {table}(b,a) VALUES ($1,$1)']:
                body = b'conflict94\0'+sql.encode()+b'\0'+struct.pack('!hI',1,0)
                send_message(wire,b'P',body)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (sql,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == b'42P08', (sql,fields)
                recover(wire)
                result = observe(execute(conn,f'SELECT a,b,c FROM {table}'.encode()))
                assert result['rows'] == [['37','37','37']], result
                count += 1
            for data,state in [(b'bad','22P02'),(b'2147483648','22003')]:
                body = b'\0repeat94_0\0'+struct.pack('!hh',0,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0)
                send_message(wire,b'B',body)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', message
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state.encode(), fields
                recover(wire)
                result = observe(execute(conn,f'SELECT a,b,c FROM {table}'.encode()))
                assert result['rows'] == [['37','37','37']], result
                count += 1
    print(f'repeated_parameter_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()))
        finish(conn)
