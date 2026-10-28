# Copyright 2026 AsterSQL.
"""Task 96 indeterminate parameter diagnostics differential against the fixed PostgreSQL 18.6 reference."""
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
    # Flush after Parse locates the diagnostic before Bind/Execute. No rows,
    # descriptions or completion messages may precede the ErrorResponse.
    for conn in connections:
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            for sql, oids, state in [
                ('SELECT $1 IS NULL', [], '42P18'),
                ('SELECT $1 IS NULL', [0], '42P18'),
                ('SELECT $1 IS NOT NULL', [0], '42P18'),
                ('SELECT $2 IS NULL', [23, 0], '42P18'),
                ('SELECT true::bytea', [], '42846'),
                ('SELECT 1::int4 UNION SELECT true', [], '42804'),
            ]:
                body = b'\0' + sql.encode() + b'\0' + struct.pack('!h',len(oids))
                body += b''.join(struct.pack('!I',oid) for oid in oids)
                send_message(wire,b'P',body)
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (sql,'Parse',message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state.encode(), (sql,'Parse',fields)
                recover(wire)
                count += 1
    for sql, identity, supplied in [
        ('SELECT $1::int4',23,0),
        ('SELECT $1 IS NULL',16,23),
        ('SELECT $1::int4 IS NULL',16,0),
    ]:
        name = f'typed96_{identity}_{supplied}'.encode()
        for conn in connections:
            assert observe(prepare(conn,name,sql.encode(),1,(c.c_uint * 1)(supplied))) == dict(status=1,state='N/A',metadata=[],rows=[])
            result = describe(conn,name)
            assert nparams(result) == 1 and paramtype(result,0) == 23
            observed = observe(result)
            assert observed == dict(status=1,state='N/A',metadata=[[identity,4 if identity == 23 else 1,-1,0]],rows=[]), (sql, conn == connections[0], observed)
        for val in [0,-2147483648,2147483647,None]:
            for infmt in [0,1]:
                data = None if val is None else (str(val).encode() if infmt == 0 else struct.pack('!i',val))
                params, lengths, formats = (s * 1)(data), (i * 1)(0 if data is None else len(data)), (i * 1)(infmt)
                for outfmt in [0,1]:
                    results = [observe(prepared(conn,name,1,params,lengths,formats,outfmt)) for conn in connections]
                    if identity == 23:
                        expected = None if val is None else (str(val).encode() if outfmt == 0 else struct.pack('!i',val)).hex()
                    else:
                        expected = (b't' if val is None else b'f') if outfmt == 0 else (b'\x01' if val is None else b'\x00')
                        expected = expected.hex()
                    assert results[0] == results[1] == dict(status=2,state='N/A',metadata=[[identity,4 if identity == 23 else 1,-1,outfmt]],rows=[[expected]]), (sql,val,infmt,outfmt,results)
                    count += 1
    print(f'indeterminate_parameter_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
