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
    let (service, addr, _) = start_with_domain();
    (service, addr)
}

fn start_with_domain() -> (
    Arc<PgService>,
    std::net::SocketAddr,
    Arc<astersql_domain::Domain>,
) {
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
        Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        false,
    )
    .unwrap();
    (service, addr, domain)
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

    let blocked = query(&mut socket, "SELECT 1");
    assert_eq!(blocked[0].0, b'E', "{blocked:?}");
    assert_eq!(error_state(&blocked[0].1), Some("25P02"));
    assert_eq!(blocked.last(), Some(&(b'Z', b"E".to_vec())));

    let committed = query(&mut socket, "COMMIT");
    assert_eq!(committed[0], (b'C', b"ROLLBACK\0".to_vec()));
    assert_eq!(committed.last(), Some(&(b'Z', b"I".to_vec())));
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

#[test]
fn pg11_17() {
    fn run(socket: &mut TcpStream, sql: &str, extended: bool) -> Vec<(u8, Vec<u8>)> {
        if !extended {
            return query(socket, sql);
        }
        parse(socket, "", sql, &[]);
        assert_eq!(read(socket), (b'1', vec![]), "{sql}");
        send(socket, b'D', b"S\0");
        assert_eq!(read(socket), (b't', 0i16.to_be_bytes().to_vec()));
        let description = read(socket);
        if sql.starts_with("SELECT") {
            assert_eq!(description.0, b'T', "{sql}");
            if sql.contains("val(") {
                let field = &description.1;
                assert_eq!(&field[..4], b"\0\x01v\0");
                assert_eq!(&field[10..14], &20u32.to_be_bytes());
                assert_eq!(&field[14..16], &8i16.to_be_bytes());
            }
        } else {
            assert_eq!(description, (b'n', vec![]));
        }
        let mut body = b"\0\0".to_vec();
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        send(socket, b'B', &body);
        assert_eq!(read(socket), (b'2', vec![]), "{sql}");
        execute(socket, "");
        send(socket, b'S', &[]);
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let done = m.0 == b'Z';
            messages.push(m);
            if done {
                return messages;
            }
        }
    }
    fn ok(socket: &mut TcpStream, sql: &str, extended: bool, status: u8) -> Vec<(u8, Vec<u8>)> {
        let m = run(socket, sql, extended);
        assert!(!m.iter().any(|m| m.0 == b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![status])), "{sql}");
        m
    }
    fn value(socket: &mut TcpStream, sql: &str, expected: Option<i64>, extended: bool, status: u8) {
        let m = ok(socket, sql, extended, status);
        let rows: Vec<_> = m.iter().filter(|m| m.0 == b'D').collect();
        assert_eq!(rows.len(), 1, "{sql}");
        let data = &rows[0].1;
        assert_eq!(&data[..2], &1i16.to_be_bytes());
        match expected {
            Some(v) => {
                let text = v.to_string();
                assert_eq!(&data[2..6], &(text.len() as i32).to_be_bytes());
                assert_eq!(&data[6..], text.as_bytes(), "{sql}");
            }
            None => assert_eq!(&data[2..], &(-1i32).to_be_bytes()),
        }
    }
    for extended in [false, true] {
        for scenario in 0..4 {
            let (service, addr) = start();
            let mut a = connect(addr);
            let mut b = connect(addr);
            ok(
                &mut a,
                "CREATE SEQUENCE public.seq_contract AS bigint INCREMENT BY 1 MINVALUE 1 MAXVALUE 1000 START WITH 10 CACHE 1 NO CYCLE",
                false,
                b'I',
            );
            ok(
                &mut a,
                "CREATE TABLE public.seq_rows (id integer PRIMARY KEY,v bigint)",
                false,
                b'I',
            );
            if scenario == 1 {
                value(
                    &mut a,
                    "SELECT nextval('public.seq_contract'::regclass) AS v",
                    Some(10),
                    extended,
                    b'I',
                );
            }
            ok(&mut a, "BEGIN", extended, b'T');
            if scenario == 1 {
                value(
                    &mut a,
                    "SELECT setval('public.seq_contract'::regclass,80,true) AS v",
                    Some(80),
                    extended,
                    b'T',
                );
                value(
                    &mut b,
                    "SELECT nextval('public.seq_contract'::regclass) AS v",
                    Some(81),
                    extended,
                    b'I',
                );
            } else {
                value(
                    &mut a,
                    "SELECT nextval('public.seq_contract'::regclass) AS v",
                    Some(10),
                    extended,
                    b'T',
                );
                ok(
                    &mut a,
                    "INSERT INTO public.seq_rows VALUES (1,10)",
                    extended,
                    b'T',
                );
                assert_eq!(
                    row_count(&ok(
                        &mut b,
                        "SELECT id,v FROM public.seq_rows ORDER BY id",
                        extended,
                        b'I'
                    )),
                    0
                );
            }
            if scenario == 2 {
                value(
                    &mut a,
                    "SELECT setval('public.seq_contract'::regclass,NULL,true) AS v",
                    None,
                    extended,
                    b'T',
                );
                let m = run(
                    &mut a,
                    "SELECT setval('public.seq_contract'::regclass,0,true) AS v",
                    extended,
                );
                let error = m.iter().find(|m| m.0 == b'E').unwrap();
                assert_eq!(error_state(&error.1), Some("22003"));
                assert_eq!(m.last(), Some(&(b'Z', vec![b'E'])));
                let blocked = query(&mut a, "SELECT 1");
                assert_eq!(error_state(&blocked[0].1), Some("25P02"));
                if extended {
                    sync(&mut a, b'E');
                }
            }
            if scenario == 3 {
                send(&mut a, b'X', &[]);
                drop(a);
                // A new connection has no currval; close must roll back its rows.
                a = connect(addr);
            } else {
                ok(&mut a, "ROLLBACK", extended, b'I');
            }
            if scenario != 3 {
                value(
                    &mut a,
                    "SELECT currval('public.seq_contract'::regclass) AS v",
                    Some(if scenario == 1 { 80 } else { 10 }),
                    extended,
                    b'I',
                );
            } else {
                let m = run(
                    &mut a,
                    "SELECT currval('public.seq_contract'::regclass) AS v",
                    extended,
                );
                assert_eq!(
                    error_state(&m.iter().find(|m| m.0 == b'E').unwrap().1),
                    Some("55000")
                );
                assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
            }
            assert_eq!(
                row_count(&ok(
                    &mut b,
                    "SELECT id,v FROM public.seq_rows ORDER BY id",
                    extended,
                    b'I'
                )),
                0
            );
            value(
                &mut b,
                "SELECT nextval('public.seq_contract'::regclass) AS v",
                Some(if scenario == 1 { 82 } else { 11 }),
                extended,
                b'I',
            );
            value(&mut a, "SELECT 1 AS ok", Some(1), extended, b'I');
            value(&mut b, "SELECT 1 AS ok", Some(1), extended, b'I');
            ok(&mut a, "DROP TABLE public.seq_rows", false, b'I');
            ok(&mut a, "DROP SEQUENCE public.seq_contract", false, b'I');
            send(&mut a, b'X', &[]);
            send(&mut b, b'X', &[]);
            service.close();
            assert!(TcpStream::connect(addr).is_err());
            eprintln!("pg11_17 extended={extended} scenario={scenario} passed");
        }
    }
}

