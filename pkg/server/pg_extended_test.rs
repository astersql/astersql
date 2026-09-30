// Copyright 2026 AsterSQL.
use crate::pg_conn::PgService;
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

fn read(socket: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0; 5];
    socket.read_exact(&mut header).unwrap();
    let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    assert!((4..1 << 20).contains(&length));
    let mut body = vec![0; length - 4];
    socket.read_exact(&mut body).unwrap();
    (header[0], body)
}
fn send(socket: &mut TcpStream, tag: u8, body: &[u8]) {
    socket.write_all(&[tag]).unwrap();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(body).unwrap();
}
fn query(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
    send(socket, b'Q', &[sql.as_bytes(), b"\0"].concat());
    let mut messages = Vec::new();
    loop {
        let message = read(socket);
        let ready = message.0 == b'Z';
        messages.push(message);
        if ready {
            return messages;
        }
    }
}
fn row(values: &[Option<&str>]) -> Vec<u8> {
    let mut body = (values.len() as i16).to_be_bytes().to_vec();
    for value in values {
        match value {
            None => body.extend_from_slice(&(-1i32).to_be_bytes()),
            Some(text) => {
                body.extend_from_slice(&(text.len() as i32).to_be_bytes());
                body.extend_from_slice(text.as_bytes());
            }
        }
    }
    body
}
#[test]
fn parse_bind_execute_sync() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    assert_eq!(read(&mut socket).0, b'R');
    while read(&mut socket).0 != b'Z' {}
    parse(&mut socket, "catalog", "SELECT current_catalog", &[]);
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(&mut socket, "", "catalog", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"P\0");
    let description = read(&mut socket);
    assert_eq!(description.0, b'T');
    assert!(description.1[2..].starts_with(b"current_catalog\0"));
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("test")])));
    assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    let startup_sql = "select round(extract(epoch from pg_postmaster_start_time() at time zone 'UTC')) as startup_time;";
    let startup = query(&mut socket, startup_sql);
    assert_eq!(
        startup.iter().map(|message| message.0).collect::<Vec<_>>(),
        b"TDCZ"
    );
    parse(&mut socket, "startup", startup_sql, &[]);
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(&mut socket, "", "startup", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"P\0");
    assert_eq!(read(&mut socket), startup[0]);
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), startup[1]);
    assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    // Fixed table-column metadata keeps Describe independent of parameter values.
    assert_eq!(query(&mut socket, "CREATE DATABASE pg_extended")[0].0, b'C');
    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE pg_extended.t (id INT, v VARCHAR(30))"
        )[0]
        .0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "INSERT INTO pg_extended.t VALUES (1, 'one'), (2, 'two')"
        )[0]
        .0,
        b'C'
    );
    parse(
        &mut socket,
        "databases",
        crate::pg_catalog::DATABASES_SQL,
        &[],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    assert_eq!(
        query(&mut socket, "CREATE DATABASE pg_catalog_after_parse")[0].0,
        b'C'
    );
    bind(&mut socket, "db_portal", "databases", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"Pdb_portal\0");
    let catalog_description = read(&mut socket);
    assert_eq!(catalog_description.0, b'T');
    execute(&mut socket, "db_portal", 0);
    let mut found = false;
    loop {
        let (tag, body) = read(&mut socket);
        if tag == b'C' {
            break;
        }
        assert_eq!(tag, b'D');
        found |= body
            .windows(b"pg_catalog_after_parse".len())
            .any(|bytes| bytes == b"pg_catalog_after_parse");
    }
    assert!(
        found,
        "prepared catalog execution must observe databases created after Parse"
    );
    parse(
        &mut socket,
        "locks",
        crate::pg_catalog::TRANSACTIONS_SQL,
        &[],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(&mut socket, "lock_portal", "locks", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    // Both PG catalog statements lack engine handles; closing one must not
    // destroy the other's portal or close an unrelated engine statement.
    send(&mut socket, b'C', b"Sdatabases\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    execute(&mut socket, "lock_portal", 0);
    assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
    send(&mut socket, b'C', b"Slocks\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    parse(
        &mut socket,
        "s",
        "SELECT id, v FROM pg_extended.t WHERE id = $1",
        &[23],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    send(&mut socket, b'D', b"Ss\0");
    let params = read(&mut socket);
    assert_eq!(
        params,
        (
            b't',
            [1i16.to_be_bytes().as_slice(), &23u32.to_be_bytes()].concat()
        )
    );
    assert_eq!(read(&mut socket).0, b'T');
    for (value, expected) in [
        (Some("1"), Some("one")),
        (Some("2"), Some("two")),
        (None, None),
    ] {
        bind(&mut socket, "", "s", &[value]);
        assert_eq!(read(&mut socket), (b'2', vec![]));
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket).0, b'T');
        execute(&mut socket, "", 0);
        if let Some(expected) = expected {
            assert_eq!(read(&mut socket), (b'D', row(&[value, Some(expected)])));
            assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
        } else {
            assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
        }
        send(&mut socket, b'S', b"");
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    }
    // An unnamed statement uses the same binding and object lifecycle.
    parse(
        &mut socket,
        "",
        "SELECT id FROM pg_extended.t ORDER BY id",
        &[],
    );
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "pages", "", &[]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "pages", 1);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("1")])));
    assert_eq!(read(&mut socket), (b's', vec![]));
    execute(&mut socket, "pages", 1);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("2")])));
    assert_eq!(read(&mut socket), (b'C', b"SELECT 2\0".to_vec()));
    send(&mut socket, b'C', b"Ppages\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    send(&mut socket, b'C', b"Ss\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    bind(&mut socket, "", "s", &[Some("1")]);
    let missing = read(&mut socket);
    assert_eq!(missing.0, b'E');
    assert!(missing.1.windows(5).any(|w| w == b"26000"));
    // Ignored Parse after error must not register this name.
    parse(&mut socket, "ignored", "SELECT 1", &[]);
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    send(&mut socket, b'D', b"Signored\0");
    assert_eq!(read(&mut socket).0, b'E');
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    // Parameter occurrence order is independent of indexed client order.
    parse(
        &mut socket,
        "repeat",
        "SELECT id FROM pg_extended.t WHERE id = $2 OR id = $1 OR id = $2",
        &[23, 23],
    );
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "", "repeat", &[Some("1"), Some("2")]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket).0, b'D');
    assert_eq!(read(&mut socket).0, b'D');
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    // Text is bound as data; quoting cannot turn a value into SQL.
    parse(
        &mut socket,
        "update",
        "UPDATE pg_extended.t SET v = $2 WHERE id = $1",
        &[23, 25],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for value in [Some("' OR 'x'='x"), None] {
        bind(&mut socket, "", "update", &[Some("1"), value]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'C', b"UPDATE 1\0".to_vec()));
        send(&mut socket, b'S', b"");
        assert_eq!(read(&mut socket).0, b'Z');
        assert_eq!(
            query(&mut socket, "SELECT v FROM pg_extended.t WHERE id = 1")[1],
            (b'D', row(&[value]))
        );
    }
    parse(
        &mut socket,
        "inferred",
        "SELECT id FROM pg_extended.t WHERE id = $1",
        &[],
    );
    let unsupported = read(&mut socket);
    assert_eq!(unsupported.0, b'E');
    assert!(unsupported.1.windows(5).any(|w| w == b"0A000"));
    // Simple Query is also discarded until Sync after an extended error.
    send(&mut socket, b'Q', b"DELETE FROM pg_extended.t\0");
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(
        query(&mut socket, "SELECT id FROM pg_extended.t")
            .iter()
            .filter(|m| m.0 == b'D')
            .count(),
        2
    );
    parse(&mut socket, "bool", "SELECT TRUE", &[]);
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "", "bool", &[]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("t")])));
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    send(&mut socket, b'X', b"");
    service.close();
}

