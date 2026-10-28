# Copyright 2026 AsterSQL.
"""Task 103 array typmod continuity differential against the fixed PostgreSQL 18.6 reference."""
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
        return dict(status=status(result), state=(ef(result, ord('C')) or b'').decode() or 'N/A', error=(ef(result, ord('M')) or b'').decode(),
                    metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))],
                    rows=[[None if null(result,r,n) else c.string_at(value(result,r,n), length(result,r,n)).hex() for n in range(cols(result))] for r in range(rows(result))])
    finally:
        clear(result)

def array_binary(parts):
    dims, elements = parts
    return (struct.pack('!iii',len(dims),int(None in elements),1043)
            + b''.join(struct.pack('!ii',*dim) for dim in dims)
            + b''.join(struct.pack('!i',-1) if v is None else struct.pack('!i',len(v.encode()))+v.encode() for v in elements))

server_version = api('PQserverVersion', i, p)
connections = []
count = 0
table = sys.argv[2]
assert table.startswith('pg06_103_') and table.removeprefix('pg06_103_').isdigit()
try:
    dsns = [os.environ['PG18_REFERENCE_DSN'], f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test']
    for dsn in dsns:
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
    def metadata(message, fmt, typmod):
        tag, body = message
        assert tag == b'T' and body[:2] == b'\x00\x01', message
        end = body.index(0,2)
        table,attr,oid,size,mod,form = struct.unpack('!IhIhih',body[end+1:])
        assert [oid,size,mod,form] == [1015,-1,typmod,fmt], message
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

    for endpoint, conn in enumerate(connections):
        if endpoint == 0:
            result = observe(execute(conn,f'CREATE TABLE {table} (v varchar(3)[])'.encode()))
            assert result['status'] == 1, result
        seed = observe(execute(conn,f"INSERT INTO {table} VALUES (NULL)".encode()))
        assert seed['status'] == 1, seed
        baseline = observe(execute(conn,f'SELECT v FROM {table}'.encode()))
        assert baseline == dict(status=2, state='N/A', error='',
                                metadata=[[1015,-1,7,0]], rows=[[None]]), baseline
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            for target_index, (target, typmod) in enumerate([('varchar(3)[]',7)]):
                for name, sql, parameter in [
                    (b'null103',f'SELECT NULL::{target}',False),
                    (b'cast103',f'SELECT CAST($1 AS {target})',True),
                    (b'zero103',f'SELECT v FROM {table} WHERE false',False),
                ]:
                    name += str(target_index).encode()
                    result_mod = 7 if name.startswith(b'zero103') else typmod
                    send(wire,b'P',name+b'\0'+sql.encode()+b'\0'+struct.pack('!h',0))
                    assert flush(wire) == (b'1',b'')
                    def statement():
                        send(wire,b'D',b'S'+name+b'\0')
                        tag, body = flush(wire)
                        assert (tag,body) == (b't',struct.pack('!hI',1,1015) if parameter else struct.pack('!h',0)), (sql,tag,body)
                        metadata(receive(wire),0,result_mod)
                    statement()
                    cases = [
                        ('{abc,abcdef,NULL}', ([(3,1)],['abc','abcdef',None]), '{abc,abc,NULL}', ([(3,1)],['abc','abc',None])),
                        ('[0:1][-2:-1]={{abcd,NULL},{界面测试,z}}', ([(2,0),(2,-2)],['abcd',None,'界面测试','z']), '[0:1][-2:-1]={{abc,NULL},{界面测,z}}', ([(2,0),(2,-2)],['abc',None,'界面测','z'])),
                        ('{}',([],[]),'{}',([],[])),
                        ('{NULL}', ([(1,1)],[None]), '{NULL}', ([(1,1)],[None])),
                        (None,None,None,None),
                    ] if parameter else [(None,None,None,None)]
                    for text, parts, expected, result_parts in cases:
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                data = None if text is None else (text.encode() if infmt == 0 else array_binary(parts))
                                params = struct.pack('!hhh',1,infmt,1)+(struct.pack('!i',-1) if data is None else struct.pack('!i',len(data))+data) if parameter else struct.pack('!hh',0,0)
                                send(wire,b'B',b'\0'+name+b'\0'+params+struct.pack('!hh',1,outfmt))
                                assert flush(wire) == (b'2',b'')
                                send(wire,b'D',b'P\0')
                                metadata(flush(wire),outfmt,result_mod)
                                statement()
                                send(wire,b'E',b'\0'+struct.pack('!i',0))
                                message = flush(wire)
                                if name.startswith(b'zero103'):
                                    assert message == (b'C',b'SELECT 0\0'), message
                                else:
                                    payload = None if expected is None else (expected.encode() if outfmt == 0 else array_binary(result_parts))
                                    row = struct.pack('!hi',1,-1) if payload is None else struct.pack('!hi',1,len(payload))+payload
                                    assert message == (b'D',row), (sql,text,infmt,outfmt,message)
                                    assert receive(wire) == (b'C',b'SELECT 1\0')
                                recover(wire)
                                count += 1
            # Malformed text dimensions fail during Bind input decoding.
            # No DataRow or completion may precede the error; Sync must recover.
            for data, infmt, state in [(b'{abc',0,b'22P02'),(b'[0:2]={a,b}',0,b'22P02'),(b'',1,b'08P01')]:
                send(wire,b'B',b'\0cast1030\0'+struct.pack('!hhh',1,infmt,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0))
                tag,body = flush(wire)
                assert tag == b'E', (tag,body)
                fields = {part[:1]:part[1:] for part in body.split(b'\0') if part}
                assert fields[b'C'] == state, fields
                recover(wire)
                count += 1
        assert observe(execute(conn,f'SELECT v FROM {table}'.encode())) == baseline
        assert observe(execute(conn,f'DROP TABLE {table}'.encode()))['status'] == 1
    print(f'array_typmod_continuity_cases={count} passed',flush=True)
finally:
    for dsn, conn in zip(dsns, connections):
        # Clean up independently of unread messages after a failed wire assertion.
        finish(conn)
        cleanup = connect(dsn.encode())
        try:
            if cleanup and connected(cleanup) == 0:
                observe(execute(cleanup,f'DROP TABLE IF EXISTS {table}'.encode()))
        finally:
            if cleanup:
                finish(cleanup)
