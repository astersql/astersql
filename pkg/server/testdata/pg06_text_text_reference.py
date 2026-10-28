# Copyright 2026 AsterSQL.
"""Replay task 33 on the fixed PG18_REFERENCE_DSN; never owns the server."""
import ctypes as c
import json
import os
import socket
import struct
import sys

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
server_version = api('PQserverVersion', i, p)
lib_version = api('PQlibVersion', i)
conn = connect(os.environ['PG18_REFERENCE_DSN'].encode())
assert conn and status(conn) == 0, 'reference connection failed'
raw = None
records = []

def send(tag, body=b''):
    raw.sendall(tag + struct.pack('!I', len(body) + 4) + body)

def exact(n):
    data = b''
    while len(data) < n:
        part = raw.recv(n - len(data))
        assert part, 'unexpected EOF'
        data += part
    return data

def read():
    tag = exact(1)
    size = struct.unpack('!I', exact(4))[0]
    assert 4 <= size < 1 << 20
    return tag, exact(size - 4)

def observe(messages):
    result = dict(tags='', metadata=[], rows=[], errors=[], parameters=[])
    for tag, body in messages:
        result['tags'] += tag.decode()
        if tag == b'T':
            count = struct.unpack('!H', body[:2])[0]
            offset = 2
            for _ in range(count):
                end = body.index(0, offset)
                oid, attr, tp, size, mod, fmt = struct.unpack('!IhIhih', body[end + 1:end + 19])
                result['metadata'].append([tp, size, mod, fmt])
                offset = end + 19
        elif tag == b'D':
            count = struct.unpack('!H', body[:2])[0]
            offset, row = 2, []
            for _ in range(count):
                size = struct.unpack('!i', body[offset:offset + 4])[0]
                offset += 4
                row.append(None if size == -1 else body[offset:offset + size].decode())
                if size != -1:
                    offset += size
            result['rows'].append(row)
        elif tag == b'E':
            fields = {}
            for field in body.rstrip(b'\0').split(b'\0'):
                fields[chr(field[0])] = field[1:].decode()
            result['errors'].append(fields['C'])
        elif tag == b't':
            count = struct.unpack('!H', body[:2])[0]
            result['parameters'] = list(struct.unpack('!' + 'I' * count, body[2:]))
    return result

def record(phase, sql, messages):
    result = observe(messages)
    records.append(dict(phase=phase, sql=sql, **result))
    return result

def query(sql, expected=None, error=None):
    sql = sql.replace('public.pg06_033', 'pg_temp.pg06_033')
    send(b'Q', sql.encode() + b'\0')
    messages = []
    while True:
        message = read()
        messages.append(message)
        if message[0] == b'Z':
            break
    result = record('Query', sql, messages)
    if error:
        assert result['errors'] == [error] and not result['rows'], result
        return result
    assert not result['errors'], result
    if expected is not None:
        assert result['rows'] == [[expected[0]]], result
        assert result['metadata'] == [[expected[1], {21: 2, 23: 4, 20: 8, 1700: -1, 700: 4, 16: 1, 25: -1}[expected[1]], -1, 0]], result
    return result

def flush_read():
    send(b'H')
    return read()

def sync():
    send(b'S')
    assert read() == (b'Z', b'I')

def parse(name, sql, oid=25, error=None):
    send(b'P', name.encode() + b'\0' + sql.encode() + b'\0' + struct.pack('!hI', 1, oid))
    message = flush_read()
    result = record('Parse', sql, [message])
    if error:
        assert result['errors'] == [error], result
        sync()
        query('SELECT 1::int4', ('1', 23))
    else:
        assert message == (b'1', b''), result

def describe(kind, name):
    send(b'D', kind + name.encode() + b'\0')
    send(b'H')
    messages = [read()]
    if kind == b'S':
        messages.append(read())
    result = record('Describe ' + kind.decode(), name, messages)
    assert result['metadata'] == [[25, -1, -1, 0]], result
    if kind == b'S':
        assert result['parameters'] == [25], result

