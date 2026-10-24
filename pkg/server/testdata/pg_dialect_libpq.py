# Copyright 2026 AsterSQL.
"""Dialect syntax and DML rejection handoff through real libpq; argv: task, local port, database.

The first-phase script is deliberately not imported: it connects at import time.
This fixture compares parse/prepare, and archives Describe separately because
boolean result typing belongs to the scalar-type/metadata plans.
"""
import copy
import ctypes as c
import ctypes.util
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import subprocess
import sys
from urllib.parse import urlsplit

from pg_core_evidence import append_observation, compare_metadata, source_snapshot, validate_provenance

task = int(sys.argv[1])
assert task in (1, 2, 3, 4, 9), 'unregistered dialect task'
evidence = Path(os.environ['PG_DIALECT_EVIDENCE_DIR'])
root = Path(__file__).resolve().parents[3]

requested_library = os.environ.get('PG_LIBPQ_LIBRARY') or (
    '/opt/homebrew/opt/libpq/lib/libpq.dylib' if sys.platform == 'darwin'
    else ctypes.util.find_library('pq'))
assert requested_library, 'libpq18 is required'
pq = c.CDLL(requested_library)
class DlInfo(c.Structure):
    _fields_ = [('filename', c.c_char_p), ('base', c.c_void_p),
                ('symbol', c.c_char_p), ('address', c.c_void_p)]
dladdr = c.CDLL(None).dladdr
dladdr.argtypes, dladdr.restype = [c.c_void_p, c.POINTER(DlInfo)], c.c_int
loaded = DlInfo()
assert dladdr(c.cast(pq.PQlibVersion, c.c_void_p), c.byref(loaded))
library = Path(os.fsdecode(loaded.filename)).resolve(strict=True)
ptr, text, integer = c.c_void_p, c.c_char_p, c.c_int

def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn

connect = api('PQconnectdb', ptr, text)
status = api('PQstatus', integer, ptr)
finish = api('PQfinish', None, ptr)
execute = api('PQexec', ptr, ptr, text)
prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
describe = api('PQdescribePrepared', ptr, ptr, text)
clear = api('PQclear', None, ptr)
close_prepared = api('PQclosePrepared', ptr, ptr, text)
exec_prepared = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
version = api('PQlibVersion', integer)
result_status = api('PQresultStatus', integer, ptr)
error_field = api('PQresultErrorField', text, ptr, integer)
rows = api('PQntuples', integer, ptr)
columns = api('PQnfields', integer, ptr)
value = api('PQgetvalue', text, ptr, integer, integer)
is_null = api('PQgetisnull', integer, ptr, integer, integer)
field_name = api('PQfname', text, ptr, integer)
field_type = api('PQftype', c.c_uint, ptr, integer)
field_size = api('PQfsize', integer, ptr, integer)
field_mod = api('PQfmod', integer, ptr, integer)
field_format = api('PQfformat', integer, ptr, integer)
nparams = api('PQnparams', integer, ptr)
param_type = api('PQparamtype', c.c_uint, ptr, integer)
transaction = api('PQtransactionStatus', integer, ptr)
parameter_status = api('PQparameterStatus', text, ptr, text)
protocol = api('PQfullProtocolVersion', integer, ptr)
connection_user = api('PQuser', text, ptr)
command_tag = api('PQcmdStatus', text, ptr)
assert version() >= 180000

cases = [
    ('positive', "SELECT 'Ab' ILIKE 'a%'", []),
    ('predicate_not', "SELECT 'Ab' NOT ILIKE 'a%'", []),
    ('outer_not', "SELECT NOT ('Ab' ILIKE 'a%')", []),
    ('protected', "SELECT 'ILIKE' /* ILIKE */", []),
    ('alias', "SELECT 'Ab' ILIKE", []),
    ('missing_operand', "SELECT 'Ab' ILIKE (", []),
    ('typed_null', "SELECT CAST(NULL AS CHAR) ILIKE 'a%'", []),
    ('parameter', "SELECT $1 ILIKE 'a%'", [25]),
    ('missing_unicode', "/*中*/ SELECT 'Ab' ILIKE (", []),
    ('missing_multiline', "SELECT\n'Ab' ILIKE (", []),
]
if task == 2:
    cases = [
        ('explicit', "SELECT 'A_b' ILIKE 'a!_b' ESCAPE '!'", []),
        ('omitted', "SELECT 'A_b' ILIKE 'a_b'", []),
        ('empty', "SELECT 'A_b' ILIKE 'a_b' ESCAPE ''", []),
        ('unicode', "SELECT 'A_b' ILIKE 'a中_b' ESCAPE '中'", []),
        ('parameter', "SELECT 'A_b' ILIKE 'a!_b' ESCAPE ($1)", [25]),
        ('missing_operand', "SELECT 'A_b' ILIKE 'a_b' ESCAPE", []),
        ('typed_null', "SELECT 'A_b' ILIKE 'a_b' ESCAPE NULL", []),
        ('multi', "SELECT 'A_b' ILIKE 'a_b' ESCAPE 'xy'", []),
        ('protected', "SELECT 'ILIKE' /* ESCAPE $1 */", []),
        ('missing_unicode', "/*中*/ SELECT 'A_b' ILIKE 'a_b' ESCAPE", []),
    ]
if task == 3:
    cases = [
        ('comparison', "SELECT 'Ab' ILIKE 'a%' = TRUE", []),
        ('not_and', "SELECT NOT 'Ab' ILIKE 'a%' AND TRUE", []),
        ('parentheses', "SELECT NOT ('Ab' ILIKE 'a%' AND TRUE)", []),
        ('predicate_not', "SELECT 'Ab' NOT ILIKE 'a%'", []),
        ('double_not', "SELECT NOT 'Ab' NOT ILIKE 'a%'", []),
        ('is_or', "SELECT 'Ab' ILIKE 'a%' IS TRUE OR FALSE", []),
        ('typed_null', "SELECT CAST(NULL AS CHAR) ILIKE 'a%' = TRUE", []),
        ('parameter', "SELECT $1 ILIKE 'a%' = TRUE", [25]),
        ('missing_operand', "/*中*/ SELECT NOT 'Ab' ILIKE (", []),
        ('protected', "SELECT 'ILIKE' /* NOT ILIKE $1 = TRUE */", []),
    ]
