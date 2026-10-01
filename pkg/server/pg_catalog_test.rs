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
fn columns(body: &[u8]) -> Vec<(String, u32)> {
    let count = i16::from_be_bytes(body[..2].try_into().unwrap());
    let mut offset = 2;
    (0..count)
        .map(|_| {
            let end = offset + body[offset..].iter().position(|b| *b == 0).unwrap();
            let name = String::from_utf8(body[offset..end].to_vec()).unwrap();
            offset = end + 1;
            let oid = u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap());
            offset += 18;
            (name, oid)
        })
        .collect()
}
fn until_ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
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
#[test]
fn catalog_projection_variants() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        Arc::new(ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::InsecureRootOnly,
        )),
        Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        false,
    )
    .unwrap();
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = [196608u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    assert_eq!(
        query(&mut socket, "CREATE DATABASE catalog_projection_live")[0].0,
        b'C'
    );
    let response = query(
        &mut socket,
        "/* projection */ SELECT datname AS \"Database Name\", N.oid::bigint AS id, NULL AS absent FROM pg_catalog.pg_database N ORDER BY N.oid LIMIT 100",
    );
    assert_eq!(response[0].0, b'T', "{response:?}");
    assert!(response[0].1[2..].starts_with(b"Database Name\0"));
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "catalog_projection_live")
        .unwrap();
    assert!(response.iter().any(|m| *m
        == (
            b'D',
            row(&[
                Some("catalog_projection_live"),
                Some(&schema.id.to_string()),
                None
            ])
        )));
    assert_eq!(
        columns(&response[0].1),
        vec![
            ("Database Name".into(), 25),
            ("id".into(), 20),
            ("absent".into(), 25)
        ]
    );
    let expected_id = schema.id.to_string();
    for sql in [
        "SELECT datname FROM pg_catalog.pg_database WHERE datname = 'catalog_projection_live'",
        "SELECT datname AS name FROM pg_catalog.pg_database WHERE datname = 'catalog_projection_live' ORDER BY name",
        "SELECT datname FROM pg_catalog.pg_database WHERE oid IS NOT NULL ORDER BY CASE WHEN datname = 'catalog_projection_live' THEN -1::bigint ELSE oid::bigint END LIMIT 1",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {result:?}"
        );
        assert_eq!(result[1], (b'D', row(&[Some("catalog_projection_live")])));
    }
    let cast = query(
        &mut socket,
        "SELECT oid::varchar AS text_id, oid::varchar::bigint AS id FROM pg_catalog.pg_database WHERE datname = 'catalog_projection_live'",
    );
    assert_eq!(
        columns(&cast[0].1),
        vec![("text_id".into(), 25), ("id".into(), 20)]
    );
    assert_eq!(
        cast[1],
        (b'D', row(&[Some(&expected_id), Some(&expected_id)]))
    );
    let empty = query(
        &mut socket,
        "SELECT oid FROM pg_catalog.pg_database LIMIT 0",
    );
    assert_eq!(empty.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    for (sql, state) in [
        (
            "SELECT oid FROM pg_catalog.pg_database UNION SELECT 1",
            b"0A000",
        ),
        ("SELECT oid FROM pg_catalog.pg_database; SELECT 1", b"0A000"),
        ("SELECT unknown FROM pg_catalog.pg_database", b"0A000"),
        ("SELECT oid, FROM pg_catalog.pg_database", b"42601"),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"EZ",
            "{sql}"
        );
        assert!(result[0].1.windows(5).any(|bytes| bytes == state));
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    }
    // A named catalog statement keeps its structure but observes metadata
    // created after Parse when its portal is first executed.
    let sql = "SELECT datname AS \"Latest Name\", oid::bigint AS id FROM pg_catalog.pg_database ORDER BY oid DESC LIMIT 1";
    send(
        &mut socket,
        b'P',
        &[
            b"catalog_variant\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scatalog_variant\0");
    send(&mut socket, b'S', b"");
    let parsed = until_ready(&mut socket);
    assert_eq!(parsed.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ");
    assert_eq!(
        columns(&parsed[2].1),
        vec![("Latest Name".into(), 25), ("id".into(), 20)]
    );
    assert_eq!(
        query(&mut socket, "CREATE DATABASE catalog_after_parse")[0].0,
        b'C'
    );
    let latest = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "catalog_after_parse")
        .unwrap();
    send(
        &mut socket,
        b'B',
        &[
            b"catalog_portal\0catalog_variant\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"catalog_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let executed = until_ready(&mut socket);
    assert_eq!(
        executed.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"2DCZ",
        "{executed:?}"
    );
    assert_eq!(
        executed[1],
        (
            b'D',
            row(&[Some("catalog_after_parse"), Some(&latest.id.to_string())])
        )
    );
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn catalog_database_description_semantics() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        Arc::new(ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::InsecureRootOnly,
        )),
        Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        false,
    )
    .unwrap();
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = [196608u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    for sql in [
        "SELECT objoid, classoid, objsubid, description FROM pg_catalog.pg_description WHERE classoid = 2615 AND objsubid = 0",
        "SELECT objoid, classoid, description FROM pg_catalog.pg_shdescription WHERE objoid = 1 AND classoid = 1262",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TCZ",
            "{sql}: {result:?}"
        );
    }
    assert_eq!(
        query(&mut socket, "CREATE DATABASE catalog_description_live")[0].0,
        b'C'
    );
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "catalog_description_live")
        .unwrap();
    let sql = "SELECT N.oid::bigint id, datname, D.description, datistemplate, datallowconn, pg_get_userbyid(datdba) owner, current_database() db, current_catalog FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_shdescription D ON N.oid = D.objoid AND D.classoid = 1262 WHERE datname = 'catalog_description_live'";
    let result = query(&mut socket, sql);
    assert_eq!(
        result.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ",
        "{result:?}"
    );
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("id".into(), 20),
            ("datname".into(), 25),
            ("description".into(), 25),
            ("datistemplate".into(), 16),
            ("datallowconn".into(), 16),
            ("owner".into(), 25),
            ("db".into(), 25),
            ("current_catalog".into(), 25)
        ]
    );
    assert_eq!(
        result[1],
        (
            b'D',
            row(&[
                Some(&schema.id.to_string()),
                Some("catalog_description_live"),
                None,
                Some("f"),
                Some("t"),
                None,
                Some("test"),
                Some("test")
            ])
        )
    );
    let namespace = query(
        &mut socket,
        "SELECT nspname, D.description FROM pg_catalog.pg_namespace N LEFT JOIN pg_catalog.pg_description D ON N.oid = D.objoid AND D.classoid = 2615 AND D.objsubid = 0 WHERE nspname = 'catalog_description_live'",
    );
    assert_eq!(
        namespace.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ",
        "{namespace:?}"
    );
    assert_eq!(
        namespace[1],
        (b'D', row(&[Some("catalog_description_live"), None]))
    );
    let description_columns = query(
        &mut socket,
        "SELECT objoid, classoid, objsubid, description FROM pg_catalog.pg_description",
    );
    assert_eq!(
        columns(&description_columns[0].1),
        vec![
            ("objoid".into(), 20),
            ("classoid".into(), 20),
            ("objsubid".into(), 23),
            ("description".into(), 25)
        ]
    );
    let unknown_owner = query(
        &mut socket,
        "SELECT pg_get_userbyid(123) FROM pg_catalog.pg_database LIMIT 1",
    );
    assert_eq!(unknown_owner[1], (b'D', row(&[None])));
    let mut selected = TcpStream::connect(addr).unwrap();
    selected
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0catalog_description_live\0\0",
    ]
    .concat();
    selected
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    selected.write_all(&startup).unwrap();
    while read(&mut selected).0 != b'Z' {}
    let current = query(
        &mut selected,
        "SELECT current_database(), current_catalog FROM pg_catalog.pg_database LIMIT 1",
    );
    assert_eq!(
        current[1],
        (
            b'D',
            row(&[
                Some("catalog_description_live"),
                Some("catalog_description_live")
            ])
        )
    );
    send(&mut selected, b'X', b"");
    let filtered = query(
        &mut socket,
        "SELECT datname FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_shdescription D ON N.oid = D.objoid WHERE D.classoid = 1262",
    );
    assert_eq!(filtered.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    // Parse records the query structure; first execution reads the latest snapshot.
    let live_sql = "SELECT datname, D.description FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_shdescription D ON N.oid = D.objoid AND D.classoid = 1262 ORDER BY datname";
    send(
        &mut socket,
        b'P',
        &[
            b"description_live\0".as_slice(),
            live_sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', b"");
    assert_eq!(
        until_ready(&mut socket)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"1Z"
    );
    assert_eq!(
        query(&mut socket, "DROP DATABASE catalog_description_live")[0].0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "CREATE DATABASE catalog_description_after_parse"
        )[0]
        .0,
        b'C'
    );
    send(
        &mut socket,
        b'B',
        &[
            b"description_portal\0description_live\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"description_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let executed = until_ready(&mut socket);
    assert_eq!(executed.first().unwrap().0, b'2');
    assert!(
        executed
            .iter()
            .any(|m| *m == (b'D', row(&[Some("catalog_description_after_parse"), None])))
    );
    assert!(
        !executed
            .iter()
            .any(|m| *m == (b'D', row(&[Some("catalog_description_live"), None])))
    );
    assert_eq!(
        executed
            .iter()
            .rev()
            .take(2)
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"ZC"
    );
    let invalid = query(
        &mut socket,
        "SELECT objsubid FROM pg_catalog.pg_shdescription",
    );
    assert_eq!(invalid.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    send(&mut socket, b'X', b"");
    service.close();
}
