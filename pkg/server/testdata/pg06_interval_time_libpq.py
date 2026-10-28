# Copyright 2026 AsterSQL.
"""Task 79 interval-to-time differential against the fixed PostgreSQL 18.6 reference."""
import ctypes as c
import os
import sys

pq = c.CDLL(os.environ.get('PG_LIBPQ_LIBRARY', '/opt/homebrew/opt/libpq/lib/libpq.dylib'))
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
p, s, i = c.c_void_p, c.c_char_p, c.c_int
connect = api('PQconnectdb', p, s)
connected = api('PQstatus', i, p)
finish = api('PQfinish', None, p)
execute = api('PQexec', p, p, s)
prepare = api('PQprepare', p, p, s, s, i, c.POINTER(c.c_uint))
describe = api('PQdescribePrepared', p, p, s)
close_prepared = api('PQclosePrepared', p, p, s)
nparams = api('PQnparams', i, p)
paramtype = api('PQparamtype', c.c_uint, p, i)
prepared = api('PQexecPrepared', p, p, s, i, c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
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
    assert result, 'libpq returned no result'
    try:
        return dict(status=status(result), state=(ef(result, ord('C')) or b'').decode() or 'N/A',
                    metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))],
                    rows=[[None if null(result,r,n) else c.string_at(value(result,r,n), length(result,r,n)).hex() for n in range(cols(result))] for r in range(rows(result))])
    finally:
        clear(result)

def query(conn, sql):
    raw = execute(conn, sql.encode())
    message = ef(raw, ord('M'))
    result = observe(raw)
    assert result['status'] in [1, 2], (sql, result, message)
    return result

def bind(conn, name, data, infmt, outfmt):
    # c_char_p preserves the pointer, including NULs inside binary payloads;
    # explicit lengths are authoritative only in binary format.
    return observe(prepared(conn, name, 1, (s*1)(data),
                            (i*1)(len(data) if data is not None else 0), (i*1)(infmt), outfmt))

