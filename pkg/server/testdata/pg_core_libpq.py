# Copyright 2026 AsterSQL.
"""Independent libpq 18 write/readback workflow; argv: TCP port, database."""
import ctypes as c
import ctypes.util
import os
import sys

library = os.environ.get("PG_LIBPQ_LIBRARY")
if not library:
    library = (
        "/opt/homebrew/opt/libpq/lib/libpq.dylib"
        if sys.platform == "darwin"
        else ctypes.util.find_library("pq")
    )
assert library, "install libpq 18 or set PG_LIBPQ_LIBRARY"
pq = c.CDLL(library)


def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn


ptr, text, integer = c.c_void_p, c.c_char_p, c.c_int
version = api("PQlibVersion", integer)
connect = api("PQconnectdb", ptr, text)
status = api("PQstatus", integer, ptr)
error = api("PQerrorMessage", text, ptr)
finish = api("PQfinish", None, ptr)
protocol = api("PQfullProtocolVersion", integer, ptr)
parameter_status = api("PQparameterStatus", text, ptr, text)
execute = api("PQexec", ptr, ptr, text)
params = api(
    "PQexecParams", ptr, ptr, text, integer, c.POINTER(c.c_uint),
    c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer,
)
result_status = api("PQresultStatus", integer, ptr)
result_error = api("PQresultErrorMessage", text, ptr)
rows = api("PQntuples", integer, ptr)
columns = api("PQnfields", integer, ptr)
value = api("PQgetvalue", text, ptr, integer, integer)
is_null = api("PQgetisnull", integer, ptr, integer, integer)
field_type = api("PQftype", c.c_uint, ptr, integer)
command_rows = api("PQcmdTuples", text, ptr)
clear = api("PQclear", None, ptr)
assert version() >= 180000, "libpq 18 is required"
print("external libpq version:", version(), flush=True)

