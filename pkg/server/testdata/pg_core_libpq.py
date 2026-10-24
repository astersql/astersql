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

conninfo = (
    f"host=127.0.0.1 port={int(sys.argv[1])} user=root dbname={sys.argv[2]} "
    "sslmode=disable gssencmode=disable connect_timeout=5"
)
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
