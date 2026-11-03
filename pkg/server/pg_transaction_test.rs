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

// Each extended statement gets a fresh unnamed statement/portal and a Sync.
fn path_request(socket: &mut TcpStream, sql: &str, extended: bool) -> Vec<(u8, Vec<u8>)> {
    if !extended {
        return query(socket, sql);
    }
    parse(socket, "", sql, &[]);
    send(socket, b'B', b"\0\0\0\0\0\0\0\0");
    send(socket, b'D', b"P\0");
    execute(socket, "");
    send(socket, b'S', &[]);
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

fn path_ok(socket: &mut TcpStream, sql: &str, extended: bool, status: u8) -> Vec<(u8, Vec<u8>)> {
    let messages = path_request(socket, sql, extended);
    assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
    assert_eq!(messages.last(), Some(&(b'Z', vec![status])), "{sql}");
    assert!(messages.iter().any(|m| m.0 == b'C'), "{sql}");
    messages
}

fn path_rows(messages: &[(u8, Vec<u8>)]) -> Vec<Vec<Option<String>>> {
    messages
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| {
            let count = i16::from_be_bytes(m.1[..2].try_into().unwrap());
            let mut offset = 2;
            (0..count)
                .map(|_| {
                    let length = i32::from_be_bytes(m.1[offset..offset + 4].try_into().unwrap());
                    offset += 4;
                    if length == -1 {
                        return None;
                    }
                    let value =
                        String::from_utf8(m.1[offset..offset + length as usize].to_vec()).unwrap();
                    offset += length as usize;
                    Some(value)
                })
                .collect()
        })
        .collect()
}

fn path_value(socket: &mut TcpStream, extended: bool, status: u8, expected: &str) {
    let messages = path_ok(socket, "SHOW search_path", extended, status);
    assert_eq!(path_rows(&messages), vec![vec![Some(expected.into())]]);
    let description = &messages.iter().find(|m| m.0 == b'T').unwrap().1;
    assert!(description.starts_with(b"\0\x01search_path\0"));
    let field = &description[14..];
    assert_eq!(u32::from_be_bytes(field[6..10].try_into().unwrap()), 25);
    assert_eq!(i16::from_be_bytes(field[10..12].try_into().unwrap()), -1);
    assert_eq!(i32::from_be_bytes(field[12..16].try_into().unwrap()), -1);
    assert_eq!(i16::from_be_bytes(field[16..18].try_into().unwrap()), 0);
}

