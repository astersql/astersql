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
fn text_row(body: &[u8]) -> Vec<Option<String>> {
    let count = i16::from_be_bytes(body[..2].try_into().unwrap()) as usize;
    let mut offset = 2;
    (0..count)
        .map(|_| {
            let length = i32::from_be_bytes(body[offset..offset + 4].try_into().unwrap());
            offset += 4;
            if length < 0 {
                None
            } else {
                let length = length as usize;
                let value = String::from_utf8(body[offset..offset + length].to_vec()).unwrap();
                offset += length;
                Some(value)
            }
        })
        .collect()
}

#[test]
fn postgres_create_table_simple_query() {
    let (domain, _) = astersql_session::runtime::CreateUnbootstrappedRootSessionForTest().unwrap();
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
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    let create = r#"CREATE TABLE public."PgCreateSimple" (
        "small" smallint,
        "regular" integer,
        "large" bigint,
        "single" real,
        "double" double precision,
        "amount" numeric(12, 3) DEFAULT 1.250,
        "ratio" decimal(8, 2),
        "enabled" boolean NOT NULL DEFAULT true,
        "fixed" char(4),
        "varying" varchar(24) DEFAULT 'hello',
        "body" text,
        "payload" bytea,
        "day" date,
        "clock" time(3),
        "stamp" timestamp(3) DEFAULT CURRENT_TIMESTAMP(3)
    )"#;
    assert_eq!(
        query(&mut socket, create)[0],
        (b'C', b"CREATE TABLE\0".to_vec())
    );

    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|schema| schema.name.lower == "test")
        .unwrap();
    let tables = domain.info_schema().SchemaTableInfos(&schema.name).unwrap();
    let table = tables
        .into_iter()
        .find(|table| table.name.lower == "pgcreatesimple")
        .unwrap();
    let table_id = crate::pg_oid::table_oid(table.model_meta.as_ref().unwrap().ID)
        .unwrap()
        .to_string();
    let namespace = crate::pg_oid::namespace_oid(schema.id).unwrap();
    let columns = include_str!("testdata/pg_datagrip/RetrieveColumns.sql")
        .replace('?', &namespace.to_string());
    let result = query(&mut socket, &columns);
    let rows = result
        .iter()
        .filter(|(tag, _)| *tag == b'D')
        .map(|(_, body)| text_row(body))
        .filter(|row| row[0].as_deref() == Some(table_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        15,
        "full DataGrip column result: {rows:?}; response: {result:?}"
    );
    let specs = rows
        .iter()
        .map(|row| row[6].as_deref().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        specs,
        [
            "smallint",
            "integer",
            "bigint",
            "real",
            "double precision",
            "numeric(12,3)",
            "numeric(8,2)",
            "boolean",
            "character(4)",
            "character varying(24)",
            "text",
            "bytea",
            "date",
            "time(3) without time zone",
            "timestamp(3) without time zone",
        ]
    );
    assert_eq!(rows[5][9].as_deref(), Some("1.250"));
    assert_eq!(rows[7][8].as_deref(), Some("t"));
    assert_eq!(rows[7][9].as_deref(), Some("true"));
    assert_eq!(rows[9][9].as_deref(), Some("'hello'::character varying"));
    let rejected = query(&mut socket, "CREATE TABLE public.bad_serial (id serial)");
    assert_eq!(
        rejected.iter().map(|message| message.0).collect::<Vec<_>>(),
        b"EZ"
    );
    assert!(rejected[0].1.windows(5).any(|bytes| bytes == b"0A000"));
    send(&mut socket, b'X', b"");
    service.close();
}
#[test]
fn simple_query_roundtrip() {
    let (domain, _) = astersql_session::runtime::CreateUnbootstrappedRootSessionForTest().unwrap();
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
    assert_eq!(&metadata[6..10], &23u32.to_be_bytes());
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
            "CREATE TABLE public.t (id INT, value VARCHAR(20))",
            "CREATE TABLE",
        ),
        (
            "INSERT INTO public.t VALUES (1, 'a'), (2, '')",
            "INSERT 0 2",
        ),
        ("UPDATE public.t SET value = 'b' WHERE id = 1", "UPDATE 1"),
        ("DELETE FROM public.t WHERE id = 2", "DELETE 1"),
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
    let selected = query(&mut socket, "SELECT id, value FROM public.t");
    assert_eq!(selected[1], (b'D', row(&[Some("1"), Some("b")])));
    assert_eq!(
        query(&mut socket, "  ; "),
        vec![(b'I', vec![]), (b'Z', b"I".to_vec())]
    );
    let multiple = query(&mut socket, "DELETE FROM public.t; SELECT 1");
    assert_eq!(multiple.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(multiple[0].1.windows(5).any(|w| w == b"0A000"));
    assert_eq!(
        query(&mut socket, "SELECT id FROM public.t")[1],
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
        query(&mut socket, "SELECT * FROM public.missing")
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
            postgresql_text_oid: None,
            postgresql_array_element: None,
            postgresql_record_fields: None,
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
fn pg_conflict_prepare_retains_clause_and_builds_analysis_sql_from_its_span() {
    let sql = "INSERT INTO t(id) VALUES (2) ON CONFLICT (id) WHERE id > 0 DO NOTHING RETURNING id";
    let plan = crate::pg_result::prepare_command(sql).unwrap();
    assert_eq!(plan.command, Some("INSERT"));
    assert!(plan.returning);
    let conflict = plan.postgresql_conflict.expect("conflict clause retained");
    assert_eq!(
        conflict.Target,
        astersql_parser_ast::PostgreSQLConflictTarget::Inference
    );
    let (start, end) = conflict.SourceSpan;
    assert_eq!(sql.get(start..end), Some(conflict.OriginalText.as_str()));
    let analysis_sql = plan.analysis_sql.expect("read-only analysis SQL");
    assert_eq!(analysis_sql, format!("{}{}", &sql[..start], &sql[end..]));
    assert!(!analysis_sql.contains("ON CONFLICT"));
    assert!(analysis_sql.contains("RETURNING id"));
    assert_eq!(
        crate::pg_result::command(sql).unwrap_err().0,
        "0A000",
        "simple Query remains gated before DML execution"
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
    let (domain, native_session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
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
        "CREATE TABLE public.pg_catalog_tx (id INT)",
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
        query(&mut socket, "INSERT INTO public.pg_catalog_tx VALUES (1)")[0].0,
        b'C'
    );
    let live = catalog_rows(&query(&mut socket, TRANSACTIONS_SQL));
    assert_eq!(live.len(), 1);
    // Native transaction IDs are observed through the native API; PG schemas
    // remain public/pg_catalog rather than exposing information_schema routing.
    let mut native_ids = Vec::new();
    for mut result in native_session
        .execute("SELECT ID FROM information_schema.tidb_trx")
        .unwrap()
    {
        while let Some(row) = result.next_row().unwrap() {
            native_ids.push(row[0].clone());
        }
        result.close().unwrap();
    }
    assert!(native_ids.iter().any(|id| Some(id) == live[0][0].as_ref()));
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
        query(&mut second, "INSERT INTO public.pg_catalog_tx VALUES (2)")[0].0,
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
    assert_eq!(before_rows[0][2].as_deref(), Some("public"));
    assert_eq!(before_rows.len(), 2); // public plus implicit pg_catalog

    assert_eq!(
        query(&mut socket, "CREATE DATABASE namespace_live")[0].0,
        b'C'
    );
    let result = query(&mut socket, sql);
    assert_eq!(result[0].0, b'T', "{result:?}");
    let rows = catalog_rows(&result);
    assert_eq!(rows, before_rows); // Other databases never become PG schemas.
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
        ("name", 19),
        ("description", 25),
        ("owner", 25),
    ] {
        assert!(result[0].1[offset..].starts_with(&[name.as_bytes(), b"\0"].concat()));
        offset += name.len() + 1;
        assert_eq!(&result[0].1[offset + 6..offset + 10], &oid.to_be_bytes());
        offset += 18;
    }
    let native = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "namespace_live")
        .unwrap();
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
    assert_eq!(current[0][2].as_deref(), Some("public"));
    assert_eq!(
        current[0][0],
        Some(namespace_oid(native.id).unwrap().to_string())
    );
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

#[test]
fn pg_introspection_namespace_session_state() {
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
    let connect = || {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let body = [
            196608u32.to_be_bytes().as_slice(),
            b"user\0root\0database\0test\0\0",
        ]
        .concat();
        socket
            .write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&body).unwrap();
        assert_eq!(read(&mut socket).0, b'R');
        while read(&mut socket).0 != b'Z' {}
        socket
    };
    let mut first = connect();
    let mut second = connect();
    let values = |socket: &mut TcpStream, sql: &str| {
        let result = query(socket, sql);
        assert_eq!(result[0].0, b'T', "{sql}: {result:?}");
        catalog_rows(&result)
    };
    assert_eq!(
        values(&mut first, "SELECT current_schema()"),
        vec![vec![Some("public".into())]]
    );
    assert_eq!(
        values(&mut first, "SELECT current_database()"),
        vec![vec![Some("test".into())]]
    );
    assert_eq!(
        values(&mut first, "SHOW search_path"),
        vec![vec![Some("public".into())]]
    );
    assert_eq!(
        query(&mut first, "CREATE DATABASE namespace_hidden")[0].0,
        b'C'
    );
    assert_eq!(
        values(
            &mut first,
            "SELECT nspname FROM pg_catalog.pg_namespace ORDER BY nspname"
        ),
        vec![vec![Some("pg_catalog".into())], vec![Some("public".into())]]
    );
    for (path, schema) in [
        ("pg_catalog, public", Some("pg_catalog")),
        ("public, pg_catalog", Some("public")),
        ("pg_catalog", Some("pg_catalog")),
        ("''", None),
    ] {
        assert_eq!(
            query(&mut first, &format!("SET search_path TO {path}"))[0].0,
            b'C'
        );
        assert_eq!(
            values(&mut first, "SELECT current_schema()"),
            vec![vec![schema.map(str::to_owned)]]
        );
        assert_eq!(
            values(&mut second, "SELECT current_schema()"),
            vec![vec![Some("public".into())]]
        );
    }
    assert_eq!(
        values(
            &mut first,
            "SELECT nspname FROM pg_namespace ORDER BY nspname"
        ),
        vec![vec![Some("pg_catalog".into())], vec![Some("public".into())]]
    );
    for (sql, state) in [
        ("SET search_path TO NULL", "0A000"),
        ("SET search_path TO public; SELECT 1", "42601"),
    ] {
        let messages = query(&mut first, sql);
        assert_eq!(messages[0].0, b'E', "{sql}");
        assert!(
            messages[0]
                .1
                .windows(7)
                .any(|part| part == [b"C", state.as_bytes(), b"\0"].concat()),
            "{sql}: {messages:?}"
        );
        assert_eq!(
            values(&mut first, "SELECT current_schema()"),
            vec![vec![None]]
        );
    }
    assert_eq!(query(&mut first, "RESET search_path")[0].0, b'C');
    assert_eq!(
        values(&mut first, "SHOW search_path"),
        vec![vec![Some("public".into())]]
    );
    assert_eq!(
        values(
            &mut first,
            "SELECT current_schema() FROM pg_catalog.pg_namespace LIMIT 1"
        ),
        vec![vec![Some("public".into())]]
    );
    assert_eq!(
        values(
            &mut first,
            "SELECT nspname FROM pg_namespace ORDER BY nspname"
        ),
        vec![vec![Some("pg_catalog".into())], vec![Some("public".into())]]
    );
    // Named Parse must not freeze the current schema; Execute sees later SET.
    send(&mut first, b'P', b"schema\0SELECT current_schema()\0\0\0");
    assert_eq!(read(&mut first).0, b'1');
    assert_eq!(
        query(&mut first, "SET search_path TO pg_catalog, public")[0].0,
        b'C'
    );
    send(&mut first, b'B', b"\0schema\0\0\0\0\0\0\0");
    send(&mut first, b'D', b"P\0");
    send(&mut first, b'E', &[0, 0, 0, 0, 0]);
    send(&mut first, b'S', &[]);
    assert_eq!(read(&mut first).0, b'2');
    assert_eq!(read(&mut first).0, b'T');
    assert_eq!(read(&mut first), (b'D', row(&[Some("pg_catalog")])));
    assert_eq!(read(&mut first).0, b'C');
    assert_eq!(read(&mut first).0, b'Z');
    assert_eq!(
        values(
            &mut first,
            "SELECT current_schema() FROM pg_catalog.pg_namespace LIMIT 1"
        ),
        vec![vec![Some("pg_catalog".into())]]
    );
    assert!(
        !domain
            .info_schema()
            .AllSchemas()
            .iter()
            .any(|schema| schema.name.lower == "public")
    );
    send(&mut first, b'X', &[]);
    send(&mut second, b'X', &[]);
    service.close();
}

#[test]
fn pg_introspection_names_live_tables() {
    let (domain, native) =
        astersql_session::runtime::CreateUnbootstrappedRootSessionForTest().unwrap();
    // Effective PG public is backed by a real current-database schema.
    native.execute("CREATE DATABASE test").unwrap();
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
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let body = [
            196608u32.to_be_bytes().as_slice(),
            format!("user\0root\0database\0{database}\0\0").as_bytes(),
        ]
        .concat();
        socket
            .write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&body).unwrap();
        assert_eq!(read(&mut socket).0, b'R');
        while read(&mut socket).0 != b'Z' {}
        socket
    };
    let mut socket = connect("test");
    let ok = |socket: &mut TcpStream, sql: &str| {
        let messages = query(socket, sql);
        assert!(messages.iter().any(|m| m.0 == b'C'), "{sql}: {messages:?}");
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
        messages
    };
    ok(
        &mut socket,
        "CREATE TABLE names_live (id INT, note VARCHAR(80))",
    );
    ok(
        &mut socket,
        "INSERT INTO names_live VALUES (1, 'public.names_live')",
    );
    let result = ok(&mut socket, "SELECT id, note FROM public.names_live");
    assert_eq!(
        catalog_rows(&result),
        vec![vec![Some("1".into()), Some("public.names_live".into())]]
    );
    assert_eq!(
        catalog_rows(&ok(
            &mut socket,
            "WITH names_live AS (SELECT 9 AS id) SELECT id FROM names_live"
        )),
        vec![vec![Some("9".into())]]
    );
    native
        .execute("CREATE VIEW test.names_view AS SELECT id FROM test.names_live")
        .unwrap();
    ok(&mut socket, "DROP VIEW public.names_view");
    ok(&mut socket, "CREATE DATABASE names_other");
    let mut other = connect("names_other");
    ok(
        &mut other,
        "CREATE TABLE public.names_live (id INT, note VARCHAR(80))",
    );
    ok(&mut other, "INSERT INTO public.names_live VALUES (2, NULL)");
    for sql in [
        "SELECT id FROM names_live",
        "SELECT id FROM public.names_live",
        "SELECT id FROM test.public.names_live",
        "SELECT \"id\" FROM \"public\".\"names_live\" /* public.fake */",
    ] {
        assert_eq!(
            catalog_rows(&ok(&mut socket, sql)),
            vec![vec![Some("1".into())]],
            "{sql}"
        );
    }
    assert_eq!(
        catalog_rows(&ok(&mut other, "SELECT id, note FROM public.names_live")),
        vec![vec![Some("2".into()), None]]
    );
    ok(
        &mut socket,
        "UPDATE public.names_live SET note = 'FROM public.fake' WHERE id = 1",
    );
    assert_eq!(
        catalog_rows(&ok(
            &mut socket,
            "SELECT a.id FROM public.names_live AS a JOIN public.names_live AS b ON a.id = b.id"
        )),
        vec![vec![Some("1".into())]]
    );
    ok(
        &mut socket,
        "ALTER TABLE public.names_live ADD COLUMN extra INT",
    );
    ok(&mut socket, "DELETE FROM public.names_live WHERE id = 1");
    assert!(catalog_rows(&ok(&mut socket, "SELECT id FROM public.names_live")).is_empty());
    ok(
        &mut socket,
        "INSERT INTO test.public.names_live VALUES (3, NULL, NULL)",
    );
    // Both protocol entrypoints must use the same name mapping.
    send(
        &mut socket,
        b'P',
        b"names\0SELECT id FROM public.names_live WHERE id = $1\0\0\x01\0\0\0\x17",
    );
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'B', b"\0names\0\0\0\0\x01\0\0\0\x013\0\0");
    send(&mut socket, b'D', b"P\0");
    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'2');
    assert_eq!(read(&mut socket).0, b'T');
    assert_eq!(read(&mut socket), (b'D', row(&[Some("3")])));
    assert_eq!(read(&mut socket).0, b'C');
    assert_eq!(read(&mut socket).0, b'Z');
    for (sql, state) in [
        ("SELECT id FROM names_other.public.names_live", "0A000"),
        ("SELECT id FROM names_other.names_live", "0A000"),
        (
            "ALTER TABLE public.names_live EXCHANGE PARTITION p WITH TABLE names_other.names_live",
            "0A000",
        ),
        ("SELECT id FROM \"PUBLIC\".names_live", "0A000"),
        ("SELECT id FROM public.\"\"", "42601"),
    ] {
        let messages = query(&mut socket, sql);
        assert_eq!(messages[0].0, b'E', "{sql}");
        assert!(
            messages[0]
                .1
                .windows(7)
                .any(|part| part == [b"C", state.as_bytes(), b"\0"].concat()),
            "{sql}: {messages:?}"
        );
        ok(&mut socket, "SELECT 1");
    }
    ok(&mut socket, "SET search_path TO pg_catalog");
    let hidden = query(&mut socket, "SELECT id FROM names_live");
    assert_eq!(hidden[0].0, b'E');
    assert!(hidden[0].1.windows(7).any(|part| part == b"C42P01\0"));
    assert_eq!(
        catalog_rows(&ok(&mut socket, "SELECT id FROM public.names_live")),
        vec![vec![Some("3".into())]]
    );
    ok(&mut socket, "RESET search_path");
    ok(&mut socket, "TRUNCATE TABLE public.names_live");
    ok(&mut socket, "DROP TABLE public.names_live");
    send(&mut socket, b'X', &[]);
    send(&mut other, b'X', &[]);
    service.close();
}

#[test]
fn pg_core_dialect_simple_query() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(server.clone());
    let address = server.postgres_listener_addr().unwrap();
    let mut mysql = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut mysql);
    struct Client(std::process::Child);
    impl Drop for Client {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Client(
        std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/pg_core_dialect.py"
            ))
            .arg(address.port().to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    use std::io::BufRead;
    let mut output = std::io::BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        if output.read_line(&mut line).unwrap() == 0 {
            break;
        }
        println!("{}", line.trim());
        if line.starts_with("stage ") {
            if line.trim() == "stage 4" {
                mysql_packet(&mut mysql, 0, &[0x1f]); // COM_RESET_CONNECTION
                assert_eq!(mysql_read(&mut mysql).first(), Some(&0));
            }
            if line.trim() == "stage 5" {
                mysql_packet(
                    &mut mysql,
                    0,
                    b"\x11root\0\0\0\x2d\0mysql_native_password\0",
                );
                assert_eq!(mysql_read(&mut mysql).first(), Some(&0));
            }
            assert_eq!(
                mysql_query(&mut mysql, "SELECT 314159 AS pg_core_mysql_identity"),
                vec![vec![Some(b"314159".to_vec())]]
            );
            child
                .0
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"continue\n")
                .unwrap();
        }
    }
    let status = child.0.wait().unwrap();
    mysql_packet(&mut mysql, 0, &[1]);
    server.close();
    assert!(TcpStream::connect(address).is_err());
    assert!(status.success(), "libpq lifecycle failed");
    let observations = crate::runtime::DIALECT_EXECUTIONS.lock().unwrap().clone();
    let pg = observations
        .iter()
        .filter(|(sql, _)| sql.contains("pg_core_pg_identity"))
        .collect::<Vec<_>>();
    let native = observations
        .iter()
        .filter(|(sql, _)| sql.contains("pg_core_mysql_identity"))
        .collect::<Vec<_>>();
    assert_eq!(pg.len(), 10, "missing real PG executions: {pg:?}");
    assert_eq!(native.len(), 8);
    assert!(
        pg.iter()
            .all(|(_, dialect)| *dialect == SqlDialect::PostgreSQL),
        "worker identities: {pg:?}"
    );
    assert!(
        native
            .iter()
            .all(|(_, dialect)| *dialect == SqlDialect::MySQL)
    );
}

#[test]
fn pg_core_integer_cast_parse_boundary() {
    // Exercise the exact command gate used before real canonical execution.
    assert_eq!(
        crate::pg_result::command("SELECT 1::int4").unwrap(),
        Some("SELECT")
    );
    assert_eq!(
        crate::pg_result::command("SELECT CAST(1 AS int8)").unwrap(),
        Some("SELECT")
    );
    assert_eq!(
        crate::pg_result::command("SELECT 1::").unwrap_err().0,
        "42601"
    );
}

#[test]
fn pg_core_integer_cast_parse_boundary_libpq() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(server.clone());
    let address = server.postgres_listener_addr().unwrap();
    let mut mysql = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut mysql);
    mysql_packet(&mut mysql, 0, b"\x03SELECT 1::int4");
    assert_eq!(mysql_read(&mut mysql).first(), Some(&0xff));
    assert_eq!(
        mysql_query(&mut mysql, "SELECT CAST(1 AS SIGNED)"),
        vec![vec![Some(b"1".to_vec())]]
    );
    mysql_packet(&mut mysql, 0, &[1]);
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_cast_syntax.py"
        ))
        .arg(address.port().to_string())
        .status()
        .unwrap();
    server.close();
    assert!(TcpStream::connect(address).is_err());
    assert!(
        status.success(),
        "libpq integer cast syntax regression failed"
    );
}

#[test]
fn pg_core_int4_wire_semantics() {
    use crate::pg_client_integration_test::start_client_server;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(server.clone());
    let address = server.postgres_listener_addr().unwrap();
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_int4.py"
        ))
        .arg(address.port().to_string())
        .status()
        .unwrap();
    server.close();
    assert!(TcpStream::connect(address).is_err());
    assert!(status.success(), "libpq int4 semantics failed");
}
#[test]
fn pg_core_int8_wire_semantics() {
    use crate::pg_client_integration_test::start_client_server;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(server.clone());
    let address = server.postgres_listener_addr().unwrap();
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_int8.py"
        ))
        .arg(address.port().to_string())
        .status()
        .unwrap();
    server.close();
    assert!(TcpStream::connect(address).is_err());
    assert!(status.success(), "libpq int8 semantics failed");
}

#[test]
fn pg_function_01() {
    signed_abs_protocol_contract();
}

fn signed_abs_protocol_contract() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (name, oid, width) in [("int4", 23u32, 4i16), ("int8", 20, 8)] {
        let maximum = if width == 4 {
            i32::MAX as i64
        } else {
            i64::MAX
        };
        for value in [
            Some(0),
            Some(1),
            Some(-1),
            Some(maximum),
            Some(-maximum),
            None,
        ] {
            let source = value.map_or_else(|| "NULL".to_owned(), |v| format!("({v})"));
            let messages = query(
                &mut socket,
                &format!("SELECT pg_catalog.abs({source}::{name}) AS v"),
            );
            assert!(
                !messages.iter().any(|(tag, _)| *tag == b'E'),
                "{name}: {messages:?}"
            );
            let description = &messages
                .iter()
                .find(|(tag, _)| *tag == b'T')
                .expect("RowDescription")
                .1;
            let mut expected = vec![0, 1, b'v', 0];
            expected.extend_from_slice(&0u32.to_be_bytes());
            expected.extend_from_slice(&0i16.to_be_bytes());
            expected.extend_from_slice(&oid.to_be_bytes());
            expected.extend_from_slice(&width.to_be_bytes());
            expected.extend_from_slice(&(-1i32).to_be_bytes());
            expected.extend_from_slice(&0i16.to_be_bytes());
            assert_eq!(description, &expected, "{name} result identity");
            assert_eq!(
                messages.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
                row(&[value.map(|v| v.abs().to_string()).as_deref()])
            );
        }
    }
    for sql in [
        "SELECT pg_catalog.abs(NULL::int4) AS v",
        "SELECT pg_catalog.abs(NULL::int8) AS v",
    ] {
        let messages = query(&mut socket, sql);
        assert_eq!(
            messages.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[None])
        );
    }
    for sql in [
        "SELECT pg_catalog.abs((-2147483648)::int4) AS v",
        "SELECT pg_catalog.abs((-9223372036854775808)::int8) AS v",
    ] {
        let messages = query(&mut socket, sql);
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("overflow ErrorResponse")
            .1;
        assert!(
            error.windows(7).any(|bytes| bytes == b"C22003\0"),
            "{messages:?}"
        );
        let mut fields = std::collections::BTreeMap::new();
        let mut offset = 0;
        while error[offset] != 0 {
            let tag = error[offset];
            offset += 1;
            let length = error[offset..].iter().position(|byte| *byte == 0).unwrap();
            fields.insert(
                tag,
                std::str::from_utf8(&error[offset..offset + length]).unwrap(),
            );
            offset += length + 1;
        }
        assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
        assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
        assert_eq!(fields.get(&b'C'), Some(&"22003"));
        assert!(!fields[&b'M'].is_empty());
        println!(
            "{sql}: evaluation error {fields:?}; D/H/P present={:?}",
            [b'D', b'H', b'P'].map(|tag| fields.contains_key(&tag))
        );
        assert_eq!(
            messages.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
            [b'E', b'Z']
        );
        assert_eq!(messages.last().unwrap().1, [b'I']);
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        assert!(
            query(&mut socket, "SELECT 1::int4")
                .iter()
                .any(|(tag, _)| *tag == b'D')
        );
    }
    // Parse/statement Describe, then rebind the same statement with text and
    // binary output and NULL. The actual session owns all result identities.
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
    for (name, oid, width) in [("int4", 23u32, 4i16), ("int8", 20, 8)] {
        let sql = format!("SELECT pg_catalog.abs($1::{name}) AS v");
        let mut parse = [b"\0".as_slice(), sql.as_bytes(), b"\0", &1i16.to_be_bytes()].concat();
        parse.extend_from_slice(&oid.to_be_bytes());
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|(tag, _)| *tag == b't').unwrap().1,
            [&1i16.to_be_bytes()[..], &oid.to_be_bytes()].concat()
        );
        let statement_description = messages
            .iter()
            .find(|(tag, _)| *tag == b'T')
            .unwrap()
            .1
            .clone();
        assert_eq!(&statement_description[10..14], &oid.to_be_bytes());
        assert_eq!(&statement_description[14..16], &width.to_be_bytes());
        let maximum = if width == 4 {
            i32::MAX as i64
        } else {
            i64::MAX
        };
        let precise = if width == 4 {
            maximum
        } else {
            9_007_199_254_740_993
        };
        for format in [0i16, 1] {
            for value in [
                Some(0i64),
                Some(-1),
                Some(2),
                Some(maximum),
                Some(-maximum),
                Some(-precise),
                None,
            ] {
                let text = value.map(|v| v.to_string());
                let mut bind = vec![0, 0, 0, 0, 0, 1]; // unnamed portal/statement, no input formats, one parameter
                match text.as_deref() {
                    Some(value) => {
                        bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
                        bind.extend_from_slice(value.as_bytes());
                    }
                    None => bind.extend_from_slice(&(-1i32).to_be_bytes()),
                }
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                send(&mut socket, b'B', &bind);
                send(&mut socket, b'D', b"P\0");
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'S', &[]);
                let messages = ready(&mut socket);
                assert!(
                    !messages.iter().any(|(tag, _)| *tag == b'E'),
                    "{messages:?}"
                );
                assert_eq!(
                    messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                    [b'2', b'T', b'D', b'C', b'Z'],
                    "one portal description; Execute adds no description"
                );
                let mut expected_description = statement_description.clone();
                let end = expected_description.len();
                expected_description[end - 2..].copy_from_slice(&format.to_be_bytes());
                assert_eq!(
                    messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1,
                    expected_description
                );
                let expected = if value.is_none() {
                    row(&[None])
                } else if format == 0 {
                    row(&[Some(&value.unwrap().abs().to_string())])
                } else {
                    let bytes = if width == 4 {
                        (value.unwrap().abs() as i32).to_be_bytes().to_vec()
                    } else {
                        value.unwrap().abs().to_be_bytes().to_vec()
                    };
                    [
                        &1i16.to_be_bytes()[..],
                        &(bytes.len() as i32).to_be_bytes(),
                        &bytes,
                    ]
                    .concat()
                };
                assert_eq!(
                    messages.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
                    expected
                );
            }
        }
        // Overflow belongs to Execute after the typed parameter was bound;
        // Sync must restore the same connection for an independent query.
        let minimum = if width == 4 {
            i32::MIN as i64
        } else {
            i64::MIN
        }
        .to_string();
        let mut bind = vec![0, 0, 0, 0, 0, 1];
        bind.extend_from_slice(&(minimum.len() as i32).to_be_bytes());
        bind.extend_from_slice(minimum.as_bytes());
        bind.extend_from_slice(&0i16.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'H', &[]);
        assert_eq!(read(&mut socket), (b'2', vec![]));
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let overflow = ready(&mut socket);
        assert_eq!(
            overflow.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'E', b'Z']
        );
        let fields = &overflow[0].1;
        for field in [b"SERROR\0".as_slice(), b"VERROR\0", b"C22003\0"] {
            assert!(
                fields.windows(field.len()).any(|bytes| bytes == field),
                "{overflow:?}"
            );
        }
        assert_eq!(overflow[1].1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
        let messages = query(
            &mut socket,
            &format!("SELECT pg_catalog.abs((-1)::{name}) AS v WHERE false"),
        );
        assert!(messages.iter().any(|(tag, _)| *tag == b'T'));
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D' || *tag == b'E'));
        assert_eq!(
            messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1,
            statement_description
        );
    }
    send(&mut socket, b'X', &[]);
    service.close();
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_01_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(server.clone());
    let mut socket = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);
    mysql_packet(&mut socket, 0, b"\x03SELECT ABS(CAST(-1 AS SIGNED)) AS v");
    assert_eq!(mysql_read(&mut socket), [1]);
    let column = mysql_read(&mut socket);
    let mut offset = 0;
    for _ in 0..6 {
        let length = usize::from(column[offset]);
        offset += length + 1;
    }
    assert_eq!(column[offset], 12);
    let metadata = &column[offset + 1..offset + 13];
    println!(
        "pg_function_01_mysql metadata: charset={} length={} code={} flags={} decimals={}",
        u16::from_le_bytes(metadata[..2].try_into().unwrap()),
        u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
        metadata[6],
        u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
        metadata[9]
    );
    assert_eq!(metadata[6], 8, "MySQL ABS retains BIGINT wire code");
    assert_eq!(u16::from_le_bytes(metadata[..2].try_into().unwrap()), 63);
    assert_eq!(u32::from_le_bytes(metadata[2..6].try_into().unwrap()), 22);
    assert_eq!(u16::from_le_bytes(metadata[7..9].try_into().unwrap()), 129);
    assert_eq!(metadata[9], 0);
    assert_eq!(mysql_read(&mut socket)[0], 0xfe);
    assert_eq!(mysql_read(&mut socket), b"\x011");
    let eof = mysql_read(&mut socket);
    assert_eq!(&eof[..3], &[0xfe, 0, 0]);
    assert!(mysql_query(&mut socket, "SHOW WARNINGS").is_empty());
    mysql_packet(&mut socket, 0, &[1]);
}

