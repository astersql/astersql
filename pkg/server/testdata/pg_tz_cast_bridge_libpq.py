# Copyright 2026 AsterSQL.
"""TZ-CAST-BRIDGE differential through libpq and an isolated PostgreSQL 18 server."""
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

pq = c.CDLL(os.environ.get('PG_LIBPQ_LIBRARY', '/opt/homebrew/opt/libpq/lib/libpq.dylib'))
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
p, s, i = c.c_void_p, c.c_char_p, c.c_int
connect = api('PQconnectdb', p, s)
finish = api('PQfinish', None, p)
execute = api('PQexec', p, p, s)
params = api('PQexecParams', p, p, s, i, c.POINTER(c.c_uint), c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
status = api('PQresultStatus', i, p)
ef = api('PQresultErrorField', s, p, i)
rows = api('PQntuples', i, p)
cols = api('PQnfields', i, p)
value = api('PQgetvalue', p, p, i, i)
length = api('PQgetlength', i, p, i, i)
null = api('PQgetisnull', i, p, i, i)
ftype = api('PQftype', c.c_uint, p, i)
fsize = api('PQfsize', i, p, i)
fmod = api('PQfmod', i, p, i)
fformat = api('PQfformat', i, p, i)
clear = api('PQclear', None, p)
version = api('PQlibVersion', i)
assert version() >= 180000

def observe(result):
    assert result
    try:
        return dict(status=status(result), state=(ef(result, ord('C')) or b'').decode(),
                    metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))],
                    rows=[[None if null(result,r,n) else c.string_at(value(result,r,n), length(result,r,n)).hex() for n in range(cols(result))] for r in range(rows(result))])
    finally:
        clear(result)

def query(conn, sql):
    result = observe(execute(conn, sql.encode()))
    assert result['status'] in [1,2], (sql,result)
    return result

def param(conn, data, infmt, outfmt, oid, target):
    return observe(params(conn, f'SELECT CAST($1 AS {target})'.encode(), 1, (c.c_uint*1)(oid),
                          (s*1)(data), (i*1)(len(data) if data is not None else 0), (i*1)(infmt), outfmt))

