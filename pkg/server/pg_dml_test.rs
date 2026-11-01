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
    let _service_guard = Pg10ServiceGuard(_service);
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

    // RETURNING executes through the shared DML row images.
    parse(
        &mut socket,
        "returning",
        "INSERT INTO public.prepared_crud VALUES ($1, $2, NULL) RETURNING id",
        &[23, 1043],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(
        &mut socket,
        "returning_p",
        "returning",
        &[],
        &[Some(b"2"), Some(b"two")],
        &[],
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    execute(&mut socket, "returning_p");
    let mut returned = Vec::new();
    loop {
        let message = read(&mut socket);
        let done = message.0 == b'C' || message.0 == b'E';
        returned.push(message);
        if done {
            break;
        }
    }
    assert_eq!(pg10_rows(&returned), vec![text_row(&[Some("2")])]);
    assert_eq!(returned.last(), Some(&(b'C', b"INSERT 0 1\0".to_vec())));
    sync(&mut socket);
    parse(
        &mut socket,
        "delete_returning",
        "DELETE FROM public.prepared_crud WHERE id = $1 RETURNING id",
        &[23],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(
        &mut socket,
        "delete_p",
        "delete_returning",
        &[],
        &[Some(b"2")],
        &[],
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    execute(&mut socket, "delete_p");
    assert_eq!(read(&mut socket), (b'D', text_row(&[Some("2")])));
    assert_eq!(read(&mut socket), (b'C', b"DELETE 1\0".to_vec()));
    sync(&mut socket);
    send(&mut socket, b'C', b"Sdelete_returning\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    sync(&mut socket);
    let rows = simple_query(&mut socket, "SELECT id FROM public.prepared_crud");
    assert_eq!(
        rows.iter().map(|message| message.0).collect::<Vec<_>>(),
        b"TCZ"
    );
    assert!(pg10_rows(&rows).is_empty());
    let cleanup = simple_query(&mut socket, "DROP TABLE public.prepared_crud");
    assert_eq!(cleanup[0].0, b'C');
}

struct Pg10ServiceGuard(Arc<PgService>);

impl Drop for Pg10ServiceGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn pg10_connect(addr: std::net::SocketAddr) -> TcpStream {
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    let mut settings = std::collections::HashMap::new();
    loop {
        let message = read(&mut socket);
        if message.0 == b'S' {
            let fields = message.1.split(|b| *b == 0).collect::<Vec<_>>();
            settings.insert(
                String::from_utf8(fields[0].to_vec()).unwrap(),
                String::from_utf8(fields[1].to_vec()).unwrap(),
            );
        }
        if message.0 == b'Z' {
            assert_eq!(message.1, b"I");
            break;
        }
    }
    assert_eq!(
        settings.get("client_encoding").map(String::as_str),
        Some("UTF8")
    );
    assert_eq!(settings.get("TimeZone").map(String::as_str), Some("UTC"));
    socket
}

fn pg10_ok(socket: &mut TcpStream, sql: &str, state: u8) -> Vec<(u8, Vec<u8>)> {
    let messages = simple_query(socket, sql);
    assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
    assert_eq!(messages.last(), Some(&(b'Z', vec![state])), "{sql}");
    messages
}

fn pg10_rows(messages: &[(u8, Vec<u8>)]) -> Vec<Vec<u8>> {
    let mut rows = messages
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| m.1.clone())
        .collect::<Vec<_>>();
    rows.sort();
    rows
}

fn pg10_description(body: &[u8]) -> Vec<(String, u32, i16, i32, u32, i16)> {
    let count = i16::from_be_bytes(body[..2].try_into().unwrap());
    let mut offset = 2;
    let mut fields = Vec::new();
    for _ in 0..count {
        let end = offset + body[offset..].iter().position(|b| *b == 0).unwrap();
        let name = String::from_utf8(body[offset..end].to_vec()).unwrap();
        offset = end + 1;
        let oid = u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap());
        let len = i16::from_be_bytes(body[offset + 10..offset + 12].try_into().unwrap());
        let modifier = i32::from_be_bytes(body[offset + 12..offset + 16].try_into().unwrap());
        let table_oid = u32::from_be_bytes(body[offset..offset + 4].try_into().unwrap());
        let attribute = i16::from_be_bytes(body[offset + 4..offset + 6].try_into().unwrap());
        fields.push((name, oid, len, modifier, table_oid, attribute));
        offset += 18;
    }
    assert_eq!(offset, body.len());
    fields
}

#[test]
fn pg10_01_behavior() {
    let (domain, mysql) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    mysql.execute("CREATE DATABASE pg10_mysql").unwrap();
    mysql.execute("USE pg10_mysql").unwrap();
    mysql
        .execute("CREATE TABLE baseline(id INT PRIMARY KEY,v INT,note TEXT)")
        .unwrap();
    mysql
        .execute("INSERT INTO baseline VALUES (1,10,'before')")
        .unwrap();
    let (mysql_statement, _, _) = mysql
        .prepare_protocol_statement("SELECT v,note FROM baseline WHERE id = ?")
        .unwrap();
    mysql.execute("BEGIN").unwrap();
    mysql
        .execute("UPDATE baseline SET v=v+1 WHERE id=1")
        .unwrap();
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
    let service_guard = Pg10ServiceGuard(service);
    let mut w = pg10_connect(addr);
    let mut o = pg10_connect(addr);
    for c in [&mut w, &mut o] {
        let isolation = pg10_rows(&pg10_ok(
            c,
            "SELECT @@transaction_isolation AS isolation",
            b'I',
        ));
        assert_eq!(isolation.len(), 1);
        eprintln!(
            "pg10_01 actual isolation: {}",
            std::str::from_utf8(&isolation[0][6..]).unwrap()
        );
    }
    let scan = "SELECT id,v,note FROM public.pg10_01 ORDER BY id";
    for extended in [false, true] {
        for commit in [false, true] {
            for (sql, names, expected, stored, count) in [
                (
                    "INSERT INTO public.pg10_01(v,note) VALUES (7,NULL) RETURNING id AS generated_id,v,note",
                    vec!["generated_id", "v", "note"],
                    vec![text_row(&[Some("42"), Some("7"), None])],
                    vec![text_row(&[Some("42"), Some("7"), None])],
                    1,
                ),
                (
                    "INSERT INTO public.pg10_01 VALUES (1,7,NULL),(2,8,'b') RETURNING *",
                    vec!["id", "v", "note"],
                    vec![
                        text_row(&[Some("1"), Some("7"), None]),
                        text_row(&[Some("2"), Some("8"), Some("b")]),
                    ],
                    vec![
                        text_row(&[Some("1"), Some("7"), None]),
                        text_row(&[Some("2"), Some("8"), Some("b")]),
                    ],
                    2,
                ),
                (
                    "INSERT INTO public.pg10_01 VALUES (1,7,NULL) RETURNING id,v+100 AS projected,note",
                    vec!["id", "projected", "note"],
                    vec![text_row(&[Some("1"), Some("107"), None])],
                    vec![text_row(&[Some("1"), Some("7"), None])],
                    1,
                ),
                (
                    "INSERT INTO public.pg10_01(id,v,note) SELECT 1,7,NULL WHERE false RETURNING id,v,note",
                    vec!["id", "v", "note"],
                    vec![],
                    vec![],
                    0,
                ),
            ] {
                pg10_ok(
                    &mut w,
                    "CREATE TABLE public.pg10_01(id integer PRIMARY KEY DEFAULT (40 + 2),v integer,note text)",
                    b'I',
                );
                assert!(pg10_rows(&pg10_ok(&mut w, scan, b'I')).is_empty());
                assert!(pg10_rows(&pg10_ok(&mut o, scan, b'I')).is_empty());
                let mut description = None;
                if extended {
                    parse(&mut w, "pg10_s", sql, &[]);
                    assert_eq!(read(&mut w), (b'1', vec![]));
                    send(&mut w, b'D', b"Spg10_s\0");
                    assert_eq!(read(&mut w).0, b't');
                    let fields = read(&mut w);
                    assert_eq!(fields.0, b'T');
                    description = Some(fields.1);
                    sync(&mut w);
                    assert!(pg10_rows(&pg10_ok(&mut w, scan, b'I')).is_empty());
                    assert!(pg10_rows(&pg10_ok(&mut o, scan, b'I')).is_empty());
                }
                pg10_ok(&mut w, "BEGIN", b'T');
                let messages = if extended {
                    bind(&mut w, "pg10_p", "pg10_s", &[], &[], &[]);
                    assert_eq!(read(&mut w), (b'2', vec![]));
                    send(&mut w, b'D', b"Ppg10_p\0");
                    assert_eq!(read(&mut w), (b'T', description.clone().unwrap()));
                    send(&mut w, b'S', &[]);
                    assert_eq!(read(&mut w), (b'Z', b"T".to_vec()));
                    assert!(pg10_rows(&pg10_ok(&mut w, scan, b'T')).is_empty());
                    assert!(pg10_rows(&pg10_ok(&mut o, scan, b'I')).is_empty());
                    execute(&mut w, "pg10_p");
                    let mut messages = vec![];
                    loop {
                        let m = read(&mut w);
                        let done = m.0 == b'C' || m.0 == b'E';
                        messages.push(m);
                        if done {
                            break;
                        }
                    }
                    assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
                    send(&mut w, b'S', &[]);
                    assert_eq!(read(&mut w), (b'Z', b"T".to_vec()));
                    // A completed portal must not write again.
                    execute(&mut w, "pg10_p");
                    assert_eq!(
                        read(&mut w),
                        (b'C', format!("INSERT 0 {count}\0").into_bytes())
                    );
                    send(&mut w, b'S', &[]);
                    assert_eq!(read(&mut w), (b'Z', b"T".to_vec()));
                    send(&mut w, b'C', b"Ppg10_p\0");
                    assert_eq!(read(&mut w), (b'3', vec![]));
                    messages
                } else {
                    pg10_ok(&mut w, sql, b'T')
                };
                let fields = description
                    .unwrap_or_else(|| messages.iter().find(|m| m.0 == b'T').unwrap().1.clone());
                let actual = pg10_description(&fields);
                assert_eq!(
                    actual.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
                    names
                );
                assert_eq!(
                    actual.iter().map(|f| (f.1, f.2, f.3)).collect::<Vec<_>>(),
                    vec![(23, 4, -1), (23, 4, -1), (25, -1, -1)]
                );
                let mut target_oid = None;
                for field in &actual {
                    let attribute = match field.0.as_str() {
                        "id" | "generated_id" => 1,
                        "v" => 2,
                        "note" => 3,
                        "projected" => 0,
                        other => panic!("unexpected returned field: {other}"),
                    };
                    assert_eq!(field.5, attribute);
                    if attribute == 0 {
                        assert_eq!(field.4, 0);
                    } else {
                        assert_ne!(field.4, 0);
                        assert_eq!(*target_oid.get_or_insert(field.4), field.4);
                    }
                }

                let mut expected = expected;
                expected.sort();
                assert_eq!(pg10_rows(&messages), expected, "{sql}");
                assert!(
                    messages
                        .iter()
                        .any(|m| m == &(b'C', format!("INSERT 0 {count}\0").into_bytes()))
                );
                let mut stored = stored;
                stored.sort();
                assert_eq!(pg10_rows(&pg10_ok(&mut w, scan, b'T')), stored);
                assert!(pg10_rows(&pg10_ok(&mut o, scan, b'I')).is_empty());
                pg10_ok(&mut w, if commit { "COMMIT" } else { "ROLLBACK" }, b'I');
                for c in [&mut w, &mut o] {
                    assert_eq!(
                        pg10_rows(&pg10_ok(c, scan, b'I')),
                        if commit { stored.clone() } else { vec![] }
                    );
                }
                if extended {
                    send(&mut w, b'C', b"Spg10_s\0");
                    assert_eq!(read(&mut w), (b'3', vec![]));
                    sync(&mut w);
                }
                pg10_ok(&mut w, "DROP TABLE public.pg10_01", b'I');
            }
        }
    }
    mysql.execute("ROLLBACK").unwrap();
    assert_eq!(
        mysql
            .execute("SELECT v,note FROM baseline WHERE id=1")
            .unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        ["10", "before"]
    );
    mysql.close_protocol_statement(mysql_statement).unwrap();
    mysql.execute("DROP DATABASE pg10_mysql").unwrap();
    for extended in [false, true] {
        for transaction in [false, true] {
            pg10_ok(
                &mut w,
                "CREATE TABLE public.pg10_01(id integer PRIMARY KEY DEFAULT (40 + 2),v integer,note text)",
                b'I',
            );
            pg10_ok(
                &mut w,
                "INSERT INTO public.pg10_01 VALUES (1,10,'before')",
                b'I',
            );
            if transaction {
                pg10_ok(&mut w, "BEGIN", b'T');
                pg10_ok(
                    &mut w,
                    "INSERT INTO public.pg10_01 VALUES (3,30,'pending')",
                    b'T',
                );
            }
            let sql =
                "INSERT INTO public.pg10_01 VALUES (2,11,NULL) RETURNING 10 / (v - 11) AS boom";
            if extended {
                parse(&mut w, "pg10_error", sql, &[]);
                assert_eq!(read(&mut w), (b'1', vec![]));
                bind(&mut w, "pg10_error_p", "pg10_error", &[], &[], &[]);
                assert_eq!(read(&mut w), (b'2', vec![]));
                execute(&mut w, "pg10_error_p");
                let error = read(&mut w);
                assert_eq!(error.0, b'E', "{error:?}");
                assert_eq!(error_state(&error.1), Some("22012"), "{error:?}");
                send(&mut w, b'S', &[]);
                assert_eq!(
                    read(&mut w),
                    (b'Z', vec![if transaction { b'E' } else { b'I' }])
                );
            } else {
                let messages = simple_query(&mut w, sql);
                assert_eq!(messages[0].0, b'E');
                assert_eq!(error_state(&messages[0].1), Some("22012"), "{messages:?}");
                assert_eq!(
                    messages.last(),
                    Some(&(b'Z', vec![if transaction { b'E' } else { b'I' }]))
                );
            }
            assert_eq!(
                pg10_rows(&pg10_ok(&mut o, scan, b'I')),
                vec![text_row(&[Some("1"), Some("10"), Some("before")])]
            );
            if transaction {
                let aborted = simple_query(&mut w, "SELECT 1 AS ok");
                assert_eq!(error_state(&aborted[0].1), Some("25P02"));
                pg10_ok(&mut w, "ROLLBACK", b'I');
            }
            for c in [&mut w, &mut o] {
                assert_eq!(
                    pg10_rows(&pg10_ok(c, scan, b'I')),
                    vec![text_row(&[Some("1"), Some("10"), Some("before")])]
                );
                pg10_ok(c, "SELECT 1 AS ok", b'I');
            }
            if extended {
                send(&mut w, b'C', b"Spg10_error\0");
                assert_eq!(read(&mut w), (b'3', vec![]));
                sync(&mut w);
            }
            pg10_ok(&mut w, "DROP TABLE public.pg10_01", b'I');
        }
    }
    pg10_ok(
        &mut w,
        "CREATE TABLE public.pg10_01(id integer PRIMARY KEY,v integer,note text)",
        b'I',
    );
    parse(
        &mut w,
        "pg10_bind",
        "INSERT INTO public.pg10_01 VALUES ($1,$2,$3) RETURNING id,v,note",
        &[23, 23, 25],
    );
    assert_eq!(read(&mut w), (b'1', vec![]));
    for (id, v, note) in [("5", "7", None), ("6", "8", Some("next"))] {
        bind(
            &mut w,
            "pg10_bind_p",
            "pg10_bind",
            &[0, 0, 0],
            &[
                Some(id.as_bytes()),
                Some(v.as_bytes()),
                note.map(str::as_bytes),
            ],
            &[],
        );
        assert_eq!(read(&mut w), (b'2', vec![]));
        execute(&mut w, "pg10_bind_p");
        let mut messages = vec![];
        loop {
            let message = read(&mut w);
            let done = message.0 == b'C' || message.0 == b'E';
            messages.push(message);
            if done {
                break;
            }
        }
        assert_eq!(
            pg10_rows(&messages),
            vec![text_row(&[Some(id), Some(v), note])],
            "{messages:?}"
        );
        assert_eq!(messages.last(), Some(&(b'C', b"INSERT 0 1\0".to_vec())));
        sync(&mut w);
        send(&mut w, b'C', b"Ppg10_bind_p\0");
        assert_eq!(read(&mut w), (b'3', vec![]));
    }
    assert_eq!(
        pg10_rows(&pg10_ok(&mut o, scan, b'I')),
        vec![
            text_row(&[Some("5"), Some("7"), None]),
            text_row(&[Some("6"), Some("8"), Some("next")])
        ]
    );
    send(&mut w, b'C', b"Spg10_bind\0");
    assert_eq!(read(&mut w), (b'3', vec![]));
    sync(&mut w);
    pg10_ok(&mut w, "DROP TABLE public.pg10_01", b'I');
    send(&mut w, b'X', &[]);
    send(&mut o, b'X', &[]);
    drop(w);
    drop(o);
    service_guard.0.close();
    assert_eq!(service_guard.0.resource_counts(), (0, 0, 0));
    drop(service_guard);
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn pg10_02_behavior() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let guard = Pg10ServiceGuard(
        PgService::start(
            listener,
            driver,
            Arc::new(CanonicalConnectionDomain::new(domain)),
            false,
        )
        .unwrap(),
    );
    let mut w = pg10_connect(addr);
    let mut o = pg10_connect(addr);
    let scan = "SELECT id,v,note FROM public.pg10_02 ORDER BY id";
    let initial = vec![
        text_row(&[Some("1"), Some("10"), None]),
        text_row(&[Some("2"), Some("20"), Some("b")]),
    ];
    for c in [&mut w, &mut o] {
        eprintln!(
            "pg10_02 isolation {:?}",
            pg10_rows(&pg10_ok(
                c,
                "SELECT @@transaction_isolation AS isolation",
                b'I'
            ))
        );
    }
    for extended in [false, true] {
        for commit in [false, true] {
            for (sql, values, stored, names, count) in [
                (
                    "UPDATE public.pg10_02 AS a SET v=v+1 WHERE id=1 RETURNING a.id,v,note",
                    vec![text_row(&[Some("1"), Some("11"), None])],
                    vec![text_row(&[Some("1"), Some("11"), None]), initial[1].clone()],
                    vec!["id", "v", "note"],
                    1,
                ),
                (
                    "UPDATE public.pg10_02 SET v=v WHERE id=1 RETURNING *",
                    vec![initial[0].clone()],
                    initial.clone(),
                    vec!["id", "v", "note"],
                    1,
                ),
                (
                    "UPDATE public.pg10_02 SET v=99 WHERE false RETURNING id,v,note",
                    vec![],
                    initial.clone(),
                    vec!["id", "v", "note"],
                    0,
                ),
                (
                    "UPDATE public.pg10_02 SET v=99 WHERE id=9 RETURNING id,v,note",
                    vec![],
                    initial.clone(),
                    vec!["id", "v", "note"],
                    0,
                ),
                (
                    "UPDATE public.pg10_02 SET v=v+1 RETURNING id,v,note",
                    vec![
                        text_row(&[Some("1"), Some("11"), None]),
                        text_row(&[Some("2"), Some("21"), Some("b")]),
                    ],
                    vec![
                        text_row(&[Some("1"), Some("11"), None]),
                        text_row(&[Some("2"), Some("21"), Some("b")]),
                    ],
                    vec!["id", "v", "note"],
                    2,
                ),
                (
                    "UPDATE public.pg10_02 AS r SET v=v+1,note=NULL WHERE id=1 RETURNING r.id,r.v AS \"After\",r.note,r.v+100 AS projected",
                    vec![text_row(&[Some("1"), Some("11"), None, Some("111")])],
                    vec![text_row(&[Some("1"), Some("11"), None]), initial[1].clone()],
                    vec!["id", "After", "note", "projected"],
                    1,
                ),
            ] {
                pg10_ok(
                    &mut w,
                    "CREATE TABLE public.pg10_02(id integer PRIMARY KEY DEFAULT (40 + 2),v integer,note text)",
                    b'I',
                );
                pg10_ok(
                    &mut w,
                    "INSERT INTO public.pg10_02 VALUES (1,10,NULL),(2,20,'b')",
                    b'I',
                );
                for c in [&mut w, &mut o] {
                    assert_eq!(pg10_rows(&pg10_ok(c, scan, b'I')), initial);
                }
                let mut description = None;
                if extended {
                    parse(&mut w, "u", sql, &[]);
                    assert_eq!(read(&mut w), (b'1', vec![]));
                    send(&mut w, b'D', b"Su\0");
                    assert_eq!(read(&mut w).0, b't');
                    let d = read(&mut w);
                    assert_eq!(d.0, b'T');
                    description = Some(d.1);
                    sync(&mut w);
                    for c in [&mut w, &mut o] {
                        assert_eq!(pg10_rows(&pg10_ok(c, scan, b'I')), initial);
                    }
                }
                pg10_ok(&mut w, "BEGIN", b'T');
                let messages = if extended {
                    bind(&mut w, "p", "u", &[], &[], &[]);
                    assert_eq!(read(&mut w), (b'2', vec![]));
                    send(&mut w, b'D', b"Pp\0");
                    assert_eq!(read(&mut w), (b'T', description.clone().unwrap()));
                    send(&mut w, b'S', &[]);
                    assert_eq!(read(&mut w), (b'Z', vec![b'T']));
                    assert_eq!(pg10_rows(&pg10_ok(&mut w, scan, b'T')), initial);
                    execute(&mut w, "p");
                    let mut ms = vec![];
                    loop {
                        let m = read(&mut w);
                        let done = matches!(m.0, b'C' | b'E');
                        ms.push(m);
                        if done {
                            break;
                        }
                    }
                    assert!(!ms.iter().any(|m| m.0 == b'E'), "{sql}: {ms:?}");
                    send(&mut w, b'S', &[]);
                    assert_eq!(read(&mut w), (b'Z', vec![b'T']));
                    send(&mut w, b'C', b"Pp\0");
                    assert_eq!(read(&mut w), (b'3', vec![]));
                    ms
                } else {
                    pg10_ok(&mut w, sql, b'T')
                };
                let fields =
                    pg10_description(&description.unwrap_or_else(|| {
                        messages.iter().find(|m| m.0 == b'T').unwrap().1.clone()
                    }));
                assert_eq!(
                    fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
                    names
                );
                for (i, f) in fields.iter().enumerate() {
                    assert_eq!(
                        (f.1, f.2, f.3),
                        if i == 2 { (25, -1, -1) } else { (23, 4, -1) }
                    );
                    assert_eq!(f.5, if i == 3 { 0 } else { (i + 1) as i16 });
                    if i == 3 {
                        assert_eq!(f.4, 0);
                    } else {
                        assert_ne!(f.4, 0);
                        assert_eq!(f.4, fields[0].4);
                    }
                }
                let mut values = values;
                values.sort();
                assert_eq!(pg10_rows(&messages), values, "{sql}");
                assert!(messages.contains(&(b'C', format!("UPDATE {count}\0").into_bytes())));
                assert_eq!(pg10_rows(&pg10_ok(&mut w, scan, b'T')), stored);
                assert_eq!(pg10_rows(&pg10_ok(&mut o, scan, b'I')), initial);
                pg10_ok(&mut w, if commit { "COMMIT" } else { "ROLLBACK" }, b'I');
                for c in [&mut w, &mut o] {
                    assert_eq!(
                        pg10_rows(&pg10_ok(c, scan, b'I')),
                        if commit {
                            stored.clone()
                        } else {
                            initial.clone()
                        }
                    );
                }
                if extended {
                    send(&mut w, b'C', b"Su\0");
                    assert_eq!(read(&mut w), (b'3', vec![]));
                    sync(&mut w);
                }
                pg10_ok(&mut w, "DROP TABLE public.pg10_02", b'I');
            }
        }
    }
    for extended in [false, true] {
        for transaction in [false, true] {
            pg10_ok(
                &mut w,
                "CREATE TABLE public.pg10_02(id integer PRIMARY KEY,v integer,note text)",
                b'I',
            );
            pg10_ok(
                &mut w,
                "INSERT INTO public.pg10_02 VALUES (1,10,'before')",
                b'I',
            );
            if transaction {
                pg10_ok(&mut w, "BEGIN", b'T');
                pg10_ok(
                    &mut w,
                    "INSERT INTO public.pg10_02 VALUES (3,30,'pending')",
                    b'T',
                );
            }
            let sql = "UPDATE public.pg10_02 SET v=11 WHERE id=1 RETURNING 10/(v-11) AS boom";
            if extended {
                parse(&mut w, "err", sql, &[]);
                assert_eq!(read(&mut w), (b'1', vec![]));
                bind(&mut w, "ep", "err", &[], &[], &[]);
                assert_eq!(read(&mut w), (b'2', vec![]));
                execute(&mut w, "ep");
                let error = read(&mut w);
                assert_eq!(error.0, b'E');
                assert_eq!(error_state(&error.1), Some("22012"));
                send(&mut w, b'S', &[]);
                assert_eq!(
                    read(&mut w),
                    (b'Z', vec![if transaction { b'E' } else { b'I' }])
                );
            } else {
                let ms = simple_query(&mut w, sql);
                assert_eq!(ms[0].0, b'E');
                assert_eq!(error_state(&ms[0].1), Some("22012"));
                assert_eq!(
                    ms.last(),
                    Some(&(b'Z', vec![if transaction { b'E' } else { b'I' }]))
                );
            }
            assert_eq!(
                pg10_rows(&pg10_ok(&mut o, scan, b'I')),
                vec![text_row(&[Some("1"), Some("10"), Some("before")])]
            );
            if transaction {
                let aborted = simple_query(&mut w, "SELECT 1 AS ok");
                assert_eq!(error_state(&aborted[0].1), Some("25P02"));
                pg10_ok(&mut w, "ROLLBACK", b'I');
            }
            for c in [&mut w, &mut o] {
                assert_eq!(
                    pg10_rows(&pg10_ok(c, scan, b'I')),
                    vec![text_row(&[Some("1"), Some("10"), Some("before")])]
                );
                pg10_ok(c, "SELECT 1 AS ok", b'I');
            }
            if extended {
                send(&mut w, b'C', b"Serr\0");
                assert_eq!(read(&mut w), (b'3', vec![]));
                sync(&mut w);
            }
            pg10_ok(&mut w, "DROP TABLE public.pg10_02", b'I');
        }
    }
    pg10_ok(
        &mut w,
        "CREATE TABLE public.pg10_02(id integer PRIMARY KEY,v integer,note text)",
        b'I',
    );
    pg10_ok(
        &mut w,
        "INSERT INTO public.pg10_02 VALUES (1,10,'before')",
        b'I',
    );
    parse(
        &mut w,
        "typed",
        "UPDATE public.pg10_02 SET v=$1,note=$2 WHERE id=1 RETURNING id,v,note",
        &[23, 25],
    );
    assert_eq!(read(&mut w), (b'1', vec![]));
    for (v, note) in [("11", None), ("12", Some("next"))] {
        bind(
            &mut w,
            "tp",
            "typed",
            &[],
            &[Some(v.as_bytes()), note.map(str::as_bytes)],
            &[],
        );
        assert_eq!(read(&mut w), (b'2', vec![]));
        execute(&mut w, "tp");
        assert_eq!(read(&mut w), (b'D', text_row(&[Some("1"), Some(v), note])));
        assert_eq!(read(&mut w), (b'C', b"UPDATE 1\0".to_vec()));
        sync(&mut w);
        assert_eq!(
            pg10_rows(&pg10_ok(&mut o, scan, b'I')),
            vec![text_row(&[Some("1"), Some(v), note])]
        );
        send(&mut w, b'C', b"Ptp\0");
        assert_eq!(read(&mut w), (b'3', vec![]));
    }
    send(&mut w, b'C', b"Styped\0");
    assert_eq!(read(&mut w), (b'3', vec![]));
    sync(&mut w);
    for sql in [
        "UPDATE public.pg10_02 SET v=99 RETURNING missing",
        "UPDATE public.pg10_02 SET v=99 FROM public.pg10_02 AS other RETURNING id",
    ] {
        let ms = simple_query(&mut w, sql);
        assert_eq!(ms[0].0, b'E');
        assert_eq!(
            pg10_rows(&pg10_ok(&mut o, scan, b'I')),
            vec![text_row(&[Some("1"), Some("12"), Some("next")])]
        );
    }
    pg10_ok(&mut w, "DROP TABLE public.pg10_02", b'I');
    send(&mut w, b'X', &[]);
    send(&mut o, b'X', &[]);
    drop(w);
    drop(o);
    guard.0.close();
    assert_eq!(guard.0.resource_counts(), (0, 0, 0));
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn pg10_03_behavior() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let guard = Pg10ServiceGuard(
        PgService::start(
            listener,
            driver,
            Arc::new(CanonicalConnectionDomain::new(domain)),
            false,
        )
        .unwrap(),
    );
    let mut w = pg10_connect(addr);
    let mut o = pg10_connect(addr);
    let scan = "SELECT id,v,note FROM public.pg10_03 ORDER BY id";
    let initial = vec![
        text_row(&[Some("1"), Some("10"), None]),
        text_row(&[Some("2"), Some("20"), Some("b")]),
    ];
    for c in [&mut w, &mut o] {
        eprintln!(
            "pg10_03 isolation {:?}",
            pg10_rows(&pg10_ok(
                c,
                "SELECT @@transaction_isolation AS isolation",
                b'I'
            ))
        );
    }
    for extended in [false, true] {
        for commit in [false, true] {
            for (sql, values, stored, names, count) in [
                (
                    "DELETE FROM public.pg10_03 WHERE id=1 RETURNING id,v,note",
                    vec![initial[0].clone()],
                    vec![initial[1].clone()],
                    vec!["id", "v", "note"],
                    1,
                ),
                (
                    "DELETE FROM public.pg10_03 WHERE id=1 RETURNING *",
                    vec![initial[0].clone()],
                    vec![initial[1].clone()],
                    vec!["id", "v", "note"],
                    1,
                ),
                (
                    "DELETE FROM public.pg10_03 WHERE false RETURNING id,v,note",
                    vec![],
                    initial.clone(),
                    vec!["id", "v", "note"],
                    0,
                ),
                (
                    "DELETE FROM public.pg10_03 WHERE id=9 RETURNING id,v,note",
                    vec![],
                    initial.clone(),
                    vec!["id", "v", "note"],
                    0,
                ),
                (
                    "DELETE FROM public.pg10_03 RETURNING id,v,note",
                    initial.clone(),
                    vec![],
                    vec!["id", "v", "note"],
                    2,
                ),
                (
                    "DELETE FROM public.pg10_03 AS r WHERE id=1 RETURNING r.id,r.v AS \"Before\",r.note,r.v+100 AS projected",
                    vec![text_row(&[Some("1"), Some("10"), None, Some("110")])],
                    vec![initial[1].clone()],
                    vec!["id", "Before", "note", "projected"],
                    1,
                ),
            ] {
                pg10_ok(
                    &mut w,
                    "CREATE TABLE public.pg10_03(id integer PRIMARY KEY DEFAULT (40 + 2),v integer,note text)",
                    b'I',
                );
                pg10_ok(
                    &mut w,
                    "INSERT INTO public.pg10_03 VALUES (1,10,NULL),(2,20,'b')",
                    b'I',
                );
                for c in [&mut w, &mut o] {
                    assert_eq!(pg10_rows(&pg10_ok(c, scan, b'I')), initial);
                }
                let mut description = None;
                if extended {
                    parse(&mut w, "u", sql, &[]);
                    assert_eq!(read(&mut w), (b'1', vec![]));
                    send(&mut w, b'D', b"Su\0");
                    assert_eq!(read(&mut w).0, b't');
                    let d = read(&mut w);
                    assert_eq!(d.0, b'T');
                    description = Some(d.1);
                    sync(&mut w);
                    for c in [&mut w, &mut o] {
                        assert_eq!(pg10_rows(&pg10_ok(c, scan, b'I')), initial);
                    }
                }
                pg10_ok(&mut w, "BEGIN", b'T');
                let messages = if extended {
                    bind(&mut w, "p", "u", &[], &[], &[]);
                    assert_eq!(read(&mut w), (b'2', vec![]));
                    send(&mut w, b'D', b"Pp\0");
                    assert_eq!(read(&mut w), (b'T', description.clone().unwrap()));
                    send(&mut w, b'S', &[]);
                    assert_eq!(read(&mut w), (b'Z', vec![b'T']));
                    assert_eq!(pg10_rows(&pg10_ok(&mut w, scan, b'T')), initial);
                    execute(&mut w, "p");
                    let mut ms = vec![];
                    loop {
                        let m = read(&mut w);
                        let done = matches!(m.0, b'C' | b'E');
                        ms.push(m);
                        if done {
                            break;
                        }
                    }
                    assert!(!ms.iter().any(|m| m.0 == b'E'), "{sql}: {ms:?}");
                    send(&mut w, b'S', &[]);
                    assert_eq!(read(&mut w), (b'Z', vec![b'T']));
                    send(&mut w, b'C', b"Pp\0");
                    assert_eq!(read(&mut w), (b'3', vec![]));
                    ms
                } else {
                    pg10_ok(&mut w, sql, b'T')
                };
                let fields =
                    pg10_description(&description.unwrap_or_else(|| {
                        messages.iter().find(|m| m.0 == b'T').unwrap().1.clone()
                    }));
                assert_eq!(
                    fields.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
                    names
                );
                for (i, f) in fields.iter().enumerate() {
                    assert_eq!(
                        (f.1, f.2, f.3),
                        if i == 2 { (25, -1, -1) } else { (23, 4, -1) }
                    );
                    assert_eq!(f.5, if i == 3 { 0 } else { (i + 1) as i16 });
                    if i == 3 {
                        assert_eq!(f.4, 0);
                    } else {
                        assert_ne!(f.4, 0);
                        assert_eq!(f.4, fields[0].4);
                    }
                }
                let mut values = values;
                values.sort();
                assert_eq!(pg10_rows(&messages), values, "{sql}");
                assert!(messages.contains(&(b'C', format!("DELETE {count}\0").into_bytes())));
                assert_eq!(pg10_rows(&pg10_ok(&mut w, scan, b'T')), stored);
                assert_eq!(pg10_rows(&pg10_ok(&mut o, scan, b'I')), initial);
                pg10_ok(&mut w, if commit { "COMMIT" } else { "ROLLBACK" }, b'I');
                for c in [&mut w, &mut o] {
                    assert_eq!(
                        pg10_rows(&pg10_ok(c, scan, b'I')),
                        if commit {
                            stored.clone()
                        } else {
                            initial.clone()
                        }
                    );
                }
                if extended {
                    send(&mut w, b'C', b"Su\0");
                    assert_eq!(read(&mut w), (b'3', vec![]));
                    sync(&mut w);
                }
                pg10_ok(&mut w, "DROP TABLE public.pg10_03", b'I');
            }
        }
    }
    for extended in [false, true] {
        for transaction in [false, true] {
            pg10_ok(
                &mut w,
                "CREATE TABLE public.pg10_03(id integer PRIMARY KEY,v integer,note text)",
                b'I',
            );
            pg10_ok(
                &mut w,
                "INSERT INTO public.pg10_03 VALUES (1,10,'before')",
                b'I',
            );
            if transaction {
                pg10_ok(&mut w, "BEGIN", b'T');
                pg10_ok(
                    &mut w,
                    "INSERT INTO public.pg10_03 VALUES (3,30,'pending')",
                    b'T',
                );
            }
            let sql = "DELETE FROM public.pg10_03 WHERE id=1 RETURNING 10/(v-10) AS boom";
            if extended {
                parse(&mut w, "err", sql, &[]);
                assert_eq!(read(&mut w), (b'1', vec![]));
                bind(&mut w, "ep", "err", &[], &[], &[]);
                assert_eq!(read(&mut w), (b'2', vec![]));
                execute(&mut w, "ep");
                let error = read(&mut w);
                assert_eq!(error.0, b'E');
                assert_eq!(error_state(&error.1), Some("22012"));
                send(&mut w, b'S', &[]);
                assert_eq!(
                    read(&mut w),
                    (b'Z', vec![if transaction { b'E' } else { b'I' }])
                );
            } else {
                let ms = simple_query(&mut w, sql);
                assert_eq!(ms[0].0, b'E');
                assert_eq!(error_state(&ms[0].1), Some("22012"));
                assert_eq!(
                    ms.last(),
                    Some(&(b'Z', vec![if transaction { b'E' } else { b'I' }]))
                );
            }
            assert_eq!(
                pg10_rows(&pg10_ok(&mut o, scan, b'I')),
                vec![text_row(&[Some("1"), Some("10"), Some("before")])]
            );
            if transaction {
                let aborted = simple_query(&mut w, "SELECT 1 AS ok");
                assert_eq!(error_state(&aborted[0].1), Some("25P02"));
                pg10_ok(&mut w, "ROLLBACK", b'I');
            }
            for c in [&mut w, &mut o] {
                assert_eq!(
                    pg10_rows(&pg10_ok(c, scan, b'I')),
                    vec![text_row(&[Some("1"), Some("10"), Some("before")])]
                );
                pg10_ok(c, "SELECT 1 AS ok", b'I');
            }
            if extended {
                send(&mut w, b'C', b"Serr\0");
                assert_eq!(read(&mut w), (b'3', vec![]));
                sync(&mut w);
            }
            pg10_ok(&mut w, "DROP TABLE public.pg10_03", b'I');
        }
    }
    pg10_ok(
        &mut w,
        "CREATE TABLE public.pg10_03(id integer PRIMARY KEY,v integer,note text)",
        b'I',
    );
    pg10_ok(
        &mut w,
        "INSERT INTO public.pg10_03 VALUES (1,10,'before')",
        b'I',
    );
    parse(
        &mut w,
        "typed",
        "DELETE FROM public.pg10_03 WHERE id=$1 RETURNING id,v,note",
        &[23],
    );
    assert_eq!(read(&mut w), (b'1', vec![]));
    for id in [None, Some("1")] {
        bind(&mut w, "tp", "typed", &[], &[id.map(str::as_bytes)], &[]);
        assert_eq!(read(&mut w), (b'2', vec![]));
        execute(&mut w, "tp");
        if id.is_some() {
            assert_eq!(
                read(&mut w),
                (b'D', text_row(&[Some("1"), Some("10"), Some("before")]))
            );
        }
        assert_eq!(
            read(&mut w),
            (
                b'C',
                if id.is_some() {
                    b"DELETE 1\0".to_vec()
                } else {
                    b"DELETE 0\0".to_vec()
                }
            )
        );
        sync(&mut w);
        assert_eq!(
            pg10_rows(&pg10_ok(&mut o, scan, b'I')),
            if id.is_some() {
                vec![]
            } else {
                vec![text_row(&[Some("1"), Some("10"), Some("before")])]
            }
        );
        send(&mut w, b'C', b"Ptp\0");
        assert_eq!(read(&mut w), (b'3', vec![]));
    }
    pg10_ok(
        &mut w,
        "INSERT INTO public.pg10_03 VALUES (1,10,'before')",
        b'I',
    );
    send(&mut w, b'C', b"Styped\0");
    assert_eq!(read(&mut w), (b'3', vec![]));
    sync(&mut w);
    for sql in [
        "DELETE FROM public.pg10_03 RETURNING missing",
        "DELETE FROM public.pg10_03 USING public.pg10_03 AS other WHERE pg10_03.id=other.id RETURNING pg10_03.id",
    ] {
        let ms = simple_query(&mut w, sql);
        assert_eq!(ms[0].0, b'E');
        assert_eq!(
            pg10_rows(&pg10_ok(&mut o, scan, b'I')),
            vec![text_row(&[Some("1"), Some("10"), Some("before")])]
        );
    }
    pg10_ok(&mut w, "DROP TABLE public.pg10_03", b'I');
    send(&mut w, b'X', &[]);
    send(&mut o, b'X', &[]);
    drop(w);
    drop(o);
    guard.0.close();
    assert_eq!(guard.0.resource_counts(), (0, 0, 0));
    assert!(TcpStream::connect(addr).is_err());
}
