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
#[test]
fn simple_query_roundtrip() {
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
    let result = query(&mut socket, "SELECT 1");
    assert_eq!(result.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
    assert_eq!(result[1].1, row(&[Some("1")]));
    assert_eq!(result[2], (b'C', b"SELECT 1\0".to_vec()));
    assert_eq!(result[3], (b'Z', b"I".to_vec()));
    // Column metadata uses PostgreSQL framing and text format, never MySQL packets.
    assert_eq!(&result[0].1[..2], &1i16.to_be_bytes());
    let name_end = result[0].1[2..].iter().position(|b| *b == 0).unwrap() + 2;
    let metadata = &result[0].1[name_end + 1..];
    assert_eq!(metadata.len(), 18);
    assert_eq!(&metadata[6..10], &20u32.to_be_bytes());
    assert_eq!(&metadata[16..], &0i16.to_be_bytes());
    for sql in [
        "SELECT current_catalog",
        "/* DataGrip */ SELECT CURRENT_CATALOG AS catalog",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}"
        );
        assert_eq!(result[1], (b'D', row(&[Some("test")])));
        let label = if sql.contains(" AS ") {
            "catalog"
        } else {
            "current_catalog"
        };
        assert!(result[0].1[2..].starts_with(&[label.as_bytes(), b"\0"].concat()));
    }
    let startup_sql = "SELECT round(extract(epoch from pg_postmaster_start_time() at time zone 'UTC')) as startup_time";
    let startup_result = query(&mut socket, startup_sql);
    assert_eq!(
        startup_result.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ"
    );
    assert!(startup_result[0].1[2..].starts_with(b"startup_time\0"));
    let type_offset = 2 + b"startup_time\0".len() + 6;
    assert_eq!(
        &startup_result[0].1[type_offset..type_offset + 4],
        &1700u32.to_be_bytes()
    );
    let timestamp: u64 = std::str::from_utf8(&startup_result[1].1[6..])
        .unwrap()
        .trim_end_matches(".0")
        .parse()
        .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(timestamp <= now + 1 && timestamp + 60 >= now);
    assert_eq!(query(&mut socket, startup_sql)[1], startup_result[1]);
    for sql in [
        "select L.transactionid::varchar::bigint as transaction_id from pg_catalog.pg_locks L where L.transactionid is not null order by pg_catalog.age(L.transactionid) desc limit 1",
        "select N.oid::bigint as id, datname as name, D.description, datistemplate as is_template, datallowconn as allow_connections, pg_catalog.pg_get_userbyid(N.datdba) as \"owner\" from pg_catalog.pg_database N left join pg_catalog.pg_shdescription D on N.oid = D.objoid order by case when datname = pg_catalog.current_database() then -1::bigint else N.oid::bigint end",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result.first().unwrap().0, b'T', "{sql}: {result:?}");
    }
    let nulls = query(&mut socket, "SELECT NULL, ''");
    assert_eq!(nulls[1], (b'D', row(&[None, Some("")])));
    for (sql, completion) in [
        ("CREATE DATABASE pg_query_roundtrip", "CREATE DATABASE"),
        (
            "CREATE TABLE pg_query_roundtrip.t (id INT, value VARCHAR(20))",
            "CREATE TABLE",
        ),
        (
            "INSERT INTO pg_query_roundtrip.t VALUES (1, 'a'), (2, '')",
            "INSERT 0 2",
        ),
        (
            "UPDATE pg_query_roundtrip.t SET value = 'b' WHERE id = 1",
            "UPDATE 1",
        ),
        ("DELETE FROM pg_query_roundtrip.t WHERE id = 2", "DELETE 1"),
    ] {
        assert_eq!(
            query(&mut socket, sql),
            vec![
                (b'C', [completion.as_bytes(), b"\0"].concat()),
                (b'Z', b"I".to_vec())
            ],
            "{sql}"
        );
    }
    let selected = query(&mut socket, "SELECT id, value FROM pg_query_roundtrip.t");
    assert_eq!(selected[1], (b'D', row(&[Some("1"), Some("b")])));
    assert_eq!(
        query(&mut socket, "  ; "),
        vec![(b'I', vec![]), (b'Z', b"I".to_vec())]
    );
    let multiple = query(&mut socket, "DELETE FROM pg_query_roundtrip.t; SELECT 1");
    assert_eq!(multiple.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(multiple[0].1.windows(5).any(|w| w == b"0A000"));
    assert_eq!(
        query(&mut socket, "SELECT id FROM pg_query_roundtrip.t")[1],
        (b'D', row(&[Some("1")]))
    );
    let semicolon = query(&mut socket, "/* query */ SELECT ';'");
    assert_eq!(semicolon[1], (b'D', row(&[Some(";")])));
    assert_eq!(
        query(&mut socket, "SELECT FROM")
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"EZ"
    );
    assert_eq!(
        query(&mut socket, "SELECT * FROM pg_query_roundtrip.missing")
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"EZ"
    );
    send(&mut socket, b'Q', b"SELECT 1");
    let malformed = read(&mut socket);
    assert_eq!(malformed.0, b'E');
    assert!(malformed.1.windows(5).any(|w| w == b"08P01"));
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    assert_eq!(
        query(&mut socket, "BEGIN").last().unwrap(),
        &(b'Z', b"T".to_vec())
    );
    assert_eq!(
        query(&mut socket, "ROLLBACK").last().unwrap(),
        &(b'Z', b"I".to_vec())
    );
    let mut other = TcpStream::connect(addr).unwrap();
    other
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg_query_roundtrip\0\0",
    ]
    .concat();
    other
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    other.write_all(&startup).unwrap();
    assert_eq!(read(&mut other).0, b'R');
    while read(&mut other).0 != b'Z' {}
    assert_eq!(
        query(&mut other, "SELECT current_catalog")[1],
        (b'D', row(&[Some("pg_query_roundtrip")]))
    );
    assert_eq!(query(&mut other, startup_sql)[1], startup_result[1]);
    send(&mut other, b'X', b"");
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn current_catalog_adaptation_preserves_sql_boundaries() {
    use crate::pg_result::adapt_query;
    for sql in [
        "SELECT 'current_catalog' AS current_catalog",
        "SELECT `current_catalog` FROM t",
        "SELECT t.current_catalog FROM t",
        "SELECT current_catalogue FROM t",
        "SELECT 1 /* current_catalog */",
    ] {
        assert_eq!(adapt_query(sql).unwrap(), sql);
    }
    assert_eq!(
        adapt_query("SELECT current_catalog, 'current_catalog', CURRENT_CATALOG AS name").unwrap(),
        "SELECT DATABASE() AS current_catalog, 'current_catalog', DATABASE() AS name",
    );
}

