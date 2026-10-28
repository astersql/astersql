# Copyright 2026 AsterSQL.
"""Task 98 numeric typmod continuity differential against the fixed PostgreSQL 18.6 reference."""
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

def numeric_binary(text):
    negative = text.startswith('-')
    integer, _, fraction = text.lstrip('+-').partition('.')
    integer = integer.zfill((len(integer)+3)//4*4)
    padded = fraction.ljust((len(fraction)+3)//4*4, '0')
    digits = [int((integer+padded)[n:n+4]) for n in range(0,len(integer+padded),4)]
    weight = len(integer)//4-1
    while digits and digits[0] == 0:
        digits.pop(0)
        weight -= 1
    while digits and digits[-1] == 0:
        digits.pop()
    return struct.pack('!hhHH',len(digits),weight if digits else 0,0x4000 if negative and digits else 0,len(fraction))+b''.join(struct.pack('!H',d) for d in digits)

server_version = api('PQserverVersion', i, p)
connections = []
count = 0
table = f'typmod98_{os.getpid()}'
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
        assert [oid,size,mod,form] == [1700,-1,524294,fmt], message
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
        result = observe(execute(conn,f'CREATE TABLE {table} (v numeric(8,2))'.encode()))
        assert result['status'] == 1, result
        assert observe(execute(conn,f"INSERT INTO {table} VALUES (1.25)".encode()))['status'] == 1
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            for name, sql, parameter in [
                (b'null98','SELECT NULL::numeric(8,2)',False),
                (b'cast98','SELECT CAST($1 AS numeric(8,2))',True),
                (b'zero98',f'SELECT v FROM {table} WHERE false',False),
            ]:
                send(wire,b'P',name+b'\0'+sql.encode()+b'\0'+struct.pack('!h',0))
                assert flush(wire) == (b'1',b'')
                def statement():
                    send(wire,b'D',b'S'+name+b'\0')
                    tag, body = flush(wire)
                    assert (tag,body) == (b't',struct.pack('!hI',1,1700) if parameter else struct.pack('!h',0)), (sql,tag,body)
                    metadata(receive(wire),0)
                statement()
                cases = [('1.234','1.23'),('1.235','1.24'),('-1.235','-1.24'),('999999.994','999999.99'),('-999999.994','-999999.99'),('0','0.00'),('-0.004','0.00'),(None,None)] if parameter else [(None,None)]
                for text, expected in cases:
                    for infmt in [0,1]:
                        for outfmt in [0,1]:
                            data = None if text is None else (text.encode() if infmt == 0 else numeric_binary(text))
                            params = struct.pack('!hhh',1,infmt,1)+(struct.pack('!i',-1) if data is None else struct.pack('!i',len(data))+data) if parameter else struct.pack('!hh',0,0)
                            send(wire,b'B',b'\0'+name+b'\0'+params+struct.pack('!hh',1,outfmt))
                            assert flush(wire) == (b'2',b'')
                            send(wire,b'D',b'P\0')
                            metadata(flush(wire),outfmt)
                            statement()
                            send(wire,b'E',b'\0'+struct.pack('!i',0))
                            message = flush(wire)
                            if name == b'zero98':
                                assert message == (b'C',b'SELECT 0\0'), message
                            else:
                                value = None if expected is None else (expected.encode() if outfmt == 0 else numeric_binary(expected))
                                row = struct.pack('!hi',1,-1) if value is None else struct.pack('!hi',1,len(value))+value
                                assert message == (b'D',row), (sql,text,infmt,outfmt,message)
                                assert receive(wire) == (b'C',b'SELECT 1\0')
                            recover(wire)
                            count += 1
            # Input syntax is checked in Bind, before any row can be produced.
            for data, infmt, state in [(b'1x',0,b'22P02'),(b'',0,b'22P02'),(struct.pack('!hhhh',0,0,1,0),1,b'22P03')]:
                send(wire,b'B',b'\0cast98\0'+struct.pack('!hhh',1,infmt,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0))
                tag,body = flush(wire)
                assert tag == b'E', (tag,body)
                fields = {part[:1]:part[1:] for part in body.split(b'\0') if part}
                assert fields[b'C'] == state, fields
                recover(wire)
                count += 1
            # Typmod overflow is checked after rounding, at Execute on both servers.
            for text in ['999999.995','-999999.995','1000000']:
                for infmt in [0,1]:
                    data = text.encode() if infmt == 0 else numeric_binary(text)
                    send(wire,b'B',b'\0cast98\0'+struct.pack('!hhh',1,infmt,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0))
                    assert flush(wire) == (b'2',b'')
                    send(wire,b'D',b'P\0')
                    metadata(flush(wire),0)
                    send(wire,b'E',b'\0'+struct.pack('!i',0))
                    tag,body = flush(wire)
                    assert tag == b'E', (text,tag,body)
                    fields = {part[:1]:part[1:] for part in body.split(b'\0') if part}
                    assert fields[b'C'] == b'22003', fields
                    recover(wire)
                    count += 1
        assert observe(execute(conn,f'DROP TABLE {table}'.encode()))['status'] == 1
    print(f'numeric_typmod_continuity_cases={count} passed',flush=True)
finally:
    for conn in connections:
        observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()))
        finish(conn)