# Differential mode keeps independent observations from both real endpoints.
def date_differential(aster_dsn):
    """DATE-DIFF: independent libpq observations plus explicit-length wire probes."""
    import copy
    import json
    import socket
    import struct
    import subprocess
    import hashlib
    from pathlib import Path

    repository = Path(__file__).resolve().parents[3]
    reference = os.environ.get('PG18_REFERENCE_DSN')
    assert reference, 'PG18_REFERENCE_DSN is required (no skip)'
    evidence = Path(os.environ['PG_DATE_EVIDENCE_DIR'])
    evidence.mkdir(parents=True, exist_ok=True)
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    fmt = api('PQfformat', integer, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    length = api('PQgetlength', integer, ptr, integer, integer)
    raw_value = api('PQgetvalue', ptr, ptr, integer, integer)
    pq_socket = api('PQsocket', integer, ptr)
    notice_api = api('PQsetNoticeReceiver', ptr, ptr, ptr, ptr)
    notices = []
    @c.CFUNCTYPE(None, ptr, ptr)
    def on_notice(arg, result):
        notices.append((ef(result, ord('C')) or b'').decode())

    def observe(result):
        assert result, 'libpq returned no result'
        try:
            return dict(status=result_status(result), state=(ef(result, ord('C')) or b'').decode(),
                        metadata=[[field_type(result, i), size(result, i), mod(result, i), fmt(result, i)]
                                  for i in range(columns(result))],
                        parameters=[pt(result, i) for i in range(np(result))],
                        rows=[[None if is_null(result, r, i) else
                               c.string_at(raw_value(result, r, i), length(result, r, i)).hex()
                               for i in range(columns(result))] for r in range(rows(result))],
                        warnings=list(notices))
        finally:
            notices.clear()
            clear(result)

    def scalar(obs):
        assert obs['status'] == 2 and len(obs['rows']) == 1, obs
        return bytes.fromhex(obs['rows'][0][0]).decode()

    class Wire:
        # Startup and authentication are real libpq; duplicate only its idle socket.
        # No libpq operations run while these raw message probes own the connection.
        def __init__(self, dsn):
            self.conn = connect(dsn.encode())
            assert self.conn and status(self.conn) == 0, 'wire connection failed'
            self.raw = socket.socket(fileno=os.dup(pq_socket(self.conn)))
            self.blocking = os.get_blocking(pq_socket(self.conn))
            self.raw.settimeout(10)
            self.diagnostics = []
        def close(self):
            self.raw.close()
            os.set_blocking(pq_socket(self.conn), self.blocking)
            finish(self.conn)
        def send(self, tag, body):
            self.raw.sendall(tag + struct.pack('!I', len(body) + 4) + body)
        def exact(self, n):
            data = b''
            while len(data) < n:
                part = self.raw.recv(n-len(data))
                assert part, 'unexpected EOF'
                data += part
            return data
        def ready(self):
            obs = dict(tags='', descriptions=[], parameters=[], rows=[], errors=[], warnings=[], ready='')
            while True:
                tag = self.exact(1).decode()
                n = struct.unpack('!I', self.exact(4))[0]
                assert 4 <= n < 1 << 20
                body = self.exact(n-4)
                # ParameterStatus and NoticeResponse are async, not phase delimiters.
                if tag not in ('S', 'N'):
                    obs['tags'] += tag
                if tag == 'T':
                    count = struct.unpack('!H', body[:2])[0]
                    at, fields = 2, []
                    for _ in range(count):
                        at = body.index(b'\0', at) + 1
                        _, _, oid, width, typmod, output = struct.unpack('!IhIhih', body[at:at+18])
                        at += 18
                        fields.append([oid, width, typmod, output])
                    obs['descriptions'].append(fields)
                elif tag == 't':
                    count = struct.unpack('!H', body[:2])[0]
                    obs['parameters'] = list(struct.unpack('!'+'I'*count, body[2:]))
                elif tag == 'D':
                    count = struct.unpack('!H', body[:2])[0]
                    at, cells = 2, []
                    for _ in range(count):
                        size_ = struct.unpack('!i', body[at:at+4])[0]
                        at += 4
                        assert size_ >= -1
                        cells.append(dict(length=size_, hex=None if size_ == -1 else body[at:at+size_].hex()))
                        if size_ >= 0:
                            at += size_
                    assert at == len(body)
                    obs['rows'].append(cells)
                elif tag in ('E', 'N'):
                    fields = {chr(f[0]): f[1:].decode(errors='replace') for f in body.split(b'\0') if f}
                    self.diagnostics.append(fields)
                    obs['errors' if tag == 'E' else 'warnings'].append(fields['C'])
                elif tag == 'Z':
                    obs['ready'] = body.decode()
                    return obs
        def query(self, sql):
            self.send(b'Q', sql.encode()+b'\0')
            return self.ready()
        def parse(self, sql, name=b'', parameter=True):
            self.send(b'P', name+b'\0'+sql.encode()+b'\0'+
                      (struct.pack('!HI',1,1082) if parameter else struct.pack('!H',0)))
        def bind(self, payload, input_, output, statement=b'', portal=b'', declared=None, parameter=True):
            val = struct.pack('!i', -1 if payload is None else len(payload) if declared is None else declared)
            values = struct.pack('!hhh',1,input_,1)+val+(payload or b'') if parameter else struct.pack('!hh',0,0)
            self.send(b'B',portal+b'\0'+statement+b'\0'+values+struct.pack('!hh',1,output))
        def execute(self, portal=b''):
            self.send(b'E', portal+b'\0'+struct.pack('!I', 0))
        def sync(self):
            self.send(b'S', b'')
            return self.ready()
        def cycle(self, sql, payload, input_, output, declared=None, parameter=True):
            self.parse(sql, parameter=parameter)
            self.send(b'D', b'S\0')
            self.bind(payload, input_, output, declared=declared, parameter=parameter)
            self.send(b'D', b'P\0')
            self.execute()
            self.execute()
            return self.sync()

    fixtures = [('2024-02-29', 8825), ('2000-02-29', 59),
                ('1999-12-31', -1), ('2000-01-01', 0), ('2000-01-02', 1),
                ('1970-01-01', -10957), ('4714-11-24 BC', -2451545),
                ('5874897-12-31', 2145031948), ('0001-01-01 BC', -730485),
                ('0001-12-31 BC', -730120), ('0001-01-01', -730119),
                ('9999-12-31', 2921939), ('10000-01-01', 2921940),
                ('-infinity', -2147483648), ('infinity', 2147483647)]
    observations, cleanup, identity = {}, {}, {}
    epoch, relatives, wire_epoch, style_binary = None, {}, None, {}
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn) == 0, side+' connection failed'
        notice_api(conn, c.cast(on_notice, ptr), None)
        collected, wire = {}, None
        def keep(case, result):
            assert case not in collected, case
            collected[case] = result
            return result
        def q(case, sql):
            return keep(case, observe(execute(conn, sql.encode())))
        def ok(obs):
            assert obs['status'] in (1, 2) and not obs['warnings'], (side, obs)
            return obs
        def check_value(obs, expected, output):
            assert obs['status'] == 2 and not obs['warnings'], (side, obs)
            assert obs['metadata'] == [[1082, 4, -1, output]], (side, obs)
            assert obs['rows'] == [[None if expected is None else expected.hex()]], (side, obs, expected)
        def zone_sql(zone):
            # The preceding DATE-EPOCH task exposes the native time_zone setting.
            # Use that real session interface until full PG GUC spelling lands.
            return f"SET TimeZone TO '{zone}'" if side == 'reference' else f"SET time_zone = '{zone}'"
        try:
            if side == 'reference':
                identity['server'] = observe(execute(conn, b"SELECT version(), current_setting('server_version_num')"))
                assert 180000 <= int(bytes.fromhex(identity['server']['rows'][0][1]).decode()) < 190000
                identity['libpq_version'] = version()
                identity['source_head'] = subprocess.check_output(['git','rev-parse','HEAD'], text=True,cwd=repository).strip()
                identity['source_diff_sha256'] = hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg/server','pkg/types','pkg/session','pkg/parser'],cwd=repository)).hexdigest()
                identity['rust_test_binary'] = os.environ['PG_DATE_RUST_BINARY']
                identity['rust_test_binary_sha256'] = hashlib.sha256(Path(identity['rust_test_binary']).read_bytes()).hexdigest()
                identity['cargo_lock_sha256'] = hashlib.sha256((repository/'Cargo.lock').read_bytes()).hexdigest()
                identity['fixture_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
                # Installed server builds do not always expose a tzdb release label.
                # Optional fingerprints identify its actual compiled zone data.
                tzdir = os.environ.get('PG_DATE_REFERENCE_TZDIR')
                identity['reference_tzdata'] = ({zone: hashlib.sha256((Path(tzdir)/zone).read_bytes()).hexdigest()
                    for zone in ['UTC','Asia/Shanghai','America/New_York','Etc/GMT+12','Etc/GMT-14']}
                    if tzdir else 'compiled tzdata directory not provided')
                identity['source_status'] = subprocess.check_output(['git','status','--short'], text=True,cwd=repository)
                tz = Path('/usr/share/zoneinfo/tzdata.zi')
                version_file = Path('/usr/share/zoneinfo/+VERSION')
                identity['tzdata'] = tz.read_text().splitlines()[0] if tz.exists() else version_file.read_text().strip() if version_file.exists() else 'system tzdata version unavailable'
                tz_binary = Path('/usr/share/zoneinfo/UTC')
                identity['tzdata_utc_sha256'] = hashlib.sha256(tz_binary.read_bytes()).hexdigest()
            for setting in ["SET DateStyle TO 'ISO, MDY'", zone_sql("UTC")]:
                setup = observe(execute(conn, setting.encode()))
                assert setup['status'] in (1,2), (side, setting, setup)
            identity[side] = {setting: scalar(ok(observe(execute(conn, sql.encode()))))
                              for setting, sql in [('DateStyle','SHOW DateStyle'),
                                  ('TimeZone','SHOW TimeZone' if side == 'reference' else 'SELECT @@time_zone')]}
            # IntervalStyle belongs to the interval/session lifecycle workstream;
            # record its actual availability without treating it as a date codec.
            identity[side]['IntervalStyle'] = observe(execute(conn,b'SHOW IntervalStyle'))
            statement = b'date_value'
            ok(keep('prepare', observe(prepare(conn, statement, b'SELECT $1::date AS d', 1, (c.c_uint*1)(1082)))))
            desc = keep('statement-describe', observe(describe(conn, statement)))
            assert desc['parameters'] == [1082] and desc['metadata'] == [[1082,4,-1,0]], (side, desc)
            for idx, (spelling, days) in enumerate(fixtures + [(None, None)]):
                binary = None if days is None else struct.pack('!i', days)
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if spelling is None else spelling.encode() if input_ == 0 else binary
                        result = keep(f'libpq-{idx}-{input_}-{output}', observe(prepared(
                            conn, statement, 1, (text*1)(payload), (integer*1)(0 if payload is None else len(payload)),
                            (integer*1)(input_), output)))
                        expected = None if spelling is None else spelling.encode() if output == 0 else binary
                        check_value(result, expected, output)
                if spelling is not None:
                    check_value(q(f'literal-{idx}', f"SELECT CAST('{spelling}' AS date) AS d"), spelling.encode(), 0)
            if side == 'aster':
                bridge = {}
                for output in (0,1):
                    obs = observe(params(conn,b'SELECT d FROM date_isolation WHERE id = 1',0,None,None,None,None,output))
                    bridge[str(output)] = obs
                    assert obs['status'] == 7 and obs['state'] == '22008' and not obs['rows'], obs
                    ok(observe(execute(conn,b'SELECT 1::int4')))
                (evidence/'mysql-zero-bridge.json').write_text(json.dumps(bridge,sort_keys=True))
            # Relative words resolve against the reference transaction instant, never host time.
            ok(observe(execute(conn,b'BEGIN')))
            if side == 'reference':
                epoch = scalar(ok(observe(execute(conn, b'SELECT EXTRACT(EPOCH FROM transaction_timestamp())'))))
                identity['transaction_epoch'] = epoch
            else:
                ok(observe(execute(conn,b'ROLLBACK')))
                ok(observe(execute(conn,('SET timestamp = '+epoch).encode())))
                ok(observe(execute(conn,b'BEGIN')))
            for word in ['now','today','yesterday','tomorrow','epoch']:
                result = q('relative-literal-'+word, f"SELECT CAST('{word}' AS date) AS d")
                if side == 'reference':
                    text_result = bytes.fromhex(result['rows'][0][0])
                    binary_result = observe(prepared(conn,statement,1,(text*1)(text_result),None,None,1))
                    relatives[word] = (text_result, bytes.fromhex(binary_result['rows'][0][0]))
                for input_ in (0,1):
                    for output in (0,1):
                        payload = word.encode() if input_ == 0 else relatives[word][1]
                        obs = keep(f'relative-{word}-{input_}-{output}',observe(prepared(conn,statement,1,(text*1)(payload),
                            (integer*1)(len(payload)),(integer*1)(input_),output)))
                        check_value(obs, relatives[word][output], output)
            ok(q('relative-end','ROLLBACK'))
            for order, expected in [('MDY',b'2003-01-02'),('DMY',b'2003-02-01')]:
                ok(observe(execute(conn,f"SET DateStyle TO 'ISO, {order}'".encode())))
                if side == 'reference':
                    encoded = observe(prepared(conn,statement,1,(text*1)(expected),None,None,1))
                    style_binary[order] = bytes.fromhex(encoded['rows'][0][0])
                for input_ in (0,1):
                    for output in (0,1):
                        payload = b'01/02/2003' if input_ == 0 else style_binary[order]
                        obs = keep(f'style-{order}-{input_}-{output}',observe(prepared(conn,statement,1,
                            (text*1)(payload),(integer*1)(len(payload)),(integer*1)(input_),output)))
                        check_value(obs,expected if output == 0 else style_binary[order],output)
                check_value(q('style-readback-'+order, f"SELECT CAST('{expected.decode()}' AS date) AS d"),expected,0)
            ok(observe(execute(conn,b"SET DateStyle TO 'ISO, MDY'")))
            wire = Wire(dsn)
            assert not wire.query("SET DateStyle TO 'ISO, MDY'")['errors']
            assert not wire.query(zone_sql("UTC"))['errors']
            for idx, (spelling, days) in enumerate(fixtures + [(None,None)]):
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if days is None else spelling.encode() if input_ == 0 else struct.pack('!i', days)
                        obs = keep(f'wire-{idx}-{input_}-{output}',wire.cycle('SELECT $1::date AS d',payload,input_,output))
                        assert obs['tags'] == '1tT2TDCCZ' and obs['parameters'] == [1082], (side,obs)
                        assert obs['descriptions'] == [[[1082,4,-1,0]],[[1082,4,-1,output]]], (side,obs)
                        expected = None if days is None else spelling.encode() if output == 0 else struct.pack('!i',days)
                        assert obs['rows'] == [[dict(length=-1 if expected is None else len(expected),hex=None if expected is None else expected.hex())]], (side,obs)
            for name, sql in [('null','SELECT NULL::date AS d'),
                              ('first-null',"SELECT NULL::date AS d UNION ALL SELECT CAST('2000-01-01' AS date) AS d"),
                              ('all-null','SELECT NULL::date AS d UNION ALL SELECT NULL::date AS d'),
                              ('zero-rows','SELECT $1::date AS d WHERE FALSE')]:
                for output in (0,1):
                    obs = keep(f'wire-{name}-{output}',wire.cycle(sql,None,0,output,parameter=name=='zero-rows'))
                    assert not obs['errors'] and obs['descriptions'] == [[[1082,4,-1,0]],[[1082,4,-1,output]]], (side,obs)
            # One prepared statement, multiple portals and result format switches.
            wire.parse('SELECT $1::date AS d', b'switch')
            assert wire.sync()['tags'] == '1Z'
            for output in (0,1,0):
                wire.bind(b'2000-01-01',0,output,statement=b'switch')
                wire.send(b'D',b'P\0');wire.execute()
                obs = wire.sync()
                keep(f'switch-{len(collected)}',obs)
                assert obs['descriptions'] == [[[1082,4,-1,output]]] and len(obs['rows']) == 1, obs
            bad_text = ['2023-02-29','1900-02-29','2024-04-31','2024-00-01','2024-13-01',
                        '2024-01-00','2024-01-32','0000-00-00','0000-01-01','4714-11-23 BC','5874898-01-01']
            bad = [(s.encode(),0,'22008',None) for s in bad_text]
            bad += [(s.encode(),0,'22007',None) for s in ['allballs','','garbage']]
            bad += [(b'\xff',0,'22021',None),(b'2024-02-29\0junk',0,'22021',None),
                    (b'',1,'08P01',None),(b'\0'*3,1,'08P01',None),(b'\0'*5,1,'22P03',None),
                    (struct.pack('!i',-2451546),1,'22008',None),(struct.pack('!i',2145031949),1,'22008',None),
                    (b'\0'*4,1,'08P01',100)]
            for idx,(payload,input_,state,declared) in enumerate(bad):
                for output in (0,1):
                    obs = keep(f'bad-{idx}-{output}',wire.cycle('SELECT $1::date AS d',payload,input_,output,declared))
                    assert obs['errors'] == [state] and not obs['rows'] and obs['ready'] == 'I', (side,idx,obs)
                    recovery = keep(f'recovery-{idx}-{output}',wire.query('SELECT 1::int4 AS n'))
                    assert recovery['rows'] == [[{'length':1,'hex':'31'}]] and not recovery['errors'], recovery
            for idx, spelling in enumerate(bad_text+['allballs','','garbage']):
                obs = q(f'bad-literal-{idx}', f"SELECT CAST('{spelling}' AS date) AS d")
                assert obs['state'] == ('22008' if idx < len(bad_text) else '22007'), (side,obs)
            assert wire.query('BEGIN')['ready'] == 'T'
            obs = keep('transaction-error',wire.cycle('SELECT $1::date AS d',b'2023-02-29',0,1))
            assert obs['errors'] == ['22008'] and obs['ready'] == 'E',obs
            assert wire.query('ROLLBACK')['ready'] == 'I'
            keep('transaction-recovery',wire.query('SELECT 1::int4 AS n'))
            # Changing session zone cannot move an already bound date.
            wire.parse('SELECT $1::date AS d',b'zone')
            wire.sync()
            for zone in ['UTC','Asia/Shanghai','America/New_York']:
                assert not wire.query(zone_sql(zone))['errors']
                wire.bind(b'1999-12-31',0,1,statement=b'zone')
                wire.execute()
                obs=keep('zone-'+zone,wire.sync())
                assert obs['rows'] == [[dict(length=4,hex='ffffffff')]],obs
            if side == 'reference':
                assert wire.query('BEGIN')['ready'] == 'T'
                instant = wire.query('SELECT EXTRACT(EPOCH FROM transaction_timestamp())')
                wire_epoch = bytes.fromhex(instant['rows'][0][0]['hex']).decode()
                identity['wire_transaction_epoch'] = wire_epoch
            else:
                assert not wire.query('SET timestamp = '+wire_epoch)['errors']
                assert wire.query('BEGIN')['ready'] == 'T'
            # Parse a relative literal, then change zone without sleeping. Two zones
            # 26 hours apart guarantee a changed calendar day at the same instant.
            assert not wire.query(zone_sql("Etc/GMT+12"))['errors']
            wire.parse("SELECT CAST('today' AS date) AS d",b'constant',parameter=False)
            assert not wire.sync()['errors']
            before=keep('prepare-relative-before',wire.query("SELECT CAST('today' AS date) AS d"))
            assert not wire.query(zone_sql("Etc/GMT-14"))['errors']
            wire.bind(None,0,0,statement=b'constant',parameter=False)
            wire.execute()
            parsed=keep('prepare-relative-execute',wire.sync())
            assert parsed['rows'] == before['rows'],(side,before,parsed)
            after=keep('prepare-relative-after',wire.query("SELECT CAST('today' AS date) AS d"))
            assert after['rows'] != before['rows'],(side,before,after)
            wire.parse('SELECT $1::date AS d',b'bound')
            wire.sync()
            wire.bind(b'today',0,0,statement=b'bound',portal=b'p')
            assert not wire.sync()['errors']
            assert not wire.query(zone_sql("Etc/GMT+12"))['errors']
            wire.execute(b'p')
            bound=keep('bind-relative-execute',wire.sync())
            assert bound['rows'] == after['rows'],(side,after,bound)
            if side == 'aster':
                print('PG_DATE_MID',flush=True)
                assert sys.stdin.readline().strip() == 'continue'
            observations[side]=collected
        finally:
            if wire is not None:
                (evidence/(side+'-diagnostics.json')).write_text(json.dumps(wire.diagnostics,sort_keys=True))
                try:wire.query('ROLLBACK')
                finally:wire.close()
            rolled=observe(execute(conn,b'ROLLBACK'))
            notices.clear()
            finish(conn)
            cleanup[side]=rolled['status'] == 1
            (evidence/(side+'.json')).write_text(json.dumps(collected,sort_keys=True))
            (evidence/'identity.json').write_text(json.dumps(identity,sort_keys=True))
            (evidence/'cleanup.json').write_text(json.dumps(cleanup))
    assert observations['aster'].keys() == observations['reference'].keys()
    for case, actual in observations['aster'].items():
        assert actual == observations['reference'][case], (case,actual,observations['reference'][case])
    for key, replacement in [('rows',[['00']]),('metadata',[[25,-1,-1,0]]),('state','XXXXX')]:
        changed=copy.deepcopy(observations['reference'])
        changed['libpq-0-0-0'][key]=replacement
        assert changed != observations['aster'], key
    summary=dict(cases=len(observations['aster']),negative_controls=3,cleanup=cleanup,passed=True,
                 capabilities=['DATE-CALENDAR','DATE-EPOCH','DATE-RANGE','DATE-ZERO',
                               'DATE-SPECIAL-STYLE','DATE-NULL-METADATA','DATE-INVALID-BINARY'])
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('date differential:',json.dumps(summary),flush=True)


def time_differential(aster_dsn):
    """TIME-DIFF: independent libpq observations plus explicit-length wire probes."""
    import copy
    import json
    import socket
    import struct
    import subprocess
    import hashlib
    from pathlib import Path

    repository = Path(__file__).resolve().parents[3]
    reference = os.environ.get('PG18_REFERENCE_DSN')
    assert reference, 'PG18_REFERENCE_DSN is required (no skip)'
    evidence = Path(os.environ['PG_TIME_EVIDENCE_DIR'])
    evidence.mkdir(parents=True, exist_ok=True)
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    fmt = api('PQfformat', integer, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    length = api('PQgetlength', integer, ptr, integer, integer)
    raw_value = api('PQgetvalue', ptr, ptr, integer, integer)
    pq_socket = api('PQsocket', integer, ptr)
    notice_api = api('PQsetNoticeReceiver', ptr, ptr, ptr, ptr)
    notices = []
    @c.CFUNCTYPE(None, ptr, ptr)
    def on_notice(arg, result):
        notices.append((ef(result, ord('C')) or b'').decode())

    def observe(result):
        assert result, 'libpq returned no result'
        try:
            return dict(status=result_status(result), state=(ef(result, ord('C')) or b'').decode(),
                        metadata=[[field_type(result, i), size(result, i), mod(result, i), fmt(result, i)]
                                  for i in range(columns(result))],
                        parameters=[pt(result, i) for i in range(np(result))],
                        rows=[[None if is_null(result, r, i) else
                               c.string_at(raw_value(result, r, i), length(result, r, i)).hex()
                               for i in range(columns(result))] for r in range(rows(result))],
                        warnings=list(notices))
        finally:
            notices.clear()
            clear(result)

    def scalar(obs):
        assert obs['status'] == 2 and len(obs['rows']) == 1, obs
        return bytes.fromhex(obs['rows'][0][0]).decode()

    class Wire:
        # Startup and authentication are real libpq; duplicate only its idle socket.
        # No libpq operations run while these raw message probes own the connection.
        def __init__(self, dsn):
            self.conn = connect(dsn.encode())
            assert self.conn and status(self.conn) == 0, 'wire connection failed'
            self.raw = socket.socket(fileno=os.dup(pq_socket(self.conn)))
            self.blocking = os.get_blocking(pq_socket(self.conn))
            self.raw.settimeout(10)
            self.diagnostics = []
        def close(self):
            self.raw.close()
            os.set_blocking(pq_socket(self.conn), self.blocking)
            finish(self.conn)
        def send(self, tag, body):
            self.raw.sendall(tag + struct.pack('!I', len(body) + 4) + body)
        def exact(self, n):
            data = b''
            while len(data) < n:
                part = self.raw.recv(n-len(data))
                assert part, 'unexpected EOF'
                data += part
            return data
        def ready(self):
            obs = dict(tags='', descriptions=[], parameters=[], rows=[], errors=[], warnings=[], ready='')
            while True:
                tag = self.exact(1).decode()
                n = struct.unpack('!I', self.exact(4))[0]
                assert 4 <= n < 1 << 20
                body = self.exact(n-4)
                # ParameterStatus and NoticeResponse are async, not phase delimiters.
                if tag not in ('S', 'N'):
                    obs['tags'] += tag
                if tag == 'T':
                    count = struct.unpack('!H', body[:2])[0]
                    at, fields = 2, []
                    for _ in range(count):
                        at = body.index(b'\0', at) + 1
                        _, _, oid, width, typmod, output = struct.unpack('!IhIhih', body[at:at+18])
                        at += 18
                        fields.append([oid, width, typmod, output])
                    obs['descriptions'].append(fields)
                elif tag == 't':
                    count = struct.unpack('!H', body[:2])[0]
                    obs['parameters'] = list(struct.unpack('!'+'I'*count, body[2:]))
                elif tag == 'D':
                    count = struct.unpack('!H', body[:2])[0]
                    at, cells = 2, []
                    for _ in range(count):
                        size_ = struct.unpack('!i', body[at:at+4])[0]
                        at += 4
                        assert size_ >= -1
                        cells.append(dict(length=size_, hex=None if size_ == -1 else body[at:at+size_].hex()))
                        if size_ >= 0:
                            at += size_
                    assert at == len(body)
                    obs['rows'].append(cells)
                elif tag in ('E', 'N'):
                    fields = {chr(f[0]): f[1:].decode(errors='replace') for f in body.split(b'\0') if f}
                    self.diagnostics.append(fields)
                    obs['errors' if tag == 'E' else 'warnings'].append(fields['C'])
                elif tag == 'Z':
                    obs['ready'] = body.decode()
                    return obs
        def query(self, sql):
            self.send(b'Q', sql.encode()+b'\0')
            return self.ready()
        def parse(self, sql, name=b'', parameter=True):
            self.send(b'P', name+b'\0'+sql.encode()+b'\0'+
                      (struct.pack('!HI',1,1083) if parameter else struct.pack('!H',0)))
        def bind(self, payload, input_, output, statement=b'', portal=b'', declared=None, parameter=True):
            val = struct.pack('!i', -1 if payload is None else len(payload) if declared is None else declared)
            values = struct.pack('!hhh',1,input_,1)+val+(payload or b'') if parameter else struct.pack('!hh',0,0)
            self.send(b'B',portal+b'\0'+statement+b'\0'+values+struct.pack('!hh',1,output))
        def execute(self, portal=b''):
            self.send(b'E', portal+b'\0'+struct.pack('!I', 0))
        def sync(self):
            self.send(b'S', b'')
            return self.ready()
        def cycle(self, sql, payload, input_, output, declared=None, parameter=True):
            self.parse(sql, parameter=parameter)
            self.send(b'D', b'S\0')
            self.bind(payload, input_, output, declared=declared, parameter=parameter)
            self.send(b'D', b'P\0')
            self.execute()
            self.execute()
            return self.sync()

    fixtures = [('00:00:00', 0), ('23:59:59.999999', 86399999999),
                ('24:00:00', 86400000000), ('12:34:56.123456',45296123456),
                ('04:05:00',14700000000), ('04:05:06',14706000000),
                ('16:05:00',57900000000)]
    observations, cleanup, identity = {}, {}, {}
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn) == 0, side+' connection failed'
        notice_api(conn, c.cast(on_notice, ptr), None)
        collected, wire = {}, None
        def keep(case, result):
            assert case not in collected, case
            collected[case] = result
            return result
        def q(case, sql):
            return keep(case, observe(execute(conn, sql.encode())))
        def ok(obs):
            assert obs['status'] in (1, 2) and not obs['warnings'], (side, obs)
            return obs
        def check_value(obs, expected, output):
            assert obs['status'] == 2 and not obs['warnings'], (side, obs)
            assert obs['metadata'] == [[1083, 8, -1, output]], (side, obs)
            assert obs['rows'] == [[None if expected is None else expected.hex()]], (side, obs, expected)
        def zone_sql(zone):
            # The preceding temporal tasks expose the native time_zone setting.
            # Use that real session interface until full PG GUC spelling lands.
            return f"SET TimeZone TO '{zone}'" if side == 'reference' else f"SET time_zone = '{zone}'"
        try:
            if side == 'reference':
                identity['server'] = observe(execute(conn, b"SELECT version(), current_setting('server_version_num')"))
                assert 180000 <= int(bytes.fromhex(identity['server']['rows'][0][1]).decode()) < 190000
                identity['libpq_version'] = version()
                identity['source_head'] = subprocess.check_output(['git','rev-parse','HEAD'], text=True,cwd=repository).strip()
                identity['source_diff_sha256'] = hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg/server','pkg/types','pkg/session','pkg/parser'],cwd=repository)).hexdigest()
                identity['rust_test_binary'] = os.environ['PG_TIME_RUST_BINARY']
                identity['rust_test_binary_sha256'] = hashlib.sha256(Path(identity['rust_test_binary']).read_bytes()).hexdigest()
                identity['cargo_lock_sha256'] = hashlib.sha256((repository/'Cargo.lock').read_bytes()).hexdigest()
                identity['fixture_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
                # Installed server builds do not always expose a tzdb release label.
                # Optional fingerprints identify its actual compiled zone data.
                tzdir = os.environ.get('PG_TIME_REFERENCE_TZDIR')
                identity['reference_tzdata'] = ({zone: hashlib.sha256((Path(tzdir)/zone).read_bytes()).hexdigest()
                    for zone in ['UTC','Asia/Shanghai','America/New_York','Etc/GMT+12','Etc/GMT-14']}
                    if tzdir else 'compiled tzdata directory not provided')
                identity['source_status'] = subprocess.check_output(['git','status','--short'], text=True,cwd=repository)
                tz = Path('/usr/share/zoneinfo/tzdata.zi')
                version_file = Path('/usr/share/zoneinfo/+VERSION')
                identity['tzdata'] = tz.read_text().splitlines()[0] if tz.exists() else version_file.read_text().strip() if version_file.exists() else 'system tzdata version unavailable'
                tz_binary = Path('/usr/share/zoneinfo/UTC')
                identity['tzdata_utc_sha256'] = hashlib.sha256(tz_binary.read_bytes()).hexdigest()
            for setting in ["SET DateStyle TO 'ISO, MDY'", zone_sql("UTC")]:
                setup = observe(execute(conn, setting.encode()))
                assert setup['status'] in (1,2), (side, setting, setup)
            identity[side] = {setting: scalar(ok(observe(execute(conn, sql.encode()))))
                              for setting, sql in [('DateStyle','SHOW DateStyle'),
                                  ('TimeZone','SHOW TimeZone' if side == 'reference' else 'SELECT @@time_zone')]}
            # IntervalStyle belongs to the interval/session lifecycle workstream;
            # record its actual availability without treating it as a time codec.
            identity[side]['IntervalStyle'] = observe(execute(conn,b'SHOW IntervalStyle'))
            statement = b'time_value'
            ok(keep('prepare', observe(prepare(conn, statement, b'SELECT $1::time AS d', 1, (c.c_uint*1)(1083)))))
            desc = keep('statement-describe', observe(describe(conn, statement)))
            assert desc['parameters'] == [1083] and desc['metadata'] == [[1083,8,-1,0]], (side, desc)
            for idx, (spelling, micros) in enumerate(fixtures + [(None, None)]):
                binary = None if micros is None else struct.pack('!q', micros)
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if spelling is None else spelling.encode() if input_ == 0 else binary
                        result = keep(f'libpq-{idx}-{input_}-{output}', observe(prepared(
                            conn, statement, 1, (text*1)(payload), (integer*1)(0 if payload is None else len(payload)),
                            (integer*1)(input_), output)))
                        expected = None if spelling is None else spelling.encode() if output == 0 else binary
                        check_value(result, expected, output)
                if spelling is not None:
                    check_value(q(f'literal-{idx}', f"SELECT CAST('{spelling}' AS time) AS d"), spelling.encode(), 0)
            if side == 'aster':
                bridge = {}
                for row in (1,2,3):
                    for output in (0,1):
                        obs = observe(params(conn,f'SELECT t FROM time_isolation WHERE id = {row}'.encode(),0,None,None,None,None,output))
                        bridge[f'{row}-{output}'] = obs
                        assert obs['status'] == 7 and obs['state'] == '0A000' and not obs['rows'], obs
                        ok(observe(execute(conn,b'SELECT 1::int4')))
                (evidence/'mysql-duration-bridge.json').write_text(json.dumps(bridge,sort_keys=True))
            for idx,(source,canonical,micros) in enumerate([
                    ('04:05','04:05:00',14700000000),('040506','04:05:06',14706000000),
                    ('04:05 PM','16:05:00',57900000000),('allballs','00:00:00',0),
                    ('04:05:06+08:00','04:05:06',14706000000),
                    ('12:34:56.1234567','12:34:56.123457',45296123457)]):
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else struct.pack('!q',micros)
                        obs=keep(f'forms-{idx}-{input_}-{output}',observe(prepared(conn,statement,1,
                            (text*1)(payload),(integer*1)(len(payload)),(integer*1)(input_),output)))
                        check_value(obs,canonical.encode() if output==0 else struct.pack('!q',micros),output)
            for precision,source,micros,canonical in [(0,'12:34:56.5',45296500000,'12:34:57'),
                    (3,'12:34:56.1234',45296123400,'12:34:56.123'),
                    (3,'12:34:56.1235',45296123500,'12:34:56.124'),
                    (0,'23:59:59.999999',86399999999,'24:00:00'),
                    (3,'23:59:59.999999',86399999999,'24:00:00'),
                    (6,'12:34:56.123456',45296123456,'12:34:56.123456')]:
                stmt=f'p{len(collected)}'.encode()
                ok(observe(prepare(conn,stmt,f'SELECT $1::time({precision}) AS d'.encode(),1,(c.c_uint*1)(1083))))
                desc=keep(f'precision-describe-{len(collected)}',observe(describe(conn,stmt)))
                assert desc['parameters']==[1083] and desc['metadata']==[[1083,8,precision,0]],(side,desc)
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else struct.pack('!q',micros)
                        obs=keep(f'precision-{len(collected)}',observe(prepared(conn,stmt,1,(text*1)(payload),
                            (integer*1)(len(payload)),(integer*1)(input_),output)))
                        assert obs['status']==2 and not obs['warnings'] and obs['metadata']==[[1083,8,precision,output]],(side,obs)
                        # Explicit expected integer rounding uses the canonical text, never Python ties-to-even.
                        h,m,sec=canonical.split(':'); whole,_,fraction=sec.partition('.')
                        result_micros=(int(h)*3600+int(m)*60+int(whole))*1000000+int(fraction.ljust(6,'0') or '0')
                        expected=canonical.encode() if output==0 else struct.pack('!q',result_micros)
                        assert obs['rows']==[[expected.hex()]],(side,obs)
            # gram.y ConstDatetime accepts Iconst, not SignedIconst. These
            # declarations never reach date.c anytime_typmod_check.
            for precision in ('-1','-0','+3'):
                for spelling in (f"CAST('12:00:00' AS time({precision}))",f"NULL::time({precision})"):
                    obs=q(f'signed-precision-{len(collected)}','SELECT '+spelling)
                    assert obs['state']=='42601',(side,obs)
                    ok(observe(execute(conn,b'SELECT 1::int4')))
            if side=='reference':
                internal=observe(execute(conn,b"SELECT pg_catalog.timetypmodin(ARRAY['-1']::cstring[])"))
                assert internal['state']=='22023',internal
                identity['reference_internal_negative_typmod']=internal
            warning=q('precision-warning',"SELECT CAST('12:00:00.123456' AS time(7))")
            assert warning['warnings']==['22023'] and warning['metadata']==[[1083,8,6,0]],(side,warning)
            wire = Wire(dsn)
            assert not wire.query("SET DateStyle TO 'ISO, MDY'")['errors']
            assert not wire.query(zone_sql("UTC"))['errors']
            for precision in ('-1','-0','+3'):
                for sql in [f'SELECT NULL::time({precision})',f'SELECT CAST(NULL AS time({precision}))']:
                    for parameter in (False,True):
                        obs=keep(f'signed-parse-{len(collected)}',wire.cycle(sql,None,0,0,parameter=parameter))
                        assert obs['tags']=='EZ' and obs['errors']==['42601'] and obs['ready']=='I', (side,obs)
                        recovery=wire.query('SELECT 1::int4')
                        assert not recovery['errors'] and recovery['rows']==[[dict(length=1,hex='31')]],(side,recovery)
            for idx, (spelling, micros) in enumerate(fixtures + [(None,None)]):
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if micros is None else spelling.encode() if input_ == 0 else struct.pack('!q', micros)
                        obs = keep(f'wire-{idx}-{input_}-{output}',wire.cycle('SELECT $1::time AS d',payload,input_,output))
                        assert obs['tags'] == '1tT2TDCCZ' and obs['parameters'] == [1083], (side,obs)
                        assert obs['descriptions'] == [[[1083,8,-1,0]],[[1083,8,-1,output]]], (side,obs)
                        expected = None if micros is None else spelling.encode() if output == 0 else struct.pack('!q',micros)
                        assert obs['rows'] == [[dict(length=-1 if expected is None else len(expected),hex=None if expected is None else expected.hex())]], (side,obs)
            for name, sql in [('null','SELECT NULL::time AS d'),
                              ('first-null',"SELECT NULL::time AS d UNION ALL SELECT CAST('00:00:00' AS time) AS d"),
                              ('all-null','SELECT NULL::time AS d UNION ALL SELECT NULL::time AS d'),
                              ('zero-rows','SELECT $1::time AS d WHERE FALSE')]:
                for output in (0,1):
                    obs = keep(f'wire-{name}-{output}',wire.cycle(sql,None,0,output,parameter=name=='zero-rows'))
                    assert not obs['errors'] and obs['descriptions'] == [[[1083,8,-1,0]],[[1083,8,-1,output]]], (side,obs)
            for precision in (0,3,6):
                for name,sql in [('typed',f'SELECT $1::time({precision}) AS d'),
                        ('null',f'SELECT NULL::time({precision}) AS d'),
                        ('first-null',f"SELECT NULL::time({precision}) AS d UNION ALL SELECT CAST('00:00:00' AS time({precision})) AS d"),
                        ('all-null',f'SELECT NULL::time({precision}) AS d UNION ALL SELECT NULL::time({precision}) AS d'),
                        ('zero-rows',f'SELECT $1::time({precision}) AS d WHERE FALSE')]:
                    for input_ in (0,1):
                        for output in (0,1):
                            obs=keep(f'null-{precision}-{name}-{input_}-{output}',wire.cycle(sql,None,input_,output,
                                parameter=name in ('typed','zero-rows')))
                            assert not obs['errors'] and obs['descriptions']==[[[1083,8,precision,0]],[[1083,8,precision,output]]],(side,obs)
                            expected=[] if name=='zero-rows' else [[dict(length=-1,hex=None)]]
                            if name=='first-null':
                                zero=b'00:00:00' if output==0 else b'\0'*8
                                expected.append([dict(length=8,hex=zero.hex())])
                            elif name=='all-null': expected.append([dict(length=-1,hex=None)])
                            assert obs['rows']==expected,(side,obs)
            # One prepared statement, multiple portals and result format switches.
            wire.parse('SELECT $1::time AS d', b'switch')
            assert wire.sync()['tags'] == '1Z'
            for output in (0,1,0):
                wire.bind(b'00:00:00',0,output,statement=b'switch')
                wire.send(b'D',b'P\0');wire.execute()
                obs = wire.sync()
                keep(f'switch-{len(collected)}',obs)
                assert obs['descriptions'] == [[[1083,8,-1,output]]] and len(obs['rows']) == 1, obs
            bad_text = ['24:00:00.000001','25:00:00','12:60:00','12:00:61']
            bad = [(s.encode(),0,'22008',None) for s in bad_text]
            bad += [(s.encode(),0,'22007',None) for s in ['','garbage']]
            bad += [(b'\xff',0,'22021',None),(b'12:00:00\0junk',0,'22021',None),
                    (b'',1,'08P01',None),(b'\0'*7,1,'08P01',None),(b'\0'*9,1,'22P03',None)]
            bad += [(struct.pack('!q',n),1,'22008',None) for n in [-1,86400000001,-9223372036854775808,9223372036854775807]]
            bad += [(b'\0'*8,1,'08P01',100)]
            for idx,(payload,input_,state,declared) in enumerate(bad):
                for output in (0,1):
                    obs = keep(f'bad-{idx}-{output}',wire.cycle('SELECT $1::time AS d',payload,input_,output,declared))
                    assert obs['errors'] == [state] and not obs['rows'] and obs['ready'] == 'I', (side,idx,obs)
                    recovery = keep(f'recovery-{idx}-{output}',wire.query('SELECT 1::int4 AS n'))
                    assert recovery['rows'] == [[{'length':1,'hex':'31'}]] and not recovery['errors'], recovery
            for idx, spelling in enumerate(bad_text+['','garbage']):
                obs = q(f'bad-literal-{idx}', f"SELECT CAST('{spelling}' AS time) AS d")
                assert obs['state'] == ('22008' if idx < len(bad_text) else '22007'), (side,obs)
            assert wire.query('BEGIN')['ready'] == 'T'
            obs = keep('transaction-error',wire.cycle('SELECT $1::time AS d',b'25:00:00',0,1))
            assert obs['errors'] == ['22008'] and obs['ready'] == 'E',obs
            assert wire.query('ROLLBACK')['ready'] == 'I'
            keep('transaction-recovery',wire.query('SELECT 1::int4 AS n'))
            # Changing session zone cannot move an already bound time.
            wire.parse('SELECT $1::time AS d',b'zone')
            wire.sync()
            for zone in ['UTC','Asia/Shanghai','America/New_York']:
                assert not wire.query(zone_sql(zone))['errors']
                wire.bind(b'04:05:06+08:00',0,1,statement=b'zone')
                wire.execute()
                obs=keep('zone-'+zone,wire.sync())
                assert obs['rows'] == [[dict(length=8,hex=struct.pack('!q',14706000000).hex())]],obs
            if side == 'aster':
                print('PG_TIME_MID',flush=True)
                assert sys.stdin.readline().strip() == 'continue'
            observations[side]=collected
        finally:
            if wire is not None:
                (evidence/(side+'-diagnostics.json')).write_text(json.dumps(wire.diagnostics,sort_keys=True))
                try:wire.query('ROLLBACK')
                finally:wire.close()
            rolled=observe(execute(conn,b'ROLLBACK'))
            notices.clear()
            finish(conn)
            cleanup[side]=rolled['status'] == 1
            (evidence/(side+'.json')).write_text(json.dumps(collected,sort_keys=True))
            (evidence/'identity.json').write_text(json.dumps(identity,sort_keys=True))
            (evidence/'cleanup.json').write_text(json.dumps(cleanup))
    assert observations['aster'].keys() == observations['reference'].keys()
    for case, actual in observations['aster'].items():
        assert actual == observations['reference'][case], (case,actual,observations['reference'][case])
    for key, replacement in [('rows',[['00']]),('metadata',[[25,-1,-1,0]]),('state','XXXXX')]:
        changed=copy.deepcopy(observations['reference'])
        changed['libpq-0-0-0'][key]=replacement
        assert changed != observations['aster'], key
    summary=dict(cases=len(observations['aster']),negative_controls=3,cleanup=cleanup,passed=True,
                 capabilities=['TIME-DAY','TIME-PRECISION','TIME-TEXT-ZONE','TIME-DURATION',
                               'TIME-NULL-METADATA','TIME-INVALID'])
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('time differential:',json.dumps(summary),flush=True)


def timestamp_differential(aster_dsn):
    """TS-DIFF: independent libpq observations plus explicit-length wire probes."""
    import copy
    import json
    import socket
    import struct
    import subprocess
    import hashlib
    from pathlib import Path

    repository = Path(__file__).resolve().parents[3]
    reference = os.environ.get('PG18_REFERENCE_DSN')
    assert reference, 'PG18_REFERENCE_DSN is required (no skip)'
    evidence = Path(os.environ['PG_TS_EVIDENCE_DIR'])
    evidence.mkdir(parents=True, exist_ok=True)
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    fmt = api('PQfformat', integer, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    length = api('PQgetlength', integer, ptr, integer, integer)
    raw_value = api('PQgetvalue', ptr, ptr, integer, integer)
    pq_socket = api('PQsocket', integer, ptr)
    notice_api = api('PQsetNoticeReceiver', ptr, ptr, ptr, ptr)
    notices = []
    @c.CFUNCTYPE(None, ptr, ptr)
    def on_notice(arg, result):
        notices.append((ef(result, ord('C')) or b'').decode())

    def observe(result):
        assert result, 'libpq returned no result'
        try:
            return dict(status=result_status(result), state=(ef(result, ord('C')) or b'').decode(),
                        metadata=[[field_type(result, i), size(result, i), mod(result, i), fmt(result, i)]
                                  for i in range(columns(result))],
                        parameters=[pt(result, i) for i in range(np(result))],
                        rows=[[None if is_null(result, r, i) else
                               c.string_at(raw_value(result, r, i), length(result, r, i)).hex()
                               for i in range(columns(result))] for r in range(rows(result))],
                        warnings=list(notices))
        finally:
            notices.clear()
            clear(result)

    def scalar(obs):
        assert obs['status'] == 2 and len(obs['rows']) == 1, obs
        return bytes.fromhex(obs['rows'][0][0]).decode()

    class Wire:
        # Startup and authentication are real libpq; duplicate only its idle socket.
        # No libpq operations run while these raw message probes own the connection.
        def __init__(self, dsn):
            self.conn = connect(dsn.encode())
            assert self.conn and status(self.conn) == 0, 'wire connection failed'
            self.raw = socket.socket(fileno=os.dup(pq_socket(self.conn)))
            self.blocking = os.get_blocking(pq_socket(self.conn))
            self.raw.settimeout(10)
            self.diagnostics = []
        def close(self):
            self.raw.close()
            os.set_blocking(pq_socket(self.conn), self.blocking)
            finish(self.conn)
        def send(self, tag, body):
            self.raw.sendall(tag + struct.pack('!I', len(body) + 4) + body)
        def exact(self, n):
            data = b''
            while len(data) < n:
                part = self.raw.recv(n-len(data))
                assert part, 'unexpected EOF'
                data += part
            return data
        def ready(self):
            obs = dict(tags='', descriptions=[], parameters=[], rows=[], errors=[], warnings=[], ready='')
            while True:
                tag = self.exact(1).decode()
                n = struct.unpack('!I', self.exact(4))[0]
                assert 4 <= n < 1 << 20
                body = self.exact(n-4)
                # ParameterStatus and NoticeResponse are async, not phase delimiters.
                if tag not in ('S', 'N'):
                    obs['tags'] += tag
                if tag == 'T':
                    count = struct.unpack('!H', body[:2])[0]
                    at, fields = 2, []
                    for _ in range(count):
                        at = body.index(b'\0', at) + 1
                        _, _, oid, width, typmod, output = struct.unpack('!IhIhih', body[at:at+18])
                        at += 18
                        fields.append([oid, width, typmod, output])
                    obs['descriptions'].append(fields)
                elif tag == 't':
                    count = struct.unpack('!H', body[:2])[0]
                    obs['parameters'] = list(struct.unpack('!'+'I'*count, body[2:]))
                elif tag == 'D':
                    count = struct.unpack('!H', body[:2])[0]
                    at, cells = 2, []
                    for _ in range(count):
                        size_ = struct.unpack('!i', body[at:at+4])[0]
                        at += 4
                        assert size_ >= -1
                        cells.append(dict(length=size_, hex=None if size_ == -1 else body[at:at+size_].hex()))
                        if size_ >= 0:
                            at += size_
                    assert at == len(body)
                    obs['rows'].append(cells)
                elif tag in ('E', 'N'):
                    fields = {chr(f[0]): f[1:].decode(errors='replace') for f in body.split(b'\0') if f}
                    self.diagnostics.append(fields)
                    obs['errors' if tag == 'E' else 'warnings'].append(fields['C'])
                elif tag == 'Z':
                    obs['ready'] = body.decode()
                    return obs
        def query(self, sql):
            self.send(b'Q', sql.encode()+b'\0')
            return self.ready()
        def parse(self, sql, name=b'', parameter=True):
            self.send(b'P', name+b'\0'+sql.encode()+b'\0'+
                      (struct.pack('!HI',1,1114) if parameter else struct.pack('!H',0)))
        def bind(self, payload, input_, output, statement=b'', portal=b'', declared=None, parameter=True):
            val = struct.pack('!i', -1 if payload is None else len(payload) if declared is None else declared)
            values = struct.pack('!hhh',1,input_,1)+val+(payload or b'') if parameter else struct.pack('!hh',0,0)
            self.send(b'B',portal+b'\0'+statement+b'\0'+values+struct.pack('!hh',1,output))
        def execute(self, portal=b''):
            self.send(b'E', portal+b'\0'+struct.pack('!I', 0))
        def sync(self):
            self.send(b'S', b'')
            return self.ready()
        def cycle(self, sql, payload, input_, output, declared=None, parameter=True):
            self.parse(sql, parameter=parameter)
            self.send(b'D', b'S\0')
            self.bind(payload, input_, output, declared=declared, parameter=parameter)
            self.send(b'D', b'P\0')
            self.execute()
            self.execute()
            return self.sync()

    fixtures = [('1999-12-31 23:59:59.999999', -1), ('2000-01-01 00:00:00', 0),
                ('2000-01-01 00:00:00.000001', 1), ('2024-02-29 00:00:00',762480000000000),
                ('4714-11-24 00:00:00 BC',-211813488000000000),
                ('294276-12-31 23:59:59.999999',9223371331199999999),
                ('10000-01-01 00:00:00',252455616000000000),
                ('0001-01-01 00:00:00 BC',-63113904000000000),
                ('0001-01-01 00:00:00',-63082281600000000),
                ('-infinity',-9223372036854775808),('infinity',9223372036854775807)]
    relatives = {}
    observations, cleanup, identity = {}, {}, {}
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn) == 0, side+' connection failed'
        notice_api(conn, c.cast(on_notice, ptr), None)
        collected, wire = {}, None
        def keep(case, result):
            assert case not in collected, case
            collected[case] = result
            return result
        def q(case, sql):
            return keep(case, observe(execute(conn, sql.encode())))
        def ok(obs):
            assert obs['status'] in (1, 2) and not obs['warnings'], (side, obs)
            return obs
        def check_value(obs, expected, output):
            assert obs['status'] == 2 and not obs['warnings'], (side, obs)
            assert obs['metadata'] == [[1114, 8, -1, output]], (side, obs)
            assert obs['rows'] == [[None if expected is None else expected.hex()]], (side, obs, expected)
        def zone_sql(zone):
            # The preceding temporal tasks expose the native time_zone setting.
            # Use that real session interface until full PG GUC spelling lands.
            return f"SET TimeZone TO '{zone}'" if side == 'reference' else f"SET time_zone = '{zone}'"
        try:
            if side == 'reference':
                identity['server'] = observe(execute(conn, b"SELECT version(), current_setting('server_version_num')"))
                assert 180000 <= int(bytes.fromhex(identity['server']['rows'][0][1]).decode()) < 190000
                identity['libpq_version'] = version()
                identity['source_head'] = subprocess.check_output(['git','rev-parse','HEAD'], text=True,cwd=repository).strip()
                identity['source_diff_sha256'] = hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg/server','pkg/types','pkg/session','pkg/parser'],cwd=repository)).hexdigest()
                identity['rust_test_binary'] = os.environ['PG_TS_RUST_BINARY']
                identity['rust_test_binary_sha256'] = hashlib.sha256(Path(identity['rust_test_binary']).read_bytes()).hexdigest()
                identity['cargo_lock_sha256'] = hashlib.sha256((repository/'Cargo.lock').read_bytes()).hexdigest()
                identity['fixture_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
                # Installed server builds do not always expose a tzdb release label.
                # Optional fingerprints identify its actual compiled zone data.
                tzdir = os.environ.get('PG_TS_REFERENCE_TZDIR')
                identity['reference_tzdata'] = ({zone: hashlib.sha256((Path(tzdir)/zone).read_bytes()).hexdigest()
                    for zone in ['UTC','Asia/Shanghai','America/New_York','Etc/GMT+12','Etc/GMT-14']}
                    if tzdir else 'compiled tzdata directory not provided')
                identity['source_status'] = subprocess.check_output(['git','status','--short'], text=True,cwd=repository)
                tz = Path('/usr/share/zoneinfo/tzdata.zi')
                version_file = Path('/usr/share/zoneinfo/+VERSION')
                identity['tzdata'] = tz.read_text().splitlines()[0] if tz.exists() else version_file.read_text().strip() if version_file.exists() else 'system tzdata version unavailable'
                tz_binary = Path('/usr/share/zoneinfo/UTC')
                identity['tzdata_utc_sha256'] = hashlib.sha256(tz_binary.read_bytes()).hexdigest()
            for setting in ["SET DateStyle TO 'ISO, MDY'", zone_sql("UTC")]:
                setup = observe(execute(conn, setting.encode()))
                assert setup['status'] in (1,2), (side, setting, setup)
            identity[side] = {setting: scalar(ok(observe(execute(conn, sql.encode()))))
                              for setting, sql in [('DateStyle','SHOW DateStyle'),
                                  ('TimeZone','SHOW TimeZone' if side == 'reference' else 'SELECT @@time_zone')]}
            # IntervalStyle belongs to the interval/session lifecycle workstream;
            # record its actual availability without treating it as a time codec.
            identity[side]['IntervalStyle'] = observe(execute(conn,b'SHOW IntervalStyle'))
            statement = b'timestamp_value'
            ok(keep('prepare', observe(prepare(conn, statement, b'SELECT $1::timestamp AS d', 1, (c.c_uint*1)(1114)))))
            desc = keep('statement-describe', observe(describe(conn, statement)))
            assert desc['parameters'] == [1114] and desc['metadata'] == [[1114,8,-1,0]], (side, desc)
            for idx, (spelling, micros) in enumerate(fixtures + [(None, None)]):
                binary = None if micros is None else struct.pack('!q', micros)
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if spelling is None else spelling.encode() if input_ == 0 else binary
                        result = keep(f'libpq-{idx}-{input_}-{output}', observe(prepared(
                            conn, statement, 1, (text*1)(payload), (integer*1)(0 if payload is None else len(payload)),
                            (integer*1)(input_), output)))
                        expected = None if spelling is None else spelling.encode() if output == 0 else binary
                        check_value(result, expected, output)
                if spelling is not None:
                    check_value(q(f'literal-{idx}', f"SELECT CAST('{spelling}' AS timestamp) AS d"), spelling.encode(), 0)
            if side == 'aster':
                bridge = {}
                for column, row, state in [('d',1,None),('t',1,'0A000'),('d',2,'22008')]:
                    for output in (0,1):
                        obs = observe(params(conn,f'SELECT {column} FROM ts_isolation WHERE id = {row}'.encode(),0,None,None,None,None,output))
                        bridge[f'{column}-{row}-{output}'] = obs
                        if state:
                            assert obs['status']==7 and obs['state']==state and not obs['rows'],obs
                        else:
                            assert obs['metadata']==[[1114,8,6,output]],obs
                            expected=b'2024-03-10 02:30:00' if output==0 else struct.pack('!q',763353000000000)
                            assert obs['rows']==[[expected.hex()]],obs
                        ok(observe(execute(conn,b'SELECT 1::int4')))
                (evidence/'mysql-bridge.json').write_text(json.dumps(bridge,sort_keys=True))
            for idx,(source,canonical,micros) in enumerate([
                    ('epoch','1970-01-01 00:00:00',-946684800000000),
                    ('2000-01-01 00:00:00.1234567','2000-01-01 00:00:00.123457',123457),
                    ('2004-10-19 10:23:54+02','2004-10-19 10:23:54',151496634000000)]):
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else struct.pack('!q',micros)
                        obs=keep(f'forms-{idx}-{input_}-{output}',observe(prepared(conn,statement,1,
                            (text*1)(payload),(integer*1)(len(payload)),(integer*1)(input_),output)))
                        check_value(obs,canonical.encode() if output==0 else struct.pack('!q',micros),output)
            for precision,source,micros,canonical,result_micros in [
                    (0,'2000-01-01 00:00:00.5',500000,'2000-01-01 00:00:01',1000000),
                    (0,'1999-12-31 23:59:59.5',-500000,'1999-12-31 23:59:59',-1000000),
                    (3,'2000-01-01 00:00:00.1234',123400,'2000-01-01 00:00:00.123',123000),
                    (3,'2000-01-01 00:00:00.1235',123500,'2000-01-01 00:00:00.124',124000),
                    (0,'2024-12-31 23:59:59.999999',789004799999999,'2025-01-01 00:00:00',789004800000000),
                    (3,'2024-12-31 23:59:59.999999',789004799999999,'2025-01-01 00:00:00',789004800000000),
                    (6,'2000-01-01 00:00:00.123456',123456,'2000-01-01 00:00:00.123456',123456)]:
                stmt=f'p{len(collected)}'.encode()
                ok(observe(prepare(conn,stmt,f'SELECT $1::timestamp({precision}) AS d'.encode(),1,(c.c_uint*1)(1114))))
                desc=keep(f'precision-describe-{len(collected)}',observe(describe(conn,stmt)))
                assert desc['parameters']==[1114] and desc['metadata']==[[1114,8,precision,0]],(side,desc)
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else struct.pack('!q',micros)
                        obs=keep(f'precision-{len(collected)}',observe(prepared(conn,stmt,1,(text*1)(payload),
                            (integer*1)(len(payload)),(integer*1)(input_),output)))
                        assert obs['status']==2 and not obs['warnings'] and obs['metadata']==[[1114,8,precision,output]],(side,obs)
                        expected=canonical.encode() if output==0 else struct.pack('!q',result_micros)
                        assert obs['rows']==[[expected.hex()]],(side,obs)
            # Transaction time is sampled on PG18 and injected through the existing
            # target session clock, never inferred from the Python host clock.
            for zone in ['UTC','Asia/Shanghai','America/New_York']:
                ok(observe(execute(conn,zone_sql(zone).encode())))
                if side=='reference':
                    ok(observe(execute(conn,b'BEGIN')))
                    epoch=scalar(ok(observe(execute(conn,b'SELECT EXTRACT(EPOCH FROM transaction_timestamp())'))))
                    relatives[zone]={'epoch':epoch}
                else:
                    ok(observe(execute(conn,('SET timestamp = '+relatives[zone]['epoch']).encode())))
                    ok(observe(execute(conn,b'BEGIN')))
                for word in ['now','today','yesterday','tomorrow']:
                    result=q('relative-literal-'+zone+'-'+word,f"SELECT TIMESTAMP '{word}'")
                    if side=='reference':
                        binary=observe(prepared(conn,statement,1,(text*1)(word.encode()),None,None,1))
                        relatives[zone][word]=[bytes.fromhex(result['rows'][0][0]),bytes.fromhex(binary['rows'][0][0])]
                    for input_ in (0,1):
                        for output in (0,1):
                            payload=word.encode() if input_==0 else relatives[zone][word][1]
                            obs=keep(f'relative-{zone}-{word}-{input_}-{output}',observe(prepared(conn,statement,1,
                                (text*1)(payload),(integer*1)(len(payload)),(integer*1)(input_),output)))
                            check_value(obs,relatives[zone][word][output],output)
                ok(observe(execute(conn,b'ROLLBACK')))
            ok(observe(execute(conn,zone_sql('UTC').encode())))
            # gram.y ConstDatetime accepts Iconst, not SignedIconst. These
            # declarations never reach date.c anytime_typmod_check.
            for precision in ('-1','-0','+3'):
                for spelling in (f"CAST('2000-01-01 12:00:00' AS timestamp({precision}))",f"NULL::timestamp({precision})"):
                    obs=q(f'signed-precision-{len(collected)}','SELECT '+spelling)
                    assert obs['state']=='42601',(side,obs)
                    ok(observe(execute(conn,b'SELECT 1::int4')))
            if side=='reference':
                internal=observe(execute(conn,b"SELECT pg_catalog.timestamptypmodin(ARRAY['-1']::cstring[])"))
                assert internal['state']=='22023',internal
                identity['reference_internal_negative_typmod']=internal
            warning=q('precision-warning',"SELECT CAST('2000-01-01 12:00:00.123456' AS timestamp(7))")
            assert warning['warnings']==['22023'] and warning['metadata']==[[1114,8,6,0]],(side,warning)
            wire = Wire(dsn)
            assert not wire.query("SET DateStyle TO 'ISO, MDY'")['errors']
            assert not wire.query(zone_sql("UTC"))['errors']
            for precision in ('-1','-0','+3'):
                for sql in [f'SELECT NULL::timestamp({precision})',f'SELECT CAST(NULL AS timestamp({precision}))']:
                    for parameter in (False,True):
                        obs=keep(f'signed-parse-{len(collected)}',wire.cycle(sql,None,0,0,parameter=parameter))
                        assert obs['tags']=='EZ' and obs['errors']==['42601'] and obs['ready']=='I', (side,obs)
                        recovery=wire.query('SELECT 1::int4')
                        assert not recovery['errors'] and recovery['rows']==[[dict(length=1,hex='31')]],(side,recovery)
            for idx, (spelling, micros) in enumerate(fixtures + [(None,None)]):
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if micros is None else spelling.encode() if input_ == 0 else struct.pack('!q', micros)
                        obs = keep(f'wire-{idx}-{input_}-{output}',wire.cycle('SELECT $1::timestamp AS d',payload,input_,output))
                        assert obs['tags'] == '1tT2TDCCZ' and obs['parameters'] == [1114], (side,obs)
                        assert obs['descriptions'] == [[[1114,8,-1,0]],[[1114,8,-1,output]]], (side,obs)
                        expected = None if micros is None else spelling.encode() if output == 0 else struct.pack('!q',micros)
                        assert obs['rows'] == [[dict(length=-1 if expected is None else len(expected),hex=None if expected is None else expected.hex())]], (side,obs)
            for name, sql in [('null','SELECT NULL::timestamp AS d'),
                              ('first-null',"SELECT NULL::timestamp AS d UNION ALL SELECT CAST('2000-01-01 00:00:00' AS timestamp) AS d"),
                              ('all-null','SELECT NULL::timestamp AS d UNION ALL SELECT NULL::timestamp AS d'),
                              ('zero-rows','SELECT $1::timestamp AS d WHERE FALSE')]:
                for output in (0,1):
                    obs = keep(f'wire-{name}-{output}',wire.cycle(sql,None,0,output,parameter=name=='zero-rows'))
                    assert not obs['errors'] and obs['descriptions'] == [[[1114,8,-1,0]],[[1114,8,-1,output]]], (side,obs)
            for precision in (0,3,6):
                for name,sql in [('typed',f'SELECT $1::timestamp({precision}) AS d'),
                        ('null',f'SELECT NULL::timestamp({precision}) AS d'),
                        ('first-null',f"SELECT NULL::timestamp({precision}) AS d UNION ALL SELECT CAST('2000-01-01 00:00:00' AS timestamp({precision})) AS d"),
                        ('all-null',f'SELECT NULL::timestamp({precision}) AS d UNION ALL SELECT NULL::timestamp({precision}) AS d'),
                        ('zero-rows',f'SELECT $1::timestamp({precision}) AS d WHERE FALSE')]:
                    for input_ in (0,1):
                        for output in (0,1):
                            obs=keep(f'null-{precision}-{name}-{input_}-{output}',wire.cycle(sql,None,input_,output,
                                parameter=name in ('typed','zero-rows')))
                            assert not obs['errors'] and obs['descriptions']==[[[1114,8,precision,0]],[[1114,8,precision,output]]],(side,obs)
                            expected=[] if name=='zero-rows' else [[dict(length=-1,hex=None)]]
                            if name=='first-null':
                                zero=b'2000-01-01 00:00:00' if output==0 else b'\0'*8
                                expected.append([dict(length=len(zero),hex=zero.hex())])
                            elif name=='all-null': expected.append([dict(length=-1,hex=None)])
                            assert obs['rows']==expected,(side,obs)
            # One prepared statement, multiple portals and result format switches.
            wire.parse('SELECT $1::timestamp AS d', b'switch')
            assert wire.sync()['tags'] == '1Z'
            for output in (0,1,0):
                wire.bind(b'2000-01-01 00:00:00',0,output,statement=b'switch')
                wire.send(b'D',b'P\0');wire.execute()
                obs = wire.sync()
                keep(f'switch-{len(collected)}',obs)
                assert obs['descriptions'] == [[[1114,8,-1,output]]] and len(obs['rows']) == 1, obs
            bad_text = ['2023-02-29 00:00:00','1900-02-29 00:00:00','2024-04-31 00:00:00',
                        '2024-13-01 00:00:00','2024-01-01 12:60:00',
                        '0000-00-00 00:00:00','0000-01-01 00:00:00','2024-00-01 00:00:00','2024-01-00 00:00:00']
            bad = [(s.encode(),0,'22008',None) for s in bad_text]
            bad += [(s.encode(),0,'22007',None) for s in ['','garbage','allballs']]
            bad += [(b'\xff',0,'22021',None),(b'2000-01-01 00:00:00\0junk',0,'22021',None),
                    (b'',1,'08P01',None),(b'\0'*7,1,'08P01',None),(b'\0'*9,1,'22P03',None)]
            bad += [(struct.pack('!q',n),1,'22008',None) for n in
                    [-211813488000000001,9223371331200000000,-9223372036854775807,9223372036854775806]]
            bad += [(b'\0'*8,1,'08P01',100)]
            for idx,(payload,input_,state,declared) in enumerate(bad):
                for output in (0,1):
                    obs = keep(f'bad-{idx}-{output}',wire.cycle('SELECT $1::timestamp AS d',payload,input_,output,declared))
                    assert obs['errors'] == [state] and not obs['rows'] and obs['ready'] == 'I', (side,idx,obs)
                    recovery = keep(f'recovery-{idx}-{output}',wire.query('SELECT 1::int4 AS n'))
                    assert recovery['rows'] == [[{'length':1,'hex':'31'}]] and not recovery['errors'], recovery
            for idx, spelling in enumerate(bad_text+['','garbage']):
                obs = q(f'bad-literal-{idx}', f"SELECT CAST('{spelling}' AS timestamp) AS d")
                assert obs['state'] == ('22008' if idx < len(bad_text) else '22007'), (side,obs)
            assert wire.query('BEGIN')['ready'] == 'T'
            obs = keep('transaction-error',wire.cycle('SELECT $1::timestamp AS d',b'2023-02-29 00:00:00',0,1))
            assert obs['errors'] == ['22008'] and obs['ready'] == 'E',obs
            assert wire.query('ROLLBACK')['ready'] == 'I'
            keep('transaction-recovery',wire.query('SELECT 1::int4 AS n'))
            # Retain the bound wall clock across a zone switch before Execute.
            assert wire.query('BEGIN')['ready']=='T'
            wire.parse('SELECT $1::timestamp AS timezone',b'zone')
            assert not wire.sync()['errors']
            for index,zone in enumerate(['UTC','Asia/Shanghai','America/New_York']):
                for input_ in (0,1):
                    for output in (0,1):
                        payload=b'2024-03-10 02:30:00-05:00' if input_==0 else struct.pack('!q',763353000000000)
                        wire.bind(payload,input_,output,statement=b'zone',portal=b'zonep')
                        assert not wire.sync()['errors']
                        assert not wire.query(zone_sql('America/New_York'))['errors']
                        wire.bind(payload,input_,output,statement=b'zone',portal=b'zonep')
                        assert not wire.sync()['errors']
                        assert not wire.query(zone_sql(zone))['errors']
                        wire.send(b'D',b'Pzonep\0');wire.execute(b'zonep');wire.execute(b'zonep')
                        wire.send(b'C',b'Pzonep\0')
                        obs=keep(f'zone-{index}-{input_}-{output}',wire.sync())
                        expected=b'2024-03-10 02:30:00' if output==0 else struct.pack('!q',763353000000000)
                        assert obs['descriptions']==[[[1114,8,-1,output]]] and obs['rows']==[[dict(length=len(expected),hex=expected.hex())]],obs
            assert wire.query('ROLLBACK')['ready']=='I'
            if side == 'reference':
                assert wire.query('BEGIN')['ready'] == 'T'
                instant = wire.query('SELECT EXTRACT(EPOCH FROM transaction_timestamp())')
                wire_epoch = bytes.fromhex(instant['rows'][0][0]['hex']).decode()
                identity['wire_transaction_epoch'] = wire_epoch
            else:
                assert not wire.query('SET timestamp = '+wire_epoch)['errors']
                assert wire.query('BEGIN')['ready'] == 'T'
            # Parse a relative literal, then change zone without sleeping. Two zones
            # 26 hours apart guarantee a changed calendar day at the same instant.
            assert not wire.query(zone_sql("Etc/GMT+12"))['errors']
            wire.parse("SELECT CAST('today' AS timestamp) AS d",b'constant',parameter=False)
            assert not wire.sync()['errors']
            before=keep('prepare-relative-before',wire.query("SELECT CAST('today' AS timestamp) AS d"))
            assert not wire.query(zone_sql("Etc/GMT-14"))['errors']
            wire.bind(None,0,0,statement=b'constant',parameter=False)
            wire.execute()
            parsed=keep('prepare-relative-execute',wire.sync())
            assert parsed['rows'] == before['rows'],(side,before,parsed)
            after=keep('prepare-relative-after',wire.query("SELECT CAST('today' AS timestamp) AS d"))
            assert after['rows'] != before['rows'],(side,before,after)
            wire.parse('SELECT $1::timestamp AS d',b'bound')
            wire.sync()
            wire.bind(b'today',0,0,statement=b'bound',portal=b'p')
            assert not wire.sync()['errors']
            assert not wire.query(zone_sql("Etc/GMT+12"))['errors']
            wire.execute(b'p')
            bound=keep('bind-relative-execute',wire.sync())
            assert bound['rows'] == after['rows'],(side,after,bound)
            if side == 'aster':
                print('PG_TS_MID',flush=True)
                assert sys.stdin.readline().strip() == 'continue'
            observations[side]=collected
        finally:
            if wire is not None:
                (evidence/(side+'-diagnostics.json')).write_text(json.dumps(wire.diagnostics,sort_keys=True))
                try:wire.query('ROLLBACK')
                finally:wire.close()
            rolled=observe(execute(conn,b'ROLLBACK'))
            notices.clear()
            finish(conn)
            cleanup[side]=rolled['status'] == 1
            (evidence/(side+'.json')).write_text(json.dumps(collected,sort_keys=True))
            (evidence/'identity.json').write_text(json.dumps(identity,sort_keys=True))
            (evidence/'cleanup.json').write_text(json.dumps(cleanup))
    assert observations['aster'].keys() == observations['reference'].keys()
    for case, actual in observations['aster'].items():
        assert actual == observations['reference'][case], (case,actual,observations['reference'][case])
    for key, replacement in [('rows',[['00']]),('metadata',[[25,-1,-1,0]]),('state','XXXXX')]:
        changed=copy.deepcopy(observations['reference'])
        changed['libpq-0-0-0'][key]=replacement
        assert changed != observations['aster'], key
    summary=dict(cases=len(observations['aster']),negative_controls=3,cleanup=cleanup,passed=True,
                 capabilities=['TS-EPOCH-FRACTION','TS-CALENDAR-PRECISION','TS-RANGE-ZERO','TS-ZONE-IDENTITY',
                               'TS-SPECIAL','TS-NULL-METADATA','TS-INVALID-LENGTH'])
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('timestamp differential:',json.dumps(summary),flush=True)


def timestamptz_differential(aster_dsn):
    """TZ-DIFF: independent libpq observations plus explicit-length wire probes."""
    import copy
    import json
    import socket
    import struct
    import subprocess
    import hashlib
    from pathlib import Path

    repository = Path(__file__).resolve().parents[3]
    reference = os.environ.get('PG18_REFERENCE_DSN')
    assert reference, 'PG18_REFERENCE_DSN is required (no skip)'
    evidence = Path(os.environ['PG_TZ_EVIDENCE_DIR'])
    evidence.mkdir(parents=True, exist_ok=True)
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    fmt = api('PQfformat', integer, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    length = api('PQgetlength', integer, ptr, integer, integer)
    raw_value = api('PQgetvalue', ptr, ptr, integer, integer)
    pq_socket = api('PQsocket', integer, ptr)
    notice_api = api('PQsetNoticeReceiver', ptr, ptr, ptr, ptr)
    notices = []
    @c.CFUNCTYPE(None, ptr, ptr)
    def on_notice(arg, result):
        notices.append((ef(result, ord('C')) or b'').decode())

    def observe(result):
        assert result, 'libpq returned no result'
        try:
            return dict(status=result_status(result), state=(ef(result, ord('C')) or b'').decode(),
                        metadata=[[field_type(result, i), size(result, i), mod(result, i), fmt(result, i)]
                                  for i in range(columns(result))],
                        parameters=[pt(result, i) for i in range(np(result))],
                        rows=[[None if is_null(result, r, i) else
                               c.string_at(raw_value(result, r, i), length(result, r, i)).hex()
                               for i in range(columns(result))] for r in range(rows(result))],
                        warnings=list(notices))
        finally:
            notices.clear()
            clear(result)

    def scalar(obs):
        assert obs['status'] == 2 and len(obs['rows']) == 1, obs
        return bytes.fromhex(obs['rows'][0][0]).decode()

    class Wire:
        # Startup and authentication are real libpq; duplicate only its idle socket.
        # No libpq operations run while these raw message probes own the connection.
        def __init__(self, dsn):
            self.conn = connect(dsn.encode())
            assert self.conn and status(self.conn) == 0, 'wire connection failed'
            self.raw = socket.socket(fileno=os.dup(pq_socket(self.conn)))
            self.blocking = os.get_blocking(pq_socket(self.conn))
            self.raw.settimeout(10)
            self.diagnostics = []
        def close(self):
            self.raw.close()
            os.set_blocking(pq_socket(self.conn), self.blocking)
            finish(self.conn)
        def send(self, tag, body):
            self.raw.sendall(tag + struct.pack('!I', len(body) + 4) + body)
        def exact(self, n):
            data = b''
            while len(data) < n:
                part = self.raw.recv(n-len(data))
                assert part, 'unexpected EOF'
                data += part
            return data
        def ready(self):
            obs = dict(tags='', descriptions=[], parameters=[], rows=[], errors=[], warnings=[], ready='')
            while True:
                tag = self.exact(1).decode()
                n = struct.unpack('!I', self.exact(4))[0]
                assert 4 <= n < 1 << 20
                body = self.exact(n-4)
                # ParameterStatus and NoticeResponse are async, not phase delimiters.
                if tag not in ('S', 'N'):
                    obs['tags'] += tag
                if tag == 'T':
                    count = struct.unpack('!H', body[:2])[0]
                    at, fields = 2, []
                    for _ in range(count):
                        at = body.index(b'\0', at) + 1
                        _, _, oid, width, typmod, output = struct.unpack('!IhIhih', body[at:at+18])
                        at += 18
                        fields.append([oid, width, typmod, output])
                    obs['descriptions'].append(fields)
                elif tag == 't':
                    count = struct.unpack('!H', body[:2])[0]
                    obs['parameters'] = list(struct.unpack('!'+'I'*count, body[2:]))
                elif tag == 'D':
                    count = struct.unpack('!H', body[:2])[0]
                    at, cells = 2, []
                    for _ in range(count):
                        size_ = struct.unpack('!i', body[at:at+4])[0]
                        at += 4
                        assert size_ >= -1
                        cells.append(dict(length=size_, hex=None if size_ == -1 else body[at:at+size_].hex()))
                        if size_ >= 0:
                            at += size_
                    assert at == len(body)
                    obs['rows'].append(cells)
                elif tag in ('E', 'N'):
                    fields = {chr(f[0]): f[1:].decode(errors='replace') for f in body.split(b'\0') if f}
                    self.diagnostics.append(fields)
                    obs['errors' if tag == 'E' else 'warnings'].append(fields['C'])
                elif tag == 'Z':
                    obs['ready'] = body.decode()
                    return obs
        def query(self, sql):
            self.send(b'Q', sql.encode()+b'\0')
            return self.ready()
        def parse(self, sql, name=b'', parameter=True):
            self.send(b'P', name+b'\0'+sql.encode()+b'\0'+
                      (struct.pack('!HI',1,1184) if parameter else struct.pack('!H',0)))
        def bind(self, payload, input_, output, statement=b'', portal=b'', declared=None, parameter=True):
            val = struct.pack('!i', -1 if payload is None else len(payload) if declared is None else declared)
            values = struct.pack('!hhh',1,input_,1)+val+(payload or b'') if parameter else struct.pack('!hh',0,0)
            self.send(b'B',portal+b'\0'+statement+b'\0'+values+struct.pack('!hh',1,output))
        def execute(self, portal=b''):
            self.send(b'E', portal+b'\0'+struct.pack('!I', 0))
        def sync(self):
            self.send(b'S', b'')
            return self.ready()
        def cycle(self, sql, payload, input_, output, declared=None, parameter=True):
            self.parse(sql, parameter=parameter)
            self.send(b'D', b'S\0')
            self.bind(payload, input_, output, declared=declared, parameter=parameter)
            self.send(b'D', b'P\0')
            self.execute()
            self.execute()
            return self.sync()

    fixtures = [('1999-12-31 23:59:59.999999', -1), ('2000-01-01 00:00:00', 0),
                ('2000-01-01 00:00:00.000001', 1), ('2024-02-29 00:00:00',762480000000000),
                ('4714-11-24 00:00:00 BC',-211813488000000000),
                ('294276-12-31 23:59:59.999999',9223371331199999999),
                ('10000-01-01 00:00:00',252455616000000000),
                ('0001-01-01 00:00:00 BC',-63113904000000000),
                ('0001-01-01 00:00:00',-63082281600000000),
                ('-infinity',-9223372036854775808),('infinity',9223372036854775807)]
    def utc(spelling):
        if spelling in ('infinity','-infinity'):
            return spelling
        return spelling[:-3]+'+00 BC' if spelling.endswith(' BC') else spelling+'+00'
    fixtures = [(utc(spelling), micros) for spelling,micros in fixtures]
    relatives = {}
    observations, cleanup, identity = {}, {}, {}
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn) == 0, side+' connection failed'
        notice_api(conn, c.cast(on_notice, ptr), None)
        collected, wire = {}, None
        def keep(case, result):
            assert case not in collected, case
            collected[case] = result
            return result
        def q(case, sql):
            return keep(case, observe(execute(conn, sql.encode())))
        def ok(obs):
            assert obs['status'] in (1, 2) and not obs['warnings'], (side, obs)
            return obs
        def check_value(obs, expected, output):
            assert obs['status'] == 2 and not obs['warnings'], (side, obs)
            assert obs['metadata'] == [[1184, 8, -1, output]], (side, obs)
            assert obs['rows'] == [[None if expected is None else expected.hex()]], (side, obs, expected)
        def zone_sql(zone):
            # The preceding temporal tasks expose the native time_zone setting.
            # Use that real session interface until full PG GUC spelling lands.
            return f"SET TimeZone TO '{zone}'" if side == 'reference' else f"SET time_zone = '{zone}'"
        try:
            if side == 'reference':
                identity['server'] = observe(execute(conn, b"SELECT version(), current_setting('server_version_num')"))
                assert 180000 <= int(bytes.fromhex(identity['server']['rows'][0][1]).decode()) < 190000
                identity['libpq_version'] = version()
                identity['source_head'] = subprocess.check_output(['git','rev-parse','HEAD'], text=True,cwd=repository).strip()
                identity['source_diff_sha256'] = hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg/server','pkg/types','pkg/session','pkg/parser'],cwd=repository)).hexdigest()
                identity['rust_test_binary'] = os.environ['PG_TZ_RUST_BINARY']
                identity['rust_test_binary_sha256'] = hashlib.sha256(Path(identity['rust_test_binary']).read_bytes()).hexdigest()
                identity['cargo_lock_sha256'] = hashlib.sha256((repository/'Cargo.lock').read_bytes()).hexdigest()
                identity['fixture_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
                # Installed server builds do not always expose a tzdb release label.
                # Optional fingerprints identify its actual compiled zone data.
                tzdir = os.environ.get('PG_TZ_REFERENCE_TZDIR')
                identity['reference_tzdata'] = ({zone: hashlib.sha256((Path(tzdir)/zone).read_bytes()).hexdigest()
                    for zone in ['UTC','Asia/Shanghai','America/New_York','Etc/GMT+12','Etc/GMT-14']}
                    if tzdir else 'compiled tzdata directory not provided')
                identity['source_status'] = subprocess.check_output(['git','status','--short'], text=True,cwd=repository)
                tz = Path('/usr/share/zoneinfo/tzdata.zi')
                version_file = Path('/usr/share/zoneinfo/+VERSION')
                identity['tzdata'] = tz.read_text().splitlines()[0] if tz.exists() else version_file.read_text().strip() if version_file.exists() else 'system tzdata version unavailable'
                tz_binary = Path('/usr/share/zoneinfo/UTC')
                identity['tzdata_utc_sha256'] = hashlib.sha256(tz_binary.read_bytes()).hexdigest()
            for setting in ["SET DateStyle TO 'ISO, MDY'", zone_sql("UTC")]:
                setup = observe(execute(conn, setting.encode()))
                assert setup['status'] in (1,2), (side, setting, setup)
            identity[side] = {setting: scalar(ok(observe(execute(conn, sql.encode()))))
                              for setting, sql in [('DateStyle','SHOW DateStyle'),
                                  ('TimeZone','SHOW TimeZone' if side == 'reference' else 'SELECT @@time_zone')]}
            # IntervalStyle belongs to the interval/session lifecycle workstream;
            # record its actual availability without treating it as a time codec.
            identity[side]['IntervalStyle'] = observe(execute(conn,b'SHOW IntervalStyle'))
            statement = b'timestamptz_value'
            ok(keep('prepare', observe(prepare(conn, statement, b'SELECT $1::timestamptz AS d', 1, (c.c_uint*1)(1184)))))
            desc = keep('statement-describe', observe(describe(conn, statement)))
            assert desc['parameters'] == [1184] and desc['metadata'] == [[1184,8,-1,0]], (side, desc)
            for idx, (spelling, micros) in enumerate(fixtures + [(None, None)]):
                binary = None if micros is None else struct.pack('!q', micros)
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if spelling is None else spelling.encode() if input_ == 0 else binary
                        result = keep(f'libpq-{idx}-{input_}-{output}', observe(prepared(
                            conn, statement, 1, (text*1)(payload), (integer*1)(0 if payload is None else len(payload)),
                            (integer*1)(input_), output)))
                        expected = None if spelling is None else spelling.encode() if output == 0 else binary
                        check_value(result, expected, output)
                if spelling is not None:
                    check_value(q(f'literal-{idx}', f"SELECT CAST('{spelling}' AS timestamptz) AS d"), spelling.encode(), 0)
            if side=='aster':
                # Native DATETIME requires an explicit interpretation zone;
                # TIMESTAMP has no proven source-zone provenance in this runtime.
                bridge={}
                for zone in ['UTC','Asia/Shanghai']:
                    ok(observe(execute(conn,zone_sql(zone).encode())))
                    for column,row,state in [('d',1,None),('t',1,'0A000'),('d',2,'22008')]:
                        for output in (0,1):
                            obs=observe(params(conn,f'SELECT CAST({column} AS timestamptz) FROM ts_isolation WHERE id={row}'.encode(),
                                0,None,None,None,None,output))
                            bridge[f'{zone}-{column}-{row}-{output}']=obs
                            if state:
                                assert obs['status']==7 and obs['state']==state and not obs['rows'],obs
                            else:
                                assert obs['status']==2 and obs['metadata']==[[1184,8,-1,output]],obs
                                expected=(b'2024-03-10 02:30:00+00' if zone=='UTC' else b'2024-03-10 02:30:00+08') if output==0 else struct.pack('!q',763353000000000-(28800000000 if zone=='Asia/Shanghai' else 0))
                                assert obs['rows']==[[expected.hex()]],obs
                            ok(observe(execute(conn,b'SELECT 1::int4')))
                (evidence/'mysql-bridge.json').write_text(json.dumps(bridge,sort_keys=True))
                ok(observe(execute(conn,zone_sql('UTC').encode())))
            for idx,(source,canonical,micros) in enumerate([
                    ('epoch','1970-01-01 00:00:00+00',-946684800000000),
                    ('2000-01-01 00:00:00.1234567','2000-01-01 00:00:00.123457+00',123457),
                    ('2004-10-19 10:23:54+02','2004-10-19 08:23:54+00',151489434000000)]):
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else struct.pack('!q',micros)
                        obs=keep(f'forms-{idx}-{input_}-{output}',observe(prepared(conn,statement,1,
                            (text*1)(payload),(integer*1)(len(payload)),(integer*1)(input_),output)))
                        check_value(obs,canonical.encode() if output==0 else struct.pack('!q',micros),output)
            for precision,source,micros,canonical,result_micros in [
                    (0,'2000-01-01 00:00:00.5',500000,'2000-01-01 00:00:01',1000000),
                    (0,'1999-12-31 23:59:59.5',-500000,'1999-12-31 23:59:59',-1000000),
                    (3,'2000-01-01 00:00:00.1234',123400,'2000-01-01 00:00:00.123',123000),
                    (3,'2000-01-01 00:00:00.1235',123500,'2000-01-01 00:00:00.124',124000),
                    (0,'2024-12-31 23:59:59.999999',789004799999999,'2025-01-01 00:00:00',789004800000000),
                    (3,'2024-12-31 23:59:59.999999',789004799999999,'2025-01-01 00:00:00',789004800000000),
                    (6,'2000-01-01 00:00:00.123456',123456,'2000-01-01 00:00:00.123456',123456)]:
                canonical = utc(canonical)
                stmt=f'p{len(collected)}'.encode()
                ok(observe(prepare(conn,stmt,f'SELECT $1::timestamptz({precision}) AS d'.encode(),1,(c.c_uint*1)(1184))))
                desc=keep(f'precision-describe-{len(collected)}',observe(describe(conn,stmt)))
                assert desc['parameters']==[1184] and desc['metadata']==[[1184,8,precision,0]],(side,desc)
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else struct.pack('!q',micros)
                        obs=keep(f'precision-{len(collected)}',observe(prepared(conn,stmt,1,(text*1)(payload),
                            (integer*1)(len(payload)),(integer*1)(input_),output)))
                        assert obs['status']==2 and not obs['warnings'] and obs['metadata']==[[1184,8,precision,output]],(side,obs)
                        expected=canonical.encode() if output==0 else struct.pack('!q',result_micros)
                        assert obs['rows']==[[expected.hex()]],(side,obs)
            # Transaction time is sampled on PG18 and injected through the existing
            # target session clock, never inferred from the Python host clock.
            for zone in ['UTC','Asia/Shanghai','America/New_York']:
                ok(observe(execute(conn,zone_sql(zone).encode())))
                if side=='reference':
                    ok(observe(execute(conn,b'BEGIN')))
                    epoch=scalar(ok(observe(execute(conn,b'SELECT EXTRACT(EPOCH FROM transaction_timestamp())'))))
                    relatives[zone]={'epoch':epoch}
                else:
                    ok(observe(execute(conn,('SET timestamp = '+relatives[zone]['epoch']).encode())))
                    ok(observe(execute(conn,b'BEGIN')))
                for word in ['now','today','yesterday','tomorrow']:
                    result=q('relative-literal-'+zone+'-'+word,f"SELECT TIMESTAMPTZ '{word}'")
                    if side=='reference':
                        binary=observe(prepared(conn,statement,1,(text*1)(word.encode()),None,None,1))
                        relatives[zone][word]=[bytes.fromhex(result['rows'][0][0]),bytes.fromhex(binary['rows'][0][0])]
                    for input_ in (0,1):
                        for output in (0,1):
                            payload=word.encode() if input_==0 else relatives[zone][word][1]
                            obs=keep(f'relative-{zone}-{word}-{input_}-{output}',observe(prepared(conn,statement,1,
                                (text*1)(payload),(integer*1)(len(payload)),(integer*1)(input_),output)))
                            check_value(obs,relatives[zone][word][output],output)
                ok(observe(execute(conn,b'ROLLBACK')))
            ok(observe(execute(conn,zone_sql('UTC').encode())))
            # The timestamptz alias accepts -0, rejects negative precision with
            # 22023, and rejects the unary-plus typmod expression with 42601.
            for precision in ('-1','-0','+3'):
                for spelling in (f"CAST('2000-01-01 12:00:00' AS timestamptz({precision}))",f"NULL::timestamptz({precision})"):
                    obs=q(f'signed-precision-{len(collected)}','SELECT '+spelling)
                    if precision=='-0':
                        assert obs['status']==2 and obs['metadata']==[[1184,8,0,0]],(side,obs)
                    else:
                        assert obs['state']==('22023' if precision=='-1' else '42601'),(side,obs)
                    ok(observe(execute(conn,b'SELECT 1::int4')))
            if side=='reference':
                internal=observe(execute(conn,b"SELECT pg_catalog.timestamptztypmodin(ARRAY['-1']::cstring[])"))
                assert internal['state']=='22023',internal
                identity['reference_internal_negative_typmod']=internal
            warning=q('precision-warning',"SELECT CAST('2000-01-01 12:00:00.123456' AS timestamptz(7))")
            assert warning['warnings']==['22023'] and warning['metadata']==[[1184,8,6,0]],(side,warning)
            wire = Wire(dsn)
            assert not wire.query("SET DateStyle TO 'ISO, MDY'")['errors']
            assert not wire.query(zone_sql("UTC"))['errors']
            for precision in ('-1','-0','+3'):
                for sql in [f'SELECT NULL::timestamptz({precision})',f'SELECT CAST(NULL AS timestamptz({precision}))']:
                    for parameter in (False,True):
                        if precision=='-0' and parameter:
                            continue  # Successful no-parameter SQL declares no unused parameter.
                        obs=keep(f'signed-parse-{len(collected)}',wire.cycle(sql,None,0,0,parameter=parameter))
                        if precision=='-0':
                            assert not obs['errors'] and obs['descriptions']==[[[1184,8,0,0]],[[1184,8,0,0]]],(side,obs)
                        else:
                            assert obs['tags']=='EZ' and obs['errors']==['22023' if precision=='-1' else '42601'] and obs['ready']=='I',(side,obs)
                        recovery=wire.query('SELECT 1::int4')
                        assert not recovery['errors'] and recovery['rows']==[[dict(length=1,hex='31')]],(side,recovery)
            for idx, (spelling, micros) in enumerate(fixtures + [(None,None)]):
                for input_ in (0,1):
                    for output in (0,1):
                        payload = None if micros is None else spelling.encode() if input_ == 0 else struct.pack('!q', micros)
                        obs = keep(f'wire-{idx}-{input_}-{output}',wire.cycle('SELECT $1::timestamptz AS d',payload,input_,output))
                        assert obs['tags'] == '1tT2TDCCZ' and obs['parameters'] == [1184], (side,obs)
                        assert obs['descriptions'] == [[[1184,8,-1,0]],[[1184,8,-1,output]]], (side,obs)
                        expected = None if micros is None else spelling.encode() if output == 0 else struct.pack('!q',micros)
                        assert obs['rows'] == [[dict(length=-1 if expected is None else len(expected),hex=None if expected is None else expected.hex())]], (side,obs)
            for name, sql in [('null','SELECT NULL::timestamptz AS d'),
                              ('first-null',"SELECT NULL::timestamptz AS d UNION ALL SELECT CAST('2000-01-01 00:00:00' AS timestamptz) AS d"),
                              ('all-null','SELECT NULL::timestamptz AS d UNION ALL SELECT NULL::timestamptz AS d'),
                              ('zero-rows','SELECT $1::timestamptz AS d WHERE FALSE')]:
                for output in (0,1):
                    obs = keep(f'wire-{name}-{output}',wire.cycle(sql,None,0,output,parameter=name=='zero-rows'))
                    assert not obs['errors'] and obs['descriptions'] == [[[1184,8,-1,0]],[[1184,8,-1,output]]], (side,obs)
            for precision in (0,3,6):
                for name,sql in [('typed',f'SELECT $1::timestamptz({precision}) AS d'),
                        ('null',f'SELECT NULL::timestamptz({precision}) AS d'),
                        ('first-null',f"SELECT NULL::timestamptz({precision}) AS d UNION ALL SELECT CAST('2000-01-01 00:00:00' AS timestamptz({precision})) AS d"),
                        ('all-null',f'SELECT NULL::timestamptz({precision}) AS d UNION ALL SELECT NULL::timestamptz({precision}) AS d'),
                        ('zero-rows',f'SELECT $1::timestamptz({precision}) AS d WHERE FALSE')]:
                    for input_ in (0,1):
                        for output in (0,1):
                            obs=keep(f'null-{precision}-{name}-{input_}-{output}',wire.cycle(sql,None,input_,output,
                                parameter=name in ('typed','zero-rows')))
                            assert not obs['errors'] and obs['descriptions']==[[[1184,8,precision,0]],[[1184,8,precision,output]]],(side,obs)
                            expected=[] if name=='zero-rows' else [[dict(length=-1,hex=None)]]
                            if name=='first-null':
                                zero=b'2000-01-01 00:00:00+00' if output==0 else b'\0'*8
                                expected.append([dict(length=len(zero),hex=zero.hex())])
                            elif name=='all-null': expected.append([dict(length=-1,hex=None)])
                            assert obs['rows']==expected,(side,obs)
            # One prepared statement, multiple portals and result format switches.
            wire.parse('SELECT $1::timestamptz AS d', b'switch')
            assert wire.sync()['tags'] == '1Z'
            for output in (0,1,0):
                wire.bind(b'2000-01-01 00:00:00',0,output,statement=b'switch')
                wire.send(b'D',b'P\0');wire.execute()
                obs = wire.sync()
                keep(f'switch-{len(collected)}',obs)
                assert obs['descriptions'] == [[[1184,8,-1,output]]] and len(obs['rows']) == 1, obs
            bad_text = ['2023-02-29 00:00:00','1900-02-29 00:00:00','2024-04-31 00:00:00',
                        '2024-13-01 00:00:00','2024-01-01 12:60:00',
                        '0000-00-00 00:00:00','0000-01-01 00:00:00','2024-00-01 00:00:00','2024-01-00 00:00:00']
            bad = [(s.encode(),0,'22008',None) for s in bad_text]
            bad += [(s.encode(),0,'22007',None) for s in ['','garbage','allballs']]
            bad += [(b'\xff',0,'22021',None),(b'2000-01-01 00:00:00\0junk',0,'22021',None),
                    (b'',1,'08P01',None),(b'\0'*7,1,'08P01',None),(b'\0'*9,1,'22P03',None)]
            bad += [(struct.pack('!q',n),1,'22008',None) for n in
                    [-211813488000000001,9223371331200000000,-9223372036854775807,9223372036854775806]]
            bad += [(b'\0'*8,1,'08P01',100)]
            bad += [(s.encode(),0,state,None) for s,state in [
                ('2000-01-01 00:00:00 Aster/Unknown','22023'),
                ('4714-11-24 00:00:00+00:00:01 BC','22008'),
                ('294276-12-31 23:59:59.999999-00:00:01','22008')]]
            for idx,(payload,input_,state,declared) in enumerate(bad):
                for output in (0,1):
                    obs = keep(f'bad-{idx}-{output}',wire.cycle('SELECT $1::timestamptz AS d',payload,input_,output,declared))
                    assert obs['errors'] == [state] and not obs['rows'] and obs['ready'] == 'I', (side,idx,obs)
                    recovery = keep(f'recovery-{idx}-{output}',wire.query('SELECT 1::int4 AS n'))
                    assert recovery['rows'] == [[{'length':1,'hex':'31'}]] and not recovery['errors'], recovery
            for idx, spelling in enumerate(bad_text+['','garbage']):
                obs = q(f'bad-literal-{idx}', f"SELECT CAST('{spelling}' AS timestamptz) AS d")
                assert obs['state'] == ('22008' if idx < len(bad_text) else '22007'), (side,obs)
            assert wire.query('BEGIN')['ready'] == 'T'
            obs = keep('transaction-error',wire.cycle('SELECT $1::timestamptz AS d',b'2023-02-29 00:00:00',0,1))
            assert obs['errors'] == ['22008'] and obs['ready'] == 'E',obs
            assert wire.query('ROLLBACK')['ready'] == 'I'
            keep('transaction-recovery',wire.query('SELECT 1::int4 AS n'))
            # Bind in New York, change display zone, and re-execute the same instant.
            assert wire.query('BEGIN')['ready']=='T'
            assert not wire.query(zone_sql('America/New_York'))['errors']
            wire.parse('SELECT $1::timestamptz AS timezone',b'zone')
            assert not wire.sync()['errors']
            for input_ in (0,1):
                payload=b'2018-11-04 01:30:00' if input_==0 else struct.pack('!q',594628200000000)
                for output in (0,1):
                    for zone,display in [('UTC',b'2018-11-04 06:30:00+00'),
                            ('Asia/Shanghai',b'2018-11-04 14:30:00+08'),
                            ('America/New_York',b'2018-11-04 01:30:00-05')]:
                        assert not wire.query(zone_sql('America/New_York'))['errors']
                        wire.bind(payload,input_,output,statement=b'zone',portal=b'zonep')
                        assert not wire.sync()['errors']
                        assert not wire.query(zone_sql(zone))['errors']
                        wire.send(b'D',b'Pzonep\0');wire.execute(b'zonep');wire.execute(b'zonep')
                        obs=keep(f'zone-{zone}-{input_}-{output}',wire.sync())
                        expected=display if output==0 else struct.pack('!q',594628200000000)
                        assert obs['descriptions']==[[[1184,8,-1,output]]] and obs['rows']==[[dict(length=len(expected),hex=expected.hex())]],obs
                        wire.send(b'C',b'Pzonep\0');wire.sync()
            assert wire.query('ROLLBACK')['ready']=='I'
            # Independently specified UTC instants cover offset equivalence and DST choices.
            from datetime import datetime, timezone
            epoch=datetime(2000,1,1,tzinfo=timezone.utc)
            for zone,source,instant in [
                    ('UTC','2000-01-01 08:00:00+08','2000-01-01T00:00:00+00:00'),
                    ('UTC','2000-01-01 00:00:00+08','1999-12-31T16:00:00+00:00'),
                    ('America/New_York','2018-03-11 02:30:00','2018-03-11T07:30:00+00:00'),
                    ('America/New_York','2018-11-04 01:30:00','2018-11-04T06:30:00+00:00'),
                    ('America/New_York','2018-11-04 01:30:00-04','2018-11-04T05:30:00+00:00'),
                    ('America/New_York','2018-11-04 01:30:00-05','2018-11-04T06:30:00+00:00')]:
                ok(observe(execute(conn,zone_sql(zone).encode())))
                delta=datetime.fromisoformat(instant)-epoch
                micros=(delta.days*86400+delta.seconds)*1000000+delta.microseconds
                binary=struct.pack('!q',micros)
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else binary
                        obs=keep(f'dst-{len(collected)}',observe(prepared(conn,statement,1,(text*1)(payload),
                            (integer*1)(len(payload)),(integer*1)(input_),output)))
                        assert obs['status']==2 and obs['metadata']==[[1184,8,-1,output]],obs
                        if output==1: assert obs['rows']==[[binary.hex()]],obs
            # Explicit casts retain source identity and exercise both codecs.
            for zone in ['UTC','Asia/Shanghai','America/New_York']:
                ok(observe(execute(conn,zone_sql(zone).encode())))
                for oid,target,spelling,micros in [
                        (1114,'timestamptz',b'2000-01-01 00:00:00',0),
                        (1184,'timestamp',b'2018-11-04 01:30:00-04',594624600000000),
                        (1184,'timestamp',b'2018-11-04 01:30:00-05',594628200000000),
                        (1114,'timestamptz',b'infinity',9223372036854775807),
                        (1184,'timestamp',None,0)]:
                    for input_ in (0,1):
                        for output in (0,1):
                            payload=None if spelling is None else spelling if input_==0 else struct.pack('!q',micros)
                            obs=keep(f'cast-{len(collected)}',observe(params(conn,f'SELECT CAST($1 AS {target})'.encode(),1,
                                (c.c_uint*1)(oid),(text*1)(payload),(integer*1)(0 if payload is None else len(payload)),
                                (integer*1)(input_),output)))
                            assert obs['status']==2 and obs['metadata']==[[1184 if target=='timestamptz' else 1114,8,-1,output]],obs
            # Relative literal is frozen at Parse; text parameter freezes at Bind.
            # Binary outputs isolate instant identity from the later display zone.
            if side=='reference':
                assert wire.query('BEGIN')['ready']=='T'
                instant=wire.query('SELECT EXTRACT(EPOCH FROM transaction_timestamp())')
                wire_epoch=bytes.fromhex(instant['rows'][0][0]['hex']).decode()
                identity['wire_transaction_epoch']=wire_epoch
            else:
                assert not wire.query('SET timestamp = '+wire_epoch)['errors']
                assert wire.query('BEGIN')['ready']=='T'
            assert not wire.query(zone_sql('Etc/GMT+12'))['errors']
            wire.parse("SELECT CAST('today' AS timestamptz)",b'constant',parameter=False)
            assert not wire.sync()['errors']
            wire.bind(None,0,1,statement=b'constant',portal=b'constantp',parameter=False)
            wire.execute(b'constantp')
            before=keep('relative-parse-before',wire.sync())
            assert not wire.query(zone_sql('Etc/GMT-14'))['errors']
            wire.bind(None,0,1,statement=b'constant',portal=b'constantp2',parameter=False)
            wire.execute(b'constantp2')
            parsed=keep('relative-parse-after',wire.sync())
            assert before['rows']==parsed['rows'],(side,before,parsed)
            wire.parse('SELECT $1::timestamptz',b'bound')
            wire.sync()
            wire.bind(b'today',0,1,statement=b'bound',portal=b'boundp')
            wire.bind(b'today',0,1,statement=b'bound',portal=b'boundp2')
            wire.execute(b'boundp')
            bound_before=keep('relative-bind-before',wire.sync())
            assert bound_before['rows']!=before['rows'],(side,before,bound_before)
            assert not wire.query(zone_sql('Etc/GMT+12'))['errors']
            wire.execute(b'boundp2')
            bound_after=keep('relative-bind-after',wire.sync())
            assert bound_after['rows']==bound_before['rows'],(side,bound_before,bound_after)
            if side == 'aster':
                print('PG_TZ_MID',flush=True)
                assert sys.stdin.readline().strip() == 'continue'
            observations[side]=collected
        finally:
            if wire is not None:
                (evidence/(side+'-diagnostics.json')).write_text(json.dumps(wire.diagnostics,sort_keys=True))
                try:wire.query('ROLLBACK')
                finally:wire.close()
            rolled=observe(execute(conn,b'ROLLBACK'))
            notices.clear()
            finish(conn)
            cleanup[side]=rolled['status'] == 1
            (evidence/(side+'.json')).write_text(json.dumps(collected,sort_keys=True))
            (evidence/'identity.json').write_text(json.dumps(identity,sort_keys=True))
            (evidence/'cleanup.json').write_text(json.dumps(cleanup))
    assert observations['aster'].keys() == observations['reference'].keys()
    for case, actual in observations['aster'].items():
        assert actual == observations['reference'][case], (case,actual,observations['reference'][case])
    for key, replacement in [('rows',[['00']]),('metadata',[[25,-1,-1,0]]),('state','XXXXX')]:
        changed=copy.deepcopy(observations['reference'])
        changed['libpq-0-0-0'][key]=replacement
        assert changed != observations['aster'], key
    summary=dict(cases=len(observations['aster']),negative_controls=3,cleanup=cleanup,passed=True,
                 capabilities=['TZ-SAME-INSTANT','TZ-SESSION','TZ-DST','TZ-PRECISION-RANGE',
                               'TZ-CAST-BRIDGE','TZ-NULL-METADATA','TZ-INVALID'])
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('timestamptz differential:',json.dumps(summary),flush=True)


def interval_differential(aster_dsn):
    """IV-DIFF: independent libpq observations plus explicit-length wire probes."""
    import copy
    import json
    import socket
    import struct
    import subprocess
    import hashlib
    from pathlib import Path

    repository = Path(__file__).resolve().parents[3]
    reference = os.environ.get('PG18_REFERENCE_DSN')
    assert reference, 'PG18_REFERENCE_DSN is required (no skip)'
    evidence = Path(os.environ['PG_IV_EVIDENCE_DIR'])
    evidence.mkdir(parents=True, exist_ok=True)
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    fmt = api('PQfformat', integer, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    length = api('PQgetlength', integer, ptr, integer, integer)
    raw_value = api('PQgetvalue', ptr, ptr, integer, integer)
    pq_socket = api('PQsocket', integer, ptr)
    notice_api = api('PQsetNoticeReceiver', ptr, ptr, ptr, ptr)
    notices = []
    @c.CFUNCTYPE(None, ptr, ptr)
    def on_notice(arg, result):
        notices.append((ef(result, ord('C')) or b'').decode())

    def observe(result):
        assert result, 'libpq returned no result'
        try:
            return dict(status=result_status(result), state=(ef(result, ord('C')) or b'').decode(),
                        metadata=[[field_type(result, i), size(result, i), mod(result, i), fmt(result, i)]
                                  for i in range(columns(result))],
                        parameters=[pt(result, i) for i in range(np(result))],
                        rows=[[None if is_null(result, r, i) else
                               c.string_at(raw_value(result, r, i), length(result, r, i)).hex()
                               for i in range(columns(result))] for r in range(rows(result))],
                        warnings=list(notices))
        finally:
            notices.clear()
            clear(result)

    def scalar(obs):
        assert obs['status'] == 2 and len(obs['rows']) == 1, obs
        return bytes.fromhex(obs['rows'][0][0]).decode()

    class Wire:
        # Startup and authentication are real libpq; duplicate only its idle socket.
        # No libpq operations run while these raw message probes own the connection.
        def __init__(self, dsn):
            self.conn = connect(dsn.encode())
            assert self.conn and status(self.conn) == 0, 'wire connection failed'
            self.raw = socket.socket(fileno=os.dup(pq_socket(self.conn)))
            self.blocking = os.get_blocking(pq_socket(self.conn))
            self.raw.settimeout(10)
            self.diagnostics = []
        def close(self):
            self.raw.close()
            os.set_blocking(pq_socket(self.conn), self.blocking)
            finish(self.conn)
        def send(self, tag, body):
            self.raw.sendall(tag + struct.pack('!I', len(body) + 4) + body)
        def exact(self, n):
            data = b''
            while len(data) < n:
                part = self.raw.recv(n-len(data))
                assert part, 'unexpected EOF'
                data += part
            return data
        def ready(self):
            obs = dict(tags='', descriptions=[], parameters=[], rows=[], errors=[], warnings=[], ready='')
            while True:
                tag = self.exact(1).decode()
                n = struct.unpack('!I', self.exact(4))[0]
                assert 4 <= n < 1 << 20
                body = self.exact(n-4)
                # ParameterStatus and NoticeResponse are async, not phase delimiters.
                if tag not in ('S', 'N'):
                    obs['tags'] += tag
                if tag == 'T':
                    count = struct.unpack('!H', body[:2])[0]
                    at, fields = 2, []
                    for _ in range(count):
                        at = body.index(b'\0', at) + 1
                        _, _, oid, width, typmod, output = struct.unpack('!IhIhih', body[at:at+18])
                        at += 18
                        fields.append([oid, width, typmod, output])
                    obs['descriptions'].append(fields)
                elif tag == 't':
                    count = struct.unpack('!H', body[:2])[0]
                    obs['parameters'] = list(struct.unpack('!'+'I'*count, body[2:]))
                elif tag == 'D':
                    count = struct.unpack('!H', body[:2])[0]
                    at, cells = 2, []
                    for _ in range(count):
                        size_ = struct.unpack('!i', body[at:at+4])[0]
                        at += 4
                        assert size_ >= -1
                        cells.append(dict(length=size_, hex=None if size_ == -1 else body[at:at+size_].hex()))
                        if size_ >= 0:
                            at += size_
                    assert at == len(body)
                    obs['rows'].append(cells)
                elif tag in ('E', 'N'):
                    fields = {chr(f[0]): f[1:].decode(errors='replace') for f in body.split(b'\0') if f}
                    self.diagnostics.append(fields)
                    obs['errors' if tag == 'E' else 'warnings'].append(fields['C'])
                elif tag == 'Z':
                    obs['ready'] = body.decode()
                    return obs
        def query(self, sql):
            self.send(b'Q', sql.encode()+b'\0')
            return self.ready()
        def parse(self, sql, name=b'', parameter=True):
            self.send(b'P', name+b'\0'+sql.encode()+b'\0'+
                      (struct.pack('!HI',1,1186) if parameter else struct.pack('!H',0)))
        def bind(self, payload, input_, output, statement=b'', portal=b'', declared=None, parameter=True):
            val = struct.pack('!i', -1 if payload is None else len(payload) if declared is None else declared)
            values = struct.pack('!hhh',1,input_,1)+val+(payload or b'') if parameter else struct.pack('!hh',0,0)
            self.send(b'B',portal+b'\0'+statement+b'\0'+values+struct.pack('!hh',1,output))
        def execute(self, portal=b''):
            self.send(b'E', portal+b'\0'+struct.pack('!I', 0))
        def sync(self):
            self.send(b'S', b'')
            return self.ready()
        def cycle(self, sql, payload, input_, output, declared=None, parameter=True):
            self.parse(sql, parameter=parameter)
            self.send(b'D', b'S\0')
            self.bind(payload, input_, output, declared=declared, parameter=parameter)
            self.send(b'D', b'P\0')
            self.execute()
            self.execute()
            return self.sync()

    canonical_values={}
    observations, cleanup, identity = {}, {}, {}
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn) == 0, side+' connection failed'
        notice_api(conn, c.cast(on_notice, ptr), None)
        collected, wire = {}, None
        def keep(case, result):
            assert case not in collected, case
            collected[case] = result
            return result
        def q(case, sql):
            return keep(case, observe(execute(conn, sql.encode())))
        def ok(obs):
            assert obs['status'] in (1, 2) and not obs['warnings'], (side, obs)
            return obs
        def zone_sql(zone):
            # The preceding temporal tasks expose the native time_zone setting.
            # Use that real session interface until full PG GUC spelling lands.
            return f"SET TimeZone TO '{zone}'" if side == 'reference' else f"SET time_zone = '{zone}'"
        try:
            if side == 'reference':
                identity['server'] = observe(execute(conn, b"SELECT version(), current_setting('server_version_num')"))
                assert 180000 <= int(bytes.fromhex(identity['server']['rows'][0][1]).decode()) < 190000
                identity['libpq_version'] = version()
                identity['source_head'] = subprocess.check_output(['git','rev-parse','HEAD'], text=True,cwd=repository).strip()
                identity['source_diff_sha256'] = hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg/server','pkg/types','pkg/session','pkg/parser'],cwd=repository)).hexdigest()
                identity['rust_test_binary'] = os.environ['PG_IV_RUST_BINARY']
                identity['rust_test_binary_sha256'] = hashlib.sha256(Path(identity['rust_test_binary']).read_bytes()).hexdigest()
                identity['cargo_lock_sha256'] = hashlib.sha256((repository/'Cargo.lock').read_bytes()).hexdigest()
                identity['fixture_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
                # Installed server builds do not always expose a tzdb release label.
                # Optional fingerprints identify its actual compiled zone data.
                tzdir = os.environ.get('PG_IV_REFERENCE_TZDIR')
                identity['reference_tzdata'] = ({zone: hashlib.sha256((Path(tzdir)/zone).read_bytes()).hexdigest()
                    for zone in ['UTC','Asia/Shanghai','America/New_York','Etc/GMT+12','Etc/GMT-14']}
                    if tzdir else 'compiled tzdata directory not provided')
                identity['source_status'] = subprocess.check_output(['git','status','--short'], text=True,cwd=repository)
                tz = Path('/usr/share/zoneinfo/tzdata.zi')
                version_file = Path('/usr/share/zoneinfo/+VERSION')
                identity['tzdata'] = tz.read_text().splitlines()[0] if tz.exists() else version_file.read_text().strip() if version_file.exists() else 'system tzdata version unavailable'
                tz_binary = Path('/usr/share/zoneinfo/UTC')
                identity['tzdata_utc_sha256'] = hashlib.sha256(tz_binary.read_bytes()).hexdigest()
            for setting in ["SET DateStyle TO 'ISO, MDY'", zone_sql("UTC")]:
                setup = observe(execute(conn, setting.encode()))
                assert setup['status'] in (1,2), (side, setting, setup)
            identity[side] = {setting: scalar(ok(observe(execute(conn, sql.encode()))))
                              for setting, sql in [('DateStyle','SHOW DateStyle'),
                                  ('TimeZone','SHOW TimeZone' if side == 'reference' else 'SELECT @@time_zone')]}
            identity[side]['IntervalStyle'] = observe(execute(conn,b'SHOW IntervalStyle'))
            ok(observe(execute(conn,b"SET IntervalStyle TO 'postgres'")))
            statement=b'interval_value'
            ok(keep('prepare',observe(prepare(conn,statement,b'SELECT $1::interval AS d',1,(c.c_uint*1)(1186)))))
            desc=keep('describe',observe(describe(conn,statement)))
            assert desc['parameters']==[1186] and desc['metadata']==[[1186,16,-1,0]],(side,desc)
            wire=Wire(dsn)
            assert not wire.query("SET IntervalStyle TO 'postgres'")['errors']
            # Tuple order below is months, days, microseconds; wire is qii.
            def binary(parts):
                m,d,t=parts
                return struct.pack('!qii',t,d,m)
            fixtures=[('1 month',(1,0,0)),('30 days',(0,30,0)),('1 day',(0,1,0)),
                ('24 hours',(0,0,86400000000)),('1.5 months',(1,15,0)),
                ('-1 month +2 days -03:04:05.123456',(-1,2,-11045123456)),
                ('-1 month -2 days -03:04:05.123456',(-1,-2,-11045123456)),
                ('0',(0,0,0)),('PT1H2M3.123456S',(0,0,3723123456)),
                ('infinity',(2147483647,2147483647,9223372036854775807)),
                ('-infinity',(-2147483648,-2147483648,-9223372036854775808)),
                ('-51 hours',(0,0,-183600000000)),('839 hours',(0,0,3020400000000)),('-51:00:00.000001',(0,0,-183600000001))]
            # Reference canonical text is independently read, while binary expectations
            # are specified here, so paired mistakes cannot pass by display equality.
            for idx,(spelling,parts) in enumerate(fixtures+[(None,None)]):
                expected_binary=None if parts is None else binary(parts)
                if side=='reference':
                    canonical=observe(prepared(conn,statement,1,(text*1)(None if spelling is None else spelling.encode()),None,None,0))
                    canonical_values[idx]=None if parts is None else bytes.fromhex(canonical['rows'][0][0])
                for input_ in (0,1):
                    for output in (0,1):
                        payload=None if parts is None else spelling.encode() if input_==0 else expected_binary
                        obs=keep(f'value-{idx}-{input_}-{output}',observe(prepared(conn,statement,1,(text*1)(payload),
                            (integer*1)(0 if payload is None else len(payload)),(integer*1)(input_),output)))
                        expected=canonical_values[idx] if output==0 else expected_binary
                        assert obs['status']==2 and not obs['warnings'] and obs['metadata']==[[1186,16,-1,output]],(side,idx,obs)
                        assert obs['rows']==[[None if expected is None else expected.hex()]],(side,idx,obs,expected)
                        w=keep(f'wire-{idx}-{input_}-{output}',wire.cycle('SELECT $1::interval AS d',payload,input_,output))
                        assert w['tags']=='1tT2TDCCZ' and w['parameters']==[1186],(side,w)
                        assert w['descriptions']==[[[1186,16,-1,0]],[[1186,16,-1,output]]],(side,w)
                        assert w['rows']==[[dict(length=-1 if expected is None else len(expected),hex=None if expected is None else expected.hex())]],(side,w)
            # Independent component extrema include mixed finite values beside sentinels.
            for idx,parts in enumerate([(2147483647,0,0),(-2147483648,0,0),(0,2147483647,0),
                    (0,-2147483648,0),(0,0,9223372036854775807),(0,0,-9223372036854775808),
                    (2147483647,-2147483648,9223372036854775806),(-2147483648,2147483647,-9223372036854775807)]):
                payload=binary(parts)
                for output in (0,1):
                    obs=keep(f'extreme-{idx}-{output}',observe(prepared(conn,statement,1,(text*1)(payload),(integer*1)(16),(integer*1)(1),output)))
                    assert obs['status']==2 and obs['metadata']==[[1186,16,-1,output]],(side,obs)
                    if output: assert obs['rows']==[[payload.hex()]],(side,obs)
                    else:
                        # Feed displayed finite text back, proving dimensions survive text.
                        back=observe(prepared(conn,statement,1,(text*1)(bytes.fromhex(obs['rows'][0][0])),None,None,1))
                        if parts[2]==-9223372036854775808 and side=='reference':
                            assert back['state']=='22007',back
                            identity['reference_min_time_text_readback']=back
                        else: assert back['rows']==[[payload.hex()]],(side,idx,back)
            for idx,parts in enumerate([(2147483647,0,0),(-2147483648,0,0),(0,2147483647,0),
                    (0,-2147483648,0),(0,0,9223372036854775807),(0,0,-9223372036854775808),
                    (2147483647,-2147483648,9223372036854775806),(-2147483648,2147483647,-9223372036854775807)]):
                source=f'{parts[0]} months {parts[1]} days {parts[2]} microseconds'.encode()
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source if input_==0 else binary(parts)
                        obs=keep(f'extreme-four-{idx}-{input_}-{output}',observe(prepared(conn,statement,1,(text*1)(payload),
                            (integer*1)(len(payload)),(integer*1)(input_),output)))
                        assert obs['status']==2 and obs['metadata']==[[1186,16,-1,output]],(side,idx,obs)
                        if output: assert obs['rows']==[[binary(parts).hex()]],(side,obs)
            for style in ['postgres','postgres_verbose','sql_standard','iso_8601']:
                ok(observe(execute(conn,f"SET IntervalStyle TO '{style}'".encode())))
                for idx,(source,parts) in enumerate(fixtures):
                    payload=binary(parts)
                    canonical=observe(prepared(conn,statement,1,(text*1)(payload),(integer*1)(16),(integer*1)(1),0))
                    # Read each server's own text output back to its original components.
                    displayed=bytes.fromhex(canonical['rows'][0][0])
                    for input_ in (0,1):
                        for output in (0,1):
                            value=displayed if input_==0 else payload
                            obs=keep(f'style-{style}-{idx}-{input_}-{output}',observe(prepared(conn,statement,1,(text*1)(value),
                                (integer*1)(len(value)),(integer*1)(input_),output)))
                            assert obs['status']==2 and obs['rows']==[[(displayed if output==0 else payload).hex()]],(side,style,obs)
                for input_ in (0,1):
                    for output in (0,1):
                        obs=keep(f'style-null-{style}-{input_}-{output}',observe(prepared(conn,statement,1,
                            (text*1)(None),None,(integer*1)(input_),output)))
                        assert obs['status']==2 and obs['metadata']==[[1186,16,-1,output]] and obs['rows']==[[None]],(side,obs)
                short=keep('short-sign-'+style,observe(prepared(conn,statement,1,(text*1)(b'-1 2:03:04'),None,None,1)))
                assert short['rows']==[[binary((0,-1,-7384000000 if style=='sql_standard' else 7384000000)).hex()]],(side,short)
            ok(observe(execute(conn,b"SET IntervalStyle TO 'postgres'")))
            for idx,(type_,source,parts,result,typmod) in enumerate([
                ('interval(3)','1.2345 seconds',(0,0,1234500),(0,0,1235000),2147418115),
                ('interval(3)','-1.2345 seconds',(0,0,-1234500),(0,0,-1235000),2147418115),
                ('interval(0)','0.5 seconds',(0,0,500000),(0,0,1000000),2147418112),
                ('interval(0)','-0.5 seconds',(0,0,-500000),(0,0,-1000000),2147418112),
                ('interval(6)','1.1234567 seconds',(0,0,1123457),(0,0,1123457),2147418118),
                ('interval(6)','59.9999997 seconds',(0,0,60000000),(0,0,60000000),2147418118),
                ('interval hour to minute','1 day 2:03:04',(0,1,7384000000),(0,1,7380000000),201392127),
                ('interval year','1 year 2 months',(14,0,0),(12,0,0),327679)]):
                stmt=f'precision{idx}'.encode()
                ok(observe(prepare(conn,stmt,f'SELECT $1::{type_} AS d'.encode(),1,(c.c_uint*1)(1186))))
                desc=keep(f'precision-desc-{idx}',observe(describe(conn,stmt)))
                assert desc['parameters']==[1186] and desc['metadata']==[[1186,16,typmod,0]],(side,desc)
                for input_ in (0,1):
                    for output in (0,1):
                        payload=source.encode() if input_==0 else binary(parts)
                        obs=keep(f'precision-{idx}-{input_}-{output}',observe(prepared(conn,stmt,1,(text*1)(payload),
                            (integer*1)(len(payload)),(integer*1)(input_),output)))
                        assert obs['status']==2 and obs['metadata']==[[1186,16,typmod,output]],(side,obs)
                        if output: assert obs['rows']==[[binary(result).hex()]],(side,obs)
            warning=q('precision-warning',"SELECT CAST('1.123456 seconds' AS interval(7))")
            assert warning['warnings']==['22023'] and warning['metadata']==[[1186,16,2147418118,0]],(side,warning)
            negative=observe(execute(conn,b'SELECT NULL::interval(-1)'))
            assert negative['state']==('42601' if side=='reference' else '22023'),(side,negative)
            identity[side]['negative_precision_contract_difference']=negative
            for typmod,type_ in [(-1,'interval'),(2147418112,'interval(0)'),(2147418115,'interval(3)'),(2147418118,'interval(6)')]:
                for name,sql in [('typed',f'SELECT $1::{type_} AS d'),('null',f'SELECT NULL::{type_} AS d'),
                    ('first-null',f"SELECT NULL::{type_} AS d UNION ALL SELECT CAST('0' AS {type_}) AS d"),
                    ('all-null',f'SELECT NULL::{type_} AS d UNION ALL SELECT NULL::{type_} AS d'),
                    ('zero-rows',f'SELECT $1::{type_} AS d WHERE FALSE')]:
                    for input_ in (0,1):
                        for output in (0,1):
                            obs=keep(f'null-{typmod}-{name}-{input_}-{output}',wire.cycle(sql,None,input_,output,parameter=name in ('typed','zero-rows')))
                            assert not obs['errors'] and obs['descriptions']==[[[1186,16,typmod,0]],[[1186,16,typmod,output]]],(side,obs)
                            expected=[] if name=='zero-rows' else [[dict(length=-1,hex=None)]]
                            if name=='first-null':
                                zero=b'00:00:00' if output==0 else b'\0'*16
                                expected.append([dict(length=len(zero),hex=zero.hex())])
                            elif name=='all-null':expected.append([dict(length=-1,hex=None)])
                            assert obs['rows']==expected,(side,obs)
            wire.parse('SELECT $1::interval AS d',b'switch')
            assert wire.sync()['tags']=='1Z'
            for output in (0,1,0):
                wire.bind(b'0',0,output,statement=b'switch');wire.send(b'D',b'P\0');wire.execute()
                obs=keep(f'switch-{len(collected)}',wire.sync())
                assert obs['descriptions']==[[[1186,16,-1,output]]] and len(obs['rows'])==1,(side,obs)
            # IntervalStyle changes the meaning of unsigned fields after a minus.
            # Parse freezes literals; Bind freezes text parameters, independent of
            # the later style used by Execute. Named portals live in one transaction.
            assert wire.query('BEGIN')['ready']=='T'
            assert not wire.query("SET IntervalStyle TO 'postgres'")['errors']
            wire.parse("SELECT CAST('-1 2:03:04' AS interval)",b'frozen',parameter=False)
            assert not wire.sync()['errors']
            wire.bind(None,0,1,statement=b'frozen',portal=b'literal',parameter=False)
            wire.parse('SELECT $1::interval',b'bound')
            assert not wire.sync()['errors']
            wire.bind(b'-1 2:03:04',0,1,statement=b'bound',portal=b'boundp')
            assert not wire.sync()['errors']
            assert not wire.query("SET IntervalStyle TO 'sql_standard'")['errors']
            for portal in (b'literal',b'boundp'):
                wire.execute(portal)
                obs=keep('frozen-'+portal.decode(),wire.sync())
                assert obs['rows']==[[dict(length=16,hex=binary((0,-1,7384000000)).hex())]],(side,obs)
            wire.bind(b'-1 2:03:04',0,1,statement=b'bound',portal=b'changed')
            wire.execute(b'changed')
            obs=keep('frozen-new-bind',wire.sync())
            assert obs['rows']==[[dict(length=16,hex=binary((0,-1,-7384000000)).hex())]],(side,obs)
            assert wire.query('ROLLBACK')['ready']=='I'
            for bound in [9223372036854775807,-9223372036854775808]:
                obs=keep(f'typmod-overflow-{bound}',wire.cycle('SELECT $1::interval(0)',binary((0,0,bound)),1,1))
                assert obs['errors']==['22008'] and obs['ready']=='I',obs
                recovery=wire.query('SELECT 1::int4')
                assert recovery['rows']==[[dict(length=1,hex='31')]] and not recovery['errors'],recovery
            bad=[(s.encode(),0,state,None) for s,state in [('', '22007'),('garbage','22007'),('now','22007'),('today','22007'),('tomorrow','22007'),('yesterday','22007'),
                ('2147483648 months','22015'),('2147483647 months 1 month','22015'),('9223372036854.775808 seconds','22015')]]
            bad += [(b'\xff',0,'22021',None),(b'1 day\0junk',0,'22021',None),
                (b'',1,'08P01',None),(b'\0'*15,1,'08P01',None),(b'\0'*17,1,'22P03',None),
                (b'\0'*16,1,'08P01',100)]
            for idx,(payload,input_,state,declared) in enumerate(bad):
                for output in (0,1):
                    obs=keep(f'bad-{idx}-{output}',wire.cycle('SELECT $1::interval AS d',payload,input_,output,declared))
                    assert obs['errors']==[state] and not obs['rows'] and obs['ready']=='I',(side,idx,obs)
                    recovery=keep(f'recovery-{idx}-{output}',wire.query('SELECT 1::int4 AS n'))
                    assert recovery['rows']==[[dict(length=1,hex='31')]] and not recovery['errors'],(side,recovery)
            for idx,(source,state) in enumerate([('garbage','22007'),('2147483648 months','22015'),
                    ('2147483647 months 1 year','22008')]):
                for parameter in (False,):
                    obs=keep(f'literal-error-{idx}-{parameter}',wire.cycle(f"SELECT CAST('{source}' AS interval)",None,0,0,parameter=parameter))
                    assert obs['tags']=='EZ' and obs['errors']==[state],(side,obs)
                    recovery=wire.query('SELECT 1::int4')
                    assert recovery['rows']==[[dict(length=1,hex='31')]] and not recovery['errors'],recovery
            assert wire.query('BEGIN')['ready']=='T'
            obs=keep('transaction-error',wire.cycle('SELECT $1::interval AS d',b'garbage',0,1))
            assert obs['errors']==['22007'] and obs['ready']=='E',obs
            assert wire.query('ROLLBACK')['ready']=='I'
            recovery=keep('transaction-recovery',wire.query('SELECT 1::int4'))
            assert recovery['rows']==[[dict(length=1,hex='31')]] and not recovery['errors'],recovery
            if side == 'aster':
                print('PG_IV_MID',flush=True)
                assert sys.stdin.readline().strip() == 'continue'
            observations[side]=collected
        finally:
            if wire is not None:
                (evidence/(side+'-diagnostics.json')).write_text(json.dumps(wire.diagnostics,sort_keys=True))
                try:wire.query('ROLLBACK')
                finally:wire.close()
            rolled=observe(execute(conn,b'ROLLBACK'))
            notices.clear()
            finish(conn)
            cleanup[side]=rolled['status'] == 1
            (evidence/(side+'.json')).write_text(json.dumps(collected,sort_keys=True))
            (evidence/'identity.json').write_text(json.dumps(identity,sort_keys=True))
            (evidence/'cleanup.json').write_text(json.dumps(cleanup))
    assert observations['aster'].keys() == observations['reference'].keys()
    for case, actual in observations['aster'].items():
        assert actual == observations['reference'][case], (case,actual,observations['reference'][case])
    for key, replacement in [('rows',[['00']]),('metadata',[[25,-1,-1,0]]),('state','XXXXX')]:
        changed=copy.deepcopy(observations['reference'])
        changed['value-0-0-0'][key]=replacement
        assert changed != observations['aster'], key
    summary=dict(cases=len(observations['aster']),negative_controls=3,cleanup=cleanup,passed=True,
                 capabilities=['IV-COMPONENTS','IV-SIGNS-STYLE','IV-PRECISION-FIELDS','IV-RANGE-SPECIAL',
                               'IV-BRIDGE','IV-NULL-METADATA','IV-INVALID'])
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('interval differential:',json.dumps(summary),flush=True)


def integer_differential(aster_dsn):
    import copy
    import signal
    import json
    from pg_core_evidence import CaseRegistry, load_cases, append_observation, compare_scalar_output, compare_rows, compare_metadata, compare_rejection, validate_provenance, source_snapshot, summarize_run, validate_whitelist, apply_whitelist
    from collections import Counter

    reference_dsn = os.environ.get("PG18_REFERENCE_DSN")
    assert reference_dsn, "PG18_REFERENCE_DSN is required (no skip)"
    prepare = api("PQprepare", ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api("PQdescribePrepared", ptr, ptr, text)
    prepared = api("PQexecPrepared", ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    field_size = api("PQfsize", integer, ptr, integer)
    field_mod = api("PQfmod", integer, ptr, integer)
    field_name = api("PQfname", text, ptr, integer)
    nparams = api("PQnparams", integer, ptr)
    param_type = api("PQparamtype", c.c_uint, ptr, integer)
    error_field = api("PQresultErrorField", text, ptr, integer)
    transaction = api("PQtransactionStatus", integer, ptr)
    connections, observations = {}, {"aster": {}, "reference": {}}
    table = f"public.pg_core_diff_{os.getpid()}"
    created = set()
    rejection_records = {"aster": {}, "reference": {}}
    rejection_contracts = {}
    pending_rejections = {"aster": [], "reference": []}
    counts = Counter()
    registry = CaseRegistry(load_cases(), table)
    statements = {}
    descriptions = {"aster": {}, "reference": {}}
    from pathlib import Path
    evidence_dir = Path(os.environ['PG_CORE_EVIDENCE_DIR'])
    run_id = evidence_dir.name
    attempt = 1
    attempts = []
    whitelist = validate_whitelist(json.loads(Path(__file__).with_name('pg_core_whitelist.json').read_text()))
    whitelist_used = []
    completed = False

    def save_attempts():
        (evidence_dir / 'attempts.json').write_text(json.dumps(attempts, sort_keys=True))
    get_length = api('PQgetlength', integer, ptr, integer, integer)
    raw_value = api('PQgetvalue', ptr, ptr, integer, integer)
    field_format = api('PQfformat', integer, ptr, integer)
    command_status = api('PQcmdStatus', text, ptr)

    def archive(case, side, actual_input, observation, outcome='collected', error_type=None):
        append_observation(evidence_dir, dict(run=run_id, case_id=case, side=side,
                           attempt=attempt, input=actual_input, observation=observation,
                           outcome=outcome, error_type=error_type))

    def interrupted(signum, frame):
        for side in connections:
            archive('interrupted', side, None, None, 'interrupted', signal.Signals(signum).name)
        raise KeyboardInterrupt("client interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)

    def observe(result, partial=None):
        assert result, "libpq returned no result"
        obs = partial if partial is not None else {}
        try:
            obs['status'] = result_status(result)
            obs['state'] = (error_field(result, ord('C')) or b'').decode()
            obs['fields'] = [(field_name(result, col).decode(), field_type(result, col),
                              field_size(result, col), field_mod(result, col))
                             for col in range(columns(result))]
            obs['param_count'] = nparams(result)
            obs['params'] = [param_type(result, i) for i in range(obs['param_count'])]
            obs['formats'] = [field_format(result, col) for col in range(columns(result))]
            obs['command_tag'] = (command_status(result) or b'').decode()
            obs['affected_rows'] = (command_rows(result) or b'').decode()
            obs['table_origins'] = None
            obs['rows'], obs['cells'] = [], []
            for row in range(rows(result)):
                values, cells = [], []
                obs['rows'].append(values)
                obs['cells'].append(cells)
                for col in range(columns(result)):
                    if is_null(result, row, col):
                        values.append(None)
                        cells.append({'kind': 'sql_null', 'length': get_length(result, row, col)})
                    else:
                        length = get_length(result, row, col)
                        payload = c.string_at(raw_value(result, row, col), length)
                        cells.append({'kind': 'value', 'length': length, 'hex': payload.hex()})
                        values.append(payload.decode())
            return obs
        finally:
            clear(result)

    def equal(case, actual, reference, row_order='strict', phase='query', context=None):
        if context is not None:
            actual, reference, used = apply_whitelist(whitelist, context, actual, reference, phase)
            whitelist_used.extend(used)
        assert compare_rows(actual, reference, row_order), (case, actual, reference)
        assert compare_metadata(actual, reference, phase), (case, actual, reference)
        keys = ('status', 'state')
        assert {k: actual[k] for k in keys} == {k: reference[k] for k in keys}, (case, actual, reference)

    def check(case, family, operation, expected_status=2, expected_rows=None,
              expected_state="", expected_params=None):
        actual_input, send = operation
        registered_input = registry.claim(case, family, actual_input)
        entry = dict(run=run_id, case_id=case, attempt=attempt, outcome='pending', assertions=0)
        attempts.append(entry)
        save_attempts()
        for side, connection in connections.items():
            partial = {}
            try:
                obs = observe(send(connection), partial)
            except BaseException as exc:
                append_observation(evidence_dir, dict(run=run_id, case_id=case, side=side,
                                   attempt=attempt, input=registered_input, observation=partial or None,
                                   outcome='collection_failed', error_type=type(exc).__name__))
                raise
            append_observation(evidence_dir, dict(run=run_id, case_id=case, side=side,
                               attempt=attempt, input=registered_input, observation=obs,
                               outcome='collected'))
            observations[side][case] = obs
            contract = registry.cases['cases'][len(registry.seen) - 1].get('rejection_contract')
            if contract is not None:
                # Snapshot and recovery evidence are attached below, after actual probes.
                rejection_contracts[case] = copy.deepcopy(contract)
                rejection_records[side][case] = {
                    'classification': 'expected_rejection',
                    'phase': {'query': 'PQexec', 'prepare': 'PQprepare',
                              'describe': 'PQdescribePrepared', 'execute': 'PQexecPrepared'}[registered_input['phase']],
                    'backend_phase': None,
                    'connection_ok': status(connection) == 0, 'error': copy.deepcopy(obs),
                    'transaction_after_error': {0: 'I', 2: 'T', 3: 'E'}.get(transaction(connection), 'unknown')}
                archive(case + '_rejection_error', side, registered_input, rejection_records[side][case])
                pending_rejections[side].append(case)
            elif family == 'recovery':
                for rejected in pending_rejections[side]:
                    rejection_records[side][rejected].update(
                        recovery=copy.deepcopy(obs),
                        transaction_after_recovery={0: 'I', 2: 'T', 3: 'E'}.get(transaction(connection), 'unknown'))
                pending_rejections[side].clear()
            assert obs["status"] == expected_status, (side, case, obs)
            entry["assertions"] += 1
            assert obs["state"] == expected_state, (side, case, obs)
            entry["assertions"] += 1
            if expected_rows is not None:
                assert obs["rows"] == expected_rows, (side, case, obs)
                entry["assertions"] += 1
            if expected_params is not None:
                assert obs["params"] == expected_params, (side, case, obs)
                entry["assertions"] += 1
            phase, statement = registered_input['phase'], registered_input['statement']
            if phase == 'describe':
                descriptions[side][statement] = copy.deepcopy(obs)
            elif phase == 'execute':
                description = descriptions[side][statement]
                assert len(registered_input['arguments']) == description['param_count'], (side, case)
                entry['assertions'] += 1
                if expected_status == 2:
                    for key in ['fields', 'formats']:
                        assert obs[key] == description[key], (side, case, key)
                        entry["assertions"] += 1
        row_order = registry.cases['cases'][len(registry.seen) - 1].get('row_order', 'strict')
        registered = next((item for item in registry.cases['cases'] if item['case_id'] == case), None)
        context = None
        if registered is not None and registered['kind'] == 'business':
            context = dict(capability=registered['family'], case_id=case,
                           sql=registered['input']['sql'],
                           type_oids=[field[1] for field in observations['reference'][case]['fields']],
                           client=json.dumps(provenance['client'], sort_keys=True),
                           environment=json.dumps({key: provenance[key] for key in
                               ['os', 'storage', 'authentication', 'endpoints']}, sort_keys=True),
                           version=provenance['target_version'],
                           implementation=json.dumps(provenance['source'], sort_keys=True),
                           configuration=json.dumps(provenance['reference_build'], sort_keys=True))
        equal(case, observations["aster"][case], observations["reference"][case], row_order, registered_input["phase"], context)
        entry["assertions"] += 3  # equal(): rows, metadata, status/SQLSTATE
        entry["outcome"] = "passed"
        save_attempts()
        counts[family] += 1
        print("registered case:", json.dumps({"case_id": case, "input": registered_input,
              "aster": observations["aster"][case], "reference": observations["reference"][case]},
              sort_keys=True), flush=True)

    def input_descriptor(phase, query, statement=None, oids=None, args=None):
        return {"phase": phase, "sql": query, "statement": statement,
                "parameter_oids": oids, "arguments": args,
                "parameter_formats": None, "result_format": 0}

    def sql(case, family, command, **expected):
        check(case, family, (input_descriptor("query", command),
                            lambda conn: execute(conn, command.encode())), **expected)

    def prepare_input(name, query, supplied):
        statements[name] = query
        oids = list(supplied) if supplied else None
        return (input_descriptor("prepare", query, name, oids), lambda conn: prepare(
            conn, name.encode(), query.encode(), len(supplied),
            (c.c_uint * len(supplied))(*supplied) if supplied else None))

    def describe_input(name):
        return (input_descriptor("describe", statements[name], name),
                lambda conn: describe(conn, name.encode()))

    def bind_values(name, args):
        arguments = [None if arg is None else str(arg) for arg in args]
        payload = [None if arg is None else arg.encode() for arg in arguments]
        return (input_descriptor("execute", statements[name], name, args=arguments),
                lambda conn: prepared(conn, name.encode(), len(payload),
                                      (text * len(payload))(*payload), None, None, 0))

    try:
        for side, dsn in [("aster", aster_dsn), ("reference", reference_dsn)]:
            connection = connect(dsn.encode())
            assert connection, "connection allocation failed"
            connections[side] = connection
            # Do not echo libpq connection errors: they may contain DSN secrets.
            assert status(connection) == 0, f"{side} connection failed"
            print(side, "protocol:", protocol(connection), "server build:",
                  parameter_status(connection, b"server_version").decode(), flush=True)
        import hashlib
        import platform
        # dladdr resolves the image actually containing PQlibVersion, even when
        # the loader was given a soname rather than an absolute file path.
        class DlInfo(c.Structure):
            _fields_ = [('filename', text), ('base', ptr), ('symbol', text), ('address', ptr)]
        dladdr = c.CDLL(None).dladdr
        dladdr.argtypes, dladdr.restype = [ptr, c.POINTER(DlInfo)], integer
        info = DlInfo()
        assert dladdr(c.cast(version, ptr), c.byref(info)), 'cannot identify loaded libpq'
        loaded_library = Path(os.fsdecode(info.filename)).resolve(strict=True)
        rust_build = json.loads((evidence_dir / 'rust-build.json').read_text())
        root = Path(__file__).resolve().parents[3]
        target = os.environ.get('PG_CORE_REFERENCE_TARGET_VERSION', '18.6')
        metadata_path = os.environ.get('PG_CORE_REFERENCE_BUILD_FILE')

        def build_metadata():
            if not metadata_path:
                return None
            # Invalid/missing metadata never bypasses the gate or the SQL run.
            try:
                return json.loads(Path(metadata_path).read_text())
            except (OSError, ValueError):
                return None

        def identities():
            source = source_snapshot(root)
            assert source['head'] == rust_build['head'], 'HEAD changed since Rust startup'
            return {'source': source,
                    'build': {'path': rust_build['path'], 'profile': rust_build['profile'],
                              'sha256': hashlib.sha256(Path(rust_build['path']).read_bytes()).hexdigest()},
                    'client': {'python': sys.version, 'library': str(loaded_library),
                               'sha256': hashlib.sha256(loaded_library.read_bytes()).hexdigest(),
                               'PQlibVersion': version()},
                    'reference_build': build_metadata()}

        provenance = dict(run=run_id, target_version=target, **identities(),
                          os=platform.platform(), storage=rust_build['storage'],
                          authentication=rust_build['authentication'], endpoints={})
        for side, conn in connections.items():
            initial = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn), 'unknown')
            path_obs = observe(execute(conn, b'SHOW search_path'))
            archive('environment_search_path', side, {'command': 'SHOW search_path'}, path_obs)
            assert path_obs['status'] == 2 and len(path_obs['rows']) == 1
            provenance['endpoints'][side] = {
                'protocol': protocol(conn),
                'server_version': parameter_status(conn, b'server_version').decode(),
                'encoding': (parameter_status(conn, b'client_encoding') or b'').decode(),
                'timezone': (parameter_status(conn, b'TimeZone') or b'').decode(),
                'search_path': path_obs['rows'][0][0], 'transaction': initial}
        (evidence_dir / 'environment.json').write_text(json.dumps(provenance, sort_keys=True))
        version_sql = "SELECT version(), current_setting('server_version_num'), '' AS archive_empty, NULL::int4 AS archive_null"
        ref = observe(execute(connections['reference'], version_sql.encode()))
        archive('reference_version', 'reference', {'command': version_sql}, ref)
        assert ref["status"] == 2, "reference version query failed"
        build, number = ref['rows'][0][:2]
        assert ref['rows'][0][2:] == ['', None]
        assert ref['cells'][0][2:] == [
            {'kind': 'value', 'length': 0, 'hex': ''}, {'kind': 'sql_null', 'length': 0}]
        assert 180000 <= int(number) < 190000, "reference must be real PostgreSQL 18.x"
        assert "PostgreSQL 18." in build and "AsterSQL" not in build
        print("reference build:", build, "server_version_num:", number, flush=True)
        provenance['reference'] = {'version': build, 'server_version_num': number}
        (evidence_dir / 'environment.json').write_text(json.dumps(provenance, sort_keys=True))

        for type_name, low, high in [("int4", -2147483648, 2147483647),
                                      ("int8", -9223372036854775808, 9223372036854775807)]:
            for syntax in ["suffix", "cast"]:
                for label, literal, expected in [
                    ("min", f"'{low}'", str(low)), ("max", f"'{high}'", str(high)),
                    ("text", "' +0017 '", "17"), ("integer", "-31", "-31"),
                    ("null", "NULL", None),
                ]:
                    expr = (f"{literal}::{type_name}" if syntax == "suffix" else
                            f"CAST({literal} AS {type_name})")
                    sql(f"{type_name}_{syntax}_{label}", "typed_null" if expected is None else "value",
                        f"SELECT {expr} AS n", expected_rows=[[expected]])
                for label, literal, state in [("over", str(high + 1), "22003"),
                                               ("under", str(low - 1), "22003"),
                                               ("invalid", "12x", "22P02")]:
                    expr = (f"'{literal}'::{type_name}" if syntax == "suffix" else
                            f"CAST('{literal}' AS {type_name})")
                    sql(f"{type_name}_{syntax}_{label}", "invalid" if state == "22P02" else "overflow",
                        f"SELECT {expr} AS n", expected_status=7, expected_state=state)
                    sql(f"recover_{type_name}_{syntax}_{label}", "recovery",
                        "SELECT 1::int4 AS n", expected_rows=[["1"]])

        sql("int_alias", "value", "SELECT '+0017'::int AS n", expected_rows=[["17"]])
        sql("nested_narrow", "value", "SELECT ('2147483647'::int8)::int4 AS n",
            expected_rows=[["2147483647"]])
        sql("nested_overflow", "overflow", "SELECT ('2147483648'::int8)::int4 AS n",
            expected_status=7, expected_state="22003")
        sql("nested_recovery", "recovery", "SELECT 1::int4 AS n", expected_rows=[["1"]])

        # Omitted, zero, and supplied source OIDs; repeated/out-of-order parameters.
        for name, query, supplied, inferred, runs in [
            ("inferred", "SELECT $2::int4 AS b, CAST($1 AS bigint) AS a, $2::int4 AS c",
             [], [20, 23], [("9007199254740993", "17"), ("-9223372036854775808", "-2147483648"),
                             (None, None)]),
            ("zero", "SELECT CAST($1 AS integer) AS n", [0], [23], [("31",), (None,)]),
            ("explicit", "SELECT $1::int4 AS n", [20], [20], [("17",), (None,)]),
            ("text_source", "SELECT $1::int8 AS n", [25], [25], [("+0017",), (None,)]),
        ]:
            check(name + "_prepare", "prepare", prepare_input(name, query, supplied), expected_status=1)
            check(name + "_describe", "describe", describe_input(name),
                  expected_status=1, expected_params=inferred)
            for i, args in enumerate(runs):
                if name == "inferred":
                    expected = [[args[1], args[0], args[1]]]
                else:
                    expected = [[None if args[0] is None else str(int(args[0]))]]
                run_label = (["initial", "boundary", "null"] if name == "inferred"
                             else ["initial", "null"])[i]
                check(f"{name}_run_{run_label}", "reexecute", bind_values(name, args), expected_rows=expected)
            for label, argument, state in [("invalid", "12x", "22P02"),
                                            ("overflow", "9223372036854775808", "22003")]:
                args = (argument, "17") if name == "inferred" else (argument,)
                check(f"{name}_{label}", label, bind_values(name, args),
                      expected_status=7, expected_state=state)
                check(f"{name}_{label}_recovery", "recovery", bind_values(name, runs[0]))

        print("PG_DIFFERENTIAL_MID", flush=True)
        assert sys.stdin.readline().strip() == "continue", "MySQL checkpoint was not acknowledged"

        for side, connection in connections.items():
            obs = observe(execute(connection,
                f"CREATE TABLE {table} (id INT PRIMARY KEY, n BIGINT, note TEXT)".encode()))
            assert obs["status"] == 1, (side, obs)
            created.add(side)
            archive('create_table', side, {'command': f'CREATE TABLE {table} (id INT PRIMARY KEY, n BIGINT, note TEXT)'}, obs)
        sql("write", "write", f"INSERT INTO {table} VALUES (1, '9007199254740993'::int8, NULL), (2, NULL, NULL)",
            expected_status=1)
        sql("readback", "readback", f"SELECT CAST(id AS int8) AS id, n::int8 AS n FROM {table} ORDER BY id",
            expected_rows=[["1", "9007199254740993"], ["2", None]])
        empty = f"SELECT id::int4 AS id, CAST(n AS bigint) AS n FROM {table} WHERE id = $1::int4"
        check("empty_prepare", "prepare", prepare_input("empty", empty, []),
              expected_status=1)
        check("empty_describe", "zero_rows", describe_input("empty"),
              expected_status=1, expected_params=[23])
        check("empty_execute", "zero_rows", bind_values("empty", [999]), expected_rows=[])
        # Reuse this run's independent writeback tables for a bounded text probe.
        for side, connection in connections.items():
            command = f"INSERT INTO {table} VALUES (3, 0, ''), (4, 0, 'NULL')"
            obs = observe(execute(connection, command.encode()))
            archive('scalar_text_setup', side, {'command': command}, obs)
            assert obs['status'] == 1, (side, obs)
        for label, key, expected in [('null', 2, [[None]]), ('empty', 3, [['']]),
                                     ('literal_null', 4, [['NULL']]), ('zero_rows', 999, [])]:
            sql('scalar_' + label, 'scalar_' + label,
                f"SELECT note FROM {table} WHERE id = {key}", expected_rows=expected)

        # Project away the unique primary key so genuine duplicate rows remain.
        command = f"INSERT INTO {table} VALUES (5, 0, ''), (6, NULL, NULL)"
        for side, connection in connections.items():
            obs = observe(execute(connection, command.encode()))
            archive('multiset_setup', side, {'command': command}, obs)
            assert obs['status'] == 1, (side, obs)
        sql('multiset_ordered', 'multiset',
            f"SELECT n, note FROM {table} WHERE id >= 2 ORDER BY id",
            expected_rows=[[None, None], ['0', ''], ['0', 'NULL'], ['0', ''], [None, None]])
        sql('multiset_unordered', 'multiset', f"SELECT n, note FROM {table} WHERE id >= 2")

        # All effect probes use this connection and only this run's owned table.
        snapshot_sql = f"SELECT id, n, note FROM {table} ORDER BY id"
        def snapshot(label):
            result = {}
            for side, connection in connections.items():
                result[side] = observe(execute(connection, snapshot_sql.encode()))
                archive(label, side, {'command': snapshot_sql}, result[side])
                assert result[side]['status'] == 2, (side, label)
            return result

        before = snapshot('rejection_before')
        write_query = f"INSERT INTO {table} VALUES ($1, 0, 'rejected')"
        check('invalid_write_prepare', 'prepare', prepare_input('invalid_write', write_query, [23]), expected_status=1)
        check('invalid_write_describe', 'describe', describe_input('invalid_write'), expected_status=1, expected_params=[23])
        check('invalid_int4_write', 'rejection_write', bind_values('invalid_write', ['12x']),
              expected_status=7, expected_state='22P02')
        sql('invalid_int4_write_recovery', 'recovery', 'SELECT 1::int4 AS n', expected_rows=[['1']])
        after = snapshot('rejection_after')
        for side in connections:
            rejection_records[side]['invalid_int4_write'].update(before=before[side], after=after[side])

        sql("begin", "transaction", "BEGIN", expected_status=1)
        sql("transaction_error", "transaction", "SELECT 'x'::int4", expected_status=7, expected_state="22P02")
        assert all(transaction(conn) == 3 for conn in connections.values()), "expected E"
        sql("transaction_failed", "transaction", "SELECT 1::int4 AS n", expected_status=7, expected_state="25P02")
        sql("rollback", "transaction", "ROLLBACK", expected_status=1)
        assert all(transaction(conn) == 0 for conn in connections.values()), "expected I"
        sql("rollback_recovery", "recovery", "SELECT 1::int4 AS n", expected_rows=[["1"]])

        after_transaction = snapshot('transaction_rejection_after')
        for side in connections:
            for case in ['transaction_error', 'transaction_failed']:
                rejection_records[side][case].update(before=after[side], after=after_transaction[side])

        # Mutate only copies of AsterSQL observations: the comparator must reject each corruption.
        for case, key, wrong in [("int4_suffix_min", "rows", [["0"]]),
                                 ("int4_suffix_min", "fields", [("n", 20, 4, -1)]),
                                 ("int4_suffix_invalid", "state", "22003")]:
            registry.claim("negative_self_check_" + key, "negative_self_check",
                           {"phase": "self_check", "source_case_id": case, "mutation_field": key})
            changed = copy.deepcopy(observations["aster"][case])
            changed[key] = wrong
            try:
                equal("negative_self_check_" + key, changed, observations["reference"][case])
            except AssertionError:
                counts["negative_self_check"] += 1
                attempts.append(dict(run=run_id, case_id='negative_self_check_' + key,
                                     attempt=attempt, outcome='passed', assertions=1))
                save_attempts()
            else:
                raise AssertionError("differential comparator missed " + key)
        import hashlib
        for source in ['scalar_null', 'scalar_empty', 'scalar_literal_null']:
            original = observations['aster'][source]
            digest = hashlib.sha256(json.dumps(original, sort_keys=True).encode()).hexdigest()
            for replacement in [None, 'unknown', 'uncollected']:
                changed = copy.deepcopy(original)
                if replacement is None:
                    del changed['cells'][0][0]['kind']
                else:
                    changed['cells'][0][0]['kind'] = replacement
                assert not compare_scalar_output(changed, observations['reference'][source],
                                                 {'result_format': 0}), source
                assert hashlib.sha256(json.dumps(original, sort_keys=True).encode()).hexdigest() == digest
            print('scalar tag self-check:', source, 'mutations=3 raw_sha256=' + digest, flush=True)
        source = observations['aster']['multiset_unordered']
        reference = observations['reference']['multiset_unordered']
        before = copy.deepcopy(source)
        reordered = copy.deepcopy(source)
        for key in ['rows', 'cells']:
            reordered[key] = reordered[key][1:] + reordered[key][:1]
        assert reordered['rows'] != source['rows']
        assert not compare_rows(reordered, source, 'strict')
        equal('multiset_permutation', reordered, reference, 'multiset')
        for mutation in ['remove_duplicate', 'add_duplicate', 'column_order', 'oid', 'typmod']:
            changed = copy.deepcopy(source)
            if mutation in ['remove_duplicate', 'add_duplicate']:
                for key in ['rows', 'cells']:
                    if mutation == 'remove_duplicate':
                        changed[key].pop()
                    else:
                        changed[key].append(copy.deepcopy(changed[key][0]))
            elif mutation == 'column_order':
                for key in ['rows', 'cells']:
                    changed[key] = [list(reversed(row)) for row in changed[key]]
            else:
                field = list(changed['fields'][0])
                field[1 if mutation == 'oid' else 3] += 1
                changed['fields'][0] = tuple(field)
            try:
                equal('multiset_' + mutation, changed, reference, 'multiset')
            except AssertionError:
                pass
            else:
                raise AssertionError('multiset comparator missed ' + mutation)
        assert source == before
        print('multiset self-check: permutation=1 rejected_mutations=5 raw_unchanged=1', flush=True)
        metadata_checks = 0
        for case, phase in [('int4_suffix_min', 'query'), ('int8_suffix_null', 'query'),
                            ('inferred_describe', 'describe'), ('zero_describe', 'describe'),
                            ('explicit_describe', 'describe'), ('text_source_describe', 'describe'),
                            ('empty_describe', 'describe'), ('empty_execute', 'execute')]:
            original = observations['aster'][case]
            baseline = copy.deepcopy(original)
            reference = observations['reference'][case]
            mutations = []
            for index, wrong in [(0, 'wrong'), (1, 999), (2, 999), (3, 999)]:
                changed = copy.deepcopy(original)
                field = list(changed['fields'][0]); field[index] = wrong
                changed['fields'][0] = tuple(field)
                mutations.append((changed, reference))
            for key, value in [('formats', [1] * len(original['fields'])),
                               ('param_count', original['param_count'] + 1)]:
                changed = copy.deepcopy(original); changed[key] = value
                mutations.append((changed, reference))
            if original['params']:
                changed = copy.deepcopy(original); changed['params'][0] = 999
                mutations.append((changed, reference))
            for key in ['fields', 'params', 'param_count', 'formats']:
                changed = copy.deepcopy(original); del changed[key]
                mutations.append((changed, copy.deepcopy(changed)))
            for changed, other in mutations:
                try:
                    equal('metadata_' + case, changed, other, phase=phase)
                except AssertionError:
                    metadata_checks += 1
                else:
                    raise AssertionError('metadata comparator missed mutation: ' + case)
            assert original == baseline, case
        print('metadata self-check: sources=8 rejected_mutations=' + str(metadata_checks)
              + ' raw_unchanged=8 parameter_descriptions=4 zero_rows=2', flush=True)
        for required in ['scalar_null', 'scalar_empty', 'scalar_literal_null', 'scalar_zero_rows',
                         "value", "typed_null", "zero_rows", "overflow", "invalid",
                         "reexecute", "recovery", "write", "readback", "negative_self_check"]:
            assert counts[required] > 0, required
        assert registry.finish() == 104
        print("registered inputs: business=101 self_check=3 missing=0 duplicate=0", flush=True)
        print("historical first-phase accounting: 94=91 business+3 self-checks; current summary follows cleanup", flush=True)
        expected = dict(provenance, **identities())
        eligible = validate_provenance(provenance, expected)
        (evidence_dir / 'provenance-expected.json').write_text(json.dumps(expected, sort_keys=True))
        # Missing source or another exact version leaves functional evidence
        # intact, but cannot become acceptance evidence for the target.
        gate = {'run': run_id, 'target_version': target,
                'actual_version_num': number, 'eligible': eligible,
                'scope': '18.4 baseline only' if eligible and target == '18.4'
                         else ('18.6 exact evidence' if eligible else 'unverified'),
                'stable_inputs': all(provenance[k] == expected[k]
                                     for k in ['source', 'build', 'client', 'reference_build'])}
        (evidence_dir / 'provenance-gate.json').write_text(json.dumps(gate, sort_keys=True))
        assert gate['stable_inputs'], 'provenance inputs changed during run'
        # Negative controls use copies of this run's real observations.
        if eligible:
            mutations = []
            changed = copy.deepcopy(provenance); changed['target_version'] = '18.6' if target == '18.4' else '18.4'
            mutations.append(changed)
            for key in ['reference', 'reference_build', 'endpoints', 'client', 'source', 'build']:
                changed = copy.deepcopy(provenance); changed[key] = None
                mutations.append(changed)
            for section, field in [('client', 'sha256'), ('source', 'workspace_sha256'),
                                   ('build', 'sha256')]:
                changed = copy.deepcopy(provenance)
                changed[section][field] = '0' * 64
                mutations.append(changed)
            changed = copy.deepcopy(provenance)
            del changed['endpoints']['aster']['search_path']
            mutations.append(changed)
            for changed in mutations:
                assert not validate_provenance(changed, expected), 'provenance mutation accepted'
            assert validate_provenance(provenance, expected)
        whitelist_report = {'rules': len(whitelist), 'used': len(whitelist_used), 'ids': whitelist_used}
        (evidence_dir / 'whitelist.json').write_text(json.dumps(whitelist_report, sort_keys=True))
        print('whitelist:', json.dumps(whitelist_report, sort_keys=True), flush=True)
        print('provenance gate:', json.dumps(gate, sort_keys=True), flush=True)
        completed = True
    finally:
        failure = sys.exc_info()[1]
        business_failed = failure is not None
        for entry in attempts:
            if entry['outcome'] == 'pending':
                entry['outcome'] = ('failed' if isinstance(failure, AssertionError) else
                                    'timeout' if isinstance(failure, TimeoutError) else 'exception')
        save_attempts()
        cleanup_errors = []
        cleanup_results = {}
        try:
            for side in sorted(created):
                management = None
                try:
                    connection = connections[side]
                    if status(connection) != 0:
                        # A failed socket cannot recover a transaction. A separate
                        # connection may only remove this run's unique table.
                        dsn = aster_dsn if side == 'aster' else reference_dsn
                        management = connect(dsn.encode())
                        assert management and status(management) == 0, 'cleanup connection failed'
                        connection = management
                    # Rollback first so cleanup also works after a failed transaction assertion.
                    rollback = observe(execute(connection, b"ROLLBACK"))
                    archive('cleanup_rollback', side, {'command': 'ROLLBACK'}, rollback)
                    obs = observe(execute(connection, f"DROP TABLE {table}".encode()))
                    archive('cleanup_drop', side, {'command': f'DROP TABLE {table}'}, obs)
                    assert rollback['status'] == 1, (side, 'cleanup rollback failed')
                    assert obs["status"] == 1, (side, "table cleanup failed")
                    cleanup_results[side] = {'rollback': True, 'drop': True}
                    # Controlled failure is reported only after removing our own table.
                    if os.environ.get('PG_CORE_INJECT_CLEANUP_FAILURE') == side:
                        raise AssertionError('controlled owned-table cleanup failure')
                except Exception as exc:
                    archive('cleanup_failed', side, None, None, 'cleanup_failed', type(exc).__name__)
                    cleanup_errors.append((side, type(exc).__name__))
                finally:
                    if management:
                        finish(management)
            if cleanup_errors and not business_failed:
                raise AssertionError(cleanup_errors)
            if not cleanup_errors:
                print("unique-table cleanup passed for", len(created), "endpoints", flush=True)
        finally:
            for side, connection in connections.items():
                finish(connection)
                archive('connection_closed', side, None, None, 'closed')
                if side in cleanup_results:
                    cleanup_results[side]['connection_closed'] = True
            try:
                if not business_failed and not cleanup_errors:
                    for case in ['invalid_int4_write', 'transaction_error', 'transaction_failed']:
                        for side in connections:
                            record = rejection_records[side][case]
                            record['cleanup'] = cleanup_results.get(side)
                            archive(case + '_rejection_contract', side, None, record)
                        actual, reference = (rejection_records[side][case] for side in ['aster', 'reference'])
                        assert compare_rejection(actual, reference, rejection_contracts[case]), case
                        mutations = {
                            'phase': None, 'connection_ok': False, 'recovery': None,
                            'after': None, 'cleanup': None, 'classification': 'unimplemented',
                            'error': dict(actual['error'], state='XX000')}
                        baseline = copy.deepcopy(actual)
                        for key, wrong in mutations.items():
                            changed = copy.deepcopy(actual)
                            changed[key] = wrong
                            assert not compare_rejection(changed, reference, rejection_contracts[case]), (case, key)
                            assert not compare_rejection(changed, changed, rejection_contracts[case]), (case, key)
                        assert actual == baseline
                    print('rejection contracts: valid=3 illegal_writes=1 negative_mutations=21 cleanup_endpoints=2', flush=True)
            finally:
                final_completed = completed and not business_failed and sys.exc_info()[0] is None
                terminal = failure or sys.exc_info()[1]
                run_error = ('failed' if isinstance(terminal, AssertionError) else
                             'timeout' if isinstance(terminal, TimeoutError) else 'exception')
                summary = summarize_run(run_id, registry.cases['cases'], attempts,
                                        not cleanup_errors and len(cleanup_results) == 2,
                                        final_completed, run_error)
                (evidence_dir / 'summary.json').write_text(json.dumps(summary, sort_keys=True))
                print('differential final summary:', json.dumps(summary, sort_keys=True), flush=True)
                if not business_failed and sys.exc_info()[0] is None:
                    assert summary['passed'], 'incomplete differential evidence'


def integer14_differential(aster_dsn):
    """Integer family only: collect each endpoint independently before comparison."""
    import copy
    import hashlib
    import json
    import signal
    from pathlib import Path
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    name = api('PQfname', text, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    transaction = api('PQtransactionStatus', integer, ptr)
    evidence = Path(os.environ['PG_INTEGER14_EVIDENCE_DIR'])
    reference = os.environ['PG18_REFERENCE_DSN']
    table = f'public.pg_integer14_{os.getpid()}'
    observations = {}
    bridges = {}
    cleanup = {}
    def interrupted(signum, frame):
        raise KeyboardInterrupt('integer14 client interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    def observe(result, phase):
        assert result
        try:
            state = ef(result, ord('C'))
            return dict(phase=phase, status=result_status(result),
                        sqlstate=state.decode() if state else None,
                        metadata=[(name(result,i).decode(),field_type(result,i),size(result,i),mod(result,i)) for i in range(columns(result))],
                        parameters=[pt(result,i) for i in range(np(result))],
                        rows=[[None if is_null(result,r,i) else value(result,r,i).decode() for i in range(columns(result))] for r in range(rows(result))])
        finally:
            clear(result)
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn)==0, f'{side} connection failed'
        collected = {}
        def q(case, sql, state=None, command=False):
            result = observe(execute(conn,sql.encode()), 'query')
            result['input'] = sql
            collected[case] = result
            assert result['status']==(7 if state else 1 if command else 2), (side,case,result)
            assert result['sqlstate']==state, (side,case,result)
            return result
        try:
            if side=='reference':
                identity=q('identity', "SELECT version(), current_setting('server_version_num')")
                assert identity['rows'][0][1]=='180006', 'exact PostgreSQL 18.6 required'
                (evidence/'reference-identity.json').write_text(json.dumps(collected.pop('identity')))
                build_path=Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE'])
                build=json.loads(build_path.read_text())
                assert hashlib.sha256(Path(build['artifact_path']).read_bytes()).hexdigest()==build['artifact_sha256']
                (evidence/'reference-build.json').write_text(json.dumps(build))
                (evidence/'client.json').write_text(json.dumps({'version':version(),'sha256':hashlib.sha256(Path(library).read_bytes()).hexdigest()}))
            if side=='reference':
                q('bridge-create',f'CREATE TABLE {table}_bridge (id INT PRIMARY KEY,a SMALLINT,b INT,c INT,d BIGINT,e NUMERIC)',command=True)
                q('bridge-write',f'INSERT INTO {table}_bridge VALUES (1,0,0,0,0,0),(2,255,65535,16777215,4294967295,18446744073709551615),(3,NULL,NULL,NULL,NULL,NULL)',command=True)
                collected.pop('bridge-create'); collected.pop('bridge-write')
            q('create',f'CREATE TABLE {table} (id INT PRIMARY KEY, n SMALLINT)',command=True)
            q('empty',f'SELECT n FROM {table}')
            q('write',f'INSERT INTO {table} VALUES (1,-32768),(2,32767),(3,NULL)',command=True)
            q('read',f'SELECT n FROM {table} ORDER BY id')
            for type_name,oid,lower,upper in [('int2',21,-32768,32767),('int4',23,-2147483648,2147483647),('int8',20,-9223372036854775808,9223372036854775807)]:
                for label,item in [('min',str(lower)),('max',str(upper)),('null',None),('space',' \t+17\n'),('hex','0x7f'),('octal','0o17'),('binary','0b101'),('separator','1_2')]:
                    sql=f"SELECT CAST({'NULL' if item is None else chr(39)+item+chr(39)} AS {type_name}) AS n"
                    q(f'{type_name}-literal-{label}',sql)
                for declared in (0,oid):
                    statement=f'i{oid}_{declared}'.encode()
                    oids=(c.c_uint*1)(declared) if declared else None
                    query=f'SELECT CAST($1 AS {type_name}) AS n'.encode()
                    result=observe(prepare(conn,statement,query,1,oids),'prepare')
                    collected[f'{type_name}-prepare-{declared}']=result
                    assert result['status']==1,(side,result)
                    result=observe(describe(conn,statement),'describe')
                    collected[f'{type_name}-describe-{declared}']=result
                    assert result['parameters']==[oid],(side,result)
                    vocabulary=[str(lower),str(upper),None,' +17 ','0x7f','0o17','0b101','1_2','0x_FF','', '1x','1.5','_1','1_','1__2','0x',str(lower-1),str(upper+1)]
                    for i,item in enumerate(vocabulary):
                        args=(text*1)(None if item is None else item.encode())
                        result=observe(prepared(conn,statement,1,args,None,None,0),'execute')
                        result['input']={'value':item,'declared_oid':declared}
                        collected[f'{type_name}-param-{declared}-{i}']=result
                        expected='22003' if i>=len(vocabulary)-2 else '22P02' if i>=9 else None
                        assert result['sqlstate']==expected,(side,type_name,i,result)
                        assert result['status']==(7 if expected else 2),(side,type_name,i,result)
                        if expected: q(f'{type_name}-sync-{declared}-{i}','SELECT 1 AS n')
            for i,item in enumerate(['0','2147483647','2147483648','9223372036854775807','9223372036854775808','-2147483648','-2147483649','-9223372036854775808','-9223372036854775809','1.0','1e0']):
                q(f'default-{i}',f'SELECT {item} AS n')
            for i,item in enumerate(['32768','-32769','bad','']):
                q(f'reject-write-{i}',f"INSERT INTO {table} VALUES (4, CAST('{item}' AS int2))",state='22003' if i<2 else '22P02')
                q(f'reject-write-recovery-{i}',f'SELECT n FROM {table} ORDER BY id')
            q('begin','BEGIN',command=True)
            q('transaction-error',"SELECT CAST('bad' AS int2)",state='22P02')
            assert transaction(conn)==3
            q('failed-transaction','SELECT 1',state='25P02')
            q('rollback','ROLLBACK',command=True)
            assert transaction(conn)==0
            q('rollback-recovery',f'SELECT n FROM {table} ORDER BY id')
            if side=='aster':
                # Reference has signed/numeric declarations covering the widened
                # domains, never MySQL unsigned syntax.
                print('PG_INTEGER14_MID',flush=True)
                assert sys.stdin.readline().strip()=='continue'
            unsigned={}
            source='public.unsigned_source' if side=='aster' else table+'_bridge'
            for case,sql in [('rows',f'SELECT a,b,c,d,e FROM {source} ORDER BY id'),('empty',f'SELECT a,b,c,d,e FROM {source} WHERE FALSE')]:
                unsigned[case]=observe(execute(conn,sql.encode()),'query')
                assert unsigned[case]['status']==2,unsigned[case]
                assert [field[1:] for field in unsigned[case]['metadata']]==[(21,2,-1),(23,4,-1),(23,4,-1),(20,8,-1),(1700,-1,-1)]
            assert unsigned['rows']['rows']==[['0']*5,['255','65535','16777215','4294967295','18446744073709551615'],[None]*5]
            bridges[side]=unsigned
            (evidence/f'unsigned-{side}.json').write_text(json.dumps({'source':source,'observations':unsigned}))
            observations[side]=collected
        finally:
            (evidence/f'{side}.json').write_text(json.dumps(collected,sort_keys=True))
            try:
                clear(execute(conn,b'ROLLBACK'))
                result=observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()),'cleanup')
                cleanup[side]=result['status']==1
                if side=='reference':
                    result=observe(execute(conn,f'DROP TABLE IF EXISTS {table}_bridge'.encode()),'cleanup')
                    cleanup[side]=cleanup[side] and result['status']==1
            finally:
                finish(conn)
                (evidence/'cleanup.json').write_text(json.dumps(cleanup))
    assert observations['aster']==observations['reference'], next(((k,observations['aster'].get(k),v) for k,v in observations['reference'].items() if observations['aster'].get(k)!=v),'different cases')
    assert bridges['aster']==bridges['reference'], bridges
    for field,replacement in [('rows',[['perturbed']]),('metadata',[('n',999,2,-1)]),('sqlstate','XXXXX')]:
        changed=copy.deepcopy(observations['aster'])
        changed['int2-literal-min'][field]=replacement
        assert changed!=observations['reference'],field
    assert all(cleanup.values()) and len(cleanup)==2
    summary={'cases':len(observations['aster']),'negative_controls':3,'unsigned_cells':15,'unsigned_empty':1,'cleanup':cleanup,'target_version':'18.6'}
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('integer14 differential:',json.dumps(summary),flush=True)


def numeric15_differential(aster_dsn):
    """Numeric family only: collect each endpoint independently before comparison."""
    import copy
    import hashlib
    import json
    import signal
    import time
    import resource
    from pathlib import Path
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    name = api('PQfname', text, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    transaction = api('PQtransactionStatus', integer, ptr)
    evidence = Path(os.environ['PG_NUMERIC15_EVIDENCE_DIR'])
    reference = os.environ['PG18_REFERENCE_DSN']
    table = f'public.pg_numeric15_{os.getpid()}'
    observations = {}
    cleanup = {}
    resources = {}
    def interrupted(signum, frame):
        raise KeyboardInterrupt('numeric15 client interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    def compact(item):
        if isinstance(item,str) and len(item)>256:
            return {'length':len(item),'sha256':hashlib.sha256(item.encode()).hexdigest()}
        return item
    def observe(result, phase):
        assert result
        try:
            state = ef(result, ord('C'))
            if state == b'XX000':
                print('numeric15 internal error:',result_error(result).decode(),flush=True)
            return dict(phase=phase, status=result_status(result),
                        sqlstate=state.decode() if state else None,
                        metadata=[(name(result,i).decode(),field_type(result,i),size(result,i),mod(result,i)) for i in range(columns(result))],
                        parameters=[pt(result,i) for i in range(np(result))],
                        rows=[[None if is_null(result,r,i) else compact(value(result,r,i).decode()) for i in range(columns(result))] for r in range(rows(result))])
        finally:
            clear(result)
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn)==0, f'{side} connection failed'
        collected = {}
        resources[side] = {}
        def q(case, sql, state=None, command=False):
            started = time.perf_counter()
            result = observe(execute(conn,sql.encode()), 'query')
            if len(sql)>1000:
                resources[side][case] = {'elapsed_seconds':time.perf_counter()-started,'input_bytes':len(sql.encode()),'python_maxrss':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'maxrss_unit':'bytes on macOS, KiB on Linux'}
            result['input'] = compact(sql)
            collected[case] = result
            assert result['status']==(7 if state else 1 if command else 2), (side,case,result)
            assert result['sqlstate']==state, (side,case,result)
            return result
        try:
            if side=='reference':
                identity=q('identity', "SELECT version(), current_setting('server_version_num')")
                assert identity['rows'][0][1]=='180006', 'exact PostgreSQL 18.6 required'
                (evidence/'reference-identity.json').write_text(json.dumps(collected.pop('identity')))
                build_path=Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE'])
                build=json.loads(build_path.read_text())
                assert hashlib.sha256(Path(build['artifact_path']).read_bytes()).hexdigest()==build['artifact_sha256']
                (evidence/'reference-build.json').write_text(json.dumps(build))
                (evidence/'client.json').write_text(json.dumps({'version':version(),'sha256':hashlib.sha256(Path(library).read_bytes()).hexdigest()}))
            q('create',f'CREATE TABLE {table} (id INT PRIMARY KEY, n NUMERIC(5,2), neg NUMERIC(2,-3), wide NUMERIC(3,5))',command=True)
            q('empty',f'SELECT n,neg,wide FROM {table}')
            q('write',f"INSERT INTO {table} VALUES (1,12.345,12500,0.001234),(2,NULL,NULL,NULL)",command=True)
            q('nan-write',f"INSERT INTO {table} VALUES (5,CAST('NaN' AS numeric),CAST('NaN' AS numeric),CAST('NaN' AS numeric))",command=True)
            q('read',f'SELECT n,neg,wide FROM {table} ORDER BY id')
            q('extra-create',f'CREATE TABLE {table}_extra (id INT PRIMARY KEY,u NUMERIC,p NUMERIC(2),h NUMERIC(1000,0))',command=True)
            q('extra-empty',f'SELECT u,p,h FROM {table}_extra')
            q('extra-write',f"INSERT INTO {table}_extra VALUES (1,CAST('Infinity' AS numeric),2.5,CAST('{ '9'*1000 }' AS numeric)),(2,CAST('NaN' AS numeric),NULL,NULL),(3,NULL,NULL,NULL)",command=True)
            q('extra-read',f'SELECT u,p,h FROM {table}_extra ORDER BY id')
            statement=b'write_numeric'
            query=f'INSERT INTO {table} (id,n,neg,wide) VALUES (3,CAST($1 AS numeric),CAST($2 AS numeric),CAST($3 AS numeric))'.encode()
            collected['write-prepare']=observe(prepare(conn,statement,query,3,(c.c_uint*3)(1700,1700,1700)),'prepare')
            assert collected['write-prepare']['status']==1,(side,collected['write-prepare'])
            collected['write-describe']=observe(describe(conn,statement),'describe')
            assert collected['write-describe']['parameters']==[1700]*3,(side,collected['write-describe'])
            collected['write-param']=observe(prepared(conn,statement,3,(text*3)(b'-12.345',b'-12500',b'0.001234'),None,None,0),'execute')
            assert collected['write-param']['status']==1,(side,collected['write-param'])
            q('write-param-read',f'SELECT n,neg,wide FROM {table} ORDER BY id')
            for label,item,expected in [('integer-max','9'*131072,None),('fraction-max','0.'+'0'*16382+'1',None),('integer-over','9'*131073,'22003'),('fraction-over','0.'+'0'*16383+'1','22003')]:
                q(label,f"SELECT CAST('{item}' AS numeric) AS n",state=expected)
                if expected:q(label+'-sync','SELECT 1 AS n')
            literals=['0','-0.000','1.2300','1e4','1e-5','NaN','Infinity','-Infinity','9'*1000,'0.'+'0'*999+'1']
            for i,item in enumerate(literals):
                q(f'literal-{i}',f"SELECT CAST('{item}' AS numeric) AS n")
            for i,(item,typ) in enumerate([('12.345','numeric(5,2)'),('-12.345','numeric(5,2)'),('12500','numeric(2,-3)'),('0.001234','numeric(3,5)'),('NULL','numeric(5,2)')]):
                expression='NULL' if item=='NULL' else "'"+item+"'"
                q(f'typmod-{i}',f'SELECT CAST({expression} AS {typ}) AS n')
            for declared in (0,1700):
                statement=f'n{declared}'.encode()
                oids=(c.c_uint*1)(declared) if declared else None
                result=observe(prepare(conn,statement,b'SELECT CAST($1 AS numeric) AS n',1,oids),'prepare')
                collected[f'prepare-{declared}']=result
                assert result['status']==1,(side,result)
                result=observe(describe(conn,statement),'describe')
                collected[f'describe-{declared}']=result
                assert result['parameters']==[1700],(side,result)
                for i,item in enumerate(literals+[None,'bad','']):
                    args=(text*1)(None if item is None else item.encode())
                    result=observe(prepared(conn,statement,1,args,None,None,0),'execute')
                    result['input']=compact(item)
                    collected[f'param-{declared}-{i}']=result
                    expected='22P02' if item in ('bad','') else None
                    assert result['sqlstate']==expected,(side,i,result)
                    assert result['status']==(7 if expected else 2),(side,i,result)
                    if expected:q(f'sync-{declared}-{i}','SELECT 1 AS n')
            for i,(item,typ) in enumerate([('2.5','int2'),('-2.5','int4'),('9223372036854775806.5','int8')]):
                q(f'cast-{i}',f"SELECT CAST(CAST('{item}' AS numeric) AS {typ}) AS n")
            for i,item in enumerate(['999.995','Infinity','-Infinity']):
                q(f'reject-write-{i}',f"INSERT INTO {table} (id,n) VALUES (4,CAST('{item}' AS numeric))",state='22003')
                q(f'reject-write-recovery-{i}',f'SELECT n,neg,wide FROM {table} ORDER BY id')
            q('begin','BEGIN',command=True)
            q('transaction-error',"SELECT CAST('bad' AS numeric)",state='22P02')
            assert transaction(conn)==3
            q('failed-transaction','SELECT 1',state='25P02')
            q('rollback','ROLLBACK',command=True)
            assert transaction(conn)==0
            q('rollback-recovery',f'SELECT n,neg,wide FROM {table} ORDER BY id')
            if side=='aster':
                print('PG_NUMERIC15_MID',flush=True)
                assert sys.stdin.readline().strip()=='continue'
            observations[side]=collected
        finally:
            (evidence/f'{side}.json').write_text(json.dumps(collected,sort_keys=True))
            try:
                clear(execute(conn,b'ROLLBACK'))
                result=observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()),'cleanup')
                cleanup[side]=result['status']==1
                extra=observe(execute(conn,f'DROP TABLE IF EXISTS {table}_extra'.encode()),'cleanup')
                cleanup[side]=cleanup[side] and extra['status']==1
            finally:
                finish(conn)
                (evidence/'cleanup.json').write_text(json.dumps(cleanup))
                (evidence/'resources.json').write_text(json.dumps(resources))
    assert observations['aster']==observations['reference'], next(((k,observations['aster'].get(k),v) for k,v in observations['reference'].items() if observations['aster'].get(k)!=v),'different cases')
    for field,replacement in [('rows',[['perturbed']]),('metadata',[('n',999,2,-1)]),('sqlstate','XXXXX')]:
        changed=copy.deepcopy(observations['aster'])
        changed['literal-0'][field]=replacement
        assert changed!=observations['reference'],field
    assert all(cleanup.values()) and len(cleanup)==2
    summary={'cases':len(observations['aster']),'negative_controls':3,'cleanup':cleanup,'target_version':'18.6'}
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('numeric15 differential:',json.dumps(summary),flush=True)


def float20_differential(aster_dsn):
    """Float family only: collect each endpoint independently before comparison."""
    import copy
    import hashlib
    import json
    import signal
    import time
    import resource
    from pathlib import Path
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    name = api('PQfname', text, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    transaction = api('PQtransactionStatus', integer, ptr)
    evidence = Path(os.environ['PG_FLOAT20_EVIDENCE_DIR'])
    reference = os.environ['PG18_REFERENCE_DSN']
    table = f'public.pg_float20_{os.getpid()}'
    observations = {}
    cleanup = {}
    resources = {}
    def interrupted(signum, frame):
        raise KeyboardInterrupt('float20 client interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    def compact(item):
        if isinstance(item,str) and len(item)>256:
            return {'length':len(item),'sha256':hashlib.sha256(item.encode()).hexdigest()}
        return item
    def observe(result, phase):
        assert result
        try:
            state = ef(result, ord('C'))
            if state == b'XX000':
                print('float20 internal error:',result_error(result).decode(),flush=True)
            return dict(phase=phase, status=result_status(result),
                        sqlstate=state.decode() if state else None,
                        metadata=[(name(result,i).decode(),field_type(result,i),size(result,i),mod(result,i)) for i in range(columns(result))],
                        parameters=[pt(result,i) for i in range(np(result))],
                        rows=[[None if is_null(result,r,i) else compact(value(result,r,i).decode()) for i in range(columns(result))] for r in range(rows(result))])
        finally:
            clear(result)
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn)==0, f'{side} connection failed'
        collected = {}
        resources[side] = {}
        def q(case, sql, state=None, command=False):
            started = time.perf_counter()
            result = observe(execute(conn,sql.encode()), 'query')
            if len(sql)>1000:
                resources[side][case] = {'elapsed_seconds':time.perf_counter()-started,'input_bytes':len(sql.encode()),'python_maxrss':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'maxrss_unit':'bytes on macOS, KiB on Linux'}
            result['input'] = compact(sql)
            collected[case] = result
            assert result['status']==(7 if state else 1 if command else 2), (side,case,result)
            assert result['sqlstate']==state, (side,case,result)
            return result
        try:
            if side=='reference':
                identity=q('identity', "SELECT version(), current_setting('server_version_num')")
                assert identity['rows'][0][1]=='180006', 'exact PostgreSQL 18.6 required'
                (evidence/'reference-identity.json').write_text(json.dumps(collected.pop('identity')))
                build_path=Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE'])
                build=json.loads(build_path.read_text())
                assert hashlib.sha256(Path(build['artifact_path']).read_bytes()).hexdigest()==build['artifact_sha256']
                (evidence/'reference-build.json').write_text(json.dumps(build))
                (evidence/'client.json').write_text(json.dumps({'version':version(),'sha256':hashlib.sha256(Path(library).read_bytes()).hexdigest()}))
            q('create',f'CREATE TABLE {table} (id INT PRIMARY KEY, f FLOAT4, d FLOAT8)',command=True)
            q('empty',f'SELECT f,d FROM {table}')
            q('write',f"INSERT INTO {table} VALUES (1,CAST('-0' AS float4),CAST('-0' AS float8)),(2,NULL,NULL)",command=True)
            q('read',f'SELECT f,d FROM {table} ORDER BY id')
            for typ,oid,width,items in [
                ('float4',700,4,['0','-0','0.1','1.000000059604644775390625','1.000000059604644775390626','16777217','3.4028234663852886e38','1.1754943508222875e-38','1.401298464324817e-45','NaN','Infinity','-Infinity']),
                ('float8',701,8,['0','-0','0.1','1.00000000000000011102230246251565404236316680908203125','9007199254740993','1.7976931348623157e308','2.2250738585072014e-308','4.9406564584124654e-324','NaN','Infinity','-Infinity'])]:
                for declared in (0,oid):
                    statement=f'f{oid}_{declared}'.encode()
                    collected[f'prepare-{oid}-{declared}']=observe(prepare(conn,statement,f'SELECT CAST($1 AS {typ}) AS n'.encode(),1,(c.c_uint*1)(declared) if declared else None),'prepare')
                    desc=observe(describe(conn,statement),'describe')
                    collected[f'describe-{oid}-{declared}']=desc
                    assert desc['parameters']==[oid] and desc['metadata']==[('n',oid,width,-1)],(side,desc)
                    for i,item in enumerate(items+[None,'bad','','1e9999','1e-9999']):
                        case=f'param-{oid}-{declared}-{i}'
                        result=observe(prepared(conn,statement,1,(text*1)(None if item is None else item.encode()),None,None,0),'execute')
                        result['input']=item
                        collected[case]=result
                        state='22P02' if item in ('bad','') else '22003' if item in ('1e9999','1e-9999') else None
                        assert result['sqlstate']==state and result['status']==(7 if state else 2),(side,case,result)
                        if state:q(case+'-sync','SELECT 1 AS n')
                        else:
                            assert result['metadata']==[('n',oid,width,-1)],(side,case,result)
                            if item is not None:
                                text_value=result['rows'][0][0]
                                import math, struct
                                number=float(text_value)
                                # NaNs compare by category; finite values and signed zero by target-width bits.
                                bits='NaN' if math.isnan(number) else struct.pack('!f' if width==4 else '!d',number).hex()
                                result['target_bits']=bits
                                roundtrip=q(case+'-roundtrip',f"SELECT CAST('{text_value}' AS {typ}) AS n")
                                assert roundtrip['rows']==result['rows'] and roundtrip['metadata']==result['metadata'],(side,case,roundtrip)
                for i,item in enumerate(items):
                    q(f'literal-{oid}-{i}',f"SELECT CAST('{item}' AS {typ}) AS n")
                for i,(item,state) in enumerate([('bad','22P02'),('1e9999','22003'),('1e-9999','22003')]):
                    q(f'reject-write-{oid}-{i}',f"INSERT INTO {table} (id,{'f' if width==4 else 'd'}) VALUES (3,CAST('{item}' AS {typ}))",state=state)
                    unchanged=q(f'no-write-{oid}-{i}',f'SELECT f,d FROM {table} ORDER BY id')
                    assert unchanged['rows']==collected['read']['rows']
            for i,(item,source,target) in enumerate([('2.5','float4','int2'),('-2.5','float8','int4'),('32767.4','float8','int2'),('2147483646.5','float8','int4'),('9223372036854774784','float8','int8'),('1.000000059604644775390625','float8','float4'),('-0','float4','float8'),('NaN','float8','float4')]):
                q(f'cast-{i}',f"SELECT CAST(CAST('{item}' AS {source}) AS {target}) AS n")
            for i,(item,target) in enumerate([('32767.5','int2'),('2147483647.5','int4'),('9223372036854775808','int8'),('NaN','int4'),('Infinity','int8'),('3.5e38','float4'),('1e-50','float4')]):
                q(f'cast-error-{i}',f"SELECT CAST(CAST('{item}' AS float8) AS {target}) AS n",state='22003')
                q(f'cast-sync-{i}','SELECT 1 AS n')
            q('begin','BEGIN',command=True)
            q('transaction-error',"SELECT CAST('bad' AS float4)",state='22P02')
            assert transaction(conn)==3
            q('failed-transaction','SELECT 1',state='25P02')
            q('rollback','ROLLBACK',command=True)
            assert transaction(conn)==0
            q('rollback-recovery',f'SELECT f,d FROM {table} ORDER BY id')
            if side=='aster':
                print('PG_FLOAT20_MID',flush=True)
                assert sys.stdin.readline().strip()=='continue'
            observations[side]=collected
        finally:
            (evidence/f'{side}.json').write_text(json.dumps(collected,sort_keys=True))
            try:
                clear(execute(conn,b'ROLLBACK'))
                result=observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()),'cleanup')
                cleanup[side]=result['status']==1
            finally:
                finish(conn)
                (evidence/'cleanup.json').write_text(json.dumps(cleanup))
                (evidence/'resources.json').write_text(json.dumps(resources))
    assert observations['aster']==observations['reference'], next(((k,observations['aster'].get(k),v) for k,v in observations['reference'].items() if observations['aster'].get(k)!=v),'different cases')
    for field,replacement in [('rows',[['perturbed']]),('metadata',[('n',999,2,-1)]),('sqlstate','XXXXX')]:
        changed=copy.deepcopy(observations['aster'])
        changed['literal-700-0'][field]=replacement
        assert changed!=observations['reference'],field
    assert all(cleanup.values()) and len(cleanup)==2
    summary={'cases':len(observations['aster']),'negative_controls':3,'cleanup':cleanup,'target_version':'18.6'}
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('float20 differential:',json.dumps(summary),flush=True)


def bytea28_differential(aster_dsn):
    """Bytea family only: collect each endpoint independently before comparison."""
    import copy
    import hashlib
    import json
    import signal
    import time
    import resource
    from pathlib import Path
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    name = api('PQfname', text, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    transaction = api('PQtransactionStatus', integer, ptr)
    evidence = Path(os.environ['PG_BYTEA28_EVIDENCE_DIR'])
    reference = os.environ['PG18_REFERENCE_DSN']
    table = f'public.pg_bytea28_{os.getpid()}'
    observations = {}
    cleanup = {}
    resources = {}
    def interrupted(signum, frame):
        raise KeyboardInterrupt('bytea28 client interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    def observe(result, phase):
        assert result
        try:
            state = ef(result, ord('C'))
            if state == b'XX000':
                print('bytea28 internal error:',result_error(result).decode(),flush=True)
            return dict(phase=phase, status=result_status(result),
                        sqlstate=state.decode() if state else None,
                        metadata=[(name(result,i).decode(),field_type(result,i),size(result,i),mod(result,i)) for i in range(columns(result))],
                        parameters=[pt(result,i) for i in range(np(result))],
                        rows=[[None if is_null(result,r,i) else value(result,r,i).decode() for i in range(columns(result))] for r in range(rows(result))])
        finally:
            clear(result)
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn)==0, f'{side} connection failed'
        collected = {}
        resources[side] = {}
        def q(case, sql, state=None, command=False):
            started = time.perf_counter()
            result = observe(execute(conn,sql.encode()), 'query')
            if len(sql)>1000 or case.startswith('max-'):
                resources[side][case] = {'elapsed_seconds':time.perf_counter()-started,'input_bytes':len(sql.encode()),'python_maxrss':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'maxrss_unit':'bytes on macOS, KiB on Linux'}
            result['input'] = sql
            collected[case] = result
            assert result['status']==(7 if state else 1 if command else 2), (side,case,result)
            assert result['sqlstate']==state, (side,case,result)
            return result
        try:
            if side=='reference':
                identity=q('identity', "SELECT version(), current_setting('server_version_num')")
                assert identity['rows'][0][1]=='180006', 'exact PostgreSQL 18.6 required'
                (evidence/'reference-identity.json').write_text(json.dumps(collected.pop('identity')))
                build_path=Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE'])
                build=json.loads(build_path.read_text())
                assert hashlib.sha256(Path(build['artifact_path']).read_bytes()).hexdigest()==build['artifact_sha256']
                (evidence/'reference-build.json').write_text(json.dumps(build))
                (evidence/'client.json').write_text(json.dumps({'version':version(),'sha256':hashlib.sha256(Path(library).read_bytes()).hexdigest()}))
            q('create',f'CREATE TABLE {table} (id INT PRIMARY KEY, b BYTEA)',command=True)
            empty=q('empty',f'SELECT b FROM {table}')
            assert empty['rows']==[] and empty['metadata']==[('b',17,-1,-1)]
            inputs=[('',b''),(None,None),(r'\x00ff00',bytes([0,255,0])),
                    (r'\000\377\000',bytes([0,255,0])),(r'a\\z',b'a\\z'),
                    (r'\303\050',bytes([195,40])),(r'\x00000100',bytes([0,0,1,0]))]
            def canonical(raw):
                return None if raw is None else '\\x'+raw.hex()
            inputs.append(('\\x'+bytes(range(256)).hex(),bytes(range(256))))
            # Reuse the OID17 parameter interface delivered by tasks 25–27.
            for declared in [17]:
                statement=f'bytea28_{declared}'.encode()
                key=f'prepare-{declared}'
                collected[key]=observe(prepare(conn,statement,b'SELECT $1 AS n',1,(c.c_uint*1)(declared) if declared else None),'prepare')
                assert collected[key]['status']==1
                desc=observe(describe(conn,statement),'describe')
                collected[f'describe-{declared}']=desc
                assert desc['parameters']==[17] and desc['metadata']==[('n',17,-1,-1)]
                for i,(item,raw) in enumerate(inputs):
                    for encoding,encoded in [('input',item),('canonical',canonical(raw))]:
                        result=observe(prepared(conn,statement,1,(text*1)(None if encoded is None else encoded.encode()),None,None,0),'execute')
                        collected[f'param-{declared}-{i}-{encoding}']=result
                        assert result['status']==2 and result['rows']==[[canonical(raw)]],(side,i,result)
                        assert result['metadata']==[('n',17,-1,-1)]
            for i,(item,raw) in enumerate(inputs):
                result=observe(params(conn,b'SELECT $1 AS n',1,(c.c_uint*1)(17),(text*1)(None if item is None else item.encode()),None,None,0),'execute')
                collected[f'direct-{i}']=result
                assert result['rows']==[[canonical(raw)]] and result['metadata']==[('n',17,-1,-1)]
                result=observe(params(conn,f'INSERT INTO {table} VALUES ({i},$1)'.encode(),1,(c.c_uint*1)(17),(text*1)(None if item is None else item.encode()),None,None,0),'execute')
                collected[f'write-{i}']=result
                assert result['status']==1
            read=q('read',f'SELECT b FROM {table} ORDER BY id')
            assert read['rows']==[[canonical(raw)] for _,raw in inputs]
            # Capture reference escape output, then rebind those bytes on both sides.
            # AsterSQL's bytea_output session setting belongs to the session workflow.
            if side=='reference':
                clear(execute(conn,b"SET bytea_output='escape'"))
                escape_rows=observe(execute(conn,f'SELECT b FROM {table} ORDER BY id'.encode()),'query')['rows']
                (evidence/'reference-escape.json').write_text(json.dumps(escape_rows))
                clear(execute(conn,b"SET bytea_output='hex'"))
            for i,([encoded],(_,raw)) in enumerate(zip(escape_rows,inputs)):
                result=observe(prepared(conn,b'bytea28_17',1,(text*1)(None if encoded is None else encoded.encode()),None,None,0),'execute')
                collected[f'escape-output-rebind-{i}']=result
                assert result['rows']==[[canonical(raw)]] and result['status']==2
            for i,(item,error_state) in enumerate([(r'\x0','22023'),(r'\xgg','22023'),(r'\400','22P02'),(r'\08','22P02'),(r'\q','22P02'),('\\','22P02')]):
                assert q(f'recover-literal-{i}','SELECT 1 AS n')['rows']==[['1']]
                result=observe(params(conn,f'INSERT INTO {table} VALUES (99,$1)'.encode(),1,(c.c_uint*1)(17),(text*1)(item.encode()),None,None,0),'bind')
                collected[f'reject-write-{i}']=result
                assert result['status']==7 and result['sqlstate']==error_state,(side,i,result)
                assert q(f'no-write-{i}',f'SELECT b FROM {table} ORDER BY id')['rows']==read['rows']
            import socket, struct
            pq_socket=api('PQsocket',integer,ptr)
            raw_conn=connect(dsn.encode())
            assert raw_conn and status(raw_conn)==0
            raw=socket.socket(fileno=os.dup(pq_socket(raw_conn)))
            original_blocking=os.get_blocking(pq_socket(raw_conn))
            raw.settimeout(10)
            def frame(tag,payload):
                return tag+struct.pack('!I',len(payload)+4)+payload
            def exact(n):
                data=b''
                while len(data)<n:
                    part=raw.recv(n-len(data))
                    assert part
                    data+=part
                return data
            try:
                for oid in [17]:
                    parse=b'\x00SELECT $1 AS n\x00'+struct.pack('!HI',1,oid)
                    bind=b'\x00\x00'+struct.pack('!HHI',0,1,0)+struct.pack('!H',0)
                    raw.sendall(frame(b'P',parse)+frame(b'B',bind)+frame(b'D',b'P\x00')+frame(b'E',b'\x00'+struct.pack('!I',0))+frame(b'S',b''))
                    portal=[]
                    while True:
                        tag=exact(1)
                        body=exact(struct.unpack('!I',exact(4))[0]-4)
                        if tag==b'T':
                            assert struct.unpack('!H',body[:2])[0]==1
                            offset=body.index(b'\x00',2)+1
                            _,_,actual_oid,width,modifier,fmt=struct.unpack('!IHIhih',body[offset:])
                            assert (actual_oid,width,modifier,fmt)==(oid,-1,-1,0)
                            portal.append(['metadata',actual_oid,width,modifier,fmt])
                        elif tag==b'D':
                            assert body==struct.pack('!HI',1,2)+b'\\x'
                            portal.append(['empty','\\x'])
                        elif tag==b'E':
                            raise AssertionError((side,oid,body))
                        if tag==b'Z':break
                    assert len(portal)==2
                    collected[f'portal-{oid}']=portal
                    for i,(payload,expected_state) in enumerate([(b'a\x00z','22021'),(b'\xff','22021'),(b'\xc3','22021'),(b'\\x0','22023'),(b'\\xgg','22023'),(b'\\400','22P02'),(b'\\q','22P02')]):
                        parse=b'\x00SELECT $1 AS n\x00'+struct.pack('!HI',1,oid)
                        bind=b'\x00\x00'+struct.pack('!HHI',0,1,len(payload))+payload+struct.pack('!H',0)
                        raw.sendall(frame(b'P',parse)+frame(b'B',bind)+frame(b'E',b'\x00'+struct.pack('!I',0))+frame(b'S',b''))
                        tags=[]
                        error_state=None
                        while True:
                            tag=exact(1)
                            body=exact(struct.unpack('!I',exact(4))[0]-4)
                            tags.append(tag.decode())
                            if tag==b'E':
                                error_state=next(x[1:].decode() for x in body.split(b'\x00') if x.startswith(b'C'))
                            if tag==b'Z':
                                assert body==b'I'
                                break
                        observed={'tags':tags,'sqlstate':error_state,'phase':'bind'}
                        collected[f'raw-{oid}-{i}']=observed
                        assert tags==['1','E','Z'] and error_state==expected_state,(side,oid,i,observed)
                        recovered=observe(execute(raw_conn,b'SELECT 1 AS n'),'query')
                        assert recovered['rows']==[['1']]
                        collected[f'raw-sync-{oid}-{i}']=recovered
            finally:
                raw.setblocking(original_blocking)
                raw.close()
                finish(raw_conn)
            q('begin','BEGIN',command=True)
            result=observe(params(conn,f'INSERT INTO {table} VALUES (99,$1)'.encode(),1,(c.c_uint*1)(17),(text*1)(b'\\xgg'),None,None,0),'bind')
            collected['transaction-error']=result
            assert result['status']==7 and result['sqlstate']=='22023'
            assert transaction(conn)==3
            q('failed-transaction','SELECT 1',state='25P02')
            q('rollback','ROLLBACK',command=True)
            assert transaction(conn)==0
            assert q('rollback-recovery',f'SELECT b FROM {table} ORDER BY id')['rows']==read['rows']
            if side=='aster':
                print('PG_BYTEA28_MID',flush=True)
                assert sys.stdin.readline().strip()=='continue'
            observations[side]=collected
        finally:
            (evidence/f'{side}.json').write_text(json.dumps(collected,sort_keys=True))
            try:
                clear(execute(conn,b'ROLLBACK'))
                result=observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()),'cleanup')
                cleanup[side]=result['status']==1
            finally:
                finish(conn)
                (evidence/'cleanup.json').write_text(json.dumps(cleanup))
                (evidence/'resources.json').write_text(json.dumps(resources))
    assert observations['aster']==observations['reference'], next(((k,observations['aster'].get(k),v) for k,v in observations['reference'].items() if observations['aster'].get(k)!=v),'different cases')
    for field,replacement in [('rows',[['perturbed']]),('metadata',[('n',999,2,-1)]),('sqlstate','XXXXX')]:
        changed=copy.deepcopy(observations['aster'])
        changed['direct-0'][field]=replacement
        assert changed!=observations['reference'],field
    assert all(cleanup.values()) and len(cleanup)==2
    summary={'cases':len(observations['aster']),'negative_controls':3,'cleanup':cleanup,'target_version':'18.6'}
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('bytea28 differential:',json.dumps(summary),flush=True)



def text24_differential(aster_dsn):
    """Float family only: collect each endpoint independently before comparison."""
    import copy
    import hashlib
    import json
    import signal
    import time
    import resource
    from pathlib import Path
    prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api('PQdescribePrepared', ptr, ptr, text)
    prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api('PQfsize', integer, ptr, integer)
    mod = api('PQfmod', integer, ptr, integer)
    name = api('PQfname', text, ptr, integer)
    np = api('PQnparams', integer, ptr)
    pt = api('PQparamtype', c.c_uint, ptr, integer)
    ef = api('PQresultErrorField', text, ptr, integer)
    transaction = api('PQtransactionStatus', integer, ptr)
    evidence = Path(os.environ['PG_TEXT24_EVIDENCE_DIR'])
    reference = os.environ['PG18_REFERENCE_DSN']
    table = f'public.pg_text24_{os.getpid()}'
    observations = {}
    cleanup = {}
    resources = {}
    def interrupted(signum, frame):
        raise KeyboardInterrupt('text24 client interrupted')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    def compact(item):
        if isinstance(item,str) and len(item)>256:
            return {'length':len(item),'sha256':hashlib.sha256(item.encode()).hexdigest()}
        return item
    def observe(result, phase):
        assert result
        try:
            state = ef(result, ord('C'))
            if state == b'XX000':
                print('text24 internal error:',result_error(result).decode(),flush=True)
            return dict(phase=phase, status=result_status(result),
                        sqlstate=state.decode() if state else None,
                        metadata=[(name(result,i).decode(),field_type(result,i),size(result,i),mod(result,i)) for i in range(columns(result))],
                        parameters=[pt(result,i) for i in range(np(result))],
                        rows=[[None if is_null(result,r,i) else compact(value(result,r,i).decode()) for i in range(columns(result))] for r in range(rows(result))])
        finally:
            clear(result)
    for side, dsn in [('reference', reference), ('aster', aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn)==0, f'{side} connection failed'
        collected = {}
        resources[side] = {}
        def q(case, sql, state=None, command=False):
            started = time.perf_counter()
            result = observe(execute(conn,sql.encode()), 'query')
            if len(sql)>1000 or case.startswith('max-'):
                resources[side][case] = {'elapsed_seconds':time.perf_counter()-started,'input_bytes':len(sql.encode()),'python_maxrss':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'maxrss_unit':'bytes on macOS, KiB on Linux'}
            result['input'] = compact(sql)
            collected[case] = result
            assert result['status']==(7 if state else 1 if command else 2), (side,case,result)
            assert result['sqlstate']==state, (side,case,result)
            return result
        try:
            if side=='reference':
                identity=q('identity', "SELECT version(), current_setting('server_version_num')")
                assert identity['rows'][0][1]=='180006', 'exact PostgreSQL 18.6 required'
                (evidence/'reference-identity.json').write_text(json.dumps(collected.pop('identity')))
                build_path=Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE'])
                build=json.loads(build_path.read_text())
                assert hashlib.sha256(Path(build['artifact_path']).read_bytes()).hexdigest()==build['artifact_sha256']
                (evidence/'reference-build.json').write_text(json.dumps(build))
                (evidence/'client.json').write_text(json.dumps({'version':version(),'sha256':hashlib.sha256(Path(library).read_bytes()).hexdigest()}))
            q('create',f'CREATE TABLE {table} (id INT PRIMARY KEY, t TEXT, v VARCHAR(2), c CHAR(2))',command=True)
            empty=q('empty',f'SELECT t,v,c FROM {table}')
            assert empty['metadata']==[('t',25,-1,-1),('v',1043,-1,6),('c',1042,-1,6)]
            q('write',f"INSERT INTO {table} VALUES (1,'é中','é中','é'),(2,'','',''),(3,NULL,NULL,NULL)",command=True)
            read=q('read',f'SELECT t,v,c FROM {table} ORDER BY id')
            assert read['rows']==[['é中','é中','é '],['','','  '],[None,None,None]]
            for typ,oid,modifier in [('text',25,-1),('varchar',1043,-1),('bpchar',1042,-1),('varchar(2)',1043,6),('char(2)',1042,6),('varchar(1)',1043,5),('char',1042,5)]:
                for declared in (0,oid):
                    statement=f't{oid}_{modifier}_{declared}'.encode()
                    collected[f'prepare-{statement!r}']=observe(prepare(conn,statement,f'SELECT CAST($1 AS {typ}) AS n'.encode(),1,(c.c_uint*1)(declared) if declared else None),'prepare')
                    desc=observe(describe(conn,statement),'describe')
                    collected[f'describe-{statement!r}']=desc
                    assert desc['parameters']==[oid] and desc['metadata']==[('n',oid,-1,modifier)],(side,desc)
                    for i,item in enumerate(['','a','é中','éx','abc','a  ',None]):
                        case=f'param-{statement!r}-{i}'
                        result=observe(prepared(conn,statement,1,(text*1)(None if item is None else item.encode()),None,None,0),'execute')
                        collected[case]=result
                        assert result['status']==2 and result['metadata']==desc['metadata'],(side,case,result)
            for i,sql in enumerate(["SELECT CAST('éx' AS varchar(2)) AS n", "SELECT CAST('é' AS char(2)) AS n", "SELECT CAST(CAST('a' AS char(3)) AS text) AS n", "SELECT CAST(CAST('a' AS char(3)) AS varchar) AS n", "SELECT CAST(NULL AS text) AS n,CAST(NULL AS varchar(2)) AS v,CAST(NULL AS char(2)) AS c"]):
                q(f'literal-{i}',sql)
            # libpq text APIs use C strings; send explicit-length text Bind on
            # its idle socket so embedded NUL cannot be silently truncated.
            import socket, struct
            pq_socket=api('PQsocket',integer,ptr)
            raw_conn=connect(dsn.encode())
            assert raw_conn and status(raw_conn)==0
            raw=socket.socket(fileno=os.dup(pq_socket(raw_conn)))
            original_blocking=os.get_blocking(pq_socket(raw_conn))
            raw.settimeout(10)
            def frame(tag,payload):
                return tag+struct.pack('!I',len(payload)+4)+payload
            def exact(n):
                data=b''
                while len(data)<n:
                    part=raw.recv(n-len(data))
                    assert part
                    data+=part
                return data
            try:
                for oid in [25,1043,1042]:
                    parse=b'\x00SELECT $1 AS n\x00'+struct.pack('!HI',1,oid)
                    bind=b'\x00\x00'+struct.pack('!HHI',0,1,0)+struct.pack('!H',0)
                    raw.sendall(frame(b'P',parse)+frame(b'B',bind)+frame(b'D',b'P\x00')+frame(b'E',b'\x00'+struct.pack('!I',0))+frame(b'S',b''))
                    portal=[]
                    while True:
                        tag=exact(1)
                        body=exact(struct.unpack('!I',exact(4))[0]-4)
                        if tag==b'T':
                            assert struct.unpack('!H',body[:2])[0]==1
                            offset=body.index(b'\x00',2)+1
                            _,_,actual_oid,width,modifier,fmt=struct.unpack('!IHIhih',body[offset:])
                            assert (actual_oid,width,modifier,fmt)==(oid,-1,-1,0)
                            portal.append(['metadata',actual_oid,width,modifier,fmt])
                        elif tag==b'D':
                            assert body==struct.pack('!HI',1,0)
                            portal.append(['empty',''])
                        elif tag==b'E':
                            raise AssertionError((side,oid,body))
                        if tag==b'Z':break
                    assert len(portal)==2
                    collected[f'portal-{oid}']=portal
                    for i,payload in enumerate([b'a\x00z',b'\xff',b'\xc3']):
                        parse=b'\x00SELECT $1 AS n\x00'+struct.pack('!HI',1,oid)
                        bind=b'\x00\x00'+struct.pack('!HHI',0,1,len(payload))+payload+struct.pack('!H',0)
                        raw.sendall(frame(b'P',parse)+frame(b'B',bind)+frame(b'E',b'\x00'+struct.pack('!I',0))+frame(b'S',b''))
                        tags=[]
                        error_state=None
                        while True:
                            tag=exact(1)
                            body=exact(struct.unpack('!I',exact(4))[0]-4)
                            tags.append(tag.decode())
                            if tag==b'E':
                                error_state=next(x[1:].decode() for x in body.split(b'\x00') if x.startswith(b'C'))
                            if tag==b'Z':
                                assert body==b'I'
                                break
                        observed={'tags':tags,'sqlstate':error_state,'phase':'bind'}
                        collected[f'raw-{oid}-{i}']=observed
                        assert tags==['1','E','Z'] and error_state=='22021',(side,oid,i,observed)
                        recovered=observe(execute(raw_conn,b'SELECT 1 AS n'),'query')
                        assert recovered['rows']==[['1']]
                        collected[f'raw-sync-{oid}-{i}']=recovered
            finally:
                raw.setblocking(original_blocking)
                raw.close()
                finish(raw_conn)
            for typ in ['varchar','char']:
                maximum=q('max-declaration-'+typ,f"SELECT CAST('é' AS {typ}(10485760)) AS n")
                expected='é' if typ=='varchar' else 'é'+' '*10485759
                assert maximum['rows']==[[compact(expected)]]
                assert maximum['metadata']==[('n',1043 if typ=='varchar' else 1042,-1,10485764)]
            # The frontend currently caps messages at 1MiB. Verify a substantial
            # input within that transport domain; max char above emits 10MiB.
            boundary='a'*65536
            statement=b'max_text24'
            collected['max-prepare']=observe(prepare(conn,statement,b'SELECT CAST($1 AS varchar(10485760)) AS n',1,(c.c_uint*1)(25)),'prepare')
            started=time.perf_counter()
            collected['max-varchar-input']=observe(prepared(conn,statement,1,(text*1)(boundary.encode()),None,None,0),'execute')
            assert collected['max-varchar-input']['status']==2
            assert collected['max-varchar-input']['rows']==[[compact(boundary)]]
            resources[side]['max-varchar-input']={'elapsed_seconds':time.perf_counter()-started,'input_bytes':len(boundary),'python_maxrss':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'maxrss_unit':'bytes on macOS, KiB on Linux'}
            for col in ['v','c']:
                q('spaces-'+col,f"INSERT INTO {table} (id,{col}) VALUES (4,'ab  ')",command=True)
                q('spaces-read-'+col,f'SELECT {col} FROM {table} WHERE id=4')
                q('spaces-delete-'+col,f'DELETE FROM {table} WHERE id=4',command=True)
                q('reject-'+col,f"INSERT INTO {table} (id,{col}) VALUES (4,'abc')",state='22001')
                assert q('no-write-'+col,f'SELECT t,v,c FROM {table} ORDER BY id')['rows']==read['rows']
            q('begin','BEGIN',command=True)
            q('transaction-error',f"INSERT INTO {table} (id,v) VALUES (4,'abc')",state='22001')
            assert transaction(conn)==3
            q('failed-transaction','SELECT 1',state='25P02')
            q('rollback','ROLLBACK',command=True)
            assert transaction(conn)==0
            q('rollback-recovery',f'SELECT t,v,c FROM {table} ORDER BY id')
            if side=='aster':
                print('PG_TEXT24_MID',flush=True)
                assert sys.stdin.readline().strip()=='continue'
            observations[side]=collected
        finally:
            (evidence/f'{side}.json').write_text(json.dumps(collected,sort_keys=True))
            try:
                clear(execute(conn,b'ROLLBACK'))
                result=observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()),'cleanup')
                cleanup[side]=result['status']==1
            finally:
                finish(conn)
                (evidence/'cleanup.json').write_text(json.dumps(cleanup))
                (evidence/'resources.json').write_text(json.dumps(resources))
    assert observations['aster']==observations['reference'], next(((k,observations['aster'].get(k),v) for k,v in observations['reference'].items() if observations['aster'].get(k)!=v),'different cases')
    for field,replacement in [('rows',[['perturbed']]),('metadata',[('n',999,2,-1)]),('sqlstate','XXXXX')]:
        changed=copy.deepcopy(observations['aster'])
        changed['literal-0'][field]=replacement
        assert changed!=observations['reference'],field
    assert all(cleanup.values()) and len(cleanup)==2
    summary={'cases':len(observations['aster']),'negative_controls':3,'cleanup':cleanup,'target_version':'18.6'}
    (evidence/'summary.json').write_text(json.dumps(summary))
    print('text24 differential:',json.dumps(summary),flush=True)


def boolean_differential(aster_dsn):
    """Compare independently collected text bool observations; never infer a side."""
    import copy
    import json
    prepare = api("PQprepare", ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
    describe = api("PQdescribePrepared", ptr, ptr, text)
    prepared = api("PQexecPrepared", ptr, ptr, text, integer, c.POINTER(text),
                   c.POINTER(integer), c.POINTER(integer), integer)
    size = api("PQfsize", integer, ptr, integer)
    mod = api("PQfmod", integer, ptr, integer)
    name = api("PQfname", text, ptr, integer)
    np = api("PQnparams", integer, ptr)
    pt = api("PQparamtype", c.c_uint, ptr, integer)
    ef = api("PQresultErrorField", text, ptr, integer)
    observations = {}
    table = f"public.pg_bool_{os.getpid()}"
    reference = os.environ.get("PG18_REFERENCE_DSN")
    assert reference, "PG18_REFERENCE_DSN is required (no skip)"
    def observe(result, phase):
        assert result
        try:
            state = ef(result, ord('C'))
            return dict(phase=phase, status=result_status(result),
                        sqlstate=state.decode() if state else None,
                        metadata=[(name(result,i).decode(),field_type(result,i),size(result,i),mod(result,i)) for i in range(columns(result))],
                        parameters=[pt(result,i) for i in range(np(result))],
                        rows=[[None if is_null(result,r,i) else value(result,r,i).decode() for i in range(columns(result))] for r in range(rows(result))])
        finally:
            clear(result)
    for side, dsn in [('reference',reference),('aster',aster_dsn)]:
        conn = connect(dsn.encode())
        assert conn and status(conn)==0, f"{side} connection failed"
        collected = {}
        def q(case,sql):
            result=observe(execute(conn,sql.encode()),'query')
            result['input']=sql
            collected[case]=result
            return result
        try:
            if side=='reference':
                identity=q('identity',"SELECT version(), current_setting('server_version_num')")
                print('reference identity:',json.dumps(identity),flush=True)
                assert int(identity['rows'][0][1])==int(os.environ.get('PG_BOOLEAN_REFERENCE_VERSION','180006')), 'exact reference version mismatch'
                reference_identity=collected.pop('identity')
                from pathlib import Path
                evidence=Path(os.environ['PG_BOOLEAN_EVIDENCE_DIR'])
                evidence.mkdir(parents=True,exist_ok=True)
                (evidence/'reference-identity.json').write_text(json.dumps(reference_identity))
            assert q('create',f'CREATE TABLE {table} (id INT PRIMARY KEY, b BOOLEAN)')['status']==1
            q('literals','SELECT TRUE AS b, FALSE AS f, CAST(NULL AS BOOLEAN) AS n')
            q('empty',f'SELECT b FROM {table}')
            q('write',f"INSERT INTO {table} VALUES (1, TRUE), (2, FALSE), (3, NULL)")
            q('read',f'SELECT b FROM {table} ORDER BY id')
            if side=='aster':
                print('PG_BOOLEAN_MID',flush=True)
                assert sys.stdin.readline().strip()=='continue'
            for declared in (0,16):
                stmt=f'b{declared}'.encode()
                oid=(c.c_uint*1)(declared) if declared else None
                collected[f'prepare-{declared}']=observe(prepare(conn,stmt,b'SELECT CAST($1 AS BOOLEAN) AS b',1,oid),'prepare')
                collected[f'describe-{declared}']=observe(describe(conn,stmt),'describe')
                vocabulary=['true','false','yes','no','on','off','1','0','t','tr','tru','f','fa','fal','fals','y','ye','n','of','  TrUe \t',None,'','o','2','truth','unknown']
                for i,item in enumerate(vocabulary):
                    args=(text*1)(None if item is None else item.encode())
                    result=observe(prepared(conn,stmt,1,args,None,None,0),'execute')
                    result['input']={'value':item,'oid':declared}
                    collected[f'param-{declared}-{i}']=result
                    assert result['status'] in (2,7)
                    if item in ('','o','2','truth','unknown'): assert result['sqlstate']=='22P02', (side,i,result)
                    elif result['status']!=2: raise AssertionError((side,i,result))
                q(f'recovery-{declared}','SELECT TRUE AS b')
            for i,sql in enumerate(["SELECT CAST(2 AS BOOLEAN) AS b", "SELECT CAST(TRUE AS INT) AS b", "SELECT CAST(TRUE AS TEXT) AS b", "SELECT CAST(NULL AS TEXT) AS b", "SELECT TRUE AS b WHERE 1", f'INSERT INTO {table} VALUES (4, 2)']):
                q(f'conversion-{i}',sql)
            q('begin','BEGIN')
            q('transaction-error',"SELECT CAST('o' AS BOOLEAN) AS b")
            q('rollback','ROLLBACK')
            q('recovered','SELECT TRUE AS b')
            q('no-write',f'SELECT b FROM {table} ORDER BY id')
            observations[side]=collected
        finally:
            from pathlib import Path
            evidence = Path(os.environ['PG_BOOLEAN_EVIDENCE_DIR'])
            evidence.mkdir(parents=True, exist_ok=True)
            (evidence / f'{side}.json').write_text(json.dumps(collected,sort_keys=True))
            try:
                clear(execute(conn,b'ROLLBACK'))
                cleanup=observe(execute(conn,f'DROP TABLE IF EXISTS {table}'.encode()),'cleanup')
                assert cleanup['status']==1, (side,'cleanup failed',cleanup)
            finally:
                finish(conn)
    assert observations['aster']==observations['reference'], next(((k,observations['aster'][k],v) for k,v in observations['reference'].items() if observations['aster'][k]!=v), 'different cases')
    for field,replacement in [('rows',[['perturbed']]),('metadata',[('b',23,4,-1)]),('sqlstate','XXXXX')]:
        changed=copy.deepcopy(observations['reference'])
        changed['literals'][field]=replacement
        assert observations['aster']!=changed, field
    print(f"boolean differential: {len(observations['aster'])} cases, 3 comparator perturbations passed",flush=True)


conninfo = (
    f"host=127.0.0.1 port={int(sys.argv[1])} user=root dbname={sys.argv[2]} "
    "sslmode=disable gssencmode=disable connect_timeout=5"
)
if len(sys.argv) > 3 and sys.argv[3] == "bytea28-differential":
    bytea28_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "interval-differential":
    interval_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "timestamptz-differential":
    timestamptz_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "timestamp-differential":
    timestamp_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "time-differential":
    time_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "date-differential":
    date_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "text24-differential":
    text24_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "float20-differential":
    float20_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "numeric15-differential":
    numeric15_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "integer14-differential":
    integer14_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "boolean-differential":
    boolean_differential(conninfo)
    sys.exit(0)
if len(sys.argv) > 3 and sys.argv[3] == "integer-differential":
    integer_differential(conninfo)
    sys.exit(0)

conn = connect(conninfo.encode())
assert conn
try:
    assert status(conn) == 0, error(conn).decode()
    print("protocol:", protocol(conn), "server build:",
          parameter_status(conn, b"server_version").decode(), flush=True)
    assert protocol(conn) == 30000

    def query(sql, arguments=None, oids=None, expected=None, types=None,
              affected=None):
        if arguments is None:
            result = execute(conn, sql.encode())
        else:
            assert len(arguments) == len(oids)
            payloads = [None if arg is None else str(arg).encode() for arg in arguments]
            result = params(
                conn, sql.encode(), len(arguments),
                (c.c_uint * len(oids))(*oids),
                (text * len(payloads))(*payloads), None, None, 0,
            )
        assert result, sql
        try:
            assert result_status(result) == (2 if expected is not None else 1), (
                sql, result_error(result).decode())
            if affected is not None:
                assert command_rows(result) == str(affected).encode(), sql
            if types is not None:
                actual_types = [field_type(result, col) for col in range(columns(result))]
                assert actual_types == types, (sql, actual_types, types)
            actual = [
                [None if is_null(result, row, col) else value(result, row, col).decode()
                 for col in range(columns(result))]
                for row in range(rows(result))
            ]
            if expected is not None:
                assert actual == expected, (sql, actual, expected)
            return actual
        finally:
            clear(result)

    table = f"public.pg_core_write_{os.getpid()}"
    query(f"CREATE TABLE {table} (id INT PRIMARY KEY, n INT, note TEXT)")
    try:
        query(f"SELECT id, n, note FROM {table}", expected=[], types=[23, 23, 25])
        cases = [(17, -41, "first write"), (29, None, ""), (43, 2147483647, None)]
        for key, number, note in cases:
            query(f"INSERT INTO {table} (id, n, note) VALUES ($1, $2, $3)",
                  arguments=[key, number, note], oids=[23, 23, 25], affected=1)
            # A fresh SELECT must see the actual write, including NULL vs empty.
            query(f"SELECT id, n, note FROM {table} WHERE id = $1",
                  arguments=[key], oids=[23],
                  expected=[[str(key), None if number is None else str(number), note]],
                  types=[23, 23, 25])
        query(f"SELECT id, n, note FROM {table} ORDER BY id",
              expected=[["17", "-41", "first write"], ["29", None, ""],
                        ["43", "2147483647", None]], types=[23, 23, 25])
        query(f"SELECT id, n, note FROM {table} WHERE id = $1",
              arguments=[999], oids=[23], expected=[], types=[23, 23, 25])
        print("effective scenarios: 3 parameterized INSERTs + primary-key readbacks; "
              "int4 OID 23, NULL/empty string and full rows passed", flush=True)
    finally:
        query(f"DROP TABLE {table}")
    print("typed empty readback, table cleanup and connection usability passed", flush=True)
finally:
    finish(conn)
