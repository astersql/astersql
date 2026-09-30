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
fn sqlstate_unique_syntax_and_recovery() {
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

    for sql in [
        "CREATE DATABASE pg_error_types",
        "CREATE TABLE pg_error_types.t (id INT PRIMARY KEY)",
        "INSERT INTO pg_error_types.t VALUES (1)",
    ] {
        assert_eq!(query(&mut socket, sql)[0].0, b'C');
    }
    for (sql, expected) in [
        ("INSERT INTO pg_error_types.t VALUES (1)", "23505"),
        ("SELECT FROM", "42601"),
        ("SELECT 1; SELECT 2", "0A000"),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
        let marker = [b"C".as_slice(), expected.as_bytes(), b"\0"].concat();
        assert!(
            result[0].1.windows(marker.len()).any(|w| w == marker),
            "{sql}: {:?}",
            String::from_utf8_lossy(&result[0].1)
        );
        assert_eq!(query(&mut socket, "SELECT 1")[1].1, row(&[Some("1")]));
    }
    send(&mut socket, b'X', b"");
    service.close();
}
#[test]
fn unknown_errors_are_not_misclassified() {
    use crate::conn::ConnError;
    assert_eq!(
        crate::pg_conn::sqlstate(&ConnError::Session(
            "application mentions duplicate entry and 23505".into()
        )),
        "XX000"
    );
    assert_eq!(
        crate::pg_conn::sqlstate(&ConnError::UnsupportedCommand(0)),
        "0A000"
    );
}