#[test]
fn pg_function_02() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    for prefix in ["", "pg_catalog."] {
        for call in [
            "ifnull(1, 2)",
            "ifnull(NULL::int4, 2)",
            "ifnull('bad'::int4, 2)",
            "date_format('2026-01-01', '%Y')",
            "date_format(NULL::text, '%Y')",
            "date_format('bad'::int4, '%Y')",
            "json_extract('{}', '$.a')",
            "json_extract(NULL::text, '$.a')",
            "json_extract('bad'::int4, '$.a')",
            "tidb_version()",
            "tidb_version(NULL::int4)",
            "tidb_version('bad'::int4)",
            "astersql_contract_missing_fn_20261023()",
        ] {
            let sql = format!("SELECT {prefix}{call} AS v");
            let messages = query(&mut socket, &sql);
            let error = &messages
                .iter()
                .find(|(tag, _)| *tag == b'E')
                .unwrap_or_else(|| panic!("expected name rejection: {sql}: {messages:?}"))
                .1;
            assert!(
                error.windows(7).any(|bytes| bytes == b"C42883\0"),
                "{sql}: {messages:?}"
            );
            let mut fields = std::collections::BTreeMap::new();
            let mut offset = 0;
            while error[offset] != 0 {
                let tag = error[offset];
                offset += 1;
                let length = error[offset..].iter().position(|byte| *byte == 0).unwrap();
                fields.insert(
                    tag,
                    std::str::from_utf8(&error[offset..offset + length]).unwrap(),
                );
                offset += length + 1;
            }
            assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'C'), Some(&"42883"));
            assert!(!fields[&b'M'].is_empty());
            println!(
                "{sql}: name analysis rejection {fields:?}; D/H/P present={:?}",
                [b'D', b'H', b'P'].map(|tag| fields.contains_key(&tag))
            );
            assert_eq!(
                messages.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
                [b'E', b'Z']
            );
            assert_eq!(messages.last().unwrap().1, [b'I']);
            // Parse must reject the name before a Bind can supply or convert values.
            let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat();
            send(&mut socket, b'P', &parse);
            send(&mut socket, b'S', &[]);
            let mut extended = Vec::new();
            loop {
                let message = read(&mut socket);
                let done = message.0 == b'Z';
                extended.push(message);
                if done {
                    break;
                }
            }
            assert_eq!(
                extended.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
                [b'E', b'Z'],
                "Parse stage: {sql}: {extended:?}"
            );
            assert!(
                extended[0].1.windows(7).any(|bytes| bytes == b"C42883\0"),
                "{sql}: {extended:?}"
            );
            assert_eq!(extended.last().unwrap().1, [b'I']);
            assert!(
                query(&mut socket, "SELECT 1::int4")
                    .iter()
                    .any(|(tag, _)| *tag == b'D')
            );
        }
    }
    send(&mut socket, b'X', &[]);
    service.close();
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_02_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(server.clone());
    let mut socket = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);

    for (sql, expected) in [
        ("SELECT IFNULL(NULL, 2) AS v", Some("2")),
        ("SELECT DATE_FORMAT('2026-01-01', '%Y') AS v", Some("2026")),
        ("SELECT JSON_EXTRACT('{\"a\":1}', '$.a') AS v", Some("1")),
        ("SELECT TIDB_VERSION() AS v", None),
    ] {
        let mut command = vec![3];
        command.extend_from_slice(sql.as_bytes());
        mysql_packet(&mut socket, 0, &command);
        assert_eq!(mysql_read(&mut socket), [1], "{sql}");
        let column = mysql_read(&mut socket);
        let mut offset = 0;
        for _ in 0..6 {
            let length = usize::from(column[offset]);
            offset += length + 1;
        }
        assert_eq!(column[offset], 12);
        let metadata = &column[offset + 1..offset + 13];
        println!(
            "{sql}: charset={} length={} code={} flags={} decimals={}",
            u16::from_le_bytes(metadata[..2].try_into().unwrap()),
            u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
            metadata[6],
            u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
            metadata[9]
        );
        assert_eq!(mysql_read(&mut socket)[0], 0xfe);
        let value = mysql_read(&mut socket);
        assert_ne!(value[0], 0xfb);
        if let Some(expected) = expected {
            assert_eq!(&value[1..], expected.as_bytes(), "{sql}");
        } else {
            assert!(value.len() > 1);
        }
        let eof = mysql_read(&mut socket);
        assert_eq!(&eof[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(&mut socket, "SHOW WARNINGS").is_empty());
    }
    mysql_packet(&mut socket, 0, &[1]);
}

