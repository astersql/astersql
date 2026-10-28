# Copyright 2026 AsterSQL.
"""Replay task 15 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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

result_format = 0

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
                row.append(None if size == -1 else (body[offset:offset + size].hex() if result_format else body[offset:offset + size].decode()))
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
    sql = sql.replace('public.pg06_015', 'pg_temp.pg06_015')
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
        assert result['metadata'] == [[expected[1], 8 if expected[1] == 701 else 4, -1, 0]], result
    return result

def flush_read():
    send(b'H')
    return read()

def sync():
    send(b'S')
    assert read() == (b'Z', b'I')

def parse(name, sql, oid=20, error=None):
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
    assert result['metadata'] == [[701, 8, -1, 0]], result
    if kind == b'S':
        assert result['parameters'] == [20], result

def bind(name, value, error=None):
    data = struct.pack('!i', -1) if value is None else struct.pack('!i', len(value)) + value.encode()
    send(b'B', b'\0' + name.encode() + b'\0' + struct.pack('!hh', 0, 1) + data + struct.pack('!h', 0))
    message = flush_read()
    result = record('Bind', name + ':' + str(value), [message])
    if error:
        assert result['errors'] == [error], result
        sync()
        query('SELECT 1::int4', ('1', 23))
        query('SELECT v FROM public.pg06_015 WHERE id=1', (None, 701))
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
    query('CREATE TEMP TABLE pg06_015 (id INT PRIMARY KEY, v float8)')
    query('INSERT INTO public.pg06_015 VALUES (1,9223372036854775807::int8)')
    query('SELECT v FROM public.pg06_015 WHERE id=1', ('9.223372036854776e+18', 701))
    query('UPDATE public.pg06_015 SET v=(-9223372036854775808)::int8 WHERE id=1')
    query('SELECT v FROM public.pg06_015 WHERE id=1', ('-9.223372036854776e+18', 701))
    query('UPDATE public.pg06_015 SET v=NULL::int8 WHERE id=1')
    query('SELECT v FROM public.pg06_015 WHERE id=1', (None, 701))
    for value, expected in [('1','1'),('-9223372036854775808','-9.223372036854776e+18'),('9223372036854775807','9.223372036854776e+18'),('9007199254740991','9.007199254740991e+15'),('9007199254740992','9.007199254740992e+15'),('9007199254740993','9.007199254740992e+15'),('9007199254740994','9.007199254740994e+15'),('9007199254740995','9.007199254740996e+15'),('-9007199254740993','-9.007199254740992e+15'),('NULL',None)]:
        query(f'SELECT (({value})::int8)::float8 AS v', (expected,701))
    for expression, expected in [('pg_catalog.float8abs((-1)::int8)','1'),('float8abs(9007199254740993::int8)','9.007199254740992e+15'),('float8abs(NULL::int8)',None),('float8abs(1)','1')]:
        query(f'SELECT {expression} AS v', (expected,701))
    for name, sql, values in [
        ('widen','SELECT $1::float8 AS v',[('-9223372036854775808','-9.223372036854776e+18'),('9223372036854775807','9.223372036854776e+18'),('9007199254740993','9.007199254740992e+15'),(None,None)]),
        ('implicit','SELECT float8abs($1::int8) AS v',[('-9223372036854775808','9.223372036854776e+18'),('9223372036854775807','9.223372036854776e+18'),(None,None)]),
    ]:
        parse(name,sql)
        describe(b'S',name)
        for value, expected in values:
            bind(name,value)
            execute(name,expected)
    for input_format in [0, 1]:
        for output_format in [0, 1]:
            for value, expected in [(-9223372036854775808, -9.223372036854776e18), (9223372036854775807, 9.223372036854776e18), (9007199254740993, 9007199254740992.0), (9007199254740995, 9007199254740996.0), (None, None)]:
                payload = None if value is None else (struct.pack('!q', value) if input_format else str(value).encode())
                data = struct.pack('!i', -1) if payload is None else struct.pack('!i', len(payload)) + payload
                send(b'B', b'\0widen\0' + struct.pack('!hhh', 1, input_format, 1) + data + struct.pack('!hh', 1, output_format))
                assert flush_read() == (b'2', b'')
                send(b'D', b'P\0')
                description = record('Describe P', 'binary matrix', [flush_read()])
                assert description['metadata'] == [[701, 8, -1, output_format]], description
                result_format = output_format
                send(b'E', b'\0' + struct.pack('!i', 0))
                send(b'H')
                result = record('Execute', f'formats={input_format}/{output_format};int8={value}', [read(), read()])
                assert result['tags'] == 'DC', result
                actual = result['rows'][0][0]
                if expected is None:
                    assert actual is None, result
                elif output_format:
                    assert actual == struct.pack('!d', expected).hex(), result
                else:
                    assert struct.pack('!d', float(actual)) == struct.pack('!d', expected), result
                result_format = 0
                sync()
    parse('assign','INSERT INTO pg_temp.pg06_015 VALUES (2,$1)')
    send(b'D',b'Sassign\0')
    send(b'H')
    result=record('Describe S','assign',[read(),read()])
    assert result['tags']=='tn' and result['parameters']==[20] and not result['metadata'], result
    for value, state in [('9223372036854775808','22003'),('-9223372036854775809','22003'),('invalid','22P02')]:
        bind('assign',value,error=state)
        assert query('SELECT count(*) FROM public.pg06_015 WHERE id=2')['rows']==[['0']]
    for index, sql in enumerate(['SELECT float8abs(NULL::text)', 'SELECT float8abs(true)']):
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
