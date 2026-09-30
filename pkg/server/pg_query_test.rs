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
fn simple_query_roundtrip() {
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
    let result = query(&mut socket, "SELECT 1");
    assert_eq!(result.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
    assert_eq!(result[1].1, row(&[Some("1")]));
    assert_eq!(result[2], (b'C', b"SELECT 1\0".to_vec()));
    assert_eq!(result[3], (b'Z', b"I".to_vec()));
    // Column metadata uses PostgreSQL framing and text format, never MySQL packets.
    assert_eq!(&result[0].1[..2], &1i16.to_be_bytes());
    let name_end = result[0].1[2..].iter().position(|b| *b == 0).unwrap() + 2;
    let metadata = &result[0].1[name_end + 1..];
    assert_eq!(metadata.len(), 18);
    assert_eq!(&metadata[6..10], &20u32.to_be_bytes());
    assert_eq!(&metadata[16..], &0i16.to_be_bytes());
    let nulls = query(&mut socket, "SELECT NULL, ''");
    assert_eq!(nulls[1], (b'D', row(&[None, Some("")])));
    for (sql, completion) in [
        ("CREATE DATABASE pg_query_roundtrip", "CREATE DATABASE"),
        (
            "CREATE TABLE pg_query_roundtrip.t (id INT, value VARCHAR(20))",
            "CREATE TABLE",
        ),
        (
            "INSERT INTO pg_query_roundtrip.t VALUES (1, 'a'), (2, '')",
            "INSERT 0 2",
        ),
        (
            "UPDATE pg_query_roundtrip.t SET value = 'b' WHERE id = 1",
            "UPDATE 1",
        ),
        ("DELETE FROM pg_query_roundtrip.t WHERE id = 2", "DELETE 1"),
    ] {
        assert_eq!(
            query(&mut socket, sql),
            vec![
                (b'C', [completion.as_bytes(), b"\0"].concat()),
                (b'Z', b"I".to_vec())
            ],
            "{sql}"
        );
    }
    let selected = query(&mut socket, "SELECT id, value FROM pg_query_roundtrip.t");
    assert_eq!(selected[1], (b'D', row(&[Some("1"), Some("b")])));
    assert_eq!(
        query(&mut socket, "  ; "),
        vec![(b'I', vec![]), (b'Z', b"I".to_vec())]
    );
    let multiple = query(&mut socket, "DELETE FROM pg_query_roundtrip.t; SELECT 1");
    assert_eq!(multiple.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(multiple[0].1.windows(5).any(|w| w == b"0A000"));
    assert_eq!(
        query(&mut socket, "SELECT id FROM pg_query_roundtrip.t")[1],
        (b'D', row(&[Some("1")]))
    );
    let semicolon = query(&mut socket, "/* query */ SELECT ';'");
    assert_eq!(semicolon[1], (b'D', row(&[Some(";")])));
    assert_eq!(
        query(&mut socket, "SELECT FROM")
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"EZ"
    );
    assert_eq!(
        query(&mut socket, "SELECT * FROM pg_query_roundtrip.missing")
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"EZ"
    );
    send(&mut socket, b'Q', b"SELECT 1");
    let malformed = read(&mut socket);
    assert_eq!(malformed.0, b'E');
    assert!(malformed.1.windows(5).any(|w| w == b"08P01"));
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    assert_eq!(
        query(&mut socket, "BEGIN").last().unwrap(),
        &(b'Z', b"T".to_vec())
    );
    assert_eq!(
        query(&mut socket, "ROLLBACK").last().unwrap(),
        &(b'Z', b"I".to_vec())
    );
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn simple_query_encoding_rejects_unrepresentable_results() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let column = ColumnInfo {
        schema: String::new(),
        table: String::new(),
        org_table: String::new(),
        name: "v".into(),
        org_name: String::new(),
        charset: 45,
        column_length: 0,
        column_type: 0,
        flags: 0,
        decimals: 0,
        default_value: None,
    };
    let mut result = QueryResult {
        columns: vec![column],
        native_types: vec![NativeType {
            code: 253,
            flags: 0,
            length: 0,
            decimal: 0,
        }],
        rows: vec![vec![Value::Text("a\0b".into())]],
        ..QueryResult::default()
    };
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.rows = vec![vec![Value::Bytes(vec![0xff])]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.rows = vec![vec![]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.rows = vec![vec![Value::Null]];
    result.columns[0].name = "a\0b".into();
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    assert_eq!(
        crate::pg_result::command("SELECT 1; SELECT 2")
            .unwrap_err()
            .0,
        "0A000"
    );
    assert_eq!(crate::pg_result::command("/* empty */").unwrap(), None);
    assert_eq!(
        crate::pg_result::command("REPLACE INTO t VALUES (1)")
            .unwrap_err()
            .0,
        "0A000"
    );
}
