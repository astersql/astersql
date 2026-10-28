# Copyright 2026 AsterSQL.
"""Replay task 77 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    def check(result,expected,fmt=0,mod=-1):
        assert result['metadata']==[[1083,8,mod,fmt]],result
        assert result['rows']==[[expected]],result
    cases=[('2000-01-01 00:00:00','00:00:00'),('1999-12-31 23:59:59.999999','23:59:59.999999'),('2024-02-29 12:34:56.123456','12:34:56.123456'),('4713-11-24 23:59:59.999999 BC','23:59:59.999999'),('294276-12-31 23:59:59.999999','23:59:59.999999'),('2000-01-01 24:00:00','00:00:00'),('infinity',None),('-infinity',None),(None,None)]
    query("SET TIME ZONE 'UTC'")
    query('CREATE TEMP TABLE pg06_077 (id int PRIMARY KEY,v time(3))')
    query('INSERT INTO pg06_077 VALUES (1,NULL)')
    send(b'P',b'explicit\0SELECT $1::time AS v\0'+struct.pack('!hI',1,1184));assert flush_read()==(b'1',b'')
    send(b'D',b'Sexplicit\0');send(b'H');r=record('Describe S','explicit',[read(),read()]);assert r['parameters']==[1184] and r['metadata']==[[1083,8,-1,0]],r
    recover()
    def bind(value,fmt=0,inputfmt=0):
        data=value.encode() if isinstance(value,str) else value
        payload=struct.pack('!i',-1) if data is None else struct.pack('!i',len(data))+data
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,inputfmt,1)+payload+struct.pack('!hh',1,fmt));assert flush_read()==(b'2',b'')
    def micros(time):
        h,m,s=time.split(':');return round((int(h)*3600+int(m)*60+float(s))*1000000)
    for zone in ['UTC','Etc/GMT-8','America/New_York']:
        query(f"SET TIME ZONE '{zone}'")
        for value,expected in cases:
            if zone!='UTC' and value and ('4713-' in value or '294276-' in value):continue
            literal="'"+value+"'" if value is not None else 'NULL'
            check(query(f'SELECT ({literal}::timestamptz)::time AS v'),None if expected is None else expected.encode().hex())
            for fmt in [0,1]:
                bind(value,fmt)
                send(b'D',b'P\0');r=record('Describe P','explicit',[flush_read()]);assert r['metadata']==[[1083,8,-1,fmt]],r
                send(b'E',b'\0'+struct.pack('!i',0));send(b'H');r=record('Execute',value,[read(),read()]);assert r['tags']=='DC',r
                wanted=None if expected is None else (expected.encode().hex() if fmt==0 else struct.pack('!q',micros(expected)).hex())
                assert r['rows']==[[wanted]],r;recover()
            for sql in [f'UPDATE pg06_077 SET v={literal}::timestamptz WHERE id=1',f'INSERT INTO pg06_077 SELECT 2,{literal}::timestamptz']:
                r=query(sql);assert r['tags']=='CZ',r
            r=query('SELECT v FROM pg06_077 ORDER BY id');assert r['metadata']==[[1083,8,3,0]],r
            rounded=None
            if expected is not None:
                us=((micros(expected)+500)//1000)*1000
                h,us=divmod(us,3600000000);m,us=divmod(us,60000000);sec,us=divmod(us,1000000)
                rounded=f'{h:02}:{m:02}:{sec:02}'+(f'.{us:06}'.rstrip('0') if us else '')
            assert r['rows']==[[None if rounded is None else rounded.encode().hex()]]*2,r
            query('DELETE FROM pg06_077 WHERE id=2')
    for zone,expected in [('UTC','23:30:00'),('Etc/GMT-8','07:30:00'),('America/New_York','18:30:00')]:
        query(f"SET TIME ZONE '{zone}'")
        bind('1999-12-31 23:30:00+00');send(b'E',b'\0'+struct.pack('!i',0));send(b'H');r=record('Execute','timezone change',[read(),read()]);assert r['tags']=='DC' and r['rows']==[[expected.encode().hex()]],r;recover()
        for sql in ["UPDATE pg06_077 SET v='1999-12-31 23:30:00+00'::timestamptz WHERE id=1","INSERT INTO pg06_077 SELECT 2,'1999-12-31 23:30:00+00'::timestamptz"]:
            assert query(sql)['tags']=='CZ'
        assert query('SELECT v FROM pg06_077 ORDER BY id')['rows']==[[expected.encode().hex()]]*2
        query('DELETE FROM pg06_077 WHERE id=2')
    for value,expected in [('2024-03-10 06:59:59+00','01:59:59'),('2024-03-10 07:00:00+00','03:00:00'),('2024-11-03 05:30:00+00','01:30:00'),('2024-11-03 06:30:00+00','01:30:00')]:
        check(query(f"SELECT ('{value}'::timestamptz)::time AS v"),expected.encode().hex())
    query("SET TIME ZONE 'UTC'")
    for value,expected in [(-1,'23:59:59.999999'),(0,'00:00:00'),(-2**63,None),(2**63-1,None)]:
        bind(struct.pack('!q',value),inputfmt=1);send(b'E',b'\0'+struct.pack('!i',0));send(b'H');r=record('Execute','binary source',[read(),read()]);assert r['tags']=='DC' and r['rows']==[[None if expected is None else expected.encode().hex()]],r;recover()
    for value in ['2000-01-01 23:59:59.999500','1999-12-31 23:59:59.999500']:
        check(query(f"SELECT ('{value}'::timestamptz)::time(3) AS v"),b'24:00:00'.hex(),mod=3)
    for value,expected in [('2000-01-01 23:59:59.999500','00:00:00'),('1999-12-31 23:59:59.999500','23:59:59.999')]:
        check(query(f"SELECT ('{value}'::timestamptz(3))::time AS v"),expected.encode().hex())
    r=query('SELECT (NULL::timestamptz)::time AS v LIMIT 0');assert r['tags']=='TCZ' and r['metadata']==[[1083,8,-1,0]],r
    original=query('SELECT v FROM pg06_077')
    for value,state in [('garbage','22007'),('2023-02-29 00:00:00','22008')]:
        data=value.encode();send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,0,1)+struct.pack('!i',len(data))+data+struct.pack('!h',0));r=record('Bind',value,[flush_read()]);assert r['errors']==[state] and not r['rows'],r;recover()
        for sql in [f"SELECT ('{value}'::timestamptz)::time",f"UPDATE pg06_077 SET v='{value}'::timestamptz WHERE id=1",f"INSERT INTO pg06_077 VALUES (3,'{value}'::timestamptz)"]:
            r=query(sql);assert r['tags']=='EZ' and r['errors']==[state] and not r['rows'],r;recover();assert query('SELECT v FROM pg06_077')==original
    send(b'P',b'implicit\0SELECT coalesce(NULL::time,NULL::timestamptz)\0'+struct.pack('!h',0));r=record('Parse','implicit',[flush_read()]);assert r['errors']==['42846'],r;recover()
    query('CREATE TEMP TABLE pg06_077_source (id int PRIMARY KEY,v timestamptz)')
    query("INSERT INTO pg06_077_source VALUES (1,'1999-12-31 23:59:59.123456'::timestamp)")
    check(query('SELECT v::time AS v FROM pg06_077_source'),b'23:59:59.123456'.hex())
    query("SET TIME ZONE 'Etc/GMT-8'")
    check(query("SELECT ('294276-12-31 23:59:59.999999+00'::timestamptz)::time AS v"),b'07:59:59.999999'.hex())
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:json.dump(records,out,indent=2,ensure_ascii=False)
