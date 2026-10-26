# Copyright 2026 AsterSQL.
"""TZ-SAME-INSTANT differential through libpq and an isolated PostgreSQL 18 server."""
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

def param(conn, data, infmt, outfmt):
    return observe(params(conn,b'SELECT $1::timestamptz',1,(c.c_uint*1)(1184),
                          (s*1)(data),(i*1)(len(data) if data is not None else 0),(i*1)(infmt),outfmt))

binpath = Path(os.environ.get('PG18_BIN', '/opt/homebrew/opt/postgresql@18/bin'))
with tempfile.TemporaryDirectory(prefix='tz24-reference-') as root:
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
        native = connect(f'host=127.0.0.1 port={sys.argv[1]} user=root dbname=tz_same_instant_diff'.encode())
        connections = [ref,native]
        v = query(ref,'SELECT version()')
        print('reference='+bytes.fromhex(v['rows'][0][0]).decode(), flush=True)
        tzroot = Path(subprocess.check_output([str(binpath/'pg_config'),'--sharedir'],text=True).strip()) / 'timezone'
        print('reference_tz_sha256='+json.dumps({zone:hashlib.sha256((tzroot/zone).read_bytes()).hexdigest() for zone in ['UTC','Asia/Shanghai','America/New_York']}),flush=True)
        for conn in connections:
            query(conn, "SET time_zone = 'UTC'" if conn == native else "SET TimeZone TO 'UTC'")
        count = 0
        for outfmt in [0,1]:
            for infmt in [0,1]:
                for text,micros in [
                    (b'2000-01-01 00:00:00+00',0),
                    (b'2000-01-01 08:00:00+08',0),
                    (b'2000-01-01 00:00:00+08',-28800000000),
                    (b'1999-12-31 23:59:59.999999+00',-1),
                    (b'2000-01-01 00:00:00.000001+00',1),
                    (b'-infinity',-(1<<63)),(b'infinity',(1<<63)-1),
                    (None,None)]:
                    data = text if infmt == 0 or text is None else micros.to_bytes(8,'big',signed=True)
                    r,a = param(ref,data,infmt,outfmt),param(native,data,infmt,outfmt)
                    assert r == a and a['metadata'] == [[1184,8,-1,outfmt]], (infmt,outfmt,data,r,a)
                    count += 1
            for infmt,data,state in [
                (0,b'','22007'),(0,b'garbage','22007'),
                (0,b'2000-02-30 00:00:00+00','22008'),
                (0,b'\xff','22021'),(1,bytes(7),'08P01'),(1,bytes(9),'22P03')]:
                r,a = param(ref,data,infmt,outfmt),param(native,data,infmt,outfmt)
                assert r == a and a['state'] == state, (infmt,data,r,a)
                assert query(ref,'SELECT 1::int4') == query(native,'SELECT 1::int4')
                count += 1
        # libpq text uses NUL termination; embedded NUL is exercised by
        # the enclosing Rust raw TCP test instead.
        print(json.dumps(dict(libpq=version(),effective_cases=count,passed=True)),flush=True)
    finally:
        for conn in connections:
            finish(conn)
        if started:
            subprocess.run([str(binpath/'pg_ctl'),'-D',root+'/data','-m','immediate','-w','stop'],check=True,stdout=subprocess.DEVNULL)
            with socket.socket() as sock:
                assert sock.connect_ex(('127.0.0.1',port)) != 0
            print('reference_port_closed=true',flush=True)
