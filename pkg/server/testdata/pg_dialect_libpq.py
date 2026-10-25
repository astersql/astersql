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
assert task in (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 24), 'unregistered dialect task'
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
if task == 5:
    cases = [
        ('projection', 'UPDATE public.t AS a SET id=$1 WHERE id=1 RETURNING a.id AS "ID", a.*', []),
        ('zero_rows', 'UPDATE public.t AS a SET id=7 WHERE FALSE RETURNING a.id', []),
        ('from_returning', 'UPDATE public.t AS a SET id=$1 FROM public.s AS b WHERE a.id=b.id RETURNING b.id AS source_id, a.*', []),
        ('missing_projection', 'UPDATE public.t SET id=1 RETURNING', []),
        ('misplaced_where', 'UPDATE public.t SET id=1 RETURNING id WHERE id=1', []),
        ('protected', "UPDATE public.t SET v='RETURNING $1' WHERE id=1 RETURNING id", []),
    ]
if task == 6:
    cases = [
        ('projection', 'DELETE FROM public.t AS a WHERE id=$1 RETURNING a.id, a.*', [23]),
        ('zero_rows', 'DELETE FROM public.t AS a WHERE FALSE RETURNING a.id', []),
        ('typed_null', 'DELETE FROM public.t AS a WHERE id=$1 RETURNING a.id', [23]),
        ('quoted_alias', 'DELETE FROM public.t AS a WHERE id=1 RETURNING a.id AS "Deleted_ID"', []),
        ('star_only', 'DELETE FROM public.t AS a RETURNING *', []),
        ('using', 'DELETE FROM public.t AS a USING public.s AS b WHERE a.id=b.id RETURNING a.id, b.id AS source_id', []),
        ('using_unaliased', 'DELETE FROM public.t USING public.s WHERE public.t.id=public.s.id RETURNING public.t.id, public.s.id AS source_id', []),
        ('missing_projection', 'DELETE FROM public.t WHERE id=1 RETURNING', []),
        ('misplaced_where', 'DELETE FROM public.t RETURNING id WHERE id=1', []),
        ('protected', "DELETE FROM public.t WHERE id=1 /* RETURNING $2 */", []),
    ]
