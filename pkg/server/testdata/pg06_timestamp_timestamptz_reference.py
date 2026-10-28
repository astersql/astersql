# Copyright 2026 AsterSQL.
"""Replay task 72 on the fixed PG18_REFERENCE_DSN; never owns the server."""
import ctypes as c
import os
import socket
import struct
import sys
import json

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
    cases=[('2000-01-01 00:00:00','2000-01-01 00:00:00+00'),('2024-02-29 00:00:00','2024-02-29 00:00:00+00'),('4713-11-24 00:00:00 BC','4713-11-24 00:00:00+00 BC'),('294276-12-31 00:00:00','294276-12-31 00:00:00+00'),('infinity','infinity'),('-infinity','-infinity'),(None,None)]
    def check(result,expected,fmt=0,mod=-1):
        assert result['metadata']==[[1184,8,mod,fmt]],result
        assert result['rows']==[[expected]],result
    for zone in ['UTC','Etc/GMT-8']:
        query(f"SET TIME ZONE '{zone}'")
        for value,expected in cases:
            literal="'"+value+"'" if value is not None else 'NULL'
            check(query(f'SELECT ({literal}::timestamp)::timestamptz AS v'),None if expected is None else (expected if zone=='UTC' else expected.replace('+00','+08')).encode().hex())
    query("SET TIME ZONE 'UTC'")
    send(b'P',b'explicit\0SELECT $1::timestamptz(3) AS v\0'+struct.pack('!hI',1,1114));assert flush_read()==(b'1',b'')
    send(b'D',b'Sexplicit\0');send(b'H');r=record('Describe S','explicit',[read(),read()]);assert r['parameters']==[1114] and r['metadata']==[[1184,8,3,0]],r
    binaries=[0,762480000000000,-211781865600000000,9223371244800000000,2**63-1,-2**63,None]
    # Calendar boundary binary expectations come from the reference epoch arithmetic.
    for fmt in [0,1]:
        for (value,expected),binary in zip(cases,binaries):
            data=None if value is None else value.encode()
            payload=struct.pack('!i',-1) if data is None else struct.pack('!i',len(data))+data
            send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,0,1)+payload+struct.pack('!hh',1,fmt));assert flush_read()==(b'2',b'')
            send(b'D',b'P\0');r=record('Describe P','explicit',[flush_read()]);assert r['metadata']==[[1184,8,3,fmt]],r
            send(b'E',b'\0'+struct.pack('!i',0));send(b'H');r=record('Execute',value,[read(),read()]);assert r['tags']=='DC',r
            wanted=None if expected is None else (expected.encode().hex() if fmt==0 else struct.pack('!q',binary).hex())
            assert r['rows']==[[wanted]],r
            recover()
    for zone,micros in [('UTC',0),('Asia/Shanghai',-28800000000),('America/New_York',18000000000)]:
        query(f"SET TIME ZONE '{zone}'")
        value=b'2000-01-01 00:00:00'
        send(b'B',b'\0explicit\0'+struct.pack('!hh',0,1)+struct.pack('!i',len(value))+value+struct.pack('!hh',1,1));assert flush_read()==(b'2',b'')
        send(b'D',b'P\0');r=record('Describe P',zone,[flush_read()]);assert r['metadata']==[[1184,8,3,1]],r
        send(b'E',b'\0'+struct.pack('!i',0));send(b'H');r=record('Execute',zone,[read(),read()]);assert r['tags']=='DC' and r['rows']==[[struct.pack('!q',micros).hex()]],r
        recover()
    query("SET TIME ZONE 'UTC'")
    for value,state in [(b'garbage','22007'),(b'2023-02-29 00:00:00','22008')]:
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,0,1)+struct.pack('!i',len(value))+value+struct.pack('!h',0));r=record('Bind',value.decode(),[flush_read()]);assert r['errors']==[state] and not r['rows'],r;recover()
    query('CREATE TEMP TABLE pg06_072 (id int PRIMARY KEY,v timestamptz)')
    for sql in ["INSERT INTO pg06_072 VALUES (1,'2024-02-29 00:00:00'::timestamp)","INSERT INTO pg06_072 SELECT 2,NULL::timestamp","UPDATE pg06_072 SET v='2000-01-01 00:00:00'::timestamp WHERE id=1"]:
        r=query(sql);assert r['tags']=='CZ',r
    original=query('SELECT v FROM pg06_072 ORDER BY id');assert original['metadata']==[[1184,8,-1,0]] and original['rows']==[[b'2000-01-01 00:00:00+00'.hex()],[None]],original
    for sql in ["UPDATE pg06_072 SET v='garbage'::timestamp WHERE id=1","INSERT INTO pg06_072 VALUES (3,'garbage'::timestamp)"]:
        r=query(sql);assert r['tags']=='EZ' and r['errors']==['22007'] and not r['rows'],r;recover();assert query('SELECT v FROM pg06_072 ORDER BY id')==original
    query("SET TIME ZONE 'Etc/GMT-8'")
    r=query("UPDATE pg06_072 SET v='2000-01-01 00:00:00'::timestamp WHERE id=1");assert r['tags']=='CZ',r
    check(query('SELECT v FROM pg06_072 WHERE id=1'),b'2000-01-01 00:00:00+08'.hex())
    query("SET TIME ZONE 'UTC'")
    check(query('SELECT v FROM pg06_072 WHERE id=1'),b'1999-12-31 16:00:00+00'.hex())
    for value,expected in cases:
        literal="'"+value+"'" if value is not None else 'NULL'
        r=query(f'UPDATE pg06_072 SET v={literal}::timestamp WHERE id=1');assert r['tags']=='CZ',r
        check(query('SELECT v FROM pg06_072 WHERE id=1'),None if expected is None else expected.encode().hex())
    for value in [struct.pack('!q',0),None]:
        payload=struct.pack('!i',-1) if value is None else struct.pack('!i',len(value))+value
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,1,1)+payload+struct.pack('!h',0));assert flush_read()==(b'2',b'')
        send(b'E',b'\0'+struct.pack('!i',0));send(b'H');r=record('Execute','binary timestamp',[read(),read()]);assert r['tags']=='DC' and r['rows']==[[None if value is None else b'2000-01-01 00:00:00+00'.hex()]],r;recover()
    for sql,expected in [("SELECT coalesce(NULL::timestamptz,'2024-02-29 00:00:00'::timestamp) AS v",'2024-02-29 00:00:00+00'),('SELECT coalesce(NULL::timestamp,NULL::timestamptz) AS v',None)]:
        check(query(sql),None if expected is None else expected.encode().hex())
    r=query('SELECT (NULL::timestamp)::timestamptz(0) AS v LIMIT 0');assert r['tags']=='TCZ' and r['metadata']==[[1184,8,0,0]],r
    query('CREATE TEMP TABLE pg06_072_source (id int PRIMARY KEY,d timestamp)')
    query("INSERT INTO pg06_072_source VALUES (1,'2024-02-29 00:00:00')")
    check(query('SELECT d::timestamptz AS v FROM pg06_072_source'),b'2024-02-29 00:00:00+00'.hex())
    for zone,value,precision,expected in [
        ('UTC','1999-12-31 23:59:59.999500',3,'1999-12-31 23:59:59.999+00'),
        ('UTC','2000-01-01 23:59:59.999500',3,'2000-01-02 00:00:00+00'),
        ('Etc/GMT-8','2000-01-01 00:00:00.123456',6,'2000-01-01 00:00:00.123456+08'),
        ('America/New_York','2024-03-10 02:30:00',6,'2024-03-10 03:30:00-04'),
        ('America/New_York','2024-11-03 01:30:00',6,'2024-11-03 01:30:00-05')]:
        query(f"SET TIME ZONE '{zone}'")
        check(query(f"SELECT ('{value}'::timestamp)::timestamptz({precision}) AS v"),expected.encode().hex(),mod=precision)
        if precision==6:
            r=query(f"UPDATE pg06_072 SET v='{value}'::timestamp WHERE id=1");assert r['tags']=='CZ',r
            check(query('SELECT v FROM pg06_072 WHERE id=1'),expected.encode().hex())
            check(query(f"SELECT coalesce(NULL::timestamptz,'{value}'::timestamp) AS v"),expected.encode().hex())
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:json.dump(records,out,indent=2,ensure_ascii=False)