#[test]
fn simple_query_encoding_rejects_unrepresentable_results() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    let column = ColumnInfo {
        schema: String::new(),
        table: String::new(),
        org_table: String::new(),
        name: "v".into(),
        org_name: String::new(),
        charset: 45,
        column_length: 0,
        column_type: 0,
        flags: 0,
        decimals: 0,
        default_value: None,
    };
    let mut result = QueryResult {
        columns: vec![column],
        native_types: vec![NativeType {
            code: 253,
            flags: 0,
            length: 0,
            decimal: 0,
        }],
        rows: vec![vec![Value::Text("a\0b".into())]],
        ..QueryResult::default()
    };
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.rows = vec![vec![Value::Bytes(vec![0xff])]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.rows = vec![vec![]];
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    result.rows = vec![vec![Value::Null]];
    result.columns[0].name = "a\0b".into();
    assert!(crate::pg_result::encode(&result, "SELECT").is_err());
    assert_eq!(
        crate::pg_result::command("SELECT 1; SELECT 2")
            .unwrap_err()
            .0,
        "0A000"
    );
    assert_eq!(crate::pg_result::command("/* empty */").unwrap(), None);
    assert_eq!(
        crate::pg_result::command("REPLACE INTO t VALUES (1)")
            .unwrap_err()
            .0,
        "0A000"
    );
}