#[test]
fn pg11_27() {
    fn run(socket: &mut TcpStream, sql: &str, extended: bool) -> Vec<(u8, Vec<u8>)> {
        if !extended {
            return query(socket, sql);
        }
        parse(socket, "", sql, &[]);
        assert_eq!(read(socket), (b'1', vec![]), "{sql}");
        send(socket, b'D', b"S\0");
        assert_eq!(read(socket), (b't', 0i16.to_be_bytes().to_vec()));
        let description = read(socket);
        if sql.starts_with("SELECT") {
            assert_eq!(description.0, b'T', "{sql}");
            if sql.contains("val(") {
                let field = &description.1;
                assert_eq!(&field[..4], b"\0\x01v\0");
                assert_eq!(&field[10..14], &20u32.to_be_bytes());
                assert_eq!(&field[14..16], &8i16.to_be_bytes());
            }
        } else {
            assert_eq!(description, (b'n', vec![]));
        }
        let mut body = b"\0\0".to_vec();
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        send(socket, b'B', &body);
        assert_eq!(read(socket), (b'2', vec![]), "{sql}");
        execute(socket, "");
        send(socket, b'S', &[]);
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let done = m.0 == b'Z';
            messages.push(m);
            if done {
                return messages;
            }
        }
    }
    fn ok(socket: &mut TcpStream, sql: &str, extended: bool, status: u8) -> Vec<(u8, Vec<u8>)> {
        let m = run(socket, sql, extended);
        assert!(!m.iter().any(|m| m.0 == b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![status])), "{sql}");
        m
    }
    fn value(socket: &mut TcpStream, sql: &str, expected: Option<i64>, extended: bool, status: u8) {
        let m = ok(socket, sql, extended, status);
        let rows: Vec<_> = m.iter().filter(|m| m.0 == b'D').collect();
        assert_eq!(rows.len(), 1, "{sql}");
        let data = &rows[0].1;
        assert_eq!(&data[..2], &1i16.to_be_bytes());
        match expected {
            Some(v) => {
                let text = v.to_string();
                assert_eq!(&data[2..6], &(text.len() as i32).to_be_bytes());
                assert_eq!(&data[6..], text.as_bytes(), "{sql}");
            }
            None => assert_eq!(&data[2..], &(-1i32).to_be_bytes()),
        }
    }
    for extended in [false, true] {
        for scenario in 0..3 {
            let (service, addr) = start();
            let mut a = connect(addr);
            let mut b = connect(addr);
            ok(
                &mut a,
                "CREATE TABLE public.identity_rollback (id bigint GENERATED ALWAYS AS IDENTITY (START WITH 10 MINVALUE 1 MAXVALUE 1000 CACHE 1) PRIMARY KEY,note text)",
                false,
                b'I',
            );
            // Resolve the owned sequence from metadata, never from its generated name.
            let m = ok(
                &mut a,
                "SELECT relname FROM pg_catalog.pg_class WHERE relkind = 'S'",
                false,
                b'I',
            );
            let row = &m.iter().find(|m| m.0 == b'D').unwrap().1;
            assert_eq!(row_count(&m), 1);
            let seq = std::str::from_utf8(&row[6..]).unwrap();
            let curr = format!("SELECT currval('{seq}'::regclass) AS v");
            let m = run(&mut b, &curr, extended);
            assert_eq!(
                error_state(&m.iter().find(|m| m.0 == b'E').unwrap().1),
                Some("55000")
            );
            assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
            ok(&mut a, "BEGIN", extended, b'T');
            ok(
                &mut a,
                "INSERT INTO public.identity_rollback(note) VALUES (NULL)",
                extended,
                b'T',
            );
            value(
                &mut a,
                "SELECT id AS v FROM public.identity_rollback",
                Some(10),
                extended,
                b'T',
            );
            value(
                &mut a,
                "SELECT note AS v FROM public.identity_rollback",
                None,
                extended,
                b'T',
            );
            assert_eq!(
                row_count(&ok(
                    &mut b,
                    "SELECT id FROM public.identity_rollback",
                    extended,
                    b'I'
                )),
                0
            );
            value(&mut a, &curr, Some(10), extended, b'T');
            if scenario == 1 {
                let m = run(
                    &mut a,
                    "INSERT INTO public.identity_rollback(id) OVERRIDING SYSTEM VALUE VALUES (NULL)",
                    extended,
                );
                assert_eq!(
                    error_state(&m.iter().find(|m| m.0 == b'E').unwrap().1),
                    Some("23502")
                );
                assert_eq!(row_count(&m), 0);
                assert_eq!(m.last(), Some(&(b'Z', vec![b'E'])));
                let blocked = query(&mut a, "SELECT 1");
                assert_eq!(
                    error_state(&blocked.iter().find(|m| m.0 == b'E').unwrap().1),
                    Some("25P02")
                );
                if extended {
                    sync(&mut a, b'E');
                }
            }
            if scenario == 2 {
                send(&mut a, b'X', &[]);
                drop(a);
                a = connect(addr);
                let m = run(&mut a, &curr, extended);
                assert_eq!(
                    error_state(&m.iter().find(|m| m.0 == b'E').unwrap().1),
                    Some("55000")
                );
            } else {
                ok(&mut a, "ROLLBACK", extended, b'I');
                value(&mut a, &curr, Some(10), extended, b'I');
            }
            for socket in [&mut a, &mut b] {
                assert_eq!(
                    row_count(&ok(
                        socket,
                        "SELECT id,note FROM public.identity_rollback",
                        extended,
                        b'I'
                    )),
                    0
                );
            }
            ok(
                &mut b,
                "INSERT INTO public.identity_rollback(note) VALUES ('survivor')",
                extended,
                b'I',
            );
            value(
                &mut b,
                "SELECT id AS v FROM public.identity_rollback",
                Some(11),
                extended,
                b'I',
            );
            value(&mut b, &curr, Some(11), extended, b'I');
            if scenario != 2 {
                value(&mut a, &curr, Some(10), extended, b'I');
            }
            send(&mut a, b'X', &[]);
            drop(a);
            // Closing A cannot clear B's private currval or its committed row.
            let committed = ok(
                &mut b,
                "SELECT id,note FROM public.identity_rollback",
                extended,
                b'I',
            );
            let rows: Vec<_> = committed
                .iter()
                .filter(|m| m.0 == b'D')
                .map(|m| m.1.as_slice())
                .collect();
            let expected = [
                2i16.to_be_bytes().as_slice(),
                2i32.to_be_bytes().as_slice(),
                b"11",
                8i32.to_be_bytes().as_slice(),
                b"survivor",
            ]
            .concat();
            assert_eq!(rows, vec![expected.as_slice()]);
            value(&mut b, &curr, Some(11), extended, b'I');
            value(
                &mut b,
                "SELECT id AS v FROM public.identity_rollback",
                Some(11),
                extended,
                b'I',
            );
            value(&mut b, "SELECT 1 AS ok", Some(1), extended, b'I');
            ok(&mut b, "DROP TABLE public.identity_rollback", false, b'I');
            send(&mut b, b'X', &[]);
            service.close();
            assert!(TcpStream::connect(addr).is_err());
            eprintln!("pg11_27 extended={extended} scenario={scenario} passed");
        }
    }
}

