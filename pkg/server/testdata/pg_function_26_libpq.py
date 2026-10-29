# Copyright 2026 AsterSQL.
"""Strict CONCAT/extrema client observations; services belong to the caller.

PORT DATABASE EVIDENCE MYSQL_PORT; --self-test is offline; --reference-only
EVIDENCE records the actual reference versions without equivalence approval.
Uses the import-safe task23/24 native bindings and task25 exact comparison and
numeric encoding helpers. PG18_REFERENCE_DSN/PG_LIBPQ_LIBRARY and
MYSQL8_REFERENCE_HOST/PORT/USER/PASSWORD/MYSQL8_CLIENT_LIBRARY follow task23.
Credentials are never archived. MySQL8 observations do not override Go/TiDB.
"""
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys

from pg_function_23_libpq import Libpq, Wire, compare_wire
from pg_function_24_libpq import Mysql
from pg_function_25_libpq import encoded, mysql_compare, pg_compare, self_test

# Source tasks15-17: immutable IDs/inputs, typed text is never unknown.
PG_CASES = (
    ('c-null', "concat('a',NULL,'b')", 25, 'ab'),
    ('c-all-null', 'concat(NULL,NULL)', 25, ''),
    ('c-mixed', "concat('v=',2::int4)", 25, 'v=2'),
    ('c-C', 'concat(\'Z\'::text COLLATE "C",\'a\'::text)', 25, 'Za'),
    ('c-one', "concat('x')", 25, 'x'),
    ('c-upper', "upper(concat('a',NULL,'b'))", 25, 'AB'),
    ('c-nested', "concat('a',concat(NULL,'b'))", 25, 'ab'),
    ('c-catalog', "pg_catalog.concat('a',NULL,'b')", 25, 'ab'),
    ('c-empty', "concat('','')", 25, ''),
) + tuple(
    (f'{name}-{case}', f'{name}({args})', oid, value)
    for name in ('greatest', 'least')
    for case, args, oid, greatest, least in (
        ('null', 'NULL,7::int4', 23, '7', '7'),
        ('all-null', 'NULL::int4,NULL::int4', 23, None, None),
        ('unknown-null', 'NULL,NULL', 25, None, None),
        ('mixed', '2::int4,10.5::numeric', 1700, '10.5', '2'),
        ('one', '7::int4', 23, '7', '7'),
        ('unknown', "2::int4,'10'", 23, '10', '2'),
        ('boundary', '(-2147483648)::int4,2147483647::int4', 23, '2147483647', '-2147483648'),
        ('numeric', '1.00000000000000000001::numeric,1.00000000000000000002::numeric', 1700, '1.00000000000000000002', '1.00000000000000000001'),
        ('negative', '(-1.00000000000000000001)::numeric,(-1.00000000000000000002)::numeric', 1700, '-1.00000000000000000001', '-1.00000000000000000002'),
        ('C-case', '\'Z\'::text COLLATE "C",\'a\'::text', 25, 'a', 'Z'),
        ('C-numeric', '\'2\'::text COLLATE "C",\'10\'::text', 25, '2', '10'),
        ('C-null', 'NULL::text COLLATE "C",NULL::text', 25, None, None),
        ('C-fallback', 'NULL::text,\'A\'::text COLLATE "C"', 25, 'A', 'A'),
        ('C-A-case', '\'A\'::text COLLATE "C",\'a\'::text', 25, 'a', 'A'),
        ('C-equal', '\'A\'::text COLLATE "C",\'A\'::text', 25, 'A', 'A'),
        ('C-leading', '\'01\'::text COLLATE "C",\'1\'::text', 25, '1', '01'),
        ('POSIX', 'NULL::text,\'A\'::text COLLATE "POSIX"', 25, 'A', 'A'),
    )
    for value in (greatest if name == 'greatest' else least,)
)
PG_REJECT = (
    ('c-arity', 'concat()', '42883'),
    ('c-catalog-arity', 'pg_catalog.concat()', '42883'),
) + tuple((f'{name}-{case}', f'{name}({args})', state)
    for name in ('greatest', 'least')
    for case, args, state in (
        ('variadic', 'VARIADIC ARRAY[1,2]', '42601'),
        ('typed-null-conflict', 'NULL::int4,NULL::text', '42804'),
        ('typed-text', "2::int4,'10'::text", '42804'),
        ('C-conflict', '\'A\'::text COLLATE "C",\'b\'::text COLLATE "POSIX"', '42P21'),
        ('C-null-conflict', 'NULL::text COLLATE "C",\'b\'::text COLLATE "POSIX"', '42P21'),
    )) + (
    ('g-arity', 'greatest()', '42601'),
    ('l-arity', 'least()', '42601'),
    ('g-catalog-ordinary', 'pg_catalog.greatest(1::int4,2::int4)', '42883'),
    ('l-catalog-ordinary', 'pg_catalog.least(1::int4,2::int4)', '42883'),
)
EXTENDED = (
    ('c-text', "concat($1::text,'b')", 25, 25, ('a',None,'A',None,'a',''), ('ab','b','Ab','b','ab','b')),
    ('c-catalog', "pg_catalog.concat($1::text,'b')", 25, 25, ('a',None,'A',None,'a',''), ('ab','b','Ab','b','ab','b')),
) + tuple(
    (f'{name}-{case}', f'{name}({args})', poid, oid, values, greatest if name == 'greatest' else least)
    for name in ('greatest', 'least')
    for case, args, poid, oid, values, greatest, least in (
        ('int', '$1::int4,7::int4', 23, 23, (2,None,10,None,2,-2147483648), ('7','7','10','7','7','7'), ('2','7','7','7','2','-2147483648')),
        ('int-null', '$1::int4,NULL::int4', 23, 23, (2,None,10,None,2), ('2',None,'10',None,'2'), ('2',None,'10',None,'2')),
        ('promote', '$1::int4,10.5::numeric', 23, 1700, (2,None,11), ('10.5','10.5','11'), ('2','10.5','10.5')),
        ('text', '$1::text,\'A\'::text COLLATE "C"', 25, 25, ('a',None,'A',None,'a',''), ('a','A','A','A','a','A'), ('A','A','A','A','A','')),
    )
)
MYSQL_CASES = (
    ('c-null', "concat('a',NULL,'b')"), ('c-all-null', 'concat(NULL,NULL)'),
    ('c-mixed', "concat('v=',2)"), ('c-text', "concat('Z','a')"),
    ('c-one', "concat('x')"), ('c-empty', "concat('','')"),
) + tuple((f'{name}-{case}', f'{name}({args})') for name in ('greatest','least')
    for case,args in (
        ('null','NULL,7'), ('right-null','7,NULL'), ('all-null','NULL,NULL'),
        ('int','2,10'), ('text',"'2','10'"), ('mixed',"2,'10'"),
        ('C-case',"'Z' COLLATE utf8mb4_bin,'a'"),
        ('C-null',"NULL,'A' COLLATE utf8mb4_bin"),
        ('C-equal',"'A' COLLATE utf8mb4_bin,'A'"),
        ('C-leading',"'01' COLLATE utf8mb4_bin,'1'"),
        ('numeric','2,10.5'), ('boundary','-2147483648,2147483647'),
    ))
