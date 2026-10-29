# Copyright 2026 AsterSQL.
"""COALESCE/NULLIF strict differential using import-safe native client bindings.

Default: PORT DATABASE EVIDENCE MYSQL_PORT. --self-test is offline;
--reference-only EVIDENCE records actual reference versions without approval.
PG18_REFERENCE_DSN/PG_LIBPQ_LIBRARY and MYSQL8_REFERENCE_HOST/PORT/USER/PASSWORD/
MYSQL8_CLIENT_LIBRARY follow task23. Services belong to the caller. Credentials
are never archived. MySQL8 is a separately identified reference, not a TiDB peer.
"""
import copy
import ctypes as c
from decimal import Decimal
import hashlib
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys

from pg_function_23_libpq import Libpq, Wire, compare, compare_wire, matrix_self_test, payload
from pg_function_24_libpq import Mysql

# Fixed IDs/order; each successful constant is also tested with zero rows.
PG_CASES = (
    ('c-value', 'coalesce(NULL::int4,7::int4)', 23, '7'),
    ('c-all-null', 'coalesce(NULL::int4,NULL::int4)', 23, None),
    ('c-unknown-null', 'coalesce(NULL,NULL)', 25, None),
    ('c-promote-first', 'coalesce(7::int4,2.50::numeric)', 1700, '7'),
    ('c-promote-second', 'coalesce(NULL::int4,2.50::numeric)', 1700, '2.50'),
    ('c-numeric-first', 'coalesce(NULL::numeric,7::int4)', 1700, '7'),
    ('c-promote-null', 'coalesce(NULL::int4,NULL::numeric)', 1700, None),
    ('c-boundary', 'coalesce((-2147483648)::int4,2147483647::int4)', 23, '-2147483648'),
    ('n-equal', 'nullif(1::int4,1::int4)', 23, None),
    ('n-unequal', 'nullif(1::int4,2::int4)', 23, '1'),
    ('n-left-null', 'nullif(NULL::int4,2::int4)', 23, None),
    ('n-right-null', 'nullif(1::int4,NULL::int4)', 23, '1'),
    ('n-all-null', 'nullif(NULL::int4,NULL::int4)', 23, None),
    ('n-promote-unequal', 'nullif(1::int4,2.2::numeric)', 1700, '1'),
    ('n-promote-equal', 'nullif(1::int4,1.00::numeric)', 1700, None),
    ('n-boundary', 'nullif((-2147483648)::int4,2147483647::int4)', 23, '-2147483648'),
    ('c-C', 'coalesce(NULL::text,\'A\'::text COLLATE "C")', 25, 'A'),
    ('n-C-case', 'nullif(\'A\'::text COLLATE "C",\'a\'::text)', 25, 'A'),
    ('n-C-equal', 'nullif(\'A\'::text COLLATE "C",\'A\'::text)', 25, None),
    ('n-C-numeric-text', 'nullif(\'01\'::text COLLATE "C",\'1\'::text)', 25, '01'),
    ('c-POSIX', 'coalesce(NULL::text,\'A\'::text COLLATE "POSIX")', 25, 'A'),
)
PG_REJECT = (
    ('c-type-conflict', "coalesce(7::int4,'x'::text)", '42804'),
    ('n-type-conflict', "nullif(7::int4,'x'::text)", '42883'),
    ('c-collation-conflict', 'coalesce(\'a\'::text COLLATE "C",\'a\'::text COLLATE "POSIX")', '42P21'),
    ('n-collation-conflict', 'nullif(\'a\'::text COLLATE "C",\'a\'::text COLLATE "POSIX")', '42P21'),
)
# id/expression/parameter OID/result OID/rebindings/expected values.
EXTENDED = (
    ('c-int', 'coalesce($1::int4,NULL::int4)', 23, 23, (-1, None, 2, -2147483648), ('-1', None, '2', '-2147483648')),
    ('c-promote', 'coalesce($1::int4,NULL::numeric)', 23, 1700, (-1, None, 2), ('-1', None, '2')),
    ('n-int', 'nullif($1::int4,2::int4)', 23, 23, (-1, None, 2, -2147483648), ('-1', None, None, '-2147483648')),
    ('n-promote', 'nullif($1::int4,2.2::numeric)', 23, 1700, (-1, None, 2), ('-1', None, '2')),
    ('c-text', 'coalesce($1::text,\'A\'::text COLLATE "C")', 25, 25, ('a', None, 'A', ''), ('a', 'A', 'A', '')),
    ('n-text', 'nullif($1::text COLLATE "C",\'A\'::text)', 25, 25, ('a', None, 'A', ''), ('a', None, None, '')),
)
MYSQL_CASES = (
    ('c-value', 'coalesce(CAST(NULL AS SIGNED),CAST(7 AS SIGNED))'),
    ('c-null', 'coalesce(CAST(NULL AS SIGNED),CAST(NULL AS SIGNED))'),
    ('c-promote', 'coalesce(CAST(7 AS SIGNED),2.50)'),
    ('c-fallback', 'coalesce(CAST(NULL AS SIGNED),2.50)'),
    ('n-equal', 'nullif(CAST(1 AS SIGNED),CAST(1 AS SIGNED))'),
    ('n-unequal', 'nullif(CAST(1 AS SIGNED),CAST(2 AS SIGNED))'),
    ('n-left-null', 'nullif(CAST(NULL AS SIGNED),CAST(2 AS SIGNED))'),
    ('n-right-null', 'nullif(CAST(1 AS SIGNED),CAST(NULL AS SIGNED))'),
    ('n-all-null', 'nullif(CAST(NULL AS SIGNED),CAST(NULL AS SIGNED))'),
    ('n-promote', 'nullif(CAST(1 AS SIGNED),2.2)'),
    ('n-promote-equal', 'nullif(CAST(1 AS SIGNED),1.00)'),
    ('c-boundary', 'coalesce(CAST(-2147483648 AS SIGNED),CAST(7 AS SIGNED))'),
    ('n-boundary', 'nullif(CAST(-2147483648 AS SIGNED),CAST(7 AS SIGNED))'),
    ('c-text', "coalesce(CAST(NULL AS CHAR),'A' COLLATE utf8mb4_bin)"),
    ('n-text-case', "nullif('A' COLLATE utf8mb4_bin,'a')"),
    ('n-text-equal', "nullif('A' COLLATE utf8mb4_bin,'A')"),
    ('n-text-numeric', "nullif('01' COLLATE utf8mb4_bin,'1')"),
    ('c-short', 'coalesce(7,10/0)'),
    ('c-required', 'coalesce(NULL,10/0)'),
    ('n-required', 'nullif(7,10/0)'),
)
MYSQL_EXTENDED = (
    ('c-int', 'coalesce(CAST(? AS SIGNED),CAST(NULL AS SIGNED))', (b'-1', None, b'2', b'-2147483648')),
    ('c-promote', 'coalesce(CAST(? AS SIGNED),2.50)', (b'-1', None, b'2')),
    ('n-int', 'nullif(CAST(? AS SIGNED),CAST(2 AS SIGNED))', (b'-1', None, b'2')),
    ('n-promote', 'nullif(CAST(? AS SIGNED),2.2)', (b'-1', None, b'2')),
    ('c-text', "coalesce(CAST(? AS CHAR),'A' COLLATE utf8mb4_bin)", (b'a', None, b'A', b'')),
    ('n-text', "nullif(CAST(? AS CHAR) COLLATE utf8mb4_bin,'A')", (b'a', None, b'A', b'')),
)