#[test]
fn pg11_28() {
    fn run(socket: &mut TcpStream, sql: &str, extended: bool) -> Vec<(u8, Vec<u8>)> {
        if !extended {
            return query(socket, sql);
        }
        parse(socket, "", sql, &[]);
        assert_eq!(read(socket), (b'1', vec![]), "{sql}");
        send(socket, b'D', b"S\0");
        assert_eq!(read(socket), (b't', 0i16.to_be_bytes().to_vec()));
        let description = read(socket);
        if sql.starts_with("SELECT") || sql.contains("RETURNING") {
            assert_eq!(description.0, b'T', "{sql}");
            if sql.contains("val(") {
                let field = &description.1;
                assert_eq!(&field[..4], b"\0\x01v\0");
                assert_eq!(&field[10..14], &20u32.to_be_bytes());
                assert_eq!(&field[14..16], &8i16.to_be_bytes());
            }
        } else {
            assert_eq!(description, (b'n', vec![]));
        }
        let mut body = b"\0\0".to_vec();
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        body.extend_from_slice(&0i16.to_be_bytes());
        send(socket, b'B', &body);
        assert_eq!(read(socket), (b'2', vec![]), "{sql}");
        execute(socket, "");
        send(socket, b'S', &[]);
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let done = m.0 == b'Z';
            messages.push(m);
            if done {
                return messages;
            }
        }
    }
    fn ok(socket: &mut TcpStream, sql: &str, extended: bool, status: u8) -> Vec<(u8, Vec<u8>)> {
        let m = run(socket, sql, extended);
        assert!(!m.iter().any(|m| m.0 == b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![status])), "{sql}");
        m
    }
    fn value(socket: &mut TcpStream, sql: &str, expected: Option<i64>, extended: bool, status: u8) {
        let m = ok(socket, sql, extended, status);
        let rows: Vec<_> = m.iter().filter(|m| m.0 == b'D').collect();
        assert_eq!(rows.len(), 1, "{sql}");
        let data = &rows[0].1;
        assert_eq!(&data[..2], &1i16.to_be_bytes());
        match expected {
            Some(v) => {
                let text = v.to_string();
                assert_eq!(&data[2..6], &(text.len() as i32).to_be_bytes());
                assert_eq!(&data[6..], text.as_bytes(), "{sql}");
            }
            None => assert_eq!(&data[2..], &(-1i32).to_be_bytes()),
        }
    }

    fn rows(m: &[(u8, Vec<u8>)]) -> Vec<Vec<u8>> {
        m.iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect()
    }
    fn row(id: &str, note: Option<&str>) -> Vec<u8> {
        let mut b = 2i16.to_be_bytes().to_vec();
        for v in [Some(id), note] {
            if let Some(v) = v {
                b.extend_from_slice(&(v.len() as i32).to_be_bytes());
                b.extend_from_slice(v.as_bytes());
            } else {
                b.extend_from_slice(&(-1i32).to_be_bytes());
            }
        }
        b
    }
    for extended in [false, true] {
        for transaction in [false, true] {
            let (service, addr) = start();
            let mut a = connect(addr);
            let mut b = connect(addr);
            ok(
                &mut a,
                "CREATE TABLE public.identity_failure (id bigint GENERATED ALWAYS AS IDENTITY (START WITH 10 MINVALUE 1 MAXVALUE 1000 CACHE 1) PRIMARY KEY,note text UNIQUE)",
                false,
                b'I',
            );
            let seed = ok(
                &mut a,
                "INSERT INTO public.identity_failure(note) VALUES ('seed') RETURNING id,note",
                extended,
                b'I',
            );
            assert_eq!(rows(&seed), vec![row("10", Some("seed"))]);
            if transaction {
                ok(&mut a, "BEGIN", extended, b'T');
            }
            let failed = run(
                &mut a,
                "INSERT INTO public.identity_failure(note) VALUES ('seed') RETURNING id,note",
                extended,
            );
            assert_eq!(
                error_state(
                    &failed
                        .iter()
                        .find(|m| m.0 == b'E')
                        .expect("unique violation")
                        .1
                ),
                Some("23505")
            );
            assert!(rows(&failed).is_empty());
            assert!(!failed.iter().any(|m| m.0 == b'C'));
            assert_eq!(
                failed.last(),
                Some(&(b'Z', vec![if transaction { b'E' } else { b'I' }]))
            );
            if transaction {
                let blocked = query(&mut a, "SELECT 1");
                assert_eq!(
                    error_state(&blocked.iter().find(|m| m.0 == b'E').unwrap().1),
                    Some("25P02")
                );
                assert_eq!(blocked.last(), Some(&(b'Z', vec![b'E'])));
                if extended {
                    sync(&mut a, b'E');
                }
                ok(&mut a, "ROLLBACK", extended, b'I');
            }
            value(&mut a, "SELECT 1 AS ok", Some(1), extended, b'I');
            let snapshot = "SELECT id,note FROM public.identity_failure ORDER BY id";
            assert_eq!(
                rows(&ok(&mut b, snapshot, extended, b'I')),
                vec![row("10", Some("seed"))]
            );
            let after = ok(
                &mut a,
                "INSERT INTO public.identity_failure(note) VALUES ('after') RETURNING id,note",
                extended,
                b'I',
            );
            assert_eq!(
                rows(&after),
                vec![row("12", Some("after"))],
                "allocated 11 must not be reclaimed"
            );
            assert_eq!(
                rows(&ok(&mut b, snapshot, extended, b'I')),
                vec![row("10", Some("seed")), row("12", Some("after"))]
            );
            // Independent UNIQUE permits multiple SQL NULLs; both consume real identities.
            for id in ["13", "14"] {
                assert_eq!(
                    rows(&ok(
                        &mut a,
                        "INSERT INTO public.identity_failure(note) VALUES (NULL) RETURNING id,note",
                        extended,
                        b'I'
                    )),
                    vec![row(id, None)]
                );
            }
            send(&mut a, b'X', &[]);
            drop(a);
            assert_eq!(
                rows(&ok(&mut b, snapshot, extended, b'I')),
                vec![
                    row("10", Some("seed")),
                    row("12", Some("after")),
                    row("13", None),
                    row("14", None)
                ]
            );
            value(&mut b, "SELECT 1 AS ok", Some(1), extended, b'I');
            ok(&mut b, "DROP TABLE public.identity_failure", false, b'I');
            send(&mut b, b'X', &[]);
            service.close();
            assert!(TcpStream::connect(addr).is_err());
            eprintln!("pg11_28 extended={extended} transaction={transaction} passed");
        }
    }
}

