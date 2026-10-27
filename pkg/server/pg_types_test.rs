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
fn oids(description: &[u8]) -> Vec<u32> {
    let count = i16::from_be_bytes(description[..2].try_into().unwrap()) as usize;
    let mut offset = 2;
    (0..count)
        .map(|_| {
            offset += description[offset..].iter().position(|b| *b == 0).unwrap() + 1;
            let oid = u32::from_be_bytes(description[offset + 6..offset + 10].try_into().unwrap());
            offset += 18;
            oid
        })
        .collect()
}
#[test]
fn common_type_oids_and_values() {
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

    for (sql, expected_oids, values) in [
        (
            "SELECT 1, -2, TRUE, FALSE, 'hello', NULL",
            vec![23, 23, 16, 16, 25, 25],
            vec![
                Some("1"),
                Some("-2"),
                Some("t"),
                Some("f"),
                Some("hello"),
                None,
            ],
        ),
        (
            "SELECT CAST(12.34 AS DECIMAL(8,2))",
            vec![1700],
            vec![Some("12.34")],
        ),
        (
            "SELECT CAST('2026-09-30' AS DATE)",
            vec![1082],
            vec![Some("2026-09-30")],
        ),
        (
            "SELECT CAST('2026-09-30 12:34:56' AS DATETIME)",
            vec![1114],
            vec![Some("2026-09-30 12:34:56")],
        ),
        (
            "SELECT CAST('12:34:56' AS TIME)",
            vec![1083],
            vec![Some("12:34:56")],
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {result:?}"
        );
        assert_eq!(oids(&result[0].1), expected_oids, "{sql}");
        assert_eq!(result[1].1, row(&values), "{sql}");
    }
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn native_metadata_bridge_preserves_boolean_and_mysql_columns() {
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(75001, 0, 45, "", None).unwrap();
    let integer = context
        .execute_query("SELECT 1 AS v", false, &CancellationToken::new())
        .unwrap()
        .remove(0);
    let boolean = context
        .execute_query("SELECT TRUE AS v", false, &CancellationToken::new())
        .unwrap()
        .remove(0);
    assert_eq!(integer.columns, boolean.columns); // Existing MySQL fallback unchanged.
    assert_eq!(integer.rows, boolean.rows);
    let flag = astersql_parser_mysql::r#type::IsBooleanFlag;
    assert_eq!(integer.native_types[0].flags & flag, 0);
    assert_ne!(boolean.native_types[0].flags & flag, 0);
    context.close().unwrap();
}

#[test]
fn unsigned_binary_and_unsupported_types() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let column = ColumnInfo {
        schema: String::new(),
        table: String::new(),
        org_table: String::new(),
        name: "v".into(),
        org_name: String::new(),
        charset: 63,
        column_length: 0,
        column_type: 8,
        flags: 32,
        decimals: 0,
        default_value: None,
    };
    let mut result = QueryResult {
        columns: vec![column],
        rows: vec![vec![Value::Unsigned(u64::MAX)]],
        native_types: vec![NativeType {
            code: 8,
            flags: 32,
            length: 20,
            decimal: 0,
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        }],
        ..QueryResult::default()
    };
    let messages = crate::pg_result::encode(&result, "SELECT").unwrap();
    assert_eq!(oids(&messages[0].1), vec![1700]);
    assert_eq!(messages[1].1, row(&[Some("18446744073709551615")]));
    result.native_types[0].code = 252;
    result.columns[0].column_type = 252;
    result.rows = vec![vec![Value::Bytes(vec![0, 255, 92])]];
    let messages = crate::pg_result::encode(&result, "SELECT").unwrap();
    assert_eq!(oids(&messages[0].1), vec![17]);
    assert_eq!(messages[1].1, row(&[Some("\\x00ff5c")]));
    result.native_types[0].code = 245;
    result.columns[0].column_type = 245; // JSON is outside the first-stage type set.
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    // NULL must not allow an unsupported native JSON type to masquerade as
    // a PG catalog text array before value encoding rejects its contents.
    result.rows = vec![vec![Value::Null]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.native_types[0].code = 11;
    result.columns[0].column_type = 11;
    result.rows = vec![vec![Value::Text("25:00:00".into())]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.native_types[0].code = 10;
    result.columns[0].column_type = 10;
    result.rows = vec![vec![Value::Text("0000-00-00".into())]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.native_types = vec![
        NativeType {
            code: 10,
            flags: 0,
            length: 0,
            decimal: 0,
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        };
        2
    ];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
}

#[test]
fn missing_original_metadata_is_explicitly_unsupported() {
    use crate::conn::{ColumnInfo, QueryResult, Value};
    // A real query always supplies native metadata, so construct the invalid
    // protocol boundary directly instead of bootstrapping a full SQL session.
    let result = QueryResult {
        columns: vec![ColumnInfo {
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "1".into(),
            org_name: String::new(),
            charset: 45,
            column_length: 1,
            column_type: 8,
            flags: 0,
            decimals: 0,
            default_value: None,
        }],
        rows: vec![vec![Value::Signed(1)]],
        ..QueryResult::default()
    };
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
}

#[test]
fn catalog_char_and_acl_wire_types() {
    let metadata = crate::pg_catalog::CatalogQuery::parse(
        "SELECT 'x'::char AS kind, T.relacl FROM pg_class T",
    )
    .unwrap()
    .unwrap()
    .metadata();
    let result = crate::conn::QueryResult {
        columns: metadata.columns,
        native_types: metadata.native_types,
        rows: vec![vec![
            crate::conn::Value::Text("x".into()),
            crate::conn::Value::Null,
        ]],
        ..Default::default()
    };
    let messages = crate::pg_result::encode(&result, "SELECT").unwrap();
    assert_eq!(oids(&messages[0].1), vec![1042, 1034]);
    assert_eq!(messages[1].1, row(&[Some("x"), None]));
}

#[test]
fn pg_binary_native_results() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let mut result = QueryResult {
        columns: vec![ColumnInfo {
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            charset: 45,
            column_length: 0,
            column_type: 8,
            flags: 0,
            decimals: 0,
            default_value: None,
        }],
        native_types: vec![NativeType {
            code: 8,
            flags: 0,
            length: 0,
            decimal: 0,
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        }],
        ..QueryResult::default()
    };
    for (code, flags, value, expected) in [
        (
            8,
            0,
            Value::Signed(i64::MIN),
            i64::MIN.to_be_bytes().to_vec(),
        ),
        (2, 0, Value::Signed(-2), (-2i16).to_be_bytes().to_vec()),
        (
            3,
            0,
            Value::Signed(i32::MAX as i64),
            i32::MAX.to_be_bytes().to_vec(),
        ),
        (
            1,
            astersql_parser_mysql::r#type::IsBooleanFlag,
            Value::Signed(0),
            vec![0],
        ),
        (4, 0, Value::Float(1.5), 1.5f32.to_be_bytes().to_vec()),
        (
            5,
            0,
            Value::Float(f64::INFINITY),
            f64::INFINITY.to_be_bytes().to_vec(),
        ),
        (
            10,
            0,
            Value::Text("1999-12-31".into()),
            (-1i32).to_be_bytes().to_vec(),
        ),
        (
            11,
            0,
            Value::Text("00:00:00.000001".into()),
            1i64.to_be_bytes().to_vec(),
        ),
        (
            12,
            0,
            Value::Text("1999-12-31 23:59:59.999999".into()),
            (-1i64).to_be_bytes().to_vec(),
        ),
        (
            crate::pg_result::CatalogColumnType::Int2Array as u8,
            0,
            Value::Text("{}".into()),
            [
                0i32.to_be_bytes().as_slice(),
                &0i32.to_be_bytes(),
                &21u32.to_be_bytes(),
            ]
            .concat(),
        ),
        (
            crate::pg_result::CatalogColumnType::Int2Vector as u8,
            0,
            Value::Text("1 -2".into()),
            [
                1i32.to_be_bytes().as_slice(),
                &0i32.to_be_bytes(),
                &21u32.to_be_bytes(),
                &2i32.to_be_bytes(),
                &0i32.to_be_bytes(),
                &2i32.to_be_bytes(),
                &1i16.to_be_bytes(),
                &2i32.to_be_bytes(),
                &(-2i16).to_be_bytes(),
            ]
            .concat(),
        ),
    ] {
        result.native_types[0].code = code;
        result.native_types[0].flags = flags;
        result.rows = vec![vec![value]];
        let messages = crate::pg_result::encode_formats(&result, "SELECT", &[1]).unwrap();
        assert_eq!(&messages[0].1[messages[0].1.len() - 2..], &[0, 1]);
        assert_eq!(
            messages[1].1,
            [
                1i16.to_be_bytes().as_slice(),
                &(expected.len() as i32).to_be_bytes(),
                &expected
            ]
            .concat()
        );
        result.rows = vec![vec![Value::Null]];
        assert_eq!(
            crate::pg_result::encode_formats(&result, "SELECT", &[1]).unwrap()[1].1,
            [1i16.to_be_bytes().as_slice(), &(-1i32).to_be_bytes()].concat()
        );
    }
    result.native_types[0].code = 252;
    result.columns[0].charset = 63;
    result.rows = vec![vec![Value::Bytes(vec![0, 255])]];
    assert_eq!(
        crate::pg_result::encode_formats(&result, "SELECT", &[1]).unwrap()[1].1,
        [
            1i16.to_be_bytes().as_slice(),
            &2i32.to_be_bytes(),
            &[0, 255]
        ]
        .concat()
    );
    assert!(crate::pg_result::encode_formats(&result, "SELECT", &[2]).is_err());
    assert!(crate::pg_result::encode_formats(&result, "SELECT", &[1, 0]).is_err());
    result.native_types[0].code = 2;
    result.columns[0].charset = 45;
    result.rows = vec![vec![Value::Signed(i64::MAX)]];
    assert!(crate::pg_result::encode_formats(&result, "SELECT", &[1]).is_err());
}

#[test]
fn pg_scalar_27_escape_output() {
    use crate::pg_result::bytea_escape;
    assert_eq!(bytea_escape(b""), b"");
    assert_eq!(bytea_escape(b" A~\\\x00\xff"), b" A~\\\\\\000\\377");
    for byte in 0..=255u8 {
        let encoded = bytea_escape(&[byte]);
        let expected = match byte {
            b'\\' => "\\\\".to_owned(),
            0x20..=0x7e => char::from(byte).to_string(),
            _ => format!("\\{byte:03o}"),
        };
        assert_eq!(encoded, expected.as_bytes(), "byte {byte}");
    }
}

#[test]
fn pg05_uuid_sql_wire_metadata_null_errors_and_recovery() {
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
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (sql, expected) in [
        (
            "SELECT '{00112233445566778899AABBCCDDEEFF}'::text::uuid",
            Some("00112233-4455-6677-8899-aabbccddeeff"),
        ),
        ("SELECT NULL::uuid", None),
        ("SELECT NULL::uuid WHERE false", None),
    ] {
        let messages = query(&mut socket, sql);
        let description = &messages
            .iter()
            .find(|m| m.0 == b'T')
            .expect("RowDescription")
            .1;
        assert_eq!(oids(description), [2950]);
        let at = 2 + description[2..].iter().position(|b| *b == 0).unwrap() + 1;
        assert_eq!(&description[at + 10..at + 12], &16i16.to_be_bytes());
        assert_eq!(&description[at + 12..at + 16], &(-1i32).to_be_bytes());
        if sql.contains("WHERE") {
            assert!(!messages.iter().any(|m| m.0 == b'D'));
        } else {
            assert_eq!(
                messages.iter().find(|m| m.0 == b'D').unwrap().1,
                row(&[expected])
            );
        }
    }
    for (sql, state) in [
        ("SELECT 'bad'::uuid", "22P02"),
        ("SELECT NULL::int4::uuid WHERE false", "0A000"),
    ] {
        let messages = query(&mut socket, sql);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("ErrorResponse")
            .1;
        assert!(
            error
                .windows(6)
                .any(|w| w == [b"C", state.as_bytes()].concat())
        );
        assert!(
            query(&mut socket, "SELECT NULL::uuid::text")
                .iter()
                .any(|m| m.0 == b'D')
        );
    }
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn pg05_uuid_output_codecs_metadata_and_null() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let mut result = QueryResult {
        columns: vec![ColumnInfo {
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            charset: 45,
            column_length: 36,
            column_type: 253,
            flags: 0,
            decimals: 0,
            default_value: None,
        }],
        native_types: vec![NativeType {
            code: astersql_parser_mysql::r#type::TypePgUuid,
            flags: 0,
            length: -1,
            decimal: -1,
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        }],
        ..QueryResult::default()
    };
    // Every byte value includes arbitrary version/variant combinations.
    let mut samples = vec![
        [0; 16],
        [255; 16],
        [
            0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255,
        ],
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128],
    ];
    samples.extend((0..=255).map(|byte| [byte; 16]));
    for bytes in samples {
        let text = astersql_types::pg_uuid::PgUuid::from_bytes(bytes).to_string();
        result.rows = vec![
            vec![Value::Null],
            vec![Value::Text(text.clone())],
            vec![Value::Null],
        ];
        for format in [0i16, 1] {
            let messages =
                crate::pg_result::encode_formats(&result, "SELECT", &[format as usize]).unwrap();
            let mut description = b"\0\x01v\0".to_vec();
            description.extend_from_slice(&[0; 6]);
            description.extend_from_slice(&2950u32.to_be_bytes());
            description.extend_from_slice(&16i16.to_be_bytes());
            description.extend_from_slice(&(-1i32).to_be_bytes());
            description.extend_from_slice(&format.to_be_bytes());
            assert_eq!(messages[0], (b'T', description.clone()));
            assert_eq!(messages[1], (b'D', row(&[None])));
            let payload = if format == 0 {
                text.as_bytes()
            } else {
                bytes.as_slice()
            };
            let expected = [
                1i16.to_be_bytes().as_slice(),
                &(payload.len() as i32).to_be_bytes(),
                payload,
            ]
            .concat();
            assert_eq!(messages[2], (b'D', expected));
            assert_eq!(messages[3], (b'D', row(&[None])));
            assert_eq!(messages[4], (b'C', b"SELECT 3\0".to_vec()));
            let saved = result.rows.clone();
            for rows in [vec![], vec![vec![Value::Null]; 2]] {
                result.rows = rows;
                let empty = crate::pg_result::encode_formats(&result, "SELECT", &[format as usize])
                    .unwrap();
                assert_eq!(empty[0], (b'T', description.clone()));
            }
            result.rows = saved;
        }
    }
    // The native type is required even when the visible MySQL column is text.
    result.native_types.clear();
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
}

#[test]
fn pg05_uuid_output_invalid_carrier_writes_no_messages() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let mut result = QueryResult {
        columns: vec![ColumnInfo {
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            charset: 45,
            column_length: 36,
            column_type: 253,
            flags: 0,
            decimals: 0,
            default_value: None,
        }],
        native_types: vec![NativeType {
            code: astersql_parser_mysql::r#type::TypePgUuid,
            flags: 0,
            length: -1,
            decimal: -1,
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        }],
        ..QueryResult::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let (mut server, _) = listener.accept().unwrap();
    for invalid in [
        Value::Text("bad".into()),
        Value::Bytes(vec![0; 16]),
        Value::Signed(0),
    ] {
        // A valid earlier row must not leak before a later invalid payload.
        result.rows = vec![
            vec![Value::Text("00112233-4455-6677-8899-aabbccddeeff".into())],
            vec![invalid],
        ];
        for format in [0, 1] {
            assert_eq!(
                crate::pg_result::encode_formats(&result, "SELECT", &[format as usize])
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
        }
        assert!(
            crate::pg_result::write_result(&mut server, &result, "SELECT", Default::default())
                .is_err()
        );
        let mut byte = [0];
        assert!(matches!(
            client.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }
    result.rows = vec![vec![Value::Null]];
    result.native_types.clear();
    assert!(
        crate::pg_result::write_result(&mut server, &result, "SELECT", Default::default()).is_err()
    );
    assert!(client.read(&mut [0]).is_err());
}

#[test]
fn pg05_json_sql_wire_metadata_null_errors_and_recovery() {
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
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (sql, expected) in [
        (
            r#"SELECT ' {"a":1.00,"a":1e9999} '::text::json"#,
            Some(r#" {"a":1.00,"a":1e9999} "#),
        ),
        ("SELECT NULL::json", None),
        ("SELECT NULL::json WHERE false", None),
    ] {
        let messages = query(&mut socket, sql);
        let description = &messages
            .iter()
            .find(|m| m.0 == b'T')
            .expect("RowDescription")
            .1;
        assert_eq!(oids(description), [114]);
        let at = 2 + description[2..].iter().position(|b| *b == 0).unwrap() + 1;
        assert_eq!(&description[at + 10..at + 12], &(-1i16).to_be_bytes());
        assert_eq!(&description[at + 12..at + 16], &(-1i32).to_be_bytes());
        if sql.contains("WHERE") {
            assert!(!messages.iter().any(|m| m.0 == b'D'));
        } else {
            assert_eq!(
                messages.iter().find(|m| m.0 == b'D').unwrap().1,
                row(&[expected])
            );
        }
    }
    for (sql, state) in [
        ("SELECT 'bad'::json", "22P02"),
        ("SELECT NULL::int4::json WHERE false", "0A000"),
    ] {
        let messages = query(&mut socket, sql);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("ErrorResponse")
            .1;
        assert!(
            error
                .windows(6)
                .any(|w| w == [b"C", state.as_bytes()].concat())
        );
        assert!(
            query(&mut socket, "SELECT NULL::json::text")
                .iter()
                .any(|m| m.0 == b'D')
        );
    }
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn pg05_json_output_codecs_metadata_and_null() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let mut result = QueryResult {
        columns: vec![ColumnInfo {
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            charset: 45,
            column_length: 0,
            column_type: 253,
            flags: 0,
            decimals: 0,
            default_value: None,
        }],
        native_types: vec![NativeType {
            code: astersql_parser_mysql::r#type::TypePgJson,
            flags: 0,
            length: -1,
            decimal: -1,
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        }],
        ..QueryResult::default()
    };
    for text in [
        "null",
        r#""null""#,
        "{}",
        "[]",
        "true",
        "false",
        "42",
        "-0",
        r#" { "b":1, "a":2,"b":3 } "#,
        "1e1000000",
        "9007199254740993",
        "18446744073709551616",
        "1.230e+02",
        "0.1234567890123456789012345678901234567890",
        "\t[true,false,null]\n",
        r#""中""#,
        r#""\u4e2d""#,
        r#""\u0000""#,
        r#""\uD800""#,
        r#"[null,{"x":[true,"中",1]}]"#,
    ] {
        let json = astersql_types::pg_json::PgJson::from_bytes(text.as_bytes()).unwrap();
        assert_eq!(json.as_bytes(), text.as_bytes());
        let text = text.to_owned();
        result.rows = vec![
            vec![Value::Null],
            vec![Value::Text(text.clone())],
            vec![Value::Null],
        ];
        for format in [0i16, 1] {
            let messages =
                crate::pg_result::encode_formats(&result, "SELECT", &[format as usize]).unwrap();
            let mut description = b"\0\x01v\0".to_vec();
            description.extend_from_slice(&[0; 6]);
            description.extend_from_slice(&114u32.to_be_bytes());
            description.extend_from_slice(&(-1i16).to_be_bytes());
            description.extend_from_slice(&(-1i32).to_be_bytes());
            description.extend_from_slice(&format.to_be_bytes());
            assert_eq!(messages[0], (b'T', description.clone()));
            assert_eq!(messages[1], (b'D', row(&[None])));
            let payload = text.as_bytes();
            let expected = [
                1i16.to_be_bytes().as_slice(),
                &(payload.len() as i32).to_be_bytes(),
                payload,
            ]
            .concat();
            assert_eq!(messages[2], (b'D', expected));
            assert_eq!(messages[3], (b'D', row(&[None])));
            assert_eq!(messages[4], (b'C', b"SELECT 3\0".to_vec()));
            let saved = result.rows.clone();
            for rows in [vec![], vec![vec![Value::Null]; 2]] {
                result.rows = rows;
                let empty = crate::pg_result::encode_formats(&result, "SELECT", &[format as usize])
                    .unwrap();
                assert_eq!(empty[0], (b'T', description.clone()));
            }
            result.rows = saved;
        }
    }
    // MySQL JSON cannot supply the PostgreSQL original-text identity, even
    // for NULL or empty sets. Ordinary text remains its own type.
    result.native_types[0].code = 245;
    for rows in [vec![], vec![vec![Value::Null]]] {
        result.rows = rows;
        for format in [0, 1] {
            assert!(crate::pg_result::encode_formats(&result, "SELECT", &[format]).is_err());
        }
    }
    result.native_types[0].code = 253;
    result.rows = vec![vec![Value::Text("null".into())]];
    assert_eq!(
        oids(&crate::pg_result::encode(&result, "SELECT").unwrap()[0].1),
        vec![25]
    );
    // The native type is required even when the visible MySQL column is text.
    result.native_types.clear();
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
}

#[test]
fn pg05_json_output_invalid_carrier_writes_no_messages() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let mut result = QueryResult {
        columns: vec![ColumnInfo {
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            charset: 45,
            column_length: 0,
            column_type: 253,
            flags: 0,
            decimals: 0,
            default_value: None,
        }],
        native_types: vec![NativeType {
            code: astersql_parser_mysql::r#type::TypePgJson,
            flags: 0,
            length: -1,
            decimal: -1,
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        }],
        ..QueryResult::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let (mut server, _) = listener.accept().unwrap();
    for invalid in [
        Value::Text("bad".into()),
        Value::Text(String::new()),
        Value::Text("{} []".into()),
        Value::Text("[1,]".into()),
        Value::Text("\x01{}".into()),
        Value::Text("\0".into()),
        Value::Bytes(b"null".to_vec()),
        Value::Signed(0),
        Value::Unsigned(0),
        Value::Float(1.0),
    ] {
        // A valid earlier row must not leak before a later invalid payload.
        result.rows = vec![vec![Value::Text("null".into())], vec![invalid]];
        for format in [0, 1] {
            assert_eq!(
                crate::pg_result::encode_formats(&result, "SELECT", &[format as usize])
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
        }
        assert!(
            crate::pg_result::write_result(&mut server, &result, "SELECT", Default::default())
                .is_err()
        );
        let mut byte = [0];
        assert!(matches!(
            client.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ));
    }
    result.rows = vec![vec![Value::Null]];
    result.native_types.clear();
    assert!(
        crate::pg_result::write_result(&mut server, &result, "SELECT", Default::default()).is_err()
    );
    assert!(client.read(&mut [0]).is_err());
}