if task == 4:
    cases = [
        ('projection', 'INSERT INTO public.t(id) VALUES ($1) RETURNING id, id+1 AS next_id, t.*', [23]),
        ('quoted', 'INSERT INTO public.t(id) VALUES (7) RETURNING id AS "Next_ID", t.*', []),
        ('plain', 'INSERT INTO public.t(id) VALUES (11)', []),
        ('missing_projection', 'INSERT INTO public.t(id) VALUES (8) RETURNING', []),
        ('protected', "INSERT INTO public.t(id,v) VALUES (12,'RETURNING') /* RETURNING $2 */", []),
        ('null_star', 'INSERT INTO public.t(id) VALUES (NULL) RETURNING *', []),
        ('bare_alias', 'INSERT INTO public.t(id) VALUES (9) RETURNING id label', []),
        ('missing_trailing', '/*中*/ INSERT INTO public.t(id) VALUES (10) RETURNING id,', []),
    ]
if task == 9:
    cases = [
        ('list', 'SELECT ARRAY[1,2]', []),
        ('empty', 'SELECT ARRAY[]', []),
        ('null', 'SELECT ARRAY[NULL,1]', []),
        ('parameters', 'SELECT ARRAY[$2,$1,$2]', [23,23]),
        ('missing_close', '/*中*/ SELECT ARRAY[1,2', []),
        ('protected', "SELECT 'ARRAY[1,2]' /* ARRAY[] $2 */", []),
    ]
assert len({case[0] for case in cases}) == (6 if task == 9 else (8 if task == 4 else 10))
(evidence / 'cases.json').write_text(json.dumps([
    dict(case_id=name, capability_id='pg.dialect.array.syntax' if task == 9 else ('pg.dialect.ilike.syntax' if task == 1 else ('pg.dialect.escape.syntax' if task == 2 else 'pg.dialect.ilike.precedence')), sql=sql,
         phase='PQprepare', parameter_oids=oids, arguments=((['7','9'] if task == 9 else [None]*len(oids)) if oids else []),
         parameter_formats=([0]*len(oids) if oids else []), result_format=0,
         initial_state='I', expected_basis=f'dialect task {task} / PostgreSQL18.6 observation / roadmap chapter 2',
         expected_state='42601' if name.startswith('missing_') else '',
         object_mapping={}, encoding='UTF8', timezone='UTC',
         side_effects='not applicable: SELECT expressions without relations',
         execution='not applicable: parse/analyze handoff only')
    for name, sql, oids in cases], indent=2))


if task == 9:
    registry = json.loads((evidence/'cases.json').read_text())
    for case in registry:
        name = case['case_id']
        case['local_expected_state'] = '42601' if name == 'missing_close' else ('' if name == 'protected' else '0A000')
        case['reference_expected_state'] = '42601' if name == 'missing_close' else ('42P18' if name == 'empty' else '')
        case['expected_state'] = None
        case['execution'] = 'reference values observed for list/null/parameters; local array execution unavailable; literal executes on both'
        case['side_effects'] = 'no writes; independent local observer checks isolation table before/after'
    (evidence/'cases.json').write_text(json.dumps(registry, indent=2))


def observe(result):
    assert result, 'libpq returned no result'
    try:
        return dict(status=result_status(result),
                    state=(error_field(result, ord('C')) or b'').decode(),
                    message=(error_field(result, ord('M')) or b'').decode(),
                    position=(error_field(result, ord('P')) or b'').decode(),
                    fields=[[field_name(result, i).decode(), field_type(result, i),
                             field_size(result, i), field_mod(result, i)] for i in range(columns(result))],
                    formats=[field_format(result, i) for i in range(columns(result))],
                    param_count=nparams(result), params=[param_type(result, i) for i in range(nparams(result))],
                    rows=[[None if is_null(result, r, col) else value(result, r, col).decode()
                           for col in range(columns(result))] for r in range(rows(result))],
                    command_tag=(command_tag(result) or b'').decode(), table_origins=None)
    finally:
        clear(result)


def compare_syntax(actual, reference, sql, invalid):
    expected_state, expected_status = ('42601', 7) if invalid else ('', 1)
    if any(result['state'] != expected_state or result['status'] != expected_status
           for result in [actual, reference]):
        return False
    if not compare_metadata(actual, reference, 'prepare'):
        return False
    if invalid:
        return actual['position'] == reference['position'] == str(len(sql) + 1)
    return not actual['position'] and not reference['position']


def archive(name, side, data, observation):
    append_observation(evidence, dict(run=evidence.name, case_id=name, side=side, attempt=1,
                                    input=data, observation=observation, outcome='collected'))


def query(conn, sql):
    return observe(execute(conn, sql.encode()))


connections = {}
collected = {'aster': {}, 'reference': {}}
failures, descriptions = [], []
assertions = 0
self_checks = 0
reference_execution_assertions = 0
consumer_assertions = 0
completed = False
cleanup_ok = True


def interrupted(signum, frame):
    raise KeyboardInterrupt('interrupted')

signal.signal(signal.SIGTERM, interrupted)
signal.signal(signal.SIGINT, interrupted)


def endpoint(conn):
    return dict(protocol=protocol(conn), role=connection_user(conn).decode(),
                server_version=parameter_status(conn, b'server_version').decode(),
                encoding=parameter_status(conn, b'client_encoding').decode(),
                timezone=parameter_status(conn, b'TimeZone').decode(),
                search_path=query(conn, 'SHOW search_path')['rows'][0][0],
                transaction='I' if transaction(conn) == 0 else str(transaction(conn)))