#[test]
fn pg11_34_implicit_identity_transaction() {
    fn insert(socket: &mut TcpStream, name: &str) -> (u8, Vec<u8>) {
        parse(
            socket,
            name,
            "INSERT INTO identity_sync(note) VALUES ('seed') RETURNING id,note",
            &[],
        );
        assert_eq!(read(socket).0, b'1');
        let body = [name.as_bytes(), b"\0", name.as_bytes(), b"\0\0\0\0\0\0\0"].concat();
        send(socket, b'B', &body);
        assert_eq!(read(socket).0, b'2');
        execute(socket, name);
        loop {
            let message = read(socket);
            if matches!(message.0, b'C' | b'E') {
                return message;
            }
        }
    }
    for scenario in [
        "commit",
        "execute_error",
        "parse_error",
        "disconnect",
        "explicit",
        "explicit_error",
    ] {
        let (service, addr) = start();
        let mut a = connect(addr);
        let mut b = connect(addr);
        assert!(!query(&mut a,"CREATE TABLE identity_sync(id bigint GENERATED ALWAYS AS IDENTITY (START 10),note text UNIQUE)").iter().any(|m| m.0==b'E'));
        let explicit = scenario.starts_with("explicit");
        if explicit {
            assert_eq!(query(&mut a, "BEGIN").last().unwrap(), &(b'Z', vec![b'T']));
        }
        assert_eq!(insert(&mut a, "first").0, b'C');
        assert!(
            path_rows(&query(&mut b, "SELECT id,note FROM identity_sync")).is_empty(),
            "{scenario}: writes before Sync"
        );
        let error = scenario.ends_with("error");
        if scenario == "parse_error" {
            parse(&mut a, "bad", "SELECT missing_identity_column", &[]);
            let message = read(&mut a);
            assert_eq!(message.0, b'E');
            assert_eq!(error_state(&message.1), Some("42703"));
        } else if error {
            let message = insert(&mut a, "second");
            assert_eq!(message.0, b'E');
            assert_eq!(error_state(&message.1), Some("23505"));
        }
        if scenario == "disconnect" {
            send(&mut a, b'X', &[]);
            drop(a);
            a = connect(addr);
        } else {
            sync(
                &mut a,
                if explicit {
                    if error { b'E' } else { b'T' }
                } else {
                    b'I'
                },
            );
        }
        if explicit {
            assert!(path_rows(&query(&mut b, "SELECT id,note FROM identity_sync")).is_empty());
            if error {
                let m = query(&mut a, "SELECT 1");
                assert_eq!(error_state(&m[0].1), Some("25P02"));
            }
            query(&mut a, "ROLLBACK");
        }
        let rows = path_rows(&query(
            &mut b,
            "SELECT id,note FROM identity_sync ORDER BY id",
        ));
        assert_eq!(
            rows,
            if scenario == "commit" {
                vec![vec![Some("10".into()), Some("seed".into())]]
            } else {
                vec![]
            },
            "{scenario}"
        );
        let next = if scenario == "execute_error" || scenario == "explicit_error" {
            "12"
        } else {
            "11"
        };
        assert_eq!(
            path_rows(&query(
                &mut b,
                "INSERT INTO identity_sync(note) VALUES ('after') RETURNING id"
            )),
            vec![vec![Some(next.into())]],
            "{scenario}: allocation must survive rollback"
        );
        query(&mut b, "DROP TABLE identity_sync");
        send(&mut a, b'X', &[]);
        send(&mut b, b'X', &[]);
        service.close();
    }
}

