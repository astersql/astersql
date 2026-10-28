# Copyright 2026 AsterSQL.
"""Replay task 20 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    sql = sql.replace('public.pg06_020', 'pg_temp.pg06_020')
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
        assert result['metadata'] == [[expected[1], {23: 4, 701: 8}[expected[1]], -1, 0]], result
    return result

def flush_read():
    send(b'H')
    return read()

def sync():
    send(b'S')
    assert read() == (b'Z', b'I')

def parse(name, sql, oid=1700, error=None):
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
        assert result['parameters'] == [1700], result

def bind(name, value, error=None):
    data = struct.pack('!i', -1) if value is None else struct.pack('!i', len(value)) + value.encode()
    send(b'B', b'\0' + name.encode() + b'\0' + struct.pack('!hh', 0, 1) + data + struct.pack('!h', 0))
    message = flush_read()
    result = record('Bind', name + ':' + str(value), [message])
    if error:
        assert result['errors'] == [error], result
        sync()
        query('SELECT 1::int4', ('1', 23))
    else:
        assert message == (b'2', b''), result

def execute(name, value=None, error=None):
    describe(b'P', '')
    send(b'E', b'\0' + struct.pack('!i', 0))
    send(b'H')
    messages = [read()]
    if not error:
        messages.append(read())
    result = record('Execute', name, messages)
    if error:
        assert result['tags'] == 'E' and result['errors'] == [error] and not result['rows'], result
    else:
        assert result['tags'] == 'DC' and result['rows'] == [[value]], result
    sync()

try:
    assert server_version(conn) == 180006 and lib_version() == 180006
    raw = socket.socket(fileno=os.dup(fd(conn)))
    raw.settimeout(10)
    query('SET plan_cache_mode = force_generic_plan')
    query('CREATE TEMP TABLE pg06_020 (id INT PRIMARY KEY, v float8)')
    query('INSERT INTO public.pg06_020 VALUES (1,9007199254740993::numeric)')
    query('INSERT INTO public.pg06_020 VALUES (4,1.5::numeric)')
    query('SELECT v FROM public.pg06_020 WHERE id=4',('1.5',701))
    query('CREATE TEMP TABLE pg06_020_source (id INT PRIMARY KEY, n numeric)')
    query('INSERT INTO public.pg06_020_source VALUES (1,9007199254740993::numeric)')
    query('INSERT INTO public.pg06_020 SELECT 3,n FROM public.pg06_020_source')
    values = [('1.5','1.5'),('9007199254740993','9.007199254740992e+15'),('9007199254740995','9.007199254740996e+15'),('1.7976931348623157e308','1.7976931348623157e+308'),('5e-324','5e-324'),(None,None)]
    for value, expected in [('1','1'), *values, ('-1.7976931348623157e308','-1.7976931348623157e+308')]:
        literal = 'NULL' if value is None else value
        query(f'SELECT (({literal})::numeric)::float8 AS v', (expected,701))
    query('SELECT float8abs((-1.5)::numeric) AS v', ('1.5',701))
    query('SELECT float8abs(NULL::numeric) AS v', (None,701))
    query('SELECT n::float8 FROM public.pg06_020_source', ('9.007199254740992e+15',701))
    for key in [1,3]:
        query(f'SELECT v FROM public.pg06_020 WHERE id={key}', ('9.007199254740992e+15',701))
    for name, sql in [('explicit','SELECT $1::float8 AS v'),('implicit','SELECT float8abs($1) AS v'),('assign','UPDATE pg_temp.pg06_020 SET v=$1 WHERE id=1')]:
        parse(name,sql)
        if name != 'assign':
            describe(b'S',name)
        else:
            send(b'D',b'Sassign\0')
            send(b'H')
            result=record('Describe S',name,[read(),read()])
            assert result['tags']=='tn' and result['parameters']==[1700] and not result['metadata'], result
        for value, expected in values:
            bind(name,value)
            if name != 'assign':
                execute(name,expected)
            else:
                send(b'D',b'P\0')
                assert flush_read()==(b'n',b'')
                send(b'E',b'\0'+struct.pack('!i',0))
                result=record('Execute',name,[flush_read()])
                assert result['tags']=='C' and not result['rows'] and not result['errors'],result
                sync()
                query('SELECT v FROM public.pg06_020 WHERE id=1',(expected,701))
        for value in ['1.7976931348623159e308','-1.7976931348623159e308','1e-324','-1e-324']:
            bind(name,value)
            if name != 'assign':
                execute(name,error='22003')
            else:
                send(b'E',b'\0'+struct.pack('!i',0))
                result=record('Execute',name,[flush_read()])
                assert result['tags']=='E' and result['errors']==['22003'] and not result['rows'],result
                sync()
            query('SELECT 1::int4',('1',23))
            query('SELECT v FROM public.pg06_020 WHERE id=1',(None if name=='assign' else '9.007199254740992e+15',701))
        bind(name,'invalid',error='22P02')
    for oid in [25,16]:
        parse('reject', 'SELECT float8abs($1)', oid=oid, error='42883')
    for value in ['1.7976931348623159e308','-1.7976931348623159e308','1e-324','-1e-324']:
        for sql in [f'SELECT ({value}::numeric)::float8',f'SELECT float8abs({value}::numeric)',f'UPDATE public.pg06_020 SET v={value}::numeric WHERE id=1',f'INSERT INTO public.pg06_020 VALUES (2,{value}::numeric)']:
            query(sql,error='22003')
            query('SELECT 1::int4',('1',23))
            query('SELECT v FROM public.pg06_020 WHERE id=1',(None,701))
            assert query('SELECT count(*) FROM public.pg06_020 WHERE id=2')['rows']==[['0']]
    query('DROP TABLE public.pg06_020, public.pg06_020_source')
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:
        raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:
            json.dump(records,out,indent=2)