binpath = Path(os.environ.get('PG18_BIN', '/opt/homebrew/opt/postgresql@18/bin'))
with tempfile.TemporaryDirectory(prefix='tz28-reference-') as root:
    with socket.socket() as sock:
        sock.bind(('127.0.0.1',0))
        port = sock.getsockname()[1]
    connections = []
    started = False
    try:
        subprocess.run([str(binpath/'initdb'),'-D',root+'/data','-U','root','-A','trust','--encoding=UTF8','--no-locale'],check=True,stdout=subprocess.DEVNULL)
        subprocess.run([str(binpath/'pg_ctl'),'-D',root+'/data','-l',root+'/log','-o',f'-h 127.0.0.1 -p {port} -k {root}','-w','start'],check=True,stdout=subprocess.DEVNULL)
        started = True
        ref = connect(f'host=127.0.0.1 port={port} user=root dbname=postgres'.encode())
        native = connect(f'host=127.0.0.1 port={sys.argv[1]} user=root dbname=tz_bridge_diff'.encode())
        connections = [ref,native]
        v = query(ref,'SELECT version()')
        print('reference='+bytes.fromhex(v['rows'][0][0]).decode(), flush=True)
        tzroot = Path(subprocess.check_output([str(binpath/'pg_config'),'--sharedir'],text=True).strip()) / 'timezone'
        print('reference_tz_sha256='+json.dumps({zone:hashlib.sha256((tzroot/zone).read_bytes()).hexdigest() for zone in ['UTC','Asia/Shanghai','America/New_York']}),flush=True)
        count = 0
        print("context=DateStyle:ISO,MDY;TimeZone:UTC;encoding:UTF8;p:-1,3",flush=True)
        for conn in connections:
            query(conn, "SET DateStyle TO 'ISO, MDY'")
            query(conn, "SET time_zone = 'UTC'" if conn==native else "SET TimeZone TO 'UTC'")
        cases = [
            ('UTC',1114,'timestamptz',b'2000-01-01 00:00:00',0),
            ('Asia/Shanghai',1114,'timestamptz',b'2000-01-01 00:00:00',0),
            ('Asia/Shanghai',1184,'timestamp',b'2000-01-01 00:00:00+00',0),
            ('America/New_York',1184,'timestamp',b'2018-11-04 01:30:00-04',594624600000000),
            ('America/New_York',1184,'timestamp',b'2018-11-04 01:30:00-05',594628200000000),
            ('America/New_York',1114,'timestamptz',b'2018-11-04 01:30:00',594610200000000),
            ('America/New_York',1114,'timestamptz',b'2018-03-11 02:30:00',574050600000000),
        ]
        for zone in ['UTC','Asia/Shanghai']:
            for oid,target in [(1114,'timestamptz'),(1184,'timestamp')]:
                for text,raw in [(b'infinity',(1<<63)-1),(b'-infinity',-(1<<63)),(None,0),
                                 (b'2000-01-01 00:00:00.123456',123456)]:
                    if text and oid==1184 and abs(raw)<1000000: text+=b'+00'
                    cases.append((zone,oid,target,text,raw))
        # Finite endpoints and typmod are exercised independently of named DST.
        for oid,target in [(1114,'timestamptz'),(1184,'timestamp')]:
            for text,raw in [(b'4714-11-24 00:00:00 BC',-211813488000000000),
                             (b'294276-12-31 23:59:59.999999',9223371331199999999)]:
                if oid==1184: text=text.replace(b' BC',b'+00 BC') if text.endswith(b' BC') else text+b'+00'
                cases.append(('UTC',oid,target,text,raw))
            cases.append(('UTC',oid,target+'(3)',b'2000-01-01 00:00:00.123456'+(b'+00' if oid==1184 else b''),123456))
        for zone,oid,target,text,raw in cases:
            for conn in connections:
                query(conn, f"SET time_zone = '{zone}'" if conn==native else f"SET TimeZone TO '{zone}'")
            for infmt in [0,1]:
                for outfmt in [0,1]:
                    data = None if text is None else (text if infmt==0 else raw.to_bytes(8,'big',signed=True))
                    r,a=[param(conn,data,infmt,outfmt,oid,target) for conn in connections]
                    assert r==a and a['status']==2,(zone,oid,target,text,infmt,outfmt,r,a)
                    count+=1
        for zone,oid,target,data,infmt,state in [
            ('UTC',1114,'timestamptz',b'garbage',0,'22007'),
            ('UTC',1184,'timestamp',b'2000-02-30 00:00:00+00',0,'22008'),
            ('UTC',1114,'timestamptz',b'\xff',0,'22021'),
            ('UTC',1184,'timestamp',b'1234567',1,'08P01'),
            ('UTC',1114,'timestamptz',bytes(9),1,'22P03'),
            ('Asia/Shanghai',1114,'timestamptz',b'4714-11-24 00:00:00 BC',0,'22008'),
            ('Asia/Shanghai',1184,'timestamp',b'294276-12-31 23:59:59.999999+00',0,'22008'),
        ]:
            for conn in connections:
                query(conn, f"SET time_zone = '{zone}'" if conn==native else f"SET TimeZone TO '{zone}'")
            for outfmt in [0,1]:
                r,a=[param(conn,data,infmt,outfmt,oid,target) for conn in connections]
                assert r==a and a['state']==state,(zone,oid,target,data,r,a)
                assert query(ref,'SELECT 1::int4')==query(native,'SELECT 1::int4')
                count+=1
        print(json.dumps(dict(libpq=version(),effective_cases=count,passed=True)),flush=True)
    finally:
        for conn in connections:
            finish(conn)
        if started:
            subprocess.run([str(binpath/'pg_ctl'),'-D',root+'/data','-m','immediate','-w','stop'],check=True,stdout=subprocess.DEVNULL)
            with socket.socket() as sock:
                assert sock.connect_ex(('127.0.0.1',port)) != 0
            print('reference_port_closed=true',flush=True)
