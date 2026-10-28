# Copyright 2026 AsterSQL.
"""Task 106 portal description and encoding continuity differential against the fixed PostgreSQL 18.6 reference."""
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
assert table.startswith('pg06_106_') and table.removeprefix('pg06_106_').isdigit()
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

def parse(wire, name, text, oids=()):
    send(wire,b'P',name+b'\0'+text.encode()+b'\0'+struct.pack('!h',len(oids))+b''.join(struct.pack('!I',oid) for oid in oids))
    assert flush(wire) == (b'1',b''), text

def describe(wire, kind, name):
    send(wire,b'D',kind+name+b'\0')
    message = flush(wire)
    if kind == b'S':
        assert message[0] == b't'
        parameters = message[1]
        message = receive(wire)
        return parameters, message
    return message

def descriptor(message, formats):
    tag, body = message
    assert tag == b'T' and struct.unpack('!h',body[:2])[0] == len(formats), message
    offset = 2
    fields = []
    for fmt in formats:
        end = body.index(0,offset)
        name = body[offset:end]
        origin, attr, oid, size, modifier, actual_format = struct.unpack('!IhIhih',body[end+1:end+19])
        assert (oid,size,modifier,actual_format) == (23,4,-1,fmt), (name,oid,size,modifier,actual_format)
        fields.append((name,origin,attr,oid,size,modifier))
        offset = end+19
    assert offset == len(body)
    return fields

def bind(wire, portal, statement, number, formats, input_format=0):
    payload = None if number is None else (str(number).encode() if input_format == 0 else struct.pack('!i',number))
    argument = struct.pack('!i',-1) if payload is None else struct.pack('!i',len(payload))+payload
    send(wire,b'B',portal+b'\0'+statement+b'\0'+struct.pack('!hhh',1,input_format,1)+argument+struct.pack('!h',len(formats))+b''.join(struct.pack('!h',f) for f in formats))
    return flush(wire)

def execute_portal(wire, portal, values, formats):
    send(wire,b'E',portal+b'\0'+struct.pack('!i',0))
    message = flush(wire)
    for values_in_row in values:
        assert len(values_in_row) == len(formats)
        body = struct.pack('!h',len(formats))
        for number, fmt in zip(values_in_row,formats):
            body += data_row(number,fmt)[2:]
        assert message == (b'D',body), (portal,formats,message,body)
        message = receive(wire)
    assert message == (b'C',f'SELECT {len(values)}\0'.encode()), (portal,message)

def error_state(message, state):
    tag, body = message
    assert tag == b'E', message  # no descriptor, BindComplete or partial DataRow
    fields = {part[:1]:part[1:] for part in body.split(b'\0') if part}
    assert fields[b'C'].decode() == state, fields

try:
    for dsn in dsns:
        conn = connect(dsn.encode())
        assert conn and connected(conn) == 0, 'connection failed'
        connections.append(conn)
    assert version() == server_version(connections[0]) == 180006
    print('reference_server=180006 libpq=180006',flush=True)
    for endpoint, conn in enumerate(connections):
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            # Both live portals share a prepared identity. Describe and Execute
            # in reverse Bind order to expose accidental statement-wide formats.
            for index, (projection, zero, formats_a, formats_b) in enumerate([
                ('$1::int4 AS v',False,[0],[1]),
                ('$1::int4 AS v, $1::int4 AS w',False,[0,1],[1,0]),
                ('$1::int4 AS v, $1::int4 AS w',True,[0,1],[1,0]),
            ]):
                statement = f'portal106_{index}'.encode()
                parse(wire,statement,'SELECT '+projection+(' WHERE false' if zero else ''),(23,))
                parameters, baseline = describe(wire,b'S',statement)
                assert parameters == struct.pack('!hI',1,23)
                identity = descriptor(baseline,[0]*len(formats_a))
                assert all(field[1:3] == (0,0) for field in identity)
                for number in [2147483647,-2147483648,None]:
                    assert bind(wire,b'a',statement,number,formats_a,0) == (b'2',b'')
                    other_number = None if number is not None else 7
                    assert bind(wire,b'b',statement,other_number,formats_b,1) == (b'2',b'')
                    for portal, value_, formats in [(b'b',other_number,formats_b),(b'a',number,formats_a)]:
                        assert descriptor(describe(wire,b'P',portal),formats) == identity
                        assert describe(wire,b'S',statement) == (parameters,baseline)
                        execute_portal(wire,portal,[] if zero else [[value_]*len(formats)],formats)
                        assert descriptor(describe(wire,b'P',portal),formats) == identity
                        send(wire,b'C',b'P'+portal+b'\0')
                        assert flush(wire) == (b'3',b'')
                        count += 1
                # Invalid output count and invalid int4 text fail in Bind.
                for number, formats, state in [(1,[0,1,0],'08P01'),('invalid',[0],'22P02')]:
                    error_state(bind(wire,b'bad',statement,number,formats),state)
                    recover(wire)
                    count += 1
                send(wire,b'C',b'S'+statement+b'\0')
                assert flush(wire) == (b'3',b'')
                recover(wire)
        # Multiple shared-kernel rows, including NULL, retain each portal format;
        # Execute starts directly with DataRow, never a descriptor.
        sql(conn,f'CREATE TABLE {table} (id integer, v integer)')
        sql(conn,f'INSERT INTO {table} VALUES (1,2147483647),(2,NULL)')
        baseline_rows = sql(conn,f'SELECT v FROM {table} ORDER BY id')['rows']
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            parse(wire,b'rows',f'SELECT v FROM {table} ORDER BY id')
            parameters, baseline = describe(wire,b'S',b'rows')
            assert parameters == struct.pack('!h',0)
            identity = descriptor(baseline,[0])
            for portal, fmt in [(b'a',0),(b'b',1)]:
                send(wire,b'B',portal+b'\0rows\0'+struct.pack('!hhhh',0,0,1,fmt))
                assert flush(wire) == (b'2',b'')
            for portal, fmt in [(b'b',1),(b'a',0)]:
                assert descriptor(describe(wire,b'P',portal),[fmt]) == identity
                execute_portal(wire,portal,[[2147483647],[None]],[fmt])
                count += 1
            recover(wire)
        # Real DDL invalidates column count or type before a replacement Bind;
        # no changed descriptor or DataRow may escape a fixed result contract.
        for label, projection, ddl in [
            ('width','*',f'ALTER TABLE {table} ADD COLUMN extra integer'),
            ('type','v',f'ALTER TABLE {table} ALTER COLUMN v TYPE bigint'),
        ]:
            with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
                wire.settimeout(10)
                statement = label.encode()
                parse(wire,statement,f'SELECT {projection} FROM {table} ORDER BY id')
                parameters, baseline = describe(wire,b'S',statement)
                assert parameters == struct.pack('!h',0)
                descriptor(baseline,[0]*(2 if label == 'width' else 1))
                recover(wire)
                sql(conn,ddl)
                send(wire,b'B',b'changed\0'+statement+b'\0'+struct.pack('!hhhh',0,0,1,1))
                error_state(flush(wire),'0A000')
                recover(wire)
                count += 1
            assert sql(conn,f'SELECT v FROM {table} ORDER BY id')['rows'] == baseline_rows
        sql(conn,f'DROP TABLE {table}')
        print(f'endpoint={endpoint} portal_cases=28 passed',flush=True)
    print(f'portal_continuity_cases={count} passed',flush=True)
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
