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
def integer_differential(aster_dsn):
    import copy
    import signal
    import json
    from pg_core_evidence import CaseRegistry, load_cases, append_observation, compare_scalar_output, compare_rows, compare_metadata
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
    counts = Counter()
    registry = CaseRegistry(load_cases(), table)
    statements = {}
    descriptions = {"aster": {}, "reference": {}}
    from pathlib import Path
    evidence_dir = Path(os.environ['PG_CORE_EVIDENCE_DIR'])
    run_id = evidence_dir.name
    attempt = 1
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

    def equal(case, actual, reference, row_order='strict', phase='query'):
        assert compare_rows(actual, reference, row_order), (case, actual, reference)
        assert compare_metadata(actual, reference, phase), (case, actual, reference)
        keys = ('status', 'state')
        assert {k: actual[k] for k in keys} == {k: reference[k] for k in keys}, (case, actual, reference)

    def check(case, family, operation, expected_status=2, expected_rows=None,
              expected_state="", expected_params=None):
        actual_input, send = operation
        registered_input = registry.claim(case, family, actual_input)
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
            assert obs["status"] == expected_status, (side, case, obs)
            assert obs["state"] == expected_state, (side, case, obs)
            if expected_rows is not None:
                assert obs["rows"] == expected_rows, (side, case, obs)
            if expected_params is not None:
                assert obs["params"] == expected_params, (side, case, obs)
            phase, statement = registered_input['phase'], registered_input['statement']
            if phase == 'describe':
                descriptions[side][statement] = copy.deepcopy(obs)
            elif phase == 'execute':
                description = descriptions[side][statement]
                assert len(registered_input['arguments']) == description['param_count'], (side, case)
                if expected_status == 2:
                    for key in ['fields', 'formats']:
                        assert obs[key] == description[key], (side, case, key)
        row_order = registry.cases['cases'][len(registry.seen) - 1].get('row_order', 'strict')
        equal(case, observations["aster"][case], observations["reference"][case], row_order, registered_input["phase"])
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
        (evidence_dir / 'environment.json').write_text(json.dumps({
            'libpq_version': version(), 'libpq_library': str(Path(library).resolve()),
            'python': sys.version, 'endpoints': {
                side: {'protocol': protocol(conn),
                       'server_version': parameter_status(conn, b'server_version').decode()}
                for side, conn in connections.items()}}, sort_keys=True))
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

        sql("begin", "transaction", "BEGIN", expected_status=1)
        sql("transaction_error", "transaction", "SELECT 'x'::int4", expected_status=7, expected_state="22P02")
        assert all(transaction(conn) == 3 for conn in connections.values()), "expected E"
        sql("transaction_failed", "transaction", "SELECT 1::int4 AS n", expected_status=7, expected_state="25P02")
        sql("rollback", "transaction", "ROLLBACK", expected_status=1)
        assert all(transaction(conn) == 0 for conn in connections.values()), "expected I"
        sql("rollback_recovery", "recovery", "SELECT 1::int4 AS n", expected_rows=[["1"]])

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
        assert registry.finish() == 100
        print("registered inputs: business=97 self_check=3 missing=0 duplicate=0", flush=True)
        print("differential effective scenarios:", sum(counts.values()), dict(sorted(counts.items())), flush=True)
    finally:
        business_failed = sys.exc_info()[0] is not None
        cleanup_errors = []
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
                    assert obs["status"] == 1, (side, "table cleanup failed")
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


conninfo = (
    f"host=127.0.0.1 port={int(sys.argv[1])} user=root dbname={sys.argv[2]} "
    "sslmode=disable gssencmode=disable connect_timeout=5"
)
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
