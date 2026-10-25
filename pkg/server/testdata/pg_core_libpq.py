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
