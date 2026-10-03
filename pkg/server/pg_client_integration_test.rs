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
    pg_introspection_clients();
}

#[test]
fn pg_introspection_clients() {
    run_clients(true);
}

#[test]
fn pg_introspection_clients_mysql_only() {
    run_clients(false);
}

#[test]
fn pg_introspection_clients_stored_view_source() {
    for (stored, expected) in [
        (
            "CREATE VIEW `as` AS SELECT 'AS WITH LOCAL CHECK OPTION' AS label",
            "SELECT 'AS WITH LOCAL CHECK OPTION' AS label",
        ),
        (
            "CREATE VIEW v AS SELECT id FROM t WITH LOCAL CHECK OPTION;",
            "SELECT id FROM t",
        ),
        (
            "CREATE VIEW v AS SELECT id FROM t WITH CASCADED CHECK OPTION",
            "SELECT id FROM t",
        ),
        (
            "CREATE VIEW v AS /* AS ignored */ SELECT 1",
            "/* AS ignored */ SELECT 1",
        ),
        ("SELECT 1", "SELECT 1"),
    ] {
        assert_eq!(
            crate::pg_name::stored_view_select(stored).unwrap(),
            expected
        );
    }
    assert!(crate::pg_name::stored_view_select("DROP TABLE t").is_err());
    assert!(crate::pg_name::stored_view_select("SELECT 1; SELECT 2").is_err());
}

