# Copyright 2026 AsterSQL.
"""varchar array libpq differential; owns only its temporary PostgreSQL reference cluster."""
import ctypes as c
import json
import os
from pathlib import Path
import signal
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
    result = observe(execute(conn, sql.encode()))
    assert result['status'] in [1, 2], (sql, result)
    return result

def bind(conn, name, data, infmt, outfmt):
    # c_char_p preserves the pointer, including NULs inside binary payloads;
    # explicit lengths are authoritative only in binary format.
    return observe(prepared(conn, name, 1, (s*1)(data),
                            (i*1)(len(data) if data is not None else 0), (i*1)(infmt), outfmt))

def interrupted(signum, frame):
    raise KeyboardInterrupt(f'signal {signum}')
signal.signal(signal.SIGTERM, interrupted)

binpath = Path(os.environ.get('PG18_BIN', '/opt/homebrew/opt/postgresql@18/bin'))
# Reference-only is a diagnostic mode. It never reports differential acceptance.
reference_only = sys.argv[1] == '--reference-only'
evidence = Path(sys.argv[3])
evidence.mkdir(parents=True, exist_ok=True)
summary = dict(passed=False, mode='reference-only' if reference_only else 'differential',
               libpq=version(), cases=0)
connections = []
count = 0
try:
    with tempfile.TemporaryDirectory(prefix='pg05-varchar-array-reference-') as root:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        started = False
        try:
            subprocess.run([str(binpath/'initdb'), '-D', root+'/data', '-U', 'root', '-A', 'trust', '--encoding=UTF8', '--no-locale'], check=True, stdout=subprocess.DEVNULL)
            started = True
            subprocess.run([str(binpath/'pg_ctl'), '-D', root+'/data', '-l', root+'/log', '-o', f'-h 127.0.0.1 -p {port} -k {root}', '-w', 'start'], check=True, stdout=subprocess.DEVNULL)
            endpoints = [f'port={port} dbname=postgres']
            if not reference_only:
                endpoints.append(f'port={int(sys.argv[1])} dbname={sys.argv[2]}')
            for endpoint in endpoints:
                conn = connect(f'host=127.0.0.1 user=root connect_timeout=10 {endpoint}'.encode())
                assert conn
                connections.append(conn)
                assert connected(conn) == 0, 'client connection failed'
            identity = {key: query(connections[0], sql) for key, sql in [
                ('version', 'SELECT version()'), ('server_version_num', 'SHOW server_version_num'),
                ('catalog_types', 'SELECT oid,typelem,typlen FROM pg_type WHERE oid IN (1043,1015) ORDER BY oid'),
                ('server_encoding', 'SHOW server_encoding'), ('client_encoding', 'SHOW client_encoding'),
                ('standard_conforming_strings', 'SHOW standard_conforming_strings'),
                ('array_nulls', 'SHOW array_nulls'),
                ('DateStyle', 'SHOW DateStyle'), ('TimeZone', 'SHOW TimeZone')]}
            actual_version = int(bytes.fromhex(identity['server_version_num']['rows'][0][0]))
            summary.update(reference=identity, reference_version_num=actual_version,
                           reference_build=subprocess.check_output([str(binpath/'pg_config'), '--configure'], text=True).strip())
            (evidence/'identity.json').write_text(json.dumps(summary, indent=2))
            print(f'reference_version_num={actual_version}', flush=True)
            assert actual_version == 180006, 'exact PostgreSQL 18.6 required'
            assert identity['catalog_types']['rows'] == [[b'1015'.hex(), b'1043'.hex(), b'-1'.hex()], [b'1043'.hex(), b'0'.hex(), b'-1'.hex()]]
            assert identity['array_nulls']['rows'] == [[b'on'.hex()]], 'array_nulls=on required'
            with (evidence/'cases.jsonl').open('w') as records:
                def record(phase, name, data, infmt, outfmt, expected):
                    global count
                    observed = [bind(conn, name, data, infmt, outfmt) for conn in connections]
                    entry = dict(phase=phase, statement=name.decode(), input_format=infmt,
                                 output_format=outfmt, parameter_oid=parameter_oids[name],
                                 input=None if data is None else data.hex(), reference=observed[0])
                    if not reference_only:
                        entry['aster'] = observed[1]
                    records.write(json.dumps(entry)+'\n')
                    records.flush()
                    for result in observed:
                        assert result == expected, entry
                    count += 1

                import struct
                def words(*values):
                    return struct.pack('!'+'i'*len(values), *values)
                def array(shape, lower, values, flags=None):
                    return words(len(shape), int(any(v is None for v in values)) if flags is None else flags, 1043) + b''.join(words(n,l) for n,l in zip(shape,lower)) + b''.join(words(-1) if v is None else words(len(v))+v for v in values)
                parameter_oids = {}
                statements = [
                    (b'declared', b'SELECT $1 AS v', [1015]),
                    (b'inferred', b'SELECT $1::varchar[] AS v', []),
                    (b'zero', b'SELECT $1::varchar[] AS v', [0]),
                    (b'empty', b'SELECT $1::varchar[] AS v WHERE false', [1015]),
                    (b'first_null', b'SELECT NULL::varchar[] AS v UNION ALL SELECT $1::varchar[] AS v', [1015])]
                for name, sql, oids in statements:
                    parameter_oids[name] = 1015
                    for index, conn in enumerate(connections):
                        result = observe(prepare(conn, name, sql, len(oids), (c.c_uint*len(oids))(*oids)))
                        assert result == dict(status=1, state='N/A', metadata=[], rows=[]), result
                        result = describe(conn, name)
                        try:
                            observed = dict(status=status(result), parameter_oids=[paramtype(result,n) for n in range(nparams(result))], metadata=[[ftype(result,n), fsize(result,n), fmod(result,n), fformat(result,n)] for n in range(cols(result))])
                            records.write(json.dumps(dict(phase='Parse/statement Describe', endpoint='reference' if index == 0 else 'aster', statement=name.decode(), observed=observed))+'\n')
                            assert observed == dict(status=1, parameter_oids=[1015], metadata=[[1015,-1,-1,0]]), observed
                        finally:
                            clear(result)
                payloads = [
                    (b'{a,b}', array([2],[1],[b'a',b'b'])),
                    (b'{{a,b},{c,d}}', array([2,2],[1,1],[b'a',b'b',b'c',b'd'])),
                    (b'[0:1][-2:-1]={{a,NULL},{b,c}}', array([2,2],[0,-2],[b'a',None,b'b',b'c'])),
                    ('[-3:-2]={é,😀}'.encode(), array([2],[-3],['é'.encode(),'😀'.encode()])),
                    (b'{}', array([],[],[])),
                    (b'{NULL}', array([1],[1],[None])),
                    (b'{NULL,NULL}', array([2],[1],[None,None])),
                    (b'{{{{{{a}}}}}}', array([1]*6,[1]*6,[b'a'])),
                    (None,None),
                    (br'{"NULL","","a,b","a\"b","a\\b"}', array([5],[1],[b'NULL',b'',b'a,b',b'a"b',b'a\\b'])),
                    ('{" null ","a b",é,中}'.encode(), array([4],[1],[b' null ',b'a b','é'.encode(),'中'.encode()])),
                    (b'{"{}","{","}"}', array([3],[1],[b'{}',b'{',b'}']))]
                for infmt in [0,1]:
                    for outfmt in [0,1]:
                        for name, _, _ in statements:
                            for text, binary in payloads:
                                data = binary if infmt else text
                                outgoing = binary if outfmt else text
                                result_rows = [] if name == b'empty' else ([[None]] if name == b'first_null' else []) + [[None if outgoing is None else outgoing.hex()]]
                                record('Bind/Execute/rebind/shape-null', name, data, infmt, outfmt,
                                       dict(status=2, state='N/A', metadata=[[1015,-1,-1,outfmt]], rows=result_rows))
                        # Input flags are not a promise about element NULLs; output is canonical.
                        if infmt:
                            for data, canonical in [(array([1],[1],[None],0),payloads[5]), (array([2],[1],[b'a',b'b'],1),payloads[0]), (words(1,0,1043,0,-7),payloads[4])]:
                                outgoing = canonical[outfmt]
                                record('Bind/Execute/noncanonical-header', b'declared',data,infmt,outfmt,
                                       dict(status=2,state='N/A',metadata=[[1015,-1,-1,outfmt]],rows=[[outgoing.hex()]]))
                modifier_cases = 0
                for name, sql, typmod in [
                    (b'mod', b'SELECT $1::varchar(2)[] AS v', 6),
                    (b'mod_empty', b'SELECT $1::varchar(2)[] AS v WHERE false', 6),
                    (b'mod_null', b'SELECT NULL::varchar(2)[] AS v UNION ALL SELECT $1::varchar(2)[] AS v', 6),
                ]:
                    parameter_oids[name] = 1015
                    for conn in connections:
                        assert observe(prepare(conn,name,sql,1,(c.c_uint*1)(1015))) == dict(status=1,state='N/A',metadata=[],rows=[])
                        result=describe(conn,name)
                        try:
                            observed=dict(status=status(result),parameter_oids=[paramtype(result,n) for n in range(nparams(result))],metadata=[[ftype(result,n),fsize(result,n),fmod(result,n),fformat(result,n)] for n in range(cols(result))])
                            records.write(json.dumps(dict(phase='modifier statement Describe',statement=name.decode(),observed=observed))+'\n')
                            assert observed==dict(status=1,parameter_oids=[1015],metadata=[[1015,-1,typmod,0]]),observed
                        finally: clear(result)
                    modifier_payloads = [
                        ('[-2:-1]={é😀x,NULL}'.encode(), array([2],[-2],['é😀x'.encode(),None]), '[-2:-1]={é😀,NULL}'.encode(), array([2],[-2],['é😀'.encode(),None])),
                        ('{éx,"a  "}'.encode(),array([2],[1],['éx'.encode(),b'a  ']),'{é,"a "}'.encode(),array([2],[1],['é'.encode(),b'a '])),
                        (b'{}',array([],[],[]),b'{}',array([],[],[])),
                        (b'{NULL,NULL}',array([2],[1],[None,None]),b'{NULL,NULL}',array([2],[1],[None,None])),
                        (None,None,None,None),
                    ]
                    for infmt in [0,1]:
                        for outfmt in [0,1]:
                            for text,binary,text_out,binary_out in modifier_payloads:
                                outgoing = binary_out if outfmt else text_out
                                result_rows=[] if name==b'mod_empty' else ([[None]] if name==b'mod_null' else [])+[[None if outgoing is None else outgoing.hex()]]
                                record('varchar modifier four directions',name,binary if infmt else text,infmt,outfmt,dict(status=2,state='N/A',metadata=[[1015,-1,typmod,outfmt]],rows=result_rows))
                                modifier_cases+=1
                    for conn in connections: assert observe(close_prepared(conn,name))==dict(status=1,state='N/A',metadata=[],rows=[])
                summary['modifier_cases']=modifier_cases
                sql_cases=0
                for sql,expected in [
                    ("SELECT ARRAY[['é😀x',NULL],['a  ','éx']]::varchar(2)[] AS v",'{{é😀,NULL},{"a ",é}}'),
                    ("SELECT ARRAY[]::varchar(2)[] AS v",'{}'),
                    ("SELECT ARRAY[NULL,NULL]::varchar(2)[] AS v",'{NULL,NULL}'),
                    ("SELECT ARRAY['é'::varchar,'😀'::varchar] AS v",'{é,😀}'),
                    ("SELECT '{abcdef}'::varchar(3)[]::varchar(2)[] AS v",'{ab}'),
                ]:
                    observed=[query(conn,sql) for conn in connections]
                    records.write(json.dumps(dict(phase='SQL constructors and nested modifier',sql=sql,observed=observed))+'\n')
                    assert observed[0]['status']==2 and observed[0]['rows']==[[expected.encode().hex()]],observed
                    assert observed[0]['metadata'][0][0]==1015,observed
                    assert all(result==observed[0] for result in observed),observed
                    sql_cases+=1
                for n in [0,10485761]:
                    for suffix in ['', ' WHERE false']:
                        sql=f'SELECT NULL::varchar({n})[] AS v{suffix}'
                        observed=[observe(execute(conn,sql.encode())) for conn in connections]
                        records.write(json.dumps(dict(phase='invalid modifier before NULL or zero rows',sql=sql,observed=observed))+'\n')
                        assert observed[0]['status']==7 and observed[0]['state']=='22023' and not observed[0]['rows'],observed
                        assert all(result==observed[0] for result in observed),observed
                        for conn in connections: assert query(conn,'SELECT 1::int4 AS v')==dict(status=2,state='N/A',metadata=[[23,4,-1,0]],rows=[['31']])
                        sql_cases+=1
                summary['sql_cases']=sql_cases

                if not reference_only:
                    print('PG05_ARRAY_MID', flush=True)
                    assert sys.stdin.readline().strip() == 'continue'
                # Classify each rejection using the actual 18.6 reference, retaining
                # full observations before asserting equality. The TCP pass below
                # separately proves Bind phase, discard-until-Sync and no DataRow.
                invalid_text = [b'',b' ',b'{',b'{a,}',b'{,a}',b'{{a,b},{c}}',b'[0:2]={a,b}',b'{"a}',b'{a\\}',b'{\xff}',b'{{{{{{{a}}}}}}}',b'[2147483647:2147483647]={a}']
                header = words(1,0,1043,1,1)
                invalid_binary = [words(-1),words(7),words(0,-1),words(0,2),words(0,0,23),words(0,0,9999),
                    words(1,0,1043,-1,1), words(2,0,1043,50000,1,50000,1),words(1,0,1043,134217728,1),
                    words(1,0,1043,1,2147483647),words(2,0,1043,0,1,-1,1),words(2,0,1043,0,1,1,2147483647),
                    words(1,0,1043,134217727,1),words(0,0,1043,0),array([1],[1],[b'a'])+b'\0']
                invalid_binary += [header[:n] for n in range(len(header))]
                invalid_binary += [header+words(4)[:n] for n in range(4)]
                invalid_binary += [header+words(n) for n in [-2,-2147483648,1,4,2147483647]]
                invalid_binary += [array([2],[1],[b'valid',bad]) for bad in [b'\xff',b'a\0b',b'\xc3']]
                diagnostics = []
                mismatches = []
                for infmt, cases in [(0,invalid_text),(1,invalid_binary)]:
                    for outfmt in [0,1]:
                        for data in cases:
                            observed = [bind(conn,b'declared',data,infmt,outfmt) for conn in connections]
                            entry = dict(phase='libpq Bind/Execute rejection (TCP phase below)',input=data.hex(),input_format=infmt,output_format=outfmt,parameter_oid=1015,reference=observed[0])
                            if not reference_only:
                                entry['aster'] = observed[1]
                            records.write(json.dumps(entry)+'\n'); records.flush()
                            assert observed[0]['status'] == 7 and not observed[0]['rows'], entry
                            diagnostics.append((infmt,outfmt,data,observed[0]['state']))
                            if any(result != observed[0] for result in observed[1:]):
                                mismatches.append(entry)
                            recoveries = []
                            for conn in connections:
                                recovered = query(conn,'SELECT 1::int4 AS v')
                                recoveries.append(recovered)
                                assert recovered == dict(status=2,state='N/A',metadata=[[23,4,-1,0]],rows=[['31']]), recovered
                            records.write(json.dumps(dict(phase='libpq Sync/recovery',query='SELECT 1::int4 AS v',observed=recoveries))+'\n')
                            count += 1

                # libpq hides portal RowDescription and the discarded messages.
                # Use the same real endpoints with startup/authentication payloads
                # deliberately excluded from evidence.
                def recv_exact(sock, n):
                    data = b''
                    while len(data) < n:
                        part = sock.recv(n-len(data))
                        assert part, 'unexpected EOF'
                        data += part
                    return data
                def send(sock, tag, body=b''):
                    sock.sendall(tag+struct.pack('!I',len(body)+4)+body)
                def read(sock):
                    tag = recv_exact(sock,1)
                    size = struct.unpack('!I',recv_exact(sock,4))[0]
                    return tag, recv_exact(sock,size-4)
                def until_ready(sock):
                    result=[]
                    while True:
                        msg=read(sock); result.append(msg)
                        if msg[0] == b'Z':
                            assert msg[1] == b'I'
                            return result
                def raw_connect(endpoint):
                    sock=socket.create_connection(('127.0.0.1',endpoint),timeout=10)
                    raw_sockets.append(sock)
                    body=words(196608)+b'user\0root\0database\0'+(b'postgres' if endpoint == port else sys.argv[2].encode())+b'\0\0'
                    sock.sendall(words(len(body)+4)+body)
                    messages=until_ready(sock)
                    assert not any(t==b'E' for t,b in messages)
                    return sock
                def raw_bind(sock, data, infmt, outfmt):
                    body=b'\0a\0'+struct.pack('!hhh',1,infmt,1)+words(-1 if data is None else len(data))+(data or b'')+struct.pack('!hh',1,outfmt)
                    send(sock,b'B',body)
                def raw_log(phase, messages, **extra):
                    records.write(json.dumps(dict(phase=phase,messages=[[t.decode(),b.hex()] for t,b in messages],**extra))+'\n'); records.flush()
                raw_sockets=[]
                tcp_cases=0
                try:
                    for endpoint in [port]+([] if reference_only else [int(sys.argv[1])]):
                        sock=raw_connect(endpoint)
                        send(sock,b'P',b'a\0SELECT $1 AS v\0'+struct.pack('!hI',1,1015))
                        send(sock,b'D',b'Sa\0'); send(sock,b'S')
                        messages=until_ready(sock)
                        expected_desc=struct.pack('!h',1)+b'v\0'+struct.pack('!IhIhih',0,0,1015,-1,-1,0)
                        assert messages == [(b'1',b''),(b't',struct.pack('!hI',1,1015)),(b'T',expected_desc),(b'Z',b'I')], messages
                        raw_log('TCP Parse/statement Describe',messages,endpoint='reference' if endpoint==port else 'aster')
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                for text,binary in payloads:
                                    data = binary if infmt else text
                                    outgoing = binary if outfmt else text
                                    raw_bind(sock,data,infmt,outfmt)
                                    send(sock,b'D',b'P\0'); send(sock,b'E',b'\0'+words(0)); send(sock,b'S')
                                    messages=until_ready(sock)
                                    expected_desc=expected_desc[:-2]+struct.pack('!h',outfmt)
                                    row=struct.pack('!h',1)+words(-1 if outgoing is None else len(outgoing))+(outgoing or b'')
                                    assert messages == [(b'2',b''),(b'T',expected_desc),(b'D',row),(b'C',b'SELECT 1\0'),(b'Z',b'I')], messages
                                    raw_log('TCP Bind/portal Describe/Execute',messages,input=None if data is None else data.hex(),input_format=infmt,output_format=outfmt,endpoint='reference' if endpoint==port else 'aster')
                                    tcp_cases+=1
                        for infmt,outfmt,data,state in diagnostics:
                            raw_bind(sock,data,infmt,outfmt)
                            # An otherwise valid write must be discarded after Bind error.
                            send(sock,b'P',b'should_skip\0CREATE TABLE array_error_side_effect (id integer)\0'+struct.pack('!h',0))
                            send(sock,b'B',b'\0should_skip\0'+struct.pack('!hhh',0,0,0))
                            send(sock,b'E',b'\0'+words(0)); send(sock,b'S')
                            messages=until_ready(sock)
                            raw_log('TCP Bind/reject/discard/Sync',messages,input=data.hex(),input_format=infmt,output_format=outfmt,endpoint='reference' if endpoint==port else 'aster')
                            assert [t for t,b in messages] == [b'E',b'Z'], messages
                            fields=dict((part[:1],part[1:]) for part in messages[0][1].split(b'\0') if part)
                            if fields[b'C'].decode()!=state:
                                mismatches.append(dict(phase='TCP Bind SQLSTATE',input=data.hex(),expected=state,actual=fields[b'C'].decode()))
                            send(sock,b'Q',b'SELECT 1::int4 AS v\0')
                            recovered=until_ready(sock)
                            assert [t for t,b in recovered]==[b'T',b'D',b'C',b'Z'], recovered
                            assert recovered[1] == (b'D',struct.pack('!h',1)+words(1)+b'1')
                            raw_log('TCP Sync/recovery SELECT 1::int4',recovered)
                            # The discarded CREATE must leave no catalog object.
                            send(sock,b'Q',b'SELECT * FROM array_error_side_effect\0')
                            absent=until_ready(sock)
                            assert [t for t,b in absent]==[b'E',b'Z'], absent
                            absent_fields=dict((part[:1],part[1:]) for part in absent[0][1].split(b'\0') if part)
                            assert absent_fields[b'C']==b'42P01', absent
                            raw_log('TCP no-side-effect/catalog-absence',absent)
                            tcp_cases+=1
                        # Retain typmod in portal Describe for each format and rebind.
                        send(sock,b'C',b'Sa\0'); send(sock,b'S')
                        assert until_ready(sock)==[(b'3',b''),(b'Z',b'I')]
                        send(sock,b'P',b'a\0SELECT $1::varchar(2)[] AS v\0'+struct.pack('!hI',1,1015))
                        send(sock,b'D',b'Sa\0'); send(sock,b'S')
                        messages=until_ready(sock)
                        expected_desc=struct.pack('!h',1)+b'v\0'+struct.pack('!IhIhih',0,0,1015,-1,6,0)
                        assert messages==[(b'1',b''),(b't',struct.pack('!hI',1,1015)),(b'T',expected_desc),(b'Z',b'I')],messages
                        raw_log('TCP modifier statement Describe',messages,endpoint='reference' if endpoint==port else 'aster')
                        for infmt in [0,1]:
                            for outfmt in [0,1]:
                                for text,binary,text_out,binary_out in modifier_payloads:
                                    data=binary if infmt else text
                                    outgoing=binary_out if outfmt else text_out
                                    raw_bind(sock,data,infmt,outfmt)
                                    send(sock,b'D',b'P\0'); send(sock,b'E',b'\0'+words(0)); send(sock,b'S')
                                    messages=until_ready(sock)
                                    desc=expected_desc[:-2]+struct.pack('!h',outfmt)
                                    row=struct.pack('!h',1)+words(-1 if outgoing is None else len(outgoing))+(outgoing or b'')
                                    assert messages==[(b'2',b''),(b'T',desc),(b'D',row),(b'C',b'SELECT 1\0'),(b'Z',b'I')],messages
                                    raw_log('TCP modifier portal Describe/Execute',messages,input=None if data is None else data.hex(),input_format=infmt,output_format=outfmt,endpoint='reference' if endpoint==port else 'aster')
                                    tcp_cases+=1
                        send(sock,b'C',b'Sa\0'); send(sock,b'S')
                        assert until_ready(sock)==[(b'3',b''),(b'Z',b'I')]
                        send(sock,b'X')
                finally:
                    for sock in raw_sockets:
                        sock.close()
                    summary.update(tcp_cases=tcp_cases,raw_clients_closed=True,mismatches=mismatches)
                assert not mismatches, mismatches[:3]
                for conn in connections:
                    for name, _, _ in statements:
                        result = observe(close_prepared(conn, name))
                        assert result['status'] == 1, result
            summary.update(cases=count, reference_matrix_passed=True)
        finally:
            for conn in connections:
                finish(conn)
            summary['clients_closed'] = True
            if started:
                subprocess.run([str(binpath/'pg_ctl'), '-D', root+'/data', '-m', 'immediate', '-w', 'stop'], check=True, stdout=subprocess.DEVNULL)
                with socket.socket() as sock:
                    summary['reference_port_closed'] = sock.connect_ex(('127.0.0.1',port)) != 0
                assert summary['reference_port_closed']
finally:
    summary.update(cases=count, reference_directory_removed='root' in globals() and not Path(root).exists())
    (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
summary['passed'] = not reference_only
(evidence/'summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary), flush=True)
