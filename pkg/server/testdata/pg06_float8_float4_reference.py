# Copyright 2026 AsterSQL.
"""Replay task 30 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    sql = sql.replace('public.pg06_030', 'pg_temp.pg06_030')
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
        assert result['metadata'] == [[expected[1], {21: 2, 23: 4, 20: 8, 1700: -1, 700: 4}[expected[1]], -1, 0]], result
    return result

def flush_read():
    send(b'H')
    return read()

def sync():
    send(b'S')
    assert read() == (b'Z', b'I')

def parse(name, sql, oid=701, error=None):
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
        assert result['parameters'] == [701], result

def bind(name, value, error=None):
    data = struct.pack('!i', -1) if value is None else struct.pack('!i', len(value)) + value.encode()
    send(b'B', b'\0' + name.encode() + b'\0' + struct.pack('!hh', 0, 1) + data + struct.pack('!h', 0))
    message = flush_read()
    result = record('Bind', name + ':' + str(value), [message])
    if error:
        assert result['errors'] == [error], result
        sync()
        query('SELECT 1::int4', ('1', 23))
        assert query('SELECT v FROM public.pg06_030 WHERE id=1')['rows'] == [[None if name == 'assign' else '1']]
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
    cases=[('1','1'),('1.0000000596046448','1'),('1.000000059604645','1.0000001'),('16777217','1.6777216e+07'),('16777219','1.677722e+07'),('3.4028235e38','3.4028235e+38'),('-3.4028235e38','-3.4028235e+38'),('1.40129846e-45','1e-45'),('NaN','NaN'),('Infinity','Infinity'),('-Infinity','-Infinity'),(None,None)]
    query('CREATE TEMP TABLE pg06_030 (id INT PRIMARY KEY,v float4)')
    query('INSERT INTO public.pg06_030 VALUES (1,1::float8)')
    query('CREATE TEMP TABLE pg06_030_source (id INT PRIMARY KEY,n float8)')
    query('INSERT INTO public.pg06_030_source VALUES (1,1.0000000596046448::float8)')
    query('INSERT INTO public.pg06_030 SELECT 3,n FROM public.pg06_030_source')
    query('SELECT v FROM public.pg06_030 WHERE id=3',('1',700))
    for value,expected in cases:
        source='NULL' if value is None else "'"+value+"'"
        query(f'SELECT ({source}::float8)::float4 AS v',(expected,700))
    for sql in ['SELECT float4abs(1::float8)','SELECT float4abs(NULL::float8)','SELECT float4abs(n) FROM pg_temp.pg06_030_source']:
        parse('reject',sql,error='42883')
    parse('reject_parameter','SELECT float4abs($1)',error='42883')
    for name,sql in [('explicit','SELECT $1::float4 AS v'),('assign','UPDATE pg_temp.pg06_030 SET v=$1 WHERE id=1')]:
        parse(name,sql)
        if name=='explicit': describe(b'S',name)
        else:
            send(b'D',b'Sassign\0');send(b'H')
            result=record('Describe S',name,[read(),read()])
            assert result['tags']=='tn' and result['parameters']==[701] and not result['metadata'],result
        for value,expected in cases:
            bind(name,value)
            if name=='explicit': execute(name,expected)
            else:
                send(b'D',b'P\0');assert flush_read()==(b'n',b'')
                send(b'E',b'\0'+struct.pack('!i',0))
                result=record('Execute',name,[flush_read()]);assert result['tags']=='C',result
                sync();query('SELECT v FROM public.pg06_030 WHERE id=1',(expected,700))
            query('SELECT 1::int4',('1',23))
        # Invalid source text fails at Bind, target range failures at Execute.
        bind(name,'invalid',error='22P02')
        for value in ['3.4028236e38','-3.4028236e38','1e-46','-1e-46']:
            bind(name,value)
            send(b'E',b'\0'+struct.pack('!i',0))
            result=record('Execute',name,[flush_read()]);assert result['tags']=='E' and result['errors']==['22003'] and not result['rows'],result
            sync();query('SELECT 1::int4',('1',23))
            query('SELECT v FROM public.pg06_030 WHERE id=1',(None if name=='assign' else '1',700))
    for value in ['3.4028236e38','-3.4028236e38','1e-46','-1e-46','invalid']:
        for sql in [f"SELECT ('{value}'::float8)::float4",f"UPDATE public.pg06_030 SET v='{value}'::float8 WHERE id=1",f"INSERT INTO public.pg06_030 VALUES (2,'{value}'::float8)"]:
            query(sql,error='22P02' if value=='invalid' else '22003')
            query('SELECT 1::int4',('1',23));query('SELECT v FROM public.pg06_030 WHERE id=1',(None,700))
            assert query('SELECT count(*) FROM public.pg06_030 WHERE id=2')['rows']==[['0']]
    for value in [1.0000000596046448,1.000000059604645,None]:
        payload=struct.pack('!i',-1) if value is None else struct.pack('!i',8)+struct.pack('!d',value)
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,1,1)+payload+struct.pack('!hh',1,1));assert flush_read()==(b'2',b'')
        send(b'D',b'P\0');result=record('Describe P binary','explicit',[flush_read()]);assert result['metadata']==[[700,4,-1,1]],result
        send(b'E',b'\0'+struct.pack('!i',0));message=flush_read()
        expected=struct.pack('!hi',1,-1) if value is None else struct.pack('!hi',1,4)+struct.pack('!f',value)
        assert message==(b'D',expected),message
        assert read()[0]==b'C';records.append(dict(phase='Execute binary',payload=message[1].hex()));sync();query('SELECT 1::int4',('1',23))
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw: raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out: json.dump(records,out,indent=2)
