# Copyright 2026 AsterSQL.
"""Replay task 65 on the fixed PG18_REFERENCE_DSN; never owns the server."""
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
    cases=[('1 year 2 mons 3 days 04:05:06.120000','1 year 2 mons 3 days 04:05:06.12'),('0 seconds','00:00:00'),('2147483647 days','2147483647 days'),('-2147483648 days','-2147483648 days'),('-1 day','-1 days'),('1.1234567 seconds','00:00:01.123457'),(None,None)]
    parts={'1 year 2 mons 3 days 04:05:06.120000':(14706120000,3,14),'0 seconds':(0,0,0),'2147483647 days':(0,2147483647,0),'-2147483648 days':(0,-2147483648,0),'-1 day':(0,-1,0),'1.1234567 seconds':(1123457,0,0)}
    for value,expected in cases:
        literal="'"+value+"'" if value is not None else 'NULL'
        result=query(f'SELECT ({literal}::interval)::text AS v')
        assert result['metadata']==[[25,-1,-1,0]] and result['rows']==[[None if expected is None else expected.encode().hex()]],result
    send(b'P',b'explicit\0SELECT $1::text AS v\0'+struct.pack('!hI',1,1186));assert flush_read()==(b'1',b'')
    send(b'D',b'Sexplicit\0');send(b'H');result=record('Describe S','explicit',[read(),read()]);assert result['parameters']==[1186] and result['metadata']==[[25,-1,-1,0]],result
    for fmt in [0,1]:
        for value,expected in cases:
            value=None if value is None else struct.pack('!qii',*parts[value]) if fmt else value.encode()
            payload=struct.pack('!i',-1) if value is None else struct.pack('!i',len(value))+value
            send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,fmt,1)+payload+struct.pack('!hh',1,fmt));assert flush_read()==(b'2',b'')
            send(b'D',b'P\0');result=record('Describe P','explicit',[flush_read()]);assert result['metadata']==[[25,-1,-1,fmt]],result
            send(b'E',b'\0'+struct.pack('!i',0));send(b'H');result=record('Execute','explicit',[read(),read()])
            assert result['tags']=='DC' and result['rows']==[[None if expected is None else expected.encode().hex()]],result
            recover()
    for value,state,fmt in [(b'garbage','22007',0),(b'2147483648 days','22015',0)]:
        send(b'B',b'\0explicit\0'+struct.pack('!hhh',1,fmt,1)+struct.pack('!i',len(value))+value+struct.pack('!h',0))
        result=record('Bind',value.hex(),[flush_read()]);assert result['errors']==[state] and not result['rows'],result;recover()
    for sql,expected,oid,mod in [
        ("SELECT ('1 year 2 mons 3 days 04:05:06.12'::interval)::text AS v",'1 year 2 mons 3 days 04:05:06.12',25,-1),
        ("SELECT ('1 year 2 mons 3 days 04:05:06.12'::interval)::varchar(6) AS v",'1 year',1043,10),
        ("SELECT ('-1 days'::interval)::char(9) AS v",'-1 days  ',1042,13),
        ("SELECT ('[0:2]={1 year 2 mons 3 days 04:05:06.12,NULL,-1 days}'::interval[])::text AS v",'[0:2]={"1 year 2 mons 3 days 04:05:06.12",NULL,"-1 days"}',25,-1),
    ]:
        result=query(sql);assert result['rows']==[[expected.encode().hex()]] and result['metadata']==[[oid,-1,mod,0]],result
    query('CREATE TEMP TABLE pg06_065 (id int PRIMARY KEY,v text)')
    for sql in ["INSERT INTO pg06_065 VALUES (1,'1 year 2 mons 3 days 04:05:06.12'::interval)","UPDATE pg06_065 SET v='-1 days'::interval","INSERT INTO pg06_065 SELECT 2,'1 year 2 mons 3 days 04:05:06.12'::interval","INSERT INTO pg06_065 VALUES (3,NULL::interval)"]:
        result=query(sql);assert result['tags']=='CZ' and not result['errors'],result
    result=query('SELECT v FROM pg06_065 ORDER BY id');assert result['rows']==[['-1 days'.encode().hex()],['1 year 2 mons 3 days 04:05:06.12'.encode().hex()],[None]] and result['metadata']==[[25,-1,-1,0]],result
    original = result
    for sql in ["UPDATE pg06_065 SET v='garbage'::interval WHERE id=1", "INSERT INTO pg06_065 VALUES (4,'garbage'::interval)"]:
        result=query(sql);assert result['errors']==['22007'] and result['tags']=='EZ' and not result['rows'],result
        recover()
        result=query('SELECT v FROM pg06_065 ORDER BY id');assert result==original,result
    for value,expected in cases:
        literal="'"+value+"'" if value is not None else 'NULL'
        result=query(f'UPDATE pg06_065 SET v={literal}::interval WHERE id=1');assert result['tags']=='CZ',result
        result=query('SELECT v FROM pg06_065 WHERE id=1');assert result['metadata']==[[25,-1,-1,0]] and result['rows']==[[None if expected is None else expected.encode().hex()]],result
    for style,expected in [('postgres','1 day 02:03:04'),('postgres_verbose','@ 1 day 2 hours 3 mins 4 secs'),('sql_standard','1 2:03:04'),('iso_8601','P1DT2H3M4S')]:
        assert query(f"SET IntervalStyle='{style}'")['tags'] in ('CZ','CSZ')
        result=query("SELECT ('1 day 02:03:04'::interval)::text AS v")
        assert result['metadata']==[[25,-1,-1,0]] and result['rows']==[[expected.encode().hex()]],result
        for sql in ["UPDATE pg06_065 SET v='1 day 02:03:04'::interval WHERE id=1","INSERT INTO pg06_065 VALUES (5,'1 day 02:03:04'::interval)","INSERT INTO pg06_065 SELECT 6,'1 day 02:03:04'::interval"]:
            result=query(sql);assert result['tags']=='CZ',result
        result=query('SELECT v FROM pg06_065 WHERE id=1 OR id=5 OR id=6 ORDER BY id')
        assert result['metadata']==[[25,-1,-1,0]] and result['rows']==[[expected.encode().hex()]]*3,result
        query('DELETE FROM pg06_065 WHERE id=5 OR id=6')
    query('RESET IntervalStyle')
    for sql in ["SELECT coalesce('1 year 2 mons 3 days 04:05:06.12'::interval,'true'::text)",'SELECT coalesce(NULL::interval,NULL::text) LIMIT 0','SELECT coalesce(NULL::text,NULL::interval) LIMIT 0']:
        send(b'P',b'bad\0'+sql.encode()+b'\0'+struct.pack('!h',0));result=record('Parse',sql,[flush_read()]);assert result['errors']==['42804'] and not result['rows'],result;recover()
    result=query('SELECT (NULL::interval)::text AS v LIMIT 0');assert result['tags']=='TCZ' and result['metadata']==[[25,-1,-1,0]],result
    print(json.dumps(dict(passed=True,server_version=server_version(conn),libpq=lib_version(),observations=len(records))))
finally:
    if raw:raw.close()
    finish(conn)
    if len(sys.argv)>1:
        with open(sys.argv[1],'w') as out:json.dump(records,out,indent=2,ensure_ascii=False)
