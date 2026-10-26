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
    cycle_oid(socket, sql, 1082, input, output, value)
}
fn cycle_oid(
    socket: &mut TcpStream,
    sql: &str,
    oid: u32,
    input: i16,
    output: i16,
    value: Option<&[u8]>,
) -> Vec<(u8, Vec<u8>)> {
    let mut parse = [b"\0".as_slice(), sql.as_bytes(), b"\0"].concat();
    parse.extend_from_slice(&1i16.to_be_bytes());
    parse.extend_from_slice(&oid.to_be_bytes());
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

fn date_text_row(messages: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let row = &messages
        .iter()
        .find(|m| m.0 == b'D')
        .unwrap_or_else(|| panic!("{messages:?}"))
        .1;
    assert_eq!(&row[..2], &1i16.to_be_bytes());
    row[6..].to_vec()
}

#[test]
fn pg_temporal_date_special_style() {
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [
        196610u32.to_be_bytes().as_slice(),
        b"user\0root\0TimeZone\0Asia/Shanghai\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    ready(&mut socket);
    for sql in ["SELECT $1", "SELECT CAST($1 AS DATE)"] {
        for input in [0, 1] {
            for output in [0, 1] {
                for (text, days) in [
                    ("-infinity", i32::MIN),
                    ("infinity", i32::MAX),
                    ("1970-01-01", -10957),
                    ("2000-01-01", 0),
                ] {
                    let binary = days.to_be_bytes();
                    let value = if input == 0 { text.as_bytes() } else { &binary };
                    let result = cycle(&mut socket, sql, input, output, Some(value));
                    assert_eq!(
                        result.iter().map(|m| m.0).collect::<Vec<_>>(),
                        b"1tT2TDCCZ",
                        "{result:?}"
                    );
                    assert_eq!(
                        result[1].1,
                        [1i16.to_be_bytes().as_slice(), &1082u32.to_be_bytes()].concat()
                    );
                    description(&result[2].1, 0);
                    description(&result[4].1, output);
                    let expected = if output == 0 {
                        text.as_bytes()
                    } else {
                        &binary
                    };
                    assert_eq!(&result[5].1[6..], expected);
                }
                let null = cycle(&mut socket, sql, input, output, None);
                assert_eq!(null.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tT2TDCCZ");
                description(&null[4].1, output);
                assert_eq!(
                    null[5].1,
                    [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
                );
            }
        }
    }
    assert_eq!(
        date_text_row(&query(&mut socket, "SELECT CAST('epoch' AS DATE)")),
        b"1970-01-01"
    );
    for (order, expected) in [("MDY", "2003-01-02"), ("DMY", "2003-02-01")] {
        let set = query(&mut socket, &format!("SET DateStyle TO 'ISO, {order}'"));
        assert!(!set.iter().any(|m| m.0 == b'E'), "{set:?}");
        assert_eq!(
            date_text_row(&query(&mut socket, "SHOW DateStyle")),
            format!("ISO, {order}").as_bytes()
        );
        assert_eq!(
            date_text_row(&query(&mut socket, "SELECT CAST('01/02/2003' AS DATE)")),
            expected.as_bytes()
        );
        for output in [0, 1] {
            let result = cycle(&mut socket, "SELECT $1", 0, output, Some(b"01/02/2003"));
            let binary = expected
                .parse::<astersql_types::pg_temporal::PgDate>()
                .unwrap()
                .to_binary();
            let expected = if output == 0 {
                expected.as_bytes()
            } else {
                &binary
            };
            assert_eq!(date_text_row(&result), expected);
        }
        assert_eq!(
            date_text_row(&query(
                &mut socket,
                &format!("SELECT CAST('{expected}' AS DATE)")
            )),
            expected.as_bytes()
        );
    }
    for bad in ["allballs", "", "garbage"] {
        for output in [0, 1] {
            error(
                &cycle(&mut socket, "SELECT $1", 0, output, Some(bad.as_bytes())),
                "22007",
            );
            assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
        }
        error(
            &query(&mut socket, &format!("SELECT CAST('{bad}' AS DATE)")),
            "22007",
        );
    }
    // Fixed UTC 1999-12-31 23:00 belongs to 2000-01-01 in the session zone.
    for sql in ["SET timestamp = 946681200", "BEGIN"] {
        let result = query(&mut socket, sql);
        assert!(!result.iter().any(|m| m.0 == b'E'), "{result:?}");
    }
    for (word, expected) in [
        ("now", "2000-01-01"),
        ("today", "2000-01-01"),
        ("yesterday", "1999-12-31"),
        ("tomorrow", "2000-01-02"),
        ("epoch", "1970-01-01"),
    ] {
        assert_eq!(
            date_text_row(&query(
                &mut socket,
                &format!("SELECT CAST('{word}' AS DATE)")
            )),
            expected.as_bytes()
        );
        for input in [0, 1] {
            for output in [0, 1] {
                let binary = expected
                    .parse::<astersql_types::pg_temporal::PgDate>()
                    .unwrap()
                    .to_binary();
                let value = if input == 0 { word.as_bytes() } else { &binary };
                let result = cycle(&mut socket, "SELECT $1", input, output, Some(value));
                description(&result[2].1, 0);
                description(&result[4].1, output);
                assert_eq!(
                    date_text_row(&result),
                    if output == 0 {
                        expected.as_bytes()
                    } else {
                        &binary
                    }
                );
            }
        }
    }
    query(&mut socket, "SET timestamp = 946854000");
    assert_eq!(
        date_text_row(&query(&mut socket, "SELECT CAST('today' AS DATE)")),
        b"2000-01-01"
    );
    query(&mut socket, "COMMIT");
    assert_eq!(
        date_text_row(&query(&mut socket, "SELECT CAST('today' AS DATE)")),
        b"2000-01-03"
    );
    send(
        &mut socket,
        b'P',
        &[
            b"constant\0SELECT CAST('today' AS DATE)\0".as_slice(),
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', &[]);
    assert_eq!(ready(&mut socket)[0].0, b'1');
    query(&mut socket, "SET timestamp = 947026800");
    send(
        &mut socket,
        b'B',
        &[
            b"\0constant\0".as_slice(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
    send(&mut socket, b'S', &[]);
    assert_eq!(date_text_row(&ready(&mut socket)), b"2000-01-03");
    assert_eq!(
        date_text_row(&query(&mut socket, "SELECT CAST('today' AS DATE)")),
        b"2000-01-05"
    );
    // The OID1082 parameter is resolved at Bind, after Parse. A subsequent
    // zone change before Execute must not reinterpret the bound day.
    send(
        &mut socket,
        b'P',
        &[
            b"relative\0SELECT $1\0".as_slice(),
            &1i16.to_be_bytes(),
            &1082u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', &[]);
    assert_eq!(ready(&mut socket)[0].0, b'1');
    query(&mut socket, "SET timestamp = 947199600");
    query(&mut socket, "BEGIN");
    send(
        &mut socket,
        b'B',
        &[
            b"bound\0relative\0".as_slice(),
            &1i16.to_be_bytes(),
            &0i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &5i32.to_be_bytes(),
            b"today",
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Pbound\0");
    send(&mut socket, b'H', &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    let described = read(&mut socket);
    assert_eq!(described.0, b'T');
    description(&described.1, 1);
    query(&mut socket, "SET time_zone = 'UTC'");
    assert_eq!(
        date_text_row(&query(&mut socket, "SELECT CAST('today' AS DATE)")),
        b"2000-01-06"
    );
    for _ in 0..2 {
        send(
            &mut socket,
            b'E',
            &[b"bound\0".as_slice(), &0i32.to_be_bytes()].concat(),
        );
    }
    send(&mut socket, b'S', &[]);
    let result = ready(&mut socket);
    assert_eq!(result.iter().map(|m| m.0).collect::<Vec<_>>(), b"DCCZ");
    assert_eq!(date_text_row(&result), 6i32.to_be_bytes());
    error_in_state(
        &cycle(&mut socket, "SELECT $1", 0, 0, Some(b"allballs")),
        "22007",
        b"E",
    );
    query(&mut socket, "ROLLBACK");
    assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    query(&mut socket, "RESET DateStyle");
    assert_eq!(
        date_text_row(&query(&mut socket, "SHOW DateStyle")),
        b"ISO, MDY"
    );
    send(&mut socket, b'X', &[]);
    service.close();
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

#[test]
fn pg_temporal_date_epoch() {
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
    // Explicit wire fixtures are independent of PgDate and chrono arithmetic.
    for (text, bytes) in [
        ("1999-12-31", [0xff, 0xff, 0xff, 0xff]),
        ("2000-01-01", [0, 0, 0, 0]),
        ("2000-01-02", [0, 0, 0, 1]),
        ("1970-01-01", (-10957i32).to_be_bytes()),
    ] {
        for input in [0, 1] {
            for output in [0, 1] {
                let value = if input == 0 { text.as_bytes() } else { &bytes };
                let messages = cycle(
                    &mut socket,
                    "SELECT CAST($1 AS DATE)",
                    input,
                    output,
                    Some(value),
                );
                assert_eq!(
                    messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"1tT2TDCCZ"
                );
                assert_eq!(
                    messages[1].1,
                    [1i16.to_be_bytes().as_slice(), &1082u32.to_be_bytes()].concat()
                );
                description(&messages[2].1, 0);
                description(&messages[4].1, output);
                let expected = if output == 0 { text.as_bytes() } else { &bytes };
                assert_eq!(
                    messages[5].1,
                    [
                        1i16.to_be_bytes().as_slice(),
                        &(expected.len() as i32).to_be_bytes(),
                        expected
                    ]
                    .concat()
                );
            }
        }
    }
    // Bind the date once. Changing the real session timezone must not move
    // the value already retained by the named portal, even across midnight.
    for (index, zone) in ["Pacific/Kiritimati", "America/Los_Angeles"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            query(&mut socket, "BEGIN").last().unwrap(),
            &(b'Z', b"T".to_vec())
        );
        let portal = format!("epoch{index}");
        let mut parse = [portal.as_bytes(), b"\0SELECT CAST($1 AS DATE)\0"].concat();
        parse.extend_from_slice(&1i16.to_be_bytes());
        parse.extend_from_slice(&1082u32.to_be_bytes());
        send(&mut socket, b'P', &parse);
        let mut bind = [portal.as_bytes(), b"\0", portal.as_bytes(), b"\0"].concat();
        bind.extend_from_slice(&0i16.to_be_bytes());
        bind.extend_from_slice(&1i16.to_be_bytes());
        bind.extend_from_slice(&10i32.to_be_bytes());
        bind.extend_from_slice(b"1970-01-01");
        bind.extend_from_slice(&1i16.to_be_bytes());
        bind.extend_from_slice(&1i16.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            b"12Z"
        );
        // The established session TimeZone interface uses the native variable
        // name time_zone; full PostgreSQL GUC spelling/lifecycle is separate.
        let changed = query(&mut socket, &format!("SET time_zone = '{zone}'"));
        assert_eq!(
            changed.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"CZ",
            "{changed:?}"
        );
        send(
            &mut socket,
            b'D',
            &[b"P".as_slice(), portal.as_bytes(), b"\0"].concat(),
        );
        send(&mut socket, b'E', &[portal.as_bytes(), &[0; 5]].concat());
        send(&mut socket, b'S', &[]);
        let result = ready(&mut socket);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{result:?}"
        );
        description(&result[0].1, 1);
        assert_eq!(
            result[1].1,
            [
                1i16.to_be_bytes().as_slice(),
                &4i32.to_be_bytes(),
                &(-10957i32).to_be_bytes()
            ]
            .concat()
        );
        assert_eq!(
            query(&mut socket, "COMMIT").last().unwrap(),
            &(b'Z', b"I".to_vec())
        );
    }
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_temporal_date_range() {
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
    // Fixed DATE-RANGE contract fixtures, without using PgDate to calculate
    // expected bytes. The adjacent BC/AD days must never display year zero.
    for (text, days) in [
        ("4714-11-24 BC", -2451545i32),
        ("5874897-12-31", 2145031948),
        ("0001-01-01 BC", -730485),
        ("0001-12-31 BC", -730120),
        ("0001-01-01", -730119),
        ("9999-12-31", 2921939),
        ("10000-01-01", 2921940),
        ("2000-01-01", 0),
    ] {
        let binary = days.to_be_bytes();
        for sql in ["SELECT $1", "SELECT CAST($1 AS DATE)"] {
            for input in [0, 1] {
                for output in [0, 1] {
                    let value = if input == 0 { text.as_bytes() } else { &binary };
                    let messages = cycle(&mut socket, sql, input, output, Some(value));
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        b"1tT2TDCCZ",
                        "{sql}, {text}, {input}/{output}: {messages:?}"
                    );
                    assert_eq!(
                        messages[1].1,
                        [1i16.to_be_bytes().as_slice(), &1082u32.to_be_bytes()].concat()
                    );
                    description(&messages[2].1, 0);
                    description(&messages[4].1, output);
                    let expected = if output == 0 {
                        text.as_bytes()
                    } else {
                        &binary
                    };
                    assert_eq!(
                        messages[5].1,
                        [
                            1i16.to_be_bytes().as_slice(),
                            &(expected.len() as i32).to_be_bytes(),
                            expected
                        ]
                        .concat()
                    );
                }
            }
        }
        // Literals use the real SQL cast path, separately from Bind decoding.
        let literal = query(&mut socket, &format!("SELECT CAST('{text}' AS DATE)"));
        assert_eq!(
            literal.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{literal:?}"
        );
        description(&literal[0].1, 0);
        assert_eq!(
            literal[1].1,
            [
                1i16.to_be_bytes().as_slice(),
                &(text.len() as i32).to_be_bytes(),
                text.as_bytes()
            ]
            .concat()
        );
    }
    for input in [0, 1] {
        for output in [0, 1] {
            for sql in [
                "SELECT CAST($1 AS DATE)",
                "SELECT CAST($1 AS DATE) WHERE FALSE",
                "SELECT CAST($1 AS DATE) UNION ALL SELECT CAST($1 AS DATE)",
            ] {
                let messages = cycle(&mut socket, sql, input, output, None);
                assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
                description(&messages[2].1, 0);
                description(&messages[4].1, output);
                let rows = messages.iter().filter(|m| m.0 == b'D').collect::<Vec<_>>();
                let count = if sql.contains("FALSE") {
                    0
                } else if sql.contains("UNION") {
                    2
                } else {
                    1
                };
                assert_eq!(rows.len(), count);
                for row in rows {
                    assert_eq!(
                        row.1,
                        [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
                    );
                }
            }
        }
    }
    for (text, days) in [
        ("4714-11-23 BC", -2451546i32),
        ("5874898-01-01", 2145031949),
    ] {
        for input in [0, 1] {
            for output in [0, 1] {
                let binary = days.to_be_bytes();
                let value = if input == 0 { text.as_bytes() } else { &binary };
                error(
                    &cycle(
                        &mut socket,
                        "SELECT CAST($1 AS DATE)",
                        input,
                        output,
                        Some(value),
                    ),
                    "22008",
                );
                assert_eq!(
                    query(&mut socket, "SELECT 1::int4")
                        .iter()
                        .map(|m| m.0)
                        .collect::<Vec<_>>(),
                    b"TDCZ"
                );
            }
        }
        error(
            &query(&mut socket, &format!("SELECT CAST('{text}' AS DATE)")),
            "22008",
        );
    }
    for text in ["0000-01-01", "0000-01-01 BC"] {
        for output in [0, 1] {
            assert_eq!(
                query(&mut socket, "BEGIN").last().unwrap(),
                &(b'Z', b"T".to_vec())
            );
            error_in_state(
                &cycle(&mut socket, "SELECT $1", 0, output, Some(text.as_bytes())),
                "22008",
                b"E",
            );
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
    }
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_temporal_date_zero() {
    use crate::conn::{CancellationToken, SessionDriver};
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

    let mysql = driver.open_ctx(75094, 0, 45, "", None).unwrap();
    let cancel = CancellationToken::new();
    // Fix the native policy explicitly: zero components remain legal in MySQL.
    mysql
        .execute_query("SET sql_mode = ''", false, &cancel)
        .unwrap();
    for text in ["0000-00-00", "0000-01-01", "2024-00-01", "2024-01-00"] {
        let sql = format!("SELECT CAST('{text}' AS DATE)");
        let native = mysql.execute_query(&sql, false, &cancel).unwrap().remove(0);
        assert_eq!(
            native.rows,
            vec![vec![crate::conn::Value::Text(text.into())]]
        );
        assert_eq!(native.native_types[0].code, 10);
        let warnings = mysql.state().warning_count;
        assert_eq!(warnings, 0);
        for output in [0, 1] {
            // Use the actual MySQL result, without synthesizing values/metadata.
            let rejected =
                crate::pg_result::encode_formats(&native, "SELECT", &[output]).unwrap_err();
            assert_eq!(crate::pg_result::encoding_sqlstate(&rejected), "22008");
            assert_eq!(mysql.state().warning_count, warnings);
            for sql in ["SELECT $1", "SELECT CAST($1 AS DATE)"] {
                error(
                    &cycle(&mut socket, sql, 0, output as i16, Some(text.as_bytes())),
                    "22008",
                );
                assert_eq!(
                    query(&mut socket, "SELECT 1::int4")
                        .iter()
                        .map(|m| m.0)
                        .collect::<Vec<_>>(),
                    b"TDCZ"
                );
            }
        }
        error(&query(&mut socket, &sql), "22008");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4")
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"TDCZ"
        );
        // Re-execute after PG rejection to prove native policy did not change.
        assert_eq!(
            mysql.execute_query(&sql, false, &cancel).unwrap()[0].rows,
            native.rows
        );
        assert_eq!(mysql.state().warning_count, warnings);
    }
    // Zero binary day count is the valid PG epoch, distinct from MySQL zero date.
    for input in [0, 1] {
        for output in [0, 1] {
            for value in [
                None,
                Some(if input == 0 {
                    b"2000-01-01".as_slice()
                } else {
                    &[0; 4]
                }),
            ] {
                let result = cycle(&mut socket, "SELECT $1", input, output, value);
                assert_eq!(result.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tT2TDCCZ");
                assert_eq!(
                    result[1].1,
                    [1i16.to_be_bytes().as_slice(), &1082u32.to_be_bytes()].concat()
                );
                description(&result[2].1, 0);
                description(&result[4].1, output);
                let expected = match value {
                    None => [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat(),
                    Some(_) => {
                        let bytes = if output == 0 {
                            b"2000-01-01".as_slice()
                        } else {
                            &[0; 4]
                        };
                        [
                            1i16.to_be_bytes().as_slice(),
                            &(bytes.len() as i32).to_be_bytes(),
                            bytes,
                        ]
                        .concat()
                    }
                };
                assert_eq!(result[5].1, expected);
            }
        }
    }
    for output in [0, 1] {
        query(&mut socket, "BEGIN");
        error_in_state(
            &cycle(&mut socket, "SELECT $1", 0, output, Some(b"0000-00-00")),
            "22008",
            b"E",
        );
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
    mysql.close().unwrap();
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_temporal_date_null_metadata() {
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    ready(&mut socket);
    let null_row = [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat();
    // The same typed parameter is NULL, epoch day zero, or an invalid empty value.
    for sql in ["SELECT $1", "SELECT $1::date", "SELECT CAST($1 AS DATE)"] {
        for input in [0, 1] {
            for output in [0, 1] {
                for is_null in [true, false] {
                    let binary = 0i32.to_be_bytes();
                    let value = if is_null {
                        None
                    } else if input == 0 {
                        Some(b"2000-01-01".as_slice())
                    } else {
                        Some(binary.as_slice())
                    };
                    let result = cycle(&mut socket, sql, input, output, value);
                    assert_eq!(
                        result.iter().map(|m| m.0).collect::<Vec<_>>(),
                        b"1tT2TDCCZ",
                        "{sql}: {result:?}"
                    );
                    assert_eq!(
                        result[1].1,
                        [1i16.to_be_bytes().as_slice(), &1082u32.to_be_bytes()].concat()
                    );
                    description(&result[2].1, 0);
                    description(&result[4].1, output);
                    let expected = if is_null {
                        null_row.clone()
                    } else {
                        let payload = if output == 0 {
                            b"2000-01-01".as_slice()
                        } else {
                            binary.as_slice()
                        };
                        [
                            1i16.to_be_bytes().as_slice(),
                            &(payload.len() as i32).to_be_bytes(),
                            payload,
                        ]
                        .concat()
                    };
                    assert_eq!(result[5].1, expected);
                }
            }
        }
    }
    // Literal NULL, first-row NULL, all NULL, and zero rows retain prepared types.
    // Rebind one named statement in each result format without changing identity.
    for (index, (sql, rows)) in [
        ("SELECT NULL::date", vec![None]),
        (
            "SELECT NULL::date UNION ALL SELECT '2000-01-01'::date",
            vec![None, Some(0i32)],
        ),
        (
            "SELECT NULL::date UNION ALL SELECT NULL::date",
            vec![None, None],
        ),
        ("SELECT NULL::date WHERE false", vec![]),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("date_null_{index}");
        send(
            &mut socket,
            b'P',
            &[
                name.as_bytes(),
                b"\0",
                sql.as_bytes(),
                b"\0",
                &0i16.to_be_bytes(),
            ]
            .concat(),
        );
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        send(&mut socket, b'S', &[]);
        let prepared = ready(&mut socket);
        assert_eq!(
            prepared.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"1tTZ",
            "{sql}: {prepared:?}"
        );
        assert_eq!(prepared[1].1, 0i16.to_be_bytes());
        description(&prepared[2].1, 0);
        for output in [0i16, 1, 0] {
            send(
                &mut socket,
                b'B',
                &[
                    b"\0".as_slice(),
                    name.as_bytes(),
                    b"\0",
                    &0i16.to_be_bytes(),
                    &0i16.to_be_bytes(),
                    &1i16.to_be_bytes(),
                    &output.to_be_bytes(),
                ]
                .concat(),
            );
            send(&mut socket, b'D', b"P\0");
            send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
            send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
            // Describe again after execution, including an exhausted zero-row portal.
            send(&mut socket, b'D', b"P\0");
            send(&mut socket, b'S', &[]);
            let result = ready(&mut socket);
            let tags = [b"2T".as_slice(), &vec![b'D'; rows.len()], b"CCTZ"].concat();
            assert_eq!(
                result.iter().map(|m| m.0).collect::<Vec<_>>(),
                tags,
                "{sql}: {result:?}"
            );
            description(&result[1].1, output);
            description(&result[result.len() - 2].1, output);
            for (message, value) in result.iter().filter(|m| m.0 == b'D').zip(&rows) {
                let expected = match value {
                    None => null_row.clone(),
                    Some(days) => {
                        let binary = days.to_be_bytes();
                        let payload = if output == 0 {
                            b"2000-01-01".as_slice()
                        } else {
                            binary.as_slice()
                        };
                        [
                            1i16.to_be_bytes().as_slice(),
                            &(payload.len() as i32).to_be_bytes(),
                            payload,
                        ]
                        .concat()
                    }
                };
                assert_eq!(message.1, expected);
            }
        }
    }
    for output in [0, 1] {
        // Empty payloads are non-NULL, including the binary zero-length case.
        error(
            &cycle(&mut socket, "SELECT $1", 0, output, Some(b"")),
            "22007",
        );
        assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
        error(
            &cycle(&mut socket, "SELECT $1", 1, output, Some(b"")),
            "08P01",
        );
        assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    }
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_temporal_date_invalid_binary() {
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    ready(&mut socket);
    for output in [0i16, 1] {
        for in_transaction in [false, true] {
            for (input, payload, state) in [
                (1, vec![], "08P01"),
                (1, vec![0; 3], "08P01"),
                (1, vec![0; 5], "22P03"),
                (1, (-2451546i32).to_be_bytes().to_vec(), "22008"),
                (1, 2145031949i32.to_be_bytes().to_vec(), "22008"),
                (0, vec![0xff], "22021"),
                (0, b"2000-01-01\0".to_vec(), "22021"),
            ] {
                if in_transaction {
                    assert!(!query(&mut socket, "BEGIN").iter().any(|m| m.0 == b'E'));
                }
                let result = cycle(
                    &mut socket,
                    "SELECT $1::date",
                    input,
                    output,
                    Some(&payload),
                );
                error_in_state(&result, state, if in_transaction { b"E" } else { b"I" });
                assert!(!result.iter().any(|m| m.0 == b'D' || m.0 == b'2'));
                if in_transaction {
                    assert_eq!(
                        query(&mut socket, "ROLLBACK").last().unwrap(),
                        &(b'Z', b"I".to_vec())
                    );
                }
                assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
            }
        }
        for input in [0i16, 1] {
            // Finite endpoints, epoch zero and both sentinels remain non-NULL.
            for (text, days) in [
                ("4714-11-24 BC", -2451545i32),
                ("5874897-12-31", 2145031948),
                ("2000-01-01", 0),
                ("-infinity", i32::MIN),
                ("infinity", i32::MAX),
            ] {
                let binary = days.to_be_bytes();
                let payload = if input == 0 { text.as_bytes() } else { &binary };
                let result = cycle(&mut socket, "SELECT $1::date", input, output, Some(payload));
                assert_eq!(
                    result.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"1tT2TDCCZ",
                    "{result:?}"
                );
                assert_eq!(
                    result[1].1,
                    [1i16.to_be_bytes().as_slice(), &1082u32.to_be_bytes()].concat()
                );
                description(&result[2].1, 0);
                description(&result[4].1, output);
                let expected = if output == 0 {
                    text.as_bytes()
                } else {
                    &binary
                };
                assert_eq!(
                    result[5].1,
                    [
                        1i16.to_be_bytes().as_slice(),
                        &(expected.len() as i32).to_be_bytes(),
                        expected
                    ]
                    .concat()
                );
            }
            let result = cycle(&mut socket, "SELECT $1::date", input, output, None);
            assert_eq!(result.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tT2TDCCZ");
            description(&result[2].1, 0);
            description(&result[4].1, output);
            assert_eq!(
                result[5].1,
                [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
            );
        }
        // The declared Bind value exceeds its complete outer message body.
        // Sync can therefore recover without guessing a TCP frame boundary.
        for declared in [4i32, 8] {
            let parse = [
                b"\0SELECT $1::date\0".as_slice(),
                &1i16.to_be_bytes(),
                &1082u32.to_be_bytes(),
            ]
            .concat();
            send(&mut socket, b'P', &parse);
            let bind = [
                b"\0\0".as_slice(),
                &1i16.to_be_bytes(),
                &1i16.to_be_bytes(),
                &1i16.to_be_bytes(),
                &declared.to_be_bytes(),
                &[0; 3],
            ]
            .concat();
            send(&mut socket, b'B', &bind);
            send(&mut socket, b'S', &[]);
            let result = ready(&mut socket);
            error(&result, "08P01");
            assert!(!result.iter().any(|m| m.0 == b'2'));
            assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
        }
    }
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_temporal_time_day() {
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    ready(&mut socket);
    let describe = |body: &[u8], format: i16| {
        assert_eq!(&body[..2], &1i16.to_be_bytes());
        let at = 3 + body[2..].iter().position(|b| *b == 0).unwrap();
        assert_eq!(&body[at + 6..at + 10], &1083u32.to_be_bytes());
        assert_eq!(&body[at + 10..at + 12], &8i16.to_be_bytes());
        assert_eq!(&body[at + 12..at + 16], &(-1i32).to_be_bytes());
        assert_eq!(&body[at + 16..at + 18], &format.to_be_bytes());
    };
    for sql in ["SELECT $1", "SELECT CAST($1 AS TIME)"] {
        for input in [0, 1] {
            for output in [0, 1] {
                for (text, micros) in [
                    ("00:00:00", 0i64),
                    ("12:34:56.123456", 45296123456),
                    ("23:59:59.999999", 86399999999),
                    ("24:00:00", 86400000000),
                ] {
                    let binary = micros.to_be_bytes();
                    let value = if input == 0 { text.as_bytes() } else { &binary };
                    let result = cycle_oid(&mut socket, sql, 1083, input, output, Some(value));
                    assert_eq!(
                        result.iter().map(|m| m.0).collect::<Vec<_>>(),
                        b"1tT2TDCCZ",
                        "{result:?}"
                    );
                    assert_eq!(
                        result[1].1,
                        [1i16.to_be_bytes().as_slice(), &1083u32.to_be_bytes()].concat()
                    );
                    describe(&result[2].1, 0);
                    describe(&result[4].1, output);
                    let expected = if output == 0 {
                        text.as_bytes()
                    } else {
                        &binary
                    };
                    assert_eq!(date_text_row(&result), expected);
                }
                let null = cycle_oid(&mut socket, sql, 1083, input, output, None);
                assert_eq!(
                    null.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"1tT2TDCCZ",
                    "{null:?}"
                );
                describe(&null[2].1, 0);
                describe(&null[4].1, output);
                assert_eq!(
                    null[5].1,
                    [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
                );
            }
        }
    }
    for input in [0, 1] {
        for output in [0, 1] {
            let binary = 86400000000i64.to_be_bytes();
            let value = if input == 0 {
                b"24:00:00".as_slice()
            } else {
                &binary
            };
            let empty = cycle_oid(
                &mut socket,
                "SELECT CAST($1 AS TIME) WHERE FALSE",
                1083,
                input,
                output,
                Some(value),
            );
            assert_eq!(
                empty.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tT2TCCZ",
                "{empty:?}"
            );
            describe(&empty[2].1, 0);
            describe(&empty[4].1, output);
            for value in [None, Some(value)] {
                let rows = cycle_oid(
                    &mut socket,
                    "SELECT CAST($1 AS TIME) UNION ALL SELECT CAST($1 AS TIME)",
                    1083,
                    input,
                    output,
                    value,
                );
                assert_eq!(
                    rows.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"1tT2TDDCCZ",
                    "{rows:?}"
                );
                describe(&rows[2].1, 0);
                describe(&rows[4].1, output);
                assert_eq!(rows[5].1, rows[6].1);
                if value.is_none() {
                    assert_eq!(
                        rows[5].1,
                        [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
                    );
                } else {
                    assert_eq!(
                        &rows[5].1[6..],
                        if output == 0 {
                            b"24:00:00".as_slice()
                        } else {
                            &binary
                        }
                    );
                }
            }
            let rows = cycle_oid(
                &mut socket,
                "SELECT CAST(NULL AS TIME) UNION ALL SELECT CAST($1 AS TIME)",
                1083,
                input,
                output,
                Some(value),
            );
            assert_eq!(
                rows.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tT2TDDCCZ",
                "{rows:?}"
            );
            describe(&rows[2].1, 0);
            describe(&rows[4].1, output);
            assert_eq!(
                rows[5].1,
                [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
            );
            assert_eq!(
                &rows[6].1[6..],
                if output == 0 {
                    b"24:00:00".as_slice()
                } else {
                    &binary
                }
            );
        }
    }
    for bad in ["24:00:00.000001", "25:00:00", "12:60:00", "12:00:61"] {
        for output in [0, 1] {
            error(
                &cycle_oid(
                    &mut socket,
                    "SELECT $1",
                    1083,
                    0,
                    output,
                    Some(bad.as_bytes()),
                ),
                "22008",
            );
            assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
        }
        error(
            &query(&mut socket, &format!("SELECT CAST('{bad}' AS TIME)")),
            "22008",
        );
        assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    }
    for (bytes, state) in [
        (vec![0; 7], "08P01"),
        (vec![0; 9], "22P03"),
        ((-1i64).to_be_bytes().to_vec(), "22008"),
        (86400000001i64.to_be_bytes().to_vec(), "22008"),
    ] {
        error(
            &cycle_oid(&mut socket, "SELECT $1", 1083, 1, 0, Some(&bytes)),
            state,
        );
        assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    }
    for (bytes, state) in [
        (b"".as_slice(), "22007"),
        (b"bad".as_slice(), "22007"),
        (b"12:00:00\0".as_slice(), "22021"),
        (&[255], "22021"),
    ] {
        error(
            &cycle_oid(&mut socket, "SELECT $1", 1083, 0, 0, Some(bytes)),
            state,
        );
        assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    }
    query(&mut socket, "BEGIN");
    error_in_state(
        &cycle_oid(&mut socket, "SELECT $1", 1083, 0, 0, Some(b"25:00:00")),
        "22008",
        b"E",
    );
    query(&mut socket, "ROLLBACK");
    assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    send(&mut socket, b'X', &[]);
    service.close();
}

fn time_precision_description(body: &[u8], format: i16, precision: i32) {
    assert_eq!(&body[..2], &1i16.to_be_bytes());
    let at = 3 + body[2..].iter().position(|b| *b == 0).unwrap();
    assert_eq!(&body[at + 6..at + 10], &1083u32.to_be_bytes());
    assert_eq!(&body[at + 10..at + 12], &8i16.to_be_bytes());
    assert_eq!(&body[at + 12..at + 16], &precision.to_be_bytes());
    assert_eq!(&body[at + 16..at + 18], &format.to_be_bytes());
}

#[test]
fn pg_temporal_time_precision() {
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    ready(&mut socket);
    for postfix in [false, true] {
        for (p, text, micros, expected, rounded) in [
            (0, "12:34:56.5", 45296500000i64, "12:34:57", 45297000000i64),
            (3, "12:34:56.1234", 45296123400, "12:34:56.123", 45296123000),
            (3, "12:34:56.1235", 45296123500, "12:34:56.124", 45296124000),
            (0, "23:59:59.999999", 86399999999, "24:00:00", 86400000000),
            (3, "23:59:59.999999", 86399999999, "24:00:00", 86400000000),
            (
                6,
                "12:34:56.1234567",
                45296123457,
                "12:34:56.123457",
                45296123457,
            ),
            (6, "00:00:00", 0, "00:00:00", 0),
            (6, "24:00:00", 86400000000, "24:00:00", 86400000000),
            // Input rounding occurs before typmod rounding (double rounding).
            (
                3,
                "12:34:56.1234999",
                45296123500,
                "12:34:56.124",
                45296124000,
            ),
        ] {
            let sql = if postfix {
                format!("SELECT $1::time({p})")
            } else {
                format!("SELECT CAST($1 AS TIME({p}))")
            };
            for input in [0, 1] {
                for output in [0, 1] {
                    let binary = micros.to_be_bytes();
                    let value = if input == 0 { text.as_bytes() } else { &binary };
                    let result = cycle_oid(&mut socket, &sql, 1083, input, output, Some(value));
                    assert_eq!(
                        result.iter().map(|m| m.0).collect::<Vec<_>>(),
                        b"1tT2TDCCZ",
                        "{sql}: {result:?}"
                    );
                    assert_eq!(
                        result[1].1,
                        [1i16.to_be_bytes().as_slice(), &1083u32.to_be_bytes()].concat()
                    );
                    time_precision_description(&result[2].1, 0, p);
                    time_precision_description(&result[4].1, output, p);
                    let rounded = rounded.to_be_bytes();
                    assert_eq!(
                        date_text_row(&result),
                        if output == 0 {
                            expected.as_bytes()
                        } else {
                            &rounded
                        },
                        "{sql} {text}"
                    );
                    let null = cycle_oid(&mut socket, &sql, 1083, input, output, None);
                    assert_eq!(
                        null.iter().map(|m| m.0).collect::<Vec<_>>(),
                        b"1tT2TDCCZ",
                        "{null:?}"
                    );
                    time_precision_description(&null[2].1, 0, p);
                    time_precision_description(&null[4].1, output, p);
                    assert_eq!(
                        null[5].1,
                        [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
                    );
                }
            }
        }
    }
    for input in [0, 1] {
        for output in [0, 1] {
            for (sql, rows) in [
                (
                    "SELECT $1::time(3) WHERE FALSE",
                    Vec::<Option<&[u8]>>::new(),
                ),
                (
                    "SELECT $1::time(3) UNION ALL SELECT $1::time(3)",
                    vec![Some(b"12:34:56.124".as_slice()); 2],
                ),
                (
                    "SELECT NULL::time(3) UNION ALL SELECT $1::time(3)",
                    vec![None, Some(b"12:34:56.124".as_slice())],
                ),
                (
                    "SELECT NULL::time(3) UNION ALL SELECT NULL::time(3) WHERE $1 IS NOT NULL",
                    vec![None, None],
                ),
            ] {
                let binary = 45296123500i64.to_be_bytes();
                let value = if input == 0 {
                    b"12:34:56.1235".as_slice()
                } else {
                    &binary
                };
                let result = cycle_oid(&mut socket, sql, 1083, input, output, Some(value));
                assert!(!result.iter().any(|m| m.0 == b'E'), "{sql}: {result:?}");
                let descriptions = result.iter().filter(|m| m.0 == b'T').collect::<Vec<_>>();
                assert_eq!(descriptions.len(), 2);
                time_precision_description(&descriptions[0].1, 0, 3);
                time_precision_description(&descriptions[1].1, output, 3);
                let actual = result.iter().filter(|m| m.0 == b'D').collect::<Vec<_>>();
                assert_eq!(actual.len(), rows.len(), "{sql}");
                for ((_, body), value) in actual.into_iter().zip(rows) {
                    let expected_binary = 45296124000i64.to_be_bytes();
                    let expected = match value {
                        None => [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat(),
                        Some(value) => {
                            let payload = if output == 0 { value } else { &expected_binary };
                            [
                                1i16.to_be_bytes().as_slice(),
                                &(payload.len() as i32).to_be_bytes(),
                                payload,
                            ]
                            .concat()
                        }
                    };
                    assert_eq!(*body, expected);
                }
            }
            for (bytes, state) in [
                (vec![0; 7], "08P01"),
                (vec![0; 9], "22P03"),
                ((-1i64).to_be_bytes().to_vec(), "22008"),
                (86400000001i64.to_be_bytes().to_vec(), "22008"),
            ] {
                error(
                    &cycle_oid(
                        &mut socket,
                        "SELECT $1::time(0)",
                        1083,
                        1,
                        output,
                        Some(&bytes),
                    ),
                    state,
                );
                assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
            }
        }
    }
    for sql in ["SELECT CAST(NULL AS TIME(-1))", "SELECT NULL::time(-1)"] {
        error(&query(&mut socket, sql), "22023");
        error(&cycle_oid(&mut socket, sql, 1083, 0, 0, None), "22023");
        assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    }
    for sql in ["SELECT CAST($1 AS TIME(7))", "SELECT $1::time(7)"] {
        for input in [0, 1] {
            for output in [0, 1] {
                let binary = 45296123457i64.to_be_bytes();
                let value = if input == 0 {
                    b"12:34:56.1234567".as_slice()
                } else {
                    &binary
                };
                let result = cycle_oid(&mut socket, sql, 1083, input, output, Some(value));
                assert!(!result.iter().any(|m| m.0 == b'E'), "{result:?}");
                let notices = result.iter().filter(|m| m.0 == b'N').collect::<Vec<_>>();
                assert_eq!(notices.len(), 1, "{result:?}");
                assert!(notices[0].1.windows(7).any(|w| w == b"C22023\0"));
                assert!(notices[0].1.windows(9).any(|w| w == b"SWARNING\0"));
                let descriptions = result.iter().filter(|m| m.0 == b'T').collect::<Vec<_>>();
                time_precision_description(&descriptions[0].1, 0, 6);
                time_precision_description(&descriptions[1].1, output, 6);
                assert_eq!(
                    date_text_row(&result),
                    if output == 0 {
                        b"12:34:56.123457".as_slice()
                    } else {
                        &binary
                    }
                );
            }
        }
    }
    for sql in [
        "SELECT CAST('12:34:56.1234567' AS TIME(7))",
        "SELECT '12:34:56.1234567'::time(7)",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().filter(|m| m.0 == b'N').count(),
            1,
            "{result:?}"
        );
        assert_eq!(date_text_row(&result), b"12:34:56.123457");
    }
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_temporal_time_text_zone() {
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
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    ready(&mut socket);

    for zone in ["+00:00", "Pacific/Kiritimati", "America/Los_Angeles"] {
        let changed = query(&mut socket, &format!("SET time_zone = '{zone}'"));
        assert_eq!(
            changed.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"CZ",
            "{changed:?}"
        );
        for (text, canonical, micros) in [
            ("04:05", "04:05:00", 14700000000i64),
            ("040506", "04:05:06", 14706000000),
            ("04:05 PM", "16:05:00", 57900000000),
            ("allballs", "00:00:00", 0),
            ("04:05:06+08:00", "04:05:06", 14706000000),
            ("12:00 AM", "00:00:00", 0),
            ("12:00 PM", "12:00:00", 43200000000),
            ("24:00", "24:00:00", 86400000000),
            ("040506.1234567", "04:05:06.123457", 14706123457),
        ] {
            for input in [0, 1] {
                for output in [0, 1] {
                    let binary = micros.to_be_bytes();
                    let value = if input == 0 { text.as_bytes() } else { &binary };
                    for sql in ["SELECT $1", "SELECT CAST($1 AS TIME)"] {
                        let rows = cycle_oid(&mut socket, sql, 1083, input, output, Some(value));
                        assert_eq!(
                            rows.iter().map(|m| m.0).collect::<Vec<_>>(),
                            b"1tT2TDCCZ",
                            "{text} {zone}: {rows:?}"
                        );
                        assert_eq!(
                            rows[1].1,
                            [1i16.to_be_bytes().as_slice(), &1083u32.to_be_bytes()].concat()
                        );
                        time_precision_description(&rows[2].1, 0, -1);
                        time_precision_description(&rows[4].1, output, -1);
                        assert_eq!(
                            date_text_row(&rows),
                            if output == 0 {
                                canonical.as_bytes()
                            } else {
                                &binary
                            }
                        );
                    }
                }
            }
            let literal = query(&mut socket, &format!("SELECT CAST('{text}' AS TIME)"));
            time_precision_description(&literal[0].1, 0, -1);
            assert_eq!(date_text_row(&literal), canonical.as_bytes());
        }
    }
    for input in [0, 1] {
        for output in [0, 1] {
            let rows = cycle_oid(&mut socket, "SELECT $1", 1083, input, output, None);
            time_precision_description(&rows[2].1, 0, -1);
            time_precision_description(&rows[4].1, output, -1);
            assert_eq!(
                rows[5].1,
                [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
            );
        }
    }
    for (text, state) in [
        ("", "22007"),
        ("garbage", "22007"),
        ("04:05 XM", "22007"),
        ("13:05 PM", "22008"),
        ("04:60", "22008"),
        ("24:01", "22008"),
        ("04:05:06+16:00", "22009"),
        ("04:05:06+08:60", "22009"),
        ("04:05:06+08:00junk", "22007"),
    ] {
        for output in [0, 1] {
            error(
                &cycle_oid(
                    &mut socket,
                    "SELECT $1",
                    1083,
                    0,
                    output,
                    Some(text.as_bytes()),
                ),
                state,
            );
            assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
        }
    }
    // Keep a bound value in a named portal while changing session timezone.
    for output in [0i16, 1] {
        query(&mut socket, "BEGIN");
        let mut parse = b"kept\0SELECT CAST($1 AS TIME)\0".to_vec();
        parse.extend_from_slice(&1i16.to_be_bytes());
        parse.extend_from_slice(&1083u32.to_be_bytes());
        send(&mut socket, b'P', &parse);
        let value = b"04:05:06+08:00";
        let mut bind = b"kept\0kept\0".to_vec();
        bind.extend_from_slice(&0i16.to_be_bytes());
        bind.extend_from_slice(&1i16.to_be_bytes());
        bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
        bind.extend_from_slice(value);
        bind.extend_from_slice(&1i16.to_be_bytes());
        bind.extend_from_slice(&output.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            b"12Z"
        );
        let changed = query(&mut socket, "SET time_zone = 'Pacific/Kiritimati'");
        assert_eq!(changed.iter().map(|m| m.0).collect::<Vec<_>>(), b"CZ");
        send(&mut socket, b'D', b"Pkept\0");
        send(&mut socket, b'E', b"kept\0\0\0\0\0");
        send(&mut socket, b'E', b"kept\0\0\0\0\0");
        send(&mut socket, b'S', &[]);
        let rows = ready(&mut socket);
        assert_eq!(
            rows.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCCZ",
            "{rows:?}"
        );
        time_precision_description(&rows[0].1, output, -1);
        assert_eq!(
            date_text_row(&rows),
            if output == 0 {
                b"04:05:06".to_vec()
            } else {
                14706000000i64.to_be_bytes().to_vec()
            }
        );
        query(&mut socket, "ROLLBACK");
        send(&mut socket, b'C', b"Skept\0");
        send(&mut socket, b'S', &[]);
        ready(&mut socket);
    }
    error(
        &cycle_oid(
            &mut socket,
            "SELECT $1",
            1266,
            0,
            0,
            Some(b"04:05:06+08:00"),
        ),
        "0A000",
    );
    assert_eq!(date_text_row(&query(&mut socket, "SELECT 1::int4")), b"1");
    send(&mut socket, b'X', &[]);
    service.close();
}
