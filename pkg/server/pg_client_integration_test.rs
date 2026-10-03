// Copyright 2026 AsterSQL.
//! Real external libpq client regression. Install libpq 18 and Python 3;
//! PG_LIBPQ_LIBRARY selects the system library. No protocol code is reimplemented.
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn postgres_client_protocol_versions() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    struct Driver;
    impl crate::server::ServerDriver for Driver {
        fn name(&self) -> &str {
            "tidb"
        }
    }
    let server = crate::server::Server::new_test(
        crate::server::ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            postgres_port: Some(0),
            status: crate::server::StatusConfig {
                report_status: false,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(Driver),
    );
    server
        .set_connection_runtime(
            driver,
            Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        )
        .unwrap();
    server
        .run(Arc::new(crate::runtime::CanonicalServerDomain::new(domain)))
        .unwrap();
    let address = server.postgres_listener_addr().unwrap();
    let mut mysql = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    mysql_handshake(&mut mysql);
    let result = Command::new("python3")
        .arg("-c")
        .arg(LIBPQ_WORKFLOW)
        .arg(address.port().to_string())
        .arg(crate::pg_catalog::DATABASES_SQL)
        .args([
            DATAGRIP_VIEW_SOURCES_SQL,
            DATAGRIP_FUNCTION_SOURCES_SQL,
            DATAGRIP_RELATIONS_SQL,
        ])
        .output();
    // The authenticated MySQL connection stays usable after the PG workload.
    mysql_packet(&mut mysql, 0, &[0x0e]); // COM_PING
    assert_eq!(mysql_read(&mut mysql).first(), Some(&0));
    mysql_packet(&mut mysql, 0, &[0x01]); // COM_QUIT
    server.close();
    let output = result.expect("Python 3 is required to call the external libpq client");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "external libpq workflow failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn mysql_read(socket: &mut TcpStream) -> Vec<u8> {
    let mut header = [0; 4];
    socket.read_exact(&mut header).unwrap();
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
    assert!(length < 1 << 20);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).unwrap();
    body
}
fn mysql_packet(socket: &mut TcpStream, sequence: u8, body: &[u8]) {
    let length = body.len();
    socket
        .write_all(&[
            length as u8,
            (length >> 8) as u8,
            (length >> 16) as u8,
            sequence,
        ])
        .unwrap();
    socket.write_all(body).unwrap();
}
fn mysql_handshake(socket: &mut TcpStream) {
    assert_eq!(mysql_read(socket).first(), Some(&10));
    let capabilities: u32 = (1 << 9) | (1 << 15) | (1 << 19);
    let mut response = capabilities.to_le_bytes().to_vec();
    response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
    response.push(45);
    response.extend_from_slice(&[0; 23]);
    response.extend_from_slice(b"root\0\0mysql_native_password\0");
    mysql_packet(socket, 1, &response);
    assert_eq!(mysql_read(socket).first(), Some(&0));
}

// DataGrip 2025.1.3 database log, 2026-10-03 08:45:58, session 1533977248.
// Keep JDBC display SQL verbatim; the client workflow converts the sole ?
// to a literal for Query or $1 for Parse, without changing inactive comments.
// Statement 1869279758.
const DATAGRIP_VIEW_SOURCES_SQL: &str = r#"select
       T.relkind as view_kind,
       T.oid as view_id,
       pg_catalog.pg_get_viewdef(T.oid, true) as source_text
from pg_catalog.pg_class T
  join pg_catalog.pg_namespace N on T.relnamespace = N.oid
where N.oid = ?::oid
  and T.relkind in ('m','v')
  --  and T.relname in ( :[*f_names] )
  --  and (pg_catalog.age(T.xmin) <= #SRCTXAGE or exists(
  --  select A.attrelid from pg_catalog.pg_attribute A where A.attrelid = T.oid and pg_catalog.age(A.xmin) <= #SRCTXAGE))
"#;

// Statement 1869279759.
const DATAGRIP_FUNCTION_SOURCES_SQL: &str = r#"with system_languages as ( select oid as lang
                           from pg_catalog.pg_language
                           where lanname in ('c','internal') )
