# Copyright 2026 AsterSQL.
"""Replay task 32 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    sql = sql.replace('public.pg06_032', 'pg_temp.pg06_032')
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
        assert result['metadata'] == [[expected[1], {21: 2, 23: 4, 20: 8, 1700: -1, 700: 4, 16: 1}[expected[1]], -1, 0]], result
    return result

def flush_read():
    send(b'H')
    return read()

def sync():
    send(b'S')
    assert read() == (b'Z', b'I')

def parse(name, sql, oid=16, error=None):
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
    assert result['metadata'] == [[23, 4, -1, 0]], result
    if kind == b'S':
        assert result['parameters'] == [16], result

def bind(name, value):
    data = struct.pack('!i', -1) if value is None else struct.pack('!i', len(value)) + value.encode()
    send(b'B', b'\0' + name.encode() + b'\0' + struct.pack('!hh', 0, 1) + data + struct.pack('!h', 0))
    message = flush_read()
    result = record('Bind', name + ':' + str(value), [message])
    assert message == (b'2', b''), result

def execute(name, value=None):
    describe(b'P', '')
    send(b'E', b'\0' + struct.pack('!i', 0))
    send(b'H')
    result = record('Execute', name, [read(), read()])
    assert result['tags'] == 'DC' and result['rows'] == [[value]], result
    sync()


try:
    assert server_version(conn) == 180006 and lib_version() == 180006
    raw = socket.socket(fileno=os.dup(fd(conn)))
    raw.settimeout(10)
    query('CREATE TEMP TABLE pg06_032 (id INT PRIMARY KEY,v int4)')
    query('CREATE TEMP TABLE pg06_032_source (v BOOLEAN)')
    query('INSERT INTO public.pg06_032 VALUES (1,7)')
    cases=[('false','0'),('true','1'),(None,None)]
    for value,expected in cases:
        source="'"+value+"'" if value is not None else 'NULL'
        query(f'SELECT ({source}::bool)::int4 AS v',(expected,23))
    query('SELECT true::int4',('1',23))
    query('SELECT false::int4',('0',23))
    query('SELECT int4not(true::int4)',('-2',23))
    query('UPDATE public.pg06_032 SET v=true::int4 WHERE id=1')
    query('SELECT v FROM public.pg06_032 WHERE id=1',('1',23))
    query('UPDATE public.pg06_032 SET v=7 WHERE id=1')
    for sql in ['SELECT $1::int4 AS v','SELECT ($1::bool)::int4 AS v']:
        if '($1::bool)' in sql:
            send(b'C',b'Sexplicit\0');assert flush_read()==(b'3',b'')
        parse('explicit',sql);describe(b'S','explicit')
        for value,expected in cases:
            bind('explicit',value);execute('explicit',expected);query('SELECT 1::int4',('1',23))
        value='invalid'
        data=struct.pack('!i',len(value))+value.encode()
        send(b'B',b'\0explicit\0'+struct.pack('!hh',0,1)+data+struct.pack('!h',0))
        result=record('Bind',value,[flush_read()]);assert result['tags']=='E' and result['errors']==['22P02'],result
        sync();query('SELECT 1::int4',('1',23))
    for source in ['true','false','NULL::bool']:
        for sql,error in [(f'INSERT INTO pg_temp.pg06_032 VALUES (2,{source})','42804'),(f'UPDATE pg_temp.pg06_032 SET v={source} WHERE id=1','42804'),(f'INSERT INTO pg_temp.pg06_032 SELECT 2,{source}','42804'),(f'SELECT int4not({source})','42883')]:
            parse('reject',sql,error=error)
            query(sql,error=error);query('SELECT 1::int4',('1',23))
            query('SELECT v FROM public.pg06_032 WHERE id=1',('7',23))
            assert query('SELECT count(*) FROM public.pg06_032 WHERE id=2')['rows']==[['0']]
    for sql,error in [('INSERT INTO pg_temp.pg06_032 VALUES (2,$1)','42804'),('UPDATE pg_temp.pg06_032 SET v=$1 WHERE id=1','42804'),('SELECT int4not($1)','42883'),('UPDATE pg_temp.pg06_032 SET v=NULL::bool WHERE id=99','42804'),('INSERT INTO pg_temp.pg06_032 SELECT 2,v FROM pg_temp.pg06_032_source','42804'),('SELECT int4not(v) FROM pg_temp.pg06_032_source','42883')]:
        parse('reject',sql,error=error)
    empty=query('SELECT (NULL::bool)::int4 AS v LIMIT 0')
    assert empty['tags']=='TCZ' and empty['metadata']==[[23,4,-1,0]] and not empty['rows'],empty
    for value in [False,True,None]:
        payload=struct.pack('!i',-1) if value is None else struct.pack('!iB',1,int(value))
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,1,1)+payload+struct.pack('!hh',1,1));assert flush_read()==(b'2',b'')
        send(b'D',b'P\0');result=record('Describe P binary','explicit',[flush_read()]);assert result['metadata']==[[23,4,-1,1]],result
        send(b'E',b'\0'+struct.pack('!i',0));message=flush_read()
        expected=struct.pack('!hi',1,-1) if value is None else struct.pack('!hii',1,4,int(value))
        assert message==(b'D',expected),message
        assert read()[0]==b'C';records.append(dict(phase='Execute binary',payload=message[1].hex()));sync();query('SELECT 1::int4',('1',23))
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:json.dump(records,out,indent=2)