def encoded(value, oid, fmt):
    if value is None or oid != 1700 or not fmt:
        return payload(value, oid, fmt)
    # PostgreSQL numeric binary format: base10000 digits, signed weight/scale.
    dec = Decimal(value)
    whole, _, frac = format(abs(dec), 'f').partition('.')
    scale = len(frac)
    left = whole.zfill((len(whole)+3)//4*4)
    right = frac.ljust((len(frac)+3)//4*4, '0')
    digits = [int((left+right)[i:i+4]) for i in range(0, len(left+right), 4)]
    weight = len(left)//4-1
    while digits and digits[0] == 0:
        digits.pop(0); weight -= 1
    while digits and digits[-1] == 0:
        digits.pop()
    if not digits:
        weight = 0
    return struct.pack('!hhHH', len(digits), weight, 0x4000 if dec < 0 else 0, scale) + b''.join(struct.pack('!H', d) for d in digits)


def mysql_compare(left, right):
    differing = [key for key in sorted(set(left) | set(right)) if key not in left or key not in right or left[key] != right[key]]
    assert not differing, 'MySQL fields differ: '+','.join(differing)


def pg_compare(left, right):
    compare(left, right)
    for field in 'SVC':
        assert left['errors'][field] == right['errors'][field], f'PG diagnostic {field}'
    for field in 'MDHP':
        assert bool(left['errors'][field]) == bool(right['errors'][field]), f'PG diagnostic presence {field}'


def self_test():
    count = matrix_self_test()
    original_mysql = dict(rows=[['31']], metadata=[['v', 8, 32896, 63, 22, 0]], state='00000')
    for key in ('rows', 'metadata', 'state'):
        altered = copy.deepcopy(original_mysql)
        altered[key] = [] if key != 'state' else '22012'
        try:
            mysql_compare(original_mysql, altered)
        except AssertionError:
            count += 1
        else:
            raise AssertionError('MySQL field corruption accepted')
    original = dict(status=7, metadata=[], parameters=[], rows=[], state='42P21',
                    errors=dict(S='ERROR', V='ERROR', C='42P21', M='conflict', D='', H='', P='12'))
    for key in ('P', 'C'):
        altered = copy.deepcopy(original)
        altered['errors'][key] = ''
        try:
            pg_compare(original, altered)
        except AssertionError:
            count += 1
        else:
            raise AssertionError('diagnostic corruption accepted')
    assert encoded('7', 1700, 1).hex() == '00010000000000000007'
    assert encoded('-1', 1700, 1).hex() == '00010000400000000001'
    return count


def run(dsns, evidence, aster_mysql=None):
    evidence.mkdir(parents=True, exist_ok=False)
    pq = Libpq()
    conns, wires, mysqls, records, failures = {}, {}, {}, {}, []
    counts = dict(assertions=0, result_scenarios=0, interleaved_probes=0)
    table = f'function25_rows_{os.getpid()}'
    mysql_table = table+'_mysql'
    owned_pg_tables, reference_database, cleanup = [], None, {}

    def check(ok, label):
        counts['assertions'] += 1
        if not ok:
            failures.append(label)

    def archive(domain, side, case, phase, obs):
        peers = records.setdefault((domain, case, phase), {})
        assert side not in peers, 'duplicate observation'
        peers[side] = obs
        with (evidence / f'{domain}-{side}.jsonl').open('a') as stream:
            stream.write(json.dumps(dict(case_id=case, phase=phase, observation=obs))+'\n')
        return obs

    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))

    def probe(label):
        for side, conn in conns.items():
            obs = archive('pg-probe', side, label, 'query', query(conn, 'SELECT 1::int4 AS v'))
            check(obs['status'] == 2 and obs['rows'] == [['31']], f'{side}/{label}: PG recovery')
            counts['interleaved_probes'] += 1
        for side, mysql in mysqls.items():
            obs = archive('mysql-probe', side, label, 'query', mysql.query('SELECT 1 AS v'))
            check(obs['code'] == 0 and obs['rows'] == [['31']], f'{side}/{label}: MySQL recovery')
            counts['interleaved_probes'] += 1

    def metadata(oid, fmt=0):
        return [[oid, 4 if oid == 23 else -1, -1, fmt, 'v', 0, 0]]

    def result(obs, side, label, oid, value, fmt=0, empty=False, error=None):
        counts['result_scenarios'] += 1
        if error:
            check(obs['status'] == 7 and obs['state'] == error, f'{side}/{label}: expected SQLSTATE {error}')
            check(not obs['metadata'] and not obs['rows'], f'{side}/{label}: no false description/payload')
            check(all(obs['errors'][k] for k in 'SVCM'), f'{side}/{label}: diagnostics')
        else:
            raw = encoded(value, oid, fmt)
            rows = [] if empty else [[None if raw is None else raw.hex()]]
            check(obs['status'] == 2 and obs['rows'] == rows, f'{side}/{label}: exact value/NULL/empty')
            check(obs['metadata'] == metadata(oid, fmt), f'{side}/{label}: exact metadata')

    def extended(label, sql, poid, oid, values, expected, empty, error_states=None, row_states=None):
        name = ('function25_'+label).encode()
        prepared = {}
        for side, conn in conns.items():
            obs = archive('pg', side, label, 'parse', pq.observe(pq.PQprepare(conn, name, sql.encode(), 0, None)))
            check(obs['status'] == 1, f'{side}/{label}: prepare')
            desc = archive('pg', side, label, 'statement-describe', pq.observe(pq.PQdescribePrepared(conn, name)))
            check(desc['parameters'] == [poid] and desc['metadata'] == metadata(oid), f'{side}/{label}: signature')
            wire = wires[side]
            parsed = archive('wire', side, label, 'parse', wire.phase(b'P', name+b'\0'+sql.encode()+b'\0\0\0', '1'))
            if parsed['errors']:
                archive('wire', side, label, 'parse-sync', wire.sync())
            else:
                desc = archive('wire', side, label, 'statement-describe', wire.phase(b'D', b'S'+name+b'\0', 'Tn'))
                check(desc['parameters'] == [poid] and desc['metadata'] == metadata(oid), f'{side}/{label}: raw signature')
            prepared[side] = obs['status'] == 1 and not parsed['errors']
        for infmt in (0, 1):
            for outfmt in (0, 1):
                for index, value in enumerate(values):
                    case = f'{label}-{infmt}-{outfmt}-{index}'
                    if row_states:
                        a, z = row_states[index]
                        for side, conn in conns.items():
                            assert query(conn, f'UPDATE {table} SET a={a},z={z}')['status'] == 1
                    data = encoded(value, poid, infmt)
                    state = error_states[index] if error_states and not empty else None
                    for side, conn in conns.items():
                        if not prepared[side]:
                            check(False, f'{side}/{case}: unavailable prepared peer')
                            continue
                        obs = archive('pg', side, case, 'execute', pq.observe(pq.PQexecPrepared(conn, name, 1,
                            (c.c_char_p*1)(data), (c.c_int*1)(0 if data is None else len(data)), (c.c_int*1)(infmt), outfmt)))
                        result(obs, side, case, oid, expected[index], outfmt, empty, state)
                        wire = wires[side]
                        body = b'\0'+name+b'\0'+struct.pack('!HHHi', 1, infmt, 1, -1 if data is None else len(data))+(data or b'')+struct.pack('!HH', 1, outfmt)
                        bind = archive('wire', side, case, 'bind', wire.phase(b'B', body, '2'))
                        check(bind['tags'] == '2', f'{side}/{case}: BindComplete')
                        if bind['errors']:
                            archive('wire', side, case, 'bind-sync', wire.sync())
                            continue
                        desc = archive('wire', side, case, 'portal-describe', wire.phase(b'D', b'P\0', 'Tn'))
                        check(desc['metadata'] == metadata(oid, outfmt), f'{side}/{case}: portal identity')
                        wire.send(b'E', b'\0\0\0\0\0'); wire.send(b'S')
                        obs = wire.read('Z')
                        if obs['errors']:
                            obs['tail'] = wire.read('Z')
                        archive('wire', side, case, 'execute', obs)
                        counts['result_scenarios'] += 1
                        check(not obs['metadata'], f'{side}/{case}: Execute has no description')
                        if state:
                            check(len(obs['errors']) == 1 and obs['errors'][0].get('C') == state and not obs['rows'], f'{side}/{case}: raw error')
                            check(obs.get('tail', {}).get('ready') == 'I', f'{side}/{case}: Sync recovery')
                        else:
                            raw = encoded(expected[index], oid, outfmt)
                            check(obs['tags'] == ('CZ' if empty else 'DCZ') and obs['rows'] == ([] if empty else [[None if raw is None else raw.hex()]]), f'{side}/{case}: raw result')
                            check(obs['lengths'] == ([] if empty else [[-1 if raw is None else len(raw)]]), f'{side}/{case}: raw lengths')
                        recovery = archive('wire', side, case, 'recovery', wire.query('SELECT 1::int4 AS v'))
                        check(recovery['rows'] == [['31']] and recovery['ready'] == 'I', f'{side}/{case}: wire recovery')
                    probe(case)
        for side, conn in conns.items():
            archive('pg', side, label, 'deallocate', query(conn, 'DEALLOCATE '+name.decode()))
            if prepared[side]:
                wire = wires[side]
                archive('wire', side, label, 'close', wire.phase(b'C', b'S'+name+b'\0', '3'))
                archive('wire', side, label, 'close-sync', wire.sync())

    try:
        repo = Path(__file__).resolve().parents[3]
        binary = os.environ.get('PG_FUNCTION25_RUST_BINARY')
        sources = ('pg_function_25_libpq.py', 'pg_function_24_libpq.py', 'pg_function_23_libpq.py')
        (evidence/'build.json').write_text(json.dumps(dict(
            head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
            fixtures={s: hashlib.sha256(Path(__file__).with_name(s).read_bytes()).hexdigest() for s in sources},
            cargo_lock_sha256=hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),
            source_diff_sha256=hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD', '--', 'pkg'], cwd=repo)).hexdigest(),
            rust_binary=binary, rust_binary_sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest() if binary else None)))
        for side, dsn in dsns:
            conn = pq.PQconnectdb(dsn.encode()); assert conn
            conns[side] = conn
            assert pq.PQstatus(conn) == 0, f'{side}: login failed (DSN redacted)'
            identity = {sql: query(conn, sql) for sql in ('SELECT version()', 'SHOW server_version_num', 'SHOW client_encoding', 'SHOW server_encoding', 'SHOW standard_conforming_strings')}
            (evidence/f'pg-{side}-identity.json').write_text(json.dumps(dict(libpq=pq.PQlibVersion(), observations=identity)))
            if side == 'reference' and len(dsns) == 2:
                check(identity['SHOW server_version_num']['rows'] == [['313830303036']], 'PG18.6 unavailable; actual version archived')
            setup = query(conn, f'CREATE TABLE {table}(a integer,z integer)')
            assert setup['status'] == 1, f'{side}: private table setup'
            owned_pg_tables.append(side)
            assert query(conn, f'INSERT INTO {table} VALUES(7,0)')['status'] == 1
            wires[side] = Wire(pq, dsn)
        if aster_mysql:
            mysqls['aster'] = Mysql('127.0.0.1', aster_mysql[0], 'root', '', aster_mysql[1])
        if os.environ.get('MYSQL8_REFERENCE_PORT'):
            mysqls['reference'] = Mysql(os.environ['MYSQL8_REFERENCE_HOST'], int(os.environ['MYSQL8_REFERENCE_PORT']), os.environ['MYSQL8_REFERENCE_USER'], os.environ['MYSQL8_REFERENCE_PASSWORD'])
            reference_database = aster_mysql[1] if aster_mysql else f'function25_ref_{os.getpid()}'
            assert reference_database.replace('_', '').isalnum()
            assert mysqls['reference'].query(f'CREATE DATABASE {reference_database}')['code'] == 0
            assert mysqls['reference'].query(f'USE {reference_database}')['code'] == 0
        else:
            check(False, 'MySQL reference missing')
        for side, mysql in mysqls.items():
            identity = {sql: mysql.query(sql) for sql in ('SELECT version()', 'SELECT @@sql_mode', 'SELECT @@character_set_connection', 'SELECT @@collation_connection')}
            (evidence/f'mysql-{side}-identity.json').write_text(json.dumps(dict(client_version=mysql.mysql_get_client_version(), observations=identity, reference_family=('TiDB' if 'tidb' in ''.join(bytes.fromhex(cell).decode() for row in identity['SELECT version()']['rows'] for cell in row if cell).lower() else 'MySQL') if side == 'reference' else 'AsterSQL')))
            assert mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code'] == 0
            assert mysql.query(f'CREATE TABLE {mysql_table}(a integer,z integer)')['code'] == 0
            assert mysql.query(f'INSERT INTO {mysql_table} VALUES(7,0)')['code'] == 0
        probe('before')
        for case, expression, oid, value in PG_CASES:
            for empty in (False, True):
                label = f'{case}-empty{int(empty)}'
                for side, conn in conns.items():
                    obs = archive('pg', side, label, 'query', query(conn, f'SELECT {expression} AS v'+(' WHERE false' if empty else '')))
                    result(obs, side, label, oid, value, empty=empty)
                probe(label)
        for case, expression, state in PG_REJECT:
            sql = f'SELECT {expression} AS v'
            for side, conn in conns.items():
                obs = archive('pg', side, case, 'query', query(conn, sql))
                result(obs, side, case, None, None, error=state)
                obs = archive('pg', side, case, 'parse', pq.observe(pq.PQprepare(conn, case.encode(), sql.encode(), 0, None)))
                check(obs['status'] == 7 and obs['state'] == state and not obs['metadata'] and not obs['rows'], f'{side}/{case}: rejected Parse')
                wire = wires[side]
                # Queue Describe+Execute after rejection: neither may pretend success.
                wire.send(b'P', case.encode()+b'\0'+sql.encode()+b'\0\0\0')
                wire.send(b'D', b'S'+case.encode()+b'\0'); wire.send(b'E', b'\0\0\0\0\0'); wire.send(b'S')
                obs = wire.read('Z')
                if obs['errors']:
                    obs['tail'] = wire.read('Z')
                archive('wire', side, case, 'rejected-pipeline', obs)
                check(obs['tags'] == 'E' and not obs['rows'] and not obs['metadata'] and obs['errors'][0].get('C') == state, f'{side}/{case}: rejection no success payload')
                check(obs.get('tail', {}).get('ready') == 'I', f'{side}/{case}: rejected pipeline Sync')
            probe(case)
        for case, expression, poid, oid, values, expected in EXTENDED:
            for empty in (False, True):
                extended(f'{case}-empty{int(empty)}', f'SELECT {expression} AS v'+(' WHERE false' if empty else ''), poid, oid, values, expected, empty)
        # Real stored row: COALESCE may skip division; NULLIF must compare it,
        # including when its first argument is NULL. Parse never evaluates rows.
        for first in (7, None):
            for side, conn in conns.items():
                assert query(conn, f'UPDATE {table} SET a={"NULL" if first is None else first},z=0')['status'] == 1
            for function in ('coalesce', 'nullif'):
                label = f'row-{function}-{first}'
                for side, conn in conns.items():
                    state = '22012' if function == 'nullif' or first is None else None
                    obs = archive('pg', side, label, 'query', query(conn, f'SELECT {function}(a,10/z) AS v FROM {table}'))
                    result(obs, side, label, 23, '7', error=state)
                probe(label)
                for empty in (False, True):
                    errors = (None, '22012', None, None) if function == 'nullif' or first is None else (None,)*4
                    expected = ('7', None, '7', '7') if first == 7 else ('5', None, None, '5')
                    if function == 'coalesce' and first == 7:
                        expected = ('7',)*4
                    if function == 'nullif' and first is None:
                        expected = (None,)*4
                    extended(f'{label}-empty{int(empty)}', f'SELECT {function}(a,10/(z+$1::int4)) AS v FROM {table}'+(' WHERE false' if empty else ''), 23, 23, (2, 0, None, 2), expected, empty, errors)
        # Exact row/parameter shapes consumed from tasks12/13, including
        # negative integer truncation, required error and post-error rebinding.
        for function in ('coalesce', 'nullif'):
            rows = [('7', 0 if function == 'coalesce' else 2), ('7', 0),
                    ('NULL' if function == 'coalesce' else '5', 2),
                    ('NULL', 2), ('NULL', 2), ('NULL', 2),
                    ('NULL' if function == 'coalesce' else '7', 0), ('7', 2)]
            expected = ('7', '7', '5', None, '-5', None, None, '7') if function == 'coalesce' else ('7', '7', None, None, None, None, None, '7')
            for empty in (False, True):
                extended(f'original-{function}-empty{int(empty)}',
                         f'SELECT {function}(a,$1::int4/z) AS v FROM {table}'+(' WHERE false' if empty else ''),
                         23, 23, (10, None, 10, None, -11, None, 10, 10), expected, empty,
                         (None, None, None, None, None, None, '22012', None), rows)
        for case, expression in MYSQL_CASES:
            for empty in (False, True):
                label = f'{case}-empty{int(empty)}'
                for side, mysql in mysqls.items():
                    obs = archive('mysql', side, label, 'query', mysql.query(f'SELECT {expression} AS v'+(' WHERE false' if empty else '')))
                    counts['result_scenarios'] += 1
                    check(obs['code'] == 0 and obs['state'] == '00000', f'{side}/{label}: MySQL own success/warning semantics')
                    if empty:
                        check(not obs['rows'], f'{side}/{label}: MySQL zero rows')
                probe('mysql-'+label)
        for case, expression, values in MYSQL_EXTENDED:
            for empty in (False, True):
                label = f'{case}-empty{int(empty)}'
                for side, mysql in mysqls.items():
                    observations = mysql.prepared(f'SELECT {expression} AS v'+(' WHERE false' if empty else ''), values)
                    check(len(observations) == len(values)+1, f'{side}/{label}: MySQL all rebindings')
                    for index, obs in enumerate(observations):
                        archive('mysql', side, label, f'prepared-{index}', obs)
                        check(obs['code'] == 0, f'{side}/{label}/{index}: native prepare/execute')
                        if index:
                            counts['result_scenarios'] += 1
                            if empty:
                                check(not obs['rows'], f'{side}/{label}/{index}: native empty')
                probe('mysql-prepared-'+label)
        for first in (7, None):
            for side, mysql in mysqls.items():
                assert mysql.query(f'UPDATE {mysql_table} SET a={"NULL" if first is None else first},z=0')['code'] == 0
            for function in ('coalesce', 'nullif'):
                label = f'row-{function}-{first}'
                for side, mysql in mysqls.items():
                    obs = archive('mysql', side, label, 'query', mysql.query(f'SELECT {function}(a,10/z) AS v FROM {mysql_table}'))
                    counts['result_scenarios'] += 1
                    check(obs['code'] == 0, f'{side}/{label}: SELECT division warning semantics')
                    for empty in (False, True):
                        observations = mysql.prepared(f'SELECT {function}(a,10/CAST(? AS SIGNED)) AS v FROM {mysql_table}'+(' WHERE false' if empty else ''), (b'2', b'0', None, b'2'))
                        for index, obs in enumerate(observations):
                            archive('mysql', side, label, f'empty{int(empty)}-prepared-{index}', obs)
                            check(obs['code'] == 0, f'{side}/{label}/{empty}/{index}: warning not error')
                            if index:
                                counts['result_scenarios'] += 1
                                if empty:
                                    check(not obs['rows'], f'{side}/{label}/{index}: row WHERE false')
                probe('mysql-'+label)
        probe('after')
        if len(dsns) == 2:
            for (domain, case, phase), peers in records.items():
                if set(peers) != {'reference', 'aster'}:
                    check(False, f'{domain}/{case}/{phase}: missing peer')
                    continue
                comparator = compare_wire if domain == 'wire' else mysql_compare if domain.startswith('mysql') else pg_compare
                counts['assertions'] += 1
                try:
                    comparator(peers['reference'], peers['aster'])
                except AssertionError as error:
                    failures.append(f'{domain}/{case}/{phase}: {error}')
        # Per-domain evidence table: no signature receives blanket approval.
        (evidence/'candidate-domains.json').write_text(json.dumps([
            dict(id=s[0], parameter_oid=s[2], result_oid=s[3], nulls='observed',
                 domain='int4' if s[2] == 23 else 'text C', evaluation='matrix recorded',
                 collation='C/POSIX cases recorded; RowDescription has no collation field',
                 folding_cache='literal/prepared rebinding observed; plan optimization not proven',
                 pushdown='unproven; no authorization', equivalence='pending full evidence')
            for s in EXTENDED], indent=2))
    except BaseException as error:
        failures.append('client aborted: '+type(error).__name__)
        raise
    finally:
        for wire in wires.values():
            wire.close()
        for side in owned_pg_tables:
            cleanup['pg_'+side+'_table_dropped'] = query(conns[side], f'DROP TABLE {table}')['status'] == 1
            check(cleanup['pg_'+side+'_table_dropped'], f'{side}: PG table cleanup')
        for conn in conns.values():
            pq.PQfinish(conn)
        if reference_database:
            cleanup['reference_database_dropped'] = mysqls['reference'].query(f'DROP DATABASE {reference_database}')['code'] == 0
            check(cleanup['reference_database_dropped'], 'reference database cleanup')
        for mysql in mysqls.values():
            mysql.close()
        breakdown = {}
        for (domain, case, phase), peers in records.items():
            is_result = domain in ('pg', 'wire') and phase in ('query', 'execute')
            is_result |= domain == 'mysql' and (phase == 'query' or ('prepared-' in phase and not phase.endswith('-0')))
            if is_result:
                key = domain+('-query' if phase == 'query' else '-execute')
                breakdown[key] = breakdown.get(key, 0)+len(peers)
        check(sum(breakdown.values()) == counts['result_scenarios'], 'result scenario accounting')
        summary = dict(**counts, result_breakdown=breakdown, observation_keys=len(records), negative_controls=self_test(), failures=failures,
                       pg_connections_closed=len(conns)+len(wires), mysql_connections_closed=len(mysqls), cleanup=cleanup,
                       scope='conditional int4/numeric/text domains only; no alias/pushdown approval')
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)
    assert records and counts['result_scenarios'] and not failures, 'conditional differential failed'


if __name__ == '__main__':
    def terminate(signum, frame):
        raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM, terminate)
    if sys.argv[1:] == ['--self-test']:
        print(json.dumps(dict(negative_controls=self_test(), pg_query_cases=len(PG_CASES), pg_rejections=len(PG_REJECT), signatures=len(EXTENDED), mysql_query_cases=len(MYSQL_CASES))))
    elif sys.argv[1] == '--reference-only':
        run([('reference', os.environ['PG18_REFERENCE_DSN'])], Path(sys.argv[2]))
    else:
        port, database, evidence, mysql_port = sys.argv[1:]
        run([('reference', os.environ['PG18_REFERENCE_DSN']), ('aster', f'host=127.0.0.1 port={int(port)} user=root dbname={database} connect_timeout=10')], Path(evidence), (int(mysql_port), database))
