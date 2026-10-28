# Copyright 2026 AsterSQL.
"""Task 104 base column provenance differential against the fixed PostgreSQL 18.6 reference."""
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
assert table.startswith('pg06_104_') and table.removeprefix('pg06_104_').isdigit()
tables = [table, table + '_other']
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

def metadata(message, name, origin, fmt):
    tag, body = message
    assert tag == b'T' and body[:2] == b'\x00\x01', message
    end = body.index(0,2)
    assert body[2:end].decode() == name, message
    fields = struct.unpack('!IhIhih',body[end+1:])
    assert fields == (*origin,23,4,-1,fmt), (name,fields,origin,fmt)

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
    for endpoint, conn in enumerate(connections):
        for name in tables:
            sql(conn,f'CREATE TABLE {name} (id integer, v integer)')
            sql(conn,f'INSERT INTO {name} VALUES (1,7),(2,NULL)')
        # Compare each endpoint with its own real catalog identity, never the
        # other server's numeric OID or a value inferred from a result alias.
        origins = []
        for name in tables:
            oid = scalar(conn,f"SELECT oid FROM pg_catalog.pg_class WHERE relname = '{name}'")
            attr = scalar(conn,f"SELECT attnum FROM pg_catalog.pg_attribute WHERE attrelid = {oid} AND attname = 'v'")
            assert oid > 0 and attr == 2
            origins.append((oid,attr))
        assert origins[0][0] != origins[1][0]
        baseline = sql(conn,f'SELECT v FROM {table} ORDER BY id')
        assert baseline['rows'] == [['37'],[None]], baseline
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            cases = [
                (f'SELECT v FROM {table} ORDER BY id','v',origins[0],[7,None]),
                (f'SELECT v AS renamed FROM {table} ORDER BY id','renamed',origins[0],[7,None]),
                (f'SELECT t.v AS renamed FROM {table} AS t ORDER BY id','renamed',origins[0],[7,None]),
                (f'SELECT v AS renamed FROM {table} WHERE false','renamed',origins[0],[]),
                (f'SELECT v AS renamed FROM {tables[1]} WHERE false','renamed',origins[1],[]),
                (f'SELECT (v + 1)::int4 AS v FROM {table} ORDER BY id','v',(0,0),[8,None]),
                (f'SELECT (v + 1)::int4 AS renamed FROM {table} WHERE false','renamed',(0,0),[]),
                (f'SELECT NULL::int4 AS v FROM {table} WHERE id = 1','v',(0,0),[None]),
                (f'SELECT 123::int4 AS v FROM {table} WHERE false','v',(0,0),[]),
            ]
            for index, (text,label,origin,values) in enumerate(cases):
                send(wire,b'Q',text.encode()+b'\0')
                metadata(receive(wire),label,origin,0)
                for number in values:
                    assert receive(wire) == (b'D',data_row(number,0)), text
                assert receive(wire) == (b'C',f'SELECT {len(values)}\0'.encode())
                assert receive(wire) == (b'Z',b'I')
                count += 1
                name = f'origin104_{index}'.encode()
                send(wire,b'P',name+b'\0'+text.encode()+b'\0'+struct.pack('!h',0))
                assert flush(wire) == (b'1',b''), text
                def statement():
                    send(wire,b'D',b'S'+name+b'\0')
                    assert flush(wire) == (b't',struct.pack('!h',0))
                    metadata(receive(wire),label,origin,0)
                statement()
                for fmt in [0,1]:
                    send(wire,b'B',b'\0'+name+b'\0'+struct.pack('!hhhh',0,0,1,fmt))
                    assert flush(wire) == (b'2',b'')
                    send(wire,b'D',b'P\0')
                    metadata(flush(wire),label,origin,fmt)
                    statement()
                    send(wire,b'E',b'\0'+struct.pack('!i',0))
                    message = flush(wire)
                    for number in values:
                        assert message == (b'D',data_row(number,fmt)), (text,fmt,message)
                        message = receive(wire)
                    assert message == (b'C',f'SELECT {len(values)}\0'.encode()), (text,message)
                    recover(wire)
                    count += 1
            # An invalid reference must fail at Parse, before any success data;
            # it must not modify either fixture, and Sync restores the session.
            send(wire,b'P',b'bad104\0'+f'SELECT v FROM {table}_missing'.encode()+b'\0'+struct.pack('!h',0))
            tag,body = flush(wire)
            assert tag == b'E', (tag,body)
            fields = {part[:1]:part[1:] for part in body.split(b'\0') if part}
            assert fields[b'C'] == b'42P01', fields
            recover(wire)
            count += 1
        assert sql(conn,f'SELECT v FROM {table} ORDER BY id') == baseline
        print(f'endpoint={endpoint} catalog_origins={origins} cases=28 passed',flush=True)
        for name in tables:
            sql(conn,f'DROP TABLE {name}')
    print(f'base_column_origin_cases={count} passed',flush=True)
finally:
    for dsn, conn in zip(dsns,connections):
        finish(conn)
        cleanup = connect(dsn.encode())
        try:
            if cleanup and connected(cleanup) == 0:
                for name in tables:
                    sql(cleanup,f'DROP TABLE IF EXISTS {name}')
        finally:
            if cleanup:
                finish(cleanup)
