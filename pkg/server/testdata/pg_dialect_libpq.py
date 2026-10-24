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
assert task in (1, 2, 3, 4, 9, 11, 13, 14, 16), 'unregistered dialect task'
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
field_table = api('PQftable', c.c_uint, ptr, integer)
field_column = api('PQftablecol', integer, ptr, integer)
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
if task == 11:
    cases = [
        ('two', 'SELECT ROW(1,2)', []),
        ('one', 'SELECT ROW(1)', []),
        ('nested', 'SELECT ROW(1,ROW(2,3))', []),
        ('parameters', 'SELECT ROW(NULL,$2,$1,$2)', [23,23]),
        ('missing_close', '/*中*/ SELECT ROW(1,', []),
        ('protected', "SELECT 'ROW(1,2)' /* ROW() $2 */", []),
    ]
if task == 13:
    identifier_table = 't13_' + evidence.name.replace('-', '_')
    relation = '"public"."' + identifier_table + '"'
    cases = [
        ('mixed_case', 'SELECT foo, Foo, "Foo", "a""b" FROM '+relation, []),
        ('lower', 'SELECT foo AS v FROM '+relation, []),
        ('folded', 'SELECT Foo AS v FROM '+relation, []),
        ('quoted', 'SELECT "foo" AS v FROM '+relation, []),
        ('escaped', 'SELECT "a""b" AS v FROM '+relation, []),
        ('protected', 'SELECT \'Foo "a""b"\' AS v /* "Foo" */ -- "bad\n', []),
        ('parameters', 'SELECT $2::int4 AS v, $1::int4 AS w FROM '+relation+' WHERE foo=1', [23,23]),
        ('empty', '/*中*/ SELECT ""', []),
        ('unclosed', '/*中*/ SELECT "broken', []),
    ]
if task == 14:
    relation_table = 't14_' + evidence.name.replace('-', '_')
    other_database = sys.argv[3] + '_other'
    cases = [
        ('unqualified', 'SELECT id FROM @t@', []),
        ('schema', 'SELECT id FROM public.@t@', []),
        ('database', 'SELECT id FROM @db@.public.@t@', []),
        ('foreign', 'SELECT id FROM @other@.public.@t@', []),
        ('schema_gate', 'SELECT id FROM @other@.@t@', []),
        ('trailing', '/*中*/ SELECT id FROM public.', []),
        ('excess', '/*中*/ SELECT id FROM @db@.public.@t@.extra', []),
        ('quoted_current', 'SELECT id FROM "@db@"."public"."@t@"', []),
        ('quoted_foreign', 'SELECT id FROM "@DB@".public.@t@', []),
        ('protected', "SELECT 'other.public.t' AS v FROM public.@t@ /* @db@.public.t.extra $2 */", []),
        ('parameters', 'SELECT $2::int4 AS v, $1::int4 AS w FROM @db@.public.@t@ WHERE id=1', [23,23]),
    ]
if task == 16:
    alias_table='t16_'+evidence.name.replace('-','_')
    cases=[
        ('alias','SELECT a.id FROM public.@t@ AS a',[]),
        ('hidden','SELECT @t@.id FROM public.@t@ AS a',[]),
        ('missing','SELECT a.missing FROM public.@t@ AS a',[]),
        ('self_join','SELECT a.id AS left_id,b.id AS right_id FROM public.@t@ a JOIN public.@t@ b ON a.id=b.id',[]),
        ('quoted','SELECT "A".id FROM public.@t@ AS "A"',[]),
    ]
