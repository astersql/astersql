# Copyright 2026 AsterSQL.
"""Task 1: real TCP differential against the shared fixed PostgreSQL 18.6."""
import ctypes as c
import json
import os
import socket
import struct
import sys
import uuid
from pathlib import Path

pq = c.CDLL(os.environ['PG_LIBPQ_LIBRARY'])
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
p, s, i = c.c_void_p, c.c_char_p, c.c_int
connect = api('PQconnectdb', p, s)
status = api('PQstatus', i, p)
finish = api('PQfinish', None, p)
fd = api('PQsocket', i, p)
version = api('PQserverVersion', i, p)
libversion = api('PQlibVersion', i)
assert libversion() == 180006
records = {}

def run(label, dsn):
    conn = connect(dsn.encode())
    assert conn and status(conn) == 0, label + ' connection failed'
    raw = None
    table = 'public.pg06_001_' + uuid.uuid4().hex
    observations = []
    created = False
    def send(tag, body=b''):
        raw.sendall(tag + struct.pack('!I', len(body)+4) + body)
    def exact(n):
        result = b''
        while len(result) < n:
            part = raw.recv(n-len(result))
            assert part, 'unexpected EOF'
            result += part
        return result
    def read():
        tag = exact(1)
        size = struct.unpack('!I', exact(4))[0]
        assert 4 <= size < 1 << 20
        return tag, exact(size-4)
    def observe(phase, messages):
        result = dict(phase=phase, tags='', metadata=[], rows=[], states=[], parameters=[])
        for tag, body in messages:
            result['tags'] += tag.decode()
            if tag == b'T':
                offset = 2
                for _ in range(struct.unpack('!h', body[:2])[0]):
                    end = body.index(0, offset)
                    fields = struct.unpack('!IhIhih', body[end+1:end+19])
                    result['metadata'].append(list(fields[2:]))
                    offset = end+19
            elif tag == b'D':
                offset, row = 2, []
                for _ in range(struct.unpack('!h', body[:2])[0]):
                    size = struct.unpack('!i', body[offset:offset+4])[0]
                    offset += 4
                    row.append(None if size == -1 else body[offset:offset+size].hex())
                    if size != -1:
                        offset += size
                result['rows'].append(row)
            elif tag == b'E':
                fields = {chr(f[0]): f[1:].decode() for f in body.rstrip(b'\0').split(b'\0')}
                result['states'].append(fields['C'])
            elif tag == b't':
                count = struct.unpack('!h', body[:2])[0]
                result['parameters'] = list(struct.unpack('!'+'I'*count, body[2:]))
        observations.append(result)
        return result
    def query(sql, value=None, check=False):
        send(b'Q', sql.encode()+b'\0')
        messages = []
        while True:
            message = read()
            messages.append(message)
            if message[0] == b'Z':
                break
        result = observe('Query', messages)
        assert not result['states'], (label, sql, result)
        if check:
            assert result['metadata'] == [[23,4,-1,0]], (sql, result)
            assert result['rows'] == [[None if value is None else value.encode().hex()]], (sql, result)
        return result
    def flush():
        send(b'H')
        return read()
    def sync():
        send(b'S')
        assert read() == (b'Z', b'I')
    try:
        if label == 'reference':
            assert version(conn) == 180006
        raw = socket.socket(fileno=os.dup(fd(conn)))
        raw.settimeout(15)
        query(f'CREATE TABLE {table} (id INT PRIMARY KEY, v INT)')
        created = True
        query(f'INSERT INTO {table} VALUES (1,32767::int2)')
        query(f'SELECT v FROM {table} WHERE id=1', '32767', True)
        query(f'UPDATE {table} SET v=(-32768)::int2 WHERE id=1')
        cases = [
            ('SELECT (1::int2)::int4 AS v', '1'),
            ('SELECT ((-32768)::int2)::int4 AS v', '-32768'),
            ('SELECT (32767::int2)::int4 AS v', '32767'),
            ('SELECT NULL::int2::int4 AS v', None),
            ('SELECT (1::int2)+(6::int4) AS v', '7'),
            ('SELECT (6::int4)-(1::int2) AS v', '5'),
            ('SELECT (1::int2)*(6::int4) AS v', '6'),
            ('SELECT (NULL::int2)+(6::int4) AS v', None),
            (f'SELECT v FROM {table} WHERE id=1', '-32768'),
            ('SELECT (1.5::numeric::int2)::int4 AS v', '2'),
            ('SELECT ((-1.5)::numeric::int2)::int4 AS v', '-2'),
            ('SELECT int4not(1::int2) AS v', '-2'),
            ('SELECT int4not(NULL::int2) AS v', None),
        ]
        for sql, value in cases:
            query(sql, value, True)
        for sql in ['SELECT (NULL::int2)::int4 AS v LIMIT 0',
                    'SELECT (1::int2)+(6::int4) AS v LIMIT 0']:
            result = query(sql)
            assert result['rows'] == [] and result['metadata'] == [[23,4,-1,0]], result
        send(b'P', b'widen\0SELECT $1::int4 AS v\0' + struct.pack('!hI',1,21))
        assert flush() == (b'1', b'')
        send(b'D', b'Swiden\0')
        send(b'H')
        desc = observe('Describe statement', [read(),read()])
        assert desc['parameters'] == [21] and desc['metadata'] == [[23,4,-1,0]], desc
        for infmt in [0,1]:
            for outfmt in [0,1]:
                for value in [-32768,32767,None]:
                    payload = None if value is None else (str(value).encode() if infmt == 0 else struct.pack('!h', value))
                    data = struct.pack('!i',-1) if payload is None else struct.pack('!i',len(payload))+payload
                    send(b'B', b'\0widen\0'+struct.pack('!hhh',1,infmt,1)+data+struct.pack('!hh',1,outfmt))
                    assert flush() == (b'2', b'')
                    send(b'D', b'P\0')
                    result = observe('Describe portal', [flush()])
                    assert result['metadata'] == [[23,4,-1,outfmt]], result
                    send(b'E', b'\0'+struct.pack('!i',0))
                    send(b'H')
                    result = observe('Execute', [read(),read()])
                    expected = None if value is None else (str(value).encode() if outfmt == 0 else struct.pack('!i',value)).hex()
                    assert result['tags']=='DC' and result['rows']==[[expected]], result
                    sync()
        for value, state in [('32768','22003'),('-32769','22003'),('invalid','22P02')]:
            data = value.encode()
            send(b'B', b'\0widen\0'+struct.pack('!hh',0,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0))
            result = observe('Bind', [flush()])
            assert result['tags']=='E' and result['states']==[state] and result['rows']==[], result
            sync()
            query('SELECT 1::int4', '1', True)
            query(f'SELECT v FROM {table} WHERE id=1', '-32768', True)
        query(f'UPDATE {table} SET v=NULL::int2 WHERE id=1')
        query(f'SELECT v FROM {table} WHERE id=1', None, True)
        records[label] = observations
    finally:
        if raw is not None:
            try:
                if created:
                    query(f'DROP TABLE {table}')
            finally:
                raw.close()
        finish(conn)

try:
    run('reference', os.environ['PG18_REFERENCE_DSN'])
    run('aster', f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test connect_timeout=10')
    assert records['reference'] == records['aster'], 'task 1 wire observations differ'
    print(json.dumps(dict(passed=True, reference_version=180006, observations_per_endpoint=len(records['aster']), formats=4)))
finally:
    evidence = os.environ.get('PG06_TASK001_EVIDENCE')
    if evidence:
        Path(evidence).write_text(json.dumps(records, indent=2))
