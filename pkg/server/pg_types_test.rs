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
fn oids(description: &[u8]) -> Vec<u32> {
    let count = i16::from_be_bytes(description[..2].try_into().unwrap()) as usize;
    let mut offset = 2;
    (0..count)
        .map(|_| {
            offset += description[offset..].iter().position(|b| *b == 0).unwrap() + 1;
            let oid = u32::from_be_bytes(description[offset + 6..offset + 10].try_into().unwrap());
            offset += 18;
            oid
        })
        .collect()
}
#[test]
fn common_type_oids_and_values() {
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

    for (sql, expected_oids, values) in [
        (
            "SELECT 1, -2, TRUE, FALSE, 'hello', NULL",
            vec![20, 20, 16, 16, 25, 25],
            vec![
                Some("1"),
                Some("-2"),
                Some("t"),
                Some("f"),
                Some("hello"),
                None,
            ],
        ),
        (
            "SELECT CAST(12.34 AS DECIMAL(8,2))",
            vec![1700],
            vec![Some("12.34")],
        ),
        (
            "SELECT CAST('2026-09-30' AS DATE)",
            vec![1082],
            vec![Some("2026-09-30")],
        ),
        (
            "SELECT CAST('2026-09-30 12:34:56' AS DATETIME)",
            vec![1114],
            vec![Some("2026-09-30 12:34:56")],
        ),
        (
            "SELECT CAST('12:34:56' AS TIME)",
            vec![1083],
            vec![Some("12:34:56")],
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {result:?}"
        );
        assert_eq!(oids(&result[0].1), expected_oids, "{sql}");
        assert_eq!(result[1].1, row(&values), "{sql}");
    }
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn native_metadata_bridge_preserves_boolean_and_mysql_columns() {
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(75001, 0, 45, "", None).unwrap();
    let integer = context
        .execute_query("SELECT 1 AS v", false, &CancellationToken::new())
        .unwrap()
        .remove(0);
    let boolean = context
        .execute_query("SELECT TRUE AS v", false, &CancellationToken::new())
        .unwrap()
        .remove(0);
    assert_eq!(integer.columns, boolean.columns); // Existing MySQL fallback unchanged.
    assert_eq!(integer.rows, boolean.rows);
    let flag = astersql_parser_mysql::r#type::IsBooleanFlag;
    assert_eq!(integer.native_types[0].flags & flag, 0);
    assert_ne!(boolean.native_types[0].flags & flag, 0);
    context.close().unwrap();
}

#[test]
fn unsigned_binary_and_unsupported_types() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let column = ColumnInfo {
        schema: String::new(),
        table: String::new(),
        org_table: String::new(),
        name: "v".into(),
        org_name: String::new(),
        charset: 63,
        column_length: 0,
        column_type: 8,
        flags: 32,
        decimals: 0,
        default_value: None,
    };
    let mut result = QueryResult {
        columns: vec![column],
        rows: vec![vec![Value::Unsigned(u64::MAX)]],
        native_types: vec![NativeType {
            code: 8,
            flags: 32,
            length: 20,
            decimal: 0,
        }],
        ..QueryResult::default()
    };
    let messages = crate::pg_result::encode(&result, "SELECT").unwrap();
    assert_eq!(oids(&messages[0].1), vec![1700]);
    assert_eq!(messages[1].1, row(&[Some("18446744073709551615")]));
    result.native_types[0].code = 252;
    result.columns[0].column_type = 252;
    result.rows = vec![vec![Value::Bytes(vec![0, 255, 92])]];
    let messages = crate::pg_result::encode(&result, "SELECT").unwrap();
    assert_eq!(oids(&messages[0].1), vec![17]);
    assert_eq!(messages[1].1, row(&[Some("\\x00ff5c")]));
    result.native_types[0].code = 245;
    result.columns[0].column_type = 245; // JSON is outside the first-stage type set.
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    // NULL must not allow an unsupported native JSON type to masquerade as
    // a PG catalog text array before value encoding rejects its contents.
    result.rows = vec![vec![Value::Null]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.native_types[0].code = 11;
    result.columns[0].column_type = 11;
    result.rows = vec![vec![Value::Text("25:00:00".into())]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.native_types[0].code = 10;
    result.columns[0].column_type = 10;
    result.rows = vec![vec![Value::Text("0000-00-00".into())]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.native_types = vec![
        NativeType {
            code: 10,
            flags: 0,
            length: 0,
            decimal: 0
        };
        2
    ];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
}

#[test]
fn missing_original_metadata_is_explicitly_unsupported() {
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(75002, 0, 45, "", None).unwrap();
    let mut result = context
        .execute_query("SELECT 1", false, &CancellationToken::new())
        .unwrap()
        .remove(0);
    result.native_types.clear();
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    context.close().unwrap();
}