#[test]
fn pg_function_03() {
    fn connect(isolated: bool) -> (Cleanup, TcpStream) {
        let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
        let mut driver = ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::InsecureRootOnly,
        );
        if isolated {
            driver.function_registry = Some(Arc::new(
                astersql_expression::formal_registry::funcs.snapshot_without_factory("abs"),
            ));
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let service = PgService::start(
            listener,
            Arc::new(driver),
            Arc::new(CanonicalConnectionDomain::new(domain)),
            false,
        )
        .unwrap();
        let cleanup = Cleanup(service);
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
        socket
            .write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&body).unwrap();
        while read(&mut socket).0 != b'Z' {}
        (cleanup, socket)
    }
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let (cleanup, mut socket) = connect(true);
    let (healthy_cleanup, mut ordinary) = connect(false);
    let address = socket.peer_addr().unwrap();
    let healthy_address = ordinary.peer_addr().unwrap();
    for (sql, state, message) in [
        ("SELECT abs((-1)::int4) AS v", "0A000", "not linked"),
        ("SELECT abs(NULL::int4) AS v", "0A000", "not linked"),
        (
            "SELECT pg_catalog.abs((-1)::int4) AS v",
            "0A000",
            "not linked",
        ),
        (
            "SELECT astersql_missing_function_03(1::int4) AS v",
            "42883",
            "astersql_missing_function_03",
        ),
    ] {
        for parse in [false, true] {
            let messages = if parse {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                // Parse describes the visible signature without constructing its
                // evaluator. The missing factory must fail when Execute builds it.
                send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'S', &[]);
                let mut messages = Vec::new();
                loop {
                    let m = read(&mut socket);
                    let done = m.0 == b'Z';
                    messages.push(m);
                    if done {
                        break;
                    }
                }
                messages
            } else {
                query(&mut socket, sql)
            };
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                if parse {
                    b"12EZ".as_slice()
                } else {
                    b"EZ".as_slice()
                },
                "{sql}: {messages:?}"
            );
            let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
            let mut fields = std::collections::BTreeMap::new();
            let mut offset = 0;
            while error[offset] != 0 {
                let tag = error[offset];
                offset += 1;
                let length = error[offset..].iter().position(|b| *b == 0).unwrap();
                fields.insert(
                    tag,
                    std::str::from_utf8(&error[offset..offset + length]).unwrap(),
                );
                offset += length + 1;
            }
            println!("{sql} parse={parse}: {fields:?}");
            assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'C'), Some(&state));
            assert!(fields[&b'M'].contains(message));
            assert!(
                [b'D', b'H', b'P']
                    .iter()
                    .all(|tag| !fields.contains_key(tag))
            );
            assert_eq!(messages.last().unwrap().1, [b'I']);
            assert_eq!(
                query(&mut socket, "SELECT 1::int4")
                    .iter()
                    .find(|m| m.0 == b'D')
                    .unwrap()
                    .1,
                row(&[Some("1")])
            );
            // A concurrent ordinary connection still uses the real ABS factory.
            let healthy = query(&mut ordinary, "SELECT abs((-1)::int4) AS v");
            assert_eq!(
                healthy.iter().find(|m| m.0 == b'D').unwrap().1,
                row(&[Some("1")])
            );
        }
    }
    for (sql, value) in [
        ("SELECT abs((-1)::int4) AS v", Some("1")),
        ("SELECT abs(NULL::int4) AS v", None),
    ] {
        let messages = query(&mut ordinary, sql);
        assert_eq!(messages.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
        let expected = [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &23u32.to_be_bytes(),
            &4i16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat();
        assert_eq!(messages[0].1, expected);
        assert_eq!(messages[1].1, row(&[value]));
    }
    send(&mut socket, b'X', &[]);
    send(&mut ordinary, b'X', &[]);
    drop(socket);
    drop(ordinary);
    drop(cleanup);
    drop(healthy_cleanup);
    assert!(TcpStream::connect(address).is_err());
    assert!(TcpStream::connect(healthy_address).is_err());
}

#[test]
fn pg_function_04() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    let messages = query(&mut socket, "SELECT round(4::int4,4::int4) AS v");
    assert_eq!(
        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ",
        "{messages:?}"
    );
    assert_eq!(messages[1].1, row(&[Some("4.0000")]));

    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }
    assert_eq!(messages[0].1, description(1700, -1, 0));
    for (sql, expected, oid, width) in [
        ("SELECT abs((-1)::int4) AS v", Some("1"), 23, 4),
        ("SELECT abs((-1)::int8) AS v", Some("1"), 20, 8),
        ("SELECT round(NULL::int4,4::int4) AS v", None, 1700, -1),
        ("SELECT round(4::int4,NULL::int4) AS v", None, 1700, -1),
        (
            "SELECT round((-15)::int4,(-1)::int4) AS v",
            Some("-20"),
            1700,
            -1,
        ),
        (
            "SELECT round(2147483647::int4,4::int4) AS v",
            Some("2147483647.0000"),
            1700,
            -1,
        ),
    ] {
        let m = query(&mut socket, sql);
        assert_eq!(
            m.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {m:?}"
        );
        assert_eq!(m[0].1, description(oid, width, 0));
        assert_eq!(m[1].1, row(&[expected]));
    }
    for (sql, state) in [
        ("SELECT abs('1'::text) AS v", "42883"),
        ("SELECT abs(NULL::text) AS v", "42883"),
        ("SELECT abs() AS v", "42883"),
        ("SELECT round('invalid'::int4,4::int4) AS v", "22P02"),
    ] {
        for extended in [false, true] {
            let m = if extended {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'S', &[]);
                ready(&mut socket)
            } else {
                query(&mut socket, sql)
            };
            println!("{sql}: {m:?}");
            let tags = m.iter().map(|m| m.0).collect::<Vec<_>>();
            assert!(
                tags == b"EZ" || (extended && tags == b"12EZ"),
                "{sql} extended={extended}: {tags:?}"
            );
            println!("error phase extended={extended}: {tags:?}");
            let mut fields = std::collections::BTreeMap::new();
            let e = &m.iter().find(|m| m.0 == b'E').unwrap().1;
            let mut offset = 0;
            while e[offset] != 0 {
                let tag = e[offset];
                offset += 1;
                let len = e[offset..].iter().position(|b| *b == 0).unwrap();
                fields.insert(tag, std::str::from_utf8(&e[offset..offset + len]).unwrap());
                offset += len + 1;
            }
            assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'C'), Some(&state));
            assert!(!fields[&b'M'].is_empty());
            println!("error fields={fields:?}");
            assert!(
                [b'D', b'H', b'P']
                    .iter()
                    .all(|tag| !fields.contains_key(tag))
            );
            assert_eq!(m.last().unwrap().1, [b'I']);
            assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));
        }
    }
    let zero = query(
        &mut socket,
        "SELECT round(4::int4,4::int4) AS v WHERE false",
    );
    assert_eq!(zero.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    assert_eq!(zero[0].1, description(1700, -1, 0));
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut m = Vec::new();
        loop {
            let x = read(socket);
            let end = x.0 == b'Z';
            m.push(x);
            if end {
                return m;
            }
        }
    }
    let sql = "SELECT round($1::int4,4::int4) AS v";
    send(
        &mut socket,
        b'P',
        &[
            b"\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &23u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"S\0");
    send(&mut socket, b'S', &[]);
    let m = ready(&mut socket);
    assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ", "{m:?}");
    assert_eq!(
        m[1].1,
        [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
    );
    assert_eq!(m[2].1, description(1700, -1, 0));
    for (format, value) in [
        (0i16, Some("4")),
        (1, Some("4")),
        (0, None),
        (1, None),
        (0, Some("5")),
    ] {
        let mut bind = vec![0, 0, 0, 0, 0, 1];
        match value {
            Some(v) => {
                bind.extend_from_slice(&(v.len() as i32).to_be_bytes());
                bind.extend_from_slice(v.as_bytes());
            }
            None => bind.extend_from_slice(&(-1i32).to_be_bytes()),
        }
        bind.extend_from_slice(&1i16.to_be_bytes());
        bind.extend_from_slice(&format.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'D', b"P\0");
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let m = ready(&mut socket);
        assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{m:?}");
        assert_eq!(m[1].1, description(1700, -1, format));
        let expected = match value {
            None => row(&[None]),
            Some(v) if format == 0 => row(&[Some(&format!("{v}.0000"))]),
            Some(v) => {
                assert_eq!(v, "4");
                // numeric_recv: ndigits=1, weight=0, positive, dscale=4, digit=4.
                let bytes = vec![0, 1, 0, 0, 0, 0, 0, 4, 0, 4];
                [
                    &1i16.to_be_bytes()[..],
                    &(bytes.len() as i32).to_be_bytes(),
                    &bytes,
                ]
                .concat()
            }
        };
        assert_eq!(m[2].1, expected);
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_05() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }

    for (sql, expected) in [
        ("SELECT substr('1234',3::int4) AS v", Some("34")),
        ("SELECT substr(NULL,3::int4) AS v", None),
        ("SELECT substr(NULL::text,3::int4) AS v", None),
        ("SELECT substr('1234',NULL::int4) AS v", None),
    ] {
        let m = query(&mut socket, sql);
        assert_eq!(
            m.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {m:?}"
        );
        assert_eq!(m[0].1, description(25, -1, 0), "{sql}");
        assert_eq!(m[1].1, row(&[expected]), "{sql}");
    }
    for (sql, state) in [
        ("SELECT substr(NULL::int4,3::int4) AS v", "42883"),
        ("SELECT substr('1234',NULL::text) AS v", "42883"),
    ] {
        for extended in [false, true] {
            let m = if extended {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'S', &[]);
                ready(&mut socket)
            } else {
                query(&mut socket, sql)
            };
            println!("{sql}: {m:?}");
            let tags = m.iter().map(|m| m.0).collect::<Vec<_>>();
            assert!(
                tags == b"EZ" || (extended && tags == b"12EZ"),
                "{sql} extended={extended}: {tags:?}"
            );
            println!("error phase extended={extended}: {tags:?}");
            let mut fields = std::collections::BTreeMap::new();
            let e = &m.iter().find(|m| m.0 == b'E').unwrap().1;
            let mut offset = 0;
            while e[offset] != 0 {
                let tag = e[offset];
                offset += 1;
                let len = e[offset..].iter().position(|b| *b == 0).unwrap();
                fields.insert(tag, std::str::from_utf8(&e[offset..offset + len]).unwrap());
                offset += len + 1;
            }
            assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'C'), Some(&state));
            assert!(!fields[&b'M'].is_empty());
            println!("error fields={fields:?}");
            assert!(
                [b'D', b'H', b'P']
                    .iter()
                    .all(|tag| !fields.contains_key(tag))
            );
            assert_eq!(m.last().unwrap().1, [b'I']);
            assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));
        }
    }
    let zero = query(
        &mut socket,
        "SELECT substr('1234',3::int4) AS v WHERE false",
    );
    assert_eq!(zero.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    assert_eq!(zero[0].1, description(25, -1, 0));
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut m = Vec::new();
        loop {
            let x = read(socket);
            let end = x.0 == b'Z';
            m.push(x);
            if end {
                return m;
            }
        }
    }
    let sql = "SELECT substr($1,3::int4) AS v";
    send(
        &mut socket,
        b'P',
        &[
            b"\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &0u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"S\0");
    send(&mut socket, b'S', &[]);
    let m = ready(&mut socket);
    assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ", "{m:?}");
    assert_eq!(
        m[1].1,
        [&1i16.to_be_bytes()[..], &25u32.to_be_bytes()].concat()
    );
    assert_eq!(m[2].1, description(25, -1, 0));
    for (format, value) in [
        (0i16, Some("1234")),
        (1, Some("1234")),
        (0, None),
        (1, None),
        (0, Some("1235")),
    ] {
        let mut bind = vec![0, 0, 0, 0, 0, 1];
        match value {
            Some(v) => {
                bind.extend_from_slice(&(v.len() as i32).to_be_bytes());
                bind.extend_from_slice(v.as_bytes());
            }
            None => bind.extend_from_slice(&(-1i32).to_be_bytes()),
        }
        bind.extend_from_slice(&1i16.to_be_bytes());
        bind.extend_from_slice(&format.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'D', b"P\0");
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let m = ready(&mut socket);
        assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{m:?}");
        assert_eq!(m[1].1, description(25, -1, format));
        let expected = row(&[value.map(|v| &v[2..])]);
        assert_eq!(m[2].1, expected);
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_06() {
    use astersql_expression::ast::SqlDialect;
    use astersql_expression::formal_registry::funcs;
    let mysql_names = funcs.names_for_dialect(SqlDialect::MySQL);
    for name in [
        "abs",
        "ifnull",
        "date_format",
        "json_extract",
        "tidb_version",
    ] {
        assert!(mysql_names.iter().any(|n| n == name), "{name}");
    }
    // Exercise the same real TCP ABS contract after adding private selector
    // coverage, including extended Describe, rebinding and error recovery.
    signed_abs_protocol_contract();
    assert_eq!(funcs.names_for_dialect(SqlDialect::MySQL), mysql_names);
}

#[test]
fn pg_function_07() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    use astersql_expression::{ast::SqlDialect, formal_registry::funcs};
    let before = funcs.names_for_dialect(SqlDialect::MySQL);
    let mut description = vec![0, 1, b'v', 0];
    description.extend_from_slice(&0u32.to_be_bytes());
    description.extend_from_slice(&0i16.to_be_bytes());
    description.extend_from_slice(&25u32.to_be_bytes());
    description.extend_from_slice(&(-1i16).to_be_bytes());
    description.extend_from_slice(&(-1i32).to_be_bytes());
    description.extend_from_slice(&0i16.to_be_bytes());
    // Existing CONCAT bypass: parse-only candidates must not change evaluation.
    for (sql, expected) in [
        ("SELECT concat('a','b') AS v", Some("ab")),
        ("SELECT concat('') AS v", Some("")),
        ("SELECT concat(NULL::text,'b') AS v", None),
    ] {
        let m = query(&mut socket, sql);
        assert_eq!(
            m.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {m:?}"
        );
        assert_eq!(m[0].1, description);
        assert_eq!(m[1].1, row(&[expected]));
        assert_eq!(m[3].1, b"I");
    }
    let zero = query(&mut socket, "SELECT concat('a','b') AS v WHERE false");
    assert_eq!(zero.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    assert_eq!(zero[0].1, description);
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let message = read(socket);
            let end = message.0 == b'Z';
            messages.push(message);
            if end {
                return messages;
            }
        }
    }
    let sql = "SELECT concat($1::text,'b') AS v";
    send(
        &mut socket,
        b'P',
        &[
            b"\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &25u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"S\0");
    send(&mut socket, b'S', &[]);
    let m = ready(&mut socket);
    assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ", "{m:?}");
    assert_eq!(
        m[1].1,
        [&1i16.to_be_bytes()[..], &25u32.to_be_bytes()].concat()
    );
    assert_eq!(m[2].1, description);
    for (format, value, expected) in [
        (0i16, Some("a"), Some("ab")),
        (1, Some("c"), Some("cb")),
        (0, None, None),
        (1, None, None),
        (0, Some("d"), Some("db")),
    ] {
        let mut bind = vec![0, 0, 0, 0, 0, 1];
        match value {
            Some(v) => {
                bind.extend_from_slice(&(v.len() as i32).to_be_bytes());
                bind.extend_from_slice(v.as_bytes());
            }
            None => bind.extend_from_slice(&(-1i32).to_be_bytes()),
        }
        bind.extend_from_slice(&1i16.to_be_bytes());
        bind.extend_from_slice(&format.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'D', b"P\0");
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let m = ready(&mut socket);
        assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{m:?}");
        let mut expected_description = description.clone();
        let n = expected_description.len();
        expected_description[n - 2..].copy_from_slice(&format.to_be_bytes());
        assert_eq!(m[1].1, expected_description);
        assert_eq!(m[2].1, row(&[expected]));
        assert_eq!(m.last().unwrap().1, b"I");
    }
    assert_eq!(funcs.names_for_dialect(SqlDialect::MySQL), before);
    drop(socket);
    service.close();
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_08() {
    use astersql_expression::ast::SqlDialect;
    use astersql_expression::formal_registry::funcs;
    let mysql_names = funcs.names_for_dialect(SqlDialect::MySQL);
    assert!(mysql_names.iter().any(|name| name == "abs"));
    // Defaults remain parse-only private declarations; ordinary ABS still uses
    // the real SQL, extended protocol and error-recovery contract.
    signed_abs_protocol_contract();
    assert_eq!(funcs.names_for_dialect(SqlDialect::MySQL), mysql_names);
}

#[test]
fn pg_function_10() {
    // Keep signed integer arithmetic and declared PG result identity coupled,
    // including int8 values that cannot round-trip through float8.
    signed_abs_protocol_contract();
}

#[test]
fn pg_function_10_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(server.clone());
    let mut socket = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);
    for (source, expected) in [
        ("0", Some("0")),
        ("1", Some("1")),
        ("-1", Some("1")),
        ("9223372036854775807", Some("9223372036854775807")),
        ("-9223372036854775807", Some("9223372036854775807")),
        ("-9007199254740993", Some("9007199254740993")),
        ("NULL", None),
    ] {
        mysql_packet(
            &mut socket,
            0,
            &[
                b"\x03SELECT ABS(CAST(".as_slice(),
                source.as_bytes(),
                b" AS SIGNED)) AS v",
            ]
            .concat(),
        );
        assert_eq!(mysql_read(&mut socket), [1]);
        let column = mysql_read(&mut socket);
        let mut offset = 0;
        for _ in 0..6 {
            let length = usize::from(column[offset]);
            offset += length + 1;
        }
        assert_eq!(column[offset], 12);
        let metadata = &column[offset + 1..offset + 13];
        println!(
            "pg_function_10_mysql metadata: charset={} length={} code={} flags={} decimals={}",
            u16::from_le_bytes(metadata[..2].try_into().unwrap()),
            u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
            metadata[6],
            u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
            metadata[9]
        );
        assert_eq!(metadata[6], 8, "MySQL ABS retains BIGINT wire code");
        assert_eq!(u16::from_le_bytes(metadata[..2].try_into().unwrap()), 63);
        assert_eq!(u32::from_le_bytes(metadata[2..6].try_into().unwrap()), 22);
        assert_eq!(
            u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
            128 | u16::from(expected.is_some())
        );
        assert_eq!(metadata[9], 0);
        assert_eq!(mysql_read(&mut socket)[0], 0xfe);
        let expected = expected.map_or_else(
            || vec![0xfb],
            |text| {
                let mut bytes = vec![text.len() as u8];
                bytes.extend_from_slice(text.as_bytes());
                bytes
            },
        );
        assert_eq!(mysql_read(&mut socket), expected);
        let eof = mysql_read(&mut socket);
        assert_eq!(&eof[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(&mut socket, "SHOW WARNINGS").is_empty());
    }
    mysql_packet(
        &mut socket,
        0,
        b"\x03SELECT ABS(CAST(-9223372036854775808 AS SIGNED)) AS v",
    );
    let error = mysql_read(&mut socket);
    assert_eq!(error[0], 0xff, "signed ABS overflow must fail: {error:?}");
    assert_eq!(u16::from_le_bytes(error[1..3].try_into().unwrap()), 1690);
    assert_eq!(&error[3..9], b"#22003");
    assert!(std::str::from_utf8(&error[9..]).unwrap().contains("BIGINT"));
    assert_eq!(
        mysql_query(&mut socket, "SELECT 1"),
        vec![vec![Some(b"1".to_vec())]]
    );
    mysql_packet(&mut socket, 0, &[1]);
}

#[test]
fn pg_function_11() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }
    // Each expression is also analyzed with zero rows. NULL propagation is
    // asserted in DataRow; RowDescription has no nullable bit.
    for (expression, expected, oid, width) in [
        ("coalesce(NULL::int4,7::int4)", Some("7"), 23, 4),
        ("coalesce(NULL::int4,NULL::int4)", None, 23, 4),
        ("coalesce(NULL,NULL)", None, 25, -1),
        ("coalesce(7::int4,2.50::numeric)", Some("7"), 1700, -1),
        ("coalesce(NULL::int4,2.50::numeric)", Some("2.50"), 1700, -1),
        ("coalesce(NULL::numeric,7::int4)", Some("7"), 1700, -1),
        ("coalesce(NULL::int4,NULL::numeric)", None, 1700, -1),
        (
            "coalesce((-2147483648)::int4,2147483647::int4)",
            Some("-2147483648"),
            23,
            4,
        ),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let m = query(&mut socket, &sql);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"TCZ".as_slice()
                } else {
                    b"TDCZ".as_slice()
                },
                "{sql}: {m:?}"
            );
            assert_eq!(m[0].1, description(oid, width, 0), "{sql}");
            if !empty {
                assert_eq!(m[1].1, row(&[expected]), "{sql}");
            }
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }
    // The same analyzed statement is rebound value/NULL/value in both wire
    // formats. Mixed numeric keeps its identity even when int4 supplies the row.
    for (expression, oid, width) in [
        ("coalesce($1::int4,NULL::int4)", 23u32, 4i16),
        ("coalesce($1::int4,NULL::numeric)", 1700, -1),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            send(
                &mut socket,
                b'P',
                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
            );
            send(&mut socket, b'D', b"S\0");
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tTZ",
                "{sql}: {m:?}"
            );
            assert_eq!(
                m[1].1,
                [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
            );
            assert_eq!(m[2].1, description(oid, width, 0));
            for (format, value) in [
                (0i16, Some(-1i32)),
                (1, None),
                (1, Some(2)),
                (0, None),
                (0, Some(3)),
            ] {
                let mut bind = vec![0, 0];
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                bind.extend_from_slice(&1i16.to_be_bytes());
                if let Some(v) = value {
                    let bytes = if format == 0 {
                        v.to_string().into_bytes()
                    } else {
                        v.to_be_bytes().to_vec()
                    };
                    bind.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    bind.extend_from_slice(&bytes);
                } else {
                    bind.extend_from_slice(&(-1i32).to_be_bytes());
                }
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                send(&mut socket, b'B', &bind);
                send(&mut socket, b'D', b"P\0");
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'D', b"S\0");
                send(&mut socket, b'S', &[]);
                let m = ready(&mut socket);
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    if empty {
                        b"2TCtTZ".as_slice()
                    } else {
                        b"2TDCtTZ".as_slice()
                    },
                    "{sql}: {m:?}"
                );
                assert_eq!(m[1].1, description(oid, width, format));
                if !empty {
                    let bytes = value.map(|v| {
                        if format == 0 {
                            v.to_string().into_bytes()
                        } else if oid == 23 {
                            v.to_be_bytes().to_vec()
                        } else {
                            // PostgreSQL numeric: ndigits, weight, sign, dscale,
                            // followed by base-10000 digits; these inputs are one digit.
                            [
                                1i16.to_be_bytes(),
                                0i16.to_be_bytes(),
                                (if v < 0 { 0x4000i16 } else { 0 }).to_be_bytes(),
                                0i16.to_be_bytes(),
                                (v.abs() as i16).to_be_bytes(),
                            ]
                            .concat()
                        }
                    });
                    let mut expected = 1i16.to_be_bytes().to_vec();
                    if let Some(bytes) = bytes {
                        expected.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                        expected.extend_from_slice(&bytes);
                    } else {
                        expected.extend_from_slice(&(-1i32).to_be_bytes());
                    }
                    assert_eq!(m[2].1, expected);
                }
                let index = if empty { 3 } else { 4 };
                assert_eq!(
                    m[index].1,
                    [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
                );
                assert_eq!(m[index + 1].1, description(oid, width, 0));
                assert_eq!(m.last().unwrap().1, b"I");
            }
        }
    }
    for (sql, state) in [
        ("SELECT coalesce(1::int4,'x'::text) AS v", "42804"),
        (
            "SELECT coalesce(NULL::int4,NULL::text) AS v WHERE false",
            "42804",
        ),
        ("SELECT pg_catalog.coalesce(1::int4,2::int4) AS v", "42883"),
    ] {
        for extended in [false, true] {
            let m = if extended {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                send(&mut socket, b'S', &[]);
                ready(&mut socket)
            } else {
                query(&mut socket, sql)
            };
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"EZ",
                "{sql}: {m:?}"
            );
            let error = &m[0].1;
            let mut offset = 0;
            let mut fields = std::collections::BTreeMap::new();
            while error[offset] != 0 {
                let tag = error[offset];
                offset += 1;
                let n = error[offset..].iter().position(|b| *b == 0).unwrap();
                fields.insert(
                    tag,
                    std::str::from_utf8(&error[offset..offset + n]).unwrap(),
                );
                offset += n + 1;
            }
            assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
            assert_eq!(fields.get(&b'C'), Some(&state));
            assert!(!fields[&b'M'].is_empty());
            assert!([b'D', b'H'].iter().all(|tag| !fields.contains_key(tag)));
            // PG18.6 points at the conflicting second operand. Qualified
            // COALESCE remains the existing name-stage rejection contract.
            let position = if state == "42804" {
                Some(if sql.contains("NULL::text") {
                    "28"
                } else {
                    "25"
                })
            } else {
                None
            };
            assert_eq!(fields.get(&b'P').copied(), position);
            println!("{sql} extended={extended}: {fields:?}");
            assert_eq!(m[1].1, b"I");
            let probe = query(&mut socket, "SELECT 1::int4");
            assert_eq!(probe[1].1, row(&[Some("1")]));
        }
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_11_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server.clone());
    let address = server.listener_addr().unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);
    fn result(socket: &mut TcpStream, expression: &str) -> (Vec<u8>, Vec<u8>) {
        mysql_packet(
            socket,
            0,
            format!("\x03SELECT {expression} AS v").as_bytes(),
        );
        assert_eq!(mysql_read(socket), [1]);
        let column = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let value = mysql_read(socket);
        let eof = mysql_read(socket);
        assert_eq!(&eof[..3], &[0xfe, 0, 0]);
        (column, value)
    }
    // Go formal FieldTypes: signed CAST width22, decimal(8,2) display10,
    // utf8mb4 CHAR(4) display16. A scalar shortcut is not a type authority.
    let expected_fields = [
        (63u16, 22u32, 8u8, 129u16, 0u8),
        (63, 22, 8, 128, 0),
        (63, 10, 246, 129, 2),
        (63, 10, 246, 128, 2),
        (46, 16, 254, 1, 31),
        (46, 16, 254, 0, 31),
        (63, 0, 6, 128, 0),
    ];
    for ((first, second, scalar, value), (charset, length, code, flags, decimals)) in [
        (
            "NULL",
            "CAST(7 AS SIGNED)",
            "CAST(7 AS SIGNED)",
            vec![1, b'7'],
        ),
        (
            "CAST(NULL AS SIGNED)",
            "CAST(NULL AS SIGNED)",
            "CAST(NULL AS SIGNED)",
            vec![0xfb],
        ),
        (
            "NULL",
            "CAST(2.50 AS DECIMAL(8,2))",
            "CAST(2.50 AS DECIMAL(8,2))",
            vec![4, b'2', b'.', b'5', b'0'],
        ),
        (
            "CAST(NULL AS DECIMAL(8,2))",
            "CAST(NULL AS DECIMAL(8,2))",
            "CAST(NULL AS DECIMAL(8,2))",
            vec![0xfb],
        ),
        (
            "NULL",
            "CAST('ab' AS CHAR(4))",
            "CAST('ab' AS CHAR(4))",
            vec![2, b'a', b'b'],
        ),
        (
            "CAST(NULL AS CHAR(4))",
            "CAST(NULL AS CHAR(4))",
            "CAST(NULL AS CHAR(4))",
            vec![0xfb],
        ),
        ("NULL", "NULL", "NULL", vec![0xfb]),
    ]
    .into_iter()
    .zip(expected_fields)
    {
        let baseline = result(&mut socket, scalar);
        let expression = format!("COALESCE({first},{second})");
        let actual = result(&mut socket, &expression);
        let mut expected_column = b"\x03def\0\0\0\x01v\0\x0c".to_vec();
        expected_column.extend_from_slice(&charset.to_le_bytes());
        expected_column.extend_from_slice(&length.to_le_bytes());
        expected_column.push(code);
        expected_column.extend_from_slice(&flags.to_le_bytes());
        expected_column.extend_from_slice(&[decimals, 0, 0]);
        assert_eq!(
            actual.0, expected_column,
            "{expression}: Go formal MySQL metadata"
        );
        assert_eq!(
            actual.1, baseline.1,
            "{expression}: scalar value intent retained"
        );
        let mut offset = 0;
        for _ in 0..6 {
            offset += usize::from(actual.0[offset]) + 1;
        }
        let metadata = &actual.0[offset + 1..offset + 13];
        println!(
            "{expression}: charset={} length={} code={} flags={} decimals={}",
            u16::from_le_bytes(metadata[..2].try_into().unwrap()),
            u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
            metadata[6],
            u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
            metadata[9],
        );
        assert_eq!(actual.1, value, "{expression}");
    }
    mysql_packet(&mut socket, 0, &[1]);
    drop(socket);
    drop(cleanup);
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_12() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}

    // Always roll back and remove our table, including assertion-unwind paths.
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query(&mut self.0, "ROLLBACK");
                query(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    let table = format!("coalesce_runtime_12_{}", address.port());
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    for sql in [
        format!("CREATE TABLE {table}(a integer,z integer)"),
        format!("INSERT INTO {table} VALUES(7,0)"),
    ] {
        let m = query(&mut socket, &sql);
        assert_eq!(
            m.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"CZ",
            "{sql}: {m:?}"
        );
    }
    let description = [
        &1i16.to_be_bytes()[..],
        b"v\0",
        &0u32.to_be_bytes(),
        &0i16.to_be_bytes(),
        &23u32.to_be_bytes(),
        &4i16.to_be_bytes(),
        &(-1i32).to_be_bytes(),
        &0i16.to_be_bytes(),
    ]
    .concat();
    let sql = format!("SELECT coalesce(a,10/z) AS v FROM {table}");
    let m = query(&mut socket, &sql);
    assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ", "{m:?}");
    assert_eq!(m[0].1, description);
    assert_eq!(m[1].1, row(&[Some("7")]));
    assert_eq!(m.last().unwrap().1, b"I");
    // Both operands are typed before evaluation even when the first is non-NULL.
    let mixed = query(
        &mut socket,
        &format!("SELECT coalesce(a,'x'::text) AS v FROM {table}"),
    );
    assert_error(&mixed, "42804", b'I');
    assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));
    let mixed_sql = format!("SELECT coalesce(a,'x'::text) AS v FROM {table}");
    send(
        &mut socket,
        b'P',
        &[
            b"\0".as_slice(),
            mixed_sql.as_bytes(),
            b"\0",
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', &[]);
    assert_error(&ready(&mut socket), "42804", b'I');
    assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));
    query(&mut socket, &format!("UPDATE {table} SET a=NULL"));
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let m = query(&mut socket, &sql);
    assert_error(&m, "22012", b'E');
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));

    fn assert_error(m: &[(u8, Vec<u8>)], state: &str, ready: u8) {
        assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ", "{m:?}");
        let error = &m[0].1;
        let mut offset = 0;
        let mut fields = std::collections::BTreeMap::new();
        while error[offset] != 0 {
            let tag = error[offset];
            offset += 1;
            let n = error[offset..].iter().position(|b| *b == 0).unwrap();
            fields.insert(
                tag,
                std::str::from_utf8(&error[offset..offset + n]).unwrap(),
            );
            offset += n + 1;
        }
        assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
        assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
        assert_eq!(fields.get(&b'C'), Some(&state));
        assert!(!fields[&b'M'].is_empty());
        assert!([b'D', b'H'].iter().all(|tag| !fields.contains_key(tag)));
        // Analysis identifies the incompatible text operand; runtime division
        // has no source cursor in the independent PG18.6 contract.
        assert_eq!(
            fields.get(&b'P').copied(),
            if state == "42804" { Some("19") } else { None }
        );
        println!("runtime COALESCE: {fields:?}");
        assert_eq!(m.last().unwrap().1, [ready]);
    }

    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    // Parse/Describe cannot evaluate the fallback. Rebind the same statement
    // against real columns, including NULL input and an empty relation result.
    for empty in [false, true] {
        let statement = if empty {
            b"runtime12_empty\0".as_slice()
        } else {
            b"runtime12_full\0".as_slice()
        };
        let statement_describe = [b"S".as_slice(), statement].concat();
        let sql = format!(
            "SELECT coalesce(a,$1::int4/z) AS v FROM {table}{}",
            if empty { " WHERE false" } else { "" }
        );
        send(
            &mut socket,
            b'P',
            &[statement, sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
        );
        send(&mut socket, b'D', &statement_describe);
        send(&mut socket, b'S', &[]);
        let m = ready(&mut socket);
        assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ", "{m:?}");
        assert_eq!(
            m[1].1,
            [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
        );
        assert_eq!(m[2].1, description);
        for (a, z, format, input, expected) in [
            ("7", 0, 0i16, Some(10i32), Some(7i32)),
            ("7", 0, 1, None, Some(7)),
            ("NULL", 2, 0, Some(10), Some(5)),
            ("NULL", 2, 1, None, None),
            ("NULL", 2, 1, Some(-11), Some(-5)),
            ("NULL", 2, 0, None, None),
        ] {
            let update = query(&mut socket, &format!("UPDATE {table} SET a={a},z={z}"));
            assert_eq!(
                update.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"CZ",
                "{update:?}"
            );
            let mut bind = [b"\0".as_slice(), statement].concat();
            bind.extend_from_slice(&1i16.to_be_bytes());
            bind.extend_from_slice(&format.to_be_bytes());
            bind.extend_from_slice(&1i16.to_be_bytes());
            if let Some(value) = input {
                let bytes = if format == 0 {
                    value.to_string().into_bytes()
                } else {
                    value.to_be_bytes().to_vec()
                };
                bind.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                bind.extend_from_slice(&bytes);
            } else {
                bind.extend_from_slice(&(-1i32).to_be_bytes());
            }
            bind.extend_from_slice(&1i16.to_be_bytes());
            bind.extend_from_slice(&format.to_be_bytes());
            send(&mut socket, b'B', &bind);
            send(&mut socket, b'D', b"P\0");
            send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
            send(&mut socket, b'D', &statement_describe);
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"2TCtTZ".as_slice()
                } else {
                    b"2TDCtTZ".as_slice()
                },
                "{m:?}"
            );
            let mut portal = description.clone();
            let len = portal.len();
            portal[len - 2..].copy_from_slice(&format.to_be_bytes());
            assert_eq!(m[1].1, portal);
            if !empty {
                let mut expected_row = 1i16.to_be_bytes().to_vec();
                if let Some(value) = expected {
                    let bytes = if format == 0 {
                        value.to_string().into_bytes()
                    } else {
                        value.to_be_bytes().to_vec()
                    };
                    expected_row.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    expected_row.extend_from_slice(&bytes);
                } else {
                    expected_row.extend_from_slice(&(-1i32).to_be_bytes());
                }
                assert_eq!(m[2].1, expected_row);
            }
            let index = if empty { 3 } else { 4 };
            assert_eq!(m[index + 1].1, description);
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }
    // A previously described named statement reaches the fallback only at Execute.
    query(&mut socket, &format!("UPDATE {table} SET a=NULL,z=0"));
    query(&mut socket, "BEGIN");
    let bind = [
        b"\0runtime12_full\0".as_slice(),
        &0i16.to_be_bytes(),
        &1i16.to_be_bytes(),
        &2i32.to_be_bytes(),
        b"10",
        &0i16.to_be_bytes(),
    ]
    .concat();
    send(&mut socket, b'B', &bind);
    send(&mut socket, b'D', b"P\0");
    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
    send(&mut socket, b'S', &[]);
    let m = ready(&mut socket);
    assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TEZ", "{m:?}");
    assert_eq!(m[1].1, description);
    assert_error(&m[2..], "22012", b'E');
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));
    drop(table_cleanup);
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_12_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server.clone());
    let address = server.listener_addr().unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);

    fn execute(socket: &mut TcpStream, sql: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, u16) {
        mysql_packet(socket, 0, format!("\x03{sql}").as_bytes());
        let first = mysql_read(socket);
        assert_ne!(first[0], 0xff, "{sql}: {first:?}");
        if first[0] == 0 {
            return (vec![], vec![], 0);
        }
        let columns = (0..first[0])
            .map(|_| mysql_read(socket))
            .collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut rows = vec![];
        loop {
            let packet = mysql_read(socket);
            if packet[0] == 0xfe && packet.len() < 9 {
                return (
                    columns,
                    rows,
                    u16::from_le_bytes(packet[1..3].try_into().unwrap()),
                );
            }
            rows.push(packet);
        }
    }
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    execute(
        &mut socket,
        "SET SESSION sql_mode='STRICT_TRANS_TABLES,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION'",
    );
    let table = format!("coalesce_mysql_runtime12_{}", address.port());
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    execute(
        &mut socket,
        &format!("CREATE TABLE {table}(a BIGINT,z BIGINT)"),
    );
    execute(&mut socket, &format!("INSERT INTO {table} VALUES(7,0)"));
    for (a, z) in [("7", 0), ("NULL", 0), ("NULL", 2)] {
        execute(&mut socket, &format!("UPDATE {table} SET a={a},z={z}"));
        let actual = execute(
            &mut socket,
            &format!("SELECT COALESCE(a,10/z) AS v FROM {table}"),
        );
        let warnings = execute(&mut socket, "SHOW WARNINGS");
        let mut offset = 0;
        for _ in 0..6 {
            offset += usize::from(actual.0[0][offset]) + 1;
        }
        let metadata = &actual.0[0][offset + 1..offset + 13];
        println!(
            "MySQL a={a} z={z}: rows={:?} warnings={} details={:?} charset={} length={} code={} flags={} decimals={}",
            actual.1,
            actual.2,
            warnings.1,
            u16::from_le_bytes(metadata[..2].try_into().unwrap()),
            u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
            metadata[6],
            u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
            metadata[9]
        );
        assert_eq!(actual.1.len(), 1);
        // Go casts to the DECIMAL family while retaining the selected arm scale.
        // The selected non-NULL arm still skips division and its warning.
        assert_eq!(actual.2, u16::from(a == "NULL" && z == 0));
        assert_eq!(warnings.1.len(), usize::from(a == "NULL" && z == 0));
        if a == "7" {
            assert_eq!(actual.1[0], vec![1, b'7']);
        } else if z == 0 {
            assert_eq!(actual.1[0], vec![0xfb]);
        } else {
            assert_eq!(actual.1[0], vec![6, b'5', b'.', b'0', b'0', b'0', b'0']);
        }
    }
    drop(table_cleanup);
    mysql_packet(&mut socket, 0, &[1]);
    drop(socket);
    drop(cleanup);
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_13() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }
    // Each expression is also analyzed with zero rows. NULL behavior is
    // asserted in DataRow; RowDescription has no nullable bit.
    for (expression, expected, oid, width) in [
        ("nullif(1::int4,1::int4)", None, 23, 4),
        ("nullif(1::int4,2::int4)", Some("1"), 23, 4),
        ("nullif(NULL::int4,2::int4)", None, 23, 4),
        ("nullif(1::int4,NULL::int4)", Some("1"), 23, 4),
        ("nullif(NULL::int4,NULL::int4)", None, 23, 4),
        ("nullif(1::int4,2.2::numeric)", Some("1"), 1700, -1),
        ("nullif(1::int4,1.00::numeric)", None, 1700, -1),
        (
            "nullif((-2147483648)::int4,2147483647::int4)",
            Some("-2147483648"),
            23,
            4,
        ),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let m = query(&mut socket, &sql);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"TCZ".as_slice()
                } else {
                    b"TDCZ".as_slice()
                },
                "{sql}: {m:?}"
            );
            assert_eq!(m[0].1, description(oid, width, 0), "{sql}");
            if !empty {
                assert_eq!(m[1].1, row(&[expected]), "{sql}");
            }
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }
    // The same analyzed statement is rebound value/NULL/value in both wire
    // formats. The numeric equality operand keeps its promoted identity.
    for (expression, oid, width) in [
        ("nullif($1::int4,2::int4)", 23u32, 4i16),
        ("nullif($1::int4,2.2::numeric)", 1700, -1),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            send(
                &mut socket,
                b'P',
                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
            );
            send(&mut socket, b'D', b"S\0");
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tTZ",
                "{sql}: {m:?}"
            );
            assert_eq!(
                m[1].1,
                [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
            );
            assert_eq!(m[2].1, description(oid, width, 0));
            for (format, value) in [
                (0i16, Some(-1i32)),
                (1, None),
                (1, Some(2)),
                (0, None),
                (0, Some(3)),
            ] {
                let mut bind = vec![0, 0];
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                bind.extend_from_slice(&1i16.to_be_bytes());
                if let Some(v) = value {
                    let bytes = if format == 0 {
                        v.to_string().into_bytes()
                    } else {
                        v.to_be_bytes().to_vec()
                    };
                    bind.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    bind.extend_from_slice(&bytes);
                } else {
                    bind.extend_from_slice(&(-1i32).to_be_bytes());
                }
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                send(&mut socket, b'B', &bind);
                send(&mut socket, b'D', b"P\0");
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'D', b"S\0");
                send(&mut socket, b'S', &[]);
                let m = ready(&mut socket);
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    if empty {
                        b"2TCtTZ".as_slice()
                    } else {
                        b"2TDCtTZ".as_slice()
                    },
                    "{sql}: {m:?}"
                );
                assert_eq!(m[1].1, description(oid, width, format));
                if !empty {
                    let bytes = value.filter(|v| oid != 23 || *v != 2).map(|v| {
                        if format == 0 {
                            v.to_string().into_bytes()
                        } else if oid == 23 {
                            v.to_be_bytes().to_vec()
                        } else {
                            // PostgreSQL numeric: ndigits, weight, sign, dscale,
                            // followed by base-10000 digits; these inputs are one digit.
                            [
                                1i16.to_be_bytes(),
                                0i16.to_be_bytes(),
                                (if v < 0 { 0x4000i16 } else { 0 }).to_be_bytes(),
                                0i16.to_be_bytes(),
                                (v.abs() as i16).to_be_bytes(),
                            ]
                            .concat()
                        }
                    });
                    let mut expected = 1i16.to_be_bytes().to_vec();
                    if let Some(bytes) = bytes {
                        expected.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                        expected.extend_from_slice(&bytes);
                    } else {
                        expected.extend_from_slice(&(-1i32).to_be_bytes());
                    }
                    assert_eq!(m[2].1, expected);
                }
                let index = if empty { 3 } else { 4 };
                assert_eq!(
                    m[index].1,
                    [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
                );
                assert_eq!(m[index + 1].1, description(oid, width, 0));
                assert_eq!(m.last().unwrap().1, b"I");
            }
        }
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_13_runtime() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}

    // Always roll back and remove our table, including assertion-unwind paths.
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query(&mut self.0, "ROLLBACK");
                query(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    let table = format!("nullif_runtime_13_{}", address.port());
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    for sql in [
        format!("CREATE TABLE {table}(a integer,z integer)"),
        format!("INSERT INTO {table} VALUES(7,2)"),
    ] {
        let m = query(&mut socket, &sql);
        assert_eq!(
            m.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"CZ",
            "{sql}: {m:?}"
        );
    }
    let description = [
        &1i16.to_be_bytes()[..],
        b"v\0",
        &0u32.to_be_bytes(),
        &0i16.to_be_bytes(),
        &23u32.to_be_bytes(),
        &4i16.to_be_bytes(),
        &(-1i32).to_be_bytes(),
        &0i16.to_be_bytes(),
    ]
    .concat();
    let sql = format!("SELECT nullif(a,10/z) AS v FROM {table}");
    let m = query(&mut socket, &sql);
    assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ", "{m:?}");
    assert_eq!(m[0].1, description);
    assert_eq!(m[1].1, row(&[Some("7")]));
    assert_eq!(m.last().unwrap().1, b"I");
    query(&mut socket, &format!("UPDATE {table} SET z=0"));
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    assert_error(&query(&mut socket, &sql), "22012", b'E');
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));

    fn assert_error(m: &[(u8, Vec<u8>)], state: &str, ready: u8) {
        assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ", "{m:?}");
        let error = &m[0].1;
        let mut offset = 0;
        let mut fields = std::collections::BTreeMap::new();
        while error[offset] != 0 {
            let tag = error[offset];
            offset += 1;
            let n = error[offset..].iter().position(|b| *b == 0).unwrap();
            fields.insert(
                tag,
                std::str::from_utf8(&error[offset..offset + n]).unwrap(),
            );
            offset += n + 1;
        }
        assert_eq!(fields.get(&b'S'), Some(&"ERROR"));
        assert_eq!(fields.get(&b'V'), Some(&"ERROR"));
        assert_eq!(fields.get(&b'C'), Some(&state));
        assert!(!fields[&b'M'].is_empty());
        assert!(
            [b'D', b'H', b'P']
                .iter()
                .all(|tag| !fields.contains_key(tag))
        );
        println!("runtime NULLIF: {fields:?}");
        assert_eq!(m.last().unwrap().1, [ready]);
    }

    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    // Parse/Describe cannot evaluate the comparison. Rebind the same statement
    // against real columns, including NULL input and an empty relation result.
    for empty in [false, true] {
        let statement = if empty {
            b"runtime13_empty\0".as_slice()
        } else {
            b"runtime13_full\0".as_slice()
        };
        let statement_describe = [b"S".as_slice(), statement].concat();
        let sql = format!(
            "SELECT nullif(a,$1::int4/z) AS v FROM {table}{}",
            if empty { " WHERE false" } else { "" }
        );
        send(
            &mut socket,
            b'P',
            &[statement, sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
        );
        send(&mut socket, b'D', &statement_describe);
        send(&mut socket, b'S', &[]);
        let m = ready(&mut socket);
        assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ", "{m:?}");
        assert_eq!(
            m[1].1,
            [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
        );
        assert_eq!(m[2].1, description);
        for (a, z, format, input, expected) in [
            ("7", 2, 0i16, Some(10i32), Some(7i32)),
            ("7", 0, 1, None, Some(7)),
            ("5", 2, 0, Some(10), None),
            ("NULL", 2, 1, None, None),
            ("NULL", 2, 1, Some(-11), None),
            ("NULL", 2, 0, None, None),
        ] {
            let update = query(&mut socket, &format!("UPDATE {table} SET a={a},z={z}"));
            assert_eq!(
                update.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"CZ",
                "{update:?}"
            );
            let mut bind = [b"\0".as_slice(), statement].concat();
            bind.extend_from_slice(&1i16.to_be_bytes());
            bind.extend_from_slice(&format.to_be_bytes());
            bind.extend_from_slice(&1i16.to_be_bytes());
            if let Some(value) = input {
                let bytes = if format == 0 {
                    value.to_string().into_bytes()
                } else {
                    value.to_be_bytes().to_vec()
                };
                bind.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                bind.extend_from_slice(&bytes);
            } else {
                bind.extend_from_slice(&(-1i32).to_be_bytes());
            }
            bind.extend_from_slice(&1i16.to_be_bytes());
            bind.extend_from_slice(&format.to_be_bytes());
            send(&mut socket, b'B', &bind);
            send(&mut socket, b'D', b"P\0");
            send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
            send(&mut socket, b'D', &statement_describe);
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"2TCtTZ".as_slice()
                } else {
                    b"2TDCtTZ".as_slice()
                },
                "{m:?}"
            );
            let mut portal = description.clone();
            let len = portal.len();
            portal[len - 2..].copy_from_slice(&format.to_be_bytes());
            assert_eq!(m[1].1, portal);
            if !empty {
                let mut expected_row = 1i16.to_be_bytes().to_vec();
                if let Some(value) = expected {
                    let bytes = if format == 0 {
                        value.to_string().into_bytes()
                    } else {
                        value.to_be_bytes().to_vec()
                    };
                    expected_row.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    expected_row.extend_from_slice(&bytes);
                } else {
                    expected_row.extend_from_slice(&(-1i32).to_be_bytes());
                }
                assert_eq!(m[2].1, expected_row);
            }
            let index = if empty { 3 } else { 4 };
            assert_eq!(m[index + 1].1, description);
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }
    // Both comparison operands are evaluated at Execute, even with non-NULL a.
    query(&mut socket, &format!("UPDATE {table} SET a=7,z=0"));
    query(&mut socket, "BEGIN");
    let bind = [
        b"\0runtime13_full\0".as_slice(),
        &0i16.to_be_bytes(),
        &1i16.to_be_bytes(),
        &2i32.to_be_bytes(),
        b"10",
        &0i16.to_be_bytes(),
    ]
    .concat();
    send(&mut socket, b'B', &bind);
    send(&mut socket, b'D', b"P\0");
    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
    send(&mut socket, b'S', &[]);
    let m = ready(&mut socket);
    assert_eq!(m.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TEZ", "{m:?}");
    assert_eq!(m[1].1, description);
    assert_error(&m[2..], "22012", b'E');
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(query(&mut socket, "SELECT 1::int4")[1].1, row(&[Some("1")]));
    drop(table_cleanup);
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_13_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server.clone());
    let address = server.listener_addr().unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);

    fn execute(socket: &mut TcpStream, sql: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, u16) {
        mysql_packet(socket, 0, format!("\x03{sql}").as_bytes());
        let first = mysql_read(socket);
        assert_ne!(first[0], 0xff, "{sql}: {first:?}");
        if first[0] == 0 {
            return (vec![], vec![], 0);
        }
        let columns = (0..first[0])
            .map(|_| mysql_read(socket))
            .collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut rows = vec![];
        loop {
            let packet = mysql_read(socket);
            if packet[0] == 0xfe && packet.len() < 9 {
                return (
                    columns,
                    rows,
                    u16::from_le_bytes(packet[1..3].try_into().unwrap()),
                );
            }
            rows.push(packet);
        }
    }
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    execute(
        &mut socket,
        "SET SESSION sql_mode='STRICT_TRANS_TABLES,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION'",
    );
    let table = format!("nullif_mysql_runtime13_{}", address.port());
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    execute(
        &mut socket,
        &format!("CREATE TABLE {table}(a BIGINT,z BIGINT)"),
    );
    execute(&mut socket, &format!("INSERT INTO {table} VALUES(7,0)"));
    for (a, z) in [("7", 0), ("NULL", 0), ("NULL", 2), ("5", 2), ("7", 2)] {
        execute(&mut socket, &format!("UPDATE {table} SET a={a},z={z}"));
        let actual = execute(
            &mut socket,
            &format!("SELECT NULLIF(a,10/z) AS v FROM {table}"),
        );
        let warnings = execute(&mut socket, "SHOW WARNINGS");
        let mut offset = 0;
        for _ in 0..6 {
            offset += usize::from(actual.0[0][offset]) + 1;
        }
        let metadata = &actual.0[0][offset + 1..offset + 13];
        println!(
            "MySQL a={a} z={z}: rows={:?} warnings={} details={:?} charset={} length={} code={} flags={} decimals={}",
            actual.1,
            actual.2,
            warnings.1,
            u16::from_le_bytes(metadata[..2].try_into().unwrap()),
            u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
            metadata[6],
            u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
            metadata[9]
        );
        assert_eq!(actual.1.len(), 1);
        // Go NULLIF rewrites to IF(eq(a,10/z),NULL,a). CompareDecimal evaluates
        // both operands even for NULL a; retain that Go behavior, independently
        // of MySQL8's first-NULL shortcut recorded by the strict differential.
        assert_eq!(actual.2, u16::from(z == 0));
        assert_eq!(
            warnings.1,
            if z == 0 {
                vec![b"\x07Warning\x041365\x0dDivision by 0".to_vec()]
            } else {
                vec![]
            }
        );
        assert_eq!(u16::from_le_bytes(metadata[..2].try_into().unwrap()), 63);
        assert_eq!(u32::from_le_bytes(metadata[2..6].try_into().unwrap()), 20);
        assert_eq!(metadata[6], 8);
        assert_eq!(u16::from_le_bytes(metadata[7..9].try_into().unwrap()), 128);
        assert_eq!(metadata[9], 0);
        if a == "7" {
            assert_eq!(actual.1[0], vec![1, b'7']);
        } else {
            assert_eq!(actual.1[0], vec![0xfb]);
        }
    }
    drop(table_cleanup);
    mysql_packet(&mut socket, 0, &[1]);
    drop(socket);
    drop(cleanup);
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_14() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }

    for (expression, expected) in [
        (r#"coalesce(NULL::text,'A'::text COLLATE "C")"#, Some("A")),
        (r#"nullif('A'::text COLLATE "C",'a'::text)"#, Some("A")),
        (r#"nullif('A'::text COLLATE "C",'A'::text)"#, None),
        (r#"nullif('01'::text COLLATE "C",'1'::text)"#, Some("01")),
        (
            r#"coalesce(NULL::text,'A'::text COLLATE "POSIX")"#,
            Some("A"),
        ),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let m = query(&mut socket, &sql);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"TCZ".as_slice()
                } else {
                    b"TDCZ".as_slice()
                },
                "{sql}: {m:?}"
            );
            assert_eq!(m[0].1, description(25, -1, 0), "{sql}");
            if !empty {
                assert_eq!(m[1].1, row(&[expected]), "{sql}");
            }
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }

    for expression in [
        r#"coalesce($1::text,'A'::text COLLATE "C")"#,
        r#"nullif($1::text COLLATE "C",'A'::text)"#,
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            send(
                &mut socket,
                b'P',
                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
            );
            send(&mut socket, b'D', b"S\0");
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tTZ",
                "{sql}: {m:?}"
            );
            assert_eq!(
                m[1].1,
                [&1i16.to_be_bytes()[..], &25u32.to_be_bytes()].concat()
            );
            assert_eq!(m[2].1, description(25, -1, 0));
            for (format, value) in [
                (0i16, Some("a")),
                (1, None),
                (1, Some("A")),
                (0, None),
                (0, Some("a")),
            ] {
                let mut bind = vec![0, 0];
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                bind.extend_from_slice(&1i16.to_be_bytes());
                if let Some(value) = value {
                    bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
                    bind.extend_from_slice(value.as_bytes());
                } else {
                    bind.extend_from_slice(&(-1i32).to_be_bytes());
                }
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                send(&mut socket, b'B', &bind);
                send(&mut socket, b'D', b"P\0");
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'S', &[]);
                let m = ready(&mut socket);
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    if empty {
                        b"2TCZ".as_slice()
                    } else {
                        b"2TDCZ".as_slice()
                    },
                    "{sql}: {m:?}"
                );
                assert_eq!(m[1].1, description(25, -1, format));
                let expected = if expression.starts_with("coalesce") {
                    Some(value.unwrap_or("A"))
                } else {
                    value.filter(|v| *v != "A")
                };
                if !empty {
                    assert_eq!(m[2].1, row(&[expected]), "{sql}");
                }
                assert_eq!(m.last().unwrap().1, b"I");
            }
        }
    }
    for expression in [
        r#"coalesce('A'::text COLLATE "C",'b'::text COLLATE "POSIX")"#,
        r#"coalesce(NULL::text COLLATE "C",'b'::text COLLATE "POSIX")"#,
        r#"nullif('A'::text COLLATE "C",'a'::text COLLATE "POSIX")"#,
    ] {
        for parse in [false, true] {
            for in_transaction in [false, true] {
                if in_transaction {
                    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                }
                let sql = format!("SELECT {expression} AS v WHERE false");
                let m = if parse {
                    send(
                        &mut socket,
                        b'P',
                        &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                    );
                    send(&mut socket, b'S', &[]);
                    ready(&mut socket)
                } else {
                    query(&mut socket, &sql)
                };
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"EZ",
                    "{sql}: {m:?}"
                );
                let e = &m[0].1;
                assert!(e.windows(7).any(|s| s == b"C42P21\0"), "{e:?}");
                assert!(e.windows(7).any(|s| s == b"SERROR\0"));
                assert!(e.windows(7).any(|s| s == b"VERROR\0"));
                let fields = e
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                for tag in b"DH" {
                    assert!(!fields.iter().any(|f| f[0] == *tag));
                }
                let expected_position = format!("P{}", sql.rfind("COLLATE").unwrap() + 1);
                assert_eq!(
                    fields.iter().find(|f| f[0] == b'P').copied(),
                    Some(expected_position.as_bytes())
                );
                assert_eq!(
                    m.last().unwrap().1,
                    if in_transaction { b"E" } else { b"I" }
                );
                if in_transaction {
                    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                }
                let probe = query(&mut socket, "SELECT 1::int4");
                assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                assert_eq!(probe[1].1, row(&[Some("1")]));
            }
        }
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_14_identity() {
    use astersql_parser_ast::SqlDialect;
    use astersql_session::runtime::ConcreteSession;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for name in ["C", "POSIX"] {
        for empty in [false, true] {
            for expr in [
                format!("coalesce(NULL::text,'A'::text COLLATE \"{name}\")"),
                format!("nullif('A'::text,'a'::text COLLATE \"{name}\")"),
            ] {
                let sql = format!(
                    "SELECT {expr} AS v{}",
                    if empty { " WHERE false" } else { "" }
                );
                let fields = pg.describe_result_fields(&sql).unwrap();
                assert_eq!(fields[0].column.FieldType.GetCollate(), name, "{sql}");
            }
            // The session API consumes engine markers; the TCP test above
            // independently verifies the public $1 spelling and translation.
            for expr in [
                format!("coalesce(?::text,'A'::text COLLATE \"{name}\")"),
                format!("nullif(?::text,'a'::text COLLATE \"{name}\")"),
            ] {
                let sql = format!(
                    "SELECT {expr} AS v{}",
                    if empty { " WHERE false" } else { "" }
                );
                let (id, count, fields) = pg
                    .prepare_protocol_statement_with_parameter_types(&sql, &[25])
                    .unwrap();
                assert_eq!(count, 1);
                assert_eq!(fields[0].column.FieldType.GetCollate(), name, "{sql}");
                pg.close_protocol_statement(id).unwrap();
            }
        }
    }
    // Casting to a noncollatable result drops the text identity. Explicit
    // input collations must not create a spurious integer-result conflict.
    let sql =
        r#"SELECT coalesce(('1'::text COLLATE "C")::int4,('2'::text COLLATE "POSIX")::int4) AS v"#;
    let fields = pg.describe_result_fields(sql).unwrap();
    assert_eq!(
        fields[0].column.FieldType.GetType(),
        astersql_parser_mysql::r#type::TypeLong
    );
    assert_eq!(
        pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
        vec!["1"]
    );
}