server_version = api('PQserverVersion', i, p)
connections = []
count = 0
try:
    for dsn in [os.environ['PG18_REFERENCE_DSN'], f'host=127.0.0.1 port={int(sys.argv[1])} user=root dbname=test']:
        conn = connect(dsn.encode())
        assert conn and connected(conn) == 0, 'connection failed'
        connections.append(conn)
    ref, native = connections
    assert version() == server_version(ref) == 180006
    print('reference_server=180006 libpq=180006', flush=True)
    assert query(ref, 'SELECT castcontext FROM pg_cast WHERE castsource=1186 AND casttarget=1083')['rows'] == [[b'a'.hex()]]
    print('pg_cast interval->time context=a (explicit and assignment)', flush=True)
    def compare(sql, expected_status=2, state='N/A'):
        global count
        results = [observe(execute(conn, (sql.replace('pg06_079', 'public.pg06_079') if conn == native else sql).encode())) for conn in connections]
        assert results[0] == results[1], (sql, results)
        assert results[0]['status'] == expected_status and results[0]['state'] == state, (sql, results)
        count += 1
        return results[0]
    fixtures = [(b'00:00:00',0,0,0),(b'12:34:56.123456',45296123456,0,0),(b'23:59:59.999999',86399999999,0,0),(b'24:00:00',86400000000,0,0),(b'-01:00:00',-3600000000,0,0),(b'1 mon 2 days 25:00:00',90000000000,2,1),(b'-1 mon -2 days -49:00:00',-176400000000,-2,-1),(b'2147483647 mons 2147483647 days 01:00:00',3600000000,2147483647,2147483647),(None,0,0,0)]
    for zone in ['UTC', 'Asia/Shanghai', 'America/New_York']:
        for conn in connections:
            query(conn, f"SET time_zone = '{zone}'" if conn == native else f"SET TimeZone TO '{zone}'")
        for target in ['time', 'time(3)', 'time(0)']:
            for conn in connections:
                result = observe(prepare(conn,b'explicit',f'SELECT $1::{target} AS v'.encode(),1,(c.c_uint*1)(1186)))
                assert result['status'] == 1, result
            descriptions = [observe(describe(conn,b'explicit')) for conn in connections]
            assert descriptions[0] == descriptions[1], descriptions
            assert descriptions[0]['metadata'][0][:2] == [1083,8]
            for conn in connections:
                result = describe(conn,b'explicit')
                assert nparams(result) == 1 and paramtype(result,0) == 1186
                clear(result)
            for text,micros,days,months in fixtures:
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        data = None if text is None else text if infmt == 0 else micros.to_bytes(8,'big',signed=True)+days.to_bytes(4,'big',signed=True)+months.to_bytes(4,'big',signed=True)
                        results = [bind(conn,b'explicit',data,infmt,outfmt) for conn in connections]
                        assert results[0] == results[1] and results[0]['status'] == 2, (zone,target,text,infmt,outfmt,results)
                        count += 1
            for text,state in [(b'garbage','22007'),(b'infinity','22008'),(b'-infinity','22008')]:
                results = [bind(conn,b'explicit',text,0,0) for conn in connections]
                assert results[0] == results[1] and results[0]['state'] == state, (text,results)
                count += 1
                compare('SELECT 1::int4')
            for conn in connections:
                clear(close_prepared(conn,b'explicit'))
        for conn in connections:
            result = observe(prepare(conn,b'lazy',b'SELECT CASE WHEN true THEN NULL::time ELSE $1::time END AS v',1,(c.c_uint*1)(1186)))
            assert result['status'] == 1, result
        results = [bind(conn,b'lazy',b'infinity',0,0) for conn in connections]
        assert results[0] == results[1] and results[0]['status'] == 2 and results[0]['rows'] == [[None]], results
        count += 1
        for conn in connections:
            clear(close_prepared(conn,b'lazy'))
        for text,_,_,_ in fixtures:
            literal = 'NULL' if text is None else "'"+text.decode()+"'"
            compare(f'SELECT ({literal}::interval)::time AS v')
        compare('SELECT (NULL::interval)::time AS v LIMIT 0')
        compare("SELECT ('23:59:59.999999'::interval(3))::time AS v")
    query(ref,'CREATE TEMP TABLE pg06_079 (id int, v time(3))')
    for text,_,_,_ in fixtures:
        literal = 'NULL' if text is None else "'"+text.decode()+"'"
        compare(f'INSERT INTO pg06_079 VALUES (1,{literal}::interval)',1)
        compare(f'INSERT INTO pg06_079 SELECT 2,{literal}::interval',1)
        compare(f'UPDATE pg06_079 SET v={literal}::interval WHERE id=1',1)
        compare('SELECT v FROM pg06_079 ORDER BY id')
        compare('DELETE FROM pg06_079',1)
    compare('INSERT INTO pg06_079 VALUES (1,NULL)',1)
    original = compare('SELECT v FROM pg06_079')
    for literal,state in [("'garbage'",'22007'),("'infinity'",'22008'),("'-infinity'",'22008')]:
        for sql in [f'SELECT ({literal}::interval)::time',f'INSERT INTO pg06_079 VALUES (2,{literal}::interval)',f'UPDATE pg06_079 SET v={literal}::interval WHERE id=1']:
            compare(sql,7,state)
            compare('SELECT 1::int4')
            assert compare('SELECT v FROM pg06_079') == original
    compare('SELECT coalesce(NULL::time,NULL::interval)',7,'42804')
    compare('SELECT 1::int4')
    # Inspect the actual protocol boundary on both authenticated libpq sockets.
    # No libpq operations run until these exchanges have drained ReadyForQuery.
    import socket
    import struct
    socket_fd = api('PQsocket', i, p)
    def receive(wire):
        def exact(size):
            data = b''
            while len(data) < size:
                part = wire.recv(size-len(data))
                assert part, 'unexpected EOF'
                data += part
            return data
        tag = exact(1)
        size = struct.unpack('!I',exact(4))[0]
        return tag,exact(size-4)
    def send_message(wire,tag,body=b''):
        wire.sendall(tag+struct.pack('!I',len(body)+4)+body)
    def recover(wire):
        send_message(wire,b'S')
        assert receive(wire) == (b'Z',b'I')
        send_message(wire,b'Q',b'SELECT 1::int4\0')
        messages = []
        while True:
            message = receive(wire)
            messages.append(message)
            if message[0] == b'Z':
                break
        assert [m[0] for m in messages] == [b'T',b'D',b'C',b'Z']
        assert messages[1][1] == struct.pack('!hi',1,1)+b'1'
    for conn in connections:
        with socket.socket(fileno=os.dup(socket_fd(conn))) as wire:
            wire.settimeout(10)
            send_message(wire,b'P',b'phase\0SELECT $1::time AS v\0'+struct.pack('!hI',1,1186))
            send_message(wire,b'H')
            assert receive(wire) == (b'1',b'')
            recover(wire)
            for text,state,phase in [(b'garbage',b'22007',b'B'),(b'infinity',b'22008',b'B'),(b'-infinity',b'22008',b'B')]:
                body = b'\0phase\0'+struct.pack('!hhi',0,1,len(text))+text+struct.pack('!h',0)
                send_message(wire,b'B',body)
                send_message(wire,b'H')
                if phase == b'E':
                    response = receive(wire)
                    assert response == (b'2',b''), response
                    send_message(wire,b'E',b'\0'+struct.pack('!i',0))
                send_message(wire,b'H')
                message = receive(wire)
                assert message[0] == b'E', (phase,message)
                fields = {part[:1]:part[1:] for part in message[1].split(b'\0') if part}
                assert fields[b'C'] == state, fields
                recover(wire)
                count += 1
    print('raw_error_phases: garbage=Bind/22007 infinity=Bind/22008; Sync recovery passed',flush=True)
    print(f'interval_time_differential_cases={count} passed',flush=True)
finally:
    for conn in connections:
        finish(conn)