/// Check complete values through two independent libpq endpoint observations.
#[test]
fn pg12_commit() {
    pg12_transaction_case("commit");
}

#[test]
fn pg12_rollback() {
    pg12_transaction_case("rollback");
}

#[test]
fn pg12_single_implicit() {
    pg12_transaction_case("single_implicit");
}

#[test]
fn pg12_repeated_begin() {
    pg12_transaction_case("repeated_begin");
}

#[test]
fn pg12_outside_end() {
    pg12_transaction_case("outside_end");
}

#[test]
fn pg12_disconnect() {
    pg12_transaction_case("disconnect");
}

#[test]
fn pg12_old_portal() {
    pg12_transaction_case("old_portal");
}

#[test]
fn pg12_failed_commit() {
    pg12_transaction_case("failed_commit");
}

#[test]
fn pg12_failed_rollback() {
    pg12_transaction_case("failed_rollback");
}

#[test]
fn pg12_extended_error() {
    pg12_transaction_case("extended_error");
}

#[test]
fn pg12_extended_success() {
    pg12_transaction_case("extended_success");
}

#[test]
fn pg12_query_block() {
    pg12_transaction_case("query_block");
}

#[test]
fn pg12_savepoint_gate() {
    pg12_transaction_case("savepoint_gate");
}

