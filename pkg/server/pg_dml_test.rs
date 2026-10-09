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

fn simple_query(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
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

fn parse(socket: &mut TcpStream, name: &str, sql: &str, oids: &[u32]) {
    let mut body = [name.as_bytes(), b"\0", sql.as_bytes(), b"\0"].concat();
    body.extend_from_slice(&(oids.len() as i16).to_be_bytes());
    for oid in oids {
        body.extend_from_slice(&oid.to_be_bytes());
    }
    send(socket, b'P', &body);
}

fn bind(
    socket: &mut TcpStream,
    portal: &str,
    statement: &str,
    formats: &[i16],
    values: &[Option<&[u8]>],
    result_formats: &[i16],
) {
    let mut body = [portal.as_bytes(), b"\0", statement.as_bytes(), b"\0"].concat();
    body.extend_from_slice(&(formats.len() as i16).to_be_bytes());
    for format in formats {
        body.extend_from_slice(&format.to_be_bytes());
    }
    body.extend_from_slice(&(values.len() as i16).to_be_bytes());
    for value in values {
        match value {
            Some(value) => {
                body.extend_from_slice(&(value.len() as i32).to_be_bytes());
                body.extend_from_slice(value);
            }
            None => body.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }
    body.extend_from_slice(&(result_formats.len() as i16).to_be_bytes());
    for format in result_formats {
        body.extend_from_slice(&format.to_be_bytes());
    }
    send(socket, b'B', &body);
}

fn execute(socket: &mut TcpStream, portal: &str) {
    let mut body = [portal.as_bytes(), b"\0"].concat();
    body.extend_from_slice(&0u32.to_be_bytes());
    send(socket, b'E', &body);
}

fn sync(socket: &mut TcpStream) {
    send(socket, b'S', &[]);
    assert_eq!(read(socket), (b'Z', b"I".to_vec()));
}

fn error_state(body: &[u8]) -> Option<&str> {
    body.split(|byte| *byte == 0)
        .find_map(|field| field.strip_prefix(b"C"))
        .and_then(|state| std::str::from_utf8(state).ok())
}

fn text_row(values: &[Option<&str>]) -> Vec<u8> {
    let mut body = (values.len() as i16).to_be_bytes().to_vec();
    for value in values {
        match value {
            Some(value) => {
                body.extend_from_slice(&(value.len() as i32).to_be_bytes());
                body.extend_from_slice(value.as_bytes());
            }
            None => body.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }
    body
}

#[test]
fn prepared_crud_roundtrip() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let _service = PgService::start(
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
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    assert_eq!(read(&mut socket).0, b'R');
    while read(&mut socket).0 != b'Z' {}

    assert_eq!(
        simple_query(&mut socket, "CREATE DATABASE pg_dml")[0].0,
        b'C'
    );
    let create = simple_query(
        &mut socket,
        "CREATE TABLE public.prepared_crud (id integer PRIMARY KEY, label varchar(30), note varchar(30))",
    );
    assert_eq!(create[0].0, b'C', "{create:?}");

    // Out-of-order markers and mixed text/binary/NULL formats must be mapped once.
    parse(
        &mut socket,
        "insert",
        "INSERT INTO public.prepared_crud (id, label, note) VALUES ($2, $1, $3)",
        &[1043, 23, 1043],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(
        &mut socket,
        "insert_portal",
        "insert",
        &[0, 1, 0],
        &[Some(b"one"), Some(&1i32.to_be_bytes()), None],
        &[],
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    execute(&mut socket, "insert_portal");
    assert_eq!(read(&mut socket), (b'C', b"INSERT 0 1\0".to_vec()));
    sync(&mut socket);

    // Repeated markers bind the same value at each engine marker occurrence.
    parse(
        &mut socket,
        "select",
        "SELECT id, label, note FROM public.prepared_crud WHERE id = $1 AND $2 = $2",
        &[23, 1043],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(
        &mut socket,
        "select_portal",
        "select",
        &[1, 0],
        &[Some(&1i32.to_be_bytes()), Some(b"same")],
        &[],
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    execute(&mut socket, "select_portal");
    assert_eq!(
        read(&mut socket),
        (b'D', text_row(&[Some("1"), Some("one"), None]))
    );
    assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
    sync(&mut socket);

    parse(
        &mut socket,
        "update",
        "UPDATE public.prepared_crud SET label = $1, note = $2 WHERE id = $3",
        &[1043, 1043, 23],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(
        &mut socket,
        "update_portal",
        "update",
        &[0, 0, 1],
        &[Some(b"two"), Some(b"updated"), Some(&1i32.to_be_bytes())],
        &[],
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    execute(&mut socket, "update_portal");
    assert_eq!(read(&mut socket), (b'C', b"UPDATE 1\0".to_vec()));
    sync(&mut socket);

    // A bad Bind enters the extended-protocol error state until Sync.
    parse(
        &mut socket,
        "bad_delete",
        "DELETE FROM public.prepared_crud WHERE id = $1",
        &[23],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(
        &mut socket,
        "bad_delete_portal",
        "bad_delete",
        &[],
        &[Some(b"not-an-integer")],
        &[],
    );
    let error = read(&mut socket);
    assert_eq!(error.0, b'E');
    assert_eq!(error_state(&error.1), Some("22P02"));
    execute(&mut socket, "bad_delete_portal");
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));

    parse(
        &mut socket,
        "delete",
        "DELETE FROM public.prepared_crud WHERE id = $1",
        &[23],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(
        &mut socket,
        "delete_portal",
        "delete",
        &[1],
        &[Some(&1i32.to_be_bytes())],
        &[],
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    execute(&mut socket, "delete_portal");
    assert_eq!(read(&mut socket), (b'C', b"DELETE 1\0".to_vec()));
    sync(&mut socket);

    // Unsupported RETURNING must fail during Parse, before a write can run.
    parse(
        &mut socket,
        "returning",
        "INSERT INTO public.prepared_crud VALUES ($1, $2, NULL) RETURNING id",
        &[23, 1043],
    );
    let error = read(&mut socket);
    assert_eq!(error.0, b'E');
    assert_eq!(error_state(&error.1), Some("0A000"));
    sync(&mut socket);
    for (name, sql) in [
        (
            "update_returning",
            "UPDATE public.prepared_crud SET label = $1 WHERE id = $2 RETURNING id",
        ),
        (
            "delete_returning",
            "DELETE FROM public.prepared_crud WHERE id = $1 RETURNING id",
        ),
    ] {
        let oids: &[u32] = if name == "update_returning" {
            &[1043, 23]
        } else {
            &[23]
        };
        parse(&mut socket, name, sql, oids);
        let error = read(&mut socket);
        assert_eq!(error.0, b'E');
        assert_eq!(error_state(&error.1), Some("0A000"));
        sync(&mut socket);
    }
    let rows = simple_query(&mut socket, "SELECT id FROM public.prepared_crud");
    assert_eq!(
        rows.iter().map(|message| message.0).collect::<Vec<_>>(),
        b"TCZ"
    );
}
