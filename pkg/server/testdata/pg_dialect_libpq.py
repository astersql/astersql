# Copyright 2026 AsterSQL.
"""ILIKE syntax handoff through real libpq; argv: task, local port, database.

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
assert task in (1, 2, 3), 'unregistered dialect task'
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
assert len({case[0] for case in cases}) == 10
(evidence / 'cases.json').write_text(json.dumps([
    dict(case_id=name, capability_id='pg.dialect.ilike.syntax' if task == 1 else ('pg.dialect.escape.syntax' if task == 2 else 'pg.dialect.ilike.precedence'), sql=sql,
         phase='PQprepare', parameter_oids=oids, arguments=([None] if oids else []),
         parameter_formats=([0] if oids else []), result_format=0,
         initial_state='I', expected_basis=f'dialect task {task} / PostgreSQL18.6 observation / roadmap chapter 2',
         expected_state='42601' if name.startswith('missing_') else '',
         object_mapping={}, encoding='UTF8', timezone='UTC',
         side_effects='not applicable: SELECT expressions without relations',
         execution='not applicable: parse/analyze handoff only')
    for name, sql, oids in cases], indent=2))


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