#[test]
fn pg11_04() {
    for extended in [false, true] {
        let (service, addr) = start();
        let mut a = connect(addr);
        let mut b = connect(addr);
        path_value(&mut a, extended, b'I', "public");
        path_value(&mut b, extended, b'I', "public");
        path_ok(
            &mut a,
            "CREATE TABLE public.path_doc (id integer PRIMARY KEY, v integer)",
            extended,
            b'I',
        );
        path_ok(
            &mut a,
            "INSERT INTO public.path_doc VALUES (1,10)",
            extended,
            b'I',
        );
        // SET and RESET each commit or roll back from an independent initial value.
        for reset in [false, true] {
            for commit in [true, false] {
                let before = if reset { "pg_catalog" } else { "public" };
                let during = if reset { "public" } else { "pg_catalog" };
                path_ok(
                    &mut a,
                    &format!("SET search_path TO {before}"),
                    extended,
                    b'I',
                );
                path_ok(&mut a, "BEGIN", extended, b'T');
                path_ok(
                    &mut a,
                    if reset {
                        "RESET search_path"
                    } else {
                        "SET search_path TO pg_catalog"
                    },
                    extended,
                    b'T',
                );
                path_value(&mut a, extended, b'T', during);
                path_ok(
                    &mut a,
                    "INSERT INTO public.path_doc VALUES (2,20)",
                    extended,
                    b'T',
                );
                let rows = path_ok(
                    &mut a,
                    "SELECT id,v FROM public.path_doc ORDER BY id",
                    extended,
                    b'T',
                );
                assert_eq!(
                    path_rows(&rows),
                    vec![
                        vec![Some("1".into()), Some("10".into())],
                        vec![Some("2".into()), Some("20".into())]
                    ]
                );
                path_value(&mut b, extended, b'I', "public");
                assert_eq!(
                    row_count(&path_ok(&mut b, "SELECT id FROM path_doc", extended, b'I')),
                    1
                );
                path_ok(
                    &mut a,
                    if commit { "COMMIT" } else { "ROLLBACK" },
                    extended,
                    b'I',
                );
                path_value(&mut a, extended, b'I', if commit { during } else { before });
                let rows = path_ok(
                    &mut b,
                    "SELECT id,v FROM path_doc ORDER BY id",
                    extended,
                    b'I',
                );
                let mut expected = vec![vec![Some("1".into()), Some("10".into())]];
                if commit {
                    expected.push(vec![Some("2".into()), Some("20".into())]);
                }
                assert_eq!(path_rows(&rows), expected);
                if (if commit { during } else { before }) == "public" {
                    assert_eq!(
                        path_rows(&path_ok(
                            &mut a,
                            "SELECT id,v FROM path_doc ORDER BY id",
                            extended,
                            b'I'
                        )),
                        expected
                    );
                }
                path_value(&mut b, extended, b'I', "public");
                if commit {
                    path_ok(
                        &mut a,
                        "DELETE FROM public.path_doc WHERE id=2",
                        extended,
                        b'I',
                    );
                }
            }
        }
        // Empty effective path is NULL, while SHOW preserves the setting; rollback restores it.
        path_ok(&mut a, "SET search_path TO public", extended, b'I');
        path_ok(&mut a, "BEGIN", extended, b'T');
        path_ok(&mut a, "SET search_path TO ''", extended, b'T');
        path_value(&mut a, extended, b'T', "\"\"");
        assert_eq!(
            path_rows(&path_ok(&mut a, "SELECT current_schema()", extended, b'T')),
            vec![vec![None]]
        );
        path_ok(&mut a, "ROLLBACK", extended, b'I');
        path_value(&mut a, extended, b'I', "public");
        // Syntax error aborts the real transaction; Sync alone must not restore it.
        path_ok(&mut a, "BEGIN", extended, b'T');
        path_ok(&mut a, "SET search_path TO pg_catalog", extended, b'T');
        path_ok(
            &mut a,
            "INSERT INTO public.path_doc VALUES (3,30)",
            extended,
            b'T',
        );
        let failure = path_request(&mut a, "SET search_path TO", extended);
        assert_eq!(
            error_state(&failure.iter().find(|m| m.0 == b'E').unwrap().1),
            Some("42601")
        );
        assert_eq!(failure.last(), Some(&(b'Z', b"E".to_vec())));
        for sql in [
            "SHOW search_path",
            "SET search_path TO public",
            "RESET search_path",
        ] {
            let failure = path_request(&mut a, sql, extended);
            assert_eq!(
                error_state(&failure.iter().find(|m| m.0 == b'E').unwrap().1),
                Some("25P02")
            );
            assert_eq!(failure.last(), Some(&(b'Z', b"E".to_vec())));
        }
        path_value(&mut b, extended, b'I', "public");
        path_ok(&mut a, "ROLLBACK", extended, b'I');
        path_value(&mut a, extended, b'I', "public");
        assert_eq!(
            path_rows(&path_ok(
                &mut a,
                "SELECT id,v FROM path_doc",
                extended,
                b'I'
            )),
            vec![vec![Some("1".into()), Some("10".into())]]
        );
        path_ok(&mut a, "RESET search_path", extended, b'I');
        path_ok(&mut a, "DROP TABLE public.path_doc", extended, b'I');
        send(&mut a, b'X', &[]);
        send(&mut b, b'X', &[]);
        service.close();
        assert!(TcpStream::connect(addr).is_err());
        eprintln!(
            "pg11_04 extended={extended}: 4 SET/RESET end variants, empty/NULL rollback, failed recovery, cleanup passed"
        );
    }
}