if task == 8:
    cases = [
        ('inference_update_both_predicates', 'INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT (id) WHERE id > 0 DO UPDATE SET v=excluded.v, id=$1 WHERE t.v IS NOT NULL RETURNING id', [23,25]),
        ('constraint_update_predicate', 'INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT ON CONSTRAINT "t_pkey" DO UPDATE SET v=excluded.v WHERE t.v IS NOT NULL RETURNING id', [23,25]),
        ('inference_update_without_action_predicate', 'INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT (id) WHERE id > 0 DO UPDATE SET v=excluded.v RETURNING id', [23,25]),
        ('update_without_target', "INSERT INTO public.t(id,v) VALUES (31,'after') ON CONFLICT DO UPDATE SET v=excluded.v RETURNING id", []),
        ('update_missing_set', "INSERT INTO public.t(id,v) VALUES (31,'after') ON CONFLICT (id) DO UPDATE WHERE v IS NULL RETURNING id", []),
        ('update_empty_assignments', "INSERT INTO public.t(id,v) VALUES (31,'after') ON CONFLICT (id) DO UPDATE SET WHERE v IS NULL RETURNING id", []),
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
if task == 10:
    cases = [
        ('explicit', 'SELECT ARRAY[ARRAY[1,2],ARRAY[3,4]]', []),
        ('implicit', 'SELECT ARRAY[[1,2],[3,4]]', []),
        ('ragged', 'SELECT ARRAY[[1,2],[3]]', []),
        ('numeric', 'SELECT ARRAY[1,2.5]', []),
        ('mixed_types', '/*中*/ SELECT ARRAY[1,true]', []),
        ('parameters', 'SELECT ARRAY[ARRAY[$2,$1],ARRAY[$1,$2]]', [23,23]),
        ('nulls', 'SELECT ARRAY[[NULL,1],[2,NULL]]', []),
        ('unclosed', '/*中*/ SELECT ARRAY[[1,2],[3,4]', []),
    ]
if task == 11:
    cases = [
        ('two', 'SELECT ROW(1,2)', []),
        ('one', 'SELECT ROW(1)', []),
        ('nested', 'SELECT ROW(1,ROW(2,3))', []),
        ('parameters', 'SELECT ROW(NULL,$2,$1,$2)', [23, 23]),
        ('missing_close', '/*中*/ SELECT ROW(1,', []),
        ('protected', "SELECT 'ROW(1,2)' /* ROW() $2 */", []),
        ('quoted', 'SELECT ROW(\'\',\'a,b\',\'a"b\',\'中 文\',\'NULL\')', []),
        ('null_field', 'SELECT ROW(NULL)', []),
        ('scalar_types', 'SELECT ROW(1.5,true,2147483648)', []),
        ('unknown_parameter', 'SELECT ROW($1)', []),
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
if task == 18:
    cte_table='t18_'+evidence.name.replace('-','_')
    cases=[
        ('shadow','WITH @t@ AS (SELECT 1 AS v) SELECT v FROM @t@',[]),
        ('explicit','WITH @t@ AS (SELECT 1 AS v) SELECT v FROM public.@t@',[]),
        ('ordered','WITH a AS (SELECT 1 AS v), b AS (SELECT v FROM a) SELECT v FROM b',[]),
        ('forward_missing','/*中*/ WITH a AS (SELECT v FROM absent18), absent18 AS (SELECT 2 AS v) SELECT v FROM a',[]),
        ('forward_table','WITH a AS (SELECT v FROM @t@), @t@ AS (SELECT 2 AS v) SELECT v FROM a',[]),
        ('self_table','WITH @t@ AS (SELECT v FROM @t@) SELECT v FROM @t@',[]),
    ]
def conflict_ignore_handoff():
    """Compare conflict syntax and read-only metadata before execution gates."""
    scope = f'dialect_07_{os.getpid()}_{int(sys.argv[2])}'
    local_dsn = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
    reference_dsn = os.environ['PG18_REFERENCE_DSN']
    mysql_dsn = urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
    mysql_env = dict(os.environ, MYSQL_PWD=mysql_dsn.password or '')
    mysql_args = [
        'mysql', '--protocol=TCP', '--connect-timeout=5', '-h', mysql_dsn.hostname,
        '-P', str(mysql_dsn.port or 3306), '-u', mysql_dsn.username or 'root',
        '--batch', '--raw', '--skip-column-names',
    ]
    cases = [
        ('omitted_target', 'valid',
         'INSERT INTO public.t(id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING id', [23]),
        ('inference_predicate', 'valid',
         'INSERT INTO public.t(id) VALUES (2) ON CONFLICT (id) WHERE id > 0 DO NOTHING', []),
        ('constraint_target', 'valid',
         'INSERT INTO public.t(id) VALUES (3) ON CONFLICT ON CONSTRAINT t_pkey DO NOTHING', []),
        ('missing_action', 'invalid',
         '/*中*/ INSERT INTO public.t(id) VALUES (4) ON CONFLICT', []),
        ('empty_target', 'invalid',
         'INSERT INTO public.t(id) VALUES (5) ON CONFLICT () DO NOTHING', []),
        ('target_missing_action', 'invalid',
         'INSERT INTO public.t(id) VALUES (6) ON CONFLICT (id) DO', []),
        ('quoted_conflict_keyword', 'invalid',
         'INSERT INTO public.t(id) VALUES (7) ON "CONFLICT" DO NOTHING', []),
        ('quoted_action_keyword', 'invalid',
         'INSERT INTO public.t(id) VALUES (8) ON CONFLICT DO "NOTHING"', []),
    ]
    connections = {}
    open_statements = []
    failures = []
    checks = 0
    reference_syntax_case_ids = set()
    reference_describe_case_ids = set()
    local_prepare_describe_case_ids = set()
    local_simple_query_gate_case_ids = set()
    local_execute_gate_case_ids = set()
    mysql_reference_rejection_case_ids = set()
    python_effective_case_ids = set()
    case_audit = {}
    cleanup_ok = True
    schema_created = False
    completed = False
    provenance_ok = False
    local_rows_preserved = True
    reference_rows_preserved = True
    registered = []

    def check(condition, message):
        nonlocal checks
        checks += 1
        if not condition:
            failures.append(message)

    def archive(case_id, side, data, result, attempt=0):
        append_observation(evidence, dict(run=evidence.name, case_id=case_id, side=side,
                                          attempt=attempt, input=data, observation=result))

    def mysql_reference(sql, checkpoint=False):
        if checkpoint:
            commands = [
                "SET SESSION sql_mode=''",
                'SET NAMES utf8mb4 COLLATE utf8mb4_general_ci',
                'USE mysql',
                'CREATE TEMPORARY TABLE t (id INT PRIMARY KEY)',
                'INSERT INTO t VALUES (31)',
                "PREPARE task7_legacy FROM 'INSERT INTO t(id) VALUES (31) ON DUPLICATE KEY UPDATE id=VALUES(id)'",
                'DEALLOCATE PREPARE task7_legacy',
                'SHOW COLUMNS FROM t',
                'SELECT id FROM t ORDER BY id',
                sql,
                'SELECT id FROM t ORDER BY id',
                'SELECT 1',
            ]
            return subprocess.run(mysql_args + ['--force'],
                                  input=';\n'.join(commands) + ';\n', env=mysql_env,
                                  capture_output=True, text=True, timeout=20)
        setup = ("SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "
                 "USE mysql; CREATE TEMPORARY TABLE t (id INT PRIMARY KEY); "
                 "INSERT INTO t VALUES (31); " + sql)
        return subprocess.run(mysql_args + ['-e', setup], env=mysql_env,
                              capture_output=True, text=True, timeout=20)

    def recover(conn, side, case_id):
        nonlocal checks
        state = transaction(conn)
        rollback_ok = True
        if state in (2, 3):
            rolled_back = query(conn, 'ROLLBACK')
            rollback_ok = rolled_back['status'] == 1
            check(rollback_ok, side + ' rollback after error')
        probe = query(conn, 'SELECT 1::int4')
        transaction_after = transaction(conn)
        archive(case_id + '_recovery', 'reference' if side.startswith('reference') else 'aster',
                dict(phase='PQexec', sql='SELECT 1::int4'),
                dict(probe, endpoint=side, transaction_after={0: 'I', 2: 'T', 3: 'E'}.get(transaction_after)))
        recovered = (rollback_ok and probe['status'] == 2 and probe['rows'] == [['1']]
                     and transaction_after == 0)
        check(recovered, side + ' same-connection recovery')
        return recovered

    def describe_origins_match(case_id, description, relation_oid):
        origins = description.get('table_origins')
        if case_id == 'omitted_target':
            return (isinstance(origins, list) and len(origins) == 1
                    and origins[0] == [relation_oid, 1])
        return origins == []

    try:
        for label, dsn in [('reference', reference_dsn), ('reference_observer', reference_dsn),
                           ('aster', local_dsn), ('aster_b', local_dsn), ('aster_observer', local_dsn)]:
            conn = connect(dsn.encode())
            connections[label] = conn
            check(bool(conn) and status(conn) == 0, label + ' connection failed')
            if label.startswith('reference'):
                check(query(conn, "SET TIME ZONE 'UTC'")['status'] == 1, label + ' timezone')
            check(parameter_status(conn, b'client_encoding') == b'UTF8', label + ' encoding')
            check(parameter_status(conn, b'TimeZone') == b'UTC', label + ' timezone status')
            check(query(conn, 'SET search_path TO public')['status'] == 1, label + ' search_path')

        ref = connections['reference']
        reference_identity = query(ref, "SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(reference_identity[1] == '180006', 'exact PostgreSQL 18.6 reference required')
        reference_build = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        mysql_identity = mysql_reference(
            'SELECT VERSION(), @@version_compile_os, @@version_compile_machine; '
            'SELECT @@sql_mode, @@character_set_connection, @@collation_connection')
        mysql_lines = mysql_identity.stdout.splitlines()
        check(mysql_identity.returncode == 0 and len(mysql_lines) == 2, 'MySQL reference identity')
        check(mysql_lines[0].startswith('8.4.7'), 'exact MySQL 8.4.7 reference required')
        mysql_build = json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text())
        check(mysql_build.get('version') == '8.4.7' and mysql_build.get('image_digest'),
              'MySQL image provenance must identify the 8.4.7 build')
        check(mysql_lines[1].split('\t') == ['', 'utf8mb4', 'utf8mb4_general_ci'],
              'MySQL reference SQL mode/charset/collation')
        (evidence / 'mysql-reference.json').write_text(json.dumps({
            'identity': mysql_lines, 'build': mysql_build,
            'sql_mode': '', 'character_set_connection': 'utf8mb4',
            'collation_connection': 'utf8mb4_general_ci',
        }, indent=2))

        build = json.loads((evidence / 'rust-build.json').read_text())
        build['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance = dict(
            run=evidence.name, target_version='18.6', source=source_snapshot(root), build=build,
            client=dict(python=sys.version, library=str(library),
                        sha256=hashlib.sha256(library.read_bytes()).hexdigest(), PQlibVersion=version()),
            os=platform.platform(), storage='CreateAnalyzeSession test storage',
            authentication='InsecureRootOnly',
            reference=dict(version=reference_identity[0], server_version_num=reference_identity[1]),
            reference_build=reference_build, mysql_reference= mysql_build,
            endpoints={label: endpoint(connections[label]) for label in
                       ['aster', 'aster_b', 'reference']})
        (evidence / 'provenance.json').write_text(json.dumps(provenance, indent=2))
        provenance_ok = validate_provenance(provenance, copy.deepcopy(provenance))
        check(provenance_ok, 'provenance gate')
        damaged = copy.deepcopy(provenance)
        damaged['reference_build']['server_version_num'] = 180004
        check(not validate_provenance(damaged, provenance), 'negative provenance check')

        check(query(ref, f'CREATE SCHEMA {scope}')['status'] == 1, 'reference schema setup')
        schema_created = True
        check(query(ref, f'CREATE TABLE {scope}.t (id integer CONSTRAINT t_pkey PRIMARY KEY)')['status'] == 1,
              'reference target table setup')
        check(query(ref, f'CREATE UNIQUE INDEX {scope}_positive_id ON {scope}.t(id) WHERE id > 0')['status'] == 1,
              'reference partial index setup')
        check(query(ref, f'INSERT INTO {scope}.t VALUES (31)')['status'] == 1,
              'reference initial row setup')

        source_identity = {}
        for side, conn, schema in [('aster', connections['aster_observer'], 'public'),
                                   ('reference', connections['reference_observer'], scope)]:
            oid_sql = ("SELECT oid FROM pg_catalog.pg_class WHERE relname='t' AND "
                       "relnamespace=(SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='" + schema + "')")
            oid_result = query(conn, oid_sql)
            archive('fixture_relation_oid', side, dict(phase='PQexec', sql=oid_sql), oid_result)
            check(oid_result['status'] == 2 and len(oid_result['rows']) == 1,
                  side + ' fixture relation identity')
            relation_oid = int(oid_result['rows'][0][0]) if oid_result['rows'] else 0
            check(relation_oid > 0, side + ' positive fixture relation OID')
            attribute_sql = ("SELECT attnum,attname FROM pg_catalog.pg_attribute WHERE attrelid="
                             + str(relation_oid) + ' AND attnum > 0 ORDER BY attnum')
            attribute_result = query(conn, attribute_sql)
            archive('fixture_relation_attributes', side,
                    dict(phase='PQexec', sql=attribute_sql), attribute_result)
            attributes = ({name: int(number) for number, name in attribute_result['rows']}
                          if attribute_result['status'] == 2 else {})
            check(attributes == {'id': 1}, side + ' fixture physical attribute identity')
            source_identity[side] = dict(relation_oid=relation_oid, attributes=attributes)

        for index, (case_id, kind, sql, oids) in enumerate(cases):
            case_failure_count = len(failures)
            reference_sql = sql.replace('public.t', f'{scope}.t')
            data = dict(phase='PQprepare', sql=sql, statement=f'case_{case_id}',
                        parameter_oids=oids, arguments=None, parameter_formats=None, result_format=0)
            registered.append(dict(case_id=case_id, capability_id='pg.dialect.insert.conflict_ignore.syntax',
                                   kind=kind, input=data, reference_sql=reference_sql,
                                   encoding='UTF8', timezone='UTC', search_path='public',
                                   initial_state='I', role={'aster': 'root', 'aster_b': 'root', 'reference': 'postgres'},
                                   object_mapping={'aster': f'{sys.argv[3]}.t', 'reference': f'{scope}.t'},
                                   source_relation_oid={side: identity['relation_oid']
                                                        for side, identity in source_identity.items()},
                                   source_attributes={side: identity['attributes']
                                                      for side, identity in source_identity.items()},
                                   expected={'reference': '42601' if kind == 'invalid' else 'Prepare/Describe success',
                                             'aster': '42601' if kind == 'invalid' else 'Prepare/Describe success; Execute 0A000',
                                             'mysql': 1064},
                                   describe_expected=(dict(fields=[['id', 23, 4, -1]], formats=[0],
                                                           parameter_count=1, parameter_oids=[23],
                                                           source_columns=[[1]])
                                                      if case_id == 'omitted_target' else
                                                      (dict(fields=[], formats=[], parameter_count=0,
                                                            parameter_oids=[], source_columns=[])
                                                       if kind == 'valid' else None)),
                                   execution_input=(dict(phase='PQexecPrepared', statement=f'{case_id}_aster',
                                                         parameter_oids=oids,
                                                         arguments=['31'] if case_id == 'omitted_target' else [],
                                                         parameter_formats=[0] if case_id == 'omitted_target' else [],
                                                         result_format=0) if kind == 'valid' else None),
                                   scope='syntax and read-only metadata handoff; simple Query and Execute are gated before writes'))
            (evidence / 'cases.json').write_text(json.dumps(registered, indent=2))
            initial_local = query(connections['aster_observer'], 'SELECT id FROM public.t ORDER BY id')
            initial_reference = query(connections['reference_observer'],
                                      f'SELECT id FROM {scope}.t ORDER BY id')
            local_state_sql = 'SELECT id FROM public.t ORDER BY id'
            reference_state_sql = f'SELECT id FROM {scope}.t ORDER BY id'
            archive(case_id + '_aster_state_before', 'aster',
                    dict(phase='PQexec', sql=local_state_sql), initial_local)
            archive(case_id + '_reference_state_before', 'reference',
                    dict(phase='PQexec', sql=reference_state_sql), initial_reference)
            check(initial_local['status'] == 2 and initial_local['rows'] == [['31']],
                  case_id + ' local initial state')
            check(initial_reference['status'] == 2 and initial_reference['rows'] == [['31']],
                  case_id + ' reference initial state')

            reference_result = observe(prepare(
                ref, data['statement'].encode(), reference_sql.encode(), len(oids),
                (c.c_uint * len(oids))(*oids) if oids else None))
            reference_result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(ref))
            archive(case_id, 'reference', dict(data, sql=reference_sql), reference_result)
            local_results = {}
            local_descriptions = {}
            order = ['aster', 'aster_b'] if index % 2 == 0 else ['aster_b', 'aster']
            for ordinal, side in enumerate(order):
                conn = connections[side]
                local_data = dict(data, statement=f'{case_id}_{side}')
                result = observe(prepare(
                    conn, local_data['statement'].encode(), sql.encode(), len(oids),
                    (c.c_uint * len(oids))(*oids) if oids else None))
                result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                local_results[side] = result
                archive(case_id, 'aster', local_data, dict(result, endpoint=side), ordinal)
                if kind == 'valid' and result['status'] == 1:
                    open_statements.append((conn, local_data['statement']))
                    description = observe(describe(conn, local_data['statement'].encode()))
                    archive(case_id + '_describe', 'aster',
                            dict(phase='PQdescribePrepared', statement=local_data['statement']),
                            dict(description, endpoint=side))
                    check(description['status'] == 1 and description['state'] == '',
                          side + ' read-only Describe success')
                    check(describe_origins_match(case_id, description,
                                                 source_identity['aster']['relation_oid']),
                          side + ' Describe source-column origin')
                    local_descriptions[side] = description
                recover(conn, side, case_id)
                if ordinal == 0:
                    print('PG_DIALECT_CHECKPOINT ' + case_id, flush=True)
                    check(sys.stdin.readline().strip() == 'continue', 'MySQL checkpoint aborted')

            if kind == 'valid':
                check(reference_result['status'] == 1 and reference_result['state'] == ''
                      and reference_result['transaction_after'] == 'I', case_id + ' PG18 parse/analysis')
                local_prepare_ok = all(result['status'] == 1 and result['state'] == ''
                                       and result['transaction_after'] == 'I'
                                       for result in local_results.values())
                check(local_prepare_ok, case_id + ' local read-only Parse')
                reference_description = None
                if reference_result['status'] == 1:
                    reference_description = observe(describe(ref, data['statement'].encode()))
                    archive(case_id + '_describe', 'reference',
                            dict(phase='PQdescribePrepared', statement=data['statement']), reference_description)
                    check(reference_description['status'] == 1, case_id + ' PG18 Describe')
                    check(describe_origins_match(case_id, reference_description,
                                                 source_identity['reference']['relation_oid']),
                          'PG18 Describe source-column origin')
                    if reference_description['status'] == 1:
                        reference_describe_case_ids.add(case_id)
                    if case_id == 'omitted_target' and reference_description['status'] == 1:
                        check(reference_description['param_count'] == 1 and reference_description['params'] == [23],
                              'explicit int4 parameter Describe')
                        check(reference_description['fields'][0][0] == 'id' and reference_description['fields'][0][1] == 23,
                              'RETURNING id Describe metadata')
                    reference_syntax_case_ids.add(case_id)
                    close_result = observe(close_prepared(ref, data['statement'].encode()))
                    archive(case_id + '_reference_close', 'reference',
                            dict(phase='PQclosePrepared', statement=data['statement']), close_result)
                    check(close_result['status'] == 1, 'reference statement close')
                metadata_matches = local_prepare_ok and all(
                    side in local_descriptions
                    and local_descriptions[side]['status'] == 1
                    and local_descriptions[side]['state'] == ''
                    and describe_origins_match(
                        case_id, local_descriptions[side], source_identity['aster']['relation_oid'])
                    and reference_description is not None
                    and reference_description['status'] == 1
                    and compare_metadata(local_descriptions[side], reference_description, 'describe')
                    for side in local_results
                )
                check(metadata_matches, case_id + ' local/reference Describe metadata')
                if metadata_matches:
                    local_prepare_describe_case_ids.add(case_id)

                local_after_prepare = query(connections['aster_observer'], local_state_sql)
                archive(case_id + '_aster_state_after_prepare_describe', 'aster',
                        dict(phase='PQexec', sql=local_state_sql), local_after_prepare)
                reference_after_prepare = query(connections['reference_observer'], reference_state_sql)
                archive(case_id + '_reference_state_after_prepare_describe', 'reference',
                        dict(phase='PQexec', sql=reference_state_sql), reference_after_prepare)
                local_prepare_unchanged = (local_after_prepare['status'] == 2
                                           and local_after_prepare['rows'] == initial_local['rows'])
                reference_prepare_unchanged = (reference_after_prepare['status'] == 2
                                               and reference_after_prepare['rows'] == initial_reference['rows'])
                local_rows_preserved &= local_prepare_unchanged
                reference_rows_preserved &= reference_prepare_unchanged
                check(local_prepare_unchanged,
                      case_id + ' local Prepare/Describe left rows unchanged')
                check(reference_prepare_unchanged,
                      case_id + ' reference Prepare/Describe left rows unchanged')

                # Simple Query and extended Execute stay separate from successful
                # read-only Prepare/Describe. Both paths reject before a write.
                simple_sql = sql.replace('$1', '31')
                all_simple_gated = True
                all_execute_gated = True
                for side in local_results:
                    conn = connections[side]
                    simple_result = query(conn, simple_sql)
                    archive(case_id + '_simple_query_gate', 'aster',
                            dict(phase='PQexec', sql=simple_sql),
                            dict(simple_result, endpoint=side,
                                 transaction_after={0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))))
                    simple_gated = (simple_result['status'] == 7 and simple_result['state'] == '0A000'
                                    and transaction(conn) == 0)
                    check(simple_gated, side + ' simple Query pre-write 0A000 gate')
                    all_simple_gated &= simple_gated
                    recover(conn, side, case_id + '_simple_query_gate')
                    after_simple_query = query(connections['aster_observer'], local_state_sql)
                    archive(case_id + '_aster_state_after_simple_query_gate', 'aster',
                            dict(phase='PQexec', sql=local_state_sql),
                            dict(after_simple_query, execution_phase='PQexec', endpoint=side))
                    simple_state_unchanged = (after_simple_query['status'] == 2
                                              and after_simple_query['rows'] == initial_local['rows'])
                    local_rows_preserved &= simple_state_unchanged
                    check(simple_state_unchanged,
                          side + ' Simple Query gate left local row unchanged')

                    arguments = ['31'] if case_id == 'omitted_target' else []
                    argument_values = ((c.c_char_p * len(arguments))(
                        *[argument.encode() for argument in arguments]) if arguments else None)
                    execution_statement = f'{case_id}_{side}'
                    execution_result = observe(exec_prepared(
                        conn, execution_statement.encode(), len(arguments), argument_values,
                        None, None, 0))
                    execution_result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                    execution_data = dict(phase='PQexecPrepared', statement=execution_statement,
                                          parameter_oids=oids, arguments=arguments,
                                          parameter_formats=[0] * len(arguments), result_format=0)
                    archive(case_id + '_execute_gate', 'aster', execution_data,
                            dict(execution_result, endpoint=side))
                    execute_gated = (execution_result['status'] == 7
                                     and execution_result['state'] == '0A000'
                                     and execution_result['transaction_after'] == 'I')
                    check(execute_gated, side + ' extended Execute pre-write 0A000 gate')
                    all_execute_gated &= execute_gated
                    recover(conn, side, case_id + '_execute_gate')
                    after_execute = query(connections['aster_observer'], local_state_sql)
                    archive(case_id + '_aster_state_after_execute_gate', 'aster',
                            dict(phase='PQexec', sql=local_state_sql),
                            dict(after_execute, execution_phase='PQexecPrepared', endpoint=side))
                    execute_state_unchanged = (after_execute['status'] == 2
                                               and after_execute['rows'] == initial_local['rows'])
                    local_rows_preserved &= execute_state_unchanged
                    check(execute_state_unchanged,
                          side + ' extended Execute gate left local row unchanged')
                    closed = observe(close_prepared(conn, execution_statement.encode()))
                    archive(case_id + '_aster_close', 'aster',
                            dict(phase='PQclosePrepared', statement=execution_statement),
                            dict(closed, endpoint=side))
                    if (conn, execution_statement) in open_statements:
                        open_statements.remove((conn, execution_statement))
                    check(closed['status'] == 1, side + ' statement close')

                local_after = query(connections['aster_observer'], 'SELECT id FROM public.t ORDER BY id')
                check(local_after['status'] == 2 and local_after['rows'] == [['31']],
                      case_id + ' local row unchanged after both execution gates')
                if all_simple_gated:
                    local_simple_query_gate_case_ids.add(case_id)
                if all_execute_gated:
                    local_execute_gate_case_ids.add(case_id)
            else:
                check(reference_result['status'] == 7 and reference_result['state'] == '42601',
                      case_id + ' PG18 syntax error')
                mapped_position_delta = len(f'{scope}.t') - len('public.t')
                expected_local_position = str(int(reference_result['position']) - mapped_position_delta)
                check(all(result['status'] == 7 and result['state'] == '42601'
                          and result['position'] == expected_local_position
                          and result['transaction_after'] == 'I' for result in local_results.values()),
                      case_id + ' same source position and error recovery')
                recover(ref, 'reference', case_id)
            first_endpoint_probe = query(connections[order[0]], 'SELECT 1::int4')
            first_endpoint_transaction = transaction(connections[order[0]])
            archive(case_id + '_endpoint_order_recovery', 'aster',
                    dict(phase='PQexec', sql='SELECT 1::int4'),
                    dict(first_endpoint_probe, endpoint=order[0],
                         transaction_after={0: 'I', 2: 'T', 3: 'E'}.get(first_endpoint_transaction)))
            check(first_endpoint_probe['status'] == 2 and first_endpoint_probe['rows'] == [['1']]
                  and first_endpoint_transaction == 0,
                  case_id + ' first PG endpoint after second endpoint')

            mysql_sql = sql.replace('public.t', 't').replace('$1', '31')
            mysql_result = mysql_reference(mysql_sql, checkpoint=True)
            combined_error = mysql_result.stderr
            match = re.search(r'ERROR\s+(\d+)\s+\(([^)]+)\)', combined_error)
            code = int(match.group(1)) if match else None
            state = match.group(2) if match else None
            mysql_observer_lines = mysql_result.stdout.splitlines()
            mysql_columns = mysql_observer_lines[0].split('\t') if mysql_observer_lines else []
            mysql_observer_rows = mysql_observer_lines[1:] if mysql_observer_lines else []
            mysql_row_before = mysql_observer_rows[0] if len(mysql_observer_rows) == 3 else None
            mysql_row_after = mysql_observer_rows[1] if len(mysql_observer_rows) == 3 else None
            mysql_recovery = mysql_observer_rows[2] if len(mysql_observer_rows) == 3 else None
            check(code == 1064 and state == '42000',
                  case_id + ' MySQL 8.4.7 must reject PG conflict syntax')
            check(mysql_columns[:4] == ['id', 'int', 'NO', 'PRI'],
                  case_id + ' MySQL reference column metadata')
            check(mysql_row_before == '31' and mysql_row_after == mysql_row_before,
                  case_id + ' MySQL reference rows unchanged')
            check(mysql_recovery == '1', case_id + ' MySQL reference same-connection recovery')
            check(combined_error.count('ERROR 1064 (42000)') == 1,
                  case_id + ' MySQL legal ON DUPLICATE Prepare and rejected syntax outcome')
            mysql_location = ('near \'"CONFLICT"' if case_id == 'quoted_conflict_keyword'
                              else "near 'CONFLICT")
            check(mysql_location in combined_error, case_id + ' MySQL syntax location')
            if code == 1064 and state == '42000':
                mysql_reference_rejection_case_ids.add(case_id)
            archive(case_id, 'mysql', dict(phase='mysql-query', command=mysql_sql),
                    dict(code=mysql_result.returncode, error_code=code, sqlstate=state,
                         error=combined_error, sql_mode='', character_set='utf8mb4',
                         collation='utf8mb4_general_ci', column_metadata=mysql_columns,
                         row_before=mysql_row_before, row_after=mysql_row_after,
                         recovery=mysql_recovery,
                         legal_prepare_sql='INSERT INTO t(id) VALUES (31) ON DUPLICATE KEY UPDATE id=VALUES(id)',
                         legal_prepare_error_count=combined_error.count('ERROR 1064 (42000)') - 1,
                         subprocess_exited=True, temporary_connection_closed=True))

            final_local = query(connections['aster_observer'], 'SELECT id FROM public.t ORDER BY id')
            final_reference = query(connections['reference_observer'],
                                    f'SELECT id FROM {scope}.t ORDER BY id')
            archive(case_id + '_aster_state_after', 'aster',
                    dict(phase='PQexec', sql=local_state_sql), final_local)
            archive(case_id + '_reference_state_after', 'reference',
                    dict(phase='PQexec', sql=reference_state_sql), final_reference)
            local_final_unchanged = (final_local['status'] == 2
                                     and final_local['rows'] == initial_local['rows'])
            reference_final_unchanged = (final_reference['status'] == 2
                                         and final_reference['rows'] == initial_reference['rows'])
            local_rows_preserved &= local_final_unchanged
            reference_rows_preserved &= reference_final_unchanged
            check(local_final_unchanged, case_id + ' local prepare side effects')
            check(reference_final_unchanged, case_id + ' reference prepare side effects')

            case_passed = len(failures) == case_failure_count
            case_audit[case_id] = dict(kind=kind, assertions_passed=case_passed,
                                       failure_count=len(failures) - case_failure_count)
            if case_passed:
                python_effective_case_ids.add(case_id)

        final_reference = query(ref, "SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(final_reference == reference_identity, 'PostgreSQL reference identity changed')
        provenance_final = copy.deepcopy(provenance)
        provenance_final['source'] = source_snapshot(root)
        provenance_final['build']['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance_final['client']['sha256'] = hashlib.sha256(library.read_bytes()).hexdigest()
        provenance_final['reference_build'] = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        check(validate_provenance(provenance, provenance_final), 'provenance changed during run')
        completed = True
    finally:
        for conn, statement in open_statements:
            try:
                close_prepared(conn, statement.encode())
            except Exception:
                cleanup_ok = False
        for conn in connections.values():
            if conn:
                try:
                    if transaction(conn) in (2, 3):
                        cleanup_ok = query(conn, 'ROLLBACK')['status'] == 1 and cleanup_ok
                    finish(conn)
                except Exception:
                    cleanup_ok = False
        if schema_created:
            cleanup_conn = connect(reference_dsn.encode())
            try:
                cleanup_ok = status(cleanup_conn) == 0 and query(
                    cleanup_conn, f'DROP SCHEMA {scope} CASCADE')['status'] == 1 and cleanup_ok
            finally:
                finish(cleanup_conn)
        reference_probe = connect(reference_dsn.encode())
        try:
            cleanup_ok = status(reference_probe) == 0 and query(
                reference_probe, 'SELECT 1::int4')['rows'] == [['1']] and cleanup_ok
        finally:
            finish(reference_probe)
        cleanup = dict(cleanup_ok=cleanup_ok, reference_schema_dropped=schema_created,
                       local_rows_preserved=local_rows_preserved,
                       reference_rows_preserved=reference_rows_preserved,
                       temporary_mysql_tables_connection_scoped=True)
        (evidence / 'reference-cleanup.json').write_text(json.dumps(cleanup, indent=2))
        summary = dict(cases=len(cases), assertions=checks, failures=failures,
                       provenance_ok=provenance_ok,
                       reference_syntax_cases=len(reference_syntax_case_ids),
                       reference_prepare_success_cases=len(reference_syntax_case_ids),
                       reference_describe_cases=len(reference_describe_case_ids),
                       local_prepare_describe_cases=len(local_prepare_describe_case_ids),
                       local_simple_query_gate_cases=len(local_simple_query_gate_case_ids),
                       local_execute_gate_cases=len(local_execute_gate_case_ids),
                       mysql_rejection_cases=len(mysql_reference_rejection_case_ids),
                       python_effective_case_ids=sorted(python_effective_case_ids),
                       case_audit=case_audit,
                       syntax_eligible=(len(python_effective_case_ids)
                                        if completed and cleanup_ok and provenance_ok and not failures else 0),
                       scope='syntax and read-only target/parameter metadata handoff; simple Query and Execute gated before writes; no conflict DML executed',
                       cleanup_ok=cleanup_ok)
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    if failures:
        raise AssertionError('conflict-ignore handoff failures: ' + '; '.join(failures))
    assert summary['syntax_eligible'] == len(cases), 'all registered conflict cases must be applicable'
    print(json.dumps(summary), flush=True)


def conflict_update_handoff():
    """Compare both ON CONFLICT predicates through read-only Describe."""
    scope = f'dialect_08_{os.getpid()}_{int(sys.argv[2])}'
    local_dsn = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
    reference_dsn = os.environ['PG18_REFERENCE_DSN']
    mysql_dsn = urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
    mysql_env = dict(os.environ, MYSQL_PWD=mysql_dsn.password or '')
    mysql_args = ['mysql', '--protocol=TCP', '--connect-timeout=5', '-h', mysql_dsn.hostname,
                  '-P', str(mysql_dsn.port or 3306), '-u', mysql_dsn.username or 'root',
                  '--batch', '--raw', '--skip-column-names']
    cases = [
        ('inference_update_both_predicates', 'valid',
         'INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT (id) WHERE id > 0 DO UPDATE SET v=excluded.v, id=$1 WHERE t.v IS NOT NULL RETURNING id', [23, 25]),
        ('constraint_update_predicate', 'valid',
         'INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT ON CONSTRAINT "t_pkey" DO UPDATE SET v=excluded.v WHERE t.v IS NOT NULL RETURNING id', [23, 25]),
        ('inference_update_without_action_predicate', 'valid',
         'INSERT INTO public.t(id,v) VALUES ($1,$2) ON CONFLICT (id) WHERE id > 0 DO UPDATE SET v=excluded.v RETURNING id', [23, 25]),
        ('update_without_target', 'invalid',
         'INSERT INTO public.t(id,v) VALUES (31,\'after\') ON CONFLICT DO UPDATE SET v=excluded.v RETURNING id', []),
        ('update_missing_set', 'invalid',
         'INSERT INTO public.t(id,v) VALUES (31,\'after\') ON CONFLICT (id) DO UPDATE WHERE v IS NULL RETURNING id', []),
        ('update_empty_assignments', 'invalid',
         'INSERT INTO public.t(id,v) VALUES (31,\'after\') ON CONFLICT (id) DO UPDATE SET WHERE v IS NULL RETURNING id', []),
    ]
    evidence.mkdir(parents=True, exist_ok=True)
    (evidence / 'cases.json').write_text(json.dumps([
        dict(case_id=case_id, capability_id='pg.dialect.insert.conflict_update.syntax',
             kind=kind, sql=sql, phase='PQprepare', parameter_oids=oids,
             arguments=['31', 'after'] if oids else [], parameter_formats=[0, 0] if oids else [],
             result_format=0, expected_state='42601' if kind == 'invalid' else '',
             expected_metadata='PG18 Prepare/Describe' if kind == 'valid' else 'not applicable: syntax error',
             execution='Aster simple Query and PQexecPrepared must be gated before writes' if kind == 'valid' else 'not applicable: syntax error')
        for case_id, kind, sql, oids in cases], indent=2))

    connections = {}
    prepared = []
    failures = []
    case_audit = {}
    assertions = 0
    provenance_ok = False
    cleanup_ok = True
    reference_schema_created = False
    local_rows_preserved = True
    reference_rows_preserved = True
    completed = False
    effective_case_ids = set()

    def check(condition, message):
        nonlocal assertions
        assertions += 1
        if not condition:
            failures.append(message)

    def archive(case_id, side, phase, sql, observation):
        append_observation(evidence, dict(run=evidence.name, case_id=case_id, side=side,
                                          attempt=1, input=dict(phase=phase, sql=sql),
                                          observation=observation))

    def recover(conn, side, case_id):
        transaction_before = transaction(conn)
        rollback_ok = True
        if transaction_before in (2, 3):
            rollback_ok = query(conn, 'ROLLBACK')['status'] == 1
        probe = query(conn, 'SELECT 1::int4')
        transaction_after = transaction(conn)
        archive(case_id + '_recovery', 'reference' if side.startswith('reference') else 'aster',
                'PQexec', 'SELECT 1::int4',
                dict(probe, endpoint=side, transaction_after={0: 'I', 2: 'T', 3: 'E'}.get(transaction_after)))
        recovered = (rollback_ok and probe['status'] == 2 and probe['rows'] == [['1']]
                     and transaction_after == 0)
        check(recovered, side + ' same-connection recovery')
        return recovered

    def mysql_reference(sql, case_id):
        mysql_sql = sql.replace('public.t', 't').replace('$1', '31').replace('$2', "'after'")
        commands = [
            "SET SESSION sql_mode=''",
            'SET NAMES utf8mb4 COLLATE utf8mb4_general_ci',
            'USE mysql',
            'CREATE TEMPORARY TABLE t (id INT PRIMARY KEY, v VARCHAR(64))',
            "INSERT INTO t VALUES (31,'before')",
            "PREPARE task8_legacy FROM 'INSERT INTO t(id,v) VALUES (31,''before'') ON DUPLICATE KEY UPDATE v=VALUES(v)'",
            'DEALLOCATE PREPARE task8_legacy',
            'SELECT id,v FROM t ORDER BY id',
            mysql_sql,
            'SELECT id,v FROM t ORDER BY id',
            'SELECT 1',
        ]
        process = subprocess.run(mysql_args + ['--force'], input=';\n'.join(commands) + ';\n',
                                 env=mysql_env, capture_output=True, text=True, timeout=20)
        output = process.stdout.splitlines()
        error = process.stderr
        matches = re.findall(r'ERROR\s+(\d+)\s+\(([^)]+)\)', error)
        before = output[0] if len(output) >= 3 else None
        after = output[1] if len(output) >= 3 else None
        recovery = output[2] if len(output) >= 3 else None
        ok = (matches == [('1064', '42000')]
              and before == '31\tbefore' and after == before and recovery == '1'
              and error.count('ERROR 1064 (42000)') == 1)
        archive(case_id, 'mysql', 'mysql-query', mysql_sql,
                dict(returncode=process.returncode, error_code=int(matches[0][0]) if matches else None,
                     sqlstate=matches[0][1] if matches else None, error=error,
                     row_before=before, row_after=after, recovery=recovery,
                     legal_on_duplicate_prepared=error.count('ERROR 1064 (42000)') == 1))
        check(ok, case_id + ' MySQL 8.4.7 syntax rejection, baseline prepare, state and recovery')
        return ok

    try:
        for label, dsn in [('reference', reference_dsn), ('reference_observer', reference_dsn),
                           ('aster', local_dsn), ('aster_b', local_dsn), ('aster_observer', local_dsn)]:
            conn = connect(dsn.encode())
            connections[label] = conn
            check(bool(conn) and status(conn) == 0, label + ' connection')
            if label.startswith('reference'):
                check(query(conn, "SET TIME ZONE 'UTC'")['status'] == 1, label + ' UTC')
            check(parameter_status(conn, b'client_encoding') == b'UTF8', label + ' UTF8')
            check(parameter_status(conn, b'TimeZone') == b'UTC', label + ' UTC status')
            check(query(conn, 'SET search_path TO public')['status'] == 1, label + ' search_path')

        ref = connections['reference']
        identity = query(ref, "SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(identity[1] == '180006', 'exact PostgreSQL 18.6 reference')
        reference_build = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        mysql_identity = subprocess.run(mysql_args + ['-e',
            "SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "
            'SELECT VERSION(),@@version_compile_os,@@version_compile_machine; '
            'SELECT @@sql_mode,@@character_set_connection,@@collation_connection'],
            env=mysql_env, capture_output=True, text=True, timeout=20)
        mysql_lines = mysql_identity.stdout.splitlines()
        check(mysql_identity.returncode == 0 and len(mysql_lines) == 2,
              'MySQL reference identity collection')
        check(bool(mysql_lines) and mysql_lines[0].startswith('8.4.7'), 'exact MySQL 8.4.7 reference')
        check(len(mysql_lines) == 2 and mysql_lines[1].split('\t') ==
              ['', 'utf8mb4', 'utf8mb4_general_ci'], 'MySQL sql_mode/charset/collation')
        mysql_build = json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text())
        check(mysql_build.get('version') == '8.4.7' and mysql_build.get('image_digest'),
              'MySQL 8.4.7 image provenance')
        (evidence / 'mysql-reference.json').write_text(json.dumps(
            dict(identity=mysql_lines, build=mysql_build, sql_mode='',
                 character_set_connection='utf8mb4', collation_connection='utf8mb4_general_ci'), indent=2))

        build = json.loads((evidence / 'rust-build.json').read_text())
        build['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance = dict(run=evidence.name, target_version='18.6', source=source_snapshot(root),
                          build=build,
                          client=dict(python=sys.version, library=str(library),
                                      sha256=hashlib.sha256(library.read_bytes()).hexdigest(),
                                      PQlibVersion=version()),
                          os=platform.platform(), storage='CreateAnalyzeSession test storage',
                          authentication='InsecureRootOnly',
                          reference=dict(version=identity[0], server_version_num=identity[1]),
                          reference_build=reference_build, mysql_reference=mysql_build,
                          endpoints={name: endpoint(connections[name])
                                     for name in ['aster', 'aster_b', 'reference']})
        (evidence / 'provenance.json').write_text(json.dumps(provenance, indent=2))
        provenance_ok = validate_provenance(provenance, copy.deepcopy(provenance))
        check(provenance_ok, 'provenance self-check')
        damaged = copy.deepcopy(provenance)
        damaged['reference_build']['server_version_num'] = 180004
        check(not validate_provenance(damaged, provenance), 'provenance negative self-check')

        check(query(ref, f'CREATE SCHEMA {scope}')['status'] == 1, 'reference schema setup')
        reference_schema_created = True
        check(query(ref, f'CREATE TABLE {scope}.t (id integer CONSTRAINT t_pkey PRIMARY KEY, v text)')['status'] == 1,
              'reference table setup')
        check(query(ref, f'CREATE UNIQUE INDEX {scope}_positive_id ON {scope}.t(id) WHERE id > 0')['status'] == 1,
              'reference partial unique index setup')
        check(query(ref, f"INSERT INTO {scope}.t VALUES (31, 'before')")['status'] == 1,
              'reference initial row')
        initial_local = query(connections['aster_observer'], 'SELECT id, v FROM public.t ORDER BY id')
        initial_reference = query(connections['reference_observer'],
                                  f'SELECT id, v FROM {scope}.t ORDER BY id')
        check(initial_local['status'] == 2 and initial_local['rows'] == [['31', 'before']],
              'Aster initial row')
        check(initial_reference['status'] == 2 and initial_reference['rows'] == [['31', 'before']],
              'PostgreSQL initial row')
        local_oid_result = query(connections['aster_observer'],
            "SELECT oid FROM pg_catalog.pg_class WHERE relname='t' AND relnamespace="
            "(SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='public')")
        reference_oid_result = query(connections['reference_observer'],
            f"SELECT oid FROM pg_catalog.pg_class WHERE relname='t' AND relnamespace="
            f"(SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='{scope}')")
        check(local_oid_result['status'] == 2 and len(local_oid_result['rows']) == 1,
              'Aster relation identity')
        check(reference_oid_result['status'] == 2 and len(reference_oid_result['rows']) == 1,
              'reference relation identity')
        local_oid = int(local_oid_result['rows'][0][0]) if local_oid_result['rows'] else 0
        reference_oid = int(reference_oid_result['rows'][0][0]) if reference_oid_result['rows'] else 0

        valid_reference_ids = set()
        valid_local_ids = set()
        syntax_negative_ids = set()
        mysql_reference_ids = set()
        for index, (case_id, kind, sql, oids) in enumerate(cases):
            case_failure_count = len(failures)
            reference_sql = sql.replace('public.t', f'{scope}.t')
            print('PG_DIALECT_CHECKPOINT ' + case_id, flush=True)
            check(sys.stdin.readline().strip() == 'continue', case_id + ' MySQL local checkpoint')
            local_state_sql = 'SELECT id, v FROM public.t ORDER BY id'
            reference_state_sql = f'SELECT id, v FROM {scope}.t ORDER BY id'
            local_before = query(connections['aster_observer'], local_state_sql)
            reference_before = query(connections['reference_observer'], reference_state_sql)
            check(local_before['status'] == 2 and local_before['rows'] == initial_local['rows'],
                  case_id + ' Aster state before Prepare')
            check(reference_before['status'] == 2 and reference_before['rows'] == initial_reference['rows'],
                  case_id + ' reference state before Prepare')

            reference_name = f'case_{index}_reference'
            reference_result = observe(prepare(ref, reference_name.encode(), reference_sql.encode(), len(oids),
                                               (c.c_uint * len(oids))(*oids) if oids else None))
            reference_result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(ref))
            archive(case_id, 'reference', 'PQprepare', reference_sql, reference_result)
            reference_description = {}
            local_results = {}
            local_descriptions = {}
            local_statement_names = {}
            order = ['aster', 'aster_b'] if index % 2 == 0 else ['aster_b', 'aster']
            for side in order:
                conn = connections[side]
                statement_name = f'{case_id}_{side}'
                local_statement_names[side] = statement_name
                result = observe(prepare(conn, statement_name.encode(), sql.encode(), len(oids),
                                         (c.c_uint * len(oids))(*oids) if oids else None))
                result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                local_results[side] = result
                archive(case_id, 'aster', 'PQprepare', sql, dict(result, endpoint=side))
                expected_state = '' if kind == 'valid' else '42601'
                expected_status = 1 if kind == 'valid' else 7
                check(result['status'] == expected_status and result['state'] == expected_state
                      and result['transaction_after'] == 'I', side + ' Prepare outcome')
                if kind == 'valid' and result['status'] == 1:
                    prepared.append((conn, statement_name))
                    description = observe(describe(conn, statement_name.encode()))
                    archive(case_id + '_describe', 'aster', 'PQdescribePrepared', statement_name, description)
                    check(description['status'] == 1 and description['state'] == '', side + ' Describe')
                    local_descriptions[side] = description
                else:
                    recover(conn, side, case_id)

            expected_state = '' if kind == 'valid' else '42601'
            expected_status = 1 if kind == 'valid' else 7
            check(reference_result['status'] == expected_status and reference_result['state'] == expected_state
                  and reference_result['transaction_after'] == 'I', 'PostgreSQL reference Prepare outcome')
            if kind == 'invalid':
                syntax_negative_ids.add(case_id)
                recover(ref, 'reference', case_id)
                local_after = query(connections['aster_observer'],
                                    'SELECT id, v FROM public.t ORDER BY id')
                reference_after = query(connections['reference_observer'],
                                        f'SELECT id, v FROM {scope}.t ORDER BY id')
                local_unchanged = local_after['status'] == 2 and local_after['rows'] == initial_local['rows']
                reference_unchanged = reference_after['status'] == 2 and reference_after['rows'] == initial_reference['rows']
                local_rows_preserved &= local_unchanged
                reference_rows_preserved &= reference_unchanged
                check(local_unchanged and reference_unchanged,
                      case_id + ' parse rejection left both tables unchanged')
            else:
                if reference_result['status'] == 1:
                    prepared.append((ref, reference_name))
                    reference_description = observe(describe(ref, reference_name.encode()))
                    archive(case_id + '_describe', 'reference', 'PQdescribePrepared', reference_name,
                            reference_description)
                    check(reference_description['status'] == 1 and reference_description['state'] == '',
                          'PG18 Describe')
                    check(reference_description['param_count'] == len(oids)
                          and reference_description['params'] == oids, 'PG18 parameter OIDs')
                    check(bool(reference_description['fields'])
                          and reference_description['fields'][0][0:2] == ['id', 23],
                          'PG18 RETURNING id metadata')
                    valid_reference_ids.add(case_id)
                metadata_ok = (reference_result['status'] == 1 and reference_description['status'] == 1
                               and all(side in local_descriptions
                                       and compare_metadata(local_descriptions[side], reference_description, 'describe')
                                       and local_descriptions[side]['fields'][0][0:2] == ['id', 23]
                                       and local_descriptions[side]['params'] == oids
                                       and local_descriptions[side]['table_origins'] == [[local_oid, 1]]
                                       for side in order))
                check(metadata_ok, case_id + ' local/PG Prepare and Describe metadata')
                if metadata_ok:
                    valid_local_ids.add(case_id)
                reference_origin_ok = (reference_description.get('table_origins') == [[reference_oid, 1]])
                check(reference_origin_ok, case_id + ' PG18 RETURNING source origin')
                for side in order:
                    conn = connections[side]
                    simple_sql = sql.replace('$1', '31').replace('$2', "'after'")
                    simple = query(conn, simple_sql)
                    simple_ok = simple['status'] == 7 and simple['state'] == '0A000'
                    archive(case_id + '_simple_query_gate', 'aster', 'PQexec', simple_sql,
                            dict(simple, endpoint=side, transaction_after={0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))))
                    check(simple_ok, side + ' Simple Query pre-write gate')
                    recover(conn, side, case_id + '_simple_query_gate')
                    values = (c.c_char_p * len(oids))(b'31', b'after')
                    lengths = (c.c_int * len(oids))(0, 0)
                    formats = (c.c_int * len(oids))(0, 0)
                    executed = observe(exec_prepared(conn, local_statement_names[side].encode(),
                                                     len(oids), values, lengths, formats, 0))
                    execute_ok = executed['status'] == 7 and executed['state'] == '0A000'
                    archive(case_id + '_execute_gate', 'aster', 'PQexecPrepared',
                            local_statement_names[side], dict(executed, endpoint=side))
                    check(execute_ok, side + ' extended Execute pre-write gate')
                    recover(conn, side, case_id + '_execute_gate')
                local_after = query(connections['aster_observer'], local_state_sql)
                reference_after = query(connections['reference_observer'], reference_state_sql)
                local_unchanged = local_after['status'] == 2 and local_after['rows'] == initial_local['rows']
                reference_unchanged = reference_after['status'] == 2 and reference_after['rows'] == initial_reference['rows']
                local_rows_preserved &= local_unchanged
                reference_rows_preserved &= reference_unchanged
                check(local_unchanged and reference_unchanged, case_id + ' state unchanged after Describe/gates')

            mysql_ok = mysql_reference(sql, case_id)
            if mysql_ok:
                mysql_reference_ids.add(case_id)
            case_audit[case_id] = dict(kind=kind, assertion_count=assertions - case_failure_count,
                                       passed=len(failures) == case_failure_count,
                                       prepare_describe=(kind == 'valid'),
                                       full_dml_semantic_success=False)
            if len(failures) == case_failure_count and mysql_ok:
                effective_case_ids.add(case_id)

        final_identity = query(ref, "SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(final_identity == identity, 'PostgreSQL reference identity unchanged')
        final_provenance = copy.deepcopy(provenance)
        final_provenance['source'] = source_snapshot(root)
        final_provenance['build']['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        final_provenance['client']['sha256'] = hashlib.sha256(library.read_bytes()).hexdigest()
        final_provenance['reference_build'] = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        check(validate_provenance(provenance, final_provenance), 'source/build/client/reference unchanged')
        completed = True
    finally:
        for conn, statement_name in prepared:
            try:
                if transaction(conn) in (2, 3):
                    cleanup_ok = query(conn, 'ROLLBACK')['status'] == 1 and cleanup_ok
                cleanup_ok = observe(close_prepared(conn, statement_name.encode()))['status'] == 1 and cleanup_ok
            except Exception:
                cleanup_ok = False
        for conn in connections.values():
            try:
                if transaction(conn) in (2, 3):
                    cleanup_ok = query(conn, 'ROLLBACK')['status'] == 1 and cleanup_ok
                finish(conn)
            except Exception:
                cleanup_ok = False
        if reference_schema_created:
            cleanup_conn = connect(reference_dsn.encode())
            try:
                cleanup_ok = status(cleanup_conn) == 0 and query(
                    cleanup_conn, f'DROP SCHEMA {scope} CASCADE')['status'] == 1 and cleanup_ok
            finally:
                finish(cleanup_conn)
        reference_probe = connect(reference_dsn.encode())
        try:
            cleanup_ok = status(reference_probe) == 0 and query(
                reference_probe, 'SELECT 1::int4')['rows'] == [['1']] and cleanup_ok
        finally:
            finish(reference_probe)
        local_file = evidence / 'mysql-local.jsonl'
        local_records = ([json.loads(line) for line in local_file.read_text().splitlines()]
                         if local_file.exists() else [])
        local_case_ids = {case_id for case_id, _, _, _ in cases}
        local_checkpoint_ids = {record.get('case_id') for record in local_records
                                if record.get('case_id') in local_case_ids
                                and 'postgresql_sql' in record}
        local_checkpoint_count = len(local_checkpoint_ids)
        ref_cleanup = dict(cleanup_ok=cleanup_ok, reference_schema_dropped=reference_schema_created,
                           local_rows_preserved=local_rows_preserved,
                           reference_rows_preserved=reference_rows_preserved)
        (evidence / 'reference-cleanup.json').write_text(json.dumps(ref_cleanup, indent=2))
        summary = dict(cases=len(cases), assertions=assertions, failures=failures,
                       provenance_ok=provenance_ok, unique_effective=(len(effective_case_ids)
                       if completed and cleanup_ok and provenance_ok and not failures else 0),
                       effective_case_ids=sorted(effective_case_ids), case_audit=case_audit,
                       valid_prepare_describe_cases=len(valid_reference_ids & valid_local_ids),
                       syntax_negative_parse_cases=len(syntax_negative_ids),
                       reference_prepare_cases=len(valid_reference_ids),
                       local_prepare_describe_cases=len(valid_local_ids),
                       mysql_reference_rejection_cases=len(mysql_reference_ids),
                       mysql_local_checkpoint_cases=local_checkpoint_count,
                       full_dml_semantic_successes=0, cleanup_ok=cleanup_ok,
                       scope='syntax and Prepare/Describe metadata only; DML was not executed successfully')
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    if failures:
        raise AssertionError('conflict-update handoff failures: ' + '; '.join(failures))
    assert summary['unique_effective'] == len(cases), 'all registered cases must be evidence-complete'
    assert summary['mysql_local_checkpoint_cases'] == len(cases)
    print(json.dumps(summary), flush=True)

if task == 24:
    cases = [(name,sql,oids) for name,sql,oids in [
        ('coexist','SELECT $1::int4 AS v',[23]),
        ('parse_error','/*中*/ SELECT )',[]),
        ('close_statement','SELECT $1::int8 AS v',[20]),
        ('close_portal','SELECT $1::int4 AS v',[23]),
        ('reconnect','SELECT $1::int4 AS v',[23]),
        ('transaction_error','/*中*/ SELECT )',[]),
    ]]
assert len({case[0] for case in cases}) == (10 if task == 11 else (10 if task == 6 else 6 if task in (5,8,18,24) else 5 if task == 16 else 11 if task == 14 else (9 if task == 13 else (6 if task in (9,11) else (8 if task in (4,10) else 10)))))
(evidence / 'cases.json').write_text(json.dumps([
    dict(case_id=name, capability_id='pg.dialect.row.syntax' if task == 11 else ('pg.dialect.array.nesting' if task == 10 else ('pg.dialect.array.syntax' if task == 9 else ('pg.dialect.ilike.syntax' if task == 1 else ('pg.dialect.escape.syntax' if task == 2 else 'pg.dialect.ilike.precedence')))), sql=sql,
         phase='PQprepare', parameter_oids=oids, arguments=((['5','7'] if task == 10 else (['7','9'] if task in (9,11) else [None]*len(oids))) if oids else []),
         parameter_formats=([0]*len(oids) if oids else []), result_format=0,
         initial_state='I', expected_basis=f'dialect task {task} / PostgreSQL18.6 observation / roadmap chapter 2',
         expected_state='42601' if name.startswith('missing_') or name == 'misplaced_where' else '',
         object_mapping={}, encoding='UTF8', timezone='UTC',
         side_effects='not applicable: SELECT expressions without relations',
         execution='not applicable: parse/analyze handoff only')
    for name, sql, oids in cases], indent=2))


if task == 10:
    registry = json.loads((evidence/'cases.json').read_text())
    for case in registry:
        name = case['case_id']
        case['expected_state_by_phase'] = {
            'ragged': {'Execute': '2202E'},
            'mixed_types': {'PQprepare': '42804'},
            'unclosed': {'PQprepare': '42601'},
            'parameters': {'PQexec': '42P02'},
        }.get(name, {})
        case['expected_rows'] = {
            'explicit': [['{{1,2},{3,4}}']],
            'implicit': [['{{1,2},{3,4}}']],
            'numeric': [['{1,2.5}']],
            'parameters': [['{{7,5},{5,7}}']],
            'nulls': [['{{NULL,1},{2,NULL}}']],
        }.get(name)
        case['execution'] = 'PQprepare/Describe/Bind/Execute and Q; nested dimensions and common element OID compared in text and binary result formats where applicable'
        case['side_effects'] = 'no writes; independent observer checks its fixture before and after'
    (evidence/'cases.json').write_text(json.dumps(registry, indent=2))

if task == 9:
    registry = json.loads((evidence/'cases.json').read_text())
    for case in registry:
        name = case['case_id']
        case['local_expected_state'] = '42601' if name == 'missing_close' else ('42P18' if name == 'empty' else '')
        case['reference_expected_state'] = '42601' if name == 'missing_close' else ('42P18' if name == 'empty' else '')
        case['expected_state'] = None
        case['execution'] = 'both endpoints Prepare/Describe/Bind/Execute; typed NULL and repeated text/binary result Bind; Q original diagnostics'
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


raw_value = c.CFUNCTYPE(ptr, ptr, integer, integer)(('PQgetvalue', pq))
value_length = api('PQgetlength', integer, ptr, integer, integer)

def observe(result, binary=False):
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
                    rows=[[None if is_null(result, r, col) else (c.string_at(raw_value(result,r,col),value_length(result,r,col)).hex() if binary else value(result, r, col).decode())
                           for col in range(columns(result))] for r in range(rows(result))],
                    command_tag=(command_tag(result) or b'').decode(),
         table_origins=([[field_table(result,i),field_column(result,i)] for i in range(columns(result))] if task in (4,5,6,7,8,9,10,11,13,14,16,18,24) else None))
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
    registry=json.loads((evidence/'cases.json').read_text())
    for case in registry:
        name=case['case_id']; state={'missing_close':'42601','unknown_parameter':'42P18'}.get(name,'')
        case.update(expected_state=state,expected_by_side={'aster':state or 'success','reference':state or 'success'},
            execution='normal Prepare/Describe/Bind/Execute text and binary; Q with parameter errors independently observed',
            expected_result_oid=None if state else (25 if name=='protected' else 2249),
            side_effects='read-only main cases; outside-scope DML safety probe never counts as a main case')
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
            kind = 'invalid' if name.startswith('missing_') else ('control' if name in ('plain', 'protected') else 'returning')
            # Explicit test-object mapping, never a product SQL adaptation.
            ref_sql = sql.replace('public.t', f'{scope}.t')
            data = dict(case_id=name, capability_id='pg.dialect.insert.returning.syntax',
                        sql=sql, reference_sql=ref_sql, phase='PQprepare', parameter_oids=oids,
                        arguments=None, parameter_formats=None, result_format=0, encoding='UTF8', timezone='UTC',
                        role={s: connection_user(connections[s]).decode() for s in ['aster', 'reference']}, search_path='public', initial_state='I',
                        object_mapping={'aster': f'{sys.argv[3]}.t', 'reference': f'{scope}.t'},
                        expected_basis='task4 syntax/gate contract and exact PostgreSQL18.6 PQprepare',
                        expected={'aster': '42601' if kind == 'invalid' else 'success',
                                  'reference': '42601' if kind == 'invalid' else 'success'},
                        unverified='RETURNING row execution remains gated; successful Prepare/Describe is not execution support')
            registry.append(data)
            (evidence / 'cases.json').write_text(json.dumps(registry, indent=2))
            before = {side: snapshot(side) for side in ['aster', 'reference']}
            results = {}
            descriptors = {}
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
                    check(desc['fields'] == expected_fields, f'{side}: descriptor mismatch: {desc}')
                    descriptors[side] = desc
                    origins = desc['table_origins']
                    expected_attributes = ([1,0,1,2] if name == 'projection' else [1,1,2] if name == 'quoted' else [1,2] if name == 'null_star' else [1] if name == 'bare_alias' else [])
                    check([origin[1] for origin in origins] == expected_attributes, 'physical attribute identity lost')
                    check(all((oid != 0) == (attribute != 0) for oid, attribute in origins), 'physical table identity lost')
                    check(desc['formats'] == [0] * len(expected_fields), 'nontext descriptor')
                    if kind == 'returning' and side == 'aster':
                        result = observe(exec_prepared(conn, statement, len(oids),
                            (text * len(oids))(*([b'7'] * len(oids))) if oids else None,
                            None, (integer * len(oids))(*([0] * len(oids))) if oids else None, 0))
                        archive(name + '_execute_gate', side,
                            dict(phase='PQexecPrepared', statement=statement.decode(), arguments=['7'] * len(oids), parameter_formats=[0] * len(oids), result_format=0), result)
                        check(result['state'] == '0A000' and result['message'] == 'DML RETURNING is unsupported by the PostgreSQL listener', 'Execute crossed the write boundary')
                        recovery(conn, side, name + '_execute')
                        check(snapshot(side) == before[side], 'Execute gate wrote rows')
                    check(observe(close_prepared(conn, statement))['status'] == 1, 'statement close failed')
                    prepared.remove((conn, statement))
                recovery(conn, side, name)
                check(snapshot(side) == before[side], 'prepare/describe/rejection wrote rows')
            check(compare(results['aster'], results['reference'], kind, sql, ref_sql), 'syntax/gate comparator mismatch')
            for mutation in ['state', 'status']:
                changed = copy.deepcopy(results['aster']); changed[mutation] = 'XX000' if mutation == 'state' else 0
                assert not compare(changed, results['reference'], kind, sql, ref_sql)
                self_checks += 1
            if kind == 'returning':
                check(compare_metadata(descriptors['aster'], descriptors['reference'], 'describe'), 'Describe mismatch')
                changed = copy.deepcopy(descriptors['aster']); changed['fields'][0][1] = 99999
                assert not compare_metadata(changed, descriptors['reference'], 'describe')
                self_checks += 1
                eligible += 1
                gate_cases += 1
                if oids:
                    for side in ['aster', 'reference']:
                        conn = connections[side]
                        actual_sql = ref_sql if side == 'reference' else sql
                        inferred = observe(prepare(conn, b'inferred_returning', actual_sql.encode(), 0, None))
                        archive(name + '_infer_prepare', side, dict(phase='PQprepare', sql=actual_sql, statement='inferred_returning', parameter_oids=[], arguments=None, parameter_formats=None, result_format=0), inferred)
                        check(inferred['status'] == 1 and inferred['state'] == '', 'target-column parameter inference failed')
                        inferred_desc = observe(describe(conn, b'inferred_returning'))
                        archive(name + '_infer_describe', side, dict(phase='PQdescribePrepared', statement='inferred_returning'), inferred_desc)
                        check(inferred_desc['params'] == [23] and inferred_desc['fields'] == descriptors[side]['fields'], 'inferred parameter/result type mismatch')
                        check(observe(close_prepared(conn, b'inferred_returning'))['status'] == 1, 'inferred statement close failed')
                        check(snapshot(side) == before[side], 'inference wrote rows')
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
                       returning_metadata_success=gate_cases if done and cleanup_ok else 0,
                       scope='readonly RETURNING Prepare/Describe; execution remains gated and has zero semantic success')
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
    assert done and cleanup_ok, 'incomplete run/cleanup'
    print(json.dumps(summary), flush=True)