def returning_handoff():
    """Keep syntax, safe rejection and RETURNING execution as distinct layers."""
    scope = f'dialect_04_{os.getpid()}_{int(sys.argv[2])}'
    connections = {}
    prepared = []
    checks = 0
    self_checks = 0
    done = False
    cleanup_ok = True
    reference_schema_created = False
    gate_cases = 0
    eligible = 0
    reference_dsn = os.environ['PG18_REFERENCE_DSN']
    local_dsn = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
    def check(condition, message):
        nonlocal checks
        assert condition, message
        checks += 1
    def recovery(conn, side, name):
        if transaction(conn) in (2, 3):
            check(query(conn, 'ROLLBACK')['status'] == 1, 'rollback failed')
        probe = query(conn, 'SELECT 1::int4')
        archive(name + '_recovery', side, dict(phase='PQexec', sql='SELECT 1::int4'), probe)
        check(probe['state'] == '' and probe['rows'] == [['1']] and transaction(conn) == 0, 'same-connection recovery failed')
    def snapshot(side):
        conn = connections[side + '_observer']
        sql = f'SELECT id, v FROM {scope}.t ORDER BY id' if side == 'reference' else 'SELECT id, v FROM public.t ORDER BY id'
        result = query(conn, sql)
        check(result['status'] == 2 and result['state'] == '', 'observer failed')
        return result['rows']
    def compare(a, r, kind, sql, ref_sql):
        if kind == 'gate':
            return (a['status'] == 7 and a['state'] == '0A000' and
                    a['message'] == 'DML RETURNING is unsupported by the PostgreSQL listener' and
                    a['position'] == '' and a['transaction_after'] == 'I' and
                    r['status'] == 1 and r['state'] == '' and r['position'] == '' and r['transaction_after'] == 'I')
        if kind == 'invalid':
            return (all(x['status'] == 7 and x['state'] == '42601' and x['transaction_after'] == 'I' for x in [a, r]) and
                    a['position'] == str(len(sql)+1) and r['position'] == str(len(ref_sql)+1))
        return all(x['status'] == 1 and x['state'] == '' and x['transaction_after'] == 'I' for x in [a, r]) and compare_metadata(a, r, 'prepare')
    try:
        for label in ['reference', 'reference_observer', 'aster', 'aster_b', 'aster_observer']:
            conn = connect((reference_dsn if label.startswith('reference') else local_dsn).encode())
            connections[label] = conn
            check(bool(conn) and status(conn) == 0, label + ' connection failed')
            if label.startswith('reference'):
                check(query(conn, "SET TIME ZONE 'UTC'")['status'] == 1, 'timezone failed')
            check(parameter_status(conn, b'client_encoding') == b'UTF8', 'encoding mismatch')
            check(parameter_status(conn, b'TimeZone') == b'UTC', 'timezone mismatch')
            check(query(conn, 'SET search_path TO public')['status'] == 1, 'search_path failed')
        ref = connections['reference']
        reference_version = query(ref, "SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(reference_version[1] == '180006', 'exact PostgreSQL18.6 required')
        reference_build = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        build = json.loads((evidence / 'rust-build.json').read_text())
        build['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance = dict(run=evidence.name, target_version='18.6', source=source_snapshot(root), build=build,
                          client=dict(python=sys.version, library=str(library), sha256=hashlib.sha256(library.read_bytes()).hexdigest(), PQlibVersion=version()),
                          os=platform.platform(), storage='CreateAnalyzeSession test storage', authentication='InsecureRootOnly',
                          reference=dict(version=reference_version[0], server_version_num=reference_version[1]), reference_build=reference_build,
                          endpoints={s: endpoint(connections[s]) for s in ['aster', 'aster_b', 'reference']})
        (evidence / 'provenance.json').write_text(json.dumps(provenance, indent=2))
        check(validate_provenance(provenance, copy.deepcopy(provenance)), 'provenance failed')
        damaged = copy.deepcopy(provenance); damaged['reference_build']['server_version_num'] = 180004
        assert not validate_provenance(damaged, provenance)
        self_checks += 1
        check(query(ref, f'CREATE SCHEMA {scope}')['status'] == 1, 'reference schema failed')
        reference_schema_created = True
        for side in ['aster', 'reference']:
            table = f'{scope}.t' if side == 'reference' else 'public.t'
            check(query(connections[side], f'CREATE TABLE {table} (id INT, v VARCHAR(20))')['status'] == 1, 'table creation failed')
        registry = []
        for index, (name, sql, oids) in enumerate(cases):
            kind = 'invalid' if name.startswith('missing_') else ('control' if name in ('plain', 'protected') else 'gate')
            # Explicit test-object mapping, never a product SQL adaptation.
            ref_sql = sql.replace('public.t', f'{scope}.t')
            data = dict(case_id=name, capability_id='pg.dialect.insert.returning.syntax',
                        sql=sql, reference_sql=ref_sql, phase='PQprepare', parameter_oids=oids,
                        arguments=None, parameter_formats=None, result_format=0, encoding='UTF8', timezone='UTC',
                        role={s: connection_user(connections[s]).decode() for s in ['aster', 'reference']}, search_path='public', initial_state='I',
                        object_mapping={'aster': f'{sys.argv[3]}.t', 'reference': f'{scope}.t'},
                        expected_basis='task4 syntax/gate contract and exact PostgreSQL18.6 PQprepare',
                        expected={'aster': '0A000' if kind == 'gate' else ('42601' if kind == 'invalid' else 'success'),
                                  'reference': '42601' if kind == 'invalid' else 'success'},
                        unverified='RETURNING binding/types and execution remain outside this gate; no local successful RETURNING metadata')
            registry.append(data)
            (evidence / 'cases.json').write_text(json.dumps(registry, indent=2))
            before = {side: snapshot(side) for side in ['aster', 'reference']}
            results = {}
            for side in ['reference', 'aster']:
                conn = connections['aster_b' if side == 'aster' and index % 2 else side]
                actual_sql = ref_sql if side == 'reference' else sql
                statement = f'returning_{index}'.encode()
                result = observe(prepare(conn, statement, actual_sql.encode(), len(oids), (c.c_uint * len(oids))(*oids) if oids else None))
                result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                archive(name, side, dict(phase='PQprepare', sql=actual_sql, statement=statement.decode(),
                                        parameter_oids=oids, arguments=None, parameter_formats=None, result_format=0), result)
                results[side] = result
                if result['status'] == 1:
                    prepared.append((conn, statement))
                    desc = observe(describe(conn, statement))
                    archive(name + '_describe', side, dict(phase='PQdescribePrepared', statement=statement.decode()), desc)
                    check(desc['status'] == 1 and desc['state'] == '', 'describe failed')
                    check(desc['params'] == oids and desc['param_count'] == len(oids), 'parameter identity lost')
                    expected_fields = [] if kind == 'control' else (
                        [['id',23,4,-1], ['next_id',23,4,-1], ['id',23,4,-1], ['v',1043,-1,24]] if name == 'projection' else
                        [['Next_ID',23,4,-1], ['id',23,4,-1], ['v',1043,-1,24]] if name == 'quoted' else
                        [['id',23,4,-1], ['v',1043,-1,24]] if name == 'null_star' else [['label',23,4,-1]])
                    check(desc['fields'] == expected_fields, 'reference/control descriptor mismatch')
                    check(desc['formats'] == [0] * len(expected_fields), 'nontext descriptor')
                    check(observe(close_prepared(conn, statement))['status'] == 1, 'statement close failed')
                    prepared.remove((conn, statement))
                recovery(conn, side, name)
                check(snapshot(side) == before[side], 'prepare/describe/rejection wrote rows')
            check(compare(results['aster'], results['reference'], kind, sql, ref_sql), 'syntax/gate comparator mismatch')
            for mutation in ['state', 'status']:
                changed = copy.deepcopy(results['aster']); changed[mutation] = 'XX000' if mutation == 'state' else 0
                assert not compare(changed, results['reference'], kind, sql, ref_sql)
                self_checks += 1
            if kind == 'gate':
                gate_cases += 1
                if not oids:
                    result = query(connections['aster'], sql)
                    archive(name + '_simple_gate', 'aster', dict(phase='PQexec', sql=sql), result)
                    check(result['status'] == 7 and result['state'] == '0A000', 'simple gate failed')
                    recovery(connections['aster'], 'aster', name + '_simple')
                    check(snapshot('aster') == before['aster'], 'simple gate wrote rows')
            elif kind == 'invalid':
                for side in ['aster', 'reference']:
                    actual_sql = ref_sql if side == 'reference' else sql
                    result = query(connections[side], actual_sql)
                    archive(name + '_simple_error', side, dict(phase='PQexec', sql=actual_sql), result)
                    check(result['status'] == 7 and result['state'] == '42601' and result['position'] == str(len(actual_sql)+1), 'simple diagnostic mismatch')
                    recovery(connections[side], side, name + '_simple')
                    check(snapshot(side) == before[side], 'invalid query wrote rows')
                eligible += 1
            else:
                for side in ['aster', 'reference']:
                    actual_sql = ref_sql if side == 'reference' else sql
                    result = query(connections[side], actual_sql)
                    archive(name + '_execute_control', side, dict(phase='PQexec', sql=actual_sql), result)
                    check(result['status'] == 1 and result['state'] == '' and result['command_tag'] == 'INSERT 0 1', 'normal INSERT failed')
                expected_rows = [['11', None]] if name == 'plain' else [['11', None], ['12', 'RETURNING']]
                for side in ['aster', 'reference']:
                    check(snapshot(side) == expected_rows, 'control values/NULL/side effects mismatch')
                eligible += 1
            print('PG_DIALECT_CHECKPOINT', name, flush=True)
            check(sys.stdin.readline().strip() == 'continue', 'MySQL checkpoint aborted')
            for label in (['aster_b', 'aster'] if index % 2 == 0 else ['aster', 'aster_b']):
                recovery(connections[label], 'aster', name + '_' + label)
                check(query(connections[label], 'SELECT id FROM public.dialect_isolation')['rows'] == [['31']], 'cross-dialect state polluted')
        for side in ['aster', 'reference']:
            conn = connections[side]
            check(query(conn, 'BEGIN')['status'] == 1, 'BEGIN failed')
            failure = observe(prepare(conn, b'failed_returning', b'INSERT INTO t VALUES (1) RETURNING', 0, None))
            failure['transaction_after'] = {0:'I',2:'T',3:'E'}.get(transaction(conn))
            archive('transaction_error', side, dict(phase='PQprepare', sql='INSERT INTO t VALUES (1) RETURNING'), failure)
            check(failure['state'] == '42601' and failure['transaction_after'] == 'E', 'transaction error state mismatch')
            recovery(conn, side, 'transaction')
        parsed = urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
        check(parsed.scheme == 'mysql' and bool(parsed.hostname) and bool(parsed.port), 'MySQL reference missing')
        mysql_env = dict(os.environ, MYSQL_PWD=parsed.password or '')
        args = ['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql):
            proc = subprocess.run(args + ['-e', "SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; " + sql], env=mysql_env, capture_output=True, text=True, timeout=15)
            return dict(code=proc.returncode, output=proc.stdout.splitlines(), error=proc.stderr)
        identity = mysql_ref('SELECT VERSION(), @@version_compile_os, @@version_compile_machine; SELECT @@sql_mode, @@character_set_connection, @@collation_connection')
        archive('reference_identity', 'mysql', None, identity)
        check(identity['code'] == 0, 'MySQL identity failed')
        table = f'mysql.pg_returning_{os.getpid()}'
        legal = mysql_ref(f'CREATE TEMPORARY TABLE {table}(id INT); INSERT INTO {table} VALUES (31); SELECT id FROM {table}; PREPARE p FROM \'SELECT id FROM {table} WHERE id=?\'; SET @v=31; EXECUTE p USING @v; DEALLOCATE PREPARE p')
        archive('legal_prepare', 'mysql', None, legal)
        check(legal['code'] == 0 and legal['output'] == ['31', '31'], 'MySQL control failed')
        for name, sql, oids in cases:
            # Own table mapping and own legal VALUES syntax isolate RETURNING.
            suffix = {'projection':'id, id+1 AS next_id, t.*', 'quoted':'id AS "Next_ID", t.*', 'null_star':'*', 'bare_alias':'id label'}.get(name)
            if suffix is None:
                continue
            sql = f'CREATE TEMPORARY TABLE {table}(id INT); INSERT INTO {table} VALUES (1) RETURNING {suffix}'
            result = mysql_ref(sql)
            archive('returning_' + name, 'mysql', dict(phase='query', sql=sql), result)
            check(result['code'] != 0 and 'ERROR 1064 (42000)' in result['error'] and 'RETURNING' in result['error'], 'MySQL syntax isolation failed')
        final = copy.deepcopy(provenance)
        final['source'] = source_snapshot(root)
        final['build']['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        final['client']['sha256'] = hashlib.sha256(library.read_bytes()).hexdigest()
        final['reference_build'] = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        final['endpoints'] = {s:endpoint(connections[s]) for s in ['aster','aster_b','reference']}
        check(validate_provenance(provenance, final), 'provenance changed')
        done = True
    finally:
        for conn, statement in prepared:
            if conn:
                observe(close_prepared(conn, statement))
        for conn in connections.values():
            if conn and transaction(conn) in (2,3):
                cleanup_ok = query(conn, 'ROLLBACK')['status'] == 1 and cleanup_ok
        if reference_schema_created:
            cleanup_ok = query(connections['reference'], f'DROP SCHEMA {scope} CASCADE')['status'] == 1 and cleanup_ok
        for conn in connections.values():
            if conn:
                finish(conn)
        summary = dict(cases=len(cases), assertions=checks, self_checks=self_checks,
                       syntax_eligible=len(cases) if done and cleanup_ok else 0,
                       gate_cases=gate_cases if done and cleanup_ok else 0,
                       unique_effective=eligible if done and cleanup_ok else 0,
                       semantic_returning_success=0, cleanup_ok=cleanup_ok,
                       scope='parser/gate evidence only; 0A000 cases do not count as successful RETURNING')
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
    assert done and cleanup_ok, 'incomplete run/cleanup'
    print(json.dumps(summary), flush=True)

def array_handoff():
    """Array syntax/gate only; preserve separate reference type-analysis outcomes."""
    connections, prepared = {}, []
    checks = 0
    self_checks = 0
    eligible = 0
    done = False
    cleanup_ok = True
    def record(name, label, data, result):
        allowed = {'phase','sql','statement','parameter_oids','arguments','parameter_formats','result_format','command'}
        archive(name, 'aster' if label.startswith('aster') else label,
                {key:value for key,value in (data or {}).items() if key in allowed},
                dict(result, connection=label))
    def check(condition, message):
        nonlocal checks
        assert condition, message
        checks += 1
    def recover(conn, label, name):
        if transaction(conn) in (2,3):
            check(query(conn, 'ROLLBACK')['status'] == 1, 'rollback failed')
        result = query(conn, 'SELECT 1::int4')
        record(name+'_recovery', label, dict(phase='PQexec', sql='SELECT 1::int4'), result)
        check(result['rows'] == [['1']] and result['state'] == '' and transaction(conn) == 0, 'original connection recovery failed')
    def compare(a, r, name, sql):
        if name == 'missing_close':
            return all(x['state']=='42601' and x['status']==7 and x['position']==str(len(sql)+1) for x in [a,r])
        if name == 'protected':
            return all(x['state']=='' and x['status']==1 for x in [a,r])
        expected_ref = '42P18' if name == 'empty' else ''
        return (a['state']=='0A000' and a['status']==7 and not a['position'] and
                a['message']=='ARRAY constructors require array type analysis support' and
                r['state']==expected_ref and r['status']==(7 if expected_ref else 1) and
                r['position']==('8' if expected_ref else ''))
    try:
        local = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
        for label in ['reference','aster','aster_b','aster_observer']:
            conn = connect((os.environ['PG18_REFERENCE_DSN'] if label=='reference' else local).encode())
            connections[label] = conn
            check(bool(conn) and status(conn)==0, label+' connection failed')
            if label=='reference': check(query(conn,"SET TIME ZONE 'UTC'")['status']==1,'timezone failed')
            check(parameter_status(conn,b'client_encoding')==b'UTF8' and parameter_status(conn,b'TimeZone')==b'UTC','connection settings')
            check(query(conn,'SET search_path TO public')['status']==1,'search path failed')
        ref = connections['reference']
        ref_version = query(ref,"SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(ref_version[1]=='180006','exact PostgreSQL18.6 required')
        build = json.loads((evidence/'rust-build.json').read_text())
        build['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance = dict(run=evidence.name, target_version='18.6', source=source_snapshot(root), build=build,
            reference=dict(version=ref_version[0],server_version_num=ref_version[1]),
            reference_build=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text()),
            client=dict(python=sys.version,library=str(library),sha256=hashlib.sha256(library.read_bytes()).hexdigest(),PQlibVersion=version()),
            os=platform.platform(),storage='CreateAnalyzeSession test storage',authentication='InsecureRootOnly',
            endpoints={s:endpoint(connections[s]) for s in ['aster','aster_b','reference']})
        (evidence/'provenance.json').write_text(json.dumps(provenance,indent=2))
        check(validate_provenance(provenance,copy.deepcopy(provenance)),'provenance invalid')
        damaged=copy.deepcopy(provenance);damaged['reference_build']['server_version_num']=180004
        check(not validate_provenance(damaged,provenance),'provenance negative self-check');self_checks+=1
        before=query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']
        check(before==[['31']],'initial observer state')
        for index,(name,sql,oids) in enumerate(cases):
            input_data=dict(capability_id='pg.dialect.array.syntax',case_id=name,sql=sql,phase='PQprepare',
                parameter_oids=oids,arguments=['7','9'] if oids else [],parameter_formats=[0]*len(oids),result_format=0,
                encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',object_mapping={},
                expected_basis='task9 syntax/gate contract and exact PostgreSQL18.6 analysis; values/OIDs unavailable on local gated constructors')
            results={}
            for label in ['reference', 'aster_b' if index%2 else 'aster']:
                conn=connections[label];check(transaction(conn)==0,'initial transaction not idle')
                statement=f'array_{index}'.encode()
                result=observe(prepare(conn,statement,sql.encode(),len(oids),(c.c_uint*len(oids))(*oids) if oids else None))
                result['transaction_after']={0:'I',2:'T',3:'E'}.get(transaction(conn))
                record(name,label,input_data,result);results['reference' if label=='reference' else 'aster']=result
                if result['status']==1:
                    prepared.append((conn,statement))
                    desc=observe(describe(conn,statement));record(name+'_describe',label,dict(input_data,phase='Describe'),desc)
                    if label=='reference' and name in ('list','null','parameters'):
                        check(desc['fields'][0][1:]==[1007,-1,-1], 'reference integer[] metadata')
                        if oids:check(desc['param_count']==2 and desc['params']==[23,23],'reference parameter identity')
                        args=(text*2)(b'7',b'9') if oids else None
                        values=observe(exec_prepared(conn,statement,len(oids),args,None,None,0));record(name+'_execute',label,dict(input_data,phase='Execute'),values)
                        expected={'list':'{1,2}','null':'{NULL,1}','parameters':'{9,7,9}'}[name]
                        check(values['rows']==[[expected]] and values['command_tag']=='SELECT 1','reference array values')
                    if name=='protected':
                        values=observe(exec_prepared(conn,statement,0,None,None,None,0));record(name+'_execute',label,dict(input_data,phase='Execute'),values)
                        check(values['rows']==[['ARRAY[1,2]']],'protected value')
                recover(conn,label,name)
            check(compare(results['aster'],results['reference'],name,sql),'prepare contract mismatch: '+name)
            if name in ('list','empty','missing_close'):
                damaged=copy.deepcopy(results['aster']);damaged['state']='42601' if name!='missing_close' else '0A000'
                check(not compare(damaged,results['reference'],name,sql),'comparator negative check');self_checks+=1
            for label in ['aster','reference']:
                result=query(connections[label],sql);record(name+'_simple',label,dict(input_data,phase='Q',arguments=[],parameter_oids=[]),result)
                if name=='protected':check(result['rows']==[['ARRAY[1,2]']],'simple literal changed')
                elif name=='missing_close':check(result['state']=='42601' and result['position']==str(len(sql)+1),'simple original position')
                elif label=='aster':check(result['state']=='0A000','simple array gate')
                elif name=='empty':check(result['state']=='42P18' and result['position']=='8','empty type analysis')
                elif name=='parameters':check(result['state']=='42P02','simple parameter requires binding')
                else:check(result['rows']==[[{'list':'{1,2}','null':'{NULL,1}'}[name]]],'simple reference array')
                recover(connections[label],label,name+'_simple')
            for label in (['aster_b','aster'] if index%2==0 else ['aster','aster_b']):
                recover(connections[label],label,name+'_isolation')
            check(query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']==before,'observer state changed')
            print('PG_DIALECT_CHECKPOINT '+name,flush=True)
            check(sys.stdin.readline().strip()=='continue','MySQL checkpoint aborted')
            eligible+=1
        for label in ['aster','reference']:
            conn=connections[label];check(query(conn,'BEGIN')['status']==1,'begin failed')
            result=observe(prepare(conn,b'transaction_bad',b'SELECT ARRAY[1,2',0,None))
            record('transaction_error',label,dict(phase='PQprepare',sql='SELECT ARRAY[1,2'),dict(result,transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conn))))
            check(result['state']=='42601' and transaction(conn)==3,'transaction error state');recover(conn,label,'transaction')
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
        env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql):
            proc=subprocess.run(args+['-e',"SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            return dict(code=proc.returncode,output=proc.stdout.splitlines(),error=proc.stderr)
        identity=mysql_ref('SELECT VERSION(), @@version_compile_os, @@version_compile_machine; SELECT @@sql_mode, @@character_set_connection, @@collation_connection')
        record('identity','mysql',None,identity);check(identity['code']==0,'MySQL identity')
        for name,sql,_ in cases:
            result=mysql_ref(sql);record(name,'mysql',dict(phase='Q',sql=sql),result)
            check((result['code']==0 and result['output']==['ARRAY[1,2]']) if name=='protected' else (result['code']!=0 and 'ERROR 1064 (42000)' in result['error']),'MySQL reference isolation')
        legal=mysql_ref("SELECT 31 AS v; PREPARE s FROM 'SELECT ? AS v'; SET @v=31; EXECUTE s USING @v; DEALLOCATE PREPARE s")
        record('legal_prepare','mysql',None,legal);check(legal['code']==0 and legal['output']==['31','31'],'MySQL prepare control')
        final=copy.deepcopy(provenance);final['source']=source_snapshot(root)
        final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        final['client']['sha256']=hashlib.sha256(library.read_bytes()).hexdigest()
        final['reference_build']=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        final['endpoints']={s:endpoint(connections[s]) for s in ['aster','aster_b','reference']}
        check(validate_provenance(provenance,final),'provenance changed')
        done=True
    finally:
        for conn,statement in prepared:
            result=observe(close_prepared(conn,statement));cleanup_ok=result['status']==1 and cleanup_ok
        for conn in connections.values():
            if conn and transaction(conn) in (2,3):cleanup_ok=query(conn,'ROLLBACK')['status']==1 and cleanup_ok
            if conn:finish(conn)
        summary=dict(cases=len(cases),assertions=checks,self_checks=self_checks,syntax_eligible=eligible if done and cleanup_ok else 0,
            gate_cases=4 if done and cleanup_ok else 0,unique_effective=2 if done and cleanup_ok else 0,
            semantic_array_success=0,cleanup_ok=cleanup_ok,scope='array syntax/gate; local array typing/binding/execute unavailable')
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and cleanup_ok,'incomplete run/cleanup'
    print(json.dumps(summary),flush=True)

if task == 9:
    array_handoff()
    sys.exit(0)

if task == 4:
    returning_handoff()
    sys.exit(0)


try:
    # Connection material never appears in observations or error messages.
    reference_dsn = os.environ['PG18_REFERENCE_DSN']
    local_dsn = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
    for label, dsn in [('reference', reference_dsn), ('aster', local_dsn), ('aster_b', local_dsn)]:
        conn = connect(dsn.encode())
        connections[label] = conn
        assert conn and status(conn) == 0, f'{label}: connection failed'
        if label == 'reference':
            assert query(conn, "SET TIME ZONE 'UTC'")['status'] == 1
        assert parameter_status(conn, b'client_encoding') == b'UTF8'
        assert parameter_status(conn, b'TimeZone') == b'UTC'
        assert query(conn, 'SET search_path TO public')['status'] == 1
    ref = connections['reference']
    reference_version = query(ref, 'SELECT version(), current_setting(\'server_version_num\')')['rows'][0]
    assert reference_version[1] == '180006', 'exact PostgreSQL18.6 required'
    build = json.loads((evidence / 'rust-build.json').read_text())
    build['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
    (evidence / 'rust-build.json').write_text(json.dumps(build, indent=2))
    reference_build = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
    provenance = dict(run=evidence.name, target_version='18.6', source=source_snapshot(root),
                      build=build, client=dict(python=sys.version, library=str(library),
                                              sha256=hashlib.sha256(library.read_bytes()).hexdigest(), PQlibVersion=version()),
                      os=platform.platform(), storage='CreateAnalyzeSession test storage', authentication='InsecureRootOnly',
                      reference=dict(version=reference_version[0], server_version_num=reference_version[1]),
                      reference_build=reference_build,
                      endpoints={s: endpoint(connections[s]) for s in ['aster', 'aster_b', 'reference']})
    (evidence / 'provenance.json').write_text(json.dumps(provenance, indent=2))
    assert validate_provenance(provenance, copy.deepcopy(provenance)), 'provenance gate failed'
    damaged = copy.deepcopy(provenance); damaged['reference_build']['server_version_num'] = 180004
    assert not validate_provenance(damaged, provenance), 'negative provenance check failed'
    self_checks += 1
    for index, (name, sql, oids) in enumerate(cases):
        for side in ['reference', 'aster']:
            conn = connections['aster_b' if side == 'aster' and index % 2 else side]
            data = dict(phase='PQprepare', sql=sql, statement=f'case_{index}', parameter_oids=oids,
                        arguments=[None] if oids else [], parameter_formats=[0] if oids else [], result_format=0)
            assert transaction(conn) == 0, 'initial state is not idle'
            result = observe(prepare(conn, data['statement'].encode(), sql.encode(), len(oids), (c.c_uint * len(oids))(*oids) if oids else None))
            result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
            archive(name, side, data, result)
            collected[side][name] = result
            if result['status'] == 1:
                desc = observe(describe(conn, data['statement'].encode()))
                archive(name + '_describe', side, dict(phase='PQdescribePrepared', statement=data['statement']), desc)
                descriptions.append((side, name, desc))
                if name == 'parameter':
                    assert desc['param_count'] == 1 and desc['params'] == [25]
                    bound = observe(exec_prepared(conn, data['statement'].encode(), 1,
                                                  (text * 1)(None), None, (integer * 1)(0), 0))
                    archive('parameter_bind_null', side,
                            dict(phase='PQexecPrepared', statement=data['statement'], arguments=[None],
                                 parameter_formats=[0], result_format=0), bound)
                    if side == 'reference':
                        assert bound['status'] == 2 and bound['state'] == '' and bound['rows'] == [[None]]
                    else:
                        # Matching execution and boolean codecs are explicitly
                        # outside task 1. Keep the actual consumer gate, never
                        # replace it with a successful semantic result.
                        assert bound['status'] == 7 and bound['state'] == '0A000'
                        altered = copy.deepcopy(bound); altered['state'] = 'XX000'
                        assert not (altered['status'] == 7 and altered['state'] == '0A000')
                        self_checks += 1
                assert observe(close_prepared(conn, data['statement'].encode()))['status'] == 1
            # PQprepare consumes the extended Sync response; never reconnect.
            recovery = query(conn, 'SELECT 1::int4')
            archive(name + '_recovery', side, dict(phase='PQexec', sql='SELECT 1::int4'), recovery)
            assert recovery['rows'] == [['1']] and recovery['state'] == '' and transaction(conn) == 0
            assert query(connections['aster_b'], 'SELECT 1::int4')['rows'] == [['1']]
            assertions += 3
        invalid = name.startswith('missing_')
        a, r = collected['aster'][name], collected['reference'][name]
        if not compare_syntax(a, r, sql, invalid):
            failures.append(dict(case_id=name, reason='syntax/status/position mismatch', aster=a, reference=r))
        # Negative checks cannot be satisfied by a catch-all rejection or a
        # missing cursor; they never contribute business case counts.
        altered = copy.deepcopy(r); altered['state'] = 'XX000'
        assert not compare_syntax(altered, r, sql, invalid)
        self_checks += 1
        if invalid:
            altered = copy.deepcopy(r); altered['position'] = ''
            assert not compare_syntax(altered, r, sql, invalid)
            self_checks += 1
        if invalid:
            simple = {}
            for side in ['aster', 'reference']:
                simple[side] = query(connections[side], sql)
                archive(name + '_simple', side, dict(phase='PQexec', sql=sql), simple[side])
                recovery = query(connections[side], 'SELECT 1::int4')
                archive(name + '_simple_recovery', side, dict(phase='PQexec', sql='SELECT 1::int4'), recovery)
                assert recovery['rows'] == [['1']] and transaction(connections[side]) == 0
            assert compare_syntax(simple['aster'], simple['reference'], sql, True)
        if name in ('alias', 'protected'):
            outputs = {}
            for side in ['aster', 'reference']:
                outputs[side] = query(connections[side], sql)
                archive(name + '_query', side, dict(phase='PQexec', sql=sql), outputs[side])
                assert outputs[side]['rows'] == [['Ab' if name == 'alias' else 'ILIKE']]
                assert outputs[side]['state'] == ''
            if name == 'alias':
                assert compare_metadata(outputs['aster'], outputs['reference'])
        assertions += 2
        print('PG_DIALECT_CHECKPOINT', name, flush=True)
        assert sys.stdin.readline().strip() == 'continue', 'MySQL checkpoint aborted'
        # Even cases: A -> M -> B -> A; odd cases reverse A and B.
        for label in (['aster_b', 'aster'] if index % 2 == 0 else ['aster', 'aster_b']):
            probe = query(connections[label], 'SELECT 1::int4')
            probe['connection'] = label
            archive(name + '_alternating_' + label, 'aster', dict(phase='PQexec', sql='SELECT 1::int4'), probe)
            assert probe['rows'] == [['1']] and transaction(connections[label]) == 0
            stored = query(connections[label], 'SELECT id FROM public.dialect_isolation')
            archive(name + '_stored_' + label, 'aster', dict(phase='PQexec', sql='SELECT id FROM public.dialect_isolation'), stored)
            assert stored['status'] == 2 and stored['rows'] == [['31']]
    if task == 3:
        expected = dict(comparison='t', not_and='f', parentheses='f', predicate_not='f',
                        double_not='t', is_or='t', typed_null=None)
        for name, sql, oids in cases:
            if name not in expected:
                continue
            result = query(ref, sql)
            archive(name + '_reference_execute', 'reference', dict(phase='PQexec', sql=sql), result)
            assert result['status'] == 2 and result['state'] == ''
            assert result['rows'] == [[expected[name]]]
            reference_execution_assertions += 2
            # Consume the same predicate through the ordinary planner without
            # claiming the out-of-scope boolean result codec is implemented.
            where_sql = 'SELECT id FROM public.dialect_isolation WHERE ' + sql[len('SELECT '):]
            local = query(connections['aster'], where_sql)
            archive(name + '_where_execute', 'aster', dict(phase='PQexec', sql=where_sql), local)
            assert local['status'] == 2 and local['state'] == ''
            assert local['rows'] == ([['31']] if expected[name] == 't' else [])
            consumer_assertions += 2
    if task == 2:
        # Freeze the semantic boundary from the exact reference, separately
        # from the local syntax-only counts. Multi-character validation is
        # deferred to execution; NULL never becomes a default backslash.
        for name, sql, oids in cases:
            if oids or name.startswith('missing_') or name == 'protected':
                continue
            result = query(ref, sql)
            archive(name + '_reference_execute', 'reference', dict(phase='PQexec', sql=sql), result)
            if name == 'multi':
                assert result['status'] == 7 and result['state'] == '22025'
                assert query(ref, 'SELECT 1::int4')['rows'] == [['1']]
            else:
                assert result['status'] == 2 and result['state'] == ''
                assert result['rows'] == [[None if name == 'typed_null' else 't']]
            reference_execution_assertions += 2
    if task == 2:
        sql = "SELECT id FROM public.dialect_isolation WHERE 'A_b' ILIKE 'a!_b' ESCAPE '!'"
        result = query(connections['aster'], sql)
        archive('legacy_ascii_execution', 'aster', dict(phase='PQexec', sql=sql), result)
        assert result['status'] == 2 and result['rows'] == [['31']]
        consumer_assertions += 1
        for name, sql, oids in cases:
            if name not in ('empty', 'unicode', 'typed_null', 'multi'):
                continue
            result = query(connections['aster'], sql)
            archive(name + '_execution_gate', 'aster', dict(phase='PQexec', sql=sql), result)
            assert result['status'] == 7 and result['state'] == '0A000'
            assert result['message'] == 'PostgreSQL ESCAPE expression evaluation is unsupported'
            assert query(connections['aster'], 'SELECT 1::int4')['rows'] == [['1']]
            assert transaction(connections['aster']) == 0
            consumer_assertions += 4
    for side in ['aster', 'reference']:
        conn = connections[side]
        assert query(conn, 'BEGIN')['status'] == 1
        failure = observe(prepare(conn, b'failed_transaction', b"SELECT 'Ab' ILIKE (", 0, None))
        failure['transaction'] = transaction(conn)
        archive('transaction_error', side, dict(phase='PQprepare', sql="SELECT 'Ab' ILIKE ("), failure)
        assert failure['state'] == '42601' and failure['transaction'] == 3
        assert query(conn, 'ROLLBACK')['status'] == 1 and transaction(conn) == 0
        recovery = query(conn, 'SELECT 1::int4')
        archive('transaction_recovery', side, dict(phase='PQexec', sql='SELECT 1::int4'), recovery)
        assert recovery['rows'] == [['1']]
    # Collect exact MySQL reference version and its own legal/proprietary inputs.
    parsed = urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
    assert parsed.scheme == 'mysql' and parsed.hostname and parsed.port
    mysql_env = dict(os.environ, MYSQL_PWD=parsed.password or '')
    mysql_args = ['mysql', '--protocol=TCP', '-h', parsed.hostname, '-P', str(parsed.port), '-u', parsed.username or 'root', '--batch', '--raw', '--skip-column-names']
    def mysql_ref(sql, type_info=False):
        options = ['--column-type-info', '--table'] if type_info else []
        proc = subprocess.run(mysql_args + options + ['-e', "SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; " + sql], env=mysql_env, capture_output=True, text=True, timeout=15)
        return dict(code=proc.returncode, output=proc.stdout.splitlines(), error=proc.stderr)
    mysql_identity = mysql_ref('SELECT VERSION(), @@version_compile_os, @@version_compile_machine; SELECT @@sql_mode, @@character_set_connection, @@collation_connection')
    assert mysql_identity['code'] == 0
    archive('reference_identity', 'mysql', None, mysql_identity)
    if task == 3:
        for mode, expected in [('', '1'), ('HIGH_NOT_PRECEDENCE', '0')]:
            result = mysql_ref(f"SET SESSION sql_mode='{mode}'; SELECT NOT 'Ab' LIKE 'x%'")
            archive('like_mode_' + (mode or 'default'), 'mysql', dict(command=f"SET SESSION sql_mode='{mode}'; SELECT NOT 'Ab' LIKE 'x%'"), result)
            assert result['code'] == 0 and result['output'] == [expected]

    (evidence / 'mysql-reference.json').write_text(json.dumps(mysql_identity))
    # A connection-local temporary table disappears even on a failed CLI call;
    # no persistent reference object or existing table is modified.
    ref_table = f'mysql.pg_dialect_01_ref_{os.getpid()}'
    legal_sql = (f'CREATE TEMPORARY TABLE {ref_table} (id INT); '
                 f'INSERT INTO {ref_table} VALUES (31); SELECT id AS v FROM {ref_table}; '
                 f"PREPARE p FROM 'SELECT id AS v FROM {ref_table} WHERE id=?'; "
                 f'SET @v=31; EXECUTE p USING @v; DEALLOCATE PREPARE p; DROP TEMPORARY TABLE {ref_table}')
    legal = mysql_ref(legal_sql, type_info=True)
    assert legal['code'] == 0
    legal['field_types'] = re.findall(r'Type:\s+(\w+)', '\n'.join(legal['output']))
    assert legal['field_types'] == ['LONG', 'LONG']
    assert sum(bool(re.fullmatch(r'\|\s*31\s*\|', line)) for line in legal['output']) == 2
    archive('reference_legal_and_prepare', 'mysql', dict(command=legal_sql), legal)
    for name, sql, oids in cases:
        if not oids:
            observation = mysql_ref(sql)
            archive('reference_' + name, 'mysql', dict(command=sql), observation)
    for name, aster_desc in [(n, d) for s, n, d in descriptions if s == 'aster']:
        reference_desc = next(d for s, n, d in descriptions if s == 'reference' and n == name)
        archive(name + '_metadata_comparison', 'aster', None,
                dict(equal=compare_metadata(aster_desc, reference_desc, 'describe'),
                     scope='boolean descriptors and default expression labels belong to scalar/metadata plans'))
    final = copy.deepcopy(provenance)
    final['source'] = source_snapshot(root)
    final['endpoints'] = {s: endpoint(connections[s]) for s in ['aster', 'aster_b', 'reference']}
    final['build']['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
    final['client']['sha256'] = hashlib.sha256(library.read_bytes()).hexdigest()
    final['reference_build'] = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
    final_version = query(ref, "SELECT version(), current_setting('server_version_num')")['rows'][0]
    final['reference'] = dict(version=final_version[0], server_version_num=final_version[1])
    assert validate_provenance(provenance, final), 'provenance changed during run'
    completed = True
finally:
    for conn in connections.values():
        if conn:
            if transaction(conn) in (2, 3):
                cleanup_ok = query(conn, 'ROLLBACK')['status'] == 1 and cleanup_ok
            finish(conn)
    summary = dict(cases=len(cases), assertions=assertions,
                   self_checks=self_checks, reference_execution_assertions=reference_execution_assertions,
                   consumer_assertions=consumer_assertions, failures=failures, cleanup_ok=cleanup_ok,
                   syntax_eligible=len(cases) if completed and cleanup_ok and not failures and assertions == 8 * len(cases) else 0,
                   unique_effective=0, scope='syntax handoff only; parameter execution gate 0A000 is not semantic success')
    (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
assert not failures, 'ILIKE handoff mismatch; see archived observations'
print(json.dumps(summary), flush=True)
