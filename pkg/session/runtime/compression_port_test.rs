// Copyright 2026 AsterSQL.

use super::{ConcreteSession, normal_ddl_fixture::Fixture};

fn query(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut sets = session
        .execute(sql)
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    let mut result = Vec::new();
    for set in &mut sets {
        while let Some(row) = set.next_row().unwrap() {
            result.push(row);
        }
        set.close().unwrap();
    }
    result
}

#[test]
fn port_7fa5afdeb8_sql_compress_binary_charset_and_null_contract() {
    let fixture = Fixture::new();
    let session = ConcreteSession::new(fixture.domain.clone());
    for (sql, expected) in [
        (
            "select hex(compress('hello world'))",
            "0B000000789C000B00F4FF68656C6C6F20776F726C6403001A0B045D",
        ),
        (
            "select hex(compress('12345'))",
            "05000000789C000500FAFF3132333435030002F80100",
        ),
        (
            "select hex(compress('23456'))",
            "05000000789C000500FAFF3233343536030003070105",
        ),
        ("select uncompress(compress('123'))", "123"),
        ("select hex(compress(''))", ""),
        ("select compress(NULL) is null", "1"),
        ("select uncompress(NULL) is null", "1"),
        ("select uncompress('1234') is null", "1"),
        ("select uncompress(12345) is null", "1"),
    ] {
        assert_eq!(
            query(&session, sql),
            vec![vec![expected.to_string()]],
            "{sql}"
        );
    }
    for (bytes, digest) in [
        ("e4b880e4ba8ce4b889", "96c5c7c96444244ef50887504bec06fa"),
        ("d2bbb6fec8fd", "2fbe29f3c2841b4df33e8c5b04c0ffde"),
        (
            "e4b880e4ba8ce4b8890000000000000000000000",
            "f5f4dcaa2c502e3294c85596ab76c981",
        ),
    ] {
        let sql = format!("select md5(compress(0x{bytes}))");
        assert_eq!(query(&session, &sql), vec![vec![digest.to_string()]]);
    }
    // Source charset.result: byte sequence after connection charset conversion.
    for bytes in ["61", "d2bbb6fec8fd"] {
        let sql = format!("select hex(uncompress(compress(0x{bytes})))");
        assert_eq!(query(&session, &sql), vec![vec![bytes.to_uppercase()]]);
    }
    assert_eq!(
        query(&session, "select uncompress('1234') is null"),
        vec![vec!["1".to_string()]]
    );
    let warnings = query(&session, "show warnings");
    assert_eq!(warnings.len(), 1);
    assert_eq!(&warnings[0][..2], &["Warning", "1259"]);
}
