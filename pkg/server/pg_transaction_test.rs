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

fn connect(addr: std::net::SocketAddr) -> TcpStream {
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    socket
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

fn error_state(body: &[u8]) -> Option<&str> {
    body.split(|byte| *byte == 0)
        .find_map(|field| field.strip_prefix(b"C"))
        .and_then(|state| std::str::from_utf8(state).ok())
}

fn row_count(messages: &[(u8, Vec<u8>)]) -> usize {
    messages.iter().filter(|message| message.0 == b'D').count()
}

fn parse(socket: &mut TcpStream, name: &str, sql: &str, oids: &[u32]) {
    let mut body = [name.as_bytes(), b"\0", sql.as_bytes(), b"\0"].concat();
    body.extend_from_slice(&(oids.len() as i16).to_be_bytes());
    for oid in oids {
        body.extend_from_slice(&oid.to_be_bytes());
    }
    send(socket, b'P', &body);
}

fn bind_int(socket: &mut TcpStream, portal: &str, statement: &str, value: i32) {
    let bytes = value.to_string();
    let mut body = [portal.as_bytes(), b"\0", statement.as_bytes(), b"\0"].concat();
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&1i16.to_be_bytes());
    body.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
    body.extend_from_slice(bytes.as_bytes());
    body.extend_from_slice(&0i16.to_be_bytes());
    send(socket, b'B', &body);
}

fn execute(socket: &mut TcpStream, portal: &str) {
    send(
        socket,
        b'E',
        &[portal.as_bytes(), b"\0", &0u32.to_be_bytes()].concat(),
    );
}

fn sync(socket: &mut TcpStream, status: u8) {
    send(socket, b'S', &[]);
    assert_eq!(read(socket), (b'Z', vec![status]));
}

fn start() -> (Arc<PgService>, std::net::SocketAddr) {
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
    (service, addr)
}

