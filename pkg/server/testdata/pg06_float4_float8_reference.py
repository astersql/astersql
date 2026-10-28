# Copyright 2026 AsterSQL.
"""Replay task 25 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    sql = sql.replace('public.pg06_025', 'pg_temp.pg06_025')
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
        assert result['metadata'] == [[expected[1], {21: 2, 23: 4, 20: 8, 1700: -1, 701: 8}[expected[1]], -1, 0]], result
    return result

def flush_read():
    send(b'H')
    return read()

def sync():
    send(b'S')
    assert read() == (b'Z', b'I')

def parse(name, sql, oid=700, error=None):
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
        assert result['parameters'] == [700], result

def bind(name, value, error=None):
    data = struct.pack('!i', -1) if value is None else struct.pack('!i', len(value)) + value.encode()
    send(b'B', b'\0' + name.encode() + b'\0' + struct.pack('!hh', 0, 1) + data + struct.pack('!h', 0))
    message = flush_read()
    result = record('Bind', name + ':' + str(value), [message])
    if error:
        assert result['errors'] == [error], result
        sync()
        query('SELECT 1::int4', ('1', 23))
        assert query('SELECT v FROM public.pg06_025 WHERE id=1')['rows'] == [[None if name == 'assign' else '16777216']]
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
    cases = [('1','1'),('1.234565','1.234565019607544'),('1.234564','1.234563946723938'),('16777217','16777216'),('16777219','16777220'),('3.4028235e38','3.4028234663852886e+38'),('-3.4028235e38','-3.4028234663852886e+38'),('1.40129846e-45','1.401298464324817e-45'),('NaN','NaN'),('Infinity','Infinity'),('-Infinity','-Infinity'),(None,None)]
    for value,expected in cases:
        source='NULL' if value is None else "'"+value+"'"
        query(f'SELECT ({source}::float4)::float8 AS v',(expected,701))
        query(f'SELECT float8abs({source}::float4) AS v',(expected.lstrip('-') if expected else None,701))
    query('CREATE TEMP TABLE pg06_025 (id INT PRIMARY KEY,v float8)')
    query('INSERT INTO public.pg06_025 VALUES (1,16777217::float4)')
    query('CREATE TEMP TABLE pg06_025_source (id INT PRIMARY KEY,n float4)')
    query('INSERT INTO public.pg06_025_source VALUES (1,1.234565::float4)')
    query('INSERT INTO public.pg06_025 SELECT 3,n FROM public.pg06_025_source')
    for ident,expected in [(1,'16777216'),(3,'1.234565019607544')]: query(f'SELECT v FROM public.pg06_025 WHERE id={ident}',(expected,701))
    errors=[('3.4028236e38','22003'),('-3.4028236e38','22003'),('1e-46','22003'),('-1e-46','22003'),('invalid','22P02')]
    for name,sql in [('explicit','SELECT $1::float8 AS v'),('implicit','SELECT float8abs($1) AS v'),('assign','UPDATE pg_temp.pg06_025 SET v=$1 WHERE id=1')]:
        parse(name,sql)
        if name=='assign':
            send(b'D',b'Sassign\0'); send(b'H')
            result=record('Describe S',name,[read(),read()])
            assert result['tags']=='tn' and result['parameters']==[700] and not result['metadata'],result
        else: describe(b'S',name)
        for value,expected in cases:
            if name=='implicit' and expected: expected=expected.lstrip('-')
            bind(name,value)
            if name!='assign': execute(name,expected)
            else:
                send(b'D',b'P\0'); assert flush_read()==(b'n',b'')
                send(b'E',b'\0'+struct.pack('!i',0))
                result=record('Execute',name,[flush_read()])
                assert result['tags']=='C' and not result['rows'] and not result['errors'],result
                sync()
                query('SELECT v FROM public.pg06_025 WHERE id=1',(expected,701))
            query('SELECT 1::int4',('1',23))
        for value,code in errors: bind(name,value,error=code)
    for value in [1.234565,None]:
        source=None if value is None else struct.pack('!f',value)
        payload=struct.pack('!i',-1) if source is None else struct.pack('!i',4)+source
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,1,1)+payload+struct.pack('!hh',1,1))
        assert flush_read()==(b'2',b'')
        send(b'D',b'P\0')
        result=record('Describe P binary','explicit',[flush_read()])
        assert result['metadata']==[[701,8,-1,1]],result
        send(b'E',b'\0'+struct.pack('!i',0))
        message=flush_read()
        expected=struct.pack('!hi',1,-1) if source is None else struct.pack('!hi',1,8)+struct.pack('!d',struct.unpack('!f',source)[0])
        assert message==(b'D',expected),message
        assert read()[0]==b'C'
        records.append(dict(phase='Execute binary',sql='explicit',payload=message[1].hex()))
        sync()
        query('SELECT 1::int4',('1',23))
    for value,code in errors:
        for sql in [f"SELECT ('{value}'::float4)::float8",f"SELECT float8abs('{value}'::float4)",f"UPDATE public.pg06_025 SET v='{value}'::float4 WHERE id=1",f"INSERT INTO public.pg06_025 VALUES (2,'{value}'::float4)"]:
            query(sql,error=code)
            query('SELECT 1::int4',('1',23))
            query('SELECT v FROM public.pg06_025 WHERE id=1',(None,701))
            assert query('SELECT count(*) FROM public.pg06_025 WHERE id=2')['rows']==[['0']]
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw: raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out: json.dump(records,out,indent=2)