def delete_returning_handoff():
    """Describe DELETE RETURNING without local writes; keep DELETE USING gated."""
    scope = f'dialect_06_{os.getpid()}_{int(sys.argv[2])}'
    local_dsn = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
    connections = {}
    prepared = []
    checks = 0
    self_checks = 0
    eligible = 0
    registry = []
    reference_schema_created = False
    done = False
    cleanup_ok = True

    def check(condition, message):
        nonlocal checks
        assert condition, message
        checks += 1

    def recovery(conn, side, name):
        if transaction(conn) in (2, 3):
            check(query(conn, 'ROLLBACK')['status'] == 1, name + ': rollback failed')
        probe = query(conn, 'SELECT 1::int4')
        archive(name + '_recovery', side, dict(phase='PQexec', sql='SELECT 1::int4'), probe)
        check(probe['status'] == 2 and probe['rows'] == [['1']] and transaction(conn) == 0,
              name + ': same-connection recovery failed')

    def snapshot(side):
        conn = connections[side + '_observer']
        relation = f'{scope}.t' if side == 'reference' else 'public.t'
        result = query(conn, f'SELECT id, v FROM {relation} ORDER BY id')
        check(result['status'] == 2 and result['state'] == '', side + ': observer failed')
        return result['rows']

    try:
        reference_dsn = os.environ['PG18_REFERENCE_DSN']
        for label in ['reference', 'reference_observer', 'aster', 'aster_b', 'aster_observer']:
            dsn = reference_dsn if label.startswith('reference') else local_dsn
            conn = connect(dsn.encode())
            connections[label] = conn
            check(bool(conn) and status(conn) == 0, label + ': connection failed')
            if label.startswith('reference'):
                check(query(conn, "SET TIME ZONE 'UTC'")['status'] == 1, 'reference timezone setup failed')
            check(parameter_status(conn, b'client_encoding') == b'UTF8', label + ': encoding mismatch')
            check(parameter_status(conn, b'TimeZone') == b'UTC', label + ': timezone mismatch')
            check(query(conn, 'SET search_path TO public')['status'] == 1, label + ': search_path failed')

        reference_version = query(connections['reference'],
                                  "SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(reference_version[1] == '180006', 'exact PostgreSQL 18.6 reference required')
        reference_build = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        build = json.loads((evidence / 'rust-build.json').read_text())
        build['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance = dict(
            run=evidence.name, target_version='18.6', source=source_snapshot(root), build=build,
            client=dict(python=sys.version, library=str(library),
                        sha256=hashlib.sha256(library.read_bytes()).hexdigest(), PQlibVersion=version()),
            os=platform.platform(), storage='CreateAnalyzeSession test storage',
            authentication='InsecureRootOnly',
            reference=dict(version=reference_version[0], server_version_num=reference_version[1]),
            reference_build=reference_build,
            endpoints={side: endpoint(connections[side]) for side in ['aster', 'aster_b', 'reference']})
        (evidence / 'provenance.json').write_text(json.dumps(provenance, indent=2))
        check(validate_provenance(provenance, copy.deepcopy(provenance)), 'provenance self-check failed')
        damaged = copy.deepcopy(provenance)
        damaged['reference']['server_version_num'] = '180004'
        check(not validate_provenance(damaged, provenance), 'provenance tamper check failed')
        self_checks += 1

        reference = connections['reference']
        check(query(reference, f'CREATE SCHEMA {scope}')['status'] == 1, 'reference schema creation failed')
        reference_schema_created = True
        for side in ['reference', 'aster']:
            prefix = f'{scope}.' if side == 'reference' else 'public.'
            conn = connections[side]
            check(query(conn, f'CREATE TABLE {prefix}t (id INT, v VARCHAR(20))')['status'] == 1,
                  side + ': target table creation failed')
            check(query(conn, f'CREATE TABLE {prefix}s (id INT)')['status'] == 1,
                  side + ': source table creation failed')
            check(query(conn, f"INSERT INTO {prefix}t VALUES (1, 'one'), (2, 'two')")['status'] == 1,
                  side + ': target seed failed')
            check(query(conn, f'INSERT INTO {prefix}s VALUES (1), (2)')['status'] == 1,
                  side + ': source seed failed')

        expected_fields = {
            'projection': [['id', 23, 4, -1], ['id', 23, 4, -1], ['v', 1043, -1, 24]],
            'zero_rows': [['id', 23, 4, -1]],
            'typed_null': [['id', 23, 4, -1]],
            'quoted_alias': [['Deleted_ID', 23, 4, -1]],
            'star_only': [['id', 23, 4, -1], ['v', 1043, -1, 24]],
            'using': [['id', 23, 4, -1], ['source_id', 23, 4, -1]],
            'using_unaliased': [['id', 23, 4, -1], ['source_id', 23, 4, -1]],
            'protected': [],
        }
        expected_params = {'projection': [23], 'typed_null': [23]}
        expected_reference_rows = {
            'projection': [['2', '2', 'two']],
            'zero_rows': [],
            'typed_null': [],
            'quoted_alias': [['1']],
            'star_only': [['1', 'one'], ['2', 'two']],
            'using': [['1', '1'], ['2', '2']],
            'using_unaliased': [['1', '1'], ['2', '2']],
        }
        for index, (name, sql, oids) in enumerate(cases):
            is_invalid = name in ('missing_projection', 'misplaced_where')
            is_gate = name in ('using', 'using_unaliased')
            ref_sql = sql.replace('public.t', f'{scope}.t').replace('public.s', f'{scope}.s')
            entry = dict(
                case_id=name, capability_id='pg.dialect.delete.returning.metadata',
                sql=sql, reference_sql=ref_sql, phase='PQprepare/PQdescribePrepared',
                parameter_oids=oids,
                arguments=(['2'] if name == 'projection' else [None] if name == 'typed_null' else []),
                parameter_formats=[0] * len(oids), result_format=0,
                encoding='UTF8', timezone='UTC',
                role={side: connection_user(connections[side]).decode() for side in ['aster', 'reference']},
                search_path='public', initial_state='I',
                object_mapping={'aster': f'{sys.argv[3]}.public.t', 'reference': f'{scope}.t'},
                expected_basis='task6 ordered DELETE RETURNING projection and PostgreSQL18.6/libpq18',
                expected={'reference': '42601' if is_invalid else 'success',
                          'aster': '42601' if is_invalid else ('0A000' if is_gate else 'success')},
                source_positions={'using_keyword': sql.index('USING') + 1 if is_gate else None},
                execution='Prepare/Describe only locally; reference DELETE RETURNING runs in a rolled-back transaction',
                side_effects='independent snapshots before/after every case must remain unchanged')
            registry.append(entry)
            (evidence / 'cases.json').write_text(json.dumps(registry, indent=2))
            before = {side: snapshot(side) for side in ['reference', 'aster']}
            observations, descriptions = {}, {}
            for side in ['reference', 'aster']:
                conn = connections['reference' if side == 'reference' else ('aster_b' if index % 2 else 'aster')]
                actual_sql = ref_sql if side == 'reference' else sql
                statement = f'delete_{index}'.encode()
                type_array = (c.c_uint * len(oids))(*oids) if oids else None
                result = observe(prepare(conn, statement, actual_sql.encode(), len(oids), type_array))
                result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                archive(name, side, dict(phase='PQprepare', sql=actual_sql,
                                         statement=statement.decode(), parameter_oids=oids), result)
                observations[side] = result
                if result['status'] == 1:
                    prepared.append((conn, statement))
                    desc = observe(describe(conn, statement))
                    archive(name + '_describe', side,
                            dict(phase='PQdescribePrepared', statement=statement.decode()), desc)
                    descriptions[side] = desc
                    check(desc['status'] == 1 and desc['state'] == '', side + ': Describe failed')
                    check(desc['fields'] == expected_fields.get(name, []),
                          f'{side}/{name}: projection order or types changed: {desc["fields"]}')
                    check(desc['params'] == expected_params.get(name, []),
                          f'{side}/{name}: parameter types changed: {desc["params"]}')
                    check(desc['formats'] == [0] * len(expected_fields.get(name, [])),
                          side + ': text result format was not preserved')
                    if side == 'aster' and name in ('projection', 'zero_rows', 'typed_null', 'quoted_alias', 'star_only'):
                        expected_attributes = {
                            'projection': [1, 1, 2], 'zero_rows': [1], 'typed_null': [1],
                            'quoted_alias': [1], 'star_only': [1, 2],
                        }[name]
                        check([origin[1] for origin in desc['table_origins']] == expected_attributes,
                              side + ': DELETE target column provenance changed')
                        check(all(origin[0] != 0 for origin in desc['table_origins']),
                              side + ': DELETE target table provenance is missing')
                    if side == 'aster' and name == 'projection':
                        values, lengths, formats = (text * 1)(b'2'), (integer * 1)(1), (integer * 1)(0)
                        execution = observe(exec_prepared(conn, statement, 1, values, lengths, formats, 0))
                        execution['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                        archive(name + '_execute_gate', side,
                                dict(phase='PQexecPrepared', statement=statement.decode(), arguments=['2'],
                                     parameter_formats=[0], result_format=0), execution)
                        check(execution['status'] == 7 and execution['state'] == '0A000',
                              'local DELETE RETURNING Execute must stop before DML dispatch')
                        check(execution['transaction_after'] == 'I',
                              'rejected DELETE RETURNING left the connection in a transaction error')
                    if side == 'reference' and name in expected_reference_rows:
                        check(query(conn, 'BEGIN')['status'] == 1, name + ': reference BEGIN failed')
                        if name in ('projection', 'typed_null'):
                            argument = b'2' if name == 'projection' else None
                            values = (text * 1)(argument)
                            lengths, formats = (integer * 1)(0), (integer * 1)(0)
                            execution = observe(exec_prepared(conn, statement, 1, values, lengths, formats, 0))
                        else:
                            execution = observe(exec_prepared(conn, statement, 0, None, None, None, 0))
                        archive(name + '_reference_execute', side,
                                dict(phase='PQexecPrepared', statement=statement.decode(),
                                     arguments=(['2'] if name == 'projection' else [None] if name == 'typed_null' else []),
                                     parameter_formats=[0] * len(oids), result_format=0), execution)
                        check(execution['status'] == 2 and execution['state'] == '' and
                              execution['rows'] == expected_reference_rows[name],
                              f'PostgreSQL DELETE RETURNING old-row values changed: {execution}')
                        check(query(conn, 'ROLLBACK')['status'] == 1, name + ': reference rollback failed')
                else:
                    expected_state = '42601' if is_invalid else ('0A000' if is_gate and side == 'aster' else '')
                    expected_status = 7 if expected_state else 1
                    check(result['state'] == expected_state and result['status'] == expected_status,
                          f'{side}/{name}: unexpected Parse result {result}')
                    if is_gate and side == 'aster':
                        expected_position = actual_sql.index('USING') + 1
                        check(result['position'] == str(expected_position),
                              f'{side}/{name}: DELETE USING gate position changed: {result["position"]}')
                    if is_invalid:
                        expected_position = len(actual_sql) + 1 if name == 'missing_projection' else len(actual_sql[:actual_sql.index('WHERE', actual_sql.index('RETURNING'))]) + 1
                        check(result['position'] == str(expected_position),
                              f'{side}/{name}: PostgreSQL error position changed: {result["position"]}')
                        changed = copy.deepcopy(result)
                        changed['state'] = '0A000'
                        assert not (changed['state'] == '42601' and changed['status'] == 7 and
                                    changed['position'] == str(expected_position))
                        self_checks += 1
                    check(result['transaction_after'] == 'I', side + ': failed Parse left transaction dirty')

            if not is_gate and not is_invalid:
                check(compare_metadata(descriptions['aster'], descriptions['reference'], 'describe'),
                      name + ': local/reference metadata differ')
            if not is_gate and not is_invalid:
                eligible += 1
            if name in descriptions:
                statement = f'delete_{index}'.encode()
                for side in ['aster', 'reference']:
                    conn = connections['reference' if side == 'reference' else ('aster_b' if index % 2 else 'aster')]
                    if (conn, statement) in prepared:
                        check(observe(close_prepared(conn, statement))['status'] == 1,
                              side + ': prepared statement close failed')
                        prepared.remove((conn, statement))
            after = {side: snapshot(side) for side in ['reference', 'aster']}
            check(after == before, name + ': Parse/Describe or rolled-back execution changed rows')
            print('PG_DIALECT_CHECKPOINT', name, flush=True)
            check(sys.stdin.readline().strip() == 'continue', 'MySQL checkpoint aborted')
            for label in ['aster', 'aster_b', 'reference']:
                recovery(connections[label], 'reference' if label == 'reference' else 'aster',
                         name + '_' + label)

        parsed = urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
        check(parsed.scheme == 'mysql' and bool(parsed.hostname) and bool(parsed.port), 'MySQL reference DSN malformed')
        mysql_env = dict(os.environ, MYSQL_PWD=parsed.password or '')
        mysql_args = ['mysql', '--protocol=TCP', '-h', parsed.hostname, '-P', str(parsed.port),
                      '-u', parsed.username or 'root', '--batch', '--raw', '--skip-column-names']
        def mysql_ref(sql):
            return subprocess.run(mysql_args + ['-e', "SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; " + sql],
                                  env=mysql_env, capture_output=True, text=True, timeout=15)
        identity = mysql_ref('SELECT VERSION(), @@version_compile_os, @@version_compile_machine; SELECT @@sql_mode, @@character_set_connection, @@collation_connection')
        archive('reference_identity', 'mysql', None,
                dict(code=identity.returncode, output=identity.stdout.splitlines(), error=identity.stderr))
        check(identity.returncode == 0 and bool(identity.stdout.strip()), 'MySQL reference identity unavailable')
        check(identity.stdout.splitlines()[0].startswith('8.4.7\t'), 'exact MySQL 8.4.7 reference required')
        raw_mysql_build = json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text())
        raw_mysql_build = raw_mysql_build[0] if isinstance(raw_mysql_build, list) else raw_mysql_build
        mysql_build = {key: raw_mysql_build[key] for key in
                       ('Id', 'RepoDigests', 'Created', 'Architecture', 'Variant', 'Os', 'Size') if key in raw_mysql_build}
        mysql_build['version'] = identity.stdout.splitlines()[0].split('\t')[0]
        (evidence / 'mysql-reference-build.json').write_text(json.dumps(mysql_build, indent=2))
        mysql_sql = 'DELETE FROM t AS a USING s AS b WHERE a.id=b.id RETURNING a.id'
        mysql_result = mysql_ref(mysql_sql)
        mysql_observation = dict(code=mysql_result.returncode, output=mysql_result.stdout.splitlines(), error=mysql_result.stderr)
        archive('delete_using_syntax', 'mysql', dict(phase='query', sql=mysql_sql), mysql_observation)
        check(mysql_result.returncode != 0 and 'ERROR 1064 (42000)' in mysql_result.stderr,
              'MySQL reference did not reject PostgreSQL DELETE USING RETURNING syntax')
        (evidence / 'mysql-reference.json').write_text(json.dumps(
            dict(identity=identity.stdout.splitlines(), sql_mode='empty', character_set='utf8mb4',
                 collation='utf8mb4_general_ci', build_file='mysql-reference-build.json',
                 delete_using={'code': mysql_result.returncode, 'error': mysql_result.stderr}), indent=2))

        final = copy.deepcopy(provenance)
        final['source'] = source_snapshot(root)
        final['build']['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        final['client']['sha256'] = hashlib.sha256(library.read_bytes()).hexdigest()
        final['reference_build'] = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        final['endpoints'] = {side: endpoint(connections[side]) for side in ['aster', 'aster_b', 'reference']}
        check(validate_provenance(provenance, final), 'build/source/provenance changed during handoff')
        done = True
    finally:
        for conn, statement in prepared:
            if conn:
                observe(close_prepared(conn, statement))
        for conn in connections.values():
            if conn and transaction(conn) in (2, 3):
                cleanup_ok = query(conn, 'ROLLBACK')['status'] == 1 and cleanup_ok
        if reference_schema_created and connections.get('reference'):
            cleanup_ok = query(connections['reference'], f'DROP SCHEMA {scope} CASCADE')['status'] == 1 and cleanup_ok
        for conn in connections.values():
            if conn:
                finish(conn)
        summary = dict(cases=len(cases), assertions=checks + int(done), self_checks=self_checks,
                       unique_effective=eligible if done and cleanup_ok else 0,
                       using_gate_cases=2 if done and cleanup_ok else 0,
                       semantic_returning_success=0, cleanup_ok=cleanup_ok,
                       scope='read-only local DELETE RETURNING Prepare/Describe; DELETE USING gate; reference execution rolls back')
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    check(done and cleanup_ok, 'completion/cleanup')
    print(json.dumps(summary), flush=True)


def update_returning_handoff():
    """Bind UPDATE RETURNING read-only and keep UPDATE FROM behind its gate."""
    scope = f'dialect_05_{os.getpid()}_{int(sys.argv[2])}'
    local_dsn = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
    connections = {}
    prepared = []
    checks = 0
    self_checks = 0
    effective = 0
    gate_cases = 0
    reference_schema_created = False
    done = False
    cleanup_ok = True
    registry = []

    def check(condition, message):
        nonlocal checks
        assert condition, message
        checks += 1

    def recovery(conn, side, name):
        if transaction(conn) in (2, 3):
            check(query(conn, 'ROLLBACK')['status'] == 1, name + ': rollback failed')
        probe = query(conn, 'SELECT 1::int4')
        archive(name + '_recovery', side, dict(phase='PQexec', sql='SELECT 1::int4'), probe)
        check(probe['status'] == 2 and probe['rows'] == [['1']] and transaction(conn) == 0,
              name + ': same-connection recovery failed')

    def snapshot(side):
        conn = connections[side + '_observer']
        relation = f'{scope}.t' if side == 'reference' else 'public.t'
        result = query(conn, f'SELECT id, v FROM {relation} ORDER BY id')
        check(result['status'] == 2 and result['state'] == '', side + ': observer failed')
        return result['rows']

    try:
        reference_dsn = os.environ['PG18_REFERENCE_DSN']
        for label in ['reference', 'reference_observer', 'aster', 'aster_b', 'aster_observer']:
            dsn = reference_dsn if label.startswith('reference') else local_dsn
            conn = connect(dsn.encode())
            connections[label] = conn
            check(bool(conn) and status(conn) == 0, label + ': connection failed')
            if label.startswith('reference'):
                check(query(conn, "SET TIME ZONE 'UTC'")['status'] == 1, 'reference timezone setup failed')
            check(parameter_status(conn, b'client_encoding') == b'UTF8', label + ': encoding mismatch')
            check(parameter_status(conn, b'TimeZone') == b'UTC', label + ': timezone mismatch')
            check(query(conn, 'SET search_path TO public')['status'] == 1, label + ': search_path failed')

        reference_version = query(connections['reference'],
                                  "SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(reference_version[1] == '180006', 'exact PostgreSQL 18.6 reference required')
        reference_build = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        build = json.loads((evidence / 'rust-build.json').read_text())
        build['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance = dict(
            run=evidence.name, target_version='18.6', source=source_snapshot(root), build=build,
            client=dict(python=sys.version, library=str(library),
                        sha256=hashlib.sha256(library.read_bytes()).hexdigest(), PQlibVersion=version()),
            os=platform.platform(), storage='CreateAnalyzeSession test storage',
            authentication='InsecureRootOnly',
            reference=dict(version=reference_version[0], server_version_num=reference_version[1]),
            reference_build=reference_build,
            endpoints={side: endpoint(connections[side]) for side in ['aster', 'aster_b', 'reference']})
        (evidence / 'provenance.json').write_text(json.dumps(provenance, indent=2))
        check(validate_provenance(provenance, copy.deepcopy(provenance)), 'provenance self-check failed')
        damaged = copy.deepcopy(provenance)
        damaged['reference']['server_version_num'] = '180004'
        check(not validate_provenance(damaged, provenance), 'provenance tamper check failed')
        self_checks += 1

        reference = connections['reference']
        check(query(reference, f'CREATE SCHEMA {scope}')['status'] == 1, 'reference schema creation failed')
        reference_schema_created = True
        for side in ['reference', 'aster']:
            prefix = f'{scope}.' if side == 'reference' else 'public.'
            conn = connections[side]
            check(query(conn, f'CREATE TABLE {prefix}t (id INT, v VARCHAR(20))')['status'] == 1,
                  side + ': target table creation failed')
            check(query(conn, f'CREATE TABLE {prefix}s (id INT)')['status'] == 1,
                  side + ': source table creation failed')
            check(query(conn, f"INSERT INTO {prefix}t VALUES (1, 'one'), (2, 'two')")['status'] == 1,
                  side + ': target seed failed')
            check(query(conn, f'INSERT INTO {prefix}s VALUES (1), (2)')['status'] == 1,
                  side + ': source seed failed')

        expected_fields = {
            'projection': [['ID', 23, 4, -1], ['id', 23, 4, -1], ['v', 1043, -1, 24]],
            'zero_rows': [['id', 23, 4, -1]],
            'from_returning': [['source_id', 23, 4, -1], ['id', 23, 4, -1], ['v', 1043, -1, 24]],
            'protected': [['id', 23, 4, -1]],
        }
        expected_params = {'projection': [23], 'from_returning': [23]}
        for index, (name, sql, oids) in enumerate(cases):
            is_invalid = name in ('missing_projection', 'misplaced_where')
            is_gate = name == 'from_returning'
            ref_sql = sql.replace('public.t', f'{scope}.t').replace('public.s', f'{scope}.s')
            entry = dict(
                case_id=name, capability_id='pg.dialect.update.returning.metadata',
                sql=sql, reference_sql=ref_sql, phase='PQprepare', parameter_oids=oids,
                arguments=None, parameter_formats=None, result_format=0, encoding='UTF8', timezone='UTC',
                role={side: connection_user(connections[side]).decode() for side in ['aster', 'reference']},
                search_path='public', initial_state='I',
                object_mapping={'aster': f'{sys.argv[3]}.public.t', 'reference': f'{scope}.t'},
                expected_basis='task5 ordered UPDATE RETURNING projection and PostgreSQL18.6/libpq18',
                expected={'reference': '42601' if is_invalid else 'success',
                          'aster': '42601' if is_invalid else ('0A000' if is_gate else 'success')},
                execution='Prepare/Describe only on both references; one local Execute probes the pre-write gate',
                side_effects='independent snapshots before/after every case must remain unchanged')
            registry.append(entry)
            (evidence / 'cases.json').write_text(json.dumps(registry, indent=2))
            before = {side: snapshot(side) for side in ['reference', 'aster']}
            observations = {}
            descriptions = {}
            for side in ['reference', 'aster']:
                conn = connections['reference' if side == 'reference' else ('aster_b' if index % 2 else 'aster')]
                actual_sql = ref_sql if side == 'reference' else sql
                statement = f'update_{index}'.encode()
                type_array = (c.c_uint * len(oids))(*oids) if oids else None
                result = observe(prepare(conn, statement, actual_sql.encode(), len(oids), type_array))
                result['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                archive(name, side, dict(phase='PQprepare', sql=actual_sql,
                                         statement=statement.decode(), parameter_oids=oids), result)
                observations[side] = result
                if result['status'] == 1:
                    prepared.append((conn, statement))
                    desc = observe(describe(conn, statement))
                    archive(name + '_describe', side,
                            dict(phase='PQdescribePrepared', statement=statement.decode()), desc)
                    descriptions[side] = desc
                    check(desc['status'] == 1 and desc['state'] == '', side + ': Describe failed')
                    check(desc['fields'] == expected_fields[name],
                          f'{side}/{name}: projection order or types changed: {desc["fields"]}')
                    check(desc['params'] == expected_params.get(name, []),
                          f'{side}/{name}: inferred parameter type changed: {desc["params"]}')
                    check(desc['formats'] == [0] * len(expected_fields[name]),
                          side + ': text result format was not preserved')
                    if name in ('projection', 'zero_rows') and side == 'aster':
                        attributes = [1, 1, 2] if name == 'projection' else [1]
                        check([origin[1] for origin in desc['table_origins']] == attributes,
                              side + ': target column provenance changed')
                        check(all(origin[0] != 0 for origin in desc['table_origins']),
                              side + ': target table provenance is missing')
                    if name == 'from_returning' and side == 'reference':
                        check([origin[1] for origin in desc['table_origins']] == [1, 1, 2],
                              'PostgreSQL UPDATE FROM lost source/target attribute provenance')
                        check(all(origin[0] != 0 for origin in desc['table_origins']),
                              'PostgreSQL UPDATE FROM source table identity is missing')
                    if name == 'projection' and side == 'aster':
                        values = (text * 1)(b'7')
                        lengths = (integer * 1)(1)
                        formats = (integer * 1)(0)
                        execution = observe(exec_prepared(conn, statement, 1, values, lengths, formats, 0))
                        execution['transaction_after'] = {0: 'I', 2: 'T', 3: 'E'}.get(transaction(conn))
                        archive(name + '_execute_gate', side,
                                dict(phase='PQexecPrepared', statement=statement.decode(),
                                     arguments=['7'], parameter_formats=[0], result_format=0), execution)
                        check(execution['status'] == 7 and execution['state'] == '0A000',
                              'local UPDATE RETURNING Execute must stop before DML dispatch')
                        check(execution['transaction_after'] == 'I',
                              'rejected UPDATE RETURNING left the connection in a transaction error')
                else:
                    expected_state = '42601' if is_invalid else ('0A000' if is_gate and side == 'aster' else '')
                    expected_status = 7 if expected_state else 1
                    check(result['state'] == expected_state and result['status'] == expected_status,
                          f'{side}/{name}: unexpected Parse result {result}')
                    if is_invalid:
                        expected_position = (len(actual_sql) + 1 if name == 'missing_projection'
                                             else len(actual_sql[:actual_sql.index('WHERE', actual_sql.index('RETURNING'))]) + 1)
                        check(result['position'] == str(expected_position),
                              f'{side}/{name}: PostgreSQL error position changed: {result["position"]}')
                    check(result['transaction_after'] == 'I', side + ': failed Parse left transaction dirty')

            if not is_gate and not is_invalid:
                check(compare_metadata(descriptions['aster'], descriptions['reference'], 'describe'),
                      name + ': local/reference metadata differ')
            if is_gate:
                gate_cases += 1
            else:
                effective += 1
            after = {side: snapshot(side) for side in ['reference', 'aster']}
            check(after == before, name + ': Prepare/Describe changed rows')
            print('PG_DIALECT_CHECKPOINT', name, flush=True)
            check(sys.stdin.readline().strip() == 'continue', 'MySQL checkpoint aborted')
            for label in ['aster', 'aster_b', 'reference']:
                recovery(connections[label], 'reference' if label == 'reference' else 'aster',
                         name + '_' + label)

        parsed = urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
        check(parsed.scheme == 'mysql' and bool(parsed.hostname) and bool(parsed.port), 'MySQL reference DSN malformed')
        mysql_env = dict(os.environ, MYSQL_PWD=parsed.password or '')
        mysql_args = ['mysql', '--protocol=TCP', '-h', parsed.hostname, '-P', str(parsed.port),
                      '-u', parsed.username or 'root', '--batch', '--raw', '--skip-column-names']
        def mysql_ref(sql):
            return subprocess.run(mysql_args + ['-e', "SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; " + sql],
                                  env=mysql_env, capture_output=True, text=True, timeout=15)
        identity = mysql_ref('SELECT VERSION(), @@version_compile_os, @@version_compile_machine; SELECT @@sql_mode, @@character_set_connection, @@collation_connection')
        archive('reference_identity', 'mysql', None,
                dict(code=identity.returncode, output=identity.stdout.splitlines(), error=identity.stderr))
        check(identity.returncode == 0 and bool(identity.stdout.strip()), 'MySQL reference identity unavailable')
        check(identity.stdout.splitlines()[0].startswith('8.4.7\t'),
              'exact MySQL 8.4.7 reference required')
        raw_mysql_build = json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text())
        raw_mysql_build = raw_mysql_build[0] if isinstance(raw_mysql_build, list) else raw_mysql_build
        mysql_build = {
            key: raw_mysql_build[key]
            for key in ('Id', 'RepoDigests', 'Created', 'Architecture', 'Variant', 'Os', 'Size')
            if key in raw_mysql_build
        }
        mysql_build['version'] = identity.stdout.splitlines()[0].split('\t')[0]
        mysql_build['version_compile_os'] = identity.stdout.splitlines()[0].split('\t')[1]
        mysql_build['version_compile_machine'] = identity.stdout.splitlines()[0].split('\t')[2]
        (evidence / 'mysql-reference-build.json').write_text(json.dumps(mysql_build, indent=2))
        mysql_sql = 'UPDATE t AS a SET id=3 FROM s AS b WHERE a.id=b.id RETURNING b.id'
        mysql_result = mysql_ref(mysql_sql)
        mysql_observation = dict(code=mysql_result.returncode, output=mysql_result.stdout.splitlines(),
                                 error=mysql_result.stderr)
        archive('update_from_syntax', 'mysql', dict(phase='query', sql=mysql_sql), mysql_observation)
        check(mysql_result.returncode != 0 and 'ERROR 1064 (42000)' in mysql_result.stderr,
              'MySQL reference did not reject PostgreSQL UPDATE FROM syntax')
        (evidence / 'mysql-reference.json').write_text(json.dumps(
            dict(identity=identity.stdout.splitlines(), sql_mode='empty', character_set='utf8mb4',
                 build_file='mysql-reference-build.json',
                 collation='utf8mb4_general_ci', update_from={'code': mysql_result.returncode,
                 'error': mysql_result.stderr}), indent=2))

        final = copy.deepcopy(provenance)
        final['source'] = source_snapshot(root)
        final['build']['sha256'] = hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        final['client']['sha256'] = hashlib.sha256(library.read_bytes()).hexdigest()
        final['reference_build'] = json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        final['endpoints'] = {side: endpoint(connections[side]) for side in ['aster', 'aster_b', 'reference']}
        check(validate_provenance(provenance, final), 'build/source/provenance changed during handoff')
        done = True
    finally:
        for conn, statement in prepared:
            if conn:
                observe(close_prepared(conn, statement))
        for conn in connections.values():
            if conn and transaction(conn) in (2, 3):
                cleanup_ok = query(conn, 'ROLLBACK')['status'] == 1 and cleanup_ok
        if reference_schema_created and connections.get('reference'):
            cleanup_ok = query(connections['reference'], f'DROP SCHEMA {scope} CASCADE')['status'] == 1 and cleanup_ok
        for conn in connections.values():
            if conn:
                finish(conn)
        summary = dict(cases=len(cases), assertions=checks + int(done), self_checks=self_checks,
                       unique_effective=effective if done and cleanup_ok else 0,
                       update_from_gate_cases=gate_cases if done and cleanup_ok else 0,
                       update_execute_gate_cases=1 if done and cleanup_ok else 0,
                       syntax_eligible=len(cases) if done and cleanup_ok else 0,
                       mysql_reference_cases=2 if done and cleanup_ok else 0,
                       cleanup_ok=cleanup_ok,
                       scope='read-only UPDATE RETURNING Prepare/Describe; UPDATE FROM gate; no DML execution')
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    check(done and cleanup_ok, 'completion/cleanup')
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
        if name == 'empty':
            return all(x['state']=='42P18' and x['status']==7 and x['position']=='8' for x in [a,r])
        return all(x['state']=='' and x['status']==1 and not x['position'] for x in [a,r])
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
                expected_basis='task9 full flat-array analysis, prepare/bind/execute and exact PostgreSQL18.6 reference')
            results={}
            executions={}
            descriptions={}
            for label in ['reference', 'aster_b' if index%2 else 'aster']:
                conn=connections[label];check(transaction(conn)==0,'initial transaction not idle')
                statement=f'array_{index}'.encode()
                result=observe(prepare(conn,statement,sql.encode(),len(oids),(c.c_uint*len(oids))(*oids) if oids else None))
                result['transaction_after']={0:'I',2:'T',3:'E'}.get(transaction(conn))
                record(name,label,input_data,result);results['reference' if label=='reference' else 'aster']=result
                if result['status']==1:
                    prepared.append((conn,statement))
                    desc=observe(describe(conn,statement));record(name+'_describe',label,dict(input_data,phase='Describe'),desc);descriptions[label]=desc
                    if name in ('list','null','parameters'):
                        check(desc['fields'][0][1:]==[1007,-1,-1], 'reference integer[] metadata')
                        if oids:check(desc['param_count']==2 and desc['params']==[23,23],'reference parameter identity')
                        args=(text*2)(b'7',b'9') if oids else None
                        values=observe(exec_prepared(conn,statement,len(oids),args,None,None,0));record(name+'_execute',label,dict(input_data,phase='Execute'),values)
                        expected={'list':'{1,2}','null':'{NULL,1}','parameters':'{9,7,9}'}[name]
                        check(values['rows']==[[expected]] and values['command_tag']=='SELECT 1','reference array values')
                        check(values['fields'][0][1:]==[1007,-1,-1] and values['formats']==[0], 'execute array type identity')
                        executions[label]=values
                        for repeat, arguments in enumerate(([None,b'9'],[None,None],[b'-2147483648',b'2147483647']) if oids else [[]]):
                            args=(text*len(arguments))(*arguments) if arguments else None
                            for result_format in [0,1]:
                                rebound=observe(exec_prepared(conn,statement,len(arguments),args,None,None,result_format),binary=result_format==1)
                                record(name+f'_rebind_{repeat}_{result_format}',label,dict(input_data,phase='Bind/Execute',arguments=[None if v is None else v.decode() for v in arguments],result_format=result_format),rebound)
                                check(rebound['state']=='' and rebound['status']==2 and rebound['fields'][0][1:]==[1007,-1,-1] and rebound['formats']==[result_format], 'repeated Bind fixed array identity')
                                executions[label+f'_{repeat}_{result_format}']=rebound
                                if result_format==0 and oids:
                                    check(rebound['rows']==[[['{9,NULL,9}','{NULL,NULL,NULL}','{2147483647,-2147483648,2147483647}'][repeat]]], 'repeated Bind values/NULL')

                    if name=='protected':
                        values=observe(exec_prepared(conn,statement,0,None,None,None,0));record(name+'_execute',label,dict(input_data,phase='Execute'),values)
                        check(values['rows']==[['ARRAY[1,2]']],'protected value')
                recover(conn,label,name)
            check(compare(results['aster'],results['reference'],name,sql),'prepare contract mismatch: '+name)
            local_label='aster_b' if index%2 else 'aster'
            if name in ('list','null','parameters'):
                check(descriptions[local_label]['fields']==descriptions['reference']['fields'] and descriptions[local_label]['params']==descriptions['reference']['params'] and descriptions[local_label]['formats']==descriptions['reference']['formats'], 'Describe parity')
                for key, actual in executions.items():
                    if key.startswith(local_label):
                        reference=executions['reference'+key[len(local_label):]]
                        check(all(actual[field]==reference[field] for field in ['rows','fields','formats','command_tag','table_origins']), 'Execute/rebind text/binary parity')

            if name in ('list','empty','missing_close'):
                damaged=copy.deepcopy(results['aster']);damaged['state']='42601' if name!='missing_close' else '0A000'
                check(not compare(damaged,results['reference'],name,sql),'comparator negative check');self_checks+=1
            for label in ['aster','reference']:
                result=query(connections[label],sql);record(name+'_simple',label,dict(input_data,phase='Q',arguments=[],parameter_oids=[]),result)
                if name=='protected':check(result['rows']==[['ARRAY[1,2]']],'simple literal changed')
                elif name=='missing_close':check(result['state']=='42601' and result['position']==str(len(sql)+1),'simple original position')
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
        variants = [
            ('list_int8','SELECT ARRAY[1,2147483648]',[],[],1016,'{1,2147483648}'),
            ('null_all','SELECT ARRAY[NULL,NULL]',[],[],1009,'{NULL,NULL}'),
            ('text_escaping',"SELECT ARRAY['中,文','NULL','']",[],[],1009,'{"中,文","NULL",""}'),
            ('parameters_inferred','SELECT ARRAY[$2,$1,$2]',[],[b'a,b',None],1009,'{NULL,"a,b",NULL}'),
            ('parameters_int8','SELECT ARRAY[$2,$1,$2]',[20,20],[b'7',b'9'],1016,'{9,7,9}'),
        ]
        (evidence/'array-variants.json').write_text(json.dumps([
            dict(case_id=name,sql=sql,parameter_oids=oids,arguments=[None if v is None else v.decode() for v in arguments],expected_array_oid=oid,expected_value=expected)
            for name,sql,oids,arguments,oid,expected in variants],ensure_ascii=False,indent=2))
        for name,sql,oids,arguments,oid,expected in variants:
            sides={}
            for label in ['reference','aster']:
                conn=connections[label];statement=('variant_'+name).encode()
                result=observe(prepare(conn,statement,sql.encode(),len(oids),(c.c_uint*len(oids))(*oids) if oids else None))
                record(name+'_prepare',label,dict(phase='PQprepare',sql=sql,parameter_oids=oids),result)
                check(result['state']=='' and result['status']==1,'array variant Prepare')
                prepared.append((conn,statement))
                desc=observe(describe(conn,statement));record(name+'_describe',label,dict(phase='Describe',sql=sql),desc)
                check(desc['fields']==[['array',oid,-1,-1]],'array variant identity')
                if arguments:check(desc['params']==([25,25] if name=='parameters_inferred' else [20,20]),'array variant parameter inference')
                for fmt in [0,1]:
                    values=observe(exec_prepared(conn,statement,len(arguments),(text*len(arguments))(*arguments) if arguments else None,None,None,fmt),binary=fmt==1)
                    record(name+f'_execute_{fmt}',label,dict(phase='Bind/Execute',sql=sql,parameter_oids=oids,arguments=[None if v is None else v.decode() for v in arguments],result_format=fmt),values)
                    check(values['state']=='' and values['status']==2 and values['fields']==[['array',oid,-1,-1]] and values['formats']==[fmt],'array variant Execute identity')
                    if fmt==0:check(values['rows']==[[expected]],'array variant exact values')
                    sides[label+str(fmt)]=values
            for fmt in [0,1]:check(all(sides['aster'+str(fmt)][field]==sides['reference'+str(fmt)][field] for field in ['fields','formats','rows','command_tag','table_origins']),'array variant text/binary parity')
        for label in ['aster','reference']:
            conn=connections[label]
            for name,sql,state in [('syntax','SELECT ARRAY[1,2','42601'),('empty_unicode','/*中*/ SELECT ARRAY[]','42P18')]:
                check(query(conn,'BEGIN')['status']==1,'begin failed')
                result=observe(prepare(conn,('transaction_'+name).encode(),sql.encode(),0,None))
                record('transaction_'+name,label,dict(phase='PQprepare',sql=sql),dict(result,transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conn))))
                check(result['state']==state and transaction(conn)==3,'transaction error state')
                check(result['position']==str(len(sql)+1 if state=='42601' else sql.index('ARRAY')+1),'transaction original character position')
                recover(conn,label,'transaction_'+name)
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
            gate_cases=0,unique_effective=eligible if done and cleanup_ok else 0,
            semantic_array_success=3 if done and cleanup_ok else 0,auxiliary_variants=5 if done and cleanup_ok else 0,cleanup_ok=cleanup_ok,scope='flat arrays through canonical prepare/bind/execute')
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and cleanup_ok,'incomplete run/cleanup'
    print(json.dumps(summary),flush=True)