assert len({case[0] for case in cases}) == (5 if task == 16 else 11 if task == 14 else (9 if task == 13 else (6 if task in (9,11) else (8 if task == 4 else 10))))
(evidence / 'cases.json').write_text(json.dumps([
    dict(case_id=name, capability_id='pg.dialect.row.syntax' if task == 11 else ('pg.dialect.array.syntax' if task == 9 else ('pg.dialect.ilike.syntax' if task == 1 else ('pg.dialect.escape.syntax' if task == 2 else 'pg.dialect.ilike.precedence'))), sql=sql,
         phase='PQprepare', parameter_oids=oids, arguments=((['7','9'] if task in (9,11) else [None]*len(oids)) if oids else []),
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


if task == 13:
    registry=json.loads((evidence/'cases.json').read_text())
    for case in registry:
        name=case['case_id']
        case['capability_id']='pg.dialect.identifier.parts'
        case['arguments']=['7','9'] if name=='parameters' else []
        case['object_mapping']={'aster':'current_database.public.'+identifier_table,'reference':'public.'+identifier_table}
        case['expected_state']=None
        case['expected_by_side']={'aster':'0A000' if name=='mixed_case' else ('42601' if name in ('empty','unclosed') else 'success'),
            'reference':'42703' if name=='mixed_case' else ('42601' if name in ('empty','unclosed') else 'success')}
        case['execution']='Prepare/Describe/Bind/Execute for successful SELECT; Q excludes parameter SQL; mixed-case gate is not exact binding success'
        case['side_effects']='no SELECT writes; independent observer checks fixture before and after; setup and cleanup recorded separately'
    (evidence/'cases.json').write_text(json.dumps(registry,indent=2))


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
                    command_tag=(command_tag(result) or b'').decode(),
                    table_origins=([[field_table(result,i),field_column(result,i)] for i in range(columns(result))] if task in (13,14,16) else None))
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


if task == 11:
    registry = json.loads((evidence/'cases.json').read_text())
    for case in registry:
        name = case['case_id']
        case['expected_by_side'] = {
            'reference': '42601' if name=='missing_close' else 'success',
            'aster': '42601' if name=='missing_close' else ('success' if name=='protected' else '0A000'),
        }
        case['expected_state'] = None
        case['local_expected_state'] = case['expected_by_side']['aster']
        case['reference_expected_state'] = case['expected_by_side']['reference']
        case['execution'] = 'reference row/control Execute; local constructor gate is not record execution'
        case['local_record_values_and_oid'] = 'unavailable: record analysis/codec external dependency' if name not in ('missing_close','protected') else 'not applicable: syntax error or scalar control'
    (evidence/'cases.json').write_text(json.dumps(registry,indent=2))

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

def row_handoff():
    """ROW syntax/gate only; preserve separate reference type-analysis outcomes."""
    connections, prepared = {}, []
    checks = 0
    self_checks = 0
    eligible = 0
    control_descriptors = {}
    metadata_mismatches = 0
    done = False
    cleanup_ok = True
    def record(name, label, data, result):
        allowed = {'phase','sql','statement','parameter_oids','arguments','parameter_formats','result_format','command'}
        archive(name, 'aster' if label.startswith('aster') else label,
                {key:value for key,value in (data or {}).items() if key in allowed},
                dict(result, connection=label, context=data))
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
        return (a['state']=='0A000' and a['status']==7 and not a['position'] and
                a['message']=='ROW constructors require record type analysis support' and
                r['state']=='' and r['status']==1 and
                r['position']=='')
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
        provenance['mysql_reference_build']=json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text())
        (evidence/'provenance.json').write_text(json.dumps(provenance,indent=2))
        check(validate_provenance(provenance,copy.deepcopy(provenance)),'provenance invalid')
        damaged=copy.deepcopy(provenance);damaged['reference_build']['server_version_num']=180004
        check(not validate_provenance(damaged,provenance),'provenance negative self-check');self_checks+=1
        before=query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']
        check(before==[['31']],'initial observer state')
        for index,(name,sql,oids) in enumerate(cases):
            input_data=dict(role={label:connection_user(conn).decode() for label,conn in connections.items()},capability_id='pg.dialect.row.syntax',case_id=name,sql=sql,phase='PQprepare',
                parameter_oids=oids,arguments=['7','9'] if oids else [],parameter_formats=[0]*len(oids),result_format=0,
                encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',object_mapping={},
                expected_basis='task11 syntax/gate contract and exact PostgreSQL18.6 analysis; values/OIDs unavailable on local gated record constructors')
            results={}
            for label in ['reference', 'aster_b' if index%2 else 'aster']:
                conn=connections[label];check(transaction(conn)==0,'initial transaction not idle')
                statement=f'row_{index}'.encode()
                result=observe(prepare(conn,statement,sql.encode(),len(oids),(c.c_uint*len(oids))(*oids) if oids else None))
                result['transaction_after']={0:'I',2:'T',3:'E'}.get(transaction(conn))
                record(name,label,input_data,result);results['reference' if label=='reference' else 'aster']=result
                if result['status']==1:
                    prepared.append((conn,statement))
                    desc=observe(describe(conn,statement));record(name+'_describe',label,dict(input_data,phase='Describe'),desc)
                    if label=='reference' and name in ('two','one','nested','parameters'):
                        check(len(desc['fields'])==1 and desc['fields'][0]==['row',2249,-1,-1], 'reference record metadata')
                        if oids:check(desc['param_count']==2 and desc['params']==[23,23],'reference parameter identity')
                        args=(text*2)(b'7',b'9') if oids else None
                        values=observe(exec_prepared(conn,statement,len(oids),args,None,None,0));record(name+'_execute',label,dict(input_data,phase='Execute'),values)
                        expected={'two':'(1,2)','one':'(1)','nested':'(1,"(2,3)")','parameters':'(,9,7,9)'}[name]
                        check(values['rows']==[[expected]] and values['command_tag']=='SELECT 1','reference record values')
                    if name=='protected':
                        control_descriptors['reference' if label=='reference' else 'aster']=desc
                        check(len(desc['fields'])==1 and desc['fields'][0][1:]==[25,-1,-1] and desc['formats']==[0] and desc['param_count']==0,'scalar control descriptor type/format/parameters')
                        values=observe(exec_prepared(conn,statement,0,None,None,None,0));record(name+'_execute',label,dict(input_data,phase='Execute'),values)
                        check(values['rows']==[['ROW(1,2)']],'protected value')
                recover(conn,label,name)
            check(compare(results['aster'],results['reference'],name,sql),'prepare contract mismatch: '+name)
            if name=='protected':
                same_metadata=compare_metadata(control_descriptors['aster'],control_descriptors['reference'],'describe')
                metadata_mismatches += int(not same_metadata)
                record('protected_metadata_comparison','aster',None,dict(equal=same_metadata,aster=control_descriptors['aster'],reference=control_descriptors['reference'],scope='unnamed scalar label is outside ROW syntax; mismatch is excluded from full effective count'))
            if name in ('two','one','missing_close'):
                damaged=copy.deepcopy(results['aster']);damaged['state']='42601' if name!='missing_close' else '0A000'
                check(not compare(damaged,results['reference'],name,sql),'comparator negative check');self_checks+=1
            if name=='missing_close':
                damaged=copy.deepcopy(results['aster']);damaged['position']='1'
                check(not compare(damaged,results['reference'],name,sql),'position comparator negative check');self_checks+=1
            for label in ['aster','reference']:
                result=query(connections[label],sql);record(name+'_simple',label,dict(input_data,phase='Q',arguments=[],parameter_oids=[]),result)
                if name=='protected':check(result['rows']==[['ROW(1,2)']],'simple literal changed')
                elif name=='missing_close':check(result['state']=='42601' and result['position']==str(len(sql)+1),'simple original position')
                elif label=='aster':check(result['state']=='0A000','simple record gate')
                elif name=='parameters':check(result['state']=='42P02','simple parameter requires binding')
                else:check(result['rows']==[[{'two':'(1,2)','one':'(1)','nested':'(1,"(2,3)")'}[name]]],'simple reference record')
                recover(connections[label],label,name+'_simple')
            for label in (['aster_b','aster'] if index%2==0 else ['aster','aster_b']):
                recover(connections[label],label,name+'_isolation')
            check(query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']==before,'observer state changed')
            print('PG_DIALECT_CHECKPOINT '+name,flush=True)
            check(sys.stdin.readline().strip()=='continue','MySQL checkpoint aborted')
            eligible+=1
        for label in ['aster','reference']:
            conn=connections[label];check(query(conn,'BEGIN')['status']==1,'begin failed')
            result=observe(prepare(conn,b'transaction_bad',b'SELECT ROW(1,',0,None))
            record('transaction_error',label,dict(phase='PQprepare',sql='SELECT ROW(1,'),dict(result,transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conn))))
            check(result['state']=='42601' and transaction(conn)==3,'transaction error state');recover(conn,label,'transaction')
        for phase,invoke in [('Q',lambda sql:query(connections['aster'],sql)),('PQprepare',lambda sql:observe(prepare(connections['aster'],b'row_write',sql.encode(),0,None)))]:
            sql='INSERT INTO public.dialect_isolation VALUES (ROW(47))'
            result=invoke(sql);record('no_write_'+phase,'aster',dict(phase=phase,sql=sql),result)
            check(result['state']=='0A000' and result['message']=='ROW constructors require record type analysis support','reject record DML before write')
            check(query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']==before,'record DML changed observer state')
            recover(connections['aster'],'aster','no_write_'+phase)
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
        env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql):
            proc=subprocess.run(args+['-e',"SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            return dict(code=proc.returncode,output=proc.stdout.splitlines(),error=proc.stderr)
        identity=mysql_ref('SELECT VERSION(), @@version_compile_os, @@version_compile_machine; SELECT @@sql_mode, @@character_set_connection, @@collation_connection')
        record('identity','mysql',None,identity);check(identity['code']==0,'MySQL identity')
        for name,sql in [('two','SELECT ROW(1,2)'),('one','SELECT ROW(1)'),('nested','SELECT ROW(1,ROW(2,3))'),('missing_close','SELECT ROW(1,'),('protected',"SELECT 'ROW(1,2)' /* ROW() $2 */")]:
            result=mysql_ref(sql);record(name,'mysql',dict(phase='Q',sql=sql),result)
            if name=='protected': check(result['code']==0 and result['output']==['ROW(1,2)'],'MySQL protected literal')
            else:
                code='1241 (21000)' if name in ('two','nested') else '1064 (42000)'
                check(result['code']!=0 and 'ERROR '+code in result['error'],'MySQL row-specific projection/syntax diagnostic')
        legal=mysql_ref("SELECT (1); SELECT ROW(1,2)=ROW(1,2); PREPARE s FROM 'SELECT ROW(?,?)=ROW(?,?)'; SET @a=1; SET @b=2; EXECUTE s USING @a,@b,@a,@b; DEALLOCATE PREPARE s")
        record('legal_prepare','mysql',None,legal);check(legal['code']==0 and legal['output']==['1','1','1'],'MySQL row comparison/prepare control')
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
            gate_cases=4 if done and cleanup_ok else 0,unique_effective=(2-metadata_mismatches) if done and cleanup_ok else 0,
            metadata_mismatch_cases=metadata_mismatches,
            semantic_row_success=0,cleanup_ok=cleanup_ok,scope='ROW syntax/gate; local record typing/binding/execute unavailable')
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and cleanup_ok,'incomplete run/cleanup'
    print(json.dumps(summary),flush=True)

def identifiers_handoff():
    """Keep quoted identity gates separate from successful identifier execution."""
    connections, prepared = {}, []
    table_oids, attributes = {}, {}
    checks = self_checks = eligible = semantic = origin_mismatches = 0
    done = False
    cleanup_ok = True
    def check(condition, message):
        nonlocal checks
        assert condition, message
        checks += 1
    def record(name, label, data, result):
        allowed = {'phase','sql','statement','parameter_oids','arguments','parameter_formats','result_format','command'}
        archive(name, 'aster' if label.startswith('aster') else label,
                {key:value for key,value in data.items() if key in allowed},
                dict(result, connection=label, context=data))
    def recover(conn, label, name):
        if transaction(conn) in (2,3):
            check(query(conn,'ROLLBACK')['status']==1,'rollback failed')
        result=query(conn,'SELECT 1::int4')
        record(name+'_recovery',label,dict(phase='Q',sql='SELECT 1::int4'),result)
        check(result['rows']==[['1']] and transaction(conn)==0,'original connection recovery')
    def quote_position(sql):
        return str(sql.index('"')+1)
    def compare(a,r,name,sql):
        if name == 'mixed_case':
            return (a['status']==7 and a['state']=='0A000' and not a['position'] and
                    a['message']=='quoted mixed-case identifiers require exact name binding support' and
                    r['status']==7 and r['state']=='42703' and r['position']==str(sql.index('"Foo"')+1))
        if name in ('empty','unclosed'):
            return all(x['status']==7 and x['state']=='42601' and x['position']==quote_position(sql) for x in (a,r))
        return all(x['status']==1 and x['state']=='' for x in (a,r))
    def compare_origins(a,r,name):
        column={'lower':'foo','folded':'foo','quoted':'foo','escaped':'a"b'}.get(name)
        if column:
            return a==[[table_oids['aster'],attributes['aster'][column]]] and r==[[table_oids['reference'],attributes['reference'][column]]]
        expected=[[0,0],[0,0]] if name=='parameters' else [[0,0]]
        return a==r==expected
    try:
        local=f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
        for label in ['reference','aster','aster_b','aster_observer']:
            conn=connect((os.environ['PG18_REFERENCE_DSN'] if label=='reference' else local).encode())
            connections[label]=conn
            check(bool(conn) and status(conn)==0,label+' connection')
            if label=='reference':check(query(conn,"SET TIME ZONE 'UTC'")['status']==1,'timezone')
            check(parameter_status(conn,b'client_encoding')==b'UTF8' and parameter_status(conn,b'TimeZone')==b'UTC','settings')
            check(query(conn,'SET search_path TO public')['status']==1,'search path')
        ref=connections['reference'];identity=query(ref,"SELECT version(),current_setting('server_version_num')")['rows'][0]
        check(identity[1]=='180006','exact 18.6 required')
        build=json.loads((evidence/'rust-build.json').read_text());build['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance=dict(run=evidence.name,target_version='18.6',source=source_snapshot(root),build=build,
            reference=dict(version=identity[0],server_version_num=identity[1]),reference_build=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text()),
            client=dict(python=sys.version,library=str(library),sha256=hashlib.sha256(library.read_bytes()).hexdigest(),PQlibVersion=version()),
            os=platform.platform(),storage='CreateAnalyzeSession test storage',authentication='InsecureRootOnly',
            endpoints={s:endpoint(connections[s]) for s in ['reference','aster','aster_b']},
            mysql_reference_build=json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text()))
        (evidence/'provenance.json').write_text(json.dumps(provenance,indent=2))
        check(validate_provenance(provenance,copy.deepcopy(provenance)),'provenance')
        bad=copy.deepcopy(provenance);bad['reference_build']['server_version_num']=180004
        check(not validate_provenance(bad,provenance),'provenance negative');self_checks+=1
        for label in ['reference','aster']:
            conn=connections[label]
            for sql in ['CREATE TABLE public.'+identifier_table+' (foo integer, "a""b" integer)',
                        'INSERT INTO public.'+identifier_table+' VALUES (1,3)']:
                result=query(conn,sql);record('setup',label,dict(phase='Q',sql=sql),result)
                check(result['status']==1,'fixture setup '+label)
        for label in ['reference','aster']:
            sql="SELECT oid FROM pg_catalog.pg_class WHERE relname='"+identifier_table+"'"
            result=query(connections[label],sql);record('registered_relation_oid',label,dict(phase='Q',sql=sql),result)
            check(result['status']==2 and len(result['rows'])==1,'registered relation identity')
            table_oids[label]=int(result['rows'][0][0]);check(table_oids[label]>0,'positive catalog OID')
            sql='SELECT attnum,attname FROM pg_catalog.pg_attribute WHERE attrelid='+str(table_oids[label])+' ORDER BY attnum'
            result=query(connections[label],sql);record('registered_attributes',label,dict(phase='Q',sql=sql),result)
            attributes[label]={name:int(number) for number,name in result['rows'] if int(number)>0}
            check(result['status']==2 and attributes[label]=={'foo':1,'a"b':2},'catalog attribute identity')
        initial=query(connections['aster_observer'],'SELECT foo,"a""b" FROM public.'+identifier_table)['rows']
        check(initial==[['1','3']],'observer initial')
        for index,(name,sql,oids) in enumerate(cases):
            data=dict(capability_id='pg.dialect.identifier.parts',case_id=name,phase='PQprepare',sql=sql,
                parameter_oids=oids,arguments=['7','9'] if oids else [],parameter_formats=[0]*len(oids),result_format=0,
                encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',
                role={s:connection_user(connections[s]).decode() for s in ['reference','aster','aster_b']},
                object_mapping={'aster':'current_database.public.'+identifier_table,'reference':'public.'+identifier_table},
                expected_basis='task13 frozen syntax/gate contract, PostgreSQL18.6; exact mixed-case binding belongs to task15')
            results,descriptors,values={},{},{}
            for label in ['reference','aster_b' if index%2 else 'aster']:
                conn=connections[label];check(transaction(conn)==0,'initial idle')
                statement=f'ident_{index}'.encode()
                result=observe(prepare(conn,statement,sql.encode(),len(oids),(c.c_uint*len(oids))(*oids) if oids else None))
                result['transaction_after']={0:'I',2:'T',3:'E'}.get(transaction(conn))
                record(name,label,data,result);side='reference' if label=='reference' else 'aster';results[side]=result
                if result['status']==1:
                    prepared.append((conn,statement))
                    desc=observe(describe(conn,statement));descriptors[side]=desc
                    record(name+'_describe',label,dict(data,phase='Describe'),desc)
                    args=(text*2)(b'7',b'9') if oids else None
                    output=observe(exec_prepared(conn,statement,len(oids),args,None,None,0));values[side]=output
                    record(name+'_execute',label,dict(data,phase='Execute'),output)
                    expected={'lower':[['1']],'folded':[['1']],'quoted':[['1']],'escaped':[['3']],
                              'protected':[['Foo "a""b"']],'parameters':[['9','7']]}.get(name)
                    check(output['status']==2 and output['rows']==expected and output['command_tag']=='SELECT 1','Execute '+label+' '+name)
                    if name=='parameters':
                        null_output=observe(exec_prepared(conn,statement,2,(text*2)(None,b'9'),None,None,0))
                        record(name+'_null_execute',label,dict(data,phase='Execute',arguments=[None,'9']),null_output)
                        check(null_output['rows']==[['9',None]] and null_output['fields']==output['fields'] and null_output['command_tag']=='SELECT 1','typed NULL identity')
                recover(conn,label,name)
            check(compare(results['aster'],results['reference'],name,sql),'identifier prepare contract: '+name)
            damaged=copy.deepcopy(results['aster']);damaged['state']='XX000'
            check(not compare(damaged,results['reference'],name,sql),'comparison negative');self_checks+=1
            if name in ('empty','unclosed'):
                damaged=copy.deepcopy(results['aster']);damaged['position']='1'
                check(not compare(damaged,results['reference'],name,sql),'position negative');self_checks+=1
            if descriptors:
                check(compare_metadata(descriptors['aster'],descriptors['reference'],'describe'),'Describe metadata '+name)
                check(descriptors['aster']['params']==oids and descriptors['aster']['param_count']==len(oids),'parameter identity')
                check(descriptors['aster']['formats']==[0]*len(descriptors['aster']['fields']),'text description')
                check(values['aster']['fields']==values['reference']['fields'] and values['aster']['formats']==values['reference']['formats'],'Execute metadata')
                damaged=copy.deepcopy(descriptors['aster']);damaged['fields'][0][1]=99999
                check(not compare_metadata(damaged,descriptors['reference'],'describe'),'metadata negative');self_checks+=1
                # Relation OIDs differ between servers; only compare mapped column
                # origins after confirming both sides actually supplied them.
                a_origins=descriptors['aster']['table_origins'];r_origins=descriptors['reference']['table_origins']
                origin_equal=compare_origins(a_origins,r_origins,name)
                damaged=copy.deepcopy(a_origins);damaged[0][0]+=1
                check(not compare_origins(damaged,r_origins,name),'origin OID negative');self_checks+=1
                if name in ('lower','folded','quoted','escaped'):
                    damaged=copy.deepcopy(a_origins);damaged[0][1]+=1
                    check(not compare_origins(damaged,r_origins,name),'origin attribute negative');self_checks+=1
                check(compare_origins(values['aster']['table_origins'],values['reference']['table_origins'],name),'Execute origin identity')
                origin_mismatches+=int(not origin_equal)
                record(name+'_origin_comparison','aster',data,dict(equal=origin_equal,aster=a_origins,reference=r_origins,
                    scope='zero relation OID cannot stand for an available physical column origin; mismatch excluded from full effective count'))
                semantic+=1
            if not oids:
                simple_origins={}
                for label in ['aster','reference']:
                    result=query(connections[label],sql);record(name+'_simple',label,dict(data,phase='Q'),result)
                    if name in ('mixed_case','empty','unclosed'):
                        expected_state='0A000' if name=='mixed_case' and label=='aster' else ('42703' if name=='mixed_case' else '42601')
                        check(result['state']==expected_state,'Q diagnostic '+name)
                        if name!='mixed_case':check(result['position']==quote_position(sql),'Q original character position')
                    else:check(result['rows']==values[label]['rows'] and result['fields']==values[label]['fields'],'Q value/metadata '+name)
                    simple_origins[label]=result['table_origins']
                    recover(connections[label],label,name+'_simple')
                if name not in ('mixed_case','empty','unclosed'):check(compare_origins(simple_origins['aster'],simple_origins['reference'],name),'Q origin identity')
            for label in (['aster_b','aster'] if index%2==0 else ['aster','aster_b']):recover(connections[label],label,name+'_isolation')
            check(query(connections['aster_observer'],'SELECT foo,"a""b" FROM public.'+identifier_table)['rows']==initial,'observer unchanged')
            print('PG_DIALECT_CHECKPOINT '+name,flush=True);check(sys.stdin.readline().strip()=='continue','MySQL checkpoint')
            eligible+=1
        for label in ['aster','reference']:
            conn=connections[label];check(query(conn,'BEGIN')['status']==1,'begin')
            sql='SELECT ""';result=observe(prepare(conn,b'bad_quote_tx',sql.encode(),0,None))
            record('transaction_error',label,dict(phase='PQprepare',sql=sql),dict(result,transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conn))))
            check(result['state']=='42601' and transaction(conn)==3,'transaction error');recover(conn,label,'transaction')
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN']);env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql):
            p=subprocess.run(args+['-e',"SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            return dict(code=p.returncode,output=p.stdout.splitlines(),error=p.stderr)
        result=mysql_ref('SELECT VERSION(),@@version_compile_os,@@version_compile_machine; SELECT @@sql_mode,@@character_set_connection,@@collation_connection,@@lower_case_table_names')
        record('identity','mysql',{},result);check(result['code']==0,'MySQL identity')
        for mode in ['', 'ANSI_QUOTES']:
            result=mysql_ref("SET SESSION sql_mode='"+mode+"'; SELECT \"Foo\"")
            record('double_quote_'+mode,'mysql',dict(phase='Q',sql='SELECT "Foo"',sql_mode=mode),result)
            check((result['code']==0 and result['output']==['Foo']) if not mode else (result['code']!=0 and '1054 (42S22)' in result['error']),'MySQL mode separation')
        result=mysql_ref("SELECT 1 AS foo; PREPARE s FROM 'SELECT ? AS v'; SET @v=31; EXECUTE s USING @v; DEALLOCATE PREPARE s")
        record('prepare_control','mysql',{},result);check(result['code']==0 and result['output']==['1','31'],'MySQL prepare control')
        final=copy.deepcopy(provenance);final['source']=source_snapshot(root);final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        check(validate_provenance(provenance,final),'source/build changed')
        done=True
    finally:
        for conn,statement in prepared:
            result=observe(close_prepared(conn,statement));cleanup_ok=result['status']==1 and cleanup_ok
        for conn in connections.values():
            if conn and transaction(conn) in (2,3):cleanup_ok=query(conn,'ROLLBACK')['status']==1 and cleanup_ok
        for label in ['reference','aster']:
            if label in connections and status(connections[label])==0:
                result=query(connections[label],'DROP TABLE IF EXISTS public.'+identifier_table)
                record('cleanup',label,dict(phase='Q',sql='DROP TABLE IF EXISTS public.'+identifier_table),result)
                cleanup_ok=result['status']==1 and cleanup_ok
        for conn in connections.values():
            if conn:finish(conn)
        summary=dict(cases=len(cases),assertions=checks,self_checks=self_checks,syntax_eligible=eligible if done and cleanup_ok else 0,
            gate_cases=1 if done and cleanup_ok else 0,value_type_cases=semantic if done and cleanup_ok else 0,
            unique_effective=eligible-1-origin_mismatches if done and cleanup_ok else 0,origin_metadata_mismatch_cases=origin_mismatches,cleanup_ok=cleanup_ok,
            scope='identifier syntax and lowercase/escaped execution; mixed-case exact binding remains explicitly gated')
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and cleanup_ok,'incomplete run/cleanup'
    print(json.dumps(summary),flush=True)

def relations_handoff():
    connections, prepared, created = {}, [], []
    checks = self_checks = eligible = semantic = gates = 0
    done = False
    cleanup_ok = True
    def check(condition, message):
        nonlocal checks
        assert condition, message
        checks += 1
    def record(name, label, data, result):
        if label=='mysql' and not data and 'sql' in result:data=dict(phase='Q',sql=result['sql'])
        allowed={'phase','sql','statement','parameter_oids','arguments','parameter_formats','result_format','command'}
        archive(name, 'aster' if label.startswith('aster') else label, {k:v for k,v in data.items() if k in allowed}, dict(result, connection=label, context=data))
    def recover(conn, label, name):
        if transaction(conn) in (2,3):
            check(query(conn,'ROLLBACK')['status']==1,'rollback failed')
        result=query(conn,'SELECT 1::int4')
        record(name+'_recovery',label,dict(phase='Q',sql='SELECT 1::int4'),result)
        check(result['rows']==[['1']] and transaction(conn)==0,'original connection recovery')
    def expected(name, label, sql):
        if name=='schema_gate':return ('0A000','') if label=='aster' else ('42P01',str(sql.index('FROM ')+6))
        if name in ('foreign','quoted_foreign'):return '0A000',str(sql.index('FROM ')+6)
        if name=='trailing':return '42601',str(len(sql)+1)
        if name=='excess':return '42601',str(sql.index('FROM ')+6)
        return '', ''
    def diagnostics(result,name,label,sql,prepare_phase):
        state,position=expected(name,label,sql)
        return result['status']==(7 if state else (1 if prepare_phase else 2)) and result['state']==state and result['position']==position
    def origins(actual, reference, expected_column):
        return actual==([[oids['aster'],1]] if expected_column else [[0,0]]*len(actual)) and reference==([[oids['reference'],1]] if expected_column else [[0,0]]*len(reference))
    def sql_for(template, db):
        return template.replace('@DB@',db.upper()).replace('@db@',db).replace('@other@',other_database).replace('@t@',relation_table)
    oids={}
    try:
        local=f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
        for label in ['reference','aster','aster_b','aster_observer']:
            conn=connect((os.environ['PG18_REFERENCE_DSN'] if label=='reference' else local).encode())
            connections[label]=conn
            check(bool(conn) and status(conn)==0,label+' connection')
            if label=='reference':check(query(conn,"SET TIME ZONE 'UTC'")['status']==1,'timezone')
            check(parameter_status(conn,b'client_encoding')==b'UTF8' and parameter_status(conn,b'TimeZone')==b'UTC','settings')
            check(query(conn,'SET search_path TO public')['status']==1,'search path')
        ref=connections['reference'];identity=query(ref,"SELECT version(),current_setting('server_version_num'),current_database()")['rows'][0]
        check(identity[1]=='180006','exact 18.6 required')
        databases={'aster':sys.argv[3],'reference':identity[2]}
        build=json.loads((evidence/'rust-build.json').read_text());build['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance=dict(run=evidence.name,target_version='18.6',source=source_snapshot(root),build=build,
            reference=dict(version=identity[0],server_version_num=identity[1]),reference_build=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text()),
            client=dict(python=sys.version,library=str(library),sha256=hashlib.sha256(library.read_bytes()).hexdigest(),PQlibVersion=version()),
            os=platform.platform(),storage='CreateAnalyzeSession test storage',authentication='InsecureRootOnly',
            endpoints={s:endpoint(connections[s]) for s in ['reference','aster','aster_b']},
            mysql_reference_build=json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text()))
        (evidence/'provenance.json').write_text(json.dumps(provenance,indent=2))
        check(validate_provenance(provenance,copy.deepcopy(provenance)),'provenance')
        bad=copy.deepcopy(provenance);bad['reference_build']['server_version_num']=180004
        check(not validate_provenance(bad,provenance),'provenance negative');self_checks+=1
        for label in ['reference','aster']:
            conn=connections[label]
            sql='CREATE TABLE public.'+relation_table+' (id integer)'
            result=query(conn,sql);record('setup',label,dict(phase='Q',sql=sql),result)
            check(result['status']==1,'create fixture '+label);created.append(label)
            check(query(conn,'INSERT INTO public.'+relation_table+' VALUES (1)')['status']==1,'insert fixture')
            oid_sql="SELECT oid FROM pg_catalog.pg_class WHERE relname='"+relation_table+"'"
            result=query(conn,oid_sql);record('relation_oid',label,dict(phase='Q',sql=oid_sql),result)
            check(result['status']==2 and len(result['rows'])==1,'registered relation OID');oids[label]=int(result['rows'][0][0])
        # Distinct native database with the same table and value=2 detects prefix truncation.
        foreign=connect((local+' dbname='+other_database).encode());connections['aster_other']=foreign
        check(status(foreign)==0,'foreign fixture connection')
        check(query(foreign,'CREATE TABLE public.'+relation_table+' (id integer)')['status']==1,'foreign table')
        check(query(foreign,'INSERT INTO public.'+relation_table+' VALUES (2)')['status']==1,'foreign value')
        # PostgreSQL cross-database rejection is independent of foreign contents.
        check(query(ref,'CREATE DATABASE '+other_database)['status']==1,'reference foreign database');created.append('reference_other')
        reference_other=connect((os.environ['PG18_REFERENCE_DSN']+' dbname='+other_database).encode());connections['reference_other']=reference_other
        check(status(reference_other)==0,'reference foreign connect')
        check(query(reference_other,'CREATE TABLE public.'+relation_table+' (id integer)')['status']==1,'reference foreign table')
        check(query(reference_other,'INSERT INTO public.'+relation_table+' VALUES (2)')['status']==1,'reference foreign value')
        registry=[]
        for index,(name,template,param_oids) in enumerate(cases):
            inputs={side:dict(capability_id='pg.dialect.relation.qualified',case_id=name,phase='PQprepare',sql=sql_for(template,db),
                parameter_oids=param_oids,arguments=['7','9'] if param_oids else [],parameter_formats=[0]*len(param_oids),result_format=0,
                encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',role=connection_user(connections[side]).decode(),
                object_mapping={'database':db,'schema':'public','table':relation_table,'foreign_database':other_database,'current_value':1,'foreign_value':2},
                expected_state=expected(name,side,sql_for(template,db)),expected_basis='task14 PostgreSQL18.6 exact reference; non-public schema remains explicit capability gate') for side,db in databases.items()}
            registry.append(dict(case_id=name,by_side=inputs));(evidence/'cases.json').write_text(json.dumps(registry,indent=2))
            outputs={};descriptors={};values={}
            selected='aster_b' if index%2 else 'aster'
            for label in ['reference',selected]:
                side='reference' if label=='reference' else 'aster';conn=connections[label];data=inputs[side];sql=data['sql']
                check(transaction(conn)==0,'initial idle')
                statement=f'relation_{index}'.encode()
                result=observe(prepare(conn,statement,sql.encode(),len(param_oids),(c.c_uint*len(param_oids))(*param_oids) if param_oids else None))
                record(name,label,data,dict(result,transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conn))));outputs[side]=result
                check(diagnostics(result,name,side,sql,True),'Prepare diagnostic '+name+' '+label)
                if result['status']==1:
                    prepared.append((conn,statement));desc=observe(describe(conn,statement));descriptors[side]=desc
                    record(name+'_describe',label,dict(data,phase='Describe'),desc)
                    args=(text*2)(b'7',b'9') if param_oids else None
                    output=observe(exec_prepared(conn,statement,len(param_oids),args,None,None,0));values[side]=output
                    record(name+'_execute',label,dict(data,phase='Bind/Execute'),output)
                    rows_expected=[['9','7']] if param_oids else ([['other.public.t']] if name=='protected' else [['1']])
                    check(output['status']==2 and output['rows']==rows_expected and output['command_tag']=='SELECT 1','Execute '+name)
                    if param_oids:
                        null_output=observe(exec_prepared(conn,statement,2,(text*2)(None,b'9'),None,None,0))
                        record(name+'_null',label,dict(data,phase='Bind/Execute',arguments=[None,'9']),null_output)
                        check(null_output['rows']==[['9',None]] and null_output['fields']==output['fields'],'NULL binding')
                recover(conn,label,name)
                if not param_oids:
                    simple=query(conn,sql);record(name+'_simple',label,dict(data,phase='Q'),simple)
                    check(diagnostics(simple,name,side,sql,False),'Q diagnostic '+name+' '+label)
                    if not expected(name,side,sql)[0]:check(simple['rows']==values[side]['rows'] and simple['fields']==values[side]['fields'] and simple['table_origins']==values[side]['table_origins'],'Q metadata/value/origin')
                    recover(conn,label,name+'_simple')
            if descriptors:
                a,r=descriptors['aster'],descriptors['reference']
                check(compare_metadata(a,r,'describe'),'Describe metadata '+name)
                check(a['params']==param_oids and a['param_count']==len(param_oids),'parameter identity')
                check(a['formats']==[0]*len(a['fields']),'text format')
                check(values['aster']['fields']==values['reference']['fields'] and values['aster']['rows']==values['reference']['rows'],'Execute differential')
                check(origins(a['table_origins'],r['table_origins'],name not in ('parameters','protected')),'mapped column origins')
                check(origins(values['aster']['table_origins'],values['reference']['table_origins'],name not in ('parameters','protected')),'Execute mapped origins')
                bad=copy.deepcopy(a);bad['fields'][0][1]=99999
                check(not compare_metadata(bad,r,'describe'),'metadata negative');self_checks+=1
                bad=copy.deepcopy(values['aster']);bad['rows']=[['2']]
                check(bad['rows']!=values['reference']['rows'],'foreign value negative');self_checks+=1
                semantic+=1
            else:
                bad=copy.deepcopy(outputs['aster']);bad['state']='XX000'
                check(not diagnostics(bad,name,'aster',inputs['aster']['sql'],True),'state negative');self_checks+=1
                if name!='schema_gate':
                    bad=copy.deepcopy(outputs['aster']);bad['position']='1'
                    check(not diagnostics(bad,name,'aster',inputs['aster']['sql'],True),'position negative');self_checks+=1
            gates+=int(name=='schema_gate')
            print('PG_DIALECT_CHECKPOINT '+name,flush=True);check(sys.stdin.readline().strip()=='continue','MySQL checkpoint')
            for label in (['aster_b','aster'] if index%2==0 else ['aster','aster_b']):recover(connections[label],label,name+'_isolation')
            for observer,connection,want in [('aster_observer',connections['aster_observer'],'1'),('aster_other',foreign,'2')]:
                sql='SELECT id FROM public.'+relation_table;observed=query(connection,sql)
                record(name+'_observer',observer,dict(phase='Q',sql=sql),observed)
                check(observed['rows']==[[want]],'observer unchanged '+observer)
            eligible+=int(name!='schema_gate')
        for label in ['aster','reference']:
            conn=connections[label];check(query(conn,'BEGIN')['status']==1,'begin')
            sql='SELECT id FROM '+other_database+'.public.'+relation_table
            result=observe(prepare(conn,b'foreign_tx',sql.encode(),0,None));record('transaction_error',label,dict(phase='PQprepare',sql=sql),dict(result,transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conn))))
            check(result['state']=='0A000' and transaction(conn)==3,'failed transaction');recover(conn,label,'transaction')
        # MySQL's own database.table semantics, actual version/build and prepare.
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN']);env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql):
            p=subprocess.run(args+['-e',"SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            return dict(sql=sql,code=p.returncode,output=p.stdout.splitlines(),error=p.stderr)
        mysql_db='ref14_'+evidence.name.replace('-','_')
        try:
            result=mysql_ref('SELECT VERSION(),@@version_compile_os,@@version_compile_machine; SELECT @@sql_mode,@@character_set_connection,@@collation_connection; CREATE DATABASE '+mysql_db+'; CREATE DATABASE '+mysql_db+'_other; CREATE TABLE '+mysql_db+'.t(id INT); INSERT INTO '+mysql_db+'.t VALUES (1); CREATE TABLE '+mysql_db+'_other.t(id INT); INSERT INTO '+mysql_db+'_other.t VALUES (2)')
            record('identity_fixture','mysql',{},result);check(result['code']==0,'MySQL fixture')
            result=mysql_ref('USE '+mysql_db+'; SELECT id FROM t; SELECT id FROM '+mysql_db+'_other.t; PREPARE s FROM \'SELECT id FROM '+mysql_db+'_other.t WHERE id=?\'; SET @v=2; EXECUTE s USING @v; DEALLOCATE PREPARE s')
            record('database_prepare','mysql',{},result);check(result['code']==0 and result['output']==['1','2','2'],'MySQL database semantics')
            for sql in ['SELECT id FROM '+mysql_db+'.public.t','SELECT id FROM '+mysql_db+'.t.extra','SELECT id FROM '+mysql_db+'.']:
                result=mysql_ref(sql);record('syntax','mysql',dict(phase='Q',sql=sql),result)
                check(result['code']!=0 and '1064 (42000)' in result['error'],'MySQL own syntax diagnostic')
        finally:
            result=mysql_ref('DROP DATABASE IF EXISTS '+mysql_db+'; DROP DATABASE IF EXISTS '+mysql_db+'_other');record('cleanup','mysql',{},result)
            cleanup_ok=result['code']==0 and cleanup_ok
        final=copy.deepcopy(provenance);final['source']=source_snapshot(root);final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        check(validate_provenance(provenance,final),'source/build changed');done=True
    finally:
        for conn,statement in prepared:
            cleanup_ok=observe(close_prepared(conn,statement))['status']==1 and cleanup_ok
        for conn in connections.values():
            if conn and transaction(conn) in (2,3):cleanup_ok=query(conn,'ROLLBACK')['status']==1 and cleanup_ok
        for label in ['aster','reference']:
            if label in created:
                result=query(connections[label],'DROP TABLE IF EXISTS public.'+relation_table);record('cleanup',label,dict(phase='Q',sql='DROP TABLE IF EXISTS public.'+relation_table),result)
                cleanup_ok=result['status']==1 and cleanup_ok
        for conn in connections.values():
            if conn:finish(conn)
        if 'reference_other' in created:
            ref=connect(os.environ['PG18_REFERENCE_DSN'].encode())
            sql='DROP DATABASE '+other_database;result=query(ref,sql)
            record('cleanup_database','reference',dict(phase='Q',sql=sql),result)
            cleanup_ok=result['status']==1 and cleanup_ok;finish(ref)
        summary=dict(cases=len(cases),assertions=checks,self_checks=self_checks,unique_effective=eligible if done and cleanup_ok else 0,
            value_type_cases=semantic if done and cleanup_ok else 0,gate_cases=gates if done and cleanup_ok else 0,cleanup_ok=cleanup_ok)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and cleanup_ok,'incomplete run/cleanup'
    print(json.dumps(summary),flush=True)

def aliases_handoff():
    conns,prepared,created={ },[],[]
    checks=self_checks=eligible=0
    done=False;clean=True
    def check(ok,message):
        nonlocal checks
        assert ok,message
        checks+=1
    def record(name,label,data,result):
        archive(name,'aster' if label.startswith('aster') else label,{k:v for k,v in data.items() if k in {'phase','sql','statement','parameter_oids','arguments','parameter_formats','result_format','command'}},dict(result,connection=label,context=data,transaction_after=({0:'I',2:'T',3:'E'}.get(transaction(conns[label])) if label in conns else None)))
    def recover(label,name):
        conn=conns[label]
        if transaction(conn) in (2,3):check(query(conn,'ROLLBACK')['status']==1,'rollback')
        result=query(conn,'SELECT 1::int4');record(name+'_recovery',label,dict(phase='Q',sql='SELECT 1::int4'),result)
        check(result['rows']==[['1']] and transaction(conn)==0,'same connection recovery '+label)
    try:
        local=f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
        for label in ['reference','aster','aster_b','aster_observer']:
            conn=connect((os.environ['PG18_REFERENCE_DSN'] if label=='reference' else local).encode());conns[label]=conn
            check(bool(conn) and status(conn)==0,'connect '+label)
            if label=='reference':check(query(conn,"SET TIME ZONE 'UTC'")['status']==1,'timezone')
            check(query(conn,'SET search_path TO public')['status']==1,'search path')
            check(parameter_status(conn,b'client_encoding')==b'UTF8' and parameter_status(conn,b'TimeZone')==b'UTC','encoding/timezone')
        identity=query(conns['reference'],"SELECT version(),current_setting('server_version_num')")['rows'][0]
        check(identity[1]=='180006','exact reference')
        build=json.loads((evidence/'rust-build.json').read_text());build['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance=dict(run=evidence.name,target_version='18.6',source=source_snapshot(root),build=build,
            reference=dict(version=identity[0],server_version_num=identity[1]),reference_build=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text()),
            client=dict(python=sys.version,library=str(library),sha256=hashlib.sha256(library.read_bytes()).hexdigest(),PQlibVersion=version()),
            os=platform.platform(),endpoints={s:endpoint(conns[s]) for s in conns},storage='CreateAnalyzeSession test storage',authentication='InsecureRootOnly',
            mysql_reference_build=json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text()))
        (evidence/'provenance.json').write_text(json.dumps(provenance,indent=2))
        check(validate_provenance(provenance,copy.deepcopy(provenance)),'provenance')
        bad=copy.deepcopy(provenance);bad['reference_build']['server_version_num']=180004
        check(not validate_provenance(bad,provenance),'provenance negative');self_checks+=1
        for label in ['reference','aster']:
            sql='CREATE TABLE public.'+alias_table+'(id integer)';result=query(conns[label],sql)
            record('setup',label,dict(phase='Q',sql=sql),result);check(result['status']==1,'create fixture');created.append(label)
            check(query(conns[label],'INSERT INTO public.'+alias_table+' VALUES (1)')['status']==1,'insert fixture')
        oids={}
        for label in ['aster','reference']:
            sql="SELECT oid FROM pg_catalog.pg_class WHERE relname='"+alias_table+"'"
            result=query(conns[label],sql);record('relation_oid',label,dict(phase='Q',sql=sql),result)
            check(result['status']==2 and len(result['rows'])==1,'physical OID');oids[label]=int(result['rows'][0][0])
        sql='SELECT id FROM public.'+alias_table;initial=query(conns['aster_observer'],sql)
        record('observer_initial','aster_observer',dict(phase='Q',sql=sql),initial);check(initial['rows']==[['1']],'initial observer')
        registry=[]
        for index,(name,template,param_oids) in enumerate(cases):
            sql=template.replace('@t@',alias_table)
            expected_state={'hidden':'42P01','missing':'42703'}.get(name,'')
            expected_rows=[['1','1']] if name=='self_join' else [['1']]
            expected_position=str(sql.index(alias_table if name=='hidden' else 'a.missing')+1) if expected_state else ''
            data=dict(capability_id='pg.dialect.alias.scope',case_id=name,phase='PQprepare',sql=sql,parameter_oids=[],arguments=[],parameter_formats=[],result_format=0,
                encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',object_mapping={'aster':'current_database.public.'+alias_table,'reference':'public.'+alias_table},
                expected_basis='task16 exact PostgreSQL18.6 alias binding',expected_state=expected_state,expected_position=expected_position,
                roles={s:connection_user(conns[s]).decode() for s in ['aster','reference']})
            registry.append(data);(evidence/'cases.json').write_text(json.dumps(registry,indent=2))
            results={};descs={}
            order=['aster','reference'] if index%2==0 else ['reference','aster_b']
            for label in order:
                side='reference' if label=='reference' else 'aster';conn=conns[label]
                check(transaction(conn)==0,'initial idle')
                statement=('alias_'+str(index)).encode()
                result=observe(prepare(conn,statement,sql.encode(),0,None));record(name,label,data,result)
                check(result['state']==expected_state and result['status']==(7 if expected_state else 1),'prepare '+name+' '+label+': '+str(result))
                if result['status']==1:prepared.append((conn,statement))
                if expected_state:
                    check(result['position']==expected_position,'original prepare position '+name+' '+label)
                    bad=copy.deepcopy(result);bad['state']='XX000';check(bad['state']!=expected_state,'state negative');self_checks+=1
                else:
                    desc=observe(describe(conn,statement));descs[side]=desc
                    record(name+'_describe',label,dict(data,phase='Describe'),desc)
                    result=observe(exec_prepared(conn,statement,0,None,None,None,0));results[side]=result
                    record(name+'_execute',label,dict(data,phase='Bind/Execute'),result)
                    check(result['status']==2 and result['rows']==expected_rows and result['command_tag']=='SELECT 1','execute '+name)
                recover(label,name)
                result=query(conn,sql);record(name+'_simple',label,dict(data,phase='Q'),result)
                check(result['state']==expected_state and result['status']==(7 if expected_state else 2),'Q diagnostic '+name)
                if expected_state:check(result['position']==expected_position,'Q position '+name)
                else:check(result['rows']==expected_rows and result['fields']==results[side]['fields'],'Q rows/metadata')
                recover(label,name+'_simple')
            if descs:
                check(compare_metadata(descs['aster'],descs['reference'],'describe'),'Describe differential '+name)
                check(descs['aster']['params']==[] and descs['aster']['formats']==[0]*len(expected_rows[0]),'params/format')
                for side in ['aster','reference']:
                    expected_origins=[[oids[side],1]]*len(expected_rows[0])
                    check(descs[side]['table_origins']==expected_origins and results[side]['table_origins']==expected_origins,'actual mapped physical origins '+name+' '+side)
                check(results['aster']['fields']==results['reference']['fields'] and results['aster']['rows']==results['reference']['rows'],'execution differential')
                bad=copy.deepcopy(descs['aster']);bad['fields'][0][1]=99999
                check(not compare_metadata(bad,descs['reference'],'describe'),'metadata negative');self_checks+=1
            print('PG_DIALECT_CHECKPOINT '+name,flush=True);check(sys.stdin.readline().strip()=='continue','MySQL checkpoint')
            for label in (['aster_b','aster'] if index%2==0 else ['aster','aster_b']):recover(label,name+'_isolation')
            sql='SELECT id FROM public.'+alias_table;result=query(conns['aster_observer'],sql)
            record(name+'_observer','aster_observer',dict(phase='Q',sql=sql),result);check(result['rows']==initial['rows'],'observer unchanged')
            eligible+=1
        parameter_results={}
        for label in ['aster','reference']:
            conn=conns[label];sql='SELECT a.id,$2::int4 AS p FROM public.'+alias_table+' a WHERE a.id=$1::int4'
            data=dict(phase='PQprepare',sql=sql,parameter_oids=[23,23],arguments=['1','9'],parameter_formats=[0,0],result_format=0)
            result=observe(prepare(conn,b'alias_parameters',sql.encode(),2,(c.c_uint*2)(23,23)))
            record('parameters',label,data,result);check(result['status']==1,'qualified parameter prepare');prepared.append((conn,b'alias_parameters'))
            desc=observe(describe(conn,b'alias_parameters'));record('parameters_describe',label,dict(data,phase='Describe'),desc)
            check(desc['params']==[23,23] and [field[1] for field in desc['fields']]==[23,23],'explicit parameter identity')
            for argument,want in [(b'9','9'),(None,None)]:
                result=observe(exec_prepared(conn,b'alias_parameters',2,(text*2)(b'1',argument),None,None,0))
                record('parameters_null' if argument is None else 'parameters_execute',label,dict(data,phase='Bind/Execute',arguments=['1',want]),result)
                check(result['status']==2 and result['rows']==[['1',want]],'qualified value/NULL parameters')
            parameter_results[label]=desc
        check(compare_metadata(parameter_results['aster'],parameter_results['reference'],'describe'),'parameter Describe differential')
        for label in ['aster','reference']:
            check(query(conns[label],'BEGIN')['status']==1,'begin transaction')
            sql='SELECT a.missing FROM public.'+alias_table+' a'
            result=observe(prepare(conns[label],b'alias_tx_error',sql.encode(),0,None))
            record('transaction_error',label,dict(phase='PQprepare',sql=sql),result)
            check(result['state']=='42703' and transaction(conns[label])==3,'transaction becomes failed');recover(label,'transaction')
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN']);env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql):
            p=subprocess.run(args+['-e',"SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            return dict(sql=sql,code=p.returncode,output=p.stdout.splitlines(),error=p.stderr)
        mysql_db='ref16_'+evidence.name.replace('-','_')
        try:
            result=mysql_ref('SELECT VERSION(),@@version_compile_os,@@version_compile_machine; SELECT @@sql_mode,@@character_set_connection,@@collation_connection; CREATE DATABASE '+mysql_db+'; CREATE TABLE '+mysql_db+'.t(id INT); INSERT INTO '+mysql_db+'.t VALUES(1)')
            record('mysql_identity','mysql',{},result);check(result['code']==0,'MySQL fixture')
            for name,sql,want in [('alias','SELECT a.id FROM '+mysql_db+'.t a',['1']),('self_join','SELECT a.id,b.id FROM '+mysql_db+'.t a JOIN '+mysql_db+'.t b ON a.id=b.id',['1\t1'])]:
                result=mysql_ref(sql);record(name,'mysql',dict(phase='Q',sql=sql),result);check(result['code']==0 and result['output']==want,'MySQL own alias')
            sql="PREPARE s FROM 'SELECT a.id FROM "+mysql_db+".t a WHERE a.id=?'; SET @v=1; EXECUTE s USING @v; DEALLOCATE PREPARE s"
            result=mysql_ref(sql);record('alias_prepare','mysql',dict(phase='PREPARE/EXECUTE',sql=sql),result);check(result['code']==0 and result['output']==['1'],'MySQL alias prepare')
            for sql in ['SELECT t.id FROM '+mysql_db+'.t a','SELECT a.missing FROM '+mysql_db+'.t a']:
                result=mysql_ref(sql);record('negative','mysql',dict(phase='Q',sql=sql),result);check(result['code']!=0 and '1054 (42S22)' in result['error'],'MySQL own diagnostic')
        finally:
            result=mysql_ref('DROP DATABASE IF EXISTS '+mysql_db);record('cleanup','mysql',{},result);clean=result['code']==0 and clean
        final=copy.deepcopy(provenance);final['source']=source_snapshot(root);final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        check(validate_provenance(provenance,final),'source unchanged');done=True
    finally:
        for conn,statement in prepared:clean=observe(close_prepared(conn,statement))['status']==1 and clean
        for conn in conns.values():
            if conn and transaction(conn) in (2,3):clean=query(conn,'ROLLBACK')['status']==1 and clean
        for label in created:
            sql='DROP TABLE IF EXISTS public.'+alias_table;result=query(conns[label],sql);record('cleanup',label,dict(phase='Q',sql=sql),result);clean=result['status']==1 and clean
        for conn in conns.values():
            if conn:finish(conn)
        summary=dict(cases=5,assertions=checks,self_checks=self_checks,unique_effective=eligible if done and clean else 0,cleanup_ok=clean)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    check(done and clean,'completion/cleanup')
    print(json.dumps(summary),flush=True)

if task == 16:
    aliases_handoff()
    sys.exit(0)

if task == 14:
    relations_handoff()
    sys.exit(0)

if task == 13:
    identifiers_handoff()
    sys.exit(0)

if task == 11:
    row_handoff()
    sys.exit(0)

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
