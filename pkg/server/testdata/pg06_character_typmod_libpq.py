# Copyright 2026 AsterSQL.
"""Task 97 character typmod continuity differential against the fixed PostgreSQL 18.6 reference."""
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
table = f'typmod97_{os.getpid()}'
try:
    for dsn in [os.environ['PG18_REFERENCE_DSN'], f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test']:
        conn = connect(dsn.encode())
        assert conn and connected(conn) == 0, 'connection failed'
        connections.append(conn)
    assert version() == server_version(connections[0]) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
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
        size = struct.unpack('!I', exact(4))[0]
        return tag, exact(size-4)
    def send(wire, tag, body=b''):
        wire.sendall(tag+struct.pack('!I',len(body)+4)+body)
    def flush(wire):
        send(wire,b'H')
        return receive(wire)
    def metadata(message, fmt):
        tag, body = message
        assert tag == b'T' and body[:2] == b'\x00\x01', message
        end = body.index(0,2)
        table,attr,oid,size,mod,form = struct.unpack('!IhIhih',body[end+1:])
        assert [oid,size,mod,form] == [1043,-1,7,fmt], message
    def recover(wire):
        send(wire,b'S')
        assert receive(wire) == (b'Z',b'I')
        send(wire,b'Q',b'SELECT 1::int4\0')
        messages = []
        while True:
            message = receive(wire)
            messages.append(message)
            if message[0] == b'Z':
                break
        assert [m[0] for m in messages] == [b'T',b'D',b'C',b'Z']
        assert messages[1][1] == struct.pack('!hi',1,1)+b'1'
    for conn in connections:
        # A uniquely named fixture on both servers. Describe must not depend
        # on executing the original zero-row query to discover its type.
        result = observe(execute(conn,f'CREATE TABLE {table} (v varchar(3))'.encode()))
        assert result['status'] == 1, result
        assert observe(execute(conn,f"INSERT INTO {table} VALUES ('中文a')".encode()))['status'] == 1
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            for name, sql, parameter in [
                (b'null97','SELECT NULL::varchar(3)',False),
                (b'cast97','SELECT CAST($1 AS varchar(3))',True),
                (b'zero97',f'SELECT v FROM {table} WHERE false',False),
            ]:
                send(wire,b'P',name+b'\0'+sql.encode()+b'\0'+struct.pack('!h',0))
                assert flush(wire) == (b'1',b'')
                def statement():
                    send(wire,b'D',b'S'+name+b'\0')
                    tag, body = flush(wire)
                    assert (tag,body) == (b't',struct.pack('!hI',1,1043) if parameter else struct.pack('!h',0)), (sql,tag,body)
                    metadata(receive(wire),0)
                statement()
                cases = [('中文','中文'),('中文a','中文a'),('中文ab','中文a'),('😀é中x','😀é中'),('éé😀','éé'),('',''),(None,None)] if parameter else [(None,None)]
                for text, expected in cases:
                    for infmt in [0,1]:
                        for outfmt in [0,1]:
                            data = None if text is None else text.encode()
                            params = struct.pack('!hhh',1,infmt,1)+(struct.pack('!i',-1) if data is None else struct.pack('!i',len(data))+data) if parameter else struct.pack('!hh',0,0)
                            send(wire,b'B',b'\0'+name+b'\0'+params+struct.pack('!hh',1,outfmt))
                            assert flush(wire) == (b'2',b'')
                            send(wire,b'D',b'P\0')
                            metadata(flush(wire),outfmt)
                            statement()
                            send(wire,b'E',b'\0'+struct.pack('!i',0))
                            message = flush(wire)
                            if name == b'zero97':
                                assert message == (b'C',b'SELECT 0\0'), message
                            else:
                                value = None if expected is None else expected.encode()
                                row = struct.pack('!hi',1,-1) if value is None else struct.pack('!hi',1,len(value))+value
                                assert message == (b'D',row), (sql,text,infmt,outfmt,message)
                                assert receive(wire) == (b'C',b'SELECT 1\0')
                            recover(wire)
                            count += 1
            # Invalid UTF8 is rejected in Bind, before any partial result.
            for infmt in [0,1]:
                for data in [b'\xff',b'\xc3',b'a\0b']:
                    send(wire,b'B',b'\0cast97\0'+struct.pack('!hhh',1,infmt,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0))
                    tag,body = flush(wire)
                    assert tag == b'E', (tag,body)
                    fields = {part[:1]:part[1:] for part in body.split(b'\0') if part}
                    assert fields[b'C'] == b'22021', fields
                    recover(wire)
                    count += 1
        assert observe(execute(conn,f'DROP TABLE {table}'.encode()))['status'] == 1
    print(f'character_typmod_continuity_cases={count} passed',flush=True)
finally:
    for conn in connections:
        observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()))
        finish(conn)