#[test]
fn pg11_05() {
    for extended in [false, true] {
        let (service, addr) = start();
        let mut a = connect(addr);
        let mut b = connect(addr);
        path_ok(
            &mut a,
            "CREATE TABLE public.pg1105_doc (id integer PRIMARY KEY, v integer)",
            extended,
            b'I',
        );
        path_ok(
            &mut a,
            "INSERT INTO public.pg1105_doc VALUES (1,10)",
            extended,
            b'I',
        );
        let notice = path_ok(
            &mut a,
            "SET LOCAL search_path TO pg_catalog",
            extended,
            b'I',
        );
        assert!(
            notice
                .iter()
                .any(|m| m.0 == b'N' && error_state(&m.1) == Some("25P01"))
        );
        path_value(&mut a, extended, b'I', "public");
        // Each sequence starts from a fresh session value; ordinary SET/RESET
        // override preceding LOCAL, but LOCAL never replaces the commit value.
        for (commands, during, committed) in [
            (
                vec!["SET LOCAL search_path TO pg_catalog"],
                "pg_catalog",
                "public",
            ),
            (
                vec![
                    "SET search_path TO pg_catalog",
                    "SET LOCAL search_path TO public",
                ],
                "public",
                "pg_catalog",
            ),
            (
                vec!["SET LOCAL search_path TO pg_catalog", "RESET search_path"],
                "public",
                "public",
            ),
            (
                vec![
                    "SET search_path TO pg_catalog",
                    "SET LOCAL search_path TO public",
                    "SET search_path TO pg_catalog",
                ],
                "pg_catalog",
                "pg_catalog",
            ),
            (
                vec![
                    "SET search_path TO pg_catalog",
                    "SET LOCAL search_path TO DEFAULT",
                ],
                "public",
                "pg_catalog",
            ),
            (vec!["SET LOCAL search_path TO ''"], "\"\"", "public"),
            (
                vec![
                    "SET search_path TO pg_catalog",
                    "RESET search_path",
                    "SET LOCAL search_path TO pg_catalog",
                ],
                "pg_catalog",
                "public",
            ),
        ] {
            for commit in [true, false] {
                path_ok(&mut a, "SET search_path TO public", extended, b'I');
                path_ok(&mut a, "BEGIN", extended, b'T');
                for sql in &commands {
                    path_ok(&mut a, sql, extended, b'T');
                }
                path_value(&mut a, extended, b'T', during);
                path_ok(
                    &mut a,
                    "INSERT INTO public.pg1105_doc VALUES (2,20)",
                    extended,
                    b'T',
                );
                assert_eq!(
                    row_count(&path_ok(
                        &mut b,
                        "SELECT id FROM pg1105_doc",
                        extended,
                        b'I'
                    )),
                    1
                );
                if during == "\"\"" {
                    assert_eq!(
                        path_rows(&path_ok(&mut a, "SELECT current_schema()", extended, b'T')),
                        vec![vec![None]]
                    );
                }
                path_value(&mut b, extended, b'I', "public");
                path_ok(
                    &mut a,
                    if commit { "COMMIT" } else { "ROLLBACK" },
                    extended,
                    b'I',
                );
                path_value(
                    &mut a,
                    extended,
                    b'I',
                    if commit { committed } else { "public" },
                );
                let expected = if commit {
                    vec![
                        vec![Some("1".into()), Some("10".into())],
                        vec![Some("2".into()), Some("20".into())],
                    ]
                } else {
                    vec![vec![Some("1".into()), Some("10".into())]]
                };
                assert_eq!(
                    path_rows(&path_ok(
                        &mut b,
                        "SELECT id,v FROM pg1105_doc ORDER BY id",
                        extended,
                        b'I'
                    )),
                    expected
                );
                if (if commit { committed } else { "public" }) == "public" {
                    assert_eq!(
                        path_rows(&path_ok(
                            &mut a,
                            "SELECT id,v FROM pg1105_doc ORDER BY id",
                            extended,
                            b'I'
                        )),
                        expected
                    );
                }
                if commit {
                    path_ok(
                        &mut a,
                        "DELETE FROM public.pg1105_doc WHERE id=2",
                        extended,
                        b'I',
                    );
                }
            }
        }
        path_ok(&mut a, "SET search_path TO public", extended, b'I');
        path_ok(&mut a, "BEGIN", extended, b'T');
        path_ok(
            &mut a,
            "SET LOCAL search_path TO pg_catalog",
            extended,
            b'T',
        );
        path_ok(
            &mut a,
            "INSERT INTO public.pg1105_doc VALUES (3,30)",
            extended,
            b'T',
        );
        for (sql, expected) in [
            ("SET LOCAL search_path TO NULL", "42601"),
            ("SET LOCAL search_path TO public", "25P02"),
        ] {
            let failure = path_request(&mut a, sql, extended);
            assert_eq!(
                error_state(&failure.iter().find(|m| m.0 == b'E').unwrap().1),
                Some(expected)
            );
            assert_eq!(failure.last(), Some(&(b'Z', b"E".to_vec())));
        }
        path_ok(&mut a, "ROLLBACK", extended, b'I');
        path_value(&mut a, extended, b'I', "public");
        path_value(&mut b, extended, b'I', "public");
        path_ok(&mut a, "RESET search_path", extended, b'I');
        assert_eq!(
            path_rows(&path_ok(
                &mut a,
                "SELECT id,v FROM pg1105_doc ORDER BY id",
                extended,
                b'I'
            )),
            vec![vec![Some("1".into()), Some("10".into())]]
        );
        path_ok(&mut a, "DROP TABLE public.pg1105_doc", extended, b'I');
        send(&mut a, b'X', &[]);
        send(&mut b, b'X', &[]);
        service.close();
        assert!(TcpStream::connect(addr).is_err());
        eprintln!(
            "pg11_05 extended={extended}: outside notice, 14 lifecycle variants, NULL/error recovery and cleanup passed"
        );
    }
}