select oid as id,
       pg_catalog.pg_get_function_arguments(oid) as arguments_def,
       pg_catalog.pg_get_function_result(oid) as result_def,
       pg_catalog.pg_get_function_sqlbody(oid) /* null */ as sqlbody_def,
       prosrc as source_text
from pg_catalog.pg_proc
where pronamespace = ?::oid
  --  and pg_proc.proname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #SRCTXAGE
  and not (prokind = 'a') /* proisagg */
  and prolang not in (select lang from system_languages)
  and prosrc is not null
"#;

// Statement 1869279760.
const DATAGRIP_RELATIONS_SQL: &str = r#"select D.objid as dependent_id,
       D.refobjid as owner_id,
       D.refobjsubid as owner_subobject_id
from pg_depend D
  join pg_class C_SEQ on D.objid    = C_SEQ.oid and D.classid    = 'pg_class'::regclass::oid
  join pg_class C_TAB on D.refobjid = C_TAB.oid and D.refclassid = 'pg_class'::regclass::oid
where C_SEQ.relkind = 'S'
  and C_TAB.relkind = 'r'
  and D.refobjsubid <> 0
  and (D.deptype = 'a' or D.deptype = 'i')
  and C_TAB.relnamespace = ?::oid
order by owner_id
"#;

const LIBPQ_WORKFLOW: &str = r#"
import ctypes as c, ctypes.util, os, sys
library = os.environ.get('PG_LIBPQ_LIBRARY')
if not library:
    library = '/opt/homebrew/opt/libpq/lib/libpq.dylib' if sys.platform == 'darwin' else ctypes.util.find_library('pq')
assert library, 'install PostgreSQL libpq 18 or set PG_LIBPQ_LIBRARY'
pq = c.CDLL(library)
def api(name, result, *args):
    fn = getattr(pq, name)
    fn.restype, fn.argtypes = result, args
    return fn
