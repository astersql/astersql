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
fn postgres_client_workflow() {
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
        .output();
    // The authenticated MySQL connection stays usable after the PG workload.
    mysql_packet(&mut mysql, 0, &[0x0e]); // COM_PING
    assert_eq!(mysql_read(&mut mysql).first(), Some(&0));
    mysql_packet(&mut mysql, 0, &[0x01]); // COM_QUIT
    server.close();
    let output = result.expect("Python 3 is required to call the external libpq client");
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
execute = api('PQexec', ptr, ptr, text)
params = api('PQexecParams', ptr, ptr, text, integer, c.POINTER(c.c_uint), c.POINTER(text), c.POINTER(integer), c.POINTER(integer), integer)
result_status = api('PQresultStatus', integer, ptr)
result_error = api('PQresultErrorMessage', text, ptr)
rows = api('PQntuples', integer, ptr)
columns = api('PQnfields', integer, ptr)
value = api('PQgetvalue', text, ptr, integer, integer)
clear = api('PQclear', None, ptr)
txn = api('PQtransactionStatus', integer, ptr)
cancel_create = api('PQcancelCreate', ptr, ptr)
cancel_blocking = api('PQcancelBlocking', integer, ptr)
cancel_finish = api('PQcancelFinish', None, ptr)
conninfo = f'host=127.0.0.1 port={sys.argv[1]} user=root dbname=test sslmode=disable gssencmode=disable connect_timeout=5 min_protocol_version=3.2 max_protocol_version=3.2'
conn = connect(conninfo.encode())
assert conn
try:
    assert status(conn) == 0, error(conn).decode()
    assert protocol(conn) == 30002, protocol(conn)
    def query(sql, expected=None, parameter=None):
        if parameter is None:
            result = execute(conn, sql.encode())
        else:
            oids = (c.c_uint * 1)(23)
            values = (text * 1)(str(parameter).encode())
            result = params(conn, sql.encode(), 1, oids, values, None, None, 0)
        assert result
        try:
            assert result_status(result) in (1, 2), result_error(result).decode()
            if expected is not None:
                actual = [[value(result, r, col).decode() for col in range(columns(result))] for r in range(rows(result))]
                assert actual == expected, (sql, actual, expected)
        finally:
            clear(result)
    for invalid_info in [conninfo.replace('user=root', 'user=intruder'), conninfo.replace('3.2', '3.0')]:
        invalid = connect(invalid_info.encode())
        try:
            assert status(invalid) != 0, 'invalid identity/version must be rejected'
        finally:
            finish(invalid)
    query('SELECT 1', [['1']])
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
    # libpq owns the variable-length 3.2 BackendKeyData/CancelRequest framing.
    # An idle cancel must complete without poisoning the next query.
    cancel = cancel_create(conn)
    assert cancel
    try:
        assert cancel_blocking(cancel) == 1
    finally:
        cancel_finish(cancel)
    query('SELECT 1', [['1']])
    query('DROP TABLE pg_real_client')
    print('3.2 startup, CRUD, typed parameters, transactions and idle cancel passed')
finally:
    finish(conn)
"#;