def array_nesting_handoff():
    """Compare nested array shape, common types and execution with PG18.6."""
    connections, prepared = {}, []
    checks = 0
    self_checks = 0
    eligible = 0
    semantic_success = 0
    semantic_errors = 0
    done = False
    cleanup_ok = True

    def check(condition, message):
        nonlocal checks
        assert condition, message
        checks += 1

    def record(name, label, data, result):
        allowed = {'phase','sql','statement','parameter_oids','arguments',
                   'parameter_formats','result_format','command'}
        append_observation(evidence, dict(
            run=evidence.name, case_id=name,
            side='aster' if label.startswith('aster') else label,
            attempt=1,
            input={key:value for key,value in (data or {}).items() if key in allowed},
            observation=dict(result, connection=label)))

    def recover(conn, label, name):
        if transaction(conn) in (2,3):
            check(query(conn, 'ROLLBACK')['status'] == 1, 'rollback failed: '+name)
        result = query(conn, 'SELECT 1::int4')
        record(name+'_recovery', label, dict(phase='PQexec',sql='SELECT 1::int4'), result)
        check(result['rows']==[['1']] and result['state']=='' and transaction(conn)==0,
              'same-connection recovery failed: '+name)

    def same_diagnostic(actual, reference):
        return all(actual[field] == reference[field] for field in ('state','status','position'))

    expected_rows = {
        'explicit': '{{1,2},{3,4}}',
        'implicit': '{{1,2},{3,4}}',
        'numeric': '{1,2.5}',
        'parameters': '{{7,5},{5,7}}',
        'nulls': '{{NULL,1},{2,NULL}}',
    }
    expected_oid = {'numeric': 1231}
    for name in ('explicit','implicit','parameters','nulls'):
        expected_oid[name] = 1007

    def prepare_and_execute(conn, label, index, name, sql, oids, arguments):
        statement = f'nested_array_{index}_{label}'.encode()
        supplied = (c.c_uint*len(oids))(*oids) if oids else None
        base = dict(capability_id='pg.dialect.array.nesting',case_id=name,sql=sql,
                    phase='PQprepare',parameter_oids=oids,arguments=arguments,
                    parameter_formats=[0]*len(oids),result_format=0,
                    encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',
                    object_mapping={},expected_basis='PostgreSQL18.6 direct PQprepare/Describe/Execute/Q observation')
        check(transaction(conn)==0, label+' must start idle')
        result = observe(prepare(conn,statement,sql.encode(),len(oids),supplied))
        result['transaction_after']={0:'I',2:'T',3:'E'}.get(transaction(conn))
        record(name,label,base,result)
        expected_prepare = {'mixed_types':'42804','unclosed':'42601'}.get(name,'')
        check(result['state']==expected_prepare and result['status']==(7 if expected_prepare else 1),
              label+' Prepare SQLSTATE/status '+name)
        if name == 'mixed_types':
            check(result['position']==str(sql.index('true')+1),label+' type-error source position')
        if name == 'unclosed':
            check(result['position']==str(len(sql)+1),label+' EOF source position')
        description = None
        executions = {}
        if not expected_prepare:
            prepared.append((conn,statement))
            description=observe(describe(conn,statement))
            record(name+'_describe',label,dict(base,phase='Describe'),description)
            oid=expected_oid.get(name,1007)
            check(description['fields']==[['array',oid,-1,-1]],label+' array Describe identity '+name)
            if oids:
                check(description['param_count']==2 and description['params']==[23,23],
                      label+' repeated parameter metadata')
            if name == 'ragged':
                executed=observe(exec_prepared(conn,statement,0,None,None,None,0))
                record(name+'_execute',label,dict(base,phase='Execute'),executed)
                check(executed['state']=='2202E' and executed['status']==7,
                      label+' ragged dimensions must fail during Execute')
                executions['text']=executed
            elif name in expected_rows:
                formats=[0,1]
                values=(text*len(arguments))(*[None if value is None else value.encode() for value in arguments]) if arguments else None
                for result_format in formats:
                    executed=observe(exec_prepared(conn,statement,len(arguments),values,None,None,result_format),binary=result_format==1)
                    record(name+'_execute_'+str(result_format),label,
                           dict(base,phase='Bind/Execute',result_format=result_format),executed)
                    check(executed['state']=='' and executed['status']==2 and
                          executed['fields']==[['array',oid,-1,-1]] and
                          executed['formats']==[result_format],label+' array Execute identity '+name)
                    if result_format==0:
                        check(executed['rows']==[[expected_rows[name]]],label+' nested array value '+name)
                    executions[str(result_format)]=executed
        recover(conn,label,name+'_prepare')
        simple=query(conn,sql)
        record(name+'_simple',label,dict(base,phase='Q',parameter_oids=[],arguments=[]),simple)
        if name == 'unclosed':
            check(simple['state']=='42601' and simple['position']==str(len(sql)+1),label+' Q EOF position')
        elif name == 'mixed_types':
            check(simple['state']=='42804' and simple['position']==str(sql.index('true')+1),label+' Q type error position')
        elif name == 'parameters':
            check(simple['state']=='42P02',label+' unbound Q parameter state')
        elif name == 'ragged':
            check(simple['state']=='2202E',label+' Q ragged shape state')
        else:
            check(simple['state']=='' and simple['rows']==[[expected_rows[name]]],label+' Q array value '+name)
        recover(conn,label,name+'_simple')
        return result,description,executions,simple

    def mysql_reference(sql):
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN'])
        env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),
              '-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        command="SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; SET time_zone='+00:00'; "+sql
        proc=subprocess.run(args+['-e',command],env=env,capture_output=True,text=True,timeout=15)
        return dict(code=proc.returncode,output=proc.stdout.splitlines(),error=proc.stderr)

    try:
        local=f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
        for label in ('reference','aster','aster_b','aster_observer'):
            conn=connect((os.environ['PG18_REFERENCE_DSN'] if label=='reference' else local).encode())
            connections[label]=conn
            check(bool(conn) and status(conn)==0,label+' connection failed')
            if label=='reference':
                check(query(conn,"SET TIME ZONE 'UTC'")['status']==1,'reference UTC setup')
            check(parameter_status(conn,b'client_encoding')==b'UTF8' and
                  parameter_status(conn,b'TimeZone')==b'UTC',label+' connection encoding/timezone')
            check(query(conn,'SET search_path TO public')['status']==1,label+' search path')
        ref=connections['reference']
        ref_version=query(ref,"SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(ref_version[1]=='180006','exact PostgreSQL18.6 reference is required')
        build=json.loads((evidence/'rust-build.json').read_text())
        build['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        mysql_build=json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text())
        provenance=dict(run=evidence.name,target_version='18.6',source=source_snapshot(root),build=build,
            reference=dict(version=ref_version[0],server_version_num=ref_version[1]),
            reference_build=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text()),
            mysql_reference_build=mysql_build,
            client=dict(python=sys.version,library=str(library),sha256=hashlib.sha256(library.read_bytes()).hexdigest(),PQlibVersion=version()),
            os=platform.platform(),storage='CreateAnalyzeSession test storage',authentication='InsecureRootOnly',
            endpoints={s:endpoint(connections[s]) for s in ('aster','aster_b','reference')})
        (evidence/'provenance.json').write_text(json.dumps(provenance,indent=2))
        check(validate_provenance(provenance,copy.deepcopy(provenance)),'provenance validation')
        damaged=copy.deepcopy(provenance);damaged['reference_build']['server_version_num']=180004
        check(not validate_provenance(damaged,provenance),'provenance negative self-check');self_checks+=1
        diagnostic={'state':'42804','status':7,'position':'28'}
        damaged_diagnostic=dict(diagnostic,state='42601')
        check(not same_diagnostic(damaged_diagnostic,diagnostic),
              'diagnostic comparator must reject a wrong SQLSTATE')
        self_checks+=1
        before=query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']
        check(before==[['31']],'observer initial state')

        for index,(name,sql,oids) in enumerate(cases):
            arguments=['5','7'] if name=='parameters' else []
            reference=prepare_and_execute(ref,'reference',index,name,sql,oids,arguments)
            local_a=prepare_and_execute(connections['aster'],'aster',index,name,sql,oids,arguments)
            print('PG_DIALECT_CHECKPOINT '+name,flush=True)
            check(sys.stdin.readline().strip()=='continue','MySQL checkpoint aborted')
            local_b=prepare_and_execute(connections['aster_b'],'aster_b',index,name,sql,oids,arguments)

            for candidate,label in ((local_a,'A'),(local_b,'B')):
                check(same_diagnostic(candidate[0],reference[0]),
                      label+' Prepare/reference contract '+name)
                check(same_diagnostic(candidate[3],reference[3]),
                      label+' Q/reference contract '+name)
                if candidate[1] is not None and reference[1] is not None:
                    check(candidate[1]['fields']==reference[1]['fields'] and
                          candidate[1]['params']==reference[1]['params'],
                          label+' Describe/reference contract '+name)
                for fmt,actual in candidate[2].items():
                    expected=reference[2][fmt]
                    check(actual['state']==expected['state'] and actual['status']==expected['status'],
                          label+' Execute/reference status '+name+' format '+fmt)
                    if actual['status']==2:
                        check(actual['fields']==expected['fields'] and actual['formats']==expected['formats'] and
                              actual['rows']==expected['rows'] and actual['command_tag']==expected['command_tag'],
                              label+' Execute/reference values '+name+' format '+fmt)

            if name in ('explicit','implicit','numeric','parameters','nulls'):
                semantic_success+=1
            else:
                semantic_errors+=1
            for label in ('aster','aster_b','aster_observer'):
                check(query(connections[label],'SELECT id FROM public.dialect_isolation')['rows']==before,
                      label+' observer state changed')
            eligible+=1

        identity=mysql_reference('SELECT VERSION(), @@version_compile_os, @@version_compile_machine; SELECT @@sql_mode, @@character_set_connection, @@collation_connection, @@time_zone')
        record('mysql_identity','mysql',dict(phase='environment'),identity)
        check(identity['code']==0 and identity['output'] and identity['output'][0].startswith('8.4.7\t'),
              'exact MySQL 8.4.7 build identity')
        check(len(identity['output']) >= 2 and identity['output'][1].split('\t') ==
              ['', 'utf8mb4', 'utf8mb4_general_ci', '+00:00'],
              'MySQL reference SQL mode/charset/collation/timezone')
        json_control=mysql_reference('SELECT JSON_ARRAY(1,2)')
        record('mysql_json_control','mysql',dict(phase='control'),json_control)
        check(json_control['code']==0 and json_control['output']==['[1, 2]'],'MySQL legal JSON_ARRAY baseline')
        prepared_control=mysql_reference("PREPARE array_control FROM 'SELECT JSON_ARRAY(?,?)'; SET @a=1; SET @b=2; EXECUTE array_control USING @a,@b; DEALLOCATE PREPARE array_control")
        record('mysql_json_prepared_control','mysql',dict(phase='prepared-control'),prepared_control)
        check(prepared_control['code']==0 and prepared_control['output']==['[1, 2]'],
              'MySQL legal prepared JSON_ARRAY control')
        for name,sql,_ in cases:
            result=mysql_reference(sql)
            record(name,'mysql',dict(phase='Q',sql=sql),result)
            check(result['code']!=0 and 'ERROR 1064 (42000)' in result['error'],
                  'MySQL 8.4.7 must reject PostgreSQL ARRAY syntax: '+name)

        final=copy.deepcopy(provenance)
        final['source']=source_snapshot(root)
        final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        final['client']['sha256']=hashlib.sha256(library.read_bytes()).hexdigest()
        final['reference_build']=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        final['mysql_reference_build']=json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text())
        final['endpoints']={s:endpoint(connections[s]) for s in ('aster','aster_b','reference')}
        check(validate_provenance(provenance,final),'provenance changed during run')
        done=True
    finally:
        for conn,statement in prepared:
            result=observe(close_prepared(conn,statement))
            cleanup_ok=result['status']==1 and cleanup_ok
        for conn in connections.values():
            if conn and transaction(conn) in (2,3):
                cleanup_ok=query(conn,'ROLLBACK')['status']==1 and cleanup_ok
            if conn:
                finish(conn)
        summary=dict(cases=len(cases),assertions=checks,self_checks=self_checks,
            syntax_eligible=eligible if done and cleanup_ok else 0,
            semantic_array_success=semantic_success if done and cleanup_ok else 0,
            semantic_error_cases=semantic_errors if done and cleanup_ok else 0,
            gate_cases=0,unique_effective=eligible if done and cleanup_ok else 0,
            cleanup_ok=cleanup_ok,scope='rectangular nested arrays, common element types, ragged execution error, parameter binding and dialect isolation')
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and cleanup_ok,'incomplete run/cleanup'
    print(json.dumps(summary),flush=True)

