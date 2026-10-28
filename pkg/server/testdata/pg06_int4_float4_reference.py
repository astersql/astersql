# Copyright 2026 AsterSQL.
"""Replay task 9 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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

def query(sql, expected=None):
    sql = sql.replace('public.pg06_009', 'pg_temp.pg06_009')
    send(b'Q', sql.encode() + b'\0')
    messages = []
    while True:
        message = read()
        messages.append(message)
        if message[0] == b'Z':
            break
    result = record('Query', sql, messages)
    assert not result['errors'], result
    if expected is not None:
        assert result['rows'] == [[expected[0]]], result
        assert result['metadata'] == [[expected[1], 4, -1, 0]], result
    return result

def flush_read():
    send(b'H')
    return read()

def sync():
    send(b'S')
    assert read() == (b'Z', b'I')

def parse(name, sql, oid=23, error=None):
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
    assert result['metadata'] == [[700, 4, -1, 0]], result
    if kind == b'S':
        assert result['parameters'] == [23], result

def bind(name, value, error=None):
    data = struct.pack('!i', -1) if value is None else struct.pack('!i', len(value)) + value.encode()
    send(b'B', b'\0' + name.encode() + b'\0' + struct.pack('!hh', 0, 1) + data + struct.pack('!h', 0))
    message = flush_read()
    result = record('Bind', name + ':' + str(value), [message])
    if error:
        assert result['errors'] == [error], result
        sync()
        query('SELECT 1::int4', ('1', 23))
        query('SELECT v FROM public.pg06_009 WHERE id=1', (None, 700))
    else:
        assert message == (b'2', b''), result

def execute(name, value):
    describe(b'P', '')
    send(b'E', b'\0' + struct.pack('!i', 0))
    send(b'H')
    messages = [read(), read()]
    result = record('Execute', name, messages)
    assert result['tags'] == 'DC' and result['rows'] == [[value]], result
    sync()

try:
    assert server_version(conn) == 180006 and lib_version() == 180006
    raw = socket.socket(fileno=os.dup(fd(conn)))
    raw.settimeout(10)
    query('CREATE TEMP TABLE pg06_009 (id INT PRIMARY KEY, v float4)')
    query('INSERT INTO public.pg06_009 VALUES (1,2147483647::int4)')
    query('SELECT v FROM public.pg06_009 WHERE id=1', ('2.1474836e+09', 700))
    query('UPDATE public.pg06_009 SET v=(-2147483648)::int4 WHERE id=1')
    query('SELECT v FROM public.pg06_009 WHERE id=1', ('-2.1474836e+09', 700))
    query('UPDATE public.pg06_009 SET v=NULL::int4 WHERE id=1')
    query('SELECT v FROM public.pg06_009 WHERE id=1', (None, 700))
    for value, expected in [('1','1'),('-2147483648','-2.1474836e+09'),('2147483647','2.1474836e+09'),('16777215','1.6777215e+07'),('16777216','1.6777216e+07'),('16777217','1.6777216e+07'),('16777218','1.6777218e+07'),('16777219','1.677722e+07'),('-16777217','-1.6777216e+07'),('NULL',None)]:
        query(f'SELECT (({value})::int4)::float4 AS v', (expected,700))
    for expression, expected in [('pg_catalog.float4abs((-1)::int4)','1'),('float4abs(16777217::int4)','1.6777216e+07'),('float4abs(NULL::int4)',None),('float4abs(1)','1')]:
        query(f'SELECT {expression} AS v', (expected,700))
    for name, sql, values in [
        ('widen','SELECT $1::float4 AS v',[('-2147483648','-2.1474836e+09'),('2147483647','2.1474836e+09'),('16777217','1.6777216e+07'),(None,None)]),
        ('implicit','SELECT float4abs($1::int4) AS v',[('-2147483648','2.1474836e+09'),('2147483647','2.1474836e+09'),(None,None)]),
    ]:
        parse(name,sql)
        describe(b'S',name)
        for value, expected in values:
            bind(name,value)
            execute(name,expected)
    parse('assign','INSERT INTO pg_temp.pg06_009 VALUES (2,$1)')
    send(b'D',b'Sassign\0')
    send(b'H')
    result=record('Describe S','assign',[read(),read()])
    assert result['tags']=='tn' and result['parameters']==[23] and not result['metadata'], result
    for value, state in [('2147483648','22003'),('-2147483649','22003'),('invalid','22P02')]:
        bind('assign',value,error=state)
        assert query('SELECT count(*) FROM public.pg06_009 WHERE id=2')['rows']==[['0']]
    for index, sql in enumerate(['SELECT float4abs(NULL::text)', 'SELECT float4abs(NULL::float8)', 'SELECT float4abs(true)']):
        # These statements have no parameters; record Parse rejection directly.
        send(b'P', f'rejected{index}'.encode()+b'\0'+sql.encode()+b'\0'+struct.pack('!h',0))
        result=record('Parse',sql,[flush_read()])
        assert result['tags']=='E' and result['errors']==['42883'], result
        sync()
        query('SELECT 1::int4',('1',23))
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:
        raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:
            json.dump(records,out,indent=2)