fn parse(socket: &mut TcpStream, name: &str, sql: &str, oids: &[u32]) {
    let mut body = [
        name.as_bytes(),
        b"\0",
        sql.as_bytes(),
        b"\0",
        &(oids.len() as i16).to_be_bytes(),
    ]
    .concat();
    for oid in oids {
        body.extend_from_slice(&oid.to_be_bytes());
    }
    send(socket, b'P', &body);
}
fn bind(socket: &mut TcpStream, portal: &str, statement: &str, values: &[Option<&str>]) {
    let mut body = [
        portal.as_bytes(),
        b"\0",
        statement.as_bytes(),
        b"\0",
        &0i16.to_be_bytes(),
        &(values.len() as i16).to_be_bytes(),
    ]
    .concat();
    for value in values {
        match value {
            Some(value) => {
                body.extend_from_slice(&(value.len() as i32).to_be_bytes());
                body.extend_from_slice(value.as_bytes());
            }
            None => body.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }
    body.extend_from_slice(&0i16.to_be_bytes());
    send(socket, b'B', &body);
}
fn execute(socket: &mut TcpStream, portal: &str, limit: u32) {
    send(
        socket,
        b'E',
        &[portal.as_bytes(), b"\0", &limit.to_be_bytes()].concat(),
    );
}

#[test]
fn marker_mapping_preserves_literals_comments_and_index_order() {
    let sql = "SELECT '$1', \"$2\", `x$3`, $2, $1, $2 /* $4 */ -- $5\n";
    let (adapted, mapping) = crate::pg_extended::markers(sql).unwrap();
    assert_eq!(
        adapted,
        "SELECT '$1', \"$2\", `x$3`, ?, ?, ? /* $4 */ -- $5\n"
    );
    assert_eq!(mapping, [1, 0, 1]);
    for invalid in [
        "SELECT $0",
        "SELECT $32768",
        "SELECT $tag$x$tag$",
        "SELECT ?",
        "SELECT /* unterminated",
        "SELECT x$1",
        "SELECT $1x",
        "SELECT /*! $1 */",
    ] {
        assert!(crate::pg_extended::markers(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn text_parameters_are_typed_and_reject_loss() {
    use crate::pg_extended::parameter;
    assert_eq!(
        parameter(23, Some(b"-2147483648")).unwrap().value,
        i32::MIN.to_le_bytes()
    );
    assert!(parameter(23, Some(b"2147483648")).is_err());
    assert_eq!(parameter(16, Some(b"true")).unwrap().value, [1]);
    assert!(parameter(16, Some(b"unknown")).is_err());
    assert_eq!(parameter(17, Some(b"\\x00ff")).unwrap().value, [0, 255]);
    assert!(parameter(17, Some(b"\\xabc")).is_err());
    assert!(parameter(25, Some(b"a\0b")).is_err());
    assert!(parameter(25, Some(&[255])).is_err());
    assert!(parameter(1082, Some(b"2026-02-29")).is_err());
    assert!(parameter(1082, Some(b"2024-02-29")).is_ok());
    assert!(parameter(1083, Some(b"24:00:00")).is_err());
    assert!(parameter(1114, Some(b"2026-09-30 12:30:00.123456")).is_ok());
    assert!(parameter(1114, Some(b"2026-09-30 12:30:00.1234567")).is_err());
    assert!(parameter(1700, Some(b"12.340")).is_ok());
    assert!(parameter(1700, Some(b"12bad")).is_err());
    assert!(parameter(23, None).unwrap().is_null);
}

#[test]
fn canonical_prepare_parameter_syntax_probe() {
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(91006, 0, 45, "", None).unwrap();
    let marker = context
        .prepare_statement("SELECT ?", &CancellationToken::new())
        .unwrap();
    assert_eq!(marker.parameter_count, 1);
    let result = context.prepare_statement("SELECT $1", &CancellationToken::new());
    eprintln!("canonical SELECT $1 preparation: {result:?}");
    assert!(
        result.is_err(),
        "remove this probe if engine supports indexed PG parameters"
    );
    context.close().unwrap();
}
