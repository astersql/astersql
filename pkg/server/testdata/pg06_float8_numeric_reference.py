# Copyright 2026 AsterSQL.
"""Replay task 29 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    sql = sql.replace('public.pg06_029', 'pg_temp.pg06_029')
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
        assert result['metadata'] == [[expected[1], {21: 2, 23: 4, 20: 8, 1700: -1}[expected[1]], -1, 0]], result
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
    assert result['metadata'] == [[1700, -1, -1, 0]], result
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
        assert query('SELECT v FROM public.pg06_029 WHERE id=1')['rows'] == [['1.23']]
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
    query('CREATE TEMP TABLE pg06_029 (id INT PRIMARY KEY, v numeric(5,2))')
    query('INSERT INTO public.pg06_029 VALUES (1,1.234565::float8)')
    cases = [('1','1'), ('1.234567890123456','1.23456789012346'), ('1.234567890123454','1.23456789012345'), ('9007199254740991','9007199254740990'), ('1e100','1'+'0'*100), ('1e-100','0.'+'0'*99+'1'), ('NaN','NaN'), ('Infinity','Infinity'), ('-Infinity','-Infinity'), (None,None)]
    for value, expected in cases:
        query('SELECT '+('NULL' if value is None else "'"+value+"'")+'::float8::numeric AS v', (expected,1700))
    parse('narrow','SELECT $1::numeric AS v')
    describe(b'S','narrow')
    for value, expected in cases:
        bind('narrow',value)
        execute('narrow',expected)
    bind('narrow','invalid',error='22P02')
    for sql in ['SELECT numeric_abs(1::float8)', 'SELECT numeric_abs(NULL::float8)']:
        parse('reject',sql,error='42883')
    parse('reject_parameter','SELECT numeric_abs($1)',error='42883')
    query('SELECT numeric_abs((-1.25)::numeric)', ('1.25',1700))
    query('SELECT numeric_abs(NULL::numeric)', (None,1700))
    for value, expected in [('999.994','999.99'),('-999.994','-999.99'),('1.235','1.24'),('-1.235','-1.24')]:
        query(f'UPDATE public.pg06_029 SET v={value}::float8 WHERE id=1')
        assert query('SELECT v FROM public.pg06_029 WHERE id=1')['rows']==[[expected]]
    query('UPDATE public.pg06_029 SET v=1.23::float8 WHERE id=1')
    for value in ['999.995','-999.995','999.9949999999999','-999.9949999999999','Infinity','-Infinity']:
        for sql in [f'SELECT ({value if value not in ["Infinity","-Infinity"] else chr(39)+value+chr(39)}::float8)::numeric(5,2)', f"UPDATE public.pg06_029 SET v='{value}'::float8 WHERE id=1", f"INSERT INTO public.pg06_029 VALUES (2,'{value}'::float8)"]:
            query(sql,error='22003')
            query('SELECT 1::int4',('1',23))
            assert query('SELECT v FROM public.pg06_029 WHERE id=1')['rows']==[['1.23']]
            assert query('SELECT count(*) FROM public.pg06_029 WHERE id=2')['rows']==[['0']]
    query('CREATE TEMP TABLE pg06_029_source (id INT PRIMARY KEY,n float8)')
    query('INSERT INTO public.pg06_029_source VALUES (1,1.234567890123456::float8)')
    query('SELECT n::numeric FROM public.pg06_029_source',('1.23456789012346',1700))
    query('INSERT INTO public.pg06_029 SELECT 3,n FROM public.pg06_029_source')
    result=query('SELECT v FROM public.pg06_029 WHERE id=3')
    assert result['rows']==[['1.23']] and result['metadata']==[[1700,-1,327686,0]],result
    parse('assign','UPDATE pg_temp.pg06_029 SET v=$1 WHERE id=1')
    send(b'D',b'Sassign\0')
    send(b'H')
    result=record('Describe S','assign',[read(),read()])
    assert result['tags']=='tn' and result['parameters']==[701] and not result['metadata'],result
    for value,expected in [('999.994','999.99'),('-999.994','-999.99'),('1.235','1.24'),('-1.235','-1.24'),(None,None),('1.23','1.23'),('999.995','1.23'),('-999.995','1.23'),('Infinity','1.23'),('-Infinity','1.23'),('999.9949999999999','1.23'),('-999.9949999999999','1.23')]:
        bind('assign',value)
        send(b'E',b'\0'+struct.pack('!i',0))
        result=record('Execute','assign',[flush_read()])
        error=value in ['999.995','-999.995','999.9949999999999','-999.9949999999999','Infinity','-Infinity']
        assert result['tags']==('E' if error else 'C') and not result['rows'],result
        assert result['errors']==(['22003'] if error else []),result
        sync()
        query('SELECT 1::int4',('1',23))
        result=query('SELECT v FROM public.pg06_029 WHERE id=1')
        assert result['rows']==[[expected]] and result['metadata']==[[1700,-1,327686,0]],result
    query('UPDATE public.pg06_029 SET v=NULL::float8 WHERE id=1')
    assert query('SELECT v FROM public.pg06_029 WHERE id=1')['rows']==[[None]]
    print(json.dumps(dict(passed=True, server_version=server_version(conn), libpq=lib_version(), observations=len(records))))
finally:
    if raw:
        raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:
            json.dump(records,out,indent=2)