def bind(name, value):
    data = struct.pack('!i', -1) if value is None else struct.pack('!i', len(value.encode())) + value.encode()
    send(b'B', b'\0' + name.encode() + b'\0' + struct.pack('!hh', 0, 1) + data + struct.pack('!h', 0))
    message = flush_read()
    result = record('Bind', name + ':' + str(value), [message])
    assert message == (b'2', b''), result

try:
    assert server_version(conn) == 180006 and lib_version() == 180006
    raw = socket.socket(fileno=os.dup(fd(conn)))
    raw.settimeout(10)
    query('CREATE TEMP TABLE pg06_033 (id INT PRIMARY KEY,v text)')
    query("INSERT INTO public.pg06_033 VALUES (1,'sentinel')")
    cases=['中文ab','中文ab  ','  ','','éé😀',None]
    for value in cases:
        source="'"+value+"'" if value is not None else 'NULL'
        query(f'SELECT ({source}::text)::text AS v',(value,25))
        query(f'UPDATE public.pg06_033 SET v={source}::text WHERE id=1')
        query('SELECT v FROM public.pg06_033 WHERE id=1',(value,25))
        result=query(f'SELECT ({source}::text)::text AS v UNION ALL SELECT NULL::text AS v')
        assert result['rows']==[[value],[None]] and result['metadata']==[[25,-1,-1,0]],result
    for name,sql in [('explicit','SELECT ($1::text)::text AS v'),('implicit','SELECT $1 UNION ALL SELECT NULL::text')]:
        parse(name,sql);describe(b'S',name)
        for fmt in [0,1]:
            for value in cases:
                payload=struct.pack('!i',-1) if value is None else struct.pack('!i',len(value.encode()))+value.encode()
                send(b'B',b'\0'+name.encode()+b'\0'+struct.pack('!hhh',1,fmt,1)+payload+struct.pack('!hh',1,fmt))
                assert flush_read()==(b'2',b'')
                send(b'D',b'P\0');result=record('Describe P',name,[flush_read()]);assert result['metadata']==[[25,-1,-1,fmt]],result
                send(b'E',b'\0'+struct.pack('!i',0));send(b'H')
                messages=[read()]
                if name=='implicit':messages.append(read())
                messages.append(read());result=record('Execute',name,messages)
                assert result['tags']==('DDC' if name=='implicit' else 'DC') and result['rows']==([[value],[None]] if name=='implicit' else [[value]]),result
                sync();query('SELECT 1::int4',('1',23))
    parse('assignment','UPDATE pg_temp.pg06_033 SET v=$1 WHERE id=1')
    send(b'D',b'Sassignment\0');send(b'H');result=record('Describe S assignment','',[read(),read()]);assert result['tags']=='tn' and result['parameters']==[25],result
    for value in cases:
        bind('assignment',value)
        send(b'D',b'P\0');assert flush_read()==(b'n',b'')
        send(b'E',b'\0'+struct.pack('!i',0));result=record('Execute assignment','',[flush_read()]);assert result['tags']=='C',result
        sync();query('SELECT v FROM public.pg06_033 WHERE id=1',(value,25))
    query("UPDATE public.pg06_033 SET v='sentinel' WHERE id=1")
    for fmt in [0,1]:
        for value in [b'\xff',b'a\0b',b'\xc3']:
            send(b'B',b'\0assignment\0'+struct.pack('!hhh',1,fmt,1)+struct.pack('!i',len(value))+value+struct.pack('!h',0))
            result=record('Bind invalid',value.hex(),[flush_read()]);assert result['tags']=='E' and result['errors']==['22021'],result
            sync();query('SELECT 1::int4',('1',23));query('SELECT v FROM public.pg06_033 WHERE id=1',('sentinel',25))
    for sql in ["SELECT ('中文ab'::text)::text(2)","SELECT (NULL::text)::text(2)","SELECT $1::text(2)"]:
        parse('reject',sql,error='42601')
        if '$' not in sql:query(sql,error='42601');query('SELECT 1::int4',('1',23))
    query('CREATE TABLE pg_temp.pg06_033_bad (v text(2))',error='42601')
    result=query('SELECT (NULL::text)::text AS v LIMIT 0');assert result['tags']=='TCZ' and result['metadata']==[[25,-1,-1,0]] and not result['rows'],result
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:json.dump(records,out,indent=2,ensure_ascii=False)