#[test]
fn pg11_06() {
    for extended in [false, true] {
        let (service, addr) = start();
        let mut a = connect(addr);
        let mut b = connect(addr);
        path_ok(
            &mut a,
            "CREATE TABLE public.pg1106_doc (id integer PRIMARY KEY, v integer)",
            extended,
            b'I',
        );
        path_ok(
            &mut a,
            "INSERT INTO public.pg1106_doc VALUES (1,10)",
            extended,
            b'I',
        );
        let baseline = vec![vec![Some("1".into()), Some("10".into())]];
        for setting in [
            "SET search_path TO pg_catalog",
            "SET search_path TO ''",
            "SET LOCAL search_path TO pg_catalog",
        ] {
            path_value(&mut a, extended, b'I', "public");
            path_ok(&mut a, "BEGIN", extended, b'T');
            path_ok(&mut a, setting, extended, b'T');
            let empty = setting == "SET search_path TO ''";
            path_value(
                &mut a,
                extended,
                b'T',
                if empty { "\"\"" } else { "pg_catalog" },
            );
            if empty {
                assert_eq!(
                    path_rows(&path_ok(&mut a, "SELECT current_schema()", extended, b'T')),
                    vec![vec![None]]
                );
            }
            path_ok(
                &mut a,
                "UPDATE public.pg1106_doc SET v=11 WHERE id=1",
                extended,
                b'T',
            );
            let failure = path_request(
                &mut a,
                "INSERT INTO public.pg1106_doc VALUES (1,99)",
                extended,
            );
            assert_eq!(
                error_state(&failure.iter().find(|m| m.0 == b'E').unwrap().1),
                Some("23505")
            );
            assert_eq!(failure.last(), Some(&(b'Z', b"E".to_vec())));
            for sql in [
                "SHOW search_path",
                "SET search_path TO public",
                "RESET search_path",
                "SET LOCAL search_path TO public",
                "SET search_path TO DEFAULT",
                "SET LOCAL search_path TO ''",
                "SELECT current_schema()",
                "SHOW search_path",
            ] {
                let blocked = path_request(&mut a, sql, extended);
                assert_eq!(
                    error_state(&blocked.iter().find(|m| m.0 == b'E').unwrap().1),
                    Some("25P02"),
                    "{sql}: {blocked:?}"
                );
                assert_eq!(blocked.last(), Some(&(b'Z', b"E".to_vec())));
                assert!(!blocked.iter().any(|m| matches!(m.0, b'C' | b'D' | b'N')));
                if extended {
                    assert_eq!(blocked.first().unwrap().0, b'E');
                    sync(&mut a, b'E');
                }
                path_value(&mut b, extended, b'I', "public");
                assert_eq!(
                    path_rows(&path_ok(
                        &mut b,
                        "SELECT id,v FROM pg1106_doc ORDER BY id",
                        extended,
                        b'I'
                    )),
                    baseline
                );
            }
            path_ok(&mut a, "ROLLBACK", extended, b'I');
            path_value(&mut a, extended, b'I', "public");
            assert_eq!(
                path_rows(&path_ok(
                    &mut a,
                    "SELECT id,v FROM pg1106_doc ORDER BY id",
                    extended,
                    b'I'
                )),
                baseline
            );
        }
        // NULL is invalid input, not a successful NULL setting.
        path_ok(&mut a, "BEGIN", extended, b'T');
        let invalid = path_request(&mut a, "SET LOCAL search_path TO NULL", extended);
        assert_eq!(
            error_state(&invalid.iter().find(|m| m.0 == b'E').unwrap().1),
            Some("42601")
        );
        assert_eq!(invalid.last(), Some(&(b'Z', b"E".to_vec())));
        path_ok(&mut a, "ROLLBACK", extended, b'I');
        path_value(&mut a, extended, b'I', "public");
        path_ok(&mut a, "RESET search_path", extended, b'I');
        path_ok(&mut a, "DROP TABLE public.pg1106_doc", extended, b'I');
        send(&mut a, b'X', &[]);
        send(&mut b, b'X', &[]);
        service.close();
        assert!(TcpStream::connect(addr).is_err());
        eprintln!(
            "pg11_06 extended={extended}: 3 duplicate-key recovery variants, 24 blocked requests, NULL recovery and cleanup passed"
        );
    }
}
