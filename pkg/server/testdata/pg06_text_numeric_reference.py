# Copyright 2026 AsterSQL.
"""Replay task 44 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    cases=[('1.25','1.25'),(' +001.250 ','1.250'),('1e-2','0.01'),('10000000000000000000000000000000000000000','10000000000000000000000000000000000000000'),('NaN','NaN'),('Infinity','Infinity'),('-Infinity','-Infinity'),('0','0'),(None,None)]
    binary={'1.25':'0002000000000002000109c4','1.250':'0002000000000003000109c4','0.01':'0001ffff000000020064','10000000000000000000000000000000000000000':'0001000a000000000001','NaN':'00000000c0000000','Infinity':'00000000d0000020','-Infinity':'00000000f0000020','0':'0000000000000000'}
    for value,expected in cases:
        literal="'"+value+"'" if value is not None else 'NULL'
        result=query(f'SELECT ({literal}::text)::numeric AS v')
        assert result['metadata']==[[1700,-1,-1,0]] and result['rows']==[[None if expected is None else expected.encode().hex()]],result
    send(b'P',b'explicit\0SELECT ($1::text)::numeric AS v\0'+struct.pack('!hI',1,25));assert flush_read()==(b'1',b'')
    send(b'D',b'Sexplicit\0');send(b'H');result=record('Describe S','explicit',[read(),read()]);assert result['parameters']==[25] and result['metadata']==[[1700,-1,-1,0]],result
    for fmt in [0,1]:
        for value,expected in cases:
            payload=struct.pack('!i',-1) if value is None else struct.pack('!i',len(value.encode()))+value.encode()
            send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,fmt,1)+payload+struct.pack('!hh',1,fmt));assert flush_read()==(b'2',b'')
            send(b'D',b'P\0');result=record('Describe P','explicit',[flush_read()]);assert result['metadata']==[[1700,-1,-1,fmt]],result
            send(b'E',b'\0'+struct.pack('!i',0));send(b'H');result=record('Execute','explicit',[read(),read()])
            expected_bytes=None if expected is None else (expected.encode() if fmt==0 else bytes.fromhex(binary[expected]))
            assert result['tags']=='DC' and result['rows']==[[None if expected_bytes is None else expected_bytes.hex()]],result
            recover()
    for value,state in [('1z','22P02'),('1.25z','22P02'),('','22P02'),('1e131072','22003'),('1e-16384','22003')]:
        data=value.encode();send(b'B',b'\0explicit\0'+struct.pack('!hh',0,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0));assert flush_read()==(b'2',b'')
        send(b'E',b'\0'+struct.pack('!i',0));result=record('Execute',value,[flush_read()]);assert result['errors']==[state] and not result['rows'],result;recover()
    result=query('SELECT (NULL::text)::numeric AS v LIMIT 0');assert result['tags']=='TCZ' and result['metadata']==[[1700,-1,-1,0]],result
    query('CREATE TEMP TABLE pg06_044_src (v text)')
    query("INSERT INTO pg06_044_src VALUES ('1'::text)")
    result=query('SELECT v::numeric FROM pg06_044_src');assert result['metadata']==[[1700,-1,-1,0]] and result['rows']==[['31']],result
    query('CREATE TEMP TABLE pg06_044 (v numeric)')
    for sql,state in [("INSERT INTO pg06_044 VALUES ('1'::text)",'42804'),("SELECT coalesce(NULL::numeric,'1'::text)",'42804'), ("SELECT coalesce(NULL::numeric,NULL::text) LIMIT 0",'42804'), ("UPDATE pg06_044 SET v=NULL::text",'42804'), ("INSERT INTO pg06_044 SELECT '1'::text",'42804')]:
        send(b'P',b'bad\0'+sql.encode()+b'\0'+struct.pack('!h',0));result=record('Parse',sql,[flush_read()]);assert result['errors']==[state],result;recover()
    result=query('SELECT v FROM pg06_044');assert result['rows']==[],result
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:json.dump(records,out,indent=2,ensure_ascii=False)