fn run_clients(pg_enabled: bool) {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let database = format!("pg_clients_{}_{}", std::process::id(), u8::from(pg_enabled));
    native
        .execute(&format!("CREATE DATABASE {database}"))
        .unwrap();
    for sql in [
        format!("CREATE TABLE {database}.client_view_base (id INT)"),
        format!(
            "CREATE VIEW `{database}`.`client_view_live` AS SELECT id FROM `{database}`.`client_view_base`"
        ),
        format!("CREATE TABLE {database}.client_isolation (id INT)"),
        format!("INSERT INTO {database}.client_isolation VALUES (17)"),
    ] {
        native.execute(&sql).unwrap();
    }
    let view_source = format!("SELECT id FROM `{database}`.`client_view_base`");
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
            postgres_port: pg_enabled.then_some(0),
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
    struct CloseServer<'a>(&'a crate::server::Server);
    impl Drop for CloseServer<'_> {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = CloseServer(&server);
    let address = server.postgres_listener_addr();
    let mut mysql = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    mysql_handshake(&mut mysql);
    mysql_isolation(&mut mysql);
    let Some(address) = address else {
        assert!(!pg_enabled);
        mysql_query(&mut mysql, "DROP DATABASE public");
        mysql_packet(&mut mysql, 0, &[0x01]);
        native
            .execute(&format!("DROP DATABASE {database}"))
            .unwrap();
        return;
    };
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
        .args([database.as_str(), view_source.as_str()])
        .output();
    let jdbc_result = run_jdbc(address.port(), &database, &view_source);
    assert_eq!(
        mysql_query(&mut mysql, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(&mut mysql, "DROP DATABASE public");
    // The authenticated MySQL connection stays usable after the PG workload.
    mysql_packet(&mut mysql, 0, &[0x0e]); // COM_PING
    assert_eq!(mysql_read(&mut mysql).first(), Some(&0));
    mysql_packet(&mut mysql, 0, &[0x01]); // COM_QUIT
    server.close();
    native
        .execute(&format!("DROP DATABASE {database}"))
        .unwrap();
    let output = result.expect("Python 3 is required to call the external libpq client");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "external libpq workflow failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    jdbc_result.unwrap();
}

fn run_jdbc(port: u16, database: &str, view_source: &str) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let libraries = std::env::var_os("PG_JDBC_LIBRARIES")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_else(|| ["42.7.13", "42.7.3"].iter().map(|version| {
            std::path::PathBuf::from(&home).join(format!("Library/Application Support/JetBrains/DataGrip2025.1/jdbc-drivers/PostgreSQL/{version}/org/postgresql/postgresql/{version}/postgresql-{version}.jar"))
        }).collect());
    let java = std::env::var("PG_JAVA").unwrap_or_else(|_| {
        "/Applications/DataGrip.app/Contents/jbr/Contents/Home/bin/java".into()
    });
    let directory =
        std::env::temp_dir().join(format!("pg-introspection-{}-{port}", std::process::id()));
    std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let source = directory.join("PgIntrospection.java");
    std::fs::write(&source, JDBC_WORKFLOW).map_err(|e| e.to_string())?;
    if libraries.is_empty() {
        return Err("PG_JDBC_LIBRARIES must include a real installed JDBC driver".into());
    }
    for library in libraries {
        if !library.is_file() {
            return Err(format!(
                "missing installed PostgreSQL JDBC driver: {}",
                library.display()
            ));
        }
        let output = Command::new(&java)
            .arg("--class-path")
            .arg(&library)
            .arg(&source)
            .arg(port.to_string())
            .args([
                DATAGRIP_VIEW_SOURCES_SQL,
                DATAGRIP_FUNCTION_SOURCES_SQL,
                DATAGRIP_RELATIONS_SQL,
            ])
            .args([database, view_source])
            .output()
            .map_err(|e| format!("start installed Java runtime: {e}"))?;
        println!("{}", String::from_utf8_lossy(&output.stdout));
        if !output.status.success() {
            return Err(format!(
                "JDBC workflow failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    Ok(())
}

const JDBC_WORKFLOW: &str = r#"
import java.sql.*;
import org.postgresql.util.PGobject;
class PgIntrospection {
    static void check(boolean ok, String message) { if (!ok) throw new AssertionError(message); }
    static String viewSource;
    static void verify(ResultSet rows, int count, boolean view) throws Exception {
        check(rows.getMetaData().getColumnCount() == count, "Describe column count");
        if (view) {
            check(rows.next(), "real view missing");
            check(rows.getString(1).equals("v"), "view kind");
            check(rows.getLong(2) > 0, "view oid");
            check(rows.getString(3).equals(viewSource), "persisted SELECT source: " + rows.getString(3));
        }
        check(!rows.next(), "unexpected introspection rows");
    }
    public static void main(String[] args) throws Exception {
        viewSource = args[5];
        String options = "sslmode=disable&gssEncMode=disable&prepareThreshold=1&connectTimeout=5&socketTimeout=10";
        try (Connection c = DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+args[0]+"/"+args[4]+"?"+options, "root", ""); Statement s = c.createStatement()) {
            // Preserve the installed driver defaults for binary parameters/results.

            System.out.println("installed PostgreSQL JDBC " + c.getMetaData().getDriverVersion() + "; " + options);
            long namespace;
            try (ResultSet r = s.executeQuery("select oid from pg_namespace where nspname='public'")) { check(r.next(), "public missing"); namespace = r.getLong(1); }
            for (int i=1; i<=3; i++) {
                try (ResultSet r = s.executeQuery(args[i].replace("?", Long.toString(namespace)))) { verify(r, i==1?3:i==2?5:3, i==1); }
                try (PreparedStatement p = c.prepareStatement(args[i])) {
                    PGobject oid = new PGobject(); oid.setType("oid"); oid.setValue(Long.toString(namespace)); p.setObject(1, oid);
                    check(p.getMetaData().getColumnCount() == (i==2?5:3), "Statement Describe");
                    for (int repeat=0; repeat<2; repeat++) { try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, i==1); } }
                    p.setLong(1, namespace);
                    for (int repeat=0; repeat<2; repeat++) { try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, i==1); } }
                    p.setNull(1, Types.OTHER, "oid"); try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, false); }
                    oid.setValue("4294967295"); p.setObject(1, oid); try (ResultSet r=p.executeQuery()) { verify(r, i==2?5:3, false); }
                }
            }
            try (PreparedStatement p=c.prepareStatement("SELECT relname FROM pg_class WHERE relname='jdbc_parse_ddl'")) {
                check(p.getMetaData().getColumnCount()==1, "Parse-before-DDL Describe");
                try(ResultSet r=p.executeQuery()) { check(!r.next(), "before DDL rows"); }
                s.execute("CREATE TABLE public.jdbc_parse_ddl (id INT)");
                try { try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1).equals("jdbc_parse_ddl") && !r.next(), "live rows after DDL"); } }
                finally { s.execute("DROP TABLE public.jdbc_parse_ddl"); }
                try(ResultSet r=p.executeQuery()) { check(!r.next(), "live rows after DROP"); }
            }
            s.execute("CREATE TABLE public.jdbc_client_live (id INT PRIMARY KEY, note VARCHAR(30))");
            try {
                try (PreparedStatement p=c.prepareStatement("INSERT INTO public.jdbc_client_live VALUES (?, ?)")) { p.setInt(1, 7); p.setString(2, "jdbc"); check(p.executeUpdate()==1, "insert count"); }
                try (PreparedStatement p=c.prepareStatement("SELECT note FROM public.jdbc_client_live WHERE id=?")) { p.setInt(1,7); try(ResultSet r=p.executeQuery()) { check(r.next() && r.getString(1).equals("jdbc") && !r.next(), "public CRUD row"); } }
                s.execute("UPDATE public.jdbc_client_live SET note='updated' WHERE id=7");
                try (ResultSet r=s.executeQuery("SELECT note FROM public.jdbc_client_live")) { check(r.next() && r.getString(1).equals("updated"), "update row"); }
                s.execute("DELETE FROM public.jdbc_client_live WHERE id=7");
                try (ResultSet r=s.executeQuery("SELECT id FROM public.jdbc_client_live")) { check(!r.next(), "delete row"); }
                try { s.executeQuery("SELECT oid FROM pg_catalog.pg_missing"); throw new AssertionError("missing catalog accepted"); } catch (SQLException e) { check("42P01".equals(e.getSQLState()), e.toString()); }
                try (ResultSet r=s.executeQuery("SELECT 1")) { check(r.next() && r.getInt(1)==1, "error recovery"); }
            } finally { s.execute("DROP TABLE public.jdbc_client_live"); }
            System.out.println("JDBC original introspection Statement/PreparedStatement, oid/NULL/boundary, Describe/Parse-before-DDL, public CRUD, SQLSTATE 42P01 and recovery passed");
        }
    }
}
"#;

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

