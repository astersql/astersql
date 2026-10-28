# Copyright 2026 AsterSQL.
"""Replay task 59 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
                row.append(None if size == -1 else body[offset:offset + size].hex())
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


def query(sql):
    send(b'Q',sql.encode()+b'\0')
    messages=[]
    while True:
        m=read();messages.append(m)
        if m[0]==b'Z':break
    return record('Query',sql,messages)

def flush_read():
    send(b'H');return read()

def recover():
    send(b'S');assert read()==(b'Z',b'I')
    result=query('SELECT 1::int4');assert result['rows']==[['31']],result


try:
    assert server_version(conn)==180006 and lib_version()==180006
    raw=socket.socket(fileno=os.dup(fd(conn)));raw.settimeout(10)
    query("SET TIME ZONE 'UTC'")
    cases=[('0','0'),('-0','-0'),('123.4500','123.45'),('1.2345678901234567','1.2345678901234567'),('1e-20','1e-20'),('1e20','1e+20'),('5e-324','5e-324'),('1.7976931348623157e308','1.7976931348623157e+308'),('NaN','NaN'),('Infinity','Infinity'),('-Infinity','-Infinity'),(None,None)]
    for value,expected in cases:
        literal="'"+value+"'" if value is not None else 'NULL'
        result=query(f'SELECT ({literal}::float8)::text AS v')
        assert result['metadata']==[[25,-1,-1,0]] and result['rows']==[[None if expected is None else expected.encode().hex()]],result
    send(b'P',b'explicit\0SELECT $1::text AS v\0'+struct.pack('!hI',1,701));assert flush_read()==(b'1',b'')
    send(b'D',b'Sexplicit\0');send(b'H');result=record('Describe S','explicit',[read(),read()]);assert result['parameters']==[701] and result['metadata']==[[25,-1,-1,0]],result
    for fmt in [0,1]:
        for value,expected in cases:
            value=None if value is None else struct.pack('!d',float(value)) if fmt else value.encode()
            payload=struct.pack('!i',-1) if value is None else struct.pack('!i',len(value))+value
            send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,fmt,1)+payload+struct.pack('!hh',1,fmt));assert flush_read()==(b'2',b'')
            send(b'D',b'P\0');result=record('Describe P','explicit',[flush_read()]);assert result['metadata']==[[25,-1,-1,fmt]],result
            send(b'E',b'\0'+struct.pack('!i',0));send(b'H');result=record('Execute','explicit',[read(),read()])
            assert result['tags']=='DC' and result['rows']==[[None if expected is None else expected.encode().hex()]],result
            recover()
    for value,state,fmt in [(b'1x','22P02',0),(b'1e309','22003',0),(b'1e-400','22003',0)]:
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,fmt,1)+struct.pack('!i',len(value))+value+struct.pack('!h',0))
        result=record('Bind',value.hex(),[flush_read()]);assert result['errors']==[state] and not result['rows'],result;recover()
    for sql,expected,oid,mod in [
        ("SELECT (1.234::float8)::text AS v",'1.234',25,-1),
        ("SELECT (123.4500::float8)::varchar(4) AS v",'123.',1043,8),
        ("SELECT ((-42.5)::float8)::char(7) AS v",'-42.5  ',1042,11),
        ("SELECT ('[0:2]={-1.25,NULL,1e+20}'::float8[])::text AS v",'[0:2]={-1.25,NULL,1e+20}',25,-1),
    ]:
        result=query(sql);assert result['rows']==[[expected.encode().hex()]] and result['metadata']==[[oid,-1,mod,0]],result
    query('CREATE TEMP TABLE pg06_059 (id int PRIMARY KEY,v text)')
    for sql in ["INSERT INTO pg06_059 VALUES (1,'42.500'::float8)","UPDATE pg06_059 SET v='1.2345678901234567'::float8","INSERT INTO pg06_059 SELECT 2,'NaN'::float8","INSERT INTO pg06_059 VALUES (3,NULL::float8)"]:
        result=query(sql);assert result['tags']=='CZ' and not result['errors'],result
    result=query('SELECT v FROM pg06_059 ORDER BY id');assert result['rows']==[['1.2345678901234567'.encode().hex()],['NaN'.encode().hex()],[None]] and result['metadata']==[[25,-1,-1,0]],result
    original = result
    for sql in ["UPDATE pg06_059 SET v='1x'::float8 WHERE id=1", "INSERT INTO pg06_059 VALUES (4,'1x'::float8)"]:
        result=query(sql);assert result['errors']==['22P02'] and result['tags']=='EZ' and not result['rows'],result
        recover()
        result=query('SELECT v FROM pg06_059 ORDER BY id');assert result==original,result
    for value,expected in cases:
        literal="'"+value+"'" if value is not None else 'NULL'
        result=query(f'UPDATE pg06_059 SET v={literal}::float8 WHERE id=1');assert result['tags']=='CZ',result
        result=query('SELECT v FROM pg06_059 WHERE id=1');assert result['metadata']==[[25,-1,-1,0]] and result['rows']==[[None if expected is None else expected.encode().hex()]],result
    for sql in ["SELECT coalesce(42::float8,'42'::text)",'SELECT coalesce(NULL::float8,NULL::text) LIMIT 0','SELECT coalesce(NULL::text,NULL::float8) LIMIT 0']:
        send(b'P',b'bad\0'+sql.encode()+b'\0'+struct.pack('!h',0));result=record('Parse',sql,[flush_read()]);assert result['errors']==['42804'] and not result['rows'],result;recover()
    result=query('SELECT (NULL::float8)::text AS v LIMIT 0');assert result['tags']=='TCZ' and result['metadata']==[[25,-1,-1,0]],result
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:json.dump(records,out,indent=2,ensure_ascii=False)