ptr, text, integer = c.c_void_p, c.c_char_p, c.c_int
version = api('PQlibVersion', integer)
assert version() >= 180000, 'protocol 3.2 requires libpq 18'
print('external PostgreSQL libpq version:', version(), flush=True)
connect = api('PQconnectdb', ptr, text)
status = api('PQstatus', integer, ptr)
error = api('PQerrorMessage', text, ptr)
finish = api('PQfinish', None, ptr)
protocol = api('PQfullProtocolVersion', integer, ptr)
parameter_status = api('PQparameterStatus', text, ptr, text)
server_version = api('PQserverVersion', integer, ptr)
execute = api('PQexec', ptr, ptr, text)
params = api('PQexecParams', ptr, ptr, text, integer, c.POINTER(c.c_uint), c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
result_status = api('PQresultStatus', integer, ptr)
result_error = api('PQresultErrorMessage', text, ptr)
rows = api('PQntuples', integer, ptr)
columns = api('PQnfields', integer, ptr)
value = api('PQgetvalue', text, ptr, integer, integer)
is_null = api('PQgetisnull', integer, ptr, integer, integer)
field_name = api('PQfname', text, ptr, integer)
field_type = api('PQftype', c.c_uint, ptr, integer)
error_field = api('PQresultErrorField', text, ptr, integer)
clear = api('PQclear', None, ptr)
txn = api('PQtransactionStatus', integer, ptr)
cancel_create = api('PQcancelCreate', ptr, ptr)
cancel_blocking = api('PQcancelBlocking', integer, ptr)
cancel_finish = api('PQcancelFinish', None, ptr)
base_conninfo = f'host=127.0.0.1 port={sys.argv[1]} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5'
for options, expected_protocol in [('', 30000), (' min_protocol_version=3.2 max_protocol_version=3.2', 30002)]:
    conninfo = base_conninfo + options
    conn = connect(conninfo.encode())
    assert conn
    try:
        assert status(conn) == 0, error(conn).decode()
        assert protocol(conn) == expected_protocol, protocol(conn)
        assert parameter_status(conn, b'server_version') == b'18.0 (AsterSQL)'
        assert server_version(conn) == 180000, server_version(conn)
        def query(sql, expected=None, parameter=None, metadata=None, extended=False, sqlstate=None, parameter_oid=23):
            if extended:
                result = params(conn, sql.encode(), 0, None, None, None, None, 0)
            elif parameter is None:
                result = execute(conn, sql.encode())
            else:
                oids = (c.c_uint * 1)(parameter_oid)
                values = (text * 1)(str(parameter).encode())
                result = params(conn, sql.encode(), 1, oids, values, None, None, 0)
            assert result
            try:
                if sqlstate is not None:
                    assert result_status(result) == 7, result_status(result)
                    assert error_field(result, ord('C')) == sqlstate.encode(), result_error(result).decode()
                    return
                assert result_status(result) in (1, 2), result_error(result).decode()
                if metadata is not None:
                    actual_metadata = [(field_name(result, col).decode(), field_type(result, col)) for col in range(columns(result))]
                    assert actual_metadata == metadata, (sql, actual_metadata, metadata)
                actual = [[None if is_null(result, r, col) else value(result, r, col).decode() for col in range(columns(result))] for r in range(rows(result))]
                if expected is not None:
                    assert actual == expected, (sql, actual, expected)
                return actual
            finally:
                clear(result)
        for invalid_info in [conninfo.replace('user=root', 'user=intruder')]:
            invalid = connect(invalid_info.encode())
            try:
                assert status(invalid) != 0, 'invalid identity must be rejected'
            finally:
                finish(invalid)
        query('SELECT 1', [['1']])
        # Source SQL remains frozen; unsupported JOIN/CTE features now fail in
        # the PG catalog layer, rather than leaking to native name resolution.
        # Use the real current namespace ID; no catalog rows are mocked.
        namespace_id = int(query("select oid from pg_catalog.pg_namespace where nspname = 'public'")[0][0])
        for label, displayed, simple_state, parse_state in zip(
                ['RetrieveViewSources', 'RetrieveFunctionSources', 'RetrieveRelations'],
                sys.argv[3:6], ['0A000', '0A000', '0A000'], ['0A000', '0A000', '0A000']):
            assert displayed.count('?') == 1, label
            query(displayed.replace('?', str(namespace_id)), sqlstate=simple_state)
            query('SELECT 1', [['1']])
            query(displayed.replace('?', '$1'), parameter=namespace_id,
                  parameter_oid=26, sqlstate=parse_state)
            query('SELECT 1', [['1']])
            print(f'{expected_protocol}: {label}: Query={simple_state}, Parse(oid 26)={parse_state}; recovery passed', flush=True)
        namespace_sql = """select N.oid::bigint as id, N.xmin as state_number, nspname as name,
            D.description, pg_catalog.pg_get_userbyid(N.nspowner) as "owner"
            from pg_catalog.pg_namespace N left join pg_catalog.pg_description D on N.oid = D.objoid
            order by case when nspname = pg_catalog.current_schema() then -1::bigint else N.oid::bigint end"""
        tablespace_sql = 'SELECT oid::bigint AS id, spcname AS name, pg_catalog.pg_get_userbyid(spcowner) AS "owner", spcacl, spcoptions FROM pg_catalog.pg_tablespace ORDER BY oid'
        query('CREATE DATABASE pg_client_catalog_live')
        query('CREATE TABLE public.pg_client_relation_live (id INT)')
        for extended in [False, True]:
            relations = query("SELECT oid, relname, relnamespace, relkind FROM pg_class WHERE relname = 'pg_client_relation_live'", metadata=[('oid', 26), ('relname', 25), ('relnamespace', 26), ('relkind', 25)], extended=extended)
            assert len(relations) == 1 and relations[0][1:] == ['pg_client_relation_live', str(namespace_id), 'r'], relations
            assert int(relations[0][0]) > 0, relations
            databases = query(sys.argv[2], metadata=[('id', 20), ('name', 25), ('description', 25), ('is_template', 16), ('allow_connections', 16), ('owner', 25)], extended=extended)
            assert databases[0][1] == 'test', databases
            assert any(r[1] == 'pg_client_catalog_live' for r in databases), databases
            assert all(int(r[0]) != 0 and r[2:] == [None, 'f', 't', None] for r in databases), databases
            assert [int(r[0]) for r in databases[1:]] == sorted(int(r[0]) for r in databases[1:]), databases
            namespaces = query(namespace_sql, metadata=[('id', 20), ('state_number', 20), ('name', 25), ('description', 25), ('owner', 25)], extended=extended)
            assert namespaces[0][2] == 'public', namespaces
            assert {r[2] for r in namespaces} == {'public', 'pg_catalog'}, namespaces
            assert all(int(r[0]) > 0 and r[1] is None and r[3:] == [None, None] for r in namespaces), namespaces
            assert [int(r[0]) for r in namespaces[1:]] == sorted(int(r[0]) for r in namespaces[1:]), namespaces
            assert len({r[0] for r in namespaces}) == len(namespaces)
            query(tablespace_sql, [], metadata=[('id', 20), ('name', 25), ('owner', 25), ('spcacl', 25), ('spcoptions', 25)], extended=extended)
            # Extracted from DataGrip PgIntroQueries.sql: the ID-only probe is
            # supported; full introspection requires features beyond this phase.
            query('select oid::bigint from pg_catalog.pg_tablespace', [], metadata=[('oid', 20)], extended=extended)
            full_tablespace_sql = 'select T.oid::bigint as id, T.spcname as name, T.xmin as state_number, pg_catalog.pg_get_userbyid(T.spcowner) as owner, pg_catalog.pg_tablespace_location(T.oid) as location, T.spcoptions as options, D.description as comment from pg_catalog.pg_tablespace T left join pg_catalog.pg_shdescription D on D.objoid = T.oid'
            query(full_tablespace_sql, extended=extended, sqlstate='0A000')
            query('SELECT 1', [['1']], extended=extended)
            for sql, state in [('SELECT oid FROM pg_catalog.pg_missing', '42P01'), ('SELECT oid FROM pg_catalog.pg_namespace GROUP BY oid', '0A000'), ('SELECT (', '42601')]:
                query(sql, extended=extended, sqlstate=state)
                query('SELECT 1', [['1']], extended=extended)
        query('DROP TABLE public.pg_client_relation_live')
        query('DROP DATABASE pg_client_catalog_live')
        assert not any(r[1] == 'pg_client_catalog_live' for r in query(sys.argv[2]))
        query('CREATE TABLE pg_real_client (id INT PRIMARY KEY, v VARCHAR(30))')
        query("INSERT INTO pg_real_client VALUES (1, 'one')")
        query("UPDATE pg_real_client SET v = 'two' WHERE id = 1")
        query('SELECT v FROM pg_real_client WHERE id = $1', [['two']], parameter=1)
        query('BEGIN')
        assert txn(conn) == 2
        query("INSERT INTO pg_real_client VALUES (2, 'committed')")
        query('COMMIT')
        assert txn(conn) == 0
        query('BEGIN')
        query("UPDATE pg_real_client SET v = 'rolled back' WHERE id = 1")
        query('ROLLBACK')
        assert txn(conn) == 0
        query('SELECT v FROM pg_real_client ORDER BY id', [['two'], ['committed']])
        query('DELETE FROM pg_real_client WHERE id = 2')
        query('SELECT id FROM pg_real_client', [['1']])
        # libpq owns the version-specific BackendKeyData/CancelRequest framing.
        # An idle cancel must complete without poisoning the next query.
        cancel = cancel_create(conn)
        assert cancel
        try:
            assert cancel_blocking(cancel) == 1
        finally:
            cancel_finish(cancel)
        query('SELECT 1', [['1']])
        query('DROP TABLE pg_real_client')
        print(f'{expected_protocol}: startup, catalogs (simple/extended metadata, NULL, rows, ordering), error recovery, CRUD, typed parameters, transactions and idle cancel passed', flush=True)
    finally:
        finish(conn)
"#;
