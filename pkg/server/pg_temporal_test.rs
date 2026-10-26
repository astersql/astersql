// Copyright 2026 AsterSQL.
use crate::pg_conn::PgService;
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

fn send(socket: &mut TcpStream, tag: u8, body: &[u8]) {
    socket.write_all(&[tag]).unwrap();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(body).unwrap();
}
fn read(socket: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0; 5];
    socket.read_exact(&mut header).unwrap();
    let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    assert!((4..1 << 20).contains(&length));
    let mut body = vec![0; length - 4];
    socket.read_exact(&mut body).unwrap();
    (header[0], body)
}
fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
    let mut messages = Vec::new();
    loop {
        let message = read(socket);
        let done = message.0 == b'Z';
        messages.push(message);
        if done {
            return messages;
        }
    }
}
fn query(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
    send(socket, b'Q', &[sql.as_bytes(), b"\0"].concat());
    ready(socket)
}
fn cycle(
    socket: &mut TcpStream,
    sql: &str,
    input: i16,
    output: i16,
    value: Option<&[u8]>,
) -> Vec<(u8, Vec<u8>)> {
    let mut parse = [b"\0".as_slice(), sql.as_bytes(), b"\0"].concat();
    parse.extend_from_slice(&1i16.to_be_bytes());
    parse.extend_from_slice(&1082u32.to_be_bytes());
    send(socket, b'P', &parse);
    send(socket, b'D', b"S\0");
    let mut bind = b"\0\0".to_vec();
    bind.extend_from_slice(&1i16.to_be_bytes());
    bind.extend_from_slice(&input.to_be_bytes());
    bind.extend_from_slice(&1i16.to_be_bytes());
    if let Some(value) = value {
        bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
        bind.extend_from_slice(value);
    } else {
        bind.extend_from_slice(&(-1i32).to_be_bytes());
    }
    bind.extend_from_slice(&1i16.to_be_bytes());
    bind.extend_from_slice(&output.to_be_bytes());
    send(socket, b'B', &bind);
    send(socket, b'D', b"P\0");
    send(socket, b'E', &[0, 0, 0, 0, 0]);
    // A second Execute must not emit the row twice.
    send(socket, b'E', &[0, 0, 0, 0, 0]);
    send(socket, b'S', &[]);
    ready(socket)
}
fn description(body: &[u8], format: i16) {
    assert_eq!(&body[..2], &1i16.to_be_bytes());
    let at = 2 + body[2..].iter().position(|b| *b == 0).unwrap() + 1;
    assert_eq!(&body[at + 6..at + 10], &1082u32.to_be_bytes());
    assert_eq!(&body[at + 10..at + 12], &4i16.to_be_bytes());
    assert_eq!(&body[at + 12..at + 16], &(-1i32).to_be_bytes());
    assert_eq!(&body[at + 16..at + 18], &format.to_be_bytes());
}
fn error(messages: &[(u8, Vec<u8>)], state: &str) {
    error_in_state(messages, state, b"I");
}
fn error_in_state(messages: &[(u8, Vec<u8>)], state: &str, status: &[u8]) {
    let errors = messages.iter().filter(|m| m.0 == b'E').collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "{messages:?}");
    assert!(
        errors[0]
            .1
            .windows(7)
            .any(|w| w == [b"C", state.as_bytes(), b"\0"].concat()),
        "{messages:?}"
    );
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap(), &(b'Z', status.to_vec()));
}
#[test]
fn pg_temporal_date_calendar() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver.clone(),
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    ready(&mut socket);
    for sql in ["SELECT $1", "SELECT CAST($1 AS DATE)"] {
        for input in [0, 1] {
            for output in [0, 1] {
                // Typed NULL is first, independently of any observed non-NULL row.
                for text in [
                    None,
                    Some("2024-02-29"),
                    Some("2000-02-29"),
                    Some("2000-01-01"),
                    Some("1999-12-31"),
                ] {
                    let bytes = text.map(|text| {
                        if input == 0 {
                            text.as_bytes().to_vec()
                        } else {
                            let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap();
                            (date
                                .signed_duration_since(
                                    chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap(),
                                )
                                .num_days() as i32)
                                .to_be_bytes()
                                .to_vec()
                        }
                    });
                    let messages = cycle(&mut socket, sql, input, output, bytes.as_deref());
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        b"1tT2TDCCZ",
                        "{sql}, {input}/{output}, {text:?}: {messages:?}"
                    );
                    assert_eq!(
                        messages[1].1,
                        [1i16.to_be_bytes().as_slice(), &1082u32.to_be_bytes()].concat()
                    );
                    description(&messages[2].1, 0);
                    description(&messages[4].1, output);
                    let mut expected = 1i16.to_be_bytes().to_vec();
                    if let Some(text) = text {
                        let value = if output == 0 {
                            text.as_bytes().to_vec()
                        } else {
                            let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap();
                            (date
                                .signed_duration_since(
                                    chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap(),
                                )
                                .num_days() as i32)
                                .to_be_bytes()
                                .to_vec()
                        };
                        expected.extend_from_slice(&(value.len() as i32).to_be_bytes());
                        expected.extend_from_slice(&value);
                    } else {
                        expected.extend_from_slice(&(-1i32).to_be_bytes());
                    }
                    assert_eq!(messages[5].1, expected);
                }
            }
        }
    }
    for input in [0, 1] {
        for output in [0, 1] {
            let empty = cycle(
                &mut socket,
                "SELECT CAST($1 AS DATE) WHERE FALSE",
                input,
                output,
                None,
            );
            assert_eq!(empty.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tT2TCCZ");
            description(&empty[2].1, 0);
            description(&empty[4].1, output);
            for text in [None, Some("2000-02-29")] {
                let bytes = text.map(|text| {
                    if input == 0 {
                        text.as_bytes().to_vec()
                    } else {
                        59i32.to_be_bytes().to_vec()
                    }
                });
                let rows = cycle(
                    &mut socket,
                    "SELECT CAST($1 AS DATE) UNION ALL SELECT CAST($1 AS DATE)",
                    input,
                    output,
                    bytes.as_deref(),
                );
                assert_eq!(
                    rows.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"1tT2TDDCCZ",
                    "{rows:?}"
                );
                description(&rows[2].1, 0);
                description(&rows[4].1, output);
                assert_eq!(rows[5].1, rows[6].1);
                if text.is_none() {
                    assert_eq!(
                        rows[5].1,
                        [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
                    );
                } else {
                    let value = if output == 0 {
                        b"2000-02-29".to_vec()
                    } else {
                        59i32.to_be_bytes().to_vec()
                    };
                    assert_eq!(
                        rows[5].1,
                        [
                            1i16.to_be_bytes().as_slice(),
                            &(value.len() as i32).to_be_bytes(),
                            &value
                        ]
                        .concat()
                    );
                }
            }
        }
    }
    for text in [
        "2023-02-29",
        "1900-02-29",
        "2024-04-31",
        "2024-00-01",
        "2024-13-01",
        "2024-01-00",
        "2024-01-32",
        "0000-00-00",
    ] {
        for output in [0, 1] {
            error(
                &cycle(&mut socket, "SELECT $1", 0, output, Some(text.as_bytes())),
                "22008",
            );
            let recovery = query(&mut socket, "SELECT 1::int4");
            assert_eq!(recovery.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
        }
        error(
            &query(&mut socket, &format!("SELECT CAST('{text}' AS DATE)")),
            "22008",
        );
    }
    for (value, state) in [
        (b"".as_slice(), "22007"),
        (b"nonsense", "22007"),
        (b"2024-02-29\0", "22021"),
        (&[255], "22021"),
    ] {
        error(&cycle(&mut socket, "SELECT $1", 0, 0, Some(value)), state);
    }
    for (value, state) in [
        (vec![], "08P01"),
        (vec![0; 3], "08P01"),
        (vec![0; 5], "22P03"),
    ] {
        error(&cycle(&mut socket, "SELECT $1", 1, 0, Some(&value)), state);
    }
    for output in [0, 1] {
        assert_eq!(
            query(&mut socket, "BEGIN").last().unwrap(),
            &(b'Z', b"T".to_vec())
        );
        let failed = cycle(&mut socket, "SELECT $1", 0, output, Some(b"2023-02-29"));
        error_in_state(&failed, "22008", b"E");
        assert_eq!(
            query(&mut socket, "ROLLBACK").last().unwrap(),
            &(b'Z', b"I".to_vec())
        );
        assert_eq!(
            query(&mut socket, "SELECT 1::int4")
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"TDCZ"
        );
    }
    // Corrupt the internal result deliberately: both formats must validate it.
    use crate::conn::{CancellationToken, SessionDriver, Value};
    let context = driver.open_ctx(75091, 0, 45, "", None).unwrap();
    let mut result = context
        .execute_query(
            "SELECT CAST('2024-02-29' AS DATE)",
            false,
            &CancellationToken::new(),
        )
        .unwrap()
        .remove(0);
    result.rows = vec![vec![Value::Text("2023-02-29".into())]];
    for format in [0, 1] {
        let error = crate::pg_result::encode_formats(&result, "SELECT", &[format]).unwrap_err();
        assert_eq!(crate::pg_result::encoding_sqlstate(&error), "22008");
    }
    context.close().unwrap();
    send(&mut socket, b'X', &[]);
    service.close();
}