#[test]
fn startup_time_probe_preserves_sql_boundaries() {
    use crate::pg_result::adapt_session_query;
    for sql in [
        "SELECT 'round(extract(epoch from pg_postmaster_start_time() at time zone \"UTC\"))'",
        "selectround(extract(epoch from pg_postmaster_start_time() at time zone 'UTC'))",
        "select round(extract(epoch from pg_postmaster_start_time() at time zone 'U TC'))",
        "select round(extract(epoch from pg_postmaster_start_time_other() at time zone 'UTC'))",
    ] {
        assert_eq!(adapt_session_query(sql, 123_500_000).unwrap(), sql);
    }
    let sql = "SeLeCt ROUND ( EXTRACT ( EPOCH FROM pg_postmaster_start_time ( ) AT TIME ZONE 'UTC' ) ) AS startup_time;";
    assert_eq!(
        adapt_session_query(sql, 123_500_000).unwrap(),
        "SELECT 124.0 AS startup_time;"
    );
}

fn catalog_rows(messages: &[(u8, Vec<u8>)]) -> Vec<Vec<Option<String>>> {
    messages
        .iter()
        .filter(|message| message.0 == b'D')
        .map(|(_, body)| {
            let count = i16::from_be_bytes(body[..2].try_into().unwrap());
            let mut offset = 2;
            (0..count)
                .map(|_| {
                    let length = i32::from_be_bytes(body[offset..offset + 4].try_into().unwrap());
                    offset += 4;
                    if length == -1 {
                        None
                    } else {
                        let end = offset + length as usize;
                        let value = String::from_utf8(body[offset..end].to_vec()).unwrap();
                        offset = end;
                        Some(value)
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn datagrip_catalog_live_metadata() {
    use crate::pg_catalog::{CatalogQuery, DATABASES_SQL, TRANSACTIONS_SQL};
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
    assert_eq!(read(&mut socket).0, b'R');
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "CREATE DATABASE IF NOT EXISTS test",
        "CREATE DATABASE pg_catalog_live",
        "CREATE TABLE test.pg_catalog_tx (id INT)",
    ] {
        assert_eq!(query(&mut socket, sql)[0].0, b'C', "{sql}");
    }
    let result = query(&mut socket, DATABASES_SQL);
    assert_eq!(result[0].0, b'T');
    let rows = catalog_rows(&result);
    assert_eq!(rows[0][1].as_deref(), Some("test"));
    let created = rows
        .iter()
        .find(|r| r[1].as_deref() == Some("pg_catalog_live"))
        .unwrap();
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|schema| schema.name.lower == "pg_catalog_live")
        .unwrap();
    assert_eq!(
        created[0],
        Some(
            crate::pg_catalog::namespace_oid(schema.id)
                .unwrap()
                .to_string()
        )
    );
    assert_eq!(created[2], None);
    assert_eq!(created[3].as_deref(), Some("f"));
    assert_eq!(created[4].as_deref(), Some("t"));
    assert_eq!(created[5], None);
    assert!(catalog_rows(&query(&mut socket, TRANSACTIONS_SQL)).is_empty());
    assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
    assert_eq!(
        query(&mut socket, "INSERT INTO test.pg_catalog_tx VALUES (1)")[0].0,
        b'C'
    );
    let live = catalog_rows(&query(&mut socket, TRANSACTIONS_SQL));
    assert_eq!(live.len(), 1);
    let native = catalog_rows(&query(
        &mut socket,
        "SELECT ID FROM information_schema.tidb_trx",
    ));
    assert!(native.iter().any(|row| row[0] == live[0][0]));
    let mut second = TcpStream::connect(addr).unwrap();
    second
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    second
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    second.write_all(&body).unwrap();
    assert_eq!(read(&mut second).0, b'R');
    while read(&mut second).0 != b'Z' {}
    assert_eq!(query(&mut second, "BEGIN")[0].0, b'C');
    assert_eq!(
        query(&mut second, "INSERT INTO test.pg_catalog_tx VALUES (2)")[0].0,
        b'C'
    );
    assert_eq!(catalog_rows(&query(&mut second, TRANSACTIONS_SQL)), live);
    assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
    let remaining = catalog_rows(&query(&mut socket, TRANSACTIONS_SQL));
    assert_eq!(remaining.len(), 1);
    assert!(
        remaining[0][0].as_ref().unwrap().parse::<i64>().unwrap()
            > live[0][0].as_ref().unwrap().parse::<i64>().unwrap()
    );
    assert_eq!(query(&mut second, "ROLLBACK")[0].0, b'C');
    assert!(catalog_rows(&query(&mut socket, TRANSACTIONS_SQL)).is_empty());
    send(&mut second, b'X', b"");
    assert_eq!(
        query(&mut socket, "DROP DATABASE pg_catalog_live")[0].0,
        b'C'
    );
    assert!(
        !catalog_rows(&query(&mut socket, DATABASES_SQL))
            .iter()
            .any(|r| r[1].as_deref() == Some("pg_catalog_live"))
    );
    for modified in [
        format!("{DATABASES_SQL}; SELECT 1"),
        format!("SELECT '{TRANSACTIONS_SQL}'"),
    ] {
        assert_eq!(CatalogQuery::classify(&modified), None);
    }
    assert_eq!(
        CatalogQuery::classify(&format!("/* intro */ {DATABASES_SQL}; -- done")),
        CatalogQuery::classify(DATABASES_SQL)
    );
    let multiple_transactions =
        CatalogQuery::parse(&TRANSACTIONS_SQL.replace("limit 1", "limit 2"))
            .unwrap()
            .unwrap();
    assert_eq!(multiple_transactions.select.limit, Some(2));
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn namespace_catalog_live_metadata() {
    use crate::pg_catalog::namespace_oid;
    assert_eq!(namespace_oid(1).unwrap(), 16386);
    assert_eq!(namespace_oid(-1).unwrap(), 16385);
    assert_eq!(namespace_oid(-2000).unwrap(), 20383);
    let max = (0x4000_0000i64 - 16384 - 1) / 2;
    assert_eq!(namespace_oid(max).unwrap(), 16384 + max * 2);
    assert_eq!(namespace_oid(-(max + 1)).unwrap(), 0x3fff_ffff);
    for id in [0, max + 1, -(max + 2), i64::MIN, i64::MAX] {
        assert!(namespace_oid(id).is_err(), "{id}");
    }
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
    let sql = r#"select N.oid::bigint as id, N.xmin as state_number, nspname as name, D.description, pg_catalog.pg_get_userbyid(N.nspowner) as "owner" from pg_catalog.pg_namespace N left join pg_catalog.pg_description D on N.oid = D.objoid order by case when nspname = pg_catalog.current_schema() then -1::bigint else N.oid::bigint end"#;
    let before = query(&mut socket, sql);
    assert_eq!(before[0].0, b'T', "{before:?}");
    let before_rows = catalog_rows(&before);
    assert_eq!(before_rows[0][2].as_deref(), Some("test"));
    assert!(
        !before_rows
            .iter()
            .any(|r| r[2].as_deref() == Some("public"))
    );
    assert_eq!(
        query(&mut socket, "CREATE DATABASE namespace_live")[0].0,
        b'C'
    );
    let result = query(&mut socket, sql);
    assert_eq!(result[0].0, b'T', "{result:?}");
    let rows = catalog_rows(&result);
    assert_eq!(rows.len(), before_rows.len() + 1);
    let ids: std::collections::HashSet<_> = rows.iter().map(|r| r[0].clone()).collect();
    assert_eq!(ids.len(), rows.len());
    for r in &rows {
        assert!(r[0].as_ref().unwrap().parse::<u32>().unwrap() > 0);
        assert_eq!(r[1], None);
        assert_eq!(r[3], None);
        assert_eq!(r[4], None);
    }
    for pair in rows[1..].windows(2) {
        assert!(
            pair[0][0].as_ref().unwrap().parse::<u32>().unwrap()
                < pair[1][0].as_ref().unwrap().parse::<u32>().unwrap()
        );
    }
    let mut offset = 2;
    for (name, oid) in [
        ("id", 20u32),
        ("state_number", 20),
        ("name", 25),
        ("description", 25),
        ("owner", 25),
    ] {
        assert!(result[0].1[offset..].starts_with(&[name.as_bytes(), b"\0"].concat()));
        offset += name.len() + 1;
        assert_eq!(&result[0].1[offset + 6..offset + 10], &oid.to_be_bytes());
        offset += 18;
    }
    let created = rows
        .iter()
        .find(|r| r[2].as_deref() == Some("namespace_live"))
        .unwrap();
    let native = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "namespace_live")
        .unwrap();
    assert_eq!(
        created[0],
        Some(namespace_oid(native.id).unwrap().to_string())
    );
    let mut second = TcpStream::connect(addr).unwrap();
    second
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0namespace_live\0\0",
    ]
    .concat();
    second
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    second.write_all(&startup).unwrap();
    while read(&mut second).0 != b'Z' {}
    let current = catalog_rows(&query(&mut second, sql));
    assert_eq!(current[0], *created);
    send(&mut second, b'X', b"");
    assert_eq!(
        query(&mut socket, "DROP DATABASE namespace_live")[0].0,
        b'C'
    );
    assert_eq!(catalog_rows(&query(&mut socket, sql)), before_rows);
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn tablespace_catalog_empty_relation() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        Arc::new(ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::InsecureRootOnly,
        )),
        Arc::new(CanonicalConnectionDomain::new(domain)),
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
    for (sql, fields) in [
        (
            "SELECT oid::bigint AS id, spcname AS name FROM pg_catalog.pg_tablespace ORDER BY oid",
            vec![("id", 20u32), ("name", 25)],
        ),
        (
            "SELECT T.oid, T.spcname, T.spcowner, T.spcacl, T.spcoptions, D.description, pg_catalog.pg_get_userbyid(T.spcowner) AS owner FROM pg_catalog.pg_tablespace T LEFT JOIN pg_catalog.pg_shdescription D ON T.oid = D.objoid ORDER BY T.oid",
            vec![
                ("oid", 26),
                ("spcname", 25),
                ("spcowner", 20),
                ("spcacl", 25),
                ("spcoptions", 25),
                ("description", 25),
                ("owner", 25),
            ],
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TCZ",
            "{result:?}"
        );
        assert!(catalog_rows(&result).is_empty());
        assert_eq!(result[1], (b'C', b"SELECT 0\0".to_vec()));
        assert_eq!(&result[0].1[..2], &(fields.len() as i16).to_be_bytes());
        let mut offset = 2;
        for (name, oid) in fields {
            assert!(result[0].1[offset..].starts_with(&[name.as_bytes(), b"\0"].concat()));
            offset += name.len() + 1;
            assert_eq!(&result[0].1[offset + 6..offset + 10], &oid.to_be_bytes());
            offset += 18;
        }
    }
    let unknown = query(
        &mut socket,
        "SELECT oid FROM pg_catalog.pg_missing_tablespace",
    );
    assert_eq!(unknown[0].0, b'E');
    assert!(
        unknown[0].1.windows(8).any(|w| w == b"C42P01\0M"),
        "{unknown:?}"
    );
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    send(&mut socket, b'X', b"");
    service.close();
}
