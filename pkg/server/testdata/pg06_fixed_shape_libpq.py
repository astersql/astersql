# Copyright 2026 AsterSQL.
"""Task 105 fixed prepared result shape differential against the fixed PostgreSQL 18.6 reference."""
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

import socket
import struct

server_version = api('PQserverVersion', i, p)
socket_fd = api('PQsocket', i, p)
connections = []
table = sys.argv[2]
assert table.startswith('pg06_105_') and table.removeprefix('pg06_105_').isdigit()
dsns = [os.environ['PG18_REFERENCE_DSN'], f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test']
count = 0

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

def data_row(number, fmt):
    payload = None if number is None else (str(number).encode() if fmt == 0 else struct.pack('!i',number))
    return struct.pack('!hi',1,-1) if payload is None else struct.pack('!hi',1,len(payload))+payload

def recover(wire):
    send(wire,b'S')
    assert receive(wire) == (b'Z',b'I')
    send(wire,b'Q',b'SELECT 1::int4\0')
    # Recovery column label differs between the endpoints; its type/value do not.
    tag, body = receive(wire)
    assert tag == b'T' and struct.unpack('!Ihih',body[-12:]) == (23,4,-1,0)
    assert receive(wire) == (b'D',data_row(1,0))
    assert receive(wire) == (b'C',b'SELECT 1\0')
    assert receive(wire) == (b'Z',b'I')

def sql(conn, text):
    result = observe(execute(conn,text.encode()))
    assert result['status'] in (1,2), (text,result)
    return result

def scalar(conn, text):
    return int(bytes.fromhex(sql(conn,text)['rows'][0][0]))

try:
    for dsn in dsns:
        conn = connect(dsn.encode())
        assert conn and connected(conn) == 0, 'connection failed'
        connections.append(conn)
    assert version() == server_version(connections[0]) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
    cases = [
        ('unchanged', 'integer', 'v', f'INSERT INTO {table} VALUES (3,9)', None, (23,4,-1,0)),
        ('same_shape', 'integer', 'v', f'ALTER TABLE {table} ADD COLUMN extra integer', None, (23,4,-1,0)),
        ('type', 'integer', 'v', f'ALTER TABLE {table} ALTER COLUMN v TYPE bigint', '0A000', (23,4,-1,0)),
        ('typmod', 'varchar(3)', 'v', f'ALTER TABLE {table} ALTER COLUMN v TYPE varchar(4)', '0A000', (1043,-1,7,0)),
        ('count', 'integer', '*', f'ALTER TABLE {table} ADD COLUMN extra integer', '0A000', (23,4,-1,0)),
        ('missing_table', 'integer', 'v', f'DROP TABLE {table}', '42P01', (23,4,-1,0)),
        ('missing_column', 'integer', 'v', f'ALTER TABLE {table} DROP COLUMN v', '42703', (23,4,-1,0)),
    ]
    for endpoint, conn in enumerate(connections):
        other = connect(dsns[endpoint].encode())
        assert other and connected(other) == 0
        try:
            with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
                wire.settimeout(10)
                for label, declaration, projection, ddl, state, expected in cases:
                    sql(other,f'CREATE TABLE {table} (id integer, v {declaration})')
                    literal = "'abc'" if declaration.startswith('varchar') else '2147483647'
                    sql(other,f'INSERT INTO {table} VALUES (1,{literal}),(2,NULL)')
                    text = f'SELECT {projection} FROM {table} ORDER BY id'
                    name = label.encode()
                    send(wire,b'P',name+b'\0'+text.encode()+b'\0'+struct.pack('!h',0))
                    assert flush(wire) == (b'1',b''), label
                    send(wire,b'D',b'S'+name+b'\0')
                    assert flush(wire) == (b't',struct.pack('!h',0))
                    initial = receive(wire)
                    assert initial[0] == b'T', initial
                    assert struct.unpack('!Ihih',initial[1][-12:]) == expected
                    assert struct.unpack('!h', initial[1][:2])[0] == (2 if projection == '*' else 1)
                    recover(wire)
                    sql(other,ddl)
                    # Both Describe and Bind must reanalyze before emitting any
                    # replacement descriptor, BindComplete or DataRow.
                    for phase in ['describe','bind']:
                        if phase == 'describe':
                            send(wire,b'D',b'S'+name+b'\0')
                            assert flush(wire) == (b't',struct.pack('!h',0))
                            message = receive(wire)
                        else:
                            send(wire,b'B',b'\0'+name+b'\0'+struct.pack('!hhh',0,0,0))
                            message = flush(wire)
                        if state:
                            tag, body = message
                            assert tag == b'E', (endpoint,label,phase,message)
                            fields = {part[:1]:part[1:] for part in body.split(b'\0') if part}
                            assert fields[b'C'].decode() == state, (endpoint,label,phase,fields)
                            if state == '0A000':
                                assert fields[b'M'] == b'cached plan must not change result type', fields
                        elif phase == 'describe':
                            assert message == initial, (label,message,initial)
                        else:
                            assert message == (b'2',b''), message
                            send(wire,b'E',b'\0'+struct.pack('!i',0))
                            assert flush(wire) == (b'D',data_row(2147483647,0))
                            assert receive(wire) == (b'D',data_row(None,0))
                            if label == 'unchanged':
                                assert receive(wire) == (b'D',data_row(9,0))
                            assert receive(wire) == (b'C', b'SELECT 3\0' if label == 'unchanged' else b'SELECT 2\0')
                        recover(wire)
                        count += 1
                    if state is None:
                        send(wire,b'B',b'\0'+name+b'\0'+struct.pack('!hhhh',0,0,1,1))
                        assert flush(wire) == (b'2',b'')
                        send(wire,b'D',b'P\0')
                        assert flush(wire) == (b'T',initial[1][:-2]+struct.pack('!h',1))
                        send(wire,b'E',b'\0'+struct.pack('!i',0))
                        assert flush(wire) == (b'D',data_row(2147483647,1))
                        assert receive(wire) == (b'D',data_row(None,1))
                        if label == 'unchanged':
                            assert receive(wire) == (b'D',data_row(9,1))
                        assert receive(wire) == (b'C', b'SELECT 3\0' if label == 'unchanged' else b'SELECT 2\0')
                        recover(wire)
                        count += 1
                    if label != 'missing_table':
                        # Failed analysis does not write to the table.
                        assert scalar(other,f'SELECT count(*) FROM {table}') == (3 if label == 'unchanged' else 2)
                    send(wire,b'C',b'S'+name+b'\0')
                    assert flush(wire) == (b'3',b'')
                    recover(wire)
                    sql(other,f'DROP TABLE IF EXISTS {table}')
                    print(f'endpoint={endpoint} case={label} describe/bind passed',flush=True)
        finally:
            finish(other)
    print(f'fixed_shape_cases={count} passed',flush=True)
finally:
    for dsn, conn in zip(dsns,connections):
        finish(conn)
        cleanup = connect(dsn.encode())
        try:
            if cleanup and connected(cleanup) == 0:
                sql(cleanup,f'DROP TABLE IF EXISTS {table}')
        finally:
            if cleanup:
                finish(cleanup)