// Use the same real MySQL connection while the independent PG listener is live.
// Native `public` remains an ordinary database, not the PG current-db mapping.
fn mysql_isolation(socket: &mut TcpStream) {
    for sql in [
        "CREATE DATABASE public",
        "CREATE TABLE public.client_isolation (id INT)",
        "INSERT INTO public.client_isolation VALUES (31)",
        "USE public",
    ] {
        assert!(mysql_query(socket, sql).is_empty(), "{sql}");
    }
    assert_eq!(
        mysql_query(socket, "SELECT id FROM client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(socket, "BEGIN");
    mysql_query(socket, "UPDATE client_isolation SET id=32");
    mysql_query(socket, "ROLLBACK");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_query(socket, "USE test");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM public.client_isolation"),
        vec![vec![Some(b"31".to_vec())]]
    );
    mysql_packet(socket, 0, b"\x02public"); // COM_INIT_DB
    assert_eq!(mysql_read(socket)[0], 0);
    mysql_query(socket, "BEGIN");
    mysql_query(socket, "UPDATE client_isolation SET id=33");
    mysql_query(socket, "COMMIT");
    assert_eq!(
        mysql_query(socket, "SELECT id FROM client_isolation"),
        vec![vec![Some(b"33".to_vec())]]
    );
    mysql_query(socket, "UPDATE client_isolation SET id=31");
    let mut request = vec![0x16]; // COM_STMT_PREPARE
    request.extend_from_slice(b"SELECT id FROM public.client_isolation WHERE id=?");
    mysql_packet(socket, 0, &request);
    let prepared = mysql_read(socket);
    assert_eq!(prepared[0], 0, "{prepared:?}");
    let id = &prepared[1..5];
    let columns = u16::from_le_bytes(prepared[5..7].try_into().unwrap());
    let parameters = u16::from_le_bytes(prepared[7..9].try_into().unwrap());
    assert_eq!((columns, parameters), (1, 1));
    for count in [parameters, columns] {
        for _ in 0..count {
            mysql_read(socket);
        }
        assert_eq!(mysql_read(socket)[0], 0xfe);
    }
    let mut execute = vec![0x17]; // COM_STMT_EXECUTE with a real integer parameter
    execute.extend_from_slice(id);
    execute.extend_from_slice(&[0, 1, 0, 0, 0, 0, 1, 3, 0]);
    execute.extend_from_slice(&31_i32.to_le_bytes());
    mysql_packet(socket, 0, &execute);
    assert_eq!(mysql_read(socket), vec![1]);
    mysql_read(socket); // column definition
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let row = mysql_read(socket);
    assert_eq!(&row[..2], &[0, 0]); // binary row, no NULL columns
    assert_eq!(i32::from_le_bytes(row[2..6].try_into().unwrap()), 31);
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let mut close = vec![0x19];
    close.extend_from_slice(id);
    mysql_packet(socket, 0, &close);
    println!(
        "MySQL native public database, USE, rollback, real relation rows and prepared integer parameter passed"
    );
}

fn mysql_query(socket: &mut TcpStream, sql: &str) -> Vec<Vec<Option<Vec<u8>>>> {
    let mut packet = vec![3];
    packet.extend_from_slice(sql.as_bytes());
    mysql_packet(socket, 0, &packet);
    let head = mysql_read(socket);
    assert_ne!(head[0], 0xff, "{sql}: {}", String::from_utf8_lossy(&head));
    if head[0] == 0 {
        return Vec::new();
    }
    let count = usize::from(head[0]);
    assert!(count < 251);
    for _ in 0..count {
        mysql_read(socket);
    }
    assert_eq!(mysql_read(socket)[0], 0xfe);
    let mut rows = Vec::new();
    loop {
        let row = mysql_read(socket);
        if row[0] == 0xfe && row.len() < 9 {
            break;
        }
        assert_ne!(row[0], 0xff, "{sql}: {row:?}");
        let mut offset = 0;
        let values = (0..count)
            .map(|_| {
                let length = row[offset];
                offset += 1;
                if length == 251 {
                    None
                } else {
                    assert!(length < 251);
                    let value = row[offset..offset + usize::from(length)].to_vec();
                    offset += usize::from(length);
                    Some(value)
                }
            })
            .collect();
        assert_eq!(offset, row.len());
        rows.push(values);
    }
    rows
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
close_prepared = api('PQclosePrepared', ptr, ptr, text)
prepare = api('PQprepare', ptr, ptr, text, text, integer, c.POINTER(c.c_uint))
describe = api('PQdescribePrepared', ptr, ptr, text)
prepared_execute = api('PQexecPrepared', ptr, ptr, text, integer, c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
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
base_conninfo = f'host=127.0.0.1 port={sys.argv[1]} user=root dbname={sys.argv[6]} sslmode=disable gssencmode=disable connect_timeout=5'
for options, expected_protocol in [('', 30000), (' min_protocol_version=3.2 max_protocol_version=3.2', 30002)]:
    conninfo = base_conninfo + options
    conn = connect(conninfo.encode())
    assert conn
    try:
        assert status(conn) == 0, error(conn).decode()
        assert protocol(conn) == expected_protocol, protocol(conn)
        assert parameter_status(conn, b'server_version') == b'18.0 (AsterSQL)'
        assert server_version(conn) == 180000, server_version(conn)
        def query(sql, expected=None, parameter=None, metadata=None, extended=False, sqlstate=None, parameter_oid=23, null_parameter=False, binary_parameter=False):
            if extended:
                result = params(conn, sql.encode(), 0, None, None, None, None, 0)
            elif parameter is None and not null_parameter:
                result = execute(conn, sql.encode())
            else:
                oids = (c.c_uint * 1)(parameter_oid)
                payload = None if null_parameter else (int(parameter).to_bytes(4, 'big') if binary_parameter else str(parameter).encode())
                values = (text * 1)(payload)
                lengths = (integer * 1)(0 if payload is None else len(payload))
                formats = (integer * 1)(int(binary_parameter))
                result = params(conn, sql.encode(), 1, oids, values, lengths, formats, 0)
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
        query('SELECT id FROM public.client_isolation', [['17']])
        # Source SQL remains frozen. Functions have no native stored-program rows;
        # native independent sequences have no auto/internal column ownership.
        # Use the real current namespace ID; no catalog rows are mocked.
        namespace_id = int(query("select oid from pg_catalog.pg_namespace where nspname = 'public'")[0][0])
        for label, displayed, simple_state, parse_state in zip(
                ['RetrieveViewSources', 'RetrieveFunctionSources', 'RetrieveRelations'],
                sys.argv[3:6], [None, None, None], [None, None, None]):
            assert displayed.count('?') == 1, label
            source_metadata = {
                'RetrieveViewSources': [('view_kind', 25), ('view_id', 26), ('source_text', 25)],
                'RetrieveFunctionSources': [('id', 26), ('arguments_def', 25), ('result_def', 25), ('sqlbody_def', 25), ('source_text', 25)],
                'RetrieveRelations': [('dependent_id', 26), ('owner_id', 26), ('owner_subobject_id', 23)],
            }.get(label)
            source_rows = None
            if label == 'RetrieveViewSources':
                view_oid = query("select oid from pg_class where relname = 'client_view_live'")[0][0]
                source_rows = [['v', view_oid, sys.argv[7]]]
            query(displayed.replace('?', str(namespace_id)), sqlstate=simple_state,
                  expected=source_rows if source_rows is not None else [] if source_metadata else None, metadata=source_metadata)
            query('SELECT 1', [['1']])
            query(displayed.replace('?', '$1'), parameter=namespace_id,
                  parameter_oid=26, sqlstate=parse_state,
                  expected=source_rows if source_rows is not None else [] if source_metadata else None, metadata=source_metadata)
            query('SELECT 1', [['1']])
            query(displayed.replace('?', '$1'), parameter_oid=26, null_parameter=True, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=4294967295, parameter_oid=26, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=namespace_id, parameter_oid=26, binary_parameter=True,
                  expected=source_rows if source_rows is not None else [], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter_oid=26, null_parameter=True, binary_parameter=True, expected=[], metadata=source_metadata)
            query(displayed.replace('?', '$1'), parameter=4294967295, parameter_oid=26, binary_parameter=True, expected=[], metadata=source_metadata)
            print(f'{expected_protocol}: {label}: Query/Execute(oid 26)={source_rows if source_rows is not None else "typed empty result"}; recovery passed', flush=True)
        for extended in [False, True]:
            for expression in ['pg_get_viewdef(NULL, true)', 'pg_get_viewdef(4294967295::oid, true)', 'pg_get_viewdef(oid, NULL)']:
                query(f"SELECT {expression} AS source_text FROM pg_class WHERE relname='client_view_live'", [[None]], metadata=[('source_text',25)], extended=extended)
            query("SELECT pg_get_viewdef(oid, 'wrong') FROM pg_class", sqlstate='0A000', extended=extended)
            query('SELECT 1', [['1']], extended=extended)
        # PQprepare performs Parse; PQdescribePrepared checks Statement Describe.
        # Keep this statement over DDL and read live rows at each Execute.
        ddl_sql = b"SELECT relname FROM pg_class WHERE relname='pg_parse_ddl_live'"
        result = prepare(conn, b'live_catalog', ddl_sql, 0, None)
        try: assert result_status(result)==1, result_error(result).decode()
        finally: clear(result)
        result = describe(conn, b'live_catalog')
        try:
            assert result_status(result)==1, result_error(result).decode()
            assert columns(result)==1 and field_name(result,0)==b'relname' and field_type(result,0)==25
        finally: clear(result)
        def live_rows():
            result = prepared_execute(conn,b'live_catalog',0,None,None,None,0)
            try:
                assert result_status(result)==2, result_error(result).decode()
                return [[value(result,r,col).decode() for col in range(columns(result))] for r in range(rows(result))]
            finally: clear(result)
        assert live_rows()==[]
        query('CREATE TABLE public.pg_parse_ddl_live (id INT)')
        try: assert live_rows()==[['pg_parse_ddl_live']]
        finally: query('DROP TABLE public.pg_parse_ddl_live')
        assert live_rows()==[]
        result = close_prepared(conn, b'live_catalog')
        try: assert result_status(result)==1, result_error(result).decode()
        finally: clear(result)
        print(f'{expected_protocol}: NULL/boundary view definitions, oid parameters, Describe and Parse-before-DDL live Execute passed', flush=True)
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
            assert databases[0][1] == sys.argv[6], databases
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