MYSQL_EXTENDED = (
    ('c-text', "concat(CAST(? AS CHAR),'b')", (b'a',None,b'A',b'')),
) + tuple((f'{name}-{case}', f'{name}({args})', values)
    for name in ('greatest','least') for case,args,values in (
        ('int','CAST(? AS SIGNED),7', (b'2',None,b'10',b'-2147483648',b'2')),
        ('text',"CAST(? AS CHAR) COLLATE utf8mb4_bin,'A'", (b'a',None,b'A',b'')),
        ('null','CAST(? AS SIGNED),NULL', (b'2',None,b'10')),
    ))
MYSQL_REJECT = (('c-arity','concat()'),) + tuple(
    (f'{name}-{count}', f'{name}({args})') for name in ('greatest','least')
    for count,args in ((0,''),(1,'7')))


def run(dsns, evidence, aster_mysql=None):
    evidence.mkdir(parents=True, exist_ok=False)
    pq = Libpq()
    conns, wires, mysqls, records, failures = {}, {}, {}, {}, []
    counts = dict(assertions=0, result_scenarios=0, interleaved_probes=0)
    reference_database, cleanup = None, {}

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

    def extended(label, sql, poid, oid, values, expected, empty):
        name = ('function26_'+label).encode()
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
            prepared[side] = (obs['status'] == 1, not parsed['errors'])
        for infmt in (0, 1):
            for outfmt in (0, 1):
                for index, value in enumerate(values):
                    case = f'{label}-{infmt}-{outfmt}-{index}'
                    data = encoded(value, poid, infmt)
                    state = None
                    for side, conn in conns.items():
                        if not prepared[side][0]:
                            check(False, f'{side}/{case}: unavailable prepared peer')
                            continue
                        obs = archive('pg', side, case, 'execute', pq.observe(pq.PQexecPrepared(conn, name, 1,
                            (c.c_char_p*1)(data), (c.c_int*1)(0 if data is None else len(data)), (c.c_int*1)(infmt), outfmt)))
                        result(obs, side, case, oid, expected[index], outfmt, empty, state)
                        if not prepared[side][1]:
                            check(False, f'{side}/{case}: unavailable raw prepared peer')
                            continue
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
            deallocated = archive('pg', side, label, 'deallocate', query(conn, 'DEALLOCATE '+chr(34)+name.decode().replace(chr(34),chr(34)*2)+chr(34)))
            check(deallocated['status'] == 1, f'{side}/{label}: named statement cleanup')
            if prepared[side][1]:
                wire = wires[side]
                archive('wire', side, label, 'close', wire.phase(b'C', b'S'+name+b'\0', '3'))
                archive('wire', side, label, 'close-sync', wire.sync())

    try:
        (evidence/'scenarios.json').write_text(json.dumps(dict(pg_query=PG_CASES, pg_reject=PG_REJECT, pg_prepared=EXTENDED, mysql_query=MYSQL_CASES, mysql_prepared=[(case, sql, [None if v is None else v.hex() for v in values]) for case, sql, values in MYSQL_EXTENDED], mysql_reject=MYSQL_REJECT), indent=2))
        repo = Path(__file__).resolve().parents[3]
        binary = os.environ.get('PG_FUNCTION26_RUST_BINARY')
        sources = ('pg_function_26_libpq.py', 'pg_function_25_libpq.py', 'pg_function_24_libpq.py', 'pg_function_23_libpq.py')
        (evidence/'build.json').write_text(json.dumps(dict(
            head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
            fixtures={s: hashlib.sha256(Path(__file__).with_name(s).read_bytes()).hexdigest() for s in sources},
            cargo_lock_sha256=hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),
            source_diff_sha256=hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD', '--', 'pkg'], cwd=repo)).hexdigest(),
            rust_binary=binary, rust_binary_sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest() if binary else None,
            reference_build=(json.loads(Path(os.environ['FUNCTION26_REFERENCE_BUILD_FILE']).read_text())
                             if os.environ.get('FUNCTION26_REFERENCE_BUILD_FILE') else None))))
        for side, dsn in dsns:
            conn = pq.PQconnectdb(dsn.encode()); assert conn
            conns[side] = conn
            assert pq.PQstatus(conn) == 0, f'{side}: login failed (DSN redacted)'
            identity = {sql: query(conn, sql) for sql in ('SELECT version()', 'SHOW server_version_num', 'SHOW client_encoding', 'SHOW server_encoding', 'SHOW standard_conforming_strings')}
            (evidence/f'pg-{side}-identity.json').write_text(json.dumps(dict(libpq=pq.PQlibVersion(), observations=identity)))
            if side == 'reference' and len(dsns) == 2:
                check(identity['SHOW server_version_num']['rows'] == [['313830303036']], 'PG18.6 unavailable; actual version archived')
            wires[side] = Wire(pq, dsn)
        if aster_mysql:
            mysqls['aster'] = Mysql('127.0.0.1', aster_mysql[0], 'root', '', aster_mysql[1])
        if os.environ.get('MYSQL8_REFERENCE_PORT'):
            mysqls['reference'] = Mysql(os.environ['MYSQL8_REFERENCE_HOST'], int(os.environ['MYSQL8_REFERENCE_PORT']), os.environ['MYSQL8_REFERENCE_USER'], os.environ['MYSQL8_REFERENCE_PASSWORD'])
            reference_database = aster_mysql[1] if aster_mysql else f'function26_ref_{os.getpid()}'
            assert reference_database.replace('_', '').isalnum()
            assert mysqls['reference'].query(f'CREATE DATABASE {reference_database}')['code'] == 0
            assert mysqls['reference'].query(f'USE {reference_database}')['code'] == 0
        else:
            check(False, 'MySQL reference missing')
        for side, mysql in mysqls.items():
            identity = {sql: mysql.query(sql) for sql in ('SELECT version()', 'SELECT @@sql_mode', 'SELECT @@character_set_connection', 'SELECT @@collation_connection')}
            (evidence/f'mysql-{side}-identity.json').write_text(json.dumps(dict(client_version=mysql.mysql_get_client_version(), observations=identity, reference_family=('TiDB' if 'tidb' in ''.join(bytes.fromhex(cell).decode() for row in identity['SELECT version()']['rows'] for cell in row if cell).lower() else 'MySQL') if side == 'reference' else 'AsterSQL')))
            assert mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code'] == 0
        probe('before')
        for case, expression, oid, value in PG_CASES:
            for empty in (False, True):
                label = f'{case}-empty{int(empty)}'
                for side, conn in conns.items():
                    obs = archive('pg', side, label, 'query', query(conn, f'SELECT {expression} AS v'+(' WHERE false' if empty else '')))
                    result(obs, side, label, oid, value, empty=empty)
                probe(label)
        for base, expression, state in PG_REJECT:
            for empty in (False, True):
                for transaction in (False, True):
                    sql = f'SELECT {expression} AS v'+(' WHERE false' if empty else '')
                    case = f'{base}-empty{int(empty)}-tx{int(transaction)}'
                    for side, conn in conns.items():
                        if transaction:
                            check(query(conn, 'BEGIN')['status'] == 1, f'{side}/{case}: BEGIN')
                            check(wires[side].query('BEGIN')['ready'] == 'T', f'{side}/{case}: raw BEGIN')
                        obs = archive('pg', side, case, 'query', query(conn, sql))
                        result(obs, side, case, None, None, error=state)
                        if transaction:
                            check(query(conn, 'ROLLBACK')['status'] == 1, f'{side}/{case}: Query error ROLLBACK')
                            check(query(conn, 'BEGIN')['status'] == 1, f'{side}/{case}: Parse BEGIN')
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
                        check(obs.get('tail', {}).get('ready') == ('E' if transaction else 'I'), f'{side}/{case}: rejected pipeline Sync')
                        if transaction:
                            check(query(conn, 'ROLLBACK')['status'] == 1, f'{side}/{case}: ROLLBACK')
                            check(wire.query('ROLLBACK')['ready'] == 'I', f'{side}/{case}: raw ROLLBACK')
                        recovery = archive('wire', side, case, 'recovery', wire.query('SELECT 1::int4 AS v'))
                        check(recovery['rows'] == [['31']] and recovery['ready'] == 'I', f'{side}/{case}: rejected pipeline recovery')
                    probe(case)
        for case, expression, poid, oid, values, expected in EXTENDED:
            for empty in (False, True):
                extended(f'{case}-empty{int(empty)}', f'SELECT {expression} AS v'+(' WHERE false' if empty else ''), poid, oid, values, expected, empty)
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
        for case, expression in MYSQL_REJECT:
            for empty in (False, True):
                label = f'{case}-empty{int(empty)}'
                sql = f'SELECT {expression} AS v'+(' WHERE false' if empty else '')
                for side, mysql in mysqls.items():
                    obs = archive('mysql', side, label, 'query', mysql.query(sql))
                    counts['result_scenarios'] += 1
                    check(obs['code'] == 1582 and obs['state'] == '42000' and not obs['metadata'] and not obs['rows'], f'{side}/{label}: arity rejection')
                    observations = mysql.prepared(sql, (b'a',))
                    for index, obs in enumerate(observations):
                        archive('mysql', side, label, f'rejected-prepare-{index}', obs)
                    check(len(observations) == 1 and observations[0]['code'] == 1582 and observations[0]['state'] == '42000' and not observations[0]['metadata'], f'{side}/{label}: no false prepared success')
                probe('mysql-reject-'+label)
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
                 domain={23:'int4', 25:'typed text', 1700:'numeric'}[s[2]], evaluation='matrix recorded',
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
        for conn in conns.values():
            pq.PQfinish(conn)
        if reference_database:
            mysql = mysqls['reference']
            pending = mysql.mysql_store_result(mysql.conn)
            if pending:
                mysql.mysql_free_result(pending)
            cleanup['reference_database_dropped'] = mysql.query(f'DROP DATABASE {reference_database}')['code'] == 0
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
                       scope='CONCAT ordinary function; GREATEST/LEAST special syntax; no CONCAT_WS alias or pushdown approval')
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)
    assert records and counts['result_scenarios'] and not failures, 'concat/extrema differential failed'


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