def row_handoff():
    """Anonymous records through normal Prepare/Describe/Bind/Execute and Q."""
    connections, prepared = {}, []
    checks = self_checks = 0
    completed_cases = set()
    semantic_cases = set()
    done = False
    cleanup_ok = True
    expected = {'two':'(1,2)','one':'(1)','nested':'(1,"(2,3)")',
                'parameters':'(,9,7,9)','protected':'ROW(1,2)',
                'quoted':'("","a,b","a""b","中 文",NULL)',
                'null_field':'()','scalar_types':'(1.5,t,2147483648)'}
    def check(condition,message):
        nonlocal checks
        assert condition,message
        checks += 1
    def record(name,label,data,result):
        allowed={'phase','sql','statement','parameter_oids','arguments','parameter_formats','result_format','command'}
        archive(name,'aster' if label.startswith('aster') else label,
                {k:v for k,v in (data or {}).items() if k in allowed},
                dict(result,connection=label,context=data))
    def recover(label,name):
        conn=connections[label]
        if transaction(conn) in (2,3):check(query(conn,'ROLLBACK')['status']==1,'rollback '+label)
        result=query(conn,'SELECT 1::int4');record(name+'_recovery',label,dict(phase='Q',sql='SELECT 1::int4'),result)
        check(result['rows']==[['1']] and result['state']=='' and transaction(conn)==0,'same connection recovery '+label)
    def compare_diagnostics(a,r):
        return all(a[k]==r[k] for k in ('state','status','position','transaction_after'))
    def compare_values(a,r):
        return all(a[k]==r[k] for k in ('status','state','fields','formats','rows','command_tag','table_origins'))
    try:
        local=f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
        for label in ['reference','aster','aster_b','aster_observer']:
            conn=connect((os.environ['PG18_REFERENCE_DSN'] if label=='reference' else local).encode());connections[label]=conn
            check(bool(conn) and status(conn)==0,'connect '+label)
            if label=='reference':check(query(conn,"SET TIME ZONE 'UTC'")['status']==1,'reference UTC')
            check(parameter_status(conn,b'client_encoding')==b'UTF8' and parameter_status(conn,b'TimeZone')==b'UTC','settings '+label)
            check(query(conn,'SET search_path TO public')['status']==1,'path '+label)
        identity=query(connections['reference'],"SELECT version(), current_setting('server_version_num')")['rows'][0]
        check(identity[1]=='180006','exact reference 18.6')
        build=json.loads((evidence/'rust-build.json').read_text());build['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        provenance=dict(run=evidence.name,target_version='18.6',source=source_snapshot(root),build=build,
            reference=dict(version=identity[0],server_version_num=identity[1]),
            reference_build=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text()),
            client=dict(python=sys.version,library=str(library),sha256=hashlib.sha256(library.read_bytes()).hexdigest(),PQlibVersion=version()),
            storage='CreateAnalyzeSession test storage',authentication='InsecureRootOnly',os=platform.platform(),
            mysql_reference_build=json.loads(Path(os.environ['MYSQL_REFERENCE_BUILD_FILE']).read_text()),
            endpoints={s:endpoint(connections[s]) for s in ['reference','aster','aster_b']})
        (evidence/'provenance.json').write_text(json.dumps(provenance,indent=2))
        check(validate_provenance(provenance,copy.deepcopy(provenance)),'provenance')
        damaged=copy.deepcopy(provenance);damaged['reference_build']['server_version_num']=180004
        check(not validate_provenance(damaged,provenance),'provenance negative');self_checks+=1
        before=query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows'];check(before==[['31']],'initial state')
        for index,(name,sql,oids) in enumerate(cases):
            base=dict(case_id=name,capability_id='pg.dialect.row.syntax',phase='PQprepare',sql=sql,parameter_oids=oids,
                arguments=['7','9'] if oids else [],parameter_formats=[0]*len(oids),result_format=0,
                role={k:connection_user(v).decode() for k,v in connections.items()},encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',object_mapping={},
                expected_basis='PostgreSQL18.6 frozen reference and anonymous record contract')
            results={};descriptions={};executions={}
            for label in ['reference','aster_b' if index%2 else 'aster']:
                side='reference' if label=='reference' else 'aster';conn=connections[label]
                check(transaction(conn)==0,'initial transaction '+label)
                stmt=f'row_{index}_{label}'.encode()
                result=observe(prepare(conn,stmt,sql.encode(),len(oids),(c.c_uint*len(oids))(*oids) if oids else None))
                result['transaction_after']={0:'I',2:'T',3:'E'}.get(transaction(conn));record(name,label,base,result);results[side]=result
                state={'missing_close':'42601','unknown_parameter':'42P18'}.get(name,'')
                check(result['state']==state and result['status']==(7 if state else 1),label+' Prepare '+name)
                if state:
                    check(result['position']==(str(len(sql)+1) if name=='missing_close' else ''),label+' original diagnostic position')
                else:
                    prepared.append((conn,stmt));desc=observe(describe(conn,stmt));descriptions[side]=desc
                    record(name+'_describe',label,dict(base,phase='Describe'),desc)
                    check(desc['fields']==[['?column?' if name=='protected' else 'row',25 if name=='protected' else 2249,-1,-1]],label+' result identity '+name)
                    check(desc['params']==oids and desc['param_count']==len(oids) and desc['formats']==[0] and desc['table_origins']==[[0,0]],label+' Describe fields/parameters/origins '+name)
                    argsets=[['7','9'],[None,'9'],[None,None]] if oids else [[]]
                    for attempt,arguments in enumerate(argsets):
                        args=(text*len(arguments))(*[None if arg is None else arg.encode() for arg in arguments]) if arguments else None
                        for fmt in [0,1]:
                            values=observe(exec_prepared(conn,stmt,len(arguments),args,None,None,fmt),binary=fmt==1)
                            record(name+f'_execute_{attempt}_{fmt}',label,dict(base,phase='Bind/Execute',arguments=arguments,result_format=fmt),values)
                            check(values['status']==2 and values['state']=='' and values['fields']==desc['fields'] and values['formats']==[fmt] and values['command_tag']=='SELECT 1',label+' execute identity '+name)
                            if fmt==0:
                                output=['(,9,7,9)','(,9,,9)','(,,,)'][attempt] if oids else expected[name]
                                check(values['rows']==[[output]],label+' actual record/NULL text '+name)
                            check(len(values['rows'])==1 and len(values['rows'][0])==1 and values['rows'][0][0] is not None,label+' record must be one nonnull composite column')
                            executions[(side,attempt,fmt)]=values
                recover(label,name)
            check(compare_diagnostics(results['aster'],results['reference']),'Prepare differential '+name)
            if name=='missing_close':
                damaged=copy.deepcopy(results['aster']);damaged['state']='0A000'
                check(not compare_diagnostics(damaged,results['reference']),'diagnostic code negative');self_checks+=1
                damaged=copy.deepcopy(results['aster']);damaged['position']='1'
                check(not compare_diagnostics(damaged,results['reference']),'diagnostic position negative');self_checks+=1
            if name in expected:
                check(compare_metadata(descriptions['aster'],descriptions['reference'],'describe'),'full Describe differential '+name)
                for attempt in range(3 if oids else 1):
                    for fmt in [0,1]:check(compare_values(executions[('aster',attempt,fmt)],executions[('reference',attempt,fmt)]),'text/binary exact differential '+name)
                if name!='protected':semantic_cases.add(name)
                if name=='nested':
                    damaged=copy.deepcopy(executions[('aster',0,1)]);damaged['rows'][0][0]='00000000'+damaged['rows'][0][0][8:]
                    check(not compare_values(damaged,executions[('reference',0,1)]),'binary structure comparator negative');self_checks+=1
                if name=='two':
                    damaged=copy.deepcopy(descriptions['aster']);damaged['fields'][0][1]=25
                    check(not compare_metadata(damaged,descriptions['reference'],'describe'),'record-vs-string identity negative');self_checks+=1
            simple_results={}
            for label in ['reference','aster']:
                result=query(connections[label],sql);simple_results[label]=result;record(name+'_Q',label,dict(base,phase='Q',arguments=[],parameter_oids=[]),result)
                state='42601' if name=='missing_close' else ('42P02' if name in ('parameters','unknown_parameter') else '')
                check(result['state']==state and result['status']==(7 if state else 2),label+' Q outcome '+name)
                if not state:check(result['rows']==[[expected[name]]] and result['fields']==descriptions['reference']['fields'],label+' Q values/metadata '+name)
                if name=='missing_close':check(result['position']==str(len(sql)+1),label+' Q position')
                recover(label,name+'_Q')
            check(all(simple_results['aster'][key]==simple_results['reference'][key] for key in ('state','status','position')),'Q differential diagnostic '+name)
            if simple_results['reference']['status']==2:check(compare_values(simple_results['aster'],simple_results['reference']),'Q full value/metadata differential '+name)
            for label in (['aster','aster_b'] if index%2 else ['aster_b','aster']):recover(label,name+'_isolation')
            check(query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']==before,'observer unchanged')
            completed_cases.add(name)
            print('PG_DIALECT_CHECKPOINT '+name,flush=True);check(sys.stdin.readline().strip()=='continue','MySQL checkpoint')
            if os.environ.get('PG_DIALECT_ROW_SIGINT_CHECK')=='1':os.kill(os.getpid(),signal.SIGINT)
        for label in ['reference','aster']:
            conn=connections[label];check(query(conn,'BEGIN')['status']==1,'BEGIN')
            result=observe(prepare(conn,b'row_bad',b'SELECT ROW(1,',0,None));record('transaction_error',label,dict(phase='PQprepare',sql='SELECT ROW(1,'),dict(result,transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conn))))
            check(result['state']=='42601' and transaction(conn)==3,'transaction E');recover(label,'transaction')
        sql='INSERT INTO public.dialect_isolation VALUES (ROW(47))'
        result=query(connections['aster'],sql);record('outside_record_DML','aster',dict(phase='Q',sql=sql),result)
        check(result['state']=='0A000','record DML not in direct constructor scope')
        check(query(connections['aster_observer'],'SELECT id FROM public.dialect_isolation')['rows']==before,'record DML must not write');recover('aster','outside_DML')
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN']);env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql(sql):
            proc=subprocess.run(args+['-e',"SET sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            return dict(code=proc.returncode,output=proc.stdout.splitlines(),error=proc.stderr)
        result=mysql('SELECT VERSION(),@@version_compile_os,@@version_compile_machine');record('identity','mysql',None,result);check(result['code']==0,'MySQL reference identity')
        for name,sql in [('two','SELECT ROW(1,2)'),('one','SELECT ROW(1)'),('nested','SELECT ROW(1,ROW(2,3))'),('missing_close','SELECT ROW(1,')]:
            result=mysql(sql);record(name,'mysql',dict(phase='Q',sql=sql),result)
            check(result['code']!=0 and ('ERROR '+('1241 (21000)' if name in ('two','nested') else '1064 (42000)')) in result['error'],'MySQL own row diagnostic')
        result=mysql("SELECT (1); SELECT ROW(1,2)=ROW(1,2); PREPARE q FROM 'SELECT ROW(?,?)=ROW(?,?)'; SET @a=1; SET @b=2; EXECUTE q USING @a,@b,@a,@b; DEALLOCATE PREPARE q; SELECT 'ROW(1,2)' /* ROW() */")
        record('legal_prepare','mysql',None,result);check(result['code']==0 and result['output']==['1','1','1','ROW(1,2)'],'MySQL legal comparison/prepared/literal')
        final=copy.deepcopy(provenance);final['source']=source_snapshot(root);final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest();final['endpoints']={s:endpoint(connections[s]) for s in ['reference','aster','aster_b']}
        final['client']['sha256']=hashlib.sha256(library.read_bytes()).hexdigest()
        final['reference_build']=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        final_identity=query(connections['reference'],"SELECT version(),current_setting('server_version_num')")['rows'][0]
        final['reference']=dict(version=final_identity[0],server_version_num=final_identity[1])
        check(validate_provenance(provenance,final),'final input provenance')
        done=True
    finally:
        for conn,stmt in prepared:cleanup_ok=observe(close_prepared(conn,stmt))['status']==1 and cleanup_ok
        for conn in connections.values():
            if conn and transaction(conn) in (2,3):cleanup_ok=query(conn,'ROLLBACK')['status']==1 and cleanup_ok
            if conn:finish(conn)
        summary=dict(cases=len(cases),assertions=checks,self_checks=self_checks,
            completed_case_ids=sorted(completed_cases) if done and cleanup_ok else [],
            unique_effective=len(completed_cases) if done and cleanup_ok else 0,
            semantic_row_case_ids=sorted(semantic_cases) if done and cleanup_ok else [],
            semantic_row_success=len(semantic_cases) if done and cleanup_ok else 0,
            syntax_eligible=len(completed_cases) if done and cleanup_ok else 0,gate_cases=0,cleanup_ok=cleanup_ok,
            outside_DML_counts_as_main_case=False)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and cleanup_ok,'incomplete row verification/cleanup'
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

def cte_handoff():
    conns,prepared,created={ },[],[]
    checks=self_checks=eligible=0
    done=False;clean=True;collecting=False;failures=[]
    def check(ok,message):
        nonlocal checks
        if not ok:
            if collecting: failures.append(message)
            else: raise AssertionError(message)
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
            sql='CREATE TABLE public.'+cte_table+'(v integer)';result=query(conns[label],sql)
            record('setup',label,dict(phase='Q',sql=sql),result);check(result['status']==1,'create fixture');created.append(label)
            check(query(conns[label],'INSERT INTO public.'+cte_table+' VALUES (9)')['status']==1,'insert fixture')
        oids={}
        for label in ['aster','reference']:
            sql="SELECT oid FROM pg_catalog.pg_class WHERE relname='"+cte_table+"'"
            result=query(conns[label],sql);record('relation_oid',label,dict(phase='Q',sql=sql),result)
            check(result['status']==2 and len(result['rows'])==1,'physical OID');oids[label]=int(result['rows'][0][0])
        sql='SELECT v FROM public.'+cte_table;initial=query(conns['aster_observer'],sql)
        record('observer_initial','aster_observer',dict(phase='Q',sql=sql),initial);check(initial['rows']==[['9']],'initial observer')
        registry=[]
        collecting=True
        for index,(name,template,param_oids) in enumerate(cases):
            sql=template.replace('@t@',cte_table)
            expected_state='42P01' if name=='forward_missing' else ''
            expected_rows=[['9']] if name in ('explicit','forward_table','self_table') else [['1']]
            expected_position=str(sql.index('absent18')+1) if expected_state else ''
            data=dict(capability_id='pg.dialect.cte.order',case_id=name,phase='PQprepare',sql=sql,parameter_oids=[],arguments=[],parameter_formats=[],result_format=0,
                encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',object_mapping={'aster':'current_database.public.'+cte_table,'reference':'public.'+cte_table},
                expected_basis='task18 exact PostgreSQL18.6 CTE binding',expected_state=expected_state,expected_position=expected_position,
                roles={s:connection_user(conns[s]).decode() for s in ['aster','reference']})
            registry.append(data);(evidence/'cases.json').write_text(json.dumps(registry,indent=2))
            results={};descs={}
            order=['aster','reference'] if index%2==0 else ['reference','aster_b']
            for label in order:
                side='reference' if label=='reference' else 'aster';conn=conns[label]
                check(transaction(conn)==0,'initial idle')
                statement=('cte_'+str(index)).encode()
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
                    expected_origins=[[oids[side],1]] if name in ('explicit','forward_table','self_table') else [[0,0]]
                    check(descs[side]['table_origins']==expected_origins and results[side]['table_origins']==expected_origins,'actual mapped physical origins '+name+' '+side)
                check(results['aster']['fields']==results['reference']['fields'] and results['aster']['rows']==results['reference']['rows'],'execution differential')
                bad=copy.deepcopy(descs['aster']);bad['fields'][0][1]=99999
                check(not compare_metadata(bad,descs['reference'],'describe'),'metadata negative');self_checks+=1
            print('PG_DIALECT_CHECKPOINT '+name,flush=True);check(sys.stdin.readline().strip()=='continue','MySQL checkpoint')
            for label in (['aster_b','aster'] if index%2==0 else ['aster','aster_b']):recover(label,name+'_isolation')
            sql='SELECT v FROM public.'+cte_table;result=query(conns['aster_observer'],sql)
            record(name+'_observer','aster_observer',dict(phase='Q',sql=sql),result);check(result['rows']==initial['rows'],'observer unchanged')
            eligible+=1
        parameter_results={}
        for label in ['aster','reference']:
            conn=conns[label];sql='/*中*/ WITH t AS (SELECT $2::int4 AS v) SELECT v,$1::int4 AS p FROM t /* public.t $1 */'
            data=dict(phase='PQprepare',sql=sql,parameter_oids=[23,23],arguments=['1','9'],parameter_formats=[0,0],result_format=0)
            result=observe(prepare(conn,b'cte_parameters',sql.encode(),2,(c.c_uint*2)(23,23)))
            record('parameters',label,data,result);check(result['status']==1,'qualified parameter prepare');prepared.append((conn,b'cte_parameters'))
            desc=observe(describe(conn,b'cte_parameters'));record('parameters_describe',label,dict(data,phase='Describe'),desc)
            check(desc['params']==[23,23] and [field[1] for field in desc['fields']]==[23,23],'explicit parameter identity')
            for argument,want in [(b'9','9'),(None,None)]:
                result=observe(exec_prepared(conn,b'cte_parameters',2,(text*2)(b'1',argument),None,None,0))
                record('parameters_null' if argument is None else 'parameters_execute',label,dict(data,phase='Bind/Execute',arguments=['1',want]),result)
                check(result['status']==2 and result['rows']==[[want,'1']],'qualified value/NULL parameters')
            parameter_results[label]=desc
        check(compare_metadata(parameter_results['aster'],parameter_results['reference'],'describe'),'parameter Describe differential')
        for label in ['aster','reference']:
            check(query(conns[label],'BEGIN')['status']==1,'begin transaction')
            sql='WITH a AS (SELECT v FROM absent18), absent18 AS (SELECT 2 AS v) SELECT v FROM a'
            result=observe(prepare(conns[label],b'cte_tx_error',sql.encode(),0,None))
            record('transaction_error',label,dict(phase='PQprepare',sql=sql),result)
            check(result['state']=='42P01' and transaction(conns[label])==3,'transaction becomes failed');recover(label,'transaction')
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN']);env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql):
            p=subprocess.run(args+['-e',"SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            return dict(sql=sql,code=p.returncode,output=p.stdout.splitlines(),error=p.stderr)
        mysql_db='ref18_'+evidence.name.replace('-','_')
        try:
            result=mysql_ref('SELECT VERSION(),@@version_compile_os,@@version_compile_machine; SELECT @@sql_mode,@@character_set_connection,@@collation_connection; CREATE DATABASE '+mysql_db+'; CREATE TABLE '+mysql_db+'.t(v INT); INSERT INTO '+mysql_db+'.t VALUES(9)')
            record('mysql_identity','mysql',{},result);check(result['code']==0,'MySQL fixture')
            for name,template,_ in cases:
                sql=template.replace('@t@','t').replace('public.t',mysql_db+'.t')
                result=mysql_ref('USE '+mysql_db+'; '+sql);record(name,'mysql',dict(phase='Q',sql=sql),result)
                if name=='forward_missing':check(result['code']!=0 and '1146 (42S02)' in result['error'],'MySQL forward diagnostic')
                else:check(result['code']==0 and result['output']==(['9'] if name in ('explicit','forward_table','self_table') else ['1']),'MySQL CTE '+name+': '+str(result))
            sql="USE "+mysql_db+"; PREPARE s FROM 'WITH t AS (SELECT ? AS v) SELECT v FROM t'; SET @v=9; EXECUTE s USING @v; DEALLOCATE PREPARE s"
            result=mysql_ref(sql);record('cte_prepare','mysql',dict(phase='PREPARE/EXECUTE',sql=sql),result);check(result['code']==0 and result['output']==['9'],'MySQL CTE prepare')

        finally:
            result=mysql_ref('DROP DATABASE IF EXISTS '+mysql_db);record('cleanup','mysql',{},result);clean=result['code']==0 and clean
        final=copy.deepcopy(provenance);final['source']=source_snapshot(root);final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        check(validate_provenance(provenance,final),'source unchanged');done=not failures
    finally:
        for conn,statement in prepared:clean=observe(close_prepared(conn,statement))['status']==1 and clean
        for conn in conns.values():
            if conn and transaction(conn) in (2,3):clean=query(conn,'ROLLBACK')['status']==1 and clean
        for label in created:
            sql='DROP TABLE IF EXISTS public.'+cte_table;result=query(conns[label],sql);record('cleanup',label,dict(phase='Q',sql=sql),result);clean=result['status']==1 and clean
        for conn in conns.values():
            if conn:finish(conn)
        summary=dict(cases=6,failures=failures,assertions=checks,self_checks=self_checks,unique_effective=eligible if done and clean else 0,cleanup_ok=clean)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert done and clean, 'completion/cleanup: '+str(failures)
    print(json.dumps(summary),flush=True)

def lifecycle_handoff():
    """Connection lifecycle through libpq18; named portals need a wire supplement."""
    import socket
    import struct
    conns, prepared, wire_sockets = {}, set(), []
    checks = self_checks = eligible = 0
    clean = True
    done = False
    registry = []
    current_case = "setup"
    def check(ok, message):
        nonlocal checks
        checks += 1
        assert ok, message
    def record(name, label, data, result):
        archive(name, 'aster' if label.startswith('aster') else ('reference' if label.startswith('reference') else label), data,
                dict(result, connection=label, lifecycle_case=current_case,
                     transaction_after={0:'I',2:'T',3:'E'}.get(transaction(conns[label])) if label in conns else None))
    local = f'host=127.0.0.1 port={int(sys.argv[2])} dbname={sys.argv[3]} user=root connect_timeout=5'
    def open_conn(label):
        conn = connect((os.environ['PG18_REFERENCE_DSN'] if label.startswith('reference') else local).encode())
        conns[label] = conn
        check(bool(conn) and status(conn) == 0, 'connect '+label)
        if label.startswith('reference'):
            check(query(conn, "SET TIME ZONE 'UTC'")['status'] == 1, 'UTC')
        check(query(conn, 'SET search_path TO public')['status'] == 1, 'public')
        return conn
    def q(label, sql, name):
        result = query(conns[label], sql)
        record(name, label, dict(phase='Q',sql=sql), result)
        return result
    def prep(label, oid, name=b'same'):
        sql = f'SELECT $1::int{4 if oid==23 else 8} AS v'
        data = dict(phase='Parse',sql=sql,statement=name.decode(),parameter_oids=[oid],arguments=[],parameter_formats=[0],result_format=0)
        result = observe(prepare(conns[label],name,sql.encode(),1,(c.c_uint*1)(oid)))
        record('prepare',label,data,result)
        check(result['status']==1, 'prepare '+label+': '+str(result))
        prepared.add((label,name))
        desc = observe(describe(conns[label],name));record('describe',label,dict(data,phase='Describe'),desc)
        check(desc['params']==[oid] and desc['param_count']==1 and desc['fields']==[['v',oid,4 if oid==23 else 8,-1]] and desc['formats']==[0], 'typed Describe '+label)
        check(desc['table_origins']==[[0,0]], 'expression has no physical origin '+label)
        return desc
    def run(label, arg, oid, name=b'same'):
        result = observe(exec_prepared(conns[label],name,1,(text*1)(arg.encode() if arg is not None else None),None,None,0))
        record('execute',label,dict(phase='Bind/Execute',statement=name.decode(),arguments=[arg],parameter_formats=[0],result_format=0),result)
        check(result['status']==2 and result['rows']==[[arg]] and result['fields']==[['v',oid,4 if oid==23 else 8,-1]] and result['command_tag']=='SELECT 1' and result['formats']==[0], 'typed value/NULL '+label)
        check(result['table_origins']==[[0,0]], 'executed expression has no physical origin '+label)
        return result
    def close(label, name=b'same'):
        result=observe(close_prepared(conns[label],name));record('close_statement',label,dict(phase='Close',statement=name.decode()),result)
        check(result['status']==1,'Close statement');prepared.discard((label,name))
    def recover(label):
        if transaction(conns[label]) in (2,3):
            check(q(label,'ROLLBACK','rollback')['status']==1,'rollback')
        result=q(label,'SELECT 1::int4 AS pg_core_pg_identity_24','recovery')
        check(result['rows']==[['1']] and result['fields']==[['pg_core_pg_identity_24',23,4,-1]] and transaction(conns[label])==0,'same connection typed recovery '+label)
    def checkpoint(name):
        forward = len(registry) % 2 == 1
        recover('aster' if forward else 'aster_b')
        print('PG_DIALECT_CHECKPOINT '+name,flush=True)
        check(sys.stdin.readline().strip()=='continue','MySQL checkpoint')
        for label in (['aster_b','aster','reference_b','reference'] if forward else ['aster','aster_b','reference','reference_b']):
            recover(label)
    scope='lifecycle24_'+evidence.name.replace('-','_')
    def observers(name):
        for label,table in [('aster_observer','public.dialect_isolation'),('reference_observer','public.'+scope)]:
            result=q(label,'SELECT id FROM '+table,name+'_observer')
            check(result['rows']==[['31']], 'independent observer unchanged')
    def register(name, sql, phase, oids=None, args=None):
        nonlocal current_case
        current_case = name
        data=dict(case_id=name,capability_id='pg.dialect.connection.lifecycle',sql=sql,phase=phase,
                  parameter_oids=oids or [],arguments=args or [],parameter_formats=[0]*len(oids or []),result_format=0,
                  encoding='UTF8',timezone='UTC',search_path='public',initial_state='I',
                  object_mapping={'aster':'current_database.public.dialect_isolation','reference':'public.'+scope},
                  roles={s:connection_user(conns[s]).decode() for s in ['aster','aster_b','reference','reference_b']},
                  expected_basis='task24 immutable connection identity; PostgreSQL18.6 actual lifecycle',
                  expected_state='42601' if name in ('parse_error','transaction_error') else '',
                  expected_position=str(sql.index(')')+1) if name in ('parse_error','transaction_error') else None)
        registry.append(data);(evidence/'cases.json').write_text(json.dumps(registry,indent=2))
    def wire_portals(label):
        # libpq does not expose Bind with a named portal. Use a separate real
        # protocol connection so its internal read buffer is never bypassed.
        host,port,user,db = ('127.0.0.1',int(sys.argv[2]),'root',sys.argv[3])
        if label=='reference':
            options=api('PQhost',text,ptr)(conns[label]).decode(),api('PQport',text,ptr)(conns[label]).decode(),connection_user(conns[label]).decode(),api('PQdb',text,ptr)(conns[label]).decode()
            host,port,user,db=options;port=int(port)
        sock=socket.create_connection((host,port),timeout=10);wire_sockets.append(sock)
        def send(tag,body):sock.sendall(tag+struct.pack('!I',len(body)+4)+body)
        def exact(n):
            result=b''
            while len(result)<n:
                part=sock.recv(n-len(result));assert part, 'wire EOF';result+=part
            return result
        def read():
            header=exact(5);return header[:1],exact(struct.unpack('!I',header[1:])[0]-4)
        startup=struct.pack('!I',196608)+b'user\0'+user.encode()+b'\0database\0'+db.encode()+b'\0\0'
        sock.sendall(struct.pack('!I',len(startup)+4)+startup)
        wire_parameters={}
        def parameter(tag,body):
            if tag==b'S':
                key,val,_=body.split(b'\0',2);wire_parameters[key.decode()]=val.decode()
        while True:
            tag,body=read();parameter(tag,body)
            if tag==b'R':check(body==b'\0\0\0\0','trust fixture authentication')
            if tag==b'Z':check(body==b'I','wire initial idle');break
        # Freeze and observe the same environment on the supplementary socket.
        def wire_query(sql):
            send(b'Q',sql.encode()+b'\0');messages=[]
            while True:
                tag,body=read();parameter(tag,body);messages.append((tag,body))
                if tag==b'Z':break
            check(not any(tag==b'E' for tag,body in messages),'wire environment command')
            return messages
        if label=='reference':wire_query("SET TIME ZONE 'UTC'")
        wire_query('SET search_path TO public')
        for sql,want in [('SHOW search_path','public')]:
            messages=wire_query(sql)
            check([body for tag,body in messages if tag==b'D']==[struct.pack('!HI',1,len(want))+want.encode()], 'wire environment '+sql)
            record('wire_environment',label,dict(phase='Q',sql=sql),dict(wire=[(tag.decode(),body.hex()) for tag,body in messages]))
        check(wire_parameters.get('TimeZone')=='UTC' and wire_parameters.get('client_encoding')=='UTF8','wire ParameterStatus environment')
        record('wire_parameters',label,{},dict(parameters=wire_parameters))
        results=[]
        for oid,arg in [(23,'7'),(20,'2147483648')]:
            sql=f'SELECT $1::int{4 if oid==23 else 8} AS v'
            send(b'P',b'same\0'+sql.encode()+b'\0'+struct.pack('!HI',1,oid))
            send(b'B',b'portal\0same\0'+struct.pack('!HHI',0,1,len(arg))+arg.encode()+struct.pack('!H',0))
            send(b'D',b'Pportal\0');send(b'E',b'portal\0'+struct.pack('!I',0))
            send(b'C',b'Pportal\0');send(b'C',b'Ssame\0');send(b'S',b'')
            messages=[]
            while True:
                tag,body=read();messages.append((tag,body))
                if tag==b'Z':break
            record('named_portal',label,dict(phase='Parse/Bind/Describe/Execute/Close/Sync',sql=sql,parameter_oids=[oid],arguments=[arg]),dict(wire=[(t.decode(),b.hex()) for t,b in messages]))
            check([t for t,b in messages]==[b'1',b'2',b'T',b'D',b'C',b'3',b'3',b'Z'],'portal lifecycle messages '+label)
            desc=messages[2][1];end=desc.index(b'\0',2)
            check(desc[2:end]==b'v' and struct.unpack('!Ihih',desc[end+7:])==(oid,4 if oid==23 else 8,-1,0),'portal exact metadata')
            check(messages[3][1]==struct.pack('!HI',1,len(arg))+arg.encode() and messages[4][1]==b'SELECT 1\0' and messages[-1][1]==b'I','portal value/command/idle')
            results.append(dict(oid=oid,arg=arg,description=desc.hex()))
        # Pipeline a command after a Parse error: only ErrorResponse and Sync's
        # ReadyForQuery may survive; the ignored Bind must not create a portal.
        sql='/*中*/ SELECT )'
        send(b'P',b'bad\0'+sql.encode()+b'\0'+struct.pack('!H',0))
        send(b'B',b'ignored\0bad\0'+struct.pack('!HHH',0,0,0));send(b'S',b'')
        messages=[]
        while True:
            tag,body=read();messages.append((tag,body))
            if tag==b'Z':break
        record('parse_error_wire',label,dict(phase='Parse/ignored Bind/Sync',sql=sql),dict(wire=[(tag.decode(),body.hex()) for tag,body in messages]))
        check([tag for tag,body in messages]==[b'E',b'Z'] and messages[-1][1]==b'I','explicit Sync recovery')
        fields={item[:1]:item[1:] for item in messages[0][1].split(b'\0') if item}
        check(fields.get(b'C')==b'42601' and fields.get(b'P')==b'14','wire original error fields')
        send(b'X',b'');sock.close();wire_sockets.remove(sock)
        return results
    reference_created=False
    try:
        for label in ['reference','reference_b','aster','aster_b','aster_observer','reference_observer']:open_conn(label)
        identity=query(conns['reference'],"SELECT version(),current_setting('server_version_num')")['rows'][0]
        check(identity[1]=='180006','exact PostgreSQL18.6')
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
        check(q('reference','CREATE TABLE public.'+scope+'(id integer)','setup')['status']==1,'reference observer table');reference_created=True
        check(q('reference','INSERT INTO public.'+scope+' VALUES(31)','setup')['status']==1,'reference observer row')
        observers('initial')
        check(q('aster',"SET search_path TO pg_catalog, public",'path')['status']==1,'A path')
        check(q('reference',"SET search_path TO pg_catalog, public",'path')['status']==1,'reference A path')
        register('coexist','SELECT $1::int4 AS v','Parse/Bind/Describe/Execute',[23],['7'])
        descs={label:prep(label,23 if label in ('aster','reference') else 20) for label in ['aster','reference','aster_b','reference_b']}
        check(compare_metadata(descs['aster'],descs['reference'],'describe') and compare_metadata(descs['aster_b'],descs['reference_b'],'describe'),'two endpoint parameter identity')
        bad=copy.deepcopy(descs['aster']);bad['fields'][0][1]=20
        check(not compare_metadata(bad,descs['reference'],'describe'),'metadata negative');self_checks+=1
        for label in ['aster','reference','reference_b','aster_b']:
            oid=23 if label in ('aster','reference') else 20;run(label,'7' if oid==23 else '2147483648',oid);run(label,None,oid)
        checkpoint('coexist');observers('coexist');eligible+=1
        register('parse_error','/*中*/ SELECT )','Parse')
        for label in ['aster','reference']:
            result=observe(prepare(conns[label],b'bad',b'/*\xe4\xb8\xad*/ SELECT )',0,None));record('parse_error',label,dict(phase='Parse',sql='/*中*/ SELECT )'),result)
            check(result['status']==7 and result['state']=='42601' and result['position']=='14' and transaction(conns[label])==0,'Parse error and libpq implicit Sync')
            recover(label)
            result=q(label,'/*中*/ SELECT )','simple_error');check(result['state']=='42601' and result['position']=='14','Q original position');recover(label)
        for label in ['aster_b','reference_b']:
            run(label,'2147483648',20);check(q(label,'SHOW search_path','B_path')['rows']==[['public']],'B path unchanged')
        checkpoint('parse_error');observers('parse_error');eligible+=1
        register('close_statement','SELECT $1::int8 AS v','Close/Parse/Describe/Execute',[20],['2147483648'])
        for label in ['aster','reference']:
            close(label);prep(label,20);run(label,'2147483648',20)
        for label in ['aster_b','reference_b']:run(label,'9',20)
        checkpoint('close_statement');observers('close_statement');eligible+=1
        register('close_portal','SELECT $1::int4 AS v','Parse/Bind/Describe/Execute/Close/Sync',[23],['7'])
        check(wire_portals('aster')==wire_portals('reference'),'named portal differential')
        checkpoint('close_portal');observers('close_portal');eligible+=1
        register('reconnect','SELECT $1::int4 AS v','Close/disconnect/Parse/Describe/Execute',[23],['11'])
        for label in ['aster','reference']:
            close(label);finish(conns.pop(label));open_conn(label)
            check(q(label,'SHOW search_path','fresh_path')['rows']==[['public']],'explicit fresh connection path')
            prep(label,23);run(label,'11',23)
        for label in ['aster_b','reference_b']:run(label,'13',20)
        checkpoint('reconnect');observers('reconnect');eligible+=1
        register('transaction_error','/*中*/ SELECT )','BEGIN/Parse/Sync/ROLLBACK/Q')
        for label in ['aster','reference']:
            check(q(label,'BEGIN','begin')['status']==1,'begin')
            result=observe(prepare(conns[label],b'bad_tx',b'/*\xe4\xb8\xad*/ SELECT )',0,None));record('transaction_error',label,dict(phase='Parse',sql='/*中*/ SELECT )'),result)
            check(result['state']=='42601' and result['position']=='14' and transaction(conns[label])==3,'failed transaction after Sync')
            recover(label);run(label,'17',23)
        checkpoint('transaction_error');observers('transaction_error');eligible+=1
        parsed=urlsplit(os.environ['MYSQL_REFERENCE_DSN']);env=dict(os.environ,MYSQL_PWD=parsed.password or '')
        args=['mysql','--protocol=TCP','-h',parsed.hostname,'-P',str(parsed.port),'-u',parsed.username or 'root','--batch','--raw','--skip-column-names']
        def mysql_ref(sql,types=False,case_name='control'):
            process=subprocess.run(args+(['--column-type-info','-vvv'] if types else [])+['-e',"SET SESSION sql_mode=''; SET NAMES utf8mb4 COLLATE utf8mb4_general_ci; "+sql],env=env,capture_output=True,text=True,timeout=15)
            result=dict(sql=sql,code=process.returncode,output=process.stdout.splitlines(),error=process.stderr)
            record('mysql_reference_'+case_name,'mysql',dict(phase='Q/PREPARE/EXECUTE',sql=sql),dict(result,lifecycle_case=case_name));return result
        identity=mysql_ref('SELECT VERSION(),@@version_compile_os,@@version_compile_machine; SELECT @@sql_mode,@@character_set_connection,@@collation_connection')
        check(identity['code']==0,'MySQL exact identity')
        scalar_baseline=mysql_ref('SELECT CAST(1 AS SIGNED) AS v',True)
        check(scalar_baseline['code']==0 and re.findall(r'Type:\s+(\w+)', '\n'.join(scalar_baseline['output']))==['LONGLONG'],'reference bare CAST baseline; local 253 is a pre-existing scalar metadata gap')
        for name,_,_ in cases:
            table='mysql.'+scope
            sql=f"CREATE TEMPORARY TABLE {table}(id INT); INSERT INTO {table} VALUES(31); SELECT id AS v FROM {table} WHERE id=CAST(31 AS SIGNED); PREPARE p FROM 'SELECT id AS v FROM {table} WHERE id=CAST(? AS SIGNED)'; SET @v=31; EXECUTE p USING @v; DEALLOCATE PREPARE p; DROP TEMPORARY TABLE {table}"
            result=mysql_ref(sql,True,case_name=name)
            check(result['code']==0 and re.findall(r'Type:\s+(\w+)', '\n'.join(result['output']))==['LONG','LONG'],'MySQL reference own types/prepare')
            check(sum(bool(re.fullmatch(r'\|\s*31\s*\|',line)) for line in result['output'])==2,'MySQL reference own values')
        result=mysql_ref('SELECT 1::int4');check(result['code']!=0 and '1064 (42000)' in result['error'],'MySQL own negative SQL')
        final=copy.deepcopy(provenance);final['source']=source_snapshot(root);final['build']['sha256']=hashlib.sha256(Path(build['path']).read_bytes()).hexdigest()
        final['client']['sha256']=hashlib.sha256(library.read_bytes()).hexdigest()
        final['reference_build']=json.loads(Path(os.environ['PG_CORE_REFERENCE_BUILD_FILE']).read_text())
        current=query(conns['reference'],"SELECT version(),current_setting('server_version_num')")['rows'][0]
        final['reference']=dict(version=current[0],server_version_num=current[1])
        check(validate_provenance(provenance,final),'source/build/client/reference unchanged');done=True
    finally:
        for sock in wire_sockets:sock.close()
        for label,name in prepared:
            if label in conns:
                if transaction(conns[label]) in (2,3):clean=query(conns[label],'ROLLBACK')['status']==1 and clean
                clean=observe(close_prepared(conns[label],name))['status']==1 and clean
        if reference_created:
            result=q('reference','DROP TABLE IF EXISTS public.'+scope,'cleanup');clean=result['status']==1 and clean
        for conn in conns.values():
            if conn:
                if transaction(conn) in (2,3):clean=query(conn,'ROLLBACK')['status']==1 and clean
                finish(conn)
        # Always persist failed-run cleanup and counts before propagating its
        # original exception. Successful runs assert completion below.
        summary=dict(cases=6,assertions=checks+int(done),self_checks=self_checks,unique_effective=eligible if done and clean else 0,cleanup_ok=clean,
                     layers=dict(libpq_cases=5,named_portal_wire_cases=1,mysql_local_checkpoints=8 if done else 0,mysql_reference_cases=6 if done else 0))
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    check(done and clean,'completion/cleanup')
    print(json.dumps(summary),flush=True)

if task == 24:
    lifecycle_handoff()
    sys.exit(0)

if task == 18:
    cte_handoff()
    sys.exit(0)

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

if task == 10:
    array_nesting_handoff()
    sys.exit(0)

if task == 5:
    update_returning_handoff()
    sys.exit(0)

if task == 6:
    delete_returning_handoff()
    sys.exit(0)

if task == 7:
    conflict_ignore_handoff()
    sys.exit(0)

if task == 8:
    conflict_update_handoff()
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