#[test]
fn pg_function_14_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server.clone());
    let address = server.listener_addr().unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);

    fn execute(socket: &mut TcpStream, sql: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, u16) {
        mysql_packet(socket, 0, format!("\x03{sql}").as_bytes());
        let first = mysql_read(socket);
        assert_ne!(first[0], 0xff, "{sql}: {first:?}");
        if first[0] == 0 {
            return (vec![], vec![], 0);
        }
        let columns = (0..first[0])
            .map(|_| mysql_read(socket))
            .collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut rows = vec![];
        loop {
            let packet = mysql_read(socket);
            if packet[0] == 0xfe && packet.len() < 9 {
                return (
                    columns,
                    rows,
                    u16::from_le_bytes(packet[1..3].try_into().unwrap()),
                );
            }
            rows.push(packet);
        }
    }

    // Explicit binary collation is distinct from a bare scalar's legacy
    // descriptor. NULLIF's constant NULL branch follows Go's IF fold type.
    let expected_fields = [
        (46u16, 4u32, 253u8, 1u16, 31u8),
        (46, 4, 253, 0, 31),
        (46, 0, 6, 0, 31),
        (63, 0, 6, 128, 0),
        (46, 4, 253, 1, 31),
    ];
    for ((expr, expected, baseline), (charset, length, code, flags, decimals)) in [
        ("coalesce(NULL,'A' COLLATE utf8mb4_bin)", Some("A"), "'A'"),
        ("nullif('A' COLLATE utf8mb4_bin,'a')", Some("A"), "'A'"),
        ("nullif('A' COLLATE utf8mb4_bin,'A')", None, "NULL"),
        ("nullif(NULL,'A' COLLATE utf8mb4_bin)", None, "NULL"),
        ("coalesce('a' COLLATE utf8mb4_bin,'A')", Some("a"), "'a'"),
    ]
    .into_iter()
    .zip(expected_fields)
    {
        let actual = execute(&mut socket, &format!("SELECT {expr} AS v"));
        let baseline = execute(&mut socket, &format!("SELECT {baseline} AS v"));
        let mut expected_column = b"\x03def\0\0\0\x01v\0\x0c".to_vec();
        expected_column.extend_from_slice(&charset.to_le_bytes());
        expected_column.extend_from_slice(&length.to_le_bytes());
        expected_column.push(code);
        expected_column.extend_from_slice(&flags.to_le_bytes());
        expected_column.extend_from_slice(&[decimals, 0, 0]);
        assert_eq!(actual.0, vec![expected_column], "Go formal metadata {expr}");
        assert_eq!(actual.1, baseline.1, "scalar value intent {expr}");
        assert_eq!(
            actual.1,
            vec![
                expected
                    .map(|v| [vec![v.len() as u8], v.as_bytes().to_vec()].concat())
                    .unwrap_or(vec![0xfb])
            ],
            "{expr}"
        );
        assert_eq!(actual.2, 0);
        let mut offset = 0;
        for _ in 0..6 {
            offset += usize::from(actual.0[0][offset]) + 1;
        }
        let metadata = &actual.0[0][offset + 1..offset + 13];
        assert_eq!(
            u16::from_le_bytes(metadata[..2].try_into().unwrap()),
            charset
        );
        assert_eq!(
            u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
            length
        );
        assert_eq!(metadata[6], code);
        assert_eq!(
            u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
            flags
        );
        assert_eq!(metadata[9], decimals);
    }
    mysql_packet(&mut socket, 0, &[1]);
    drop(socket);
    drop(cleanup);
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_15() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }

    for (expression, expected) in [
        ("concat('a',NULL,'b')", Some("ab")),
        ("concat(NULL,NULL)", Some("")),
        ("concat('v=',2::int4)", Some("v=2")),
        (r#"concat('Z'::text COLLATE "C",'a'::text)"#, Some("Za")),
        ("concat('x')", Some("x")),
        ("upper(concat('a',NULL,'b'))", Some("AB")),
        ("concat('a',concat(NULL,'b'))", Some("ab")),
        ("pg_catalog.concat('a',NULL,'b')", Some("ab")),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let m = query(&mut socket, &sql);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"TCZ".as_slice()
                } else {
                    b"TDCZ".as_slice()
                },
                "{sql}: {m:?}"
            );
            assert_eq!(m[0].1, description(25, -1, 0), "{sql}");
            if !empty {
                assert_eq!(m[1].1, row(&[expected]), "{sql}");
            }
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }

    for expression in ["concat($1::text,'b')", "pg_catalog.concat($1::text,'b')"] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            send(
                &mut socket,
                b'P',
                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
            );
            send(&mut socket, b'D', b"S\0");
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tTZ",
                "{sql}: {m:?}"
            );
            assert_eq!(
                m[1].1,
                [&1i16.to_be_bytes()[..], &25u32.to_be_bytes()].concat()
            );
            assert_eq!(m[2].1, description(25, -1, 0));
            for (format, value) in [
                (0i16, Some("a")),
                (1, None),
                (1, Some("A")),
                (0, None),
                (0, Some("a")),
            ] {
                let mut bind = vec![0, 0];
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                bind.extend_from_slice(&1i16.to_be_bytes());
                if let Some(value) = value {
                    bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
                    bind.extend_from_slice(value.as_bytes());
                } else {
                    bind.extend_from_slice(&(-1i32).to_be_bytes());
                }
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                send(&mut socket, b'B', &bind);
                send(&mut socket, b'D', b"P\0");
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'S', &[]);
                let m = ready(&mut socket);
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    if empty {
                        b"2TCZ".as_slice()
                    } else {
                        b"2TDCZ".as_slice()
                    },
                    "{sql}: {m:?}"
                );
                assert_eq!(m[1].1, description(25, -1, format));
                let joined = format!("{}b", value.unwrap_or(""));
                let expected = Some(joined.as_str());
                if !empty {
                    assert_eq!(m[2].1, row(&[expected]), "{sql}");
                }
                assert_eq!(m.last().unwrap().1, b"I");
            }
        }
    }
    for expression in ["concat()", "pg_catalog.concat()"] {
        for parse in [false, true] {
            for in_transaction in [false, true] {
                if in_transaction {
                    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                }
                let sql = format!("SELECT {expression} AS v WHERE false");
                let m = if parse {
                    send(
                        &mut socket,
                        b'P',
                        &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                    );
                    send(&mut socket, b'S', &[]);
                    ready(&mut socket)
                } else {
                    query(&mut socket, &sql)
                };
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"EZ",
                    "{sql}: {m:?}"
                );
                let e = &m[0].1;
                assert!(e.windows(7).any(|s| s == b"C42883\0"), "{e:?}");
                assert!(e.windows(7).any(|s| s == b"SERROR\0"));
                assert!(e.windows(7).any(|s| s == b"VERROR\0"));
                let fields = e
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                assert!(!fields.iter().any(|f| f[0] == b'D'));
                assert!(fields.iter().any(|f| f[0] == b'H' && f.len() > 1));
                assert!(fields.iter().any(|f| *f == b"P8"));
                assert_eq!(
                    m.last().unwrap().1,
                    if in_transaction { b"E" } else { b"I" }
                );
                if in_transaction {
                    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                }
                let probe = query(&mut socket, "SELECT 1::int4");
                assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                assert_eq!(probe[1].1, row(&[Some("1")]));
            }
        }
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_15_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server.clone());
    let address = server.listener_addr().unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);

    fn execute(socket: &mut TcpStream, sql: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, u16) {
        mysql_packet(socket, 0, format!("\x03{sql}").as_bytes());
        let first = mysql_read(socket);
        assert_ne!(first[0], 0xff, "{sql}: {first:?}");
        if first[0] == 0 {
            return (vec![], vec![], 0);
        }
        let columns = (0..first[0])
            .map(|_| mysql_read(socket))
            .collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut rows = vec![];
        loop {
            let packet = mysql_read(socket);
            if packet[0] == 0xfe && packet.len() < 9 {
                return (
                    columns,
                    rows,
                    u16::from_le_bytes(packet[1..3].try_into().unwrap()),
                );
            }
            rows.push(packet);
        }
    }

    for (expr, expected, baseline) in [
        ("concat('a',NULL,'b')", None, "NULL"),
        ("concat(NULL,NULL)", None, "NULL"),
        ("concat('v=',2)", Some("v=2"), "'v=2'"),
        ("concat('Z','a')", Some("Za"), "'Za'"),
        ("concat('x')", Some("x"), "'x'"),
        ("concat('','')", Some(""), "''"),
    ] {
        let actual = execute(&mut socket, &format!("SELECT {expr} AS v"));
        let baseline = execute(&mut socket, &format!("SELECT {baseline} AS v"));
        // Go getFunction fields differ from a selected literal's fields.
        // Exact tuples below are independently observed on the repository Go server.
        assert_eq!(actual.0.len(), 1);
        assert_eq!(baseline.0.len(), 1);
        assert_eq!(
            actual.1,
            vec![
                expected
                    .map(|v| [vec![v.len() as u8], v.as_bytes().to_vec()].concat())
                    .unwrap_or(vec![0xfb])
            ],
            "{expr}"
        );
        assert_eq!(actual.2, 0);
        let mut offset = 0;
        for _ in 0..6 {
            offset += usize::from(actual.0[0][offset]) + 1;
        }
        let metadata = &actual.0[0][offset + 1..offset + 13];
        let expected = match expr {
            "concat('a',NULL,'b')" => (46u16, 12u32, 253u8, 0u16, 31u8),
            "concat(NULL,NULL)" => (63u16, 0u32, 253u8, 128u16, 31u8),
            "concat('v=',2)" => (46u16, 88u32, 253u8, 1u16, 31u8),
            "concat('Z','a')" => (46u16, 8u32, 253u8, 1u16, 31u8),
            "concat('x')" => (46u16, 4u32, 253u8, 1u16, 31u8),
            "concat('','')" => (46u16, 0u32, 253u8, 1u16, 31u8),
            _ => unreachable!("all original cases have independent Go observations"),
        };
        assert_eq!(
            (
                u16::from_le_bytes(metadata[..2].try_into().unwrap()),
                u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
                metadata[6],
                u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
                metadata[9]
            ),
            expected,
            "Go native fields: {expr}"
        );
    }
    mysql_packet(&mut socket, 0, b"\x03SELECT concat() AS v");
    let error = mysql_read(&mut socket);
    assert_eq!(error[0], 0xff);
    assert_eq!(u16::from_le_bytes(error[1..3].try_into().unwrap()), 1582);
    assert_eq!(&error[3..9], b"#42000");
    assert_eq!(execute(&mut socket, "SELECT 1 AS v").1, vec![vec![1, b'1']]);
    mysql_packet(&mut socket, 0, &[1]);
    drop(socket);
    drop(cleanup);
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_16() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }

    for (expression, expected, oid, width) in [
        ("greatest(NULL,7::int4)", Some("7"), 23, 4),
        ("least(NULL,7::int4)", Some("7"), 23, 4),
        ("greatest(NULL::int4,NULL::int4)", None, 23, 4),
        ("least(NULL::int4,NULL::int4)", None, 23, 4),
        ("greatest(NULL,NULL)", None, 25, -1),
        ("least(NULL,NULL)", None, 25, -1),
        ("greatest(2::int4,10.5::numeric)", Some("10.5"), 1700, -1),
        ("least(2::int4,10.5::numeric)", Some("2"), 1700, -1),
        ("greatest(7::int4)", Some("7"), 23, 4),
        ("least(7::int4)", Some("7"), 23, 4),
        ("greatest(2::int4,'10')", Some("10"), 23, 4),
        (
            "least((-2147483648)::int4,2147483647::int4)",
            Some("-2147483648"),
            23,
            4,
        ),
        (
            "greatest(1.00000000000000000001::numeric,1.00000000000000000002::numeric)",
            Some("1.00000000000000000002"),
            1700,
            -1,
        ),
        (
            "least((-1.00000000000000000001)::numeric,(-1.00000000000000000002)::numeric)",
            Some("-1.00000000000000000002"),
            1700,
            -1,
        ),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let m = query(&mut socket, &sql);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"TCZ".as_slice()
                } else {
                    b"TDCZ".as_slice()
                },
                "{sql}: {m:?}"
            );
            assert_eq!(m[0].1, description(oid, width, 0), "{sql}");
            if !empty {
                assert_eq!(m[1].1, row(&[expected]), "{sql}");
            }
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }

    for expression in [
        "greatest($1::int4,7::int4)",
        "least($1::int4,7::int4)",
        "greatest($1::int4,NULL::int4)",
        "least($1::int4,NULL::int4)",
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            send(
                &mut socket,
                b'P',
                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
            );
            send(&mut socket, b'D', b"S\0");
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tTZ",
                "{sql}: {m:?}"
            );
            assert_eq!(
                m[1].1,
                [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
            );
            assert_eq!(m[2].1, description(23, 4, 0));
            for (format, value) in [
                (0i16, Some(2i32)),
                (1, None),
                (1, Some(10i32)),
                (0, None),
                (0, Some(2i32)),
            ] {
                let mut bind = vec![0, 0];
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                bind.extend_from_slice(&1i16.to_be_bytes());
                if let Some(value) = value {
                    let bytes = if format == 0 {
                        value.to_string().into_bytes()
                    } else {
                        value.to_be_bytes().to_vec()
                    };
                    bind.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    bind.extend_from_slice(&bytes);
                } else {
                    bind.extend_from_slice(&(-1i32).to_be_bytes());
                }
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                send(&mut socket, b'B', &bind);
                send(&mut socket, b'D', b"P\0");
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'D', b"S\0");
                send(&mut socket, b'S', &[]);
                let m = ready(&mut socket);
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    if empty {
                        b"2TCtTZ".as_slice()
                    } else {
                        b"2TDCtTZ".as_slice()
                    },
                    "{sql}: {m:?}"
                );
                assert_eq!(m[1].1, description(23, 4, format));
                assert_eq!(
                    m[m.len() - 3].1,
                    [&1i16.to_be_bytes()[..], &23u32.to_be_bytes()].concat()
                );
                assert_eq!(m[m.len() - 2].1, description(23, 4, 0));
                let selected = if expression.contains("NULL") {
                    value
                } else if expression.starts_with("greatest") {
                    Some(value.unwrap_or(7).max(7))
                } else {
                    Some(value.unwrap_or(7).min(7))
                };
                if !empty {
                    let bytes = selected.map(|selected| {
                        if format == 0 {
                            selected.to_string().into_bytes()
                        } else {
                            selected.to_be_bytes().to_vec()
                        }
                    });
                    let length = bytes.as_ref().map_or(-1, |b| b.len() as i32);
                    let bytes = bytes.unwrap_or_default();
                    assert_eq!(
                        m[2].1,
                        [&1i16.to_be_bytes()[..], &length.to_be_bytes(), &bytes].concat(),
                        "{sql}"
                    );
                }
                assert_eq!(m.last().unwrap().1, b"I");
            }
        }
    }
    for (expression, state) in [
        ("greatest(1::int4,'a'::text)", "42804"),
        ("least(NULL::int4,NULL::text)", "42804"),
        ("greatest()", "42601"),
        ("least()", "42601"),
        ("greatest(VARIADIC ARRAY[1,2])", "42601"),
        ("least(VARIADIC ARRAY[1,2])", "42601"),
    ] {
        for parse in [false, true] {
            for in_transaction in [false, true] {
                if in_transaction {
                    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                }
                let sql = format!("SELECT {expression} AS v WHERE false");
                let m = if parse {
                    send(
                        &mut socket,
                        b'P',
                        &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                    );
                    send(&mut socket, b'S', &[]);
                    ready(&mut socket)
                } else {
                    query(&mut socket, &sql)
                };
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"EZ",
                    "{sql}: {m:?}"
                );
                let e = &m[0].1;
                assert!(
                    e.windows(7).any(|s| s == format!("C{state}\0").as_bytes()),
                    "{e:?}"
                );
                assert!(e.windows(7).any(|s| s == b"SERROR\0"));
                assert!(e.windows(7).any(|s| s == b"VERROR\0"));
                let fields = e
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                for tag in b"DH" {
                    assert!(!fields.iter().any(|f| f[0] == *tag), "{sql}: {fields:?}");
                }
                let position = fields
                    .iter()
                    .find(|f| f[0] == b'P')
                    .expect("real PG cursor");
                let expected = if state == "42804" {
                    25
                } else if expression.starts_with("greatest") {
                    17
                } else {
                    14
                };
                assert_eq!(&position[1..], expected.to_string().as_bytes(), "{sql}");
                assert_eq!(
                    m.last().unwrap().1,
                    if in_transaction { b"E" } else { b"I" }
                );
                if in_transaction {
                    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                }
                let probe = query(&mut socket, "SELECT 1::int4");
                assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                assert_eq!(probe[1].1, row(&[Some("1")]));
            }
        }
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_16_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server.clone());
    let address = server.listener_addr().unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);

    fn execute(socket: &mut TcpStream, sql: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, u16) {
        mysql_packet(socket, 0, format!("\x03{sql}").as_bytes());
        let first = mysql_read(socket);
        assert_ne!(first[0], 0xff, "{sql}: {first:?}");
        if first[0] == 0 {
            return (vec![], vec![], 0);
        }
        let columns = (0..first[0])
            .map(|_| mysql_read(socket))
            .collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut rows = vec![];
        loop {
            let packet = mysql_read(socket);
            if packet[0] == 0xfe && packet.len() < 9 {
                return (
                    columns,
                    rows,
                    u16::from_le_bytes(packet[1..3].try_into().unwrap()),
                );
            }
            rows.push(packet);
        }
    }

    for (expr, expected, baseline) in [
        ("greatest(NULL,7)", None, "NULL"),
        ("least(7,NULL)", None, "NULL"),
        ("greatest(2,10)", Some("10"), "10"),
        ("least(2,10)", Some("2"), "2"),
        ("greatest('2','10')", Some("2"), "'2'"),
        ("least('2','10')", Some("10"), "'10'"),
        ("greatest(2,'10')", Some("2"), "'2'"),
        ("least(2,'10')", Some("10"), "'10'"),
    ] {
        let actual = execute(&mut socket, &format!("SELECT {expr} AS v"));
        let baseline = execute(&mut socket, &format!("SELECT {baseline} AS v"));
        // Go getFunction fields differ from a selected literal's fields.
        // Exact tuples below are independently observed on the repository Go server.
        assert_eq!(actual.0.len(), 1);
        assert_eq!(baseline.0.len(), 1);
        assert_eq!(
            actual.1,
            vec![
                expected
                    .map(|v| [vec![v.len() as u8], v.as_bytes().to_vec()].concat())
                    .unwrap_or(vec![0xfb])
            ],
            "{expr}"
        );
        assert_eq!(actual.2, 0);
        let mut offset = 0;
        for _ in 0..6 {
            offset += usize::from(actual.0[0][offset]) + 1;
        }
        let metadata = &actual.0[0][offset + 1..offset + 13];
        let expected = match expr {
            "greatest(NULL,7)" => (63u16, 1u32, 8u8, 128u16, 0u8),
            "least(7,NULL)" => (63u16, 1u32, 8u8, 128u16, 0u8),
            "greatest(2,10)" => (63u16, 2u32, 8u8, 129u16, 0u8),
            "least(2,10)" => (63u16, 2u32, 8u8, 129u16, 0u8),
            "greatest('2','10')" => (46u16, 8u32, 253u8, 1u16, 0u8),
            "least('2','10')" => (46u16, 8u32, 253u8, 1u16, 0u8),
            "greatest(2,'10')" => (46u16, 80u32, 253u8, 1u16, 0u8),
            "least(2,'10')" => (46u16, 80u32, 253u8, 1u16, 0u8),
            _ => unreachable!("all original cases have independent Go observations"),
        };
        assert_eq!(
            (
                u16::from_le_bytes(metadata[..2].try_into().unwrap()),
                u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
                metadata[6],
                u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
                metadata[9]
            ),
            expected,
            "Go native fields: {expr}"
        );
    }
    for expression in ["greatest(7)", "least(7)", "greatest()", "least()"] {
        mysql_packet(
            &mut socket,
            0,
            format!("\x03SELECT {expression} AS v").as_bytes(),
        );
        let error = mysql_read(&mut socket);
        assert_eq!(error[0], 0xff, "{expression}: {error:?}");
        assert_eq!(u16::from_le_bytes(error[1..3].try_into().unwrap()), 1582);
        assert_eq!(&error[3..9], b"#42000");
        assert_eq!(execute(&mut socket, "SELECT 1 AS v").1, vec![vec![1, b'1']]);
    }
    mysql_packet(&mut socket, 0, &[1]);
    drop(socket);
    drop(cleanup);
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_17() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = Vec::new();
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    fn description(oid: u32, width: i16, format: i16) -> Vec<u8> {
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &oid.to_be_bytes(),
            &width.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &format.to_be_bytes(),
        ]
        .concat()
    }

    for (expression, expected) in [
        (r#"greatest('Z'::text COLLATE "C",'a'::text)"#, Some("a")),
        (r#"least('Z'::text COLLATE "C",'a'::text)"#, Some("Z")),
        (r#"greatest('2'::text COLLATE "C",'10'::text)"#, Some("2")),
        (r#"greatest(NULL::text COLLATE "C",NULL::text)"#, None),
        (r#"greatest(NULL::text,'A'::text COLLATE "C")"#, Some("A")),
        (r#"least('A'::text COLLATE "C",'a'::text)"#, Some("A")),
        (r#"least('A'::text COLLATE "C",'A'::text)"#, Some("A")),
        (r#"least('01'::text COLLATE "C",'1'::text)"#, Some("01")),
        (
            r#"greatest(NULL::text,'A'::text COLLATE "POSIX")"#,
            Some("A"),
        ),
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let m = query(&mut socket, &sql);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                if empty {
                    b"TCZ".as_slice()
                } else {
                    b"TDCZ".as_slice()
                },
                "{sql}: {m:?}"
            );
            assert_eq!(m[0].1, description(25, -1, 0), "{sql}");
            if !empty {
                assert_eq!(m[1].1, row(&[expected]), "{sql}");
            }
            assert_eq!(m.last().unwrap().1, b"I");
        }
    }

    for expression in [
        r#"greatest($1::text,'A'::text COLLATE "C")"#,
        r#"least($1::text COLLATE "C",'A'::text)"#,
    ] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            send(
                &mut socket,
                b'P',
                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
            );
            send(&mut socket, b'D', b"S\0");
            send(&mut socket, b'S', &[]);
            let m = ready(&mut socket);
            assert_eq!(
                m.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"1tTZ",
                "{sql}: {m:?}"
            );
            assert_eq!(
                m[1].1,
                [&1i16.to_be_bytes()[..], &25u32.to_be_bytes()].concat()
            );
            assert_eq!(m[2].1, description(25, -1, 0));
            for (format, value) in [
                (0i16, Some("a")),
                (1, None),
                (1, Some("A")),
                (0, None),
                (0, Some("a")),
            ] {
                let mut bind = vec![0, 0];
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                bind.extend_from_slice(&1i16.to_be_bytes());
                if let Some(value) = value {
                    bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
                    bind.extend_from_slice(value.as_bytes());
                } else {
                    bind.extend_from_slice(&(-1i32).to_be_bytes());
                }
                bind.extend_from_slice(&1i16.to_be_bytes());
                bind.extend_from_slice(&format.to_be_bytes());
                send(&mut socket, b'B', &bind);
                send(&mut socket, b'D', b"P\0");
                send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                send(&mut socket, b'S', &[]);
                let m = ready(&mut socket);
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    if empty {
                        b"2TCZ".as_slice()
                    } else {
                        b"2TDCZ".as_slice()
                    },
                    "{sql}: {m:?}"
                );
                assert_eq!(m[1].1, description(25, -1, format));
                let expected = if expression.starts_with("greatest") {
                    Some(value.unwrap_or("A").max("A"))
                } else {
                    Some(value.unwrap_or("A").min("A"))
                };
                if !empty {
                    assert_eq!(m[2].1, row(&[expected]), "{sql}");
                }
                assert_eq!(m.last().unwrap().1, b"I");
            }
        }
    }
    for expression in [
        r#"greatest('A'::text COLLATE "C",'b'::text COLLATE "POSIX")"#,
        r#"greatest(NULL::text COLLATE "C",'b'::text COLLATE "POSIX")"#,
        r#"least('A'::text COLLATE "C",'a'::text COLLATE "POSIX")"#,
    ] {
        for parse in [false, true] {
            for in_transaction in [false, true] {
                if in_transaction {
                    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                }
                let sql = format!("SELECT {expression} AS v WHERE false");
                let m = if parse {
                    send(
                        &mut socket,
                        b'P',
                        &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                    );
                    send(&mut socket, b'S', &[]);
                    ready(&mut socket)
                } else {
                    query(&mut socket, &sql)
                };
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"EZ",
                    "{sql}: {m:?}"
                );
                let e = &m[0].1;
                assert!(e.windows(7).any(|s| s == b"C42P21\0"), "{e:?}");
                assert!(e.windows(7).any(|s| s == b"SERROR\0"));
                assert!(e.windows(7).any(|s| s == b"VERROR\0"));
                let fields = e
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>();
                assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                for tag in b"DH" {
                    assert!(!fields.iter().any(|f| f[0] == *tag));
                }
                let expected_position = sql.rfind("COLLATE").unwrap() + 1;
                assert!(
                    fields.iter().any(
                        |f| f[0] == b'P' && &f[1..] == expected_position.to_string().as_bytes()
                    )
                );
                assert_eq!(
                    m.last().unwrap().1,
                    if in_transaction { b"E" } else { b"I" }
                );
                if in_transaction {
                    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                }
                let probe = query(&mut socket, "SELECT 1::int4");
                assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                assert_eq!(probe[1].1, row(&[Some("1")]));
            }
        }
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_17_identity() {
    use astersql_parser_ast::SqlDialect;
    use astersql_session::runtime::ConcreteSession;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for name in ["C", "POSIX"] {
        for empty in [false, true] {
            for expr in [
                format!("greatest(NULL::text,'A'::text COLLATE \"{name}\")"),
                format!("least('A'::text,'a'::text COLLATE \"{name}\")"),
            ] {
                let sql = format!(
                    "SELECT {expr} AS v{}",
                    if empty { " WHERE false" } else { "" }
                );
                let fields = pg.describe_result_fields(&sql).unwrap();
                assert_eq!(fields[0].column.FieldType.GetCollate(), name, "{sql}");
            }
            // The session API consumes engine markers; the TCP test above
            // independently verifies the public $1 spelling and translation.
            for expr in [
                format!("greatest(?::text,'A'::text COLLATE \"{name}\")"),
                format!("least(?::text,'a'::text COLLATE \"{name}\")"),
            ] {
                let sql = format!(
                    "SELECT {expr} AS v{}",
                    if empty { " WHERE false" } else { "" }
                );
                let (id, count, fields) = pg
                    .prepare_protocol_statement_with_parameter_types(&sql, &[25])
                    .unwrap();
                assert_eq!(count, 1);
                assert_eq!(fields[0].column.FieldType.GetCollate(), name, "{sql}");
                pg.close_protocol_statement(id).unwrap();
            }
        }
    }
    // Casting to a noncollatable result drops the text identity. Explicit
    // input collations must not create a spurious integer-result conflict.
    let sql =
        r#"SELECT greatest(('1'::text COLLATE "C")::int4,('2'::text COLLATE "POSIX")::int4) AS v"#;
    let fields = pg.describe_result_fields(sql).unwrap();
    assert_eq!(
        fields[0].column.FieldType.GetType(),
        astersql_parser_mysql::r#type::TypeLong
    );
    assert_eq!(
        pg.execute(sql).unwrap()[0].next_row().unwrap().unwrap(),
        vec!["2"]
    );
}

#[test]
fn pg_function_17_mysql() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server.clone());
    let address = server.listener_addr().unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    mysql_handshake(&mut socket);

    fn execute(socket: &mut TcpStream, sql: &str) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, u16) {
        mysql_packet(socket, 0, format!("\x03{sql}").as_bytes());
        let first = mysql_read(socket);
        assert_ne!(first[0], 0xff, "{sql}: {first:?}");
        if first[0] == 0 {
            return (vec![], vec![], 0);
        }
        let columns = (0..first[0])
            .map(|_| mysql_read(socket))
            .collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let mut rows = vec![];
        loop {
            let packet = mysql_read(socket);
            if packet[0] == 0xfe && packet.len() < 9 {
                return (
                    columns,
                    rows,
                    u16::from_le_bytes(packet[1..3].try_into().unwrap()),
                );
            }
            rows.push(packet);
        }
    }

    for (expr, expected, baseline) in [
        ("greatest('Z' COLLATE utf8mb4_bin,'a')", Some("a"), "'a'"),
        ("least('Z' COLLATE utf8mb4_bin,'a')", Some("Z"), "'Z'"),
        ("greatest(NULL,'A' COLLATE utf8mb4_bin)", None, "NULL"),
        ("least('A' COLLATE utf8mb4_bin,'a')", Some("A"), "'A'"),
        ("least('A' COLLATE utf8mb4_bin,'A')", Some("A"), "'A'"),
        ("least(NULL,'A' COLLATE utf8mb4_bin)", None, "NULL"),
        ("greatest('a' COLLATE utf8mb4_bin,'A')", Some("a"), "'a'"),
    ] {
        let actual = execute(&mut socket, &format!("SELECT {expr} AS v"));
        let baseline = execute(&mut socket, &format!("SELECT {baseline} AS v"));
        // Go getFunction fields differ from a selected literal's fields.
        // Exact tuples below are independently observed on the repository Go server.
        assert_eq!(actual.0.len(), 1);
        assert_eq!(baseline.0.len(), 1);
        assert_eq!(
            actual.1,
            vec![
                expected
                    .map(|v| [vec![v.len() as u8], v.as_bytes().to_vec()].concat())
                    .unwrap_or(vec![0xfb])
            ],
            "{expr}"
        );
        assert_eq!(actual.2, 0);
        let mut offset = 0;
        for _ in 0..6 {
            offset += usize::from(actual.0[0][offset]) + 1;
        }
        let metadata = &actual.0[0][offset + 1..offset + 13];
        let expected = match expr {
            "greatest('Z' COLLATE utf8mb4_bin,'a')" => (46u16, 4u32, 253u8, 1u16, 0u8),
            "least('Z' COLLATE utf8mb4_bin,'a')" => (46u16, 4u32, 253u8, 1u16, 0u8),
            "greatest(NULL,'A' COLLATE utf8mb4_bin)" => (46u16, 4u32, 253u8, 0u16, 0u8),
            "least('A' COLLATE utf8mb4_bin,'a')" => (46u16, 4u32, 253u8, 1u16, 0u8),
            "least('A' COLLATE utf8mb4_bin,'A')" => (46u16, 4u32, 253u8, 1u16, 0u8),
            "least(NULL,'A' COLLATE utf8mb4_bin)" => (46u16, 4u32, 253u8, 0u16, 0u8),
            "greatest('a' COLLATE utf8mb4_bin,'A')" => (46u16, 4u32, 253u8, 1u16, 0u8),
            _ => unreachable!("all original cases have independent Go observations"),
        };
        assert_eq!(
            (
                u16::from_le_bytes(metadata[..2].try_into().unwrap()),
                u32::from_le_bytes(metadata[2..6].try_into().unwrap()),
                metadata[6],
                u16::from_le_bytes(metadata[7..9].try_into().unwrap()),
                metadata[9]
            ),
            expected,
            "Go native fields: {expr}"
        );
    }
    mysql_packet(&mut socket, 0, &[1]);
    drop(socket);
    drop(cleanup);
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_18() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = vec![];
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    let table = format!("ifnull_pg18_{}", address.port());
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query(&mut self.0, "ROLLBACK");
                query(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    for sql in [
        format!("CREATE TABLE {table}(v integer)"),
        format!("INSERT INTO {table} VALUES(1)"),
    ] {
        assert_eq!(
            query(&mut socket, &sql)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"CZ",
            "{sql}"
        );
    }
    for prefix in ["", "pg_catalog."] {
        for call in [
            "ifnull('ok'::char(8),'backup'::char(12))",
            "ifnull(NULL::char(8),'backup'::char(12))",
            "ifnull(NULL::char(8),NULL::char(12))",
            "ifnull('bad'::int4,2)",
            "ifnull(7::int4,1/0)",
            "ifnull($1::text,$2::text)",
        ] {
            let expected_state = if call.contains("'bad'::int4") {
                "22P02"
            } else {
                "42883"
            };
            for parse in [false, true] {
                // Parameter markers belong only to the extended protocol.
                if !parse && call.contains('$') {
                    continue;
                }
                for empty in [false, true] {
                    for transaction in [false, true] {
                        if transaction {
                            assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                        }
                        let sql = format!(
                            "SELECT {prefix}{call} AS v{}",
                            if empty { " WHERE false" } else { "" }
                        );
                        let messages = if parse {
                            send(
                                &mut socket,
                                b'P',
                                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()]
                                    .concat(),
                            );
                            send(&mut socket, b'S', &[]);
                            ready(&mut socket)
                        } else {
                            query(&mut socket, &sql)
                        };
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            b"EZ",
                            "{sql}: {messages:?}"
                        );
                        let fields = messages[0]
                            .1
                            .split(|b| *b == 0)
                            .filter(|f| !f.is_empty())
                            .collect::<Vec<_>>();
                        let code_field = format!("C{expected_state}");
                        for field in [b"SERROR".as_slice(), b"VERROR", code_field.as_bytes()] {
                            assert!(fields.contains(&field), "{sql}: {fields:?}");
                        }
                        assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                        assert!(!fields.iter().any(|f| f[0] == b'D'), "{sql}: {fields:?}");
                        assert_eq!(
                            fields.iter().any(|f| f[0] == b'H'),
                            expected_state == "42883",
                            "{sql}: {fields:?}"
                        );
                        assert!(fields.iter().any(|f| f[0] == b'P'), "{sql}: {fields:?}");
                        assert_eq!(
                            messages.last().unwrap().1,
                            if transaction { b"E" } else { b"I" }
                        );
                        if transaction {
                            assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                        }
                        let probe = query(&mut socket, "SELECT 1::int4");
                        assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                        assert_eq!(probe[1].1, row(&[Some("1")]));
                    }
                }
            }
        }
    }
    for prefix in ["", "pg_catalog."] {
        for parse in [false, true] {
            let sql = format!("INSERT INTO {table} VALUES({prefix}ifnull(1,2))");
            let messages = if parse {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                send(&mut socket, b'S', &[]);
                ready(&mut socket)
            } else {
                query(&mut socket, &sql)
            };
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"EZ",
                "{sql}: {messages:?}"
            );
            assert!(messages[0].1.windows(7).any(|f| f == b"C42883\0"));
            let unchanged = query(&mut socket, &format!("SELECT v FROM {table}"));
            assert_eq!(unchanged.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
            assert_eq!(unchanged[1].1, row(&[Some("1")]));
            assert_eq!(unchanged.last().unwrap().1, b"I");
        }
    }
    // Name rejection must not change COALESCE's independent PG behavior.
    let coalesce = query(&mut socket, "SELECT coalesce(NULL::int4,7::int4) AS v");
    assert_eq!(coalesce.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
    assert_eq!(
        coalesce[0].1,
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &23u32.to_be_bytes(),
            &4i16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes()
        ]
        .concat()
    );
    assert_eq!(coalesce[1].1, row(&[Some("7")]));
    drop(table_cleanup);
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_18_mysql() {
    use crate::mysql_compat_test_support::{
        CLIENT_DEPRECATE_EOF, ColumnDefinition, MysqlCompatServer, MysqlTestClient, TextResultSet,
        TextValue, WireResponse, put_lenenc_bytes,
    };
    fn result(response: WireResponse, context: &str) -> TextResultSet {
        let WireResponse::ResultSet(result) = response else {
            panic!("{context}: {response:?}")
        };
        result
    }
    fn bytes(value: Option<&str>) -> TextValue {
        value
            .map(|v| TextValue::Bytes(v.as_bytes().to_vec()))
            .unwrap_or(TextValue::Null)
    }
    fn description(
        length: u32,
        code: u8,
        flags: u16,
        charset: u16,
        decimals: u8,
    ) -> Vec<ColumnDefinition> {
        vec![ColumnDefinition {
            catalog: "def".into(),
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            character_set: charset,
            column_length: length,
            column_type: code,
            flags,
            decimals,
            default_value: None,
        }]
    }
    let server = MysqlCompatServer::start().unwrap();
    let address = server.mysql_addr();
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, Some("test"))
        .unwrap();
    let table = format!("ifnull_contract18_{}", address.port());
    struct TableCleanup(MysqlTestClient, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = self.0.query(&format!("DROP TABLE IF EXISTS {}", self.1));
        }
    }
    let table_cleanup = TableCleanup(
        server
            .connect_root(CLIENT_DEPRECATE_EOF, Some("test"))
            .unwrap(),
        table.clone(),
    );
    assert!(matches!(
        client
            .query(&format!("CREATE TABLE {table}(a CHAR(8),z BIGINT)"))
            .unwrap(),
        WireResponse::Ok(_)
    ));
    // Full wire metadata is compared with the declared aggregate result type,
    // including expression origin and charset/length/type/flags/decimals.
    for (expression, expected) in [
        (
            "IFNULL(CAST('ok' AS CHAR(8)),CAST('backup' AS CHAR(12)))",
            Some("ok"),
        ),
        (
            "IFNULL(CAST(NULL AS CHAR(8)),CAST('backup' AS CHAR(12)))",
            Some("backup"),
        ),
        ("IFNULL(CAST(NULL AS CHAR(8)),CAST(NULL AS CHAR(12)))", None),
        ("IFNULL(NULL,NULL)", None),
        (
            "IFNULL(CAST('' AS CHAR(8)),CAST('backup' AS CHAR(12)))",
            Some(""),
        ),
        (
            "IFNULL(CAST('你好' AS CHAR(8)),CAST('backup' AS CHAR(12)))",
            Some("你好"),
        ),
        (
            "IFNULL(CAST(7 AS SIGNED),CAST('backup' AS CHAR(12)))",
            Some("7"),
        ),
    ] {
        // Go's ifNullFoldHandler keeps a non-NULL constant's own type,
        // or the second arm's type when the first folds to NULL. Parameter
        // and column operands below separately exercise aggregate metadata.
        let expected_columns = if expression == "IFNULL(NULL,NULL)" {
            description(0, 6, 128, 63, 0)
        } else if expression.contains("SIGNED") {
            description(80, 253, 1, 46, 31)
        } else if expression.starts_with("IFNULL(CAST(NULL") {
            description(48, 253, if expected.is_some() { 1 } else { 0 }, 46, 31)
        } else {
            description(32, 253, 1, 46, 31)
        };
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty {
                    format!(" FROM {table}")
                } else {
                    String::new()
                }
            );
            let actual = result(client.query(&sql).unwrap(), &sql);
            assert_eq!(actual.columns, expected_columns, "{sql}");
            assert_eq!(
                actual.rows,
                if empty {
                    vec![]
                } else {
                    vec![vec![bytes(expected)]]
                },
                "{sql}"
            );
            assert_eq!(actual.warnings, 0, "{sql}");
        }
    }
    for empty in [false, true] {
        let sql = format!(
            "SELECT IFNULL(CAST(? AS CHAR(8)),CAST(? AS CHAR(12))) AS v{}",
            if empty {
                format!(" FROM {table}")
            } else {
                String::new()
            }
        );
        let prepared = client.prepare(&sql).unwrap();
        assert_eq!(prepared.parameter_count, 2);
        assert_eq!(prepared.columns, description(48, 253, 0, 46, 31), "{sql}");
        for (first, second) in [
            (Some("ok"), Some("backup")),
            (None, Some("backup")),
            (None, None),
            (Some(""), Some("backup")),
            (Some("你好"), None),
            (None, Some("again")),
        ] {
            let mut payload = prepared.statement_id.to_le_bytes().to_vec();
            payload.push(0);
            payload.extend_from_slice(&1u32.to_le_bytes());
            payload.push(u8::from(first.is_none()) | (u8::from(second.is_none()) << 1));
            payload.push(1);
            payload.extend_from_slice(&[253, 0, 253, 0]);
            for value in [first, second].into_iter().flatten() {
                put_lenenc_bytes(&mut payload, value.as_bytes());
            }
            let actual = result(client.execute_prepared(&payload).unwrap(), &sql);
            assert_eq!(actual.columns, description(48, 253, 1, 46, 31), "{sql}");
            assert_eq!(
                actual.rows,
                if empty {
                    vec![]
                } else {
                    vec![vec![bytes(first.or(second))]]
                },
                "{sql}"
            );
            assert_eq!(
                actual.warnings,
                u16::from(!empty),
                "only a table-free projection rejects the TableDual cache plan"
            );
        }
        client.close_prepared(prepared.statement_id).unwrap();
    }
    // Table operands prevent constant folding from substituting for runtime
    // short circuiting. ABS(BIGINT_MIN) is an actual reached-branch error.
    assert!(matches!(
        client
            .query(&format!(
                "INSERT INTO {table} VALUES('ok',-9223372036854775808)"
            ))
            .unwrap(),
        WireResponse::Ok(_)
    ));
    for prepared in [false, true] {
        let sql =
            format!("SELECT IFNULL(a,CAST(ABS(CAST(z AS SIGNED)) AS CHAR(24))) AS v FROM {table}");
        let statement = prepared.then(|| client.prepare(&sql).unwrap());
        if let Some(statement) = &statement {
            assert_eq!(statement.columns, description(96, 253, 0, 46, 31), "{sql}");
        }
        for null in [false, true, false] {
            assert!(matches!(
                client
                    .query(&format!(
                        "UPDATE {table} SET a={}",
                        if null { "NULL" } else { "'ok'" }
                    ))
                    .unwrap(),
                WireResponse::Ok(_)
            ));
            let response = if let Some(statement) = &statement {
                let mut payload = statement.statement_id.to_le_bytes().to_vec();
                payload.push(0);
                payload.extend_from_slice(&1u32.to_le_bytes());
                client.execute_prepared(&payload).unwrap()
            } else {
                client.query(&sql).unwrap()
            };
            if null {
                let WireResponse::Err(error) = response else {
                    panic!("reached ABS error: {response:?}")
                };
                assert_eq!(error.code, 1690);
                assert_eq!(error.sql_state, Some(*b"22003"));
                assert!(!error.message.is_empty());
            } else {
                let result = result(response, &sql);
                assert_eq!(result.columns, description(96, 253, 0, 46, 31), "{sql}");
                assert_eq!(result.rows, vec![vec![bytes(Some("ok"))]]);
            }
            assert_eq!(
                result(client.query("SELECT 1 AS v").unwrap(), "recovery").rows,
                vec![vec![bytes(Some("1"))]]
            );
            // SELECT and its failed fallback must never change table contents.
            assert_eq!(
                result(
                    client.query(&format!("SELECT a,z FROM {table}")).unwrap(),
                    "no writes"
                )
                .rows,
                vec![vec![
                    bytes(if null { None } else { Some("ok") }),
                    bytes(Some("-9223372036854775808"))
                ]]
            );
        }
        if let Some(statement) = statement {
            client.close_prepared(statement.statement_id).unwrap();
        }
    }
    // An error in the first operand must never be mistaken for NULL.
    for prepared in [false, true] {
        let sql =
            format!("SELECT IFNULL(CAST(ABS(CAST(z AS SIGNED)) AS CHAR(24)),a) AS v FROM {table}");
        let statement = prepared.then(|| client.prepare(&sql).unwrap());
        let response = if let Some(statement) = &statement {
            let mut payload = statement.statement_id.to_le_bytes().to_vec();
            payload.push(0);
            payload.extend_from_slice(&1u32.to_le_bytes());
            client.execute_prepared(&payload).unwrap()
        } else {
            client.query(&sql).unwrap()
        };
        let WireResponse::Err(error) = response else {
            panic!("first operand error: {response:?}")
        };
        assert_eq!(error.code, 1690);
        assert_eq!(error.sql_state, Some(*b"22003"));
        assert!(!error.message.is_empty());
        assert_eq!(
            result(
                client.query("SELECT 1 AS v").unwrap(),
                "first error recovery"
            )
            .rows,
            vec![vec![bytes(Some("1"))]]
        );
        if let Some(statement) = statement {
            client.close_prepared(statement.statement_id).unwrap();
        }
    }
    drop(table_cleanup);
    drop(client);
    server.close();
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_19() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = vec![];
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    let table = format!("date_format_pg19_{}", address.port());
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query(&mut self.0, "ROLLBACK");
                query(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    for sql in [
        format!("CREATE TABLE {table}(v integer)"),
        format!("INSERT INTO {table} VALUES(1)"),
    ] {
        assert_eq!(
            query(&mut socket, &sql)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"CZ",
            "{sql}"
        );
    }
    for prefix in ["", "pg_catalog."] {
        for call in [
            "date_format('2026-01-02','%Y-%m-%d')",
            "date_format(NULL::text,'%Y')",
            "date_format('2026-01-02',NULL::text)",
            "date_format('2026-01-02','')",
            "date_format('bad'::int4,'%Y')",
            "date_format('0000-00-00','%Y')",
            "date_format($1::text,$2::text)",
        ] {
            let expected_state = if call.contains("'bad'::int4") {
                "22P02"
            } else {
                "42883"
            };
            for parse in [false, true] {
                // Parameter markers belong only to the extended protocol.
                if !parse && call.contains('$') {
                    continue;
                }
                for empty in [false, true] {
                    for transaction in [false, true] {
                        if transaction {
                            assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                        }
                        let sql = format!(
                            "SELECT {prefix}{call} AS v{}",
                            if empty { " WHERE false" } else { "" }
                        );
                        let messages = if parse {
                            send(
                                &mut socket,
                                b'P',
                                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()]
                                    .concat(),
                            );
                            send(&mut socket, b'S', &[]);
                            ready(&mut socket)
                        } else {
                            query(&mut socket, &sql)
                        };
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            b"EZ",
                            "{sql}: {messages:?}"
                        );
                        let fields = messages[0]
                            .1
                            .split(|b| *b == 0)
                            .filter(|f| !f.is_empty())
                            .collect::<Vec<_>>();
                        let code_field = format!("C{expected_state}");
                        for field in [b"SERROR".as_slice(), b"VERROR", code_field.as_bytes()] {
                            assert!(fields.contains(&field), "{sql}: {fields:?}");
                        }
                        assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                        assert!(!fields.iter().any(|f| f[0] == b'D'), "{sql}: {fields:?}");
                        assert_eq!(
                            fields.iter().any(|f| f[0] == b'H'),
                            expected_state == "42883",
                            "{sql}: {fields:?}"
                        );
                        assert!(fields.iter().any(|f| f[0] == b'P'), "{sql}: {fields:?}");
                        assert_eq!(
                            messages.last().unwrap().1,
                            if transaction { b"E" } else { b"I" }
                        );
                        if transaction {
                            assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                        }
                        let probe = query(&mut socket, "SELECT 1::int4");
                        assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                        assert_eq!(probe[1].1, row(&[Some("1")]));
                    }
                }
            }
        }
    }
    for prefix in ["", "pg_catalog."] {
        for parse in [false, true] {
            let sql = format!("INSERT INTO {table} VALUES({prefix}date_format('bad'::int4,'%Y'))");
            let messages = if parse {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                send(&mut socket, b'S', &[]);
                ready(&mut socket)
            } else {
                query(&mut socket, &sql)
            };
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"EZ",
                "{sql}: {messages:?}"
            );
            assert!(messages[0].1.windows(7).any(|f| f == b"C22P02\0"));
            let unchanged = query(&mut socket, &format!("SELECT v FROM {table}"));
            assert_eq!(unchanged.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
            assert_eq!(unchanged[1].1, row(&[Some("1")]));
            assert_eq!(unchanged.last().unwrap().1, b"I");
        }
    }
    // Name rejection must not change COALESCE's independent PG behavior.
    let coalesce = query(&mut socket, "SELECT coalesce(NULL::int4,7::int4) AS v");
    assert_eq!(coalesce.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
    assert_eq!(
        coalesce[0].1,
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &23u32.to_be_bytes(),
            &4i16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes()
        ]
        .concat()
    );
    assert_eq!(coalesce[1].1, row(&[Some("7")]));
    drop(table_cleanup);
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_19_mysql() {
    use crate::mysql_compat_test_support::{
        CLIENT_DEPRECATE_EOF, ColumnDefinition, MysqlCompatServer, TextValue, WireResponse,
        put_lenenc_bytes,
    };
    let server = MysqlCompatServer::start().unwrap();
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, Some("test"))
        .unwrap();
    for setting in [
        "SET sql_mode='STRICT_TRANS_TABLES,NO_ZERO_DATE,NO_ZERO_IN_DATE'",
        "SET time_zone='+00:00'",
        "SET lc_time_names='en_US'",
    ] {
        assert!(
            matches!(client.query(setting).unwrap(), WireResponse::Ok(_)),
            "{setting}"
        );
    }
    for (expression, expected) in [
        ("DATE_FORMAT('2026-01-02','%Y-%m-%d')", Some("2026-01-02")),
        ("DATE_FORMAT(NULL,'%Y')", None),
        ("DATE_FORMAT('2026-01-02',NULL)", None),
        ("DATE_FORMAT('2026-01-02','')", Some("")),
    ] {
        let response = client.query(&format!("SELECT {expression} AS v")).unwrap();
        let WireResponse::ResultSet(result) = response else {
            panic!("{expression}: {response:?}")
        };
        assert_eq!(
            result.rows,
            vec![vec![
                expected
                    .map(|v| TextValue::Bytes(v.as_bytes().to_vec()))
                    .unwrap_or(TextValue::Null)
            ]],
            "{expression}"
        );
    }
    // Go evalString handles invalid time through handleInvalidTimeError:
    // a SELECT yields NULL and a warning even under the fixed strict mode.
    for input in ["bad", "0000-00-00", "2026-00-02", "2026-02-31"] {
        let sql = format!("SELECT DATE_FORMAT('{input}','%Y-%m-%d') AS v");
        let response = client.query(&sql).unwrap();
        let WireResponse::ResultSet(result) = response else {
            panic!("invalid date must yield NULL plus warning: {sql}: {response:?}")
        };
        assert_eq!(result.rows, vec![vec![TextValue::Null]], "{sql}");
        assert_eq!(result.warnings, 1, "{sql}");
        let WireResponse::ResultSet(warnings) = client.query("SHOW WARNINGS").unwrap() else {
            panic!("SHOW WARNINGS")
        };
        assert_eq!(warnings.rows.len(), 1, "{sql}");
        assert_eq!(warnings.rows[0][0], TextValue::Bytes(b"Warning".to_vec()));
        assert_eq!(warnings.rows[0][1], TextValue::Bytes(b"1292".to_vec()));
        let WireResponse::ResultSet(probe) = client.query("SELECT 1 AS v").unwrap() else {
            panic!("recovery")
        };
        assert_eq!(probe.rows, vec![vec![TextValue::Bytes(b"1".to_vec())]]);
    }
    for (expression, expected) in [
        ("DATE_FORMAT(0,'%Y')", None::<&str>),
        ("DATE_FORMAT(0.0,'%Y')", None),
        // String zero is converted under NO_ZERO_DATE before the mask.
    ] {
        let WireResponse::ResultSet(result) =
            client.query(&format!("SELECT {expression} AS v")).unwrap()
        else {
            panic!("{expression}")
        };
        assert_eq!(
            result.rows,
            vec![vec![
                expected
                    .map(|v| TextValue::Bytes(v.as_bytes().to_vec()))
                    .unwrap_or(TextValue::Null)
            ]],
            "{expression}"
        );
        assert_eq!(result.warnings, 0, "{expression}");
    }
    let WireResponse::ResultSet(zero) = client
        .query("SELECT DATE_FORMAT('0000-00-00','0') AS v")
        .unwrap()
    else {
        panic!("zero date")
    };
    assert_eq!(zero.rows, vec![vec![TextValue::Null]]);
    assert_eq!(zero.warnings, 1);
    for expression in [
        "DATE_FORMAT(NULL,ABS(CAST('-9223372036854775808' AS SIGNED)))",
        "DATE_FORMAT(ABS(CAST('-9223372036854775808' AS SIGNED)),NULL)",
        "DATE_FORMAT('2026-01-02',ABS(CAST('-9223372036854775808' AS SIGNED)))",
    ] {
        let WireResponse::Err(error) = client.query(&format!("SELECT {expression} AS v")).unwrap()
        else {
            panic!("{expression}")
        };
        assert_eq!(error.code, 1690, "{expression}");
        assert_eq!(error.sql_state, Some(*b"22003"), "{expression}");
        let WireResponse::ResultSet(probe) = client.query("SELECT 1 AS v").unwrap() else {
            panic!("recovery")
        };
        assert_eq!(probe.rows, vec![vec![TextValue::Bytes(b"1".to_vec())]]);
    }
    // Preserve all five Go TestDateFormat rows, including fractional
    // seconds, week-year boundaries, year zero and unknown format tokens.
    for (date, mask, expected) in [
        (
            "2010-01-07 23:12:34.12345",
            "%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y %%",
            "Jan January 01 1 7th 07 7 007 23 11 12 PM 11:12:34 PM 23:12:34 34 123450 01 01 01 01 Thu Thursday 4 2010 2010 2010 10 %",
        ),
        (
            "2012-12-21 23:12:34.123456",
            "%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y %%",
            "Dec December 12 12 21st 21 21 356 23 11 12 PM 11:12:34 PM 23:12:34 34 123456 51 51 51 51 Fri Friday 5 2012 2012 2012 12 %",
        ),
        (
            "0000-01-01 00:00:00.123456",
            "%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %v %x %Y %y %%",
            "Jan January 01 1 1st 01 1 001 0 12 00 AM 12:00:00 AM 00:00:00 00 123456 52 4294967295 0000 00 %",
        ),
        (
            "2016-09-3 00:59:59.123456",
            "abc%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y!123 %%xyz %z",
            "abcSep September 09 9 3rd 03 3 247 0 12 59 AM 12:59:59 AM 00:59:59 59 123456 35 35 35 35 Sat Saturday 6 2016 2016 2016 16!123 %xyz z",
        ),
        (
            "2012-10-01 00:00:00",
            "%b %M %m %c %D %d %e %j %k %H %i %p %r %T %s %f %v %x %Y %y %%",
            "Oct October 10 10 1st 01 1 275 0 00 00 AM 12:00:00 AM 00:00:00 00 000000 40 2012 2012 12 %",
        ),
    ] {
        let sql = format!("SELECT DATE_FORMAT('{date}','{mask}') AS v");
        let WireResponse::ResultSet(result) = client.query(&sql).unwrap() else {
            panic!("{sql}")
        };
        assert_eq!(
            result.rows,
            vec![vec![TextValue::Bytes(expected.as_bytes().to_vec())]],
            "{sql}"
        );
        assert_eq!(result.warnings, 0, "{sql}");
    }
    fn description(length: u32, flags: u16) -> Vec<ColumnDefinition> {
        vec![ColumnDefinition {
            catalog: "def".into(),
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            character_set: 46,
            column_length: length,
            column_type: 253,
            flags,
            decimals: 31,
            default_value: None,
        }]
    }
    // Go getFunction expands the declared mask length, not the selected value.
    for (mask, length) in [("%Y-%m-%d", 176), ("%r%r", 88), ("", 0)] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT DATE_FORMAT('2026-01-02','{mask}') AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let WireResponse::ResultSet(result) = client.query(&sql).unwrap() else {
                panic!("{sql}")
            };
            assert_eq!(result.columns, description(length, 1), "{sql}");
            assert_eq!(result.rows.len(), usize::from(!empty));
        }
    }
    for empty in [false, true] {
        let sql = format!(
            "SELECT DATE_FORMAT(CAST(? AS CHAR(26)),CAST(? AS CHAR(8))) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let prepared = client.prepare(&sql).unwrap();
        assert_eq!(prepared.parameter_count, 2);
        assert_eq!(prepared.columns, description(176, 0), "{sql}");
        for (date, mask, expected) in [
            (Some("2026-01-02"), Some("%Y-%m-%d"), Some("2026-01-02")),
            (None, Some("%Y"), None),
            (Some("2026-01-02"), None, None),
            (Some("2026-01-02"), Some(""), Some("")),
            (Some("2026-01-02"), Some("%Y"), Some("2026")),
            (Some("bad"), Some("%Y"), None),
            (Some("2026-01-02"), Some("%Y"), Some("2026")),
        ] {
            let mut payload = prepared.statement_id.to_le_bytes().to_vec();
            payload.push(0);
            payload.extend_from_slice(&1u32.to_le_bytes());
            payload.push(u8::from(date.is_none()) | (u8::from(mask.is_none()) << 1));
            payload.push(1);
            payload.extend_from_slice(&[253, 0, 253, 0]);
            for value in [date, mask].into_iter().flatten() {
                put_lenenc_bytes(&mut payload, value.as_bytes());
            }
            let WireResponse::ResultSet(result) = client.execute_prepared(&payload).unwrap() else {
                panic!("{sql}")
            };
            assert_eq!(result.columns, description(176, 1), "{sql}");
            assert_eq!(
                result.warnings,
                1 + if date == Some("bad") {
                    if empty { 3 } else { 4 }
                } else {
                    0
                },
                "{sql}"
            );
            assert_eq!(
                result.rows,
                if empty {
                    vec![]
                } else {
                    vec![vec![
                        expected
                            .map(|v| TextValue::Bytes(v.as_bytes().to_vec()))
                            .unwrap_or(TextValue::Null),
                    ]]
                },
                "{sql}"
            );
        }
        client.close_prepared(prepared.statement_id).unwrap();
    }
    drop(client);
    let address = server.mysql_addr();
    server.close();
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_20() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = vec![];
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    let table = format!("json_extract_pg20_{}", address.port());
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query(&mut self.0, "ROLLBACK");
                query(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    for sql in [
        format!("CREATE TABLE {table}(v integer)"),
        format!("INSERT INTO {table} VALUES(1)"),
    ] {
        assert_eq!(
            query(&mut socket, &sql)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"CZ",
            "{sql}"
        );
    }
    for prefix in ["", "pg_catalog."] {
        for call in [
            r#"json_extract('{"a":1}','$.a')"#,
            "json_extract(NULL::text,'$.a')",
            "json_extract('{}',NULL::text)",
            "json_extract('bad'::int4,'$InvalidPath')",
            "json_extract('{}','$InvalidPath')",
            "json_extract('null','$')",
            "json_extract('{}')",
            "json_extract($1::text,$2::text)",
        ] {
            let expected_state = if call.contains("'bad'::int4") {
                "22P02"
            } else {
                "42883"
            };
            for parse in [false, true] {
                // Parameter markers belong only to the extended protocol.
                if !parse && call.contains('$') {
                    continue;
                }
                for empty in [false, true] {
                    for transaction in [false, true] {
                        if transaction {
                            assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                        }
                        let sql = format!(
                            "SELECT {prefix}{call} AS v{}",
                            if empty { " WHERE false" } else { "" }
                        );
                        let messages = if parse {
                            send(
                                &mut socket,
                                b'P',
                                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()]
                                    .concat(),
                            );
                            send(&mut socket, b'S', &[]);
                            ready(&mut socket)
                        } else {
                            query(&mut socket, &sql)
                        };
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            b"EZ",
                            "{sql}: {messages:?}"
                        );
                        let fields = messages[0]
                            .1
                            .split(|b| *b == 0)
                            .filter(|f| !f.is_empty())
                            .collect::<Vec<_>>();
                        let code_field = format!("C{expected_state}");
                        for field in [b"SERROR".as_slice(), b"VERROR", code_field.as_bytes()] {
                            assert!(fields.contains(&field), "{sql}: {fields:?}");
                        }
                        assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                        assert!(!fields.iter().any(|f| f[0] == b'D'), "{sql}: {fields:?}");
                        assert_eq!(
                            fields.iter().any(|f| f[0] == b'H'),
                            expected_state == "42883",
                            "{sql}: {fields:?}"
                        );
                        assert!(fields.iter().any(|f| f[0] == b'P'), "{sql}: {fields:?}");
                        assert_eq!(
                            messages.last().unwrap().1,
                            if transaction { b"E" } else { b"I" }
                        );
                        if transaction {
                            assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                        }
                        let probe = query(&mut socket, "SELECT 1::int4");
                        assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                        assert_eq!(probe[1].1, row(&[Some("1")]));
                    }
                }
            }
        }
    }
    for prefix in ["", "pg_catalog."] {
        for parse in [false, true] {
            let sql = format!(
                "INSERT INTO {table} VALUES({prefix}json_extract('bad'::int4,'$InvalidPath'))"
            );
            let messages = if parse {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                send(&mut socket, b'S', &[]);
                ready(&mut socket)
            } else {
                query(&mut socket, &sql)
            };
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"EZ",
                "{sql}: {messages:?}"
            );
            assert!(messages[0].1.windows(7).any(|f| f == b"C22P02\0"));
            let unchanged = query(&mut socket, &format!("SELECT v FROM {table}"));
            assert_eq!(unchanged.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
            assert_eq!(unchanged[1].1, row(&[Some("1")]));
            assert_eq!(unchanged.last().unwrap().1, b"I");
        }
    }
    // Name rejection must not change COALESCE's independent PG behavior.
    let coalesce = query(&mut socket, "SELECT coalesce(NULL::int4,7::int4) AS v");
    assert_eq!(coalesce.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
    assert_eq!(
        coalesce[0].1,
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &23u32.to_be_bytes(),
            &4i16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes()
        ]
        .concat()
    );
    assert_eq!(coalesce[1].1, row(&[Some("7")]));
    drop(table_cleanup);
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_20_mysql() {
    use crate::mysql_compat_test_support::{
        CLIENT_DEPRECATE_EOF, ColumnDefinition, MysqlCompatServer, TextValue, WireResponse,
        put_lenenc_bytes,
    };
    let server = MysqlCompatServer::start().unwrap();
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, Some("test"))
        .unwrap();
    fn description(flags: u16) -> Vec<ColumnDefinition> {
        vec![ColumnDefinition {
            catalog: "def".into(),
            schema: String::new(),
            table: String::new(),
            org_table: String::new(),
            name: "v".into(),
            org_name: String::new(),
            character_set: 46,
            column_length: 16_777_216,
            column_type: 245,
            flags,
            decimals: 0,
            default_value: None,
        }]
    }
    let cases = [
        (r#"JSON_EXTRACT('{"a":1}', '$.a')"#, Some("1")),
        (r#"JSON_EXTRACT('{"a":null}', '$.a')"#, Some("null")),
        ("JSON_EXTRACT('{}', '$.missing')", None),
        ("JSON_EXTRACT(NULL, '$.a')", None),
        ("JSON_EXTRACT('{}', NULL)", None),
        ("JSON_EXTRACT(NULL, NULL)", None),
        ("JSON_EXTRACT('null', '$')", Some("null")),
        (
            r#"JSON_EXTRACT('{"a": [{"aa": [{"aaa": 1}]}], "aaa": 2}', '$.a[0].aa[0].aaa', '$.aaa')"#,
            Some("[1, 2]"),
        ),
    ];
    for (expression, expected) in cases {
        for empty in [false, true] {
            let sql = format!(
                "SELECT {expression} AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            let response = client.query(&sql).unwrap();
            let WireResponse::ResultSet(result) = response else {
                panic!("{sql}: {response:?}")
            };
            assert_eq!(result.columns.len(), 1);
            assert_eq!(result.columns[0].name, "v");
            assert_eq!(
                result.columns[0].column_type, 245,
                "{sql}: {:?}",
                result.columns
            );
            assert_eq!(result.columns, description(128), "{sql}");
            assert_eq!(result.warnings, 0);
            assert_eq!(
                result.rows,
                if empty {
                    vec![]
                } else {
                    vec![vec![
                        expected
                            .map(|v| TextValue::Bytes(v.as_bytes().to_vec()))
                            .unwrap_or(TextValue::Null),
                    ]]
                },
                "{sql}"
            );
        }
    }
    for expression in [
        "JSON_EXTRACT(1, '$')",
        "JSON_EXTRACT('bad', '$.a')",
        "JSON_EXTRACT('{}', '$InvalidPath')",
        r#"JSON_EXTRACT('{"a":1}', '$.a', '$InvalidPath')"#,
        "JSON_EXTRACT('{}')",
    ] {
        let response = client.query(&format!("SELECT {expression} AS v")).unwrap();
        let WireResponse::Err(error) = response else {
            panic!("{expression}: {response:?}")
        };
        assert_ne!(error.code, 0);
        assert!(error.sql_state.is_some());
        assert!(!error.message.is_empty());
        eprintln!("JSON Query rejection {expression}: {error:?}");
        let WireResponse::ResultSet(probe) = client.query("SELECT 1 AS v").unwrap() else {
            panic!("recovery")
        };
        assert_eq!(probe.rows, vec![vec![TextValue::Bytes(b"1".to_vec())]]);
    }
    for empty in [false, true] {
        let sql = format!(
            "SELECT JSON_EXTRACT(?, ?) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let prepared = client.prepare(&sql).unwrap();
        assert_eq!(prepared.parameter_count, 2);
        assert_eq!(prepared.columns, description(128), "{sql}");
        for (document, path, expected) in [
            (Some(r#"{"a":1}"#), Some("$.a"), Some("1")),
            (Some(r#"{"a":null}"#), Some("$.a"), Some("null")),
            (Some("{}"), Some("$.missing"), None),
            (None, Some("$.a"), None),
            (Some("{}"), None, None),
            (Some(r#"{"a":2}"#), Some("$.a"), Some("2")),
        ] {
            let mut payload = prepared.statement_id.to_le_bytes().to_vec();
            payload.push(0);
            payload.extend_from_slice(&1u32.to_le_bytes());
            payload.push(u8::from(document.is_none()) | (u8::from(path.is_none()) << 1));
            payload.push(1);
            payload.extend_from_slice(&[253, 0, 253, 0]);
            for value in [document, path].into_iter().flatten() {
                put_lenenc_bytes(&mut payload, value.as_bytes());
            }
            let response = client.execute_prepared(&payload).unwrap();
            let WireResponse::ResultSet(result) = response else {
                panic!("{sql}: {response:?}")
            };
            assert_eq!(result.columns, prepared.columns, "{sql}");
            assert_eq!(
                result.rows,
                if empty {
                    vec![]
                } else {
                    vec![vec![
                        expected
                            .map(|v| TextValue::Bytes(v.as_bytes().to_vec()))
                            .unwrap_or(TextValue::Null),
                    ]]
                },
                "{sql}: {document:?} {path:?}"
            );
        }
        if !empty {
            for (document, path) in [("bad", "$.a"), ("{}", "$InvalidPath")] {
                let mut payload = prepared.statement_id.to_le_bytes().to_vec();
                payload.push(0);
                payload.extend_from_slice(&1u32.to_le_bytes());
                payload.extend_from_slice(&[0, 1, 253, 0, 253, 0]);
                for value in [document, path] {
                    put_lenenc_bytes(&mut payload, value.as_bytes());
                }
                let response = client.execute_prepared(&payload).unwrap();
                let WireResponse::Err(error) = response else {
                    panic!("{document} {path}: {response:?}")
                };
                assert_ne!(error.code, 0);
                assert!(error.sql_state.is_some());
                assert!(!error.message.is_empty());
                eprintln!("JSON Execute rejection {document} {path}: {error:?}");
                let WireResponse::ResultSet(probe) = client.query("SELECT 1 AS v").unwrap() else {
                    panic!("recovery")
                };
                assert_eq!(probe.rows, vec![vec![TextValue::Bytes(b"1".to_vec())]]);
            }
        }
        client.close_prepared(prepared.statement_id).unwrap();
    }
    drop(client);
    let address = server.mysql_addr();
    server.close();
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_21() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        let mut messages = vec![];
        loop {
            let m = read(socket);
            let end = m.0 == b'Z';
            messages.push(m);
            if end {
                return messages;
            }
        }
    }
    let version_before = query(&mut socket, "SELECT version() AS v");
    assert_eq!(
        version_before.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ"
    );
    assert_ne!(
        text_row(&version_before[1].1),
        vec![Some(astersql_util_printer::GetTiDBInfo())]
    );
    let table = format!("tidb_version_pg21_{}", address.port());
    struct TableCleanup(TcpStream, String);
    impl Drop for TableCleanup {
        fn drop(&mut self) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                query(&mut self.0, "ROLLBACK");
                query(&mut self.0, &format!("DROP TABLE IF EXISTS {}", self.1));
            }));
        }
    }
    let table_cleanup = TableCleanup(socket.try_clone().unwrap(), table.clone());
    for sql in [
        format!("CREATE TABLE {table}(v integer)"),
        format!("INSERT INTO {table} VALUES(1)"),
    ] {
        assert_eq!(
            query(&mut socket, &sql)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"CZ",
            "{sql}"
        );
    }
    for prefix in ["", "pg_catalog."] {
        for call in [
            "tidb_version()",
            "tidb_version(NULL)",
            "tidb_version('bad'::int4)",
            "tidb_version($1::text)",
        ] {
            let expected_state = if call.contains("'bad'::int4") {
                "22P02"
            } else {
                "42883"
            };
            for parse in [false, true] {
                // Parameter markers belong only to the extended protocol.
                if !parse && call.contains('$') {
                    continue;
                }
                for empty in [false, true] {
                    for transaction in [false, true] {
                        if transaction {
                            assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
                        }
                        let sql = format!(
                            "SELECT {prefix}{call} AS v{}",
                            if empty { " WHERE false" } else { "" }
                        );
                        let messages = if parse {
                            send(
                                &mut socket,
                                b'P',
                                &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()]
                                    .concat(),
                            );
                            send(&mut socket, b'S', &[]);
                            ready(&mut socket)
                        } else {
                            query(&mut socket, &sql)
                        };
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            b"EZ",
                            "{sql}: {messages:?}"
                        );
                        let fields = messages[0]
                            .1
                            .split(|b| *b == 0)
                            .filter(|f| !f.is_empty())
                            .collect::<Vec<_>>();
                        let code_field = format!("C{expected_state}");
                        for field in [b"SERROR".as_slice(), b"VERROR", code_field.as_bytes()] {
                            assert!(fields.contains(&field), "{sql}: {fields:?}");
                        }
                        assert!(fields.iter().any(|f| f[0] == b'M' && f.len() > 1));
                        assert!(!fields.iter().any(|f| f[0] == b'D'), "{sql}: {fields:?}");
                        assert_eq!(
                            fields.iter().any(|f| f[0] == b'H'),
                            expected_state == "42883",
                            "{sql}: {fields:?}"
                        );
                        assert!(fields.iter().any(|f| f[0] == b'P'), "{sql}: {fields:?}");
                        assert_eq!(
                            messages.last().unwrap().1,
                            if transaction { b"E" } else { b"I" }
                        );
                        if transaction {
                            assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
                        }
                        let probe = query(&mut socket, "SELECT 1::int4");
                        assert_eq!(probe.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
                        assert_eq!(probe[1].1, row(&[Some("1")]));
                    }
                }
            }
        }
    }
    for prefix in ["", "pg_catalog."] {
        for parse in [false, true] {
            let sql = format!("INSERT INTO {table} VALUES({prefix}tidb_version('bad'::int4))");
            let messages = if parse {
                send(
                    &mut socket,
                    b'P',
                    &[b"\0".as_slice(), sql.as_bytes(), b"\0", &0i16.to_be_bytes()].concat(),
                );
                send(&mut socket, b'S', &[]);
                ready(&mut socket)
            } else {
                query(&mut socket, &sql)
            };
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"EZ",
                "{sql}: {messages:?}"
            );
            assert!(messages[0].1.windows(7).any(|f| f == b"C22P02\0"));
            let unchanged = query(&mut socket, &format!("SELECT v FROM {table}"));
            assert_eq!(unchanged.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
            assert_eq!(unchanged[1].1, row(&[Some("1")]));
            assert_eq!(unchanged.last().unwrap().1, b"I");
        }
    }
    // Name rejection must not change COALESCE's independent PG behavior.
    let coalesce = query(&mut socket, "SELECT coalesce(NULL::int4,7::int4) AS v");
    assert_eq!(coalesce.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
    assert_eq!(
        coalesce[0].1,
        [
            &1i16.to_be_bytes()[..],
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &23u32.to_be_bytes(),
            &4i16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes()
        ]
        .concat()
    );
    assert_eq!(coalesce[1].1, row(&[Some("7")]));
    assert_eq!(query(&mut socket, "SELECT version() AS v"), version_before);
    drop(table_cleanup);
    send(&mut socket, b'X', &[]);
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg_function_21_mysql() {
    use crate::mysql_compat_test_support::{
        CLIENT_DEPRECATE_EOF, ColumnDefinition, MysqlCompatServer, TextValue, WireResponse,
    };
    let expected = astersql_util_printer::GetTiDBInfo();
    assert!(!expected.is_empty());
    let server = MysqlCompatServer::start().unwrap();
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, Some("test"))
        .unwrap();
    for empty in [false, true] {
        let sql = format!(
            "SELECT TIDB_VERSION() AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let WireResponse::ResultSet(result) = client.query(&sql).unwrap() else {
            panic!("Query result")
        };
        assert_eq!(
            result.rows,
            if empty {
                vec![]
            } else {
                vec![vec![TextValue::Bytes(expected.as_bytes().to_vec())]]
            },
            "{sql}"
        );
        assert_eq!(result.warnings, 0);
        assert_eq!(result.columns.len(), 1);
        let column = &result.columns[0];
        assert_eq!(column.name, "v");
        assert_eq!(
            column.column_length,
            (expected.len() * 4) as u32,
            "{column:?}"
        );
        assert_eq!(
            result.columns,
            vec![ColumnDefinition {
                catalog: "def".into(),
                schema: String::new(),
                table: String::new(),
                org_table: String::new(),
                name: "v".into(),
                org_name: String::new(),
                character_set: 46,
                column_length: (expected.len() * 4) as u32,
                column_type: 253,
                flags: 1,
                decimals: 31,
                default_value: None,
            }]
        );
        let prepared = client.prepare(&sql).unwrap();
        assert_eq!(prepared.parameter_count, 0);
        assert_eq!(prepared.columns, result.columns);
        for _ in 0..3 {
            let mut payload = prepared.statement_id.to_le_bytes().to_vec();
            payload.push(0);
            payload.extend_from_slice(&1u32.to_le_bytes());
            let WireResponse::ResultSet(executed) = client.execute_prepared(&payload).unwrap()
            else {
                panic!("Execute result")
            };
            assert_eq!(executed.columns, prepared.columns);
            assert_eq!(executed.rows, result.rows);
            assert_eq!(executed.warnings, 0);
        }
        client.close_prepared(prepared.statement_id).unwrap();
    }
    let response = client.query("SELECT TIDB_VERSION(NULL) AS v").unwrap();
    let WireResponse::Err(error) = response else {
        panic!("argument rejection: {response:?}")
    };
    assert_eq!(error.code, 1582);
    assert_eq!(error.sql_state, Some(*b"42000"));
    assert!(error.message.contains("parameter count"), "{error:?}");
    eprintln!("TIDB_VERSION argument Query error: {error:?}");
    let error = client
        .prepare("SELECT TIDB_VERSION(NULL) AS v")
        .unwrap_err();
    assert!(
        error.contains("parameter count") || error.contains("arguments"),
        "{error}"
    );
    let response = client.query("SELECT TIDB_VERSION() AS v").unwrap();
    let WireResponse::ResultSet(result) = response else {
        panic!("recovery")
    };
    assert_eq!(
        result.rows,
        vec![vec![TextValue::Bytes(expected.into_bytes())]]
    );
    drop(client);
    let address = server.mysql_addr();
    server.close();
    drop(server);
    assert!(TcpStream::connect(address).is_err());
}

#[test]
fn pg08_01_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-day",
            "SELECT date_trunc('day'::text, '2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("2024-01-02 00:00:00"),
            false,
        ),
        (
            "S-week",
            "SELECT date_trunc('week'::text, '2021-01-01 12:34:56'::timestamp) AS v",
            Some("2020-12-28 00:00:00"),
            false,
        ),
        (
            "S-milliseconds",
            "SELECT date_trunc('milliseconds'::text, '2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("2024-01-02 00:00:40.123"),
            false,
        ),
        (
            "S-null",
            "SELECT date_trunc('day'::text, NULL::timestamp) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT date_trunc('day'::text, NULL::timestamp) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT date_trunc('day'::text, date_trunc('hour'::text, '2024-01-02 12:34:56'::timestamp)) AS v",
            Some("2024-01-02 00:00:00"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&1114u32.to_be_bytes());
        expected.extend_from_slice(&8i16.to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("timezone", "0A000")] {
        let messages = query(
            &mut socket,
            &format!("SELECT date_trunc('{unit}'::text, '2024-01-02 12:34:56'::timestamp) AS v"),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT date_trunc($2::text,$1::timestamp) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1114u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &1114u32.to_be_bytes());
        assert_eq!(&description[14..16], &8i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-01 00:00:40.123456".to_vec())
                    } else {
                        Some(40_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    for payload in [value.as_deref(), Some(b"milliseconds".as_slice())] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some("2000-01-01 00:00:40.123")])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &8i32.to_be_bytes(),
                                &40_123_000i64.to_be_bytes(),
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("timezone", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = "SELECT date_trunc($2::text,$1::timestamp) AS v";
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'S', &[]);
        let parsed = ready(&mut socket);
        assert!(!parsed.iter().any(|m| m.0 == b'E'));
        let mut bind = vec![0, 0, 0, 0, 0, 2];
        for value in ["2000-01-01 00:00:40", unit] {
            bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
            bind.extend_from_slice(value.as_bytes());
        }
        bind.extend_from_slice(&0i16.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_01_sql: 36 scenarios passed; listener closed");
}

#[test]
fn pg08_02_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-day",
            "SELECT date_trunc('day'::text, '2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("2024-01-02 00:00:00+00"),
            false,
        ),
        (
            "S-week",
            "SELECT date_trunc('week'::text, '2021-01-01 12:34:56'::timestamptz) AS v",
            Some("2020-12-28 00:00:00+00"),
            false,
        ),
        (
            "S-milliseconds",
            "SELECT date_trunc('milliseconds'::text, '2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("2024-01-02 00:00:40.123+00"),
            false,
        ),
        (
            "S-null",
            "SELECT date_trunc('day'::text, NULL::timestamptz) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT date_trunc('day'::text, NULL::timestamptz) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT date_trunc('day'::text, date_trunc('hour'::text, '2024-01-02 12:34:56'::timestamptz)) AS v",
            Some("2024-01-02 00:00:00+00"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&1184u32.to_be_bytes());
        expected.extend_from_slice(&8i16.to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("timezone", "0A000")] {
        let messages = query(
            &mut socket,
            &format!("SELECT date_trunc('{unit}'::text, '2024-01-02 12:34:56'::timestamptz) AS v"),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT date_trunc($2::text,$1::timestamptz) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1184u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &1184u32.to_be_bytes());
        assert_eq!(&description[14..16], &8i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-01 00:00:40.123456".to_vec())
                    } else {
                        Some(40_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    for payload in [value.as_deref(), Some(b"milliseconds".as_slice())] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some("2000-01-01 00:00:40.123+00")])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &8i32.to_be_bytes(),
                                &40_123_000i64.to_be_bytes(),
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("timezone", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = "SELECT date_trunc($2::text,$1::timestamptz) AS v";
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'S', &[]);
        let parsed = ready(&mut socket);
        assert!(!parsed.iter().any(|m| m.0 == b'E'));
        let mut bind = vec![0, 0, 0, 0, 0, 2];
        for value in ["2000-01-01 00:00:40", unit] {
            bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
            bind.extend_from_slice(value.as_bytes());
        }
        bind.extend_from_slice(&0i16.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    for (zone, source, expected) in [
        ("UTC", "2024-01-02 01:30:00+00", "2024-01-02 00:00:00+00"),
        (
            "America/New_York",
            "2024-01-02 01:30:00+00",
            "2024-01-01 00:00:00-05",
        ),
        (
            "America/New_York",
            "2024-03-10 16:00:00+00",
            "2024-03-10 00:00:00-05",
        ),
        (
            "America/New_York",
            "2024-03-11 16:00:00+00",
            "2024-03-11 00:00:00-04",
        ),
        (
            "America/New_York",
            "2024-11-03 16:00:00+00",
            "2024-11-03 00:00:00-04",
        ),
        (
            "America/New_York",
            "2024-11-04 16:00:00+00",
            "2024-11-04 00:00:00-05",
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(
            &mut socket,
            &format!("SELECT date_trunc('day'::text,'{source}'::timestamptz) AS v"),
        );
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
        assert_eq!(
            messages.iter().find(|m| m.0 == b'D').unwrap().1,
            row(&[Some(expected)])
        );
        assert_eq!(
            &messages.iter().find(|m| m.0 == b'T').unwrap().1[10..14],
            &1184u32.to_be_bytes()
        );
    }
    assert!(
        !query(&mut socket, "SET time_zone = 'UTC'")
            .iter()
            .any(|m| m.0 == b'E')
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_02_sql: 42 scenarios passed; listener closed");
}

#[test]
fn pg08_03_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-day",
            "SELECT date_trunc('day'::text, '2024-01-02 00:00:40.123456'::timestamptz,'UTC'::text) AS v",
            Some("2024-01-02 00:00:00+00"),
            false,
        ),
        (
            "S-week",
            "SELECT date_trunc('week'::text, '2021-01-01 12:34:56'::timestamptz,'UTC'::text) AS v",
            Some("2020-12-28 00:00:00+00"),
            false,
        ),
        (
            "S-milliseconds",
            "SELECT date_trunc('milliseconds'::text, '2024-01-02 00:00:40.123456'::timestamptz,'UTC'::text) AS v",
            Some("2024-01-02 00:00:40.123+00"),
            false,
        ),
        (
            "S-null",
            "SELECT date_trunc('day'::text, NULL::timestamptz,'UTC'::text) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT date_trunc('day'::text, NULL::timestamptz,'UTC'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT date_trunc('day'::text, date_trunc('hour'::text, '2024-01-02 12:34:56'::timestamptz,'UTC'::text),'UTC'::text) AS v",
            Some("2024-01-02 00:00:00+00"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&1184u32.to_be_bytes());
        expected.extend_from_slice(&8i16.to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("timezone", "0A000")] {
        let messages = query(
            &mut socket,
            &format!(
                "SELECT date_trunc('{unit}'::text, '2024-01-02 12:34:56'::timestamptz,'UTC'::text) AS v"
            ),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT date_trunc($2::text,$1::timestamptz,$3::text) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &3i16.to_be_bytes()[..],
                &1184u32.to_be_bytes(),
                &25u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &1184u32.to_be_bytes());
        assert_eq!(&description[14..16], &8i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..4 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-01 00:00:40.123456".to_vec())
                    } else {
                        Some(40_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&3i16.to_be_bytes());
                    for payload in [
                        value.as_deref(),
                        Some(b"milliseconds".as_slice()),
                        if index == 3 {
                            None
                        } else {
                            Some(b"UTC".as_slice())
                        },
                    ] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 || index == 3 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some("2000-01-01 00:00:40.123+00")])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &8i32.to_be_bytes(),
                                &40_123_000i64.to_be_bytes(),
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("timezone", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = "SELECT date_trunc($2::text,$1::timestamptz,$3::text) AS v";
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'S', &[]);
        let parsed = ready(&mut socket);
        assert!(!parsed.iter().any(|m| m.0 == b'E'));
        let mut bind = vec![0, 0, 0, 0, 0, 3];
        for value in ["2000-01-01 00:00:40", unit, "UTC"] {
            bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
            bind.extend_from_slice(value.as_bytes());
        }
        bind.extend_from_slice(&0i16.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    for (zone, source, expected) in [
        ("UTC", "2024-01-02 01:30:00+00", "2024-01-02 00:00:00+00"),
        (
            "America/New_York",
            "2024-01-02 01:30:00+00",
            "2024-01-01 00:00:00-05",
        ),
        (
            "America/New_York",
            "2024-03-10 16:00:00+00",
            "2024-03-10 00:00:00-05",
        ),
        (
            "America/New_York",
            "2024-03-11 16:00:00+00",
            "2024-03-11 00:00:00-04",
        ),
        (
            "America/New_York",
            "2024-11-03 16:00:00+00",
            "2024-11-03 00:00:00-04",
        ),
        (
            "America/New_York",
            "2024-11-04 16:00:00+00",
            "2024-11-04 00:00:00-05",
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(
            &mut socket,
            &format!("SELECT date_trunc('day'::text,'{source}'::timestamptz,'{zone}'::text) AS v"),
        );
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
        assert_eq!(
            messages.iter().find(|m| m.0 == b'D').unwrap().1,
            row(&[Some(expected)])
        );
        assert_eq!(
            &messages.iter().find(|m| m.0 == b'T').unwrap().1[10..14],
            &1184u32.to_be_bytes()
        );
    }
    assert!(
        !query(&mut socket, "SET time_zone = 'UTC'")
            .iter()
            .any(|m| m.0 == b'E')
    );
    // S-inferred-zone: an OID0 zone parameter is text before any Bind value.
    for sql in [
        "SELECT date_trunc($2::text,$1::timestamptz,$3) AS v",
        "SELECT date_trunc($2,$1,$3) AS v",
    ] {
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &3i16.to_be_bytes()[..],
                &1184u32.to_be_bytes(),
                &25u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        assert_eq!(
            &messages.iter().find(|m| m.0 == b'T').unwrap().1[10..14],
            &1184u32.to_be_bytes()
        );
        let mut bind = vec![0, 0, 0, 0, 0, 3];
        for value in ["2024-01-02 01:30:00+00", "day", "America/New_York"] {
            bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
            bind.extend_from_slice(value.as_bytes());
        }
        bind.extend_from_slice(&0i16.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let executed = ready(&mut socket);
        assert_eq!(
            executed.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'2', b'D', b'C', b'Z'],
            "{executed:?}"
        );
        assert_eq!(
            executed.iter().find(|m| m.0 == b'D').unwrap().1,
            row(&[Some("2024-01-01 05:00:00+00")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_03_sql: 52 scenarios passed; listener closed");
}

#[test]
fn pg08_04_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-hour",
            "SELECT date_trunc('hour'::text, '2 years 3 months 4 days 05:06:07.123456'::interval) AS v",
            Some("2 years 3 mons 4 days 05:00:00"),
            false,
        ),
        (
            "S-negative",
            "SELECT date_trunc('hour'::text, '-1 day -02:03:04'::interval) AS v",
            Some("-1 days -02:00:00"),
            false,
        ),
        (
            "S-milliseconds",
            "SELECT date_trunc('milliseconds'::text, '2 years 3 months 4 days 05:06:07.123456'::interval) AS v",
            Some("2 years 3 mons 4 days 05:06:07.123"),
            false,
        ),
        (
            "S-null-unit",
            "SELECT date_trunc(NULL::text, '1 day'::interval) AS v",
            None,
            false,
        ),
        (
            "S-null",
            "SELECT date_trunc('day'::text, NULL::interval) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT date_trunc('day'::text, NULL::interval) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT date_trunc('day'::text, date_trunc('hour'::text, '2 years 3 months 4 days 05:06:07.123456'::interval)) AS v",
            Some("2 years 3 mons 4 days"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&1186u32.to_be_bytes());
        expected.extend_from_slice(&16i16.to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("week", "0A000")] {
        let messages = query(
            &mut socket,
            &format!(
                "SELECT date_trunc('{unit}'::text, '2 years 3 months 4 days 05:06:07.123456'::interval) AS v"
            ),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT date_trunc($2::text,$1::interval) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1186u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &1186u32.to_be_bytes());
        assert_eq!(&description[14..16], &16i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..4 {
                    // Interval binary layout is elapsed microseconds, days, then months.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2 years 3 mons 4 days 00:00:40.123456".to_vec())
                    } else {
                        Some(
                            [
                                &40_123_456i64.to_be_bytes()[..],
                                &4i32.to_be_bytes(),
                                &27i32.to_be_bytes(),
                            ]
                            .concat(),
                        )
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    for payload in [
                        value.as_deref(),
                        if index == 3 {
                            None
                        } else {
                            Some(b"milliseconds".as_slice())
                        },
                    ] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 || index == 3 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some("2 years 3 mons 4 days 00:00:40.123")])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &16i32.to_be_bytes(),
                                &40_123_000i64.to_be_bytes(),
                                &4i32.to_be_bytes(),
                                &27i32.to_be_bytes(),
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("nonsense", "22023"), ("week", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = "SELECT date_trunc($2::text,$1::interval) AS v";
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'S', &[]);
        let parsed = ready(&mut socket);
        assert!(!parsed.iter().any(|m| m.0 == b'E'));
        let mut bind = vec![0, 0, 0, 0, 0, 2];
        for value in ["1 day", unit] {
            bind.extend_from_slice(&(value.len() as i32).to_be_bytes());
            bind.extend_from_slice(value.as_bytes());
        }
        bind.extend_from_slice(&0i16.to_be_bytes());
        send(&mut socket, b'B', &bind);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        if unit == "week" {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part == b"DMonths usually have fractional weeks.")
            );
        }
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_04_sql: 45 scenarios passed; listener closed");
}

#[test]
fn pg08_05_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-second",
            "SELECT EXTRACT(second FROM '2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("40.123456"),
            false,
        ),
        (
            "S-microseconds",
            "SELECT EXTRACT(microseconds FROM '2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("40123456"),
            false,
        ),
        (
            "S-year",
            "SELECT EXTRACT(year FROM '2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("2024"),
            false,
        ),
        (
            "S-month",
            "SELECT EXTRACT(month FROM '2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("1"),
            false,
        ),
        (
            "S-day",
            "SELECT EXTRACT(day FROM '2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("2"),
            false,
        ),
        (
            "S-week",
            "SELECT EXTRACT(week FROM '2021-01-01 12:34:56'::timestamp) AS v",
            Some("53"),
            false,
        ),
        (
            "S-isoyear",
            "SELECT EXTRACT(isoyear FROM '2021-01-01 12:34:56'::timestamp) AS v",
            Some("2020"),
            false,
        ),
        (
            "S-null",
            "SELECT EXTRACT(second FROM NULL::timestamp) AS v",
            None,
            false,
        ),
        (
            "S-infinity-year",
            "SELECT EXTRACT(year FROM 'infinity'::timestamp) AS v",
            Some("Infinity"),
            false,
        ),
        (
            "S-negative-infinity-year",
            "SELECT EXTRACT(year FROM '-infinity'::timestamp) AS v",
            Some("-Infinity"),
            false,
        ),
        (
            "S-infinity-second",
            "SELECT EXTRACT(second FROM 'infinity'::timestamp) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT EXTRACT(second FROM NULL::timestamp) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT EXTRACT(second FROM date_trunc('milliseconds'::text,'2024-01-02 00:00:40.123456'::timestamp)) AS v",
            Some("40.123000"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&1700u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        let messages = query(
            &mut socket,
            &format!("SELECT EXTRACT({unit} FROM '2024-01-02 12:34:56'::timestamp) AS v"),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT EXTRACT(second FROM $1::timestamp) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [&1i16.to_be_bytes()[..], &1114u32.to_be_bytes(),].concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &1700u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-01 00:00:40.123456".to_vec())
                    } else {
                        Some(40_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    for payload in [value.as_deref()] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some("40.123456")])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &14i32.to_be_bytes(),
                                &[0, 3, 0, 0, 0, 0, 0, 6, 0, 40, 4, 210, 21, 224],
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql =
            format!("SELECT EXTRACT({unit} FROM '2024-01-02 00:00:40.123456'::timestamp) AS v");
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_05_sql: 43 scenarios passed; listener closed");
}

#[test]
fn pg08_06_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-second",
            "SELECT EXTRACT(second FROM '2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("40.123456"),
            false,
        ),
        (
            "S-microseconds",
            "SELECT EXTRACT(microseconds FROM '2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("40123456"),
            false,
        ),
        (
            "S-year",
            "SELECT EXTRACT(year FROM '2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("2024"),
            false,
        ),
        (
            "S-month",
            "SELECT EXTRACT(month FROM '2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("1"),
            false,
        ),
        (
            "S-day",
            "SELECT EXTRACT(day FROM '2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("2"),
            false,
        ),
        (
            "S-week",
            "SELECT EXTRACT(week FROM '2021-01-01 12:34:56'::timestamptz) AS v",
            Some("53"),
            false,
        ),
        (
            "S-isoyear",
            "SELECT EXTRACT(isoyear FROM '2021-01-01 12:34:56'::timestamptz) AS v",
            Some("2020"),
            false,
        ),
        (
            "S-null",
            "SELECT EXTRACT(second FROM NULL::timestamptz) AS v",
            None,
            false,
        ),
        (
            "S-infinity-year",
            "SELECT EXTRACT(year FROM 'infinity'::timestamptz) AS v",
            Some("Infinity"),
            false,
        ),
        (
            "S-negative-infinity-year",
            "SELECT EXTRACT(year FROM '-infinity'::timestamptz) AS v",
            Some("-Infinity"),
            false,
        ),
        (
            "S-infinity-second",
            "SELECT EXTRACT(second FROM 'infinity'::timestamptz) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT EXTRACT(second FROM NULL::timestamptz) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT EXTRACT(second FROM date_trunc('milliseconds'::text,'2024-01-02 00:00:40.123456'::timestamptz)) AS v",
            Some("40.123000"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&1700u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023")] {
        let messages = query(
            &mut socket,
            &format!("SELECT EXTRACT({unit} FROM '2024-01-02 12:34:56'::timestamptz) AS v"),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT EXTRACT(second FROM $1::timestamptz) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [&1i16.to_be_bytes()[..], &1184u32.to_be_bytes(),].concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &1700u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-01 00:00:40.123456".to_vec())
                    } else {
                        Some(40_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    for payload in [value.as_deref()] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some("40.123456")])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &14i32.to_be_bytes(),
                                &[0, 3, 0, 0, 0, 0, 0, 6, 0, 40, 4, 210, 21, 224],
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql =
            format!("SELECT EXTRACT({unit} FROM '2024-01-02 00:00:40.123456'::timestamptz) AS v");
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_06_sql: 40 scenarios passed; listener closed");
}

#[test]
fn pg08_07_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-year",
            "SELECT EXTRACT(year FROM '27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("2"),
            false,
        ),
        (
            "S-month",
            "SELECT EXTRACT(month FROM '27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("3"),
            false,
        ),
        (
            "S-day",
            "SELECT EXTRACT(day FROM '27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("4"),
            false,
        ),
        (
            "S-hour",
            "SELECT EXTRACT(hour FROM '27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("5"),
            false,
        ),
        (
            "S-second",
            "SELECT EXTRACT(second FROM '27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("7.123456"),
            false,
        ),
        (
            "S-minute",
            "SELECT EXTRACT(minute FROM '80 minutes'::interval) AS v",
            Some("20"),
            false,
        ),
        (
            "S-negative-year",
            "SELECT EXTRACT(year FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-2"),
            false,
        ),
        (
            "S-negative-month",
            "SELECT EXTRACT(month FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-3"),
            false,
        ),
        (
            "S-negative-day",
            "SELECT EXTRACT(day FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-4"),
            false,
        ),
        (
            "S-negative-hour",
            "SELECT EXTRACT(hour FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-5"),
            false,
        ),
        (
            "S-negative-second",
            "SELECT EXTRACT(second FROM '-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-7.123456"),
            false,
        ),
        (
            "S-negative-fraction",
            "SELECT EXTRACT(second FROM '-0.123456 seconds'::interval) AS v",
            Some("-0.123456"),
            false,
        ),
        (
            "S-long-hour",
            "SELECT EXTRACT(hour FROM '2 days 27 hours'::interval) AS v",
            Some("27"),
            false,
        ),
        (
            "S-microseconds",
            "SELECT EXTRACT(microseconds FROM '27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("7123456"),
            false,
        ),
        (
            "S-milliseconds",
            "SELECT EXTRACT(milliseconds FROM '27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("7123.456"),
            false,
        ),
        (
            "S-null",
            "SELECT EXTRACT(second FROM NULL::interval) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT EXTRACT(second FROM NULL::interval) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT EXTRACT(second FROM date_trunc('milliseconds'::text,'27 months 4 days 05:06:07.123456'::interval)) AS v",
            Some("7.123000"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&1700u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        let messages = query(
            &mut socket,
            &format!(
                "SELECT EXTRACT({unit} FROM '27 months 4 days 05:06:07.123456'::interval) AS v"
            ),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT EXTRACT(second FROM $1::interval) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [&1i16.to_be_bytes()[..], &1186u32.to_be_bytes(),].concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &1700u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"40.123456 seconds".to_vec())
                    } else {
                        Some(
                            [
                                &40_123_456i64.to_be_bytes()[..],
                                &0i32.to_be_bytes(),
                                &0i32.to_be_bytes(),
                            ]
                            .concat(),
                        )
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    for payload in [value.as_deref()] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some("40.123456")])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &14i32.to_be_bytes(),
                                &[0, 3, 0, 0, 0, 0, 0, 6, 0, 40, 4, 210, 21, 224],
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = format!(
            "SELECT EXTRACT({unit} FROM '27 months 4 days 05:06:07.123456'::interval) AS v"
        );
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_07_sql: 46 scenarios passed; listener closed");
}

#[test]
fn pg08_08_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-second",
            "SELECT date_part('second'::text,'2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("40.123456"),
            false,
        ),
        (
            "S-microseconds",
            "SELECT date_part('microseconds'::text,'2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("40123456"),
            false,
        ),
        (
            "S-year",
            "SELECT date_part('year'::text,'2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("2024"),
            false,
        ),
        (
            "S-month",
            "SELECT date_part('month'::text,'2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("1"),
            false,
        ),
        (
            "S-day",
            "SELECT date_part('day'::text,'2024-01-02 00:00:40.123456'::timestamp) AS v",
            Some("2"),
            false,
        ),
        (
            "S-week",
            "SELECT date_part('week'::text,'2021-01-01 12:34:56'::timestamp) AS v",
            Some("53"),
            false,
        ),
        (
            "S-isoyear",
            "SELECT date_part('isoyear'::text,'2021-01-01 12:34:56'::timestamp) AS v",
            Some("2020"),
            false,
        ),
        (
            "S-null",
            "SELECT date_part('second'::text,NULL::timestamp) AS v",
            None,
            false,
        ),
        (
            "S-infinity-year",
            "SELECT date_part('year'::text,'infinity'::timestamp) AS v",
            Some("Infinity"),
            false,
        ),
        (
            "S-negative-infinity-year",
            "SELECT date_part('year'::text,'-infinity'::timestamp) AS v",
            Some("-Infinity"),
            false,
        ),
        (
            "S-infinity-second",
            "SELECT date_part('second'::text,'infinity'::timestamp) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT date_part('second'::text,NULL::timestamp) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT date_part('second'::text,date_trunc('milliseconds'::text,'2024-01-02 00:00:40.123456'::timestamp)) AS v",
            Some("40.123"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&701u32.to_be_bytes());
        expected.extend_from_slice(&8i16.to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        let messages = query(
            &mut socket,
            &format!("SELECT date_part('{unit}'::text,'2024-01-02 12:34:56'::timestamp) AS v"),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT date_part($2::text,$1::timestamp) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1114u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &701u32.to_be_bytes());
        assert_eq!(&description[14..16], &8i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-01 00:00:40.123456".to_vec())
                    } else {
                        Some(40_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 2 {
                        b"microseconds"
                    } else {
                        b"second"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some(if index == 2 { "40123456" } else { "40.123456" })])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &8i32.to_be_bytes(),
                                &(if index == 2 {
                                    40123456.0f64
                                } else {
                                    40.123456f64
                                })
                                .to_be_bytes(),
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = format!(
            "SELECT date_part('{unit}'::text,'2024-01-02 00:00:40.123456'::timestamp) AS v"
        );
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_08_sql: 43 scenarios passed; listener closed");
}

#[test]
fn pg08_09_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-second",
            "SELECT date_part('second'::text,'2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("40.123456"),
            false,
        ),
        (
            "S-microseconds",
            "SELECT date_part('microseconds'::text,'2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("40123456"),
            false,
        ),
        (
            "S-year",
            "SELECT date_part('year'::text,'2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("2024"),
            false,
        ),
        (
            "S-month",
            "SELECT date_part('month'::text,'2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("1"),
            false,
        ),
        (
            "S-day",
            "SELECT date_part('day'::text,'2024-01-02 00:00:40.123456'::timestamptz) AS v",
            Some("2"),
            false,
        ),
        (
            "S-week",
            "SELECT date_part('week'::text,'2021-01-01 12:34:56'::timestamptz) AS v",
            Some("53"),
            false,
        ),
        (
            "S-isoyear",
            "SELECT date_part('isoyear'::text,'2021-01-01 12:34:56'::timestamptz) AS v",
            Some("2020"),
            false,
        ),
        (
            "S-null",
            "SELECT date_part('second'::text,NULL::timestamptz) AS v",
            None,
            false,
        ),
        (
            "S-infinity-year",
            "SELECT date_part('year'::text,'infinity'::timestamptz) AS v",
            Some("Infinity"),
            false,
        ),
        (
            "S-negative-infinity-year",
            "SELECT date_part('year'::text,'-infinity'::timestamptz) AS v",
            Some("-Infinity"),
            false,
        ),
        (
            "S-infinity-second",
            "SELECT date_part('second'::text,'infinity'::timestamptz) AS v",
            None,
            false,
        ),
        (
            "S-zero",
            "SELECT date_part('second'::text,NULL::timestamptz) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-nested",
            "SELECT date_part('second'::text,date_trunc('milliseconds'::text,'2024-01-02 00:00:40.123456'::timestamptz)) AS v",
            Some("40.123"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&701u32.to_be_bytes());
        expected.extend_from_slice(&8i16.to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023")] {
        let messages = query(
            &mut socket,
            &format!("SELECT date_part('{unit}'::text,'2024-01-02 12:34:56'::timestamptz) AS v"),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT date_part($2::text,$1::timestamptz) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1184u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &701u32.to_be_bytes());
        assert_eq!(&description[14..16], &8i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // 2000-01-01 is PostgreSQL's binary timestamp epoch.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-01 00:00:40.123456".to_vec())
                    } else {
                        Some(40_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 2 {
                        b"microseconds"
                    } else {
                        b"second"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some(if index == 2 { "40123456" } else { "40.123456" })])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &8i32.to_be_bytes(),
                                &(if index == 2 {
                                    40123456.0f64
                                } else {
                                    40.123456f64
                                })
                                .to_be_bytes(),
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = format!(
            "SELECT date_part('{unit}'::text,'2024-01-02 00:00:40.123456'::timestamptz) AS v"
        );
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_09_sql: 40 scenarios passed; listener closed");
}
#[test]
fn pg08_10_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-Q-year",
            "SELECT date_part('year'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("2"),
            false,
        ),
        (
            "S-Q-month",
            "SELECT date_part('month'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("3"),
            false,
        ),
        (
            "S-Q-day",
            "SELECT date_part('day'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("4"),
            false,
        ),
        (
            "S-Q-hour",
            "SELECT date_part('hour'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("5"),
            false,
        ),
        (
            "S-Q-second",
            "SELECT date_part('second'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("7.123456"),
            false,
        ),
        (
            "S-Q-minute",
            "SELECT date_part('minute'::text,'80 minutes'::interval) AS v",
            Some("20"),
            false,
        ),
        (
            "S-Q-negative-year",
            "SELECT date_part('year'::text,'-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-2"),
            false,
        ),
        (
            "S-Q-negative-month",
            "SELECT date_part('month'::text,'-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-3"),
            false,
        ),
        (
            "S-Q-negative-day",
            "SELECT date_part('day'::text,'-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-4"),
            false,
        ),
        (
            "S-Q-negative-hour",
            "SELECT date_part('hour'::text,'-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-5"),
            false,
        ),
        (
            "S-Q-negative-second",
            "SELECT date_part('second'::text,'-27 months -4 days -05:06:07.123456'::interval) AS v",
            Some("-7.123456"),
            false,
        ),
        (
            "S-Q-negative-fraction",
            "SELECT date_part('second'::text,'-0.123456 seconds'::interval) AS v",
            Some("-0.123456"),
            false,
        ),
        (
            "S-Q-long-hour",
            "SELECT date_part('hour'::text,'2 days 27 hours'::interval) AS v",
            Some("27"),
            false,
        ),
        (
            "S-Q-microseconds",
            "SELECT date_part('microseconds'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("7123456"),
            false,
        ),
        (
            "S-Q-milliseconds",
            "SELECT date_part('milliseconds'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("7123.456"),
            false,
        ),
        (
            "S-Q-null",
            "SELECT date_part('second'::text,NULL::interval) AS v",
            None,
            false,
        ),
        (
            "S-Q-zero",
            "SELECT date_part('second'::text,NULL::interval) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-Q-nested",
            "SELECT date_part('second'::text,date_trunc('milliseconds'::text,'27 months 4 days 05:06:07.123456'::interval)) AS v",
            Some("7.123"),
            false,
        ),
        (
            "S-Q-negative-milliseconds",
            "SELECT date_part('milliseconds'::text,'-0.123456 seconds'::interval) AS v",
            Some("-123.456"),
            false,
        ),
        (
            "S-Q-negative-minute",
            "SELECT date_part('minute'::text,'-80 minutes'::interval) AS v",
            Some("-20"),
            false,
        ),
        (
            "S-Q-epoch",
            "SELECT date_part('epoch'::text,'27 months 4 days 05:06:07.123456'::interval) AS v",
            Some("71255167.123456"),
            false,
        ),
        (
            "S-Q-week",
            "SELECT date_part('week'::text,'-15 days'::interval) AS v",
            Some("-2"),
            false,
        ),
        (
            "S-Q-quarter",
            "SELECT date_part('quarter'::text,'-27 months'::interval) AS v",
            Some("-2"),
            false,
        ),
        (
            "S-Q-quarter-year",
            "SELECT date_part('quarter'::text,'-24 months'::interval) AS v",
            Some("-1"),
            false,
        ),
        (
            "S-Q-quarter-zero",
            "SELECT date_part('quarter'::text,'0'::interval) AS v",
            Some("1"),
            false,
        ),
        (
            "S-Q-decade",
            "SELECT date_part('decade'::text,'24001 months'::interval) AS v",
            Some("200"),
            false,
        ),
        (
            "S-Q-century",
            "SELECT date_part('century'::text,'24001 months'::interval) AS v",
            Some("20"),
            false,
        ),
        (
            "S-Q-millennium",
            "SELECT date_part('millennium'::text,'24001 months'::interval) AS v",
            Some("2"),
            false,
        ),
        (
            "S-Q-infinity-hour",
            "SELECT date_part('hour'::text,'infinity'::interval) AS v",
            Some("Infinity"),
            false,
        ),
        (
            "S-Q-negative-infinity-day",
            "SELECT date_part('day'::text,'-infinity'::interval) AS v",
            Some("-Infinity"),
            false,
        ),
        (
            "S-Q-infinity-month",
            "SELECT date_part('month'::text,'infinity'::interval) AS v",
            None,
            false,
        ),
        (
            "S-Q-infinity-week",
            "SELECT date_part('week'::text,'infinity'::interval) AS v",
            None,
            false,
        ),
        (
            "S-Q-rounding",
            "SELECT date_part('second'::text,'1.003691 seconds'::interval) AS v",
            Some("1.0036909999999999"),
            false,
        ),
        (
            "S-Q-negative-rounding",
            "SELECT date_part('second'::text,'-1.003691 seconds'::interval) AS v",
            Some("-1.0036909999999999"),
            false,
        ),
        (
            "S-Q-null-unit",
            "SELECT date_part(NULL::text,'80 minutes'::interval) AS v",
            None,
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&701u32.to_be_bytes());
        expected.extend_from_slice(&8i16.to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        let messages = query(
            &mut socket,
            &format!(
                "SELECT date_part('{unit}'::text,'27 months 4 days 05:06:07.123456'::interval) AS v"
            ),
        );
        let error = &messages
            .iter()
            .find(|(tag, _)| *tag == b'E')
            .expect("error response")
            .1;
        assert!(
            error
                .windows(7)
                .any(|bytes| bytes == format!("C{state}\0").as_bytes()),
            "{unit}: {messages:?}"
        );
        assert!(!messages.iter().any(|(tag, _)| *tag == b'D'));
        let recovered = query(&mut socket, "SELECT 1::int4 AS v");
        assert_eq!(
            recovered.iter().find(|(tag, _)| *tag == b'D').unwrap().1,
            row(&[Some("1")])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT date_part($2::text,$1::interval) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1186u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &701u32.to_be_bytes());
        assert_eq!(&description[14..16], &8i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    // Binary interval stores microseconds, days and months independently.
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"40.123456 seconds".to_vec())
                    } else {
                        Some(
                            [
                                40_123_456i64.to_be_bytes().as_slice(),
                                &0i32.to_be_bytes(),
                                &0i32.to_be_bytes(),
                            ]
                            .concat(),
                        )
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 2 {
                        b"microseconds"
                    } else {
                        b"second"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else if output_format == 0 {
                            row(&[Some(if index == 2 { "40123456" } else { "40.123456" })])
                        } else {
                            [
                                &1i16.to_be_bytes()[..],
                                &8i32.to_be_bytes(),
                                &(if index == 2 {
                                    40123456.0f64
                                } else {
                                    40.123456f64
                                })
                                .to_be_bytes(),
                            ]
                            .concat()
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    for (unit, state) in [("aster_bad_unit", "22023"), ("timezone", "0A000")] {
        assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
        let sql = format!(
            "SELECT date_part('{unit}'::text,'27 months 4 days 05:06:07.123456'::interval) AS v"
        );
        send(
            &mut socket,
            b'P',
            &[b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'B', &[0, 0, 0, 0, 0, 0, 0, 0]);
        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        let error = &messages
            .iter()
            .find(|m| m.0 == b'E')
            .expect("runtime error")
            .1;
        assert!(
            error
                .windows(7)
                .any(|b| b == format!("C{state}\0").as_bytes()),
            "{messages:?}"
        );
        for field in [b'S', b'V', b'C', b'M'] {
            assert!(
                error
                    .split(|b| *b == 0)
                    .any(|part| part.first() == Some(&field) && part.len() > 1)
            );
        }
        let phase = if messages.iter().any(|m| m.0 == b'2') {
            "Execute"
        } else {
            "Bind"
        };
        println!("S-transaction-{unit}: phase={phase} error bytes={error:?}");
        assert!(!messages.iter().any(|m| m.0 == b'D'));
        assert_eq!(messages.last().unwrap().1, b"E");
        assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
        assert_eq!(
            query(&mut socket, "SELECT 1::int4 AS v")
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("1")])
        );
    }
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_10_sql: 65 scenarios passed; listener closed");
}

#[test]
fn pg08_11_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, value, zero) in [
        (
            "S-Q-microseconds",
            "SELECT to_char('2024-01-02 03:04:05.123456'::timestamp,'YYYY-MM-DD HH24:MI:SS.US'::text) AS v",
            Some("2024-01-02 03:04:05.123456"),
            false,
        ),
        (
            "S-Q-iso",
            "SELECT to_char('2021-01-01'::timestamp,'IYYY-IW'::text) AS v",
            Some("2020-53"),
            false,
        ),
        (
            "S-Q-modifiers",
            "SELECT to_char('2024-01-02'::timestamp,'\"Year=\"YYYY FMDD DDTH'::text) AS v",
            Some("Year=2024 2 02ND"),
            false,
        ),
        (
            "S-Q-percent",
            "SELECT to_char('2024-01-02'::timestamp,'%Y'::text) AS v",
            Some("%4"),
            false,
        ),
        (
            "S-Q-quoted",
            "SELECT to_char('2024-01-02'::timestamp,'\"YYYY\"'::text) AS v",
            Some("YYYY"),
            false,
        ),
        (
            "S-Q-empty",
            "SELECT to_char('2024-01-02'::timestamp,''::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-infinity",
            "SELECT to_char('infinity'::timestamp,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-negative-infinity",
            "SELECT to_char('-infinity'::timestamp,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-null",
            "SELECT to_char(NULL::timestamp,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-null-template",
            "SELECT to_char('2024-01-02'::timestamp,NULL::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-zero",
            "SELECT to_char(NULL::timestamp,'YYYY'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-Q-nested",
            "SELECT to_char(date_trunc('day'::text,'2024-01-02 03:04:05'::timestamp),'YYYY-MM-DD HH24:MI:SS'::text) AS v",
            Some("2024-01-02 00:00:00"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT to_char($1::timestamp,$2::text) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1114u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &25u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-02 00:00:40.123456".to_vec())
                    } else {
                        Some(86_440_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 2 {
                        b"YYYY-MM-DD"
                    } else {
                        b"HH24:MI:SS.US"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else {
                            row(&[Some(if index == 2 {
                                "2000-01-02"
                            } else {
                                "00:00:40.123456"
                            })])
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_11_sql: 37 scenarios passed; listener closed");
}

#[test]
fn pg08_12_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, zone, sql, value, zero) in [
        (
            "utc",
            "UTC",
            "SELECT to_char('2024-01-02 01:30:00+00'::timestamptz,'YYYY-MM-DD HH24:MI TZH:TZM'::text) AS v",
            Some("2024-01-02 01:30 +00:00"),
            false,
        ),
        (
            "ny",
            "America/New_York",
            "SELECT to_char('2024-01-02 01:30:00+00'::timestamptz,'YYYY-MM-DD HH24:MI TZH:TZM'::text) AS v",
            Some("2024-01-01 20:30 -05:00"),
            false,
        ),
        (
            "spring-before",
            "America/New_York",
            "SELECT to_char('2024-03-10 06:30:00+00'::timestamptz,'YYYY-MM-DD HH24:MI OF'::text) AS v",
            Some("2024-03-10 01:30 -05"),
            false,
        ),
        (
            "spring-after",
            "America/New_York",
            "SELECT to_char('2024-03-10 07:30:00+00'::timestamptz,'YYYY-MM-DD HH24:MI OF'::text) AS v",
            Some("2024-03-10 03:30 -04"),
            false,
        ),
        (
            "fall-before",
            "America/New_York",
            "SELECT to_char('2024-11-03 05:30:00+00'::timestamptz,'YYYY-MM-DD HH24:MI TZH:TZM'::text) AS v",
            Some("2024-11-03 01:30 -04:00"),
            false,
        ),
        (
            "fall-after",
            "America/New_York",
            "SELECT to_char('2024-11-03 06:30:00+00'::timestamptz,'YYYY-MM-DD HH24:MI TZH:TZM'::text) AS v",
            Some("2024-11-03 01:30 -05:00"),
            false,
        ),
        (
            "kathmandu",
            "Asia/Kathmandu",
            "SELECT to_char('2024-01-02 01:30:00+00'::timestamptz,'HH24:MI OF TZH:TZM'::text) AS v",
            Some("07:15 +05:45 +05:45"),
            false,
        ),
        (
            "microseconds",
            "America/New_York",
            "SELECT to_char('2024-01-02 01:30:40.123456+00'::timestamptz,'HH24:MI:SS.US'::text) AS v",
            Some("20:30:40.123456"),
            false,
        ),
        (
            "empty",
            "UTC",
            "SELECT to_char('2024-01-02 01:30:00+00'::timestamptz,''::text) AS v",
            None,
            false,
        ),
        (
            "infinity",
            "America/New_York",
            "SELECT to_char('infinity'::timestamptz,'YYYY OF'::text) AS v",
            None,
            false,
        ),
        (
            "negative-infinity",
            "America/New_York",
            "SELECT to_char('-infinity'::timestamptz,'YYYY OF'::text) AS v",
            None,
            false,
        ),
        (
            "null",
            "UTC",
            "SELECT to_char(NULL::timestamptz,'YYYY OF'::text) AS v",
            None,
            false,
        ),
        (
            "null-template",
            "UTC",
            "SELECT to_char('2024-01-02 01:30:00+00'::timestamptz,NULL::text) AS v",
            None,
            false,
        ),
        (
            "zero",
            "UTC",
            "SELECT to_char(NULL::timestamptz,'YYYY'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "nested",
            "America/New_York",
            "SELECT to_char(date_trunc('day'::text,'2024-01-02 01:30:00+00'::timestamptz),'YYYY-MM-DD HH24:MI TZH:TZM'::text) AS v",
            Some("2024-01-01 00:00 -05:00"),
            false,
        ),
        (
            "timestamp-UTC",
            "UTC",
            "SELECT to_char('2024-01-02 01:30:00'::timestamp,'YYYY-MM-DD HH24:MI'::text) AS v",
            Some("2024-01-02 01:30"),
            false,
        ),
        (
            "timestamp-America/New_York",
            "America/New_York",
            "SELECT to_char('2024-01-02 01:30:00'::timestamp,'YYYY-MM-DD HH24:MI'::text) AS v",
            Some("2024-01-02 01:30"),
            false,
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT to_char($1::timestamptz,$2::text) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        query(&mut socket, "SET time_zone = 'UTC'");
        let parse = [b"fmt12\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"Sfmt12\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1184u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &25u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        query(&mut socket, "SET time_zone = 'America/New_York'");
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-02 00:00:40.123456+00".to_vec())
                    } else {
                        Some(86_440_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = b"\0fmt12\0".to_vec();
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 2 {
                        b"YYYY-MM-DD"
                    } else {
                        b"HH24:MI:SS.US TZH:TZM"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else {
                            row(&[Some(if index == 2 {
                                "2000-01-01"
                            } else {
                                "19:00:40.123456 -05:00"
                            })])
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
        send(&mut socket, b'C', b"Sfmt12\0");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'3', b'Z']
        );
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_12_sql: 42 scenarios passed; listener closed");
}

#[test]
fn pg08_13_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, zone, sql, value, zero) in [
        (
            "hours",
            "UTC",
            "SELECT to_char('36 hours'::interval,'HH24:MI:SS'::text) AS v",
            Some("36:00:00"),
            false,
        ),
        (
            "negative",
            "UTC",
            "SELECT to_char('-36 hours -2 minutes -3.123456 seconds'::interval,'HH24:MI:SS.US'::text) AS v",
            Some("-36:-02:-03.-123456"),
            false,
        ),
        (
            "components",
            "UTC",
            "SELECT to_char('14 months 3 days 36:02:03.123456'::interval,'YYYY-MM-DD HH24:MI:SS.US'::text) AS v",
            Some("0001-02-03 36:02:03.123456"),
            false,
        ),
        (
            "negative-months",
            "UTC",
            "SELECT to_char('-14 months 3 days -00:00:00.123456'::interval,'YYYY-MM-DD HH24:MI:SS.US'::text) AS v",
            Some("-0001--02-03 00:00:00.-123456"),
            false,
        ),
        (
            "fill",
            "UTC",
            "SELECT to_char('36:02:03.000004'::interval,'FMHH24 FMMI FMSS FMUS'::text) AS v",
            Some("36 2 3 000004"),
            false,
        ),
        (
            "literal",
            "UTC",
            "SELECT to_char('36 hours'::interval,'hello!'::text) AS v",
            Some("hello!"),
            false,
        ),
        (
            "quoted",
            "UTC",
            "SELECT to_char('36 hours'::interval,'\"HH24\" HH24'::text) AS v",
            Some("HH24 36"),
            false,
        ),
        (
            "percent",
            "UTC",
            "SELECT to_char('36 hours'::interval,'%Y'::text) AS v",
            Some("%0"),
            false,
        ),
        (
            "empty",
            "UTC",
            "SELECT to_char('36 hours'::interval,''::text) AS v",
            None,
            false,
        ),
        (
            "infinity",
            "UTC",
            "SELECT to_char('infinity'::interval,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "negative-infinity",
            "UTC",
            "SELECT to_char('-infinity'::interval,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "null",
            "UTC",
            "SELECT to_char(NULL::interval,'HH24'::text) AS v",
            None,
            false,
        ),
        (
            "null-template",
            "UTC",
            "SELECT to_char('36 hours'::interval,NULL::text) AS v",
            None,
            false,
        ),
        (
            "zero",
            "UTC",
            "SELECT to_char(NULL::interval,'HH24'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "nested",
            "UTC",
            "SELECT to_char(date_trunc('hour'::text,'36:02:03'::interval),'HH24:MI:SS'::text) AS v",
            Some("36:00:00"),
            false,
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT to_char($1::interval,$2::text) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        query(&mut socket, "SET time_zone = 'UTC'");
        let parse = [b"fmt13\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"Sfmt13\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1186u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &25u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        query(&mut socket, "SET time_zone = 'America/New_York'");
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..4 {
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"14 months 3 days 36:02:03.123456".to_vec())
                    } else {
                        Some(
                            [
                                129_723_123_456i64.to_be_bytes().as_slice(),
                                3i32.to_be_bytes().as_slice(),
                                14i32.to_be_bytes().as_slice(),
                            ]
                            .concat(),
                        )
                    };
                    let mut bind = b"\0fmt13\0".to_vec();
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 3 {
                        b"TZH"
                    } else if index == 2 {
                        b"YYYY-MM-DD"
                    } else {
                        b"HH24:MI:SS.US"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    if index == 3 && !empty {
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            [b'2', b'T', b'E', b'Z']
                        );
                        let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
                        assert!(error.windows(7).any(|b| b == b"C22007\0"));
                        assert!(
                            error.split(|b| *b == 0).any(
                                |v| v == b"HIntervals are not tied to specific calendar dates."
                            )
                        );
                        assert_eq!(messages.last().unwrap().1, b"I");
                        assert_eq!(
                            query(&mut socket, "SELECT 1::int4 AS v")
                                .iter()
                                .find(|m| m.0 == b'D')
                                .unwrap()
                                .1,
                            row(&[Some("1")])
                        );
                        continue;
                    }
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else {
                            row(&[Some(if index == 2 {
                                "0001-02-03"
                            } else {
                                "36:02:03.123456"
                            })])
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
        send(&mut socket, b'C', b"Sfmt13\0");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'3', b'Z']
        );
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_13_sql: 48 scenarios passed; listener closed");
}

#[test]
fn pg08_14_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, zone, sql, value, zero) in [
        (
            "main",
            "UTC",
            "SELECT to_char('148.5'::numeric,'FM999.990'::text) AS v",
            Some("148.500"),
            false,
        ),
        (
            "negative",
            "UTC",
            "SELECT to_char('-148.5'::numeric,'FM999.990'::text) AS v",
            Some("-148.500"),
            false,
        ),
        (
            "spaces",
            "UTC",
            "SELECT to_char('12'::numeric,'9999'::text) AS v",
            Some("   12"),
            false,
        ),
        (
            "zero-fill",
            "UTC",
            "SELECT to_char('12'::numeric,'0000'::text) AS v",
            Some(" 0012"),
            false,
        ),
        (
            "fraction",
            "UTC",
            "SELECT to_char('0.1'::numeric,'FM9.990'::text) AS v",
            Some(".100"),
            false,
        ),
        (
            "optional",
            "UTC",
            "SELECT to_char('12.3'::numeric,'FM99.999'::text) AS v",
            Some("12.3"),
            false,
        ),
        (
            "round",
            "UTC",
            "SELECT to_char('9.995'::numeric,'FM99.00'::text) AS v",
            Some("10.00"),
            false,
        ),
        (
            "negative-round",
            "UTC",
            "SELECT to_char('-9.995'::numeric,'FM99.00'::text) AS v",
            Some("-10.00"),
            false,
        ),
        (
            "overflow",
            "UTC",
            "SELECT to_char('99.995'::numeric,'99.99'::text) AS v",
            Some(" ##.##"),
            false,
        ),
        (
            "zero",
            "UTC",
            "SELECT to_char('0'::numeric,'999'::text) AS v",
            Some("   0"),
            false,
        ),
        (
            "fill-zero",
            "UTC",
            "SELECT to_char('0'::numeric,'FM999'::text) AS v",
            Some("0"),
            false,
        ),
        (
            "literal-spaces",
            "UTC",
            "SELECT to_char('12'::numeric,'  FM999  '::text) AS v",
            Some("  12  "),
            false,
        ),
        (
            "quoted",
            "UTC",
            "SELECT to_char('148.5'::numeric,'\"x\"FM999.990'::text) AS v",
            Some("x148.500"),
            false,
        ),
        (
            "empty",
            "UTC",
            "SELECT to_char('1'::numeric,''::text) AS v",
            Some(""),
            false,
        ),
        (
            "null",
            "UTC",
            "SELECT to_char(NULL::numeric,'FM999'::text) AS v",
            None,
            false,
        ),
        (
            "null-template",
            "UTC",
            "SELECT to_char('1'::numeric,NULL::text) AS v",
            None,
            false,
        ),
        (
            "exact",
            "UTC",
            "SELECT to_char('9007199254740993'::numeric,'FM9999999999999999.0'::text) AS v",
            Some("9007199254740993.0"),
            false,
        ),
        (
            "zero-row",
            "UTC",
            "SELECT to_char(NULL::numeric,'FM999'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "nested",
            "UTC",
            "SELECT to_char(round('148.5'::numeric,1::int4),'FM999.990'::text) AS v",
            Some("148.500"),
            false,
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT to_char($1::numeric,$2::text) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        query(&mut socket, "SET time_zone = 'UTC'");
        let parse = [b"fmt14\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"Sfmt14\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1700u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &25u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        query(&mut socket, "SET time_zone = 'America/New_York'");
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..4 {
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"148.5".to_vec())
                    } else {
                        Some(
                            // Independent numeric_send payload for 148.5.
                            vec![0, 2, 0, 0, 0, 0, 0, 1, 0, 148, 19, 136],
                        )
                    };
                    let mut bind = b"\0fmt14\0".to_vec();
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 3 {
                        b"99.9V99"
                    } else if index == 2 {
                        b"FM000.0"
                    } else {
                        b"FM999.990"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    if index == 3 && !empty {
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            [b'2', b'T', b'E', b'Z']
                        );
                        let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
                        assert!(error.windows(7).any(|b| b == b"C42601\0"));
                        assert_eq!(messages.last().unwrap().1, b"I");
                        assert_eq!(
                            query(&mut socket, "SELECT 1::int4 AS v")
                                .iter()
                                .find(|m| m.0 == b'D')
                                .unwrap()
                                .1,
                            row(&[Some("1")])
                        );
                        continue;
                    }
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else {
                            row(&[Some(if index == 2 { "148.5" } else { "148.500" })])
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
        send(&mut socket, b'C', b"Sfmt14\0");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'3', b'Z']
        );
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_14_sql: 52 scenarios passed; listener closed");
}

#[test]
fn pg08_15_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, zone, sql, value, zero) in [
        (
            "minus",
            "UTC",
            "SELECT to_char('-485'::int4,'999MI'::text) AS v",
            Some("485-"),
            false,
        ),
        (
            "positive-mi",
            "UTC",
            "SELECT to_char('485'::int4,'999MI'::text) AS v",
            Some("485 "),
            false,
        ),
        (
            "zero-fill",
            "UTC",
            "SELECT to_char('12'::int8,'FM000'::text) AS v",
            Some("012"),
            false,
        ),
        (
            "i4-min",
            "UTC",
            "SELECT to_char('-2147483648'::int4,'FM9999999999'::text) AS v",
            Some("-2147483648"),
            false,
        ),
        (
            "i4-max",
            "UTC",
            "SELECT to_char('2147483647'::int4,'FM9999999999'::text) AS v",
            Some("2147483647"),
            false,
        ),
        (
            "i8-min",
            "UTC",
            "SELECT to_char('-9223372036854775808'::int8,'FM9999999999999999999'::text) AS v",
            Some("-9223372036854775808"),
            false,
        ),
        (
            "i8-max",
            "UTC",
            "SELECT to_char('9223372036854775807'::int8,'FM9999999999999999999'::text) AS v",
            Some("9223372036854775807"),
            false,
        ),
        (
            "exact",
            "UTC",
            "SELECT to_char('9007199254740993'::int8,'FM9999999999999999'::text) AS v",
            Some("9007199254740993"),
            false,
        ),
        (
            "i4-null",
            "UTC",
            "SELECT to_char(NULL::int4,'999MI'::text) AS v",
            None,
            false,
        ),
        (
            "i8-null",
            "UTC",
            "SELECT to_char(NULL::int8,'FM000'::text) AS v",
            None,
            false,
        ),
        (
            "i4-null-template",
            "UTC",
            "SELECT to_char('12'::int4,NULL::text) AS v",
            None,
            false,
        ),
        (
            "i8-null-template",
            "UTC",
            "SELECT to_char('12'::int8,NULL::text) AS v",
            None,
            false,
        ),
        (
            "fm-mi",
            "UTC",
            "SELECT to_char('12'::int8,'FM999MI'::text) AS v",
            Some("12"),
            false,
        ),
        (
            "quoted-mi",
            "UTC",
            "SELECT to_char('12'::int4,'FM999\"MI\"'::text) AS v",
            Some("12MI"),
            false,
        ),
        (
            "spaces",
            "UTC",
            "SELECT to_char('12'::int8,'  FM999  '::text) AS v",
            Some("  12  "),
            false,
        ),
        (
            "empty",
            "UTC",
            "SELECT to_char('1'::int4,''::text) AS v",
            Some(""),
            false,
        ),
        (
            "zero",
            "UTC",
            "SELECT to_char('0'::int8,'FM000'::text) AS v",
            Some("000"),
            false,
        ),
        (
            "int4-zero-row",
            "UTC",
            "SELECT to_char(NULL::int4,'FM000'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "int4-nested",
            "UTC",
            "SELECT to_char(abs('-12'::int4),'FM000'::text) AS v",
            Some("012"),
            false,
        ),
        (
            "int8-zero-row",
            "UTC",
            "SELECT to_char(NULL::int8,'FM000'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "int8-nested",
            "UTC",
            "SELECT to_char(abs('-12'::int8),'FM000'::text) AS v",
            Some("012"),
            false,
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
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
    for (source_type, oid) in [("int4", 23u32), ("int8", 20u32)] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT to_char($1::{source_type},$2::text) AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            query(&mut socket, "SET time_zone = 'UTC'");
            let parse = [b"fmt15\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
            send(&mut socket, b'P', &parse);
            send(&mut socket, b'D', b"Sfmt15\0");
            send(&mut socket, b'S', &[]);
            let messages = ready(&mut socket);
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                [b'1', b't', b'T', b'Z'],
                "{messages:?}"
            );
            assert_eq!(
                messages.iter().find(|m| m.0 == b't').unwrap().1,
                [
                    &2i16.to_be_bytes()[..],
                    &oid.to_be_bytes(),
                    &25u32.to_be_bytes()
                ]
                .concat()
            );
            let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
            assert_eq!(&description[10..14], &25u32.to_be_bytes());
            assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
            assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
            query(&mut socket, "SET time_zone = 'America/New_York'");
            for input_format in [0i16, 1] {
                for output_format in [0i16, 1] {
                    for index in 0..4 {
                        let value = if index == 1 {
                            None
                        } else if input_format == 0 {
                            Some(b"12".to_vec())
                        } else {
                            Some(if oid == 23 {
                                12i32.to_be_bytes().to_vec()
                            } else {
                                12i64.to_be_bytes().to_vec()
                            })
                        };
                        let mut bind = b"\0fmt15\0".to_vec();
                        bind.extend_from_slice(&1i16.to_be_bytes());
                        bind.extend_from_slice(&input_format.to_be_bytes());
                        bind.extend_from_slice(&2i16.to_be_bytes());
                        let unit: &[u8] = if index == 3 {
                            b"99.9V99"
                        } else if index == 2 {
                            b"FM999MI"
                        } else {
                            b"FM000"
                        };
                        for payload in [value.as_deref(), Some(unit)] {
                            bind.extend_from_slice(
                                &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                            );
                            if let Some(v) = payload {
                                bind.extend_from_slice(v);
                            }
                        }
                        bind.extend_from_slice(&1i16.to_be_bytes());
                        bind.extend_from_slice(&output_format.to_be_bytes());
                        send(&mut socket, b'B', &bind);
                        send(&mut socket, b'D', b"P\0");
                        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                        send(&mut socket, b'S', &[]);
                        let messages = ready(&mut socket);
                        if index == 3 && !empty {
                            assert_eq!(
                                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                                [b'2', b'T', b'E', b'Z']
                            );
                            let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
                            assert!(error.windows(7).any(|b| b == b"C42601\0"));
                            assert_eq!(messages.last().unwrap().1, b"I");
                            assert_eq!(
                                query(&mut socket, "SELECT 1::int4 AS v")
                                    .iter()
                                    .find(|m| m.0 == b'D')
                                    .unwrap()
                                    .1,
                                row(&[Some("1")])
                            );
                            continue;
                        }
                        let expected_tags = if empty {
                            vec![b'2', b'T', b'C', b'Z']
                        } else {
                            vec![b'2', b'T', b'D', b'C', b'Z']
                        };
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            expected_tags,
                            "{messages:?}"
                        );
                        let mut portal = description.clone();
                        let n = portal.len();
                        portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                        assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                        if !empty {
                            let expected = if index == 1 {
                                row(&[None])
                            } else {
                                row(&[Some(if index == 2 { "12" } else { "012" })])
                            };
                            assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                        }
                        assert_eq!(messages.last().unwrap().1, b"I");
                    }
                }
            }
            send(&mut socket, b'C', b"Sfmt15\0");
            send(&mut socket, b'S', &[]);
            assert_eq!(
                ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
                [b'3', b'Z']
            );
        }
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_15_sql: 86 scenarios passed; listener closed");
}

#[test]
fn pg08_16_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, zone, sql, value, zero) in [
        (
            "float4-basic",
            "UTC",
            "SELECT to_char('125.5'::float4,'FM999.0'::text) AS v",
            Some("125.5"),
            false,
        ),
        (
            "float4-tie",
            "UTC",
            "SELECT to_char('2.5'::float4,'FM9'::text) AS v",
            Some("2"),
            false,
        ),
        (
            "float4-precision",
            "UTC",
            "SELECT to_char('1.23456789'::float4,'FM9.999999999'::text) AS v",
            Some("1.23457"),
            false,
        ),
        (
            "float4-edge",
            "UTC",
            "SELECT to_char('9.999999'::float4,'FM99.999999999'::text) AS v",
            Some("10."),
            false,
        ),
        (
            "float4-nan",
            "UTC",
            "SELECT to_char('NaN'::float4,'FM999.0'::text) AS v",
            Some("NaN"),
            false,
        ),
        (
            "float4-inf",
            "UTC",
            "SELECT to_char('Infinity'::float4,'FM999.0'::text) AS v",
            Some("###."),
            false,
        ),
        (
            "float4-ninf",
            "UTC",
            "SELECT to_char('-Infinity'::float4,'FM999.0'::text) AS v",
            Some("-###."),
            false,
        ),
        (
            "float4-negzero",
            "UTC",
            "SELECT to_char('-0'::float4,'FM999.0'::text) AS v",
            Some("-.0"),
            false,
        ),
        (
            "float4-empty",
            "UTC",
            "SELECT to_char('1'::float4,''::text) AS v",
            Some(""),
            false,
        ),
        (
            "float4-null",
            "UTC",
            "SELECT to_char(NULL::float4,'FM999.0'::text) AS v",
            None,
            false,
        ),
        (
            "float4-null-template",
            "UTC",
            "SELECT to_char('1'::float4,NULL::text) AS v",
            None,
            false,
        ),
        (
            "float4-shift",
            "UTC",
            "SELECT to_char('1.23456789'::float4,'FM99V99'::text) AS v",
            Some("123"),
            false,
        ),
        (
            "float8-basic",
            "UTC",
            "SELECT to_char('125.5'::float8,'FM999.0'::text) AS v",
            Some("125.5"),
            false,
        ),
        (
            "float8-tie",
            "UTC",
            "SELECT to_char('2.5'::float8,'FM9'::text) AS v",
            Some("2"),
            false,
        ),
        (
            "float8-precision",
            "UTC",
            "SELECT to_char('1.23456789'::float8,'FM9.999999999'::text) AS v",
            Some("1.23456789"),
            false,
        ),
        (
            "float8-edge",
            "UTC",
            "SELECT to_char('9.999999'::float8,'FM99.999999999'::text) AS v",
            Some("9.999999"),
            false,
        ),
        (
            "float8-nan",
            "UTC",
            "SELECT to_char('NaN'::float8,'FM999.0'::text) AS v",
            Some("NaN"),
            false,
        ),
        (
            "float8-inf",
            "UTC",
            "SELECT to_char('Infinity'::float8,'FM999.0'::text) AS v",
            Some("###.#"),
            false,
        ),
        (
            "float8-ninf",
            "UTC",
            "SELECT to_char('-Infinity'::float8,'FM999.0'::text) AS v",
            Some("-###.#"),
            false,
        ),
        (
            "float8-negzero",
            "UTC",
            "SELECT to_char('-0'::float8,'FM999.0'::text) AS v",
            Some("-.0"),
            false,
        ),
        (
            "float8-empty",
            "UTC",
            "SELECT to_char('1'::float8,''::text) AS v",
            Some(""),
            false,
        ),
        (
            "float8-null",
            "UTC",
            "SELECT to_char(NULL::float8,'FM999.0'::text) AS v",
            None,
            false,
        ),
        (
            "float8-null-template",
            "UTC",
            "SELECT to_char('1'::float8,NULL::text) AS v",
            None,
            false,
        ),
        (
            "float8-shift",
            "UTC",
            "SELECT to_char('1.23456789'::float8,'FM99V99'::text) AS v",
            Some("123"),
            false,
        ),
        (
            "float4-zero-row",
            "UTC",
            "SELECT to_char(NULL::float4,'FM000'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "float4-nested",
            "UTC",
            "SELECT to_char(float4abs('-12'::float4),'FM000'::text) AS v",
            Some("012"),
            false,
        ),
        (
            "float8-zero-row",
            "UTC",
            "SELECT to_char(NULL::float8,'FM000'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "float8-nested",
            "UTC",
            "SELECT to_char(float8abs('-12'::float8),'FM000'::text) AS v",
            Some("012"),
            false,
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
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
    for (source_type, oid) in [("float4", 700u32), ("float8", 701u32)] {
        for empty in [false, true] {
            let sql = format!(
                "SELECT to_char($1::{source_type},$2::text) AS v{}",
                if empty { " WHERE false" } else { "" }
            );
            query(&mut socket, "SET time_zone = 'UTC'");
            let parse = [b"fmt16\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
            send(&mut socket, b'P', &parse);
            send(&mut socket, b'D', b"Sfmt16\0");
            send(&mut socket, b'S', &[]);
            let messages = ready(&mut socket);
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                [b'1', b't', b'T', b'Z'],
                "{messages:?}"
            );
            assert_eq!(
                messages.iter().find(|m| m.0 == b't').unwrap().1,
                [
                    &2i16.to_be_bytes()[..],
                    &oid.to_be_bytes(),
                    &25u32.to_be_bytes()
                ]
                .concat()
            );
            let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
            assert_eq!(&description[10..14], &25u32.to_be_bytes());
            assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
            assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
            query(&mut socket, "SET time_zone = 'America/New_York'");
            for input_format in [0i16, 1] {
                for output_format in [0i16, 1] {
                    for index in 0..4 {
                        let value = if index == 1 {
                            None
                        } else if input_format == 0 {
                            Some(b"12".to_vec())
                        } else {
                            Some(if oid == 700 {
                                12f32.to_be_bytes().to_vec()
                            } else {
                                12f64.to_be_bytes().to_vec()
                            })
                        };
                        let mut bind = b"\0fmt16\0".to_vec();
                        bind.extend_from_slice(&1i16.to_be_bytes());
                        bind.extend_from_slice(&input_format.to_be_bytes());
                        bind.extend_from_slice(&2i16.to_be_bytes());
                        let unit: &[u8] = if index == 3 {
                            b"99.9V99"
                        } else if index == 2 {
                            b"FM999MI"
                        } else {
                            b"FM000"
                        };
                        for payload in [value.as_deref(), Some(unit)] {
                            bind.extend_from_slice(
                                &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                            );
                            if let Some(v) = payload {
                                bind.extend_from_slice(v);
                            }
                        }
                        bind.extend_from_slice(&1i16.to_be_bytes());
                        bind.extend_from_slice(&output_format.to_be_bytes());
                        send(&mut socket, b'B', &bind);
                        send(&mut socket, b'D', b"P\0");
                        send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                        send(&mut socket, b'S', &[]);
                        let messages = ready(&mut socket);
                        if index == 3 && !empty {
                            assert_eq!(
                                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                                [b'2', b'T', b'E', b'Z']
                            );
                            let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
                            assert!(error.windows(7).any(|b| b == b"C42601\0"));
                            assert_eq!(messages.last().unwrap().1, b"I");
                            assert_eq!(
                                query(&mut socket, "SELECT 1::int4 AS v")
                                    .iter()
                                    .find(|m| m.0 == b'D')
                                    .unwrap()
                                    .1,
                                row(&[Some("1")])
                            );
                            continue;
                        }
                        let expected_tags = if empty {
                            vec![b'2', b'T', b'C', b'Z']
                        } else {
                            vec![b'2', b'T', b'D', b'C', b'Z']
                        };
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            expected_tags,
                            "{messages:?}"
                        );
                        let mut portal = description.clone();
                        let n = portal.len();
                        portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                        assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                        if !empty {
                            let expected = if index == 1 {
                                row(&[None])
                            } else {
                                row(&[Some(if index == 2 { "12" } else { "012" })])
                            };
                            assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                        }
                        assert_eq!(messages.last().unwrap().1, b"I");
                    }
                }
            }
            send(&mut socket, b'C', b"Sfmt16\0");
            send(&mut socket, b'S', &[]);
            assert_eq!(
                ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
                [b'3', b'Z']
            );
        }
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_16_sql: 93 scenarios passed; listener closed");
}

#[test]
fn pg08_17_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    let changed = query(&mut socket, "SET lc_time = 'de_DE.UTF-8'");
    assert!(!changed.iter().any(|m| m.0 == b'E'), "{changed:?}");
    let observed = query(
        &mut socket,
        "SELECT to_char('2024-01-02'::timestamp,'TMMonth TMDay'::text) AS v",
    );
    assert_eq!(
        observed.iter().find(|m| m.0 == b'D').unwrap().1,
        row(&[Some("Januar Dienstag")])
    );
    query(&mut socket, "SET lc_time = 'C'");
    for (id, sql, value, zero) in [
        (
            "S-Q-microseconds",
            "SELECT to_char('2024-01-02 03:04:05.123456'::timestamp,'YYYY-MM-DD HH24:MI:SS.US'::text) AS v",
            Some("2024-01-02 03:04:05.123456"),
            false,
        ),
        (
            "S-Q-iso",
            "SELECT to_char('2021-01-01'::timestamp,'IYYY-IW'::text) AS v",
            Some("2020-53"),
            false,
        ),
        (
            "S-Q-modifiers",
            "SELECT to_char('2024-01-02'::timestamp,'\"Year=\"YYYY FMDD DDTH'::text) AS v",
            Some("Year=2024 2 02ND"),
            false,
        ),
        (
            "S-Q-percent",
            "SELECT to_char('2024-01-02'::timestamp,'%Y'::text) AS v",
            Some("%4"),
            false,
        ),
        (
            "S-Q-quoted",
            "SELECT to_char('2024-01-02'::timestamp,'\"YYYY\"'::text) AS v",
            Some("YYYY"),
            false,
        ),
        (
            "S-Q-empty",
            "SELECT to_char('2024-01-02'::timestamp,''::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-infinity",
            "SELECT to_char('infinity'::timestamp,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-negative-infinity",
            "SELECT to_char('-infinity'::timestamp,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-null",
            "SELECT to_char(NULL::timestamp,'YYYY'::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-null-template",
            "SELECT to_char('2024-01-02'::timestamp,NULL::text) AS v",
            None,
            false,
        ),
        (
            "S-Q-zero",
            "SELECT to_char(NULL::timestamp,'YYYY'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "S-Q-nested",
            "SELECT to_char(date_trunc('day'::text,'2024-01-02 03:04:05'::timestamp),'YYYY-MM-DD HH24:MI:SS'::text) AS v",
            Some("2024-01-02 00:00:00"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT to_char($1::timestamp,$2::text) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        let parse = [b"\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"S\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1114u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &25u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..3 {
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"2000-01-02 00:00:40.123456".to_vec())
                    } else {
                        Some(86_440_123_456i64.to_be_bytes().to_vec())
                    };
                    let mut bind = vec![0, 0];
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 2 {
                        b"TMMonth TMDay"
                    } else {
                        b"HH24:MI:SS.US"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else {
                            row(&[Some(if index == 2 {
                                "January Sunday"
                            } else {
                                "00:00:40.123456"
                            })])
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_17_sql: 38 scenarios passed; listener closed");
}

#[test]
fn pg08_18_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, zone, sql, value, zero) in [
        (
            "main",
            "UTC",
            "SELECT to_char('1234.5'::numeric,'FM9G999D0'::text) AS v",
            Some("1,234.5"),
            false,
        ),
        (
            "negative",
            "UTC",
            "SELECT to_char('-148.5'::numeric,'FM999D990'::text) AS v",
            Some("-148.500"),
            false,
        ),
        (
            "spaces",
            "UTC",
            "SELECT to_char('12'::numeric,'9999'::text) AS v",
            Some("   12"),
            false,
        ),
        (
            "zero-fill",
            "UTC",
            "SELECT to_char('12'::numeric,'0000'::text) AS v",
            Some(" 0012"),
            false,
        ),
        (
            "fraction",
            "UTC",
            "SELECT to_char('0.1'::numeric,'FM9.990'::text) AS v",
            Some(".100"),
            false,
        ),
        (
            "optional",
            "UTC",
            "SELECT to_char('12.3'::numeric,'FM99.999'::text) AS v",
            Some("12.3"),
            false,
        ),
        (
            "round",
            "UTC",
            "SELECT to_char('9.995'::numeric,'FM99.00'::text) AS v",
            Some("10.00"),
            false,
        ),
        (
            "negative-round",
            "UTC",
            "SELECT to_char('-9.995'::numeric,'FM99.00'::text) AS v",
            Some("-10.00"),
            false,
        ),
        (
            "overflow",
            "UTC",
            "SELECT to_char('99.995'::numeric,'99.99'::text) AS v",
            Some(" ##.##"),
            false,
        ),
        (
            "zero",
            "UTC",
            "SELECT to_char('0'::numeric,'999'::text) AS v",
            Some("   0"),
            false,
        ),
        (
            "fill-zero",
            "UTC",
            "SELECT to_char('0'::numeric,'FM999'::text) AS v",
            Some("0"),
            false,
        ),
        (
            "literal-spaces",
            "UTC",
            "SELECT to_char('12'::numeric,'  FM999  '::text) AS v",
            Some("  12  "),
            false,
        ),
        (
            "quoted",
            "UTC",
            "SELECT to_char('148.5'::numeric,'\"x\"FM999D990'::text) AS v",
            Some("x148.500"),
            false,
        ),
        (
            "empty",
            "UTC",
            "SELECT to_char('1'::numeric,''::text) AS v",
            Some(""),
            false,
        ),
        (
            "null",
            "UTC",
            "SELECT to_char(NULL::numeric,'FM999'::text) AS v",
            None,
            false,
        ),
        (
            "null-template",
            "UTC",
            "SELECT to_char('1'::numeric,NULL::text) AS v",
            None,
            false,
        ),
        (
            "exact",
            "UTC",
            "SELECT to_char('9007199254740993'::numeric,'FM9999999999999999.0'::text) AS v",
            Some("9007199254740993.0"),
            false,
        ),
        (
            "zero-row",
            "UTC",
            "SELECT to_char(NULL::numeric,'FM999'::text) AS v WHERE false",
            None,
            true,
        ),
        (
            "nested",
            "UTC",
            "SELECT to_char(round('148.5'::numeric,1::int4),'FM999D990'::text) AS v",
            Some("148.500"),
            false,
        ),
    ] {
        assert!(
            !query(&mut socket, &format!("SET time_zone = '{zone}'"))
                .iter()
                .any(|m| m.0 == b'E')
        );
        let messages = query(&mut socket, sql);
        assert!(
            !messages.iter().any(|(tag, _)| *tag == b'E'),
            "{id}: {messages:?}"
        );
        let description = &messages.iter().find(|(tag, _)| *tag == b'T').unwrap().1;
        let mut expected = vec![0, 1, b'v', 0];
        expected.extend_from_slice(&0u32.to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        expected.extend_from_slice(&25u32.to_be_bytes());
        expected.extend_from_slice(&(-1i16).to_be_bytes());
        expected.extend_from_slice(&(-1i32).to_be_bytes());
        expected.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description, &expected, "{id}");
        let rows: Vec<_> = messages.iter().filter(|(tag, _)| *tag == b'D').collect();
        if zero {
            assert!(rows.is_empty(), "{id}");
        } else {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].1, row(&[value]), "{id}");
        }
    }
    for (numeric, monetary, sql, wanted) in [
        (
            "de_DE.UTF-8",
            "C",
            "SELECT to_char('1234.5'::numeric,'FM9G999D0'::text) AS v",
            Some("1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "C",
            "SELECT to_char('1234.5'::numeric,'FM9,999.0'::text) AS v",
            Some("1,234.5"),
        ),
        (
            "C",
            "C",
            "SELECT to_char('1234.5'::numeric,'FML9G999D0'::text) AS v",
            Some(" 1,234.5"),
        ),
        (
            "C",
            "de_DE.UTF-8",
            "SELECT to_char('1234.5'::numeric,'FML9G999D0'::text) AS v",
            Some("€1,234.5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('1234.5'::numeric,'FML9G999D0'::text) AS v",
            Some("€1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('1234.5'::numeric,'FMS9G999D0'::text) AS v",
            Some("+1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('-1234.5'::numeric,'FMS9G999D0'::text) AS v",
            Some("-1.234,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('-1234.5'::numeric,'FM9G999D0S'::text) AS v",
            Some("1.234,5-"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('12.5'::numeric,'9G999D0'::text) AS v",
            Some("    12,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('0.1'::numeric,'FM9G999D0'::text) AS v",
            Some(",1"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('0'::numeric,'FM9G999D0'::text) AS v",
            Some(",0"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('12.5'::numeric,'\"D G S L\" FM999D0'::text) AS v",
            Some("D G S L 12,5"),
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char(NULL::numeric,'FML9G999D0S'::text) AS v",
            None,
        ),
        (
            "de_DE.UTF-8",
            "de_DE.UTF-8",
            "SELECT to_char('1234.5'::numeric,NULL::text) AS v",
            None,
        ),
    ] {
        for (key, value) in [("lc_numeric", numeric), ("lc_monetary", monetary)] {
            let changed = query(&mut socket, &format!("SET {key}='{value}'"));
            assert!(!changed.iter().any(|m| m.0 == b'E'), "{changed:?}");
        }
        let messages = query(&mut socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
        assert_eq!(
            messages.iter().find(|m| m.0 == b'D').unwrap().1,
            row(&[wanted])
        );
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT to_char($1::numeric,$2::text) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        query(&mut socket, "SET lc_numeric = 'C'");
        let parse = [b"fmt18\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat();
        send(&mut socket, b'P', &parse);
        send(&mut socket, b'D', b"Sfmt18\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1700u32.to_be_bytes(),
                &25u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &25u32.to_be_bytes());
        assert_eq!(&description[14..16], &(-1i16).to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        query(&mut socket, "SET lc_numeric = 'de_DE.UTF-8'");
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..4 {
                    let value = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"148.5".to_vec())
                    } else {
                        Some(
                            // Independent numeric_send payload for 148.5.
                            vec![0, 2, 0, 0, 0, 0, 0, 1, 0, 148, 19, 136],
                        )
                    };
                    let mut bind = b"\0fmt18\0".to_vec();
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    let unit: &[u8] = if index == 3 {
                        b"99.9V99"
                    } else if index == 2 {
                        b"FM000.0"
                    } else {
                        b"FM999D990"
                    };
                    for payload in [value.as_deref(), Some(unit)] {
                        bind.extend_from_slice(
                            &payload.map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = payload {
                            bind.extend_from_slice(v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    if index == 3 && !empty {
                        assert_eq!(
                            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                            [b'2', b'T', b'E', b'Z']
                        );
                        let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
                        assert!(error.windows(7).any(|b| b == b"C42601\0"));
                        assert_eq!(messages.last().unwrap().1, b"I");
                        assert_eq!(
                            query(&mut socket, "SELECT 1::int4 AS v")
                                .iter()
                                .find(|m| m.0 == b'D')
                                .unwrap()
                                .1,
                            row(&[Some("1")])
                        );
                        continue;
                    }
                    let expected_tags = if empty {
                        vec![b'2', b'T', b'C', b'Z']
                    } else {
                        vec![b'2', b'T', b'D', b'C', b'Z']
                    };
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        expected_tags,
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let expected = if index == 1 {
                            row(&[None])
                        } else {
                            row(&[Some(if index == 2 { "148.5" } else { "148,500" })])
                        };
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                    assert_eq!(messages.last().unwrap().1, b"I");
                }
            }
        }
        send(&mut socket, b'C', b"Sfmt18\0");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'3', b'Z']
        );
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_18_sql: 66 scenarios passed; listener closed");
}

#[test]
fn pg08_19_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, wanted, zero) in [
        (
            "empty",
            "SELECT array_length('{}'::int4[],1::int4) AS v",
            None,
            false,
        ),
        (
            "null-array",
            "SELECT array_length(NULL::int4[],1::int4) AS v",
            None,
            false,
        ),
        (
            "null-dimension",
            "SELECT array_length('{1,NULL}'::int4[],NULL::int4) AS v",
            None,
            false,
        ),
        (
            "null-element",
            "SELECT array_length('{1,NULL}'::int4[],1::int4) AS v",
            Some("2"),
            false,
        ),
        (
            "axis1",
            "SELECT array_length('{{1,2,3},{4,5,6}}'::int4[],1::int4) AS v",
            Some("2"),
            false,
        ),
        (
            "axis2",
            "SELECT array_length('{{1,2,3},{4,5,6}}'::int4[],2::int4) AS v",
            Some("3"),
            false,
        ),
        (
            "axis3",
            "SELECT array_length('{{1,2,3},{4,5,6}}'::int4[],3::int4) AS v",
            None,
            false,
        ),
        (
            "axis0",
            "SELECT array_length('{{1,2,3},{4,5,6}}'::int4[],0::int4) AS v",
            None,
            false,
        ),
        (
            "axis-negative",
            "SELECT array_length('{{1,2,3},{4,5,6}}'::int4[],-1::int4) AS v",
            None,
            false,
        ),
        (
            "lower-zero",
            "SELECT array_length('[0:1]={10,20}'::int4[],1::int4) AS v",
            Some("2"),
            false,
        ),
        (
            "lower-negative",
            "SELECT array_length('[-2:-1]={10,20}'::int4[],1::int4) AS v",
            Some("2"),
            false,
        ),
        (
            "zero",
            "SELECT array_length(NULL::int4[],1::int4) AS v WHERE false",
            None,
            true,
        ),
        (
            "nested",
            "SELECT abs(array_length('{1,NULL}'::int4[],1::int4)) AS v",
            Some("2"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{id}: {messages:?}");
        let description = &messages.iter().find(|m| m.0 == b'T').unwrap().1;
        assert_eq!(&description[2..4], b"v\0");
        assert_eq!(&description[10..14], &23u32.to_be_bytes());
        assert_eq!(&description[14..16], &4i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        if zero {
            assert!(!messages.iter().any(|m| m.0 == b'D'));
        } else {
            assert_eq!(
                messages.iter().find(|m| m.0 == b'D').unwrap().1,
                row(&[wanted]),
                "{id}"
            );
        }
    }
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT array_length($1::int4[],$2::int4) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        send(
            &mut socket,
            b'P',
            &[b"arr19\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'D', b"Sarr19\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [
                &2i16.to_be_bytes()[..],
                &1007u32.to_be_bytes(),
                &23u32.to_be_bytes()
            ]
            .concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &23u32.to_be_bytes());
        assert_eq!(&description[14..16], &4i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..4 {
                    let a = if index == 1 {
                        None
                    } else if input_format == 0 {
                        Some(b"[0:1]={10,20}".to_vec())
                    } else {
                        let mut b = Vec::new();
                        for n in [1i32, 0, 23, 2, 0, 4, 10, 4, 20] {
                            b.extend_from_slice(&n.to_be_bytes());
                        }
                        Some(b)
                    };
                    let d = if index == 2 {
                        None
                    } else if input_format == 0 {
                        Some(b"1".to_vec())
                    } else {
                        Some(1i32.to_be_bytes().to_vec())
                    };
                    let mut bind = b"\0arr19\0".to_vec();
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&2i16.to_be_bytes());
                    for v in [a, d] {
                        bind.extend_from_slice(
                            &v.as_ref().map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = v {
                            bind.extend_from_slice(&v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        if empty {
                            vec![b'2', b'T', b'C', b'Z']
                        } else {
                            vec![b'2', b'T', b'D', b'C', b'Z']
                        },
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let value = if index == 1 || index == 2 {
                            None
                        } else if output_format == 0 {
                            Some(b"2".to_vec())
                        } else {
                            Some(2i32.to_be_bytes().to_vec())
                        };
                        let mut expected = 1i16.to_be_bytes().to_vec();
                        expected.extend_from_slice(
                            &value.as_ref().map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = value {
                            expected.extend_from_slice(&v);
                        }
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                }
            }
        }
        send(&mut socket, b'C', b"Sarr19\0");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'3', b'Z']
        );
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT array_length('x'::text,1::int4) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_19_sql: 46 scenarios passed; listener closed");
}

#[test]
fn pg08_20_sql() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service.clone());
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for (id, sql, wanted, zero) in [
        (
            "empty",
            "SELECT cardinality('{}'::int4[]) AS v",
            Some("0"),
            false,
        ),
        (
            "null-array",
            "SELECT cardinality(NULL::int4[]) AS v",
            None,
            false,
        ),
        (
            "null-element",
            "SELECT cardinality('{1,NULL}'::int4[]) AS v",
            Some("2"),
            false,
        ),
        (
            "multidim",
            "SELECT cardinality('{{1,2,3},{4,5,6}}'::int4[]) AS v",
            Some("6"),
            false,
        ),
        (
            "lower-zero",
            "SELECT cardinality('[0:1]={10,20}'::int4[]) AS v",
            Some("2"),
            false,
        ),
        (
            "lower-negative",
            "SELECT cardinality('[-2:-1]={10,20}'::int4[]) AS v",
            Some("2"),
            false,
        ),
        (
            "zero",
            "SELECT cardinality(NULL::int4[]) AS v WHERE false",
            None,
            true,
        ),
        (
            "nested",
            "SELECT abs(cardinality('{1,NULL}'::int4[])) AS v",
            Some("2"),
            false,
        ),
    ] {
        let messages = query(&mut socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{id}: {messages:?}");
        let description = &messages.iter().find(|m| m.0 == b'T').unwrap().1;
        assert_eq!(&description[2..4], b"v\0");
        assert_eq!(&description[10..14], &23u32.to_be_bytes());
        assert_eq!(&description[14..16], &4i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        if zero {
            assert!(!messages.iter().any(|m| m.0 == b'D'));
        } else {
            assert_eq!(
                messages.iter().find(|m| m.0 == b'D').unwrap().1,
                row(&[wanted]),
                "{id}"
            );
        }
    }
    fn ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
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
    for empty in [false, true] {
        let sql = format!(
            "SELECT cardinality($1::int4[]) AS v{}",
            if empty { " WHERE false" } else { "" }
        );
        send(
            &mut socket,
            b'P',
            &[b"arr20\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
        );
        send(&mut socket, b'D', b"Sarr20\0");
        send(&mut socket, b'S', &[]);
        let messages = ready(&mut socket);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'1', b't', b'T', b'Z'],
            "{messages:?}"
        );
        assert_eq!(
            messages.iter().find(|m| m.0 == b't').unwrap().1,
            [&1i16.to_be_bytes()[..], &1007u32.to_be_bytes()].concat()
        );
        let description = messages.iter().find(|m| m.0 == b'T').unwrap().1.clone();
        assert_eq!(&description[10..14], &23u32.to_be_bytes());
        assert_eq!(&description[14..16], &4i16.to_be_bytes());
        assert_eq!(&description[16..20], &(-1i32).to_be_bytes());
        for input_format in [0i16, 1] {
            for output_format in [0i16, 1] {
                for index in 0..4 {
                    let a = if index == 1 {
                        None
                    } else if index == 2 {
                        Some(if input_format == 0 {
                            b"{}".to_vec()
                        } else {
                            [0i32, 0, 23].iter().flat_map(|n| n.to_be_bytes()).collect()
                        })
                    } else if input_format == 0 {
                        Some(b"[0:1]={10,20}".to_vec())
                    } else {
                        let mut b = Vec::new();
                        for n in [1i32, 0, 23, 2, 0, 4, 10, 4, 20] {
                            b.extend_from_slice(&n.to_be_bytes());
                        }
                        Some(b)
                    };
                    let mut bind = b"\0arr20\0".to_vec();
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&input_format.to_be_bytes());
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    for v in [a] {
                        bind.extend_from_slice(
                            &v.as_ref().map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = v {
                            bind.extend_from_slice(&v);
                        }
                    }
                    bind.extend_from_slice(&1i16.to_be_bytes());
                    bind.extend_from_slice(&output_format.to_be_bytes());
                    send(&mut socket, b'B', &bind);
                    send(&mut socket, b'D', b"P\0");
                    send(&mut socket, b'E', &[0, 0, 0, 0, 0]);
                    send(&mut socket, b'S', &[]);
                    let messages = ready(&mut socket);
                    assert_eq!(
                        messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                        if empty {
                            vec![b'2', b'T', b'C', b'Z']
                        } else {
                            vec![b'2', b'T', b'D', b'C', b'Z']
                        },
                        "{messages:?}"
                    );
                    let mut portal = description.clone();
                    let n = portal.len();
                    portal[n - 2..].copy_from_slice(&output_format.to_be_bytes());
                    assert_eq!(messages.iter().find(|m| m.0 == b'T').unwrap().1, portal);
                    if !empty {
                        let value = if index == 1 {
                            None
                        } else if output_format == 0 {
                            Some(if index == 2 {
                                b"0".to_vec()
                            } else {
                                b"2".to_vec()
                            })
                        } else {
                            Some(
                                (if index == 2 { 0i32 } else { 2i32 })
                                    .to_be_bytes()
                                    .to_vec(),
                            )
                        };
                        let mut expected = 1i16.to_be_bytes().to_vec();
                        expected.extend_from_slice(
                            &value.as_ref().map_or(-1, |v| v.len() as i32).to_be_bytes(),
                        );
                        if let Some(v) = value {
                            expected.extend_from_slice(&v);
                        }
                        assert_eq!(messages.iter().find(|m| m.0 == b'D').unwrap().1, expected);
                    }
                }
            }
        }
        send(&mut socket, b'C', b"Sarr20\0");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            ready(&mut socket).iter().map(|m| m.0).collect::<Vec<_>>(),
            [b'3', b'Z']
        );
    }
    assert_eq!(query(&mut socket, "BEGIN").last().unwrap().1, b"T");
    let bad = "SELECT cardinality('x'::text) AS v";
    send(
        &mut socket,
        b'P',
        &[b"\0".as_slice(), bad.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut socket, b'S', &[]);
    let messages = ready(&mut socket);
    let error = &messages.iter().find(|m| m.0 == b'E').unwrap().1;
    assert!(error.windows(7).any(|b| b == b"C42883\0"), "{messages:?}");
    for field in [b'S', b'V', b'C', b'M'] {
        assert!(
            error
                .split(|b| *b == 0)
                .any(|part| part.first() == Some(&field) && part.len() > 1)
        );
    }
    assert!(!messages.iter().any(|m| m.0 == b'D'));
    assert_eq!(messages.last().unwrap().1, b"E");
    assert_eq!(query(&mut socket, "ROLLBACK").last().unwrap().1, b"I");
    assert_eq!(
        query(&mut socket, "SELECT 1::int4 AS v")
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[Some("1")])
    );
    drop(socket);
    drop(cleanup);
    assert!(TcpStream::connect(address).is_err());
    println!("pg08_20_sql: 41 scenarios passed; listener closed");
}
