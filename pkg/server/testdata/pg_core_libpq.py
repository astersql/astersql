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

    def interrupted(signum, frame):
        raise KeyboardInterrupt("client interrupted")

    signal.signal(signal.SIGTERM, interrupted)

    def observe(result):
        assert result, "libpq returned no result"
        try:
            return {
                "status": result_status(result),
                "state": (error_field(result, ord("C")) or b"").decode(),
                "fields": [(field_name(result, col).decode(), field_type(result, col),
                            field_size(result, col), field_mod(result, col))
                           for col in range(columns(result))],
                "rows": [[None if is_null(result, row, col) else
                          value(result, row, col).decode()
                          for col in range(columns(result))]
                         for row in range(rows(result))],
                "params": [param_type(result, i) for i in range(nparams(result))],
            }
        finally:
            clear(result)

    def equal(case, actual, reference):
        assert actual == reference, (case, actual, reference)

    def check(case, family, operation, expected_status=2, expected_rows=None,
              expected_state="", expected_params=None):
        for side, connection in connections.items():
            obs = observe(operation(connection))
            observations[side][case] = obs
            assert obs["status"] == expected_status, (side, case, obs)
            assert obs["state"] == expected_state, (side, case, obs)
            if expected_rows is not None:
                assert obs["rows"] == expected_rows, (side, case, obs)
            if expected_params is not None:
                assert obs["params"] == expected_params, (side, case, obs)
        equal(case, observations["aster"][case], observations["reference"][case])
        counts[family] += 1

    def sql(case, family, command, **expected):
        check(case, family, lambda conn: execute(conn, command.encode()), **expected)

    def bind_values(name, args):
        payload = [None if arg is None else str(arg).encode() for arg in args]
        return lambda conn: prepared(conn, name.encode(), len(payload),
                                     (text * len(payload))(*payload), None, None, 0)

    try:
        for side, dsn in [("aster", aster_dsn), ("reference", reference_dsn)]:
            connection = connect(dsn.encode())
            assert connection, "connection allocation failed"
            connections[side] = connection
            # Do not echo libpq connection errors: they may contain DSN secrets.
            assert status(connection) == 0, f"{side} connection failed"
        ref = observe(execute(connections["reference"],
                              b"SELECT version(), current_setting('server_version_num')"))
        assert ref["status"] == 2, "reference version query failed"
        build, number = ref["rows"][0]
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
            check(name + "_prepare", "prepare", lambda conn: prepare(
                conn, name.encode(), query.encode(), len(supplied),
                (c.c_uint * len(supplied))(*supplied) if supplied else None), expected_status=1)
            check(name + "_describe", "describe", lambda conn: describe(conn, name.encode()),
                  expected_status=1, expected_params=inferred)
            for i, args in enumerate(runs):
                if name == "inferred":
                    expected = [[args[1], args[0], args[1]]]
                else:
                    expected = [[None if args[0] is None else str(int(args[0]))]]
                check(f"{name}_run_{i}", "reexecute", bind_values(name, args), expected_rows=expected)
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
                f"CREATE TABLE {table} (id INT PRIMARY KEY, n BIGINT)".encode()))
            assert obs["status"] == 1, (side, obs)
            created.add(side)
        sql("write", "write", f"INSERT INTO {table} VALUES (1, '9007199254740993'::int8), (2, NULL)",
            expected_status=1)
        sql("readback", "readback", f"SELECT CAST(id AS int8) AS id, n::int8 AS n FROM {table} ORDER BY id",
            expected_rows=[["1", "9007199254740993"], ["2", None]])
        empty = f"SELECT id::int4 AS id, CAST(n AS bigint) AS n FROM {table} WHERE id = $1::int4"
        check("empty_prepare", "prepare", lambda conn: prepare(conn, b"empty", empty.encode(), 0, None),
              expected_status=1)
        check("empty_describe", "zero_rows", lambda conn: describe(conn, b"empty"),
              expected_status=1, expected_params=[23])
        check("empty_execute", "zero_rows", bind_values("empty", [999]), expected_rows=[])
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
            changed = copy.deepcopy(observations["aster"][case])
            changed[key] = wrong
            try:
                equal("negative_self_check_" + key, changed, observations["reference"][case])
            except AssertionError:
                counts["negative_self_check"] += 1
            else:
                raise AssertionError("differential comparator missed " + key)
        for required in ["value", "typed_null", "zero_rows", "overflow", "invalid",
                         "reexecute", "recovery", "write", "readback", "negative_self_check"]:
            assert counts[required] > 0, required
        print("differential effective scenarios:", sum(counts.values()), dict(sorted(counts.items())), flush=True)
    finally:
        cleanup_errors = []
        try:
            for side in sorted(created):
                try:
                    connection = connections[side]
                    # Rollback first so cleanup also works after a failed transaction assertion.
                    observe(execute(connection, b"ROLLBACK"))
                    obs = observe(execute(connection, f"DROP TABLE {table}".encode()))
                    assert obs["status"] == 1, (side, "table cleanup failed")
                except Exception as exc:
                    cleanup_errors.append((side, str(exc)))
            assert not cleanup_errors, cleanup_errors
            print("unique-table cleanup passed for", len(created), "endpoints", flush=True)
        finally:
            for connection in connections.values():
                finish(connection)


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