#[test]
fn pg12_table_ddl_gate() {
    pg12_transaction_case("table_ddl_gate");
}

#[test]
fn pg12_explicit_sync() {
    pg12_transaction_case("explicit_sync");
}

fn pg12_transaction_case(case: &str) {
    assert!(
        std::env::var_os("PG18_REFERENCE_DSN").is_some(),
        "PG18_REFERENCE_DSN is required (no skip)"
    );
    let (service, addr, domain) = start_with_domain();
    struct Close(Arc<PgService>);
    impl Drop for Close {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let guard = Close(service);
    let database = format!("pg12_{case}_{}_{}", std::process::id(), addr.port());
    let mut admin = connect(addr);
    let created = query(&mut admin, &format!("CREATE DATABASE {database}"));
    assert_eq!(created[0].0, b'C', "{created:?}");
    // Reuse the existing DataRow decoder; inspect every RowDescription field
    // needed by this integer/NULL contract rather than accepting row counts.
    let probe = query(&mut admin, "SELECT 1::int4 AS id, NULL::int4 AS value");
    assert_eq!(path_rows(&probe), vec![vec![Some("1".into()), None]]);
    let description = &probe.iter().find(|m| m.0 == b'T').unwrap().1;
    assert_eq!(i16::from_be_bytes(description[..2].try_into().unwrap()), 2);
    let mut offset = 2;
    for name in [b"id".as_slice(), b"value".as_slice()] {
        let end = offset + description[offset..].iter().position(|b| *b == 0).unwrap();
        assert_eq!(&description[offset..end], name);
        let field = &description[end + 1..end + 19];
        assert_eq!(u32::from_be_bytes(field[6..10].try_into().unwrap()), 23);
        assert_eq!(i16::from_be_bytes(field[10..12].try_into().unwrap()), 4);
        assert_eq!(i32::from_be_bytes(field[12..16].try_into().unwrap()), -1);
        assert_eq!(i16::from_be_bytes(field[16..18].try_into().unwrap()), 0);
        offset = end + 19;
    }
    assert_eq!(offset, description.len());
    let barrier = std::env::temp_dir().join(format!("{database}_cleanup"));
    let ddl_snapshot = barrier.with_extension("ddl");
    let ddl_reply = barrier.with_extension("reply");
    let mut child = std::process::Command::new("python3")
        .env("PG12_DDL_SNAPSHOT", &ddl_snapshot)
        .env("PG12_CLEANUP_BARRIER", &barrier)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg12_transaction_libpq.py"
        ))
        .args([
            "--case",
            case,
            "--host",
            "127.0.0.1",
            "--port",
            &addr.port().to_string(),
            "--database",
            &database,
        ])
        .spawn()
        .expect("Python3 required");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(100);
    while child.try_wait().expect("poll client").is_none() {
        // The surviving connections are Rust admin, Python admin and B.
        // Socket removal follows synchronous context.close(), so both counts
        // reaching three confirms rollback has completed before observation.
        if ddl_snapshot.exists() {
            // Read the real canonical schema after the client's completed SQL;
            // the handshake never changes execution or returns query results.
            let version = domain.info_schema().SchemaMetaVersion();
            std::fs::write(&ddl_reply, version.to_string()).unwrap();
            std::fs::remove_file(&ddl_snapshot).unwrap();
        }
        let (active, sockets, _) = guard.0.resource_counts();
        if barrier.exists() && active == 3 && sockets == 3 {
            std::fs::remove_file(&barrier).unwrap();
        }
        if std::time::Instant::now() >= deadline {
            child.kill().expect("stop timed out client");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let output = child.wait_with_output();
    let _ = std::fs::remove_file(&barrier);
    let _ = std::fs::remove_file(&ddl_snapshot);
    let _ = std::fs::remove_file(&ddl_reply);
    let cleanup = query(&mut admin, &format!("DROP DATABASE {database}"));
    send(&mut admin, b'X', &[]);
    drop(admin);
    guard.0.close();
    assert_eq!(guard.0.resource_counts(), (0, 0, 0), "PG resources leaked");
    drop(guard);
    assert!(TcpStream::connect(addr).is_err(), "PG listener leaked");
    assert_eq!(cleanup[0].0, b'C', "{cleanup:?}");
    let output = output.expect("Python3 required");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
