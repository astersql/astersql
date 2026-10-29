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
    assert_eq!(u32::from_le_bytes(metadata[2..6].try_into().unwrap()), 20);
    assert_eq!(u16::from_le_bytes(metadata[7..9].try_into().unwrap()), 128);
    assert_eq!(metadata[9], 31);
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
        assert_eq!(u32::from_le_bytes(metadata[2..6].try_into().unwrap()), 20);
        assert_eq!(u16::from_le_bytes(metadata[7..9].try_into().unwrap()), 128);
        assert_eq!(metadata[9], 31);
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
            assert!(
                [b'D', b'H', b'P']
                    .iter()
                    .all(|tag| !fields.contains_key(tag))
            );
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
    // Compare the complete MySQL wire description (code, flags, charset,
    // length, decimals and origin) to the same declared scalar type. No PG OIDs.
    for (first, second, scalar, value) in [
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
    ] {
        let baseline = result(&mut socket, scalar);
        let expression = format!("COALESCE({first},{second})");
        let actual = result(&mut socket, &expression);
        assert_eq!(actual.0, baseline.0, "{expression}: MySQL metadata");
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