#[test]
fn failed_transaction_requires_rollback() {
    let (service, addr) = start();
    let mut socket = connect(addr);
    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE public.pg_tx_failed (id INT PRIMARY KEY)"
        )[0]
        .0,
        b'C'
    );
    assert_eq!(
        query(&mut socket, "BEGIN").last(),
        Some(&(b'Z', b"T".to_vec()))
    );
    assert_eq!(
        query(&mut socket, "INSERT INTO public.pg_tx_failed VALUES (1)")[0].0,
        b'C'
    );

    let failure = query(&mut socket, "INSERT INTO public.pg_tx_failed VALUES (1)");
    assert_eq!(failure[0].0, b'E');
    assert_eq!(failure.last(), Some(&(b'Z', b"E".to_vec())));

    for sql in ["SELECT 1", "COMMIT"] {
        let blocked = query(&mut socket, sql);
        assert_eq!(blocked[0].0, b'E', "{sql}: {blocked:?}");
        assert_eq!(error_state(&blocked[0].1), Some("25P02"), "{sql}");
        assert_eq!(blocked.last(), Some(&(b'Z', b"E".to_vec())));
    }

    assert_eq!(
        query(&mut socket, "ROLLBACK").last(),
        Some(&(b'Z', b"I".to_vec()))
    );
    assert_eq!(
        row_count(&query(&mut socket, "SELECT id FROM public.pg_tx_failed")),
        0
    );
    assert_eq!(
        query(&mut socket, "SELECT 1").last(),
        Some(&(b'Z', b"I".to_vec()))
    );

    assert_eq!(
        query(&mut socket, "BEGIN").last(),
        Some(&(b'Z', b"T".to_vec()))
    );
    let ddl = query(&mut socket, "CREATE TABLE public.pg_tx_ddl (id INT)");
    assert_eq!(error_state(&ddl[0].1), Some("0A000"));
    assert_eq!(ddl.last(), Some(&(b'Z', b"E".to_vec())));
    assert_eq!(
        query(&mut socket, "ROLLBACK").last(),
        Some(&(b'Z', b"I".to_vec()))
    );

    parse(
        &mut socket,
        "duplicate",
        "INSERT INTO public.pg_tx_failed VALUES ($1)",
        &[23],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    assert_eq!(
        query(&mut socket, "BEGIN").last(),
        Some(&(b'Z', b"T".to_vec()))
    );
    for portal in ["first", "duplicate"] {
        bind_int(&mut socket, portal, "duplicate", 2);
        assert_eq!(read(&mut socket), (b'2', vec![]));
        execute(&mut socket, portal);
        let response = read(&mut socket);
        if portal == "first" {
            assert_eq!(response, (b'C', b"INSERT 0 1\0".to_vec()));
            sync(&mut socket, b'T');
        } else {
            assert_eq!(response.0, b'E');
            assert_eq!(error_state(&response.1), Some("23505"));
            sync(&mut socket, b'E');
        }
    }
    parse(&mut socket, "blocked", "SELECT 1", &[]);
    let blocked = read(&mut socket);
    assert_eq!(blocked.0, b'E');
    assert_eq!(error_state(&blocked.1), Some("25P02"));
    sync(&mut socket, b'E');
    assert_eq!(
        query(&mut socket, "ROLLBACK").last(),
        Some(&(b'Z', b"I".to_vec()))
    );
    assert_eq!(
        row_count(&query(&mut socket, "SELECT id FROM public.pg_tx_failed")),
        0
    );
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn prepared_dml_commit_and_rollback() {
    let (service, addr) = start();
    let mut writer = connect(addr);
    let mut observer = connect(addr);
    assert_eq!(
        query(
            &mut writer,
            "CREATE TABLE public.pg_tx_prepared (id INT PRIMARY KEY)"
        )[0]
        .0,
        b'C'
    );
    parse(
        &mut writer,
        "insert",
        "INSERT INTO public.pg_tx_prepared VALUES ($1)",
        &[23],
    );
    assert_eq!(read(&mut writer), (b'1', vec![]));

    assert_eq!(
        query(&mut writer, "BEGIN").last(),
        Some(&(b'Z', b"T".to_vec()))
    );
    bind_int(&mut writer, "p1", "insert", 1);
    assert_eq!(read(&mut writer), (b'2', vec![]));
    execute(&mut writer, "p1");
    assert_eq!(read(&mut writer), (b'C', b"INSERT 0 1\0".to_vec()));
    sync(&mut writer, b'T');
    assert_eq!(
        row_count(&query(
            &mut observer,
            "SELECT id FROM public.pg_tx_prepared"
        )),
        0
    );
    assert_eq!(
        query(&mut writer, "COMMIT").last(),
        Some(&(b'Z', b"I".to_vec()))
    );
    assert_eq!(
        row_count(&query(
            &mut observer,
            "SELECT id FROM public.pg_tx_prepared"
        )),
        1
    );

    assert_eq!(
        query(&mut writer, "BEGIN").last(),
        Some(&(b'Z', b"T".to_vec()))
    );
    bind_int(&mut writer, "p2", "insert", 2);
    assert_eq!(read(&mut writer), (b'2', vec![]));
    execute(&mut writer, "p2");
    assert_eq!(read(&mut writer), (b'C', b"INSERT 0 1\0".to_vec()));
    sync(&mut writer, b'T');
    assert_eq!(
        query(&mut writer, "ROLLBACK").last(),
        Some(&(b'Z', b"I".to_vec()))
    );
    assert_eq!(
        row_count(&query(
            &mut observer,
            "SELECT id FROM public.pg_tx_prepared"
        )),
        1
    );

    assert_eq!(
        query(&mut writer, "BEGIN").last(),
        Some(&(b'Z', b"T".to_vec()))
    );
    assert_eq!(
        query(&mut writer, "INSERT INTO public.pg_tx_prepared VALUES (3)")[0].0,
        b'C'
    );
    send(&mut writer, b'X', &[]);
    drop(writer);
    assert_eq!(
        row_count(&query(
            &mut observer,
            "SELECT id FROM public.pg_tx_prepared"
        )),
        1
    );
    send(&mut observer, b'X', &[]);
    service.close();
}
