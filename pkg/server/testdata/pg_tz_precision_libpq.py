# Copyright 2026 AsterSQL.
"""TZ-PRECISION-RANGE differential through libpq and an isolated PostgreSQL 18 server."""
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

def param(conn, data, infmt, outfmt, precision=-1):
    return observe(params(conn,('SELECT $1::timestamptz' + (f'({precision})' if precision >= 0 else '')).encode(),1,(c.c_uint*1)(1184),
                          (s*1)(data),(i*1)(len(data) if data is not None else 0),(i*1)(infmt),outfmt))

binpath = Path(os.environ.get('PG18_BIN', '/opt/homebrew/opt/postgresql@18/bin'))
with tempfile.TemporaryDirectory(prefix='tz27-reference-') as root:
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
        native = connect(f'host=127.0.0.1 port={sys.argv[1]} user=root dbname=tz_precision_diff'.encode())
        connections = [ref,native]
        v = query(ref,'SELECT version()')
        print('reference='+bytes.fromhex(v['rows'][0][0]).decode(), flush=True)
        tzroot = Path(subprocess.check_output([str(binpath/'pg_config'),'--sharedir'],text=True).strip()) / 'timezone'
        print('reference_tz_sha256='+json.dumps({zone:hashlib.sha256((tzroot/zone).read_bytes()).hexdigest() for zone in ['UTC','Asia/Shanghai','America/New_York']}),flush=True)
        count = 0
        print("context=DateStyle:ISO,MDY;TimeZone:UTC;encoding:UTF8;p:-1,0,3,6,7",flush=True)
        for conn in connections:
            query(conn, "SET DateStyle TO 'ISO, MDY'")
            query(conn, "SET time_zone = 'UTC'" if conn==native else "SET TimeZone TO 'UTC'")
        cases = [
            (b'2000-01-01 00:00:00.000001+00',1,-1),
            (b'1999-12-31 23:59:59.999999+00',-1,6),
            (b'2000-01-01 00:00:00.5+00',500000,0),
            (b'1999-12-31 23:59:59.5+00',-500000,0),
            (b'2000-01-01 23:59:59.9999999+00',86400000000,6),
            (b'2000-01-01 00:00:00.1234567+00',123457,6),
            (b'4714-11-24 00:00:00+00 BC',-211813488000000000,-1),
            (b'294276-12-31 23:59:59.999999+00',9223371331199999999,-1),
            (b'0001-01-01 00:00:00+00 BC',-63113904000000000,-1),
            (b'294277-01-01 00:59:59.999999+01',9223371331199999999,-1),
            (b'4714-11-23 23:00:00-01 BC',-211813488000000000,-1),
            (b'infinity',(1<<63)-1,0),
            (b'-infinity',-(1<<63),3),
            (None,0,3),
        ]
        for text,micros,precision in cases:
            for infmt in [0,1]:
                for outfmt in [0,1]:
                    data = text if infmt==0 else micros.to_bytes(8,'big',signed=True) if text is not None else None
                    r,a = [param(conn,data,infmt,outfmt,precision) for conn in connections]
                    assert r==a and a['status']==2 and a['metadata']==[[1184,8,precision,outfmt]],(text,infmt,outfmt,r,a)
                    count+=1
        invalid = [(0,t.encode(),'22008') for t in [
            '4714-11-23 23:59:59.999999+00 BC','294277-01-01 00:00:00+00',
            '4714-11-24 00:00:00+00:00:01 BC','294276-12-31 23:59:59.999999-00:00:01',
            '0000-01-01 00:00:00+00','2000-00-01 00:00:00+00','2000-01-00 00:00:00+00']]
        invalid += [(1,n.to_bytes(8,'big',signed=True),'22008') for n in [-211813488000000001,9223371331200000000]]
        invalid += [(0,b'','22007'),(0,b'garbage','22007'),(0,b'\xff','22021'),(1,bytes(7),'08P01'),(1,bytes(9),'22P03')]
        for infmt,data,state in invalid:
            for outfmt in [0,1]:
                r,a = [param(conn,data,infmt,outfmt) for conn in connections]
                assert r==a and a['state']==state,(data,infmt,outfmt,r,a)
                assert query(ref,'SELECT 1::int4')==query(native,'SELECT 1::int4')
                count+=1
        for outfmt in [0,1]:
            r,a=[param(conn,b'2000-01-01 00:00:00.1234567+00',0,outfmt,7) for conn in connections]
            assert r==a and a['status']==2 and a['metadata']==[[1184,8,6,outfmt]],(r,a)
            count+=1
        for precision in [-1,-7]:
            sql=f'SELECT NULL::timestamptz({precision})'.encode()
            r,a=[observe(execute(conn,sql)) for conn in connections]
            assert r==a and a['state']=='22023',(r,a)
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
