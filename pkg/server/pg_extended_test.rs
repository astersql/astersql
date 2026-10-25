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
fn parse_bind_execute_sync() {
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
    parse(&mut socket, "catalog", "SELECT current_catalog", &[]);
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(&mut socket, "", "catalog", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"P\0");
    let description = read(&mut socket);
    assert_eq!(description.0, b'T');
    assert!(description.1[2..].starts_with(b"current_catalog\0"));
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("test")])));
    assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    let startup_sql = "select round(extract(epoch from pg_postmaster_start_time() at time zone 'UTC')) as startup_time;";
    let startup = query(&mut socket, startup_sql);
    assert_eq!(
        startup.iter().map(|message| message.0).collect::<Vec<_>>(),
        b"TDCZ"
    );
    parse(&mut socket, "startup", startup_sql, &[]);
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(&mut socket, "", "startup", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"P\0");
    assert_eq!(read(&mut socket), startup[0]);
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), startup[1]);
    assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    // Fixed table-column metadata keeps Describe independent of parameter values.
    assert_eq!(query(&mut socket, "CREATE DATABASE pg_extended")[0].0, b'C');
    assert_eq!(
        query(&mut socket, "CREATE TABLE public.t (id INT, v VARCHAR(30))")[0].0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "INSERT INTO public.t VALUES (1, 'one'), (2, 'two')"
        )[0]
        .0,
        b'C'
    );
    parse(
        &mut socket,
        "databases",
        crate::pg_catalog::DATABASES_SQL,
        &[],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    assert_eq!(
        query(&mut socket, "CREATE DATABASE pg_catalog_after_parse")[0].0,
        b'C'
    );
    bind(&mut socket, "db_portal", "databases", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"Pdb_portal\0");
    let catalog_description = read(&mut socket);
    assert_eq!(catalog_description.0, b'T');
    execute(&mut socket, "db_portal", 0);
    let mut found = false;
    loop {
        let (tag, body) = read(&mut socket);
        if tag == b'C' {
            break;
        }
        assert_eq!(tag, b'D');
        found |= body
            .windows(b"pg_catalog_after_parse".len())
            .any(|bytes| bytes == b"pg_catalog_after_parse");
    }
    assert!(
        found,
        "prepared catalog execution must observe databases created after Parse"
    );
    // Catalog metadata exists even when there are no rows, at both Describe targets.
    for (name, sql) in [
        (
            "empty_namespaces",
            "SELECT oid::bigint AS id, xmin AS state_number, nspname AS name FROM pg_catalog.pg_namespace LIMIT 0",
        ),
        (
            "tablespaces",
            "SELECT oid::bigint AS id, spcname AS name, spcacl, spcoptions FROM pg_catalog.pg_tablespace ORDER BY oid",
        ),
    ] {
        let simple = query(&mut socket, sql);
        assert_eq!(simple.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
        parse(&mut socket, name, sql, &[]);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket), (b't', 0i16.to_be_bytes().to_vec()));
        assert_eq!(read(&mut socket), simple[0]);
        bind(&mut socket, name, name, &[]);
        assert_eq!(read(&mut socket), (b'2', vec![]));
        send(&mut socket, b'D', &[b"P", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket), simple[0]);
        execute(&mut socket, name, 0);
        assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        send(&mut socket, b'C', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket), (b'3', vec![]));
    }
    let namespace_sql = "SELECT nspname AS name, xmin AS state_number FROM pg_catalog.pg_namespace WHERE nspname = 'pg_namespace_after_parse'";
    parse(&mut socket, "namespace_live", namespace_sql, &[]);
    assert_eq!(read(&mut socket), (b'1', vec![]));
    send(&mut socket, b'D', b"Snamespace_live\0");
    assert_eq!(read(&mut socket), (b't', 0i16.to_be_bytes().to_vec()));
    let namespace_description = read(&mut socket);
    assert_eq!(namespace_description.0, b'T');
    assert_eq!(
        query(&mut socket, "CREATE DATABASE pg_namespace_after_parse")[0].0,
        b'C'
    );
    bind(&mut socket, "namespace_live", "namespace_live", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"Pnamespace_live\0");
    assert_eq!(read(&mut socket), namespace_description);
    execute(&mut socket, "namespace_live", 0);
    // A new native database does not create a PG schema in this connection.
    assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    parse(
        &mut socket,
        "locks",
        crate::pg_catalog::TRANSACTIONS_SQL,
        &[],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind(&mut socket, "lock_portal", "locks", &[]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    // Both PG catalog statements lack engine handles; closing one must not
    // destroy the other's portal or close an unrelated engine statement.
    send(&mut socket, b'C', b"Sdatabases\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    execute(&mut socket, "lock_portal", 0);
    assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
    send(&mut socket, b'C', b"Slocks\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    parse(
        &mut socket,
        "s",
        "SELECT id, v FROM public.t WHERE id = $1",
        &[23],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    send(&mut socket, b'D', b"Ss\0");
    let params = read(&mut socket);
    assert_eq!(
        params,
        (
            b't',
            [1i16.to_be_bytes().as_slice(), &23u32.to_be_bytes()].concat()
        )
    );
    assert_eq!(read(&mut socket).0, b'T');
    for (value, expected) in [
        (Some("1"), Some("one")),
        (Some("2"), Some("two")),
        (None, None),
    ] {
        bind(&mut socket, "", "s", &[value]);
        assert_eq!(read(&mut socket), (b'2', vec![]));
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket).0, b'T');
        execute(&mut socket, "", 0);
        if let Some(expected) = expected {
            assert_eq!(read(&mut socket), (b'D', row(&[value, Some(expected)])));
            assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
        } else {
            assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
        }
        send(&mut socket, b'S', b"");
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
    }
    // An unnamed statement uses the same binding and object lifecycle.
    parse(&mut socket, "", "SELECT id FROM public.t ORDER BY id", &[]);
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "pages", "", &[]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "pages", 1);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("1")])));
    assert_eq!(read(&mut socket), (b's', vec![]));
    execute(&mut socket, "pages", 1);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("2")])));
    assert_eq!(read(&mut socket), (b'C', b"SELECT 2\0".to_vec()));
    send(&mut socket, b'C', b"Ppages\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    send(&mut socket, b'C', b"Ss\0");
    assert_eq!(read(&mut socket), (b'3', vec![]));
    bind(&mut socket, "", "s", &[Some("1")]);
    let missing = read(&mut socket);
    assert_eq!(missing.0, b'E');
    assert!(missing.1.windows(5).any(|w| w == b"26000"));
    // Ignored Parse after error must not register this name.
    parse(&mut socket, "ignored", "SELECT 1", &[]);
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    send(&mut socket, b'D', b"Signored\0");
    assert_eq!(read(&mut socket).0, b'E');
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    // JDBC closes a statement whose Parse failed before pipelining the next
    // query. A missing Close target is successful, so recovery must not discard
    // that query until another Sync. Portals have the same Close contract.
    for target in [b"Signored\0".as_slice(), b"Pabsent_portal\0".as_slice()] {
        send(&mut socket, b'C', target);
        parse(&mut socket, "close_recovery", "SELECT 1", &[]);
        bind(&mut socket, "close_recovery", "close_recovery", &[]);
        execute(&mut socket, "close_recovery", 0);
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'3', vec![]));
        assert_eq!(read(&mut socket), (b'1', vec![]));
        assert_eq!(read(&mut socket), (b'2', vec![]));
        assert_eq!(read(&mut socket), (b'D', row(&[Some("1")])));
        assert_eq!(read(&mut socket), (b'C', b"SELECT 1\0".to_vec()));
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        send(&mut socket, b'C', b"Sclose_recovery\0");
        assert_eq!(read(&mut socket), (b'3', vec![]));
    }
    // Parameter occurrence order is independent of indexed client order.
    parse(
        &mut socket,
        "repeat",
        "SELECT id FROM public.t WHERE id = $2 OR id = $1 OR id = $2",
        &[23, 23],
    );
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "", "repeat", &[Some("1"), Some("2")]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket).0, b'D');
    assert_eq!(read(&mut socket).0, b'D');
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    // Text is bound as data; quoting cannot turn a value into SQL.
    parse(
        &mut socket,
        "update",
        "UPDATE public.t SET v = $2 WHERE id = $1",
        &[23, 25],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for value in [Some("' OR 'x'='x"), None] {
        bind(&mut socket, "", "update", &[Some("1"), value]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'C', b"UPDATE 1\0".to_vec()));
        send(&mut socket, b'S', b"");
        assert_eq!(read(&mut socket).0, b'Z');
        assert_eq!(
            query(&mut socket, "SELECT v FROM public.t WHERE id = 1")[1],
            (b'D', row(&[value]))
        );
    }
    parse(
        &mut socket,
        "inferred",
        "SELECT id FROM public.t WHERE id = $1",
        &[],
    );
    let unsupported = read(&mut socket);
    assert_eq!(unsupported.0, b'E');
    assert!(unsupported.1.windows(5).any(|w| w == b"42P18"));
    // Simple Query is also discarded until Sync after an extended error.
    send(&mut socket, b'Q', b"DELETE FROM public.t\0");
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(
        query(&mut socket, "SELECT id FROM public.t")
            .iter()
            .filter(|m| m.0 == b'D')
            .count(),
        2
    );
    parse(&mut socket, "bool", "SELECT TRUE", &[]);
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "", "bool", &[]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("t")])));
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket).0, b'Z');
    send(&mut socket, b'X', b"");
    service.close();
}

fn parse(socket: &mut TcpStream, name: &str, sql: &str, oids: &[u32]) {
    let mut body = [
        name.as_bytes(),
        b"\0",
        sql.as_bytes(),
        b"\0",
        &(oids.len() as i16).to_be_bytes(),
    ]
    .concat();
    for oid in oids {
        body.extend_from_slice(&oid.to_be_bytes());
    }
    send(socket, b'P', &body);
}
fn bind(socket: &mut TcpStream, portal: &str, statement: &str, values: &[Option<&str>]) {
    let mut body = [
        portal.as_bytes(),
        b"\0",
        statement.as_bytes(),
        b"\0",
        &0i16.to_be_bytes(),
        &(values.len() as i16).to_be_bytes(),
    ]
    .concat();
    for value in values {
        match value {
            Some(value) => {
                body.extend_from_slice(&(value.len() as i32).to_be_bytes());
                body.extend_from_slice(value.as_bytes());
            }
            None => body.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }
    body.extend_from_slice(&0i16.to_be_bytes());
    send(socket, b'B', &body);
}
fn execute(socket: &mut TcpStream, portal: &str, limit: u32) {
    send(
        socket,
        b'E',
        &[portal.as_bytes(), b"\0", &limit.to_be_bytes()].concat(),
    );
}

#[test]
fn postgres_create_table_extended_query() {
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
    let body = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    parse(
        &mut socket,
        "create_pg",
        r#"CREATE TABLE public."PgCreateExtended" (
            "id" integer NOT NULL,
            "payload" bytea,
            "amount" numeric(9, 2) DEFAULT 2.50,
            "stamp" timestamp(2) DEFAULT CURRENT_TIMESTAMP(2)
        )"#,
        &[],
    );
    assert_eq!(read(&mut socket), (b'1', Vec::new()));
    bind(&mut socket, "create_portal", "create_pg", &[]);
    assert_eq!(read(&mut socket), (b'2', Vec::new()));
    execute(&mut socket, "create_portal", 0);
    assert_eq!(read(&mut socket), (b'C', b"CREATE TABLE\0".to_vec()));
    send(&mut socket, b'S', b"");
    assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));

    let folded = query(
        &mut socket,
        "SELECT a.attname, pg_catalog.format_type(a.atttypid,a.atttypmod) FROM pg_catalog.pg_attribute a WHERE a.attrelid='public.pgcreateextended'::regclass::oid ORDER BY a.attnum",
    );
    assert_state(&folded[0], "42P01");
    let catalog = query(
        &mut socket,
        "SELECT a.attname, pg_catalog.format_type(a.atttypid,a.atttypmod) FROM pg_catalog.pg_attribute a WHERE a.attrelid='public.\"PgCreateExtended\"'::regclass::oid ORDER BY a.attnum",
    );
    for expected in [
        row(&[Some("id"), Some("integer")]),
        row(&[Some("payload"), Some("bytea")]),
        row(&[Some("amount"), Some("numeric(9,2)")]),
        row(&[Some("stamp"), Some("timestamp(2) without time zone")]),
    ] {
        assert!(catalog.contains(&(b'D', expected)), "{catalog:?}");
    }
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn marker_mapping_preserves_literals_comments_and_index_order() {
    let sql = "SELECT '$1', \"$2\", `x$3`, $2, $1, $2 /* $4 */ -- $5\n";
    let (adapted, mapping) = crate::pg_extended::markers(sql).unwrap();
    assert_eq!(
        adapted,
        "SELECT '$1', \"$2\", `x$3`, ?, ?, ? /* $4 */ -- $5\n"
    );
    assert_eq!(mapping, [1, 0, 1]);
    for invalid in [
        "SELECT $0",
        "SELECT $32768",
        "SELECT $tag$x$tag$",
        "SELECT ?",
        "SELECT /* unterminated",
        "SELECT x$1",
        "SELECT $1x",
        "SELECT /*! $1 */",
    ] {
        assert!(crate::pg_extended::markers(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn text_parameters_are_typed_and_reject_loss() {
    use crate::pg_extended::parameter;
    assert_eq!(
        parameter(23, Some(b"-2147483648")).unwrap().value,
        i32::MIN.to_le_bytes()
    );
    assert!(parameter(23, Some(b"2147483648")).is_err());
    assert_eq!(parameter(16, Some(b"true")).unwrap().value, [1]);
    assert!(parameter(16, Some(b"unknown")).is_err());
    assert_eq!(parameter(17, Some(b"\\x00ff")).unwrap().value, [0, 255]);
    assert!(parameter(17, Some(b"\\xabc")).is_err());
    assert!(parameter(25, Some(b"a\0b")).is_err());
    assert!(parameter(25, Some(&[255])).is_err());
    assert!(parameter(1082, Some(b"2026-02-29")).is_err());
    assert!(parameter(1082, Some(b"2024-02-29")).is_ok());
    assert!(parameter(1083, Some(b"24:00:00")).is_err());
    assert!(parameter(1114, Some(b"2026-09-30 12:30:00.123456")).is_ok());
    assert!(parameter(1114, Some(b"2026-09-30 12:30:00.1234567")).is_err());
    assert!(parameter(1700, Some(b"12.340")).is_ok());
    assert!(parameter(1700, Some(b"12bad")).is_err());
    assert!(parameter(23, None).unwrap().is_null);
}

#[test]
fn canonical_prepare_parameter_syntax_probe() {
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(91006, 0, 45, "", None).unwrap();
    let marker = context
        .prepare_statement("SELECT ?", &CancellationToken::new())
        .unwrap();
    assert_eq!(marker.parameter_count, 1);
    let result = context.prepare_statement("SELECT $1", &CancellationToken::new());
    eprintln!("canonical SELECT $1 preparation: {result:?}");
    assert!(
        result.is_err(),
        "remove this probe if engine supports indexed PG parameters"
    );
    context.close().unwrap();
}

#[test]
fn pg_introspection_parameters_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE TABLE test.parameter_live (id INT)")
        .unwrap();
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
        .set_read_timeout(Some(Duration::from_secs(20)))
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
    while read(&mut socket).0 != b'Z' {}
    parse(
        &mut socket,
        "oid",
        "SELECT $1::oid AS id, relname, relname = 'parameter_live' AS matched FROM pg_class WHERE relname = 'parameter_live' AND ($1::oid = $1::oid)",
        &[26],
    );
    assert_eq!(
        read(&mut socket),
        (b'1', vec![]),
        "catalog Parse must accept OID parameters"
    );
    send(&mut socket, b'D', b"Soid\0");
    assert_eq!(
        read(&mut socket),
        (
            b't',
            [
                1i16.to_be_bytes().as_slice(),
                26u32.to_be_bytes().as_slice()
            ]
            .concat()
        )
    );
    let description = read(&mut socket);
    assert_eq!(description.0, b'T');
    let mut fields = &description.1[2..];
    for oid in [26u32, 25, 16] {
        let end = fields.iter().position(|b| *b == 0).unwrap();
        fields = &fields[end + 1..];
        assert_eq!(u32::from_be_bytes(fields[6..10].try_into().unwrap()), oid);
        fields = &fields[18..];
    }
    assert!(fields.is_empty());
    let namespace = query(
        &mut socket,
        "SELECT oid::varchar FROM pg_namespace WHERE nspname = 'public'",
    );
    let namespace = namespace.iter().find(|m| m.0 == b'D').unwrap();
    let namespace = String::from_utf8(namespace.1[6..].to_vec()).unwrap();

    bind(&mut socket, "p", "oid", &[Some("4294967295")]);
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"Pp\0");
    assert_eq!(read(&mut socket), description);
    execute(&mut socket, "p", 0);
    assert_eq!(
        read(&mut socket),
        (
            b'D',
            row(&[Some("4294967295"), Some("parameter_live"), Some("t")])
        )
    );
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    // DataGrip sends the schema OID in network byte order even when results
    // use text. Exercise Bind on the real listener, including unsigned limits.
    for number in [0u32, 2200, u32::MAX] {
        let body = [
            b"\0oid\0".as_slice(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &4i32.to_be_bytes(),
            &number.to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat();
        send(&mut socket, b'B', &body);
        assert_eq!(read(&mut socket), (b'2', vec![]), "binary OID Bind");
        execute(&mut socket, "", 0);
        assert_eq!(
            read(&mut socket),
            (
                b'D',
                row(&[Some(&number.to_string()), Some("parameter_live"), Some("t")])
            )
        );
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    for (value, expected) in [
        (None, None),
        (Some(&[0u8, 0, 1][..]), Some("22P03")),
        (Some(&[0u8, 0, 0, 0, 1][..]), Some("22P03")),
    ] {
        bind_formats(&mut socket, "oid", &[1], &[value]);
        let response = read(&mut socket);
        if let Some(state) = expected {
            assert_eq!(response.0, b'E');
            assert!(
                response
                    .1
                    .windows(6)
                    .any(|w| w == format!("C{state}").as_bytes())
            );
        } else {
            assert_eq!(response, (b'2', vec![]));
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
        }
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    }
    parse(
        &mut socket,
        "mixed",
        "SELECT $1::oid AS id, $2::bigint AS number, $3::varchar AS label FROM pg_class WHERE relname = 'parameter_live'",
        &[26, 20, 25],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind_formats(
        &mut socket,
        "mixed",
        &[1, 1, 0],
        &[
            Some(&u32::MAX.to_be_bytes()),
            Some(&i64::MIN.to_be_bytes()),
            Some(b"mixed"),
        ],
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    execute(&mut socket, "", 0);
    assert_eq!(
        read(&mut socket),
        (
            b'D',
            row(&[
                Some("4294967295"),
                Some("-9223372036854775808"),
                Some("mixed")
            ])
        )
    );
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    for (formats, state) in [(&[2][..], "08P01"), (&[1, 0][..], "08P01")] {
        bind_formats(&mut socket, "oid", formats, &[Some(&2200u32.to_be_bytes())]);
        let response = read(&mut socket);
        assert_eq!(response.0, b'E');
        assert!(
            response
                .1
                .windows(6)
                .any(|w| w == format!("C{state}").as_bytes())
        );
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    let unbound = query(&mut socket, "SELECT $1::oid FROM pg_class");
    assert_eq!(unbound[0].0, b'E');
    assert!(unbound[0].1.windows(6).any(|w| w == b"C42P02"));
    // Inference, out-of-order/repeated indexes, and NULL keep fixed metadata.
    for (name, sql, oids, values, expected) in [
        (
            "namespace",
            "SELECT relname FROM pg_class WHERE relnamespace = $1::oid AND relname = 'parameter_live' AND $1::oid IN ($1::oid)",
            vec![],
            vec![Some(namespace.as_str())],
            vec![Some("parameter_live")],
        ),
        (
            "inferred",
            "SELECT $1::oid AS id FROM pg_class WHERE relname = 'parameter_live'",
            vec![],
            vec![Some("0")],
            vec![Some("0")],
        ),
        (
            "zero_type",
            "SELECT $1::oid FROM pg_class WHERE relname = 'parameter_live'",
            vec![0],
            vec![Some("42")],
            vec![Some("42")],
        ),
        (
            "text_cast",
            "SELECT $1::oid FROM pg_class WHERE relname = 'parameter_live'",
            vec![25],
            vec![Some("4294967295")],
            vec![Some("4294967295")],
        ),
        (
            "explicit",
            "SELECT $1 AS id FROM pg_class WHERE relname = 'parameter_live'",
            vec![26],
            vec![Some("42")],
            vec![Some("42")],
        ),
        (
            "reordered",
            "SELECT $2::oid AS second, $1::oid AS first, $2::oid AS again FROM pg_class WHERE relname = 'parameter_live'",
            vec![],
            vec![Some("12"), Some("34")],
            vec![Some("34"), Some("12"), Some("34")],
        ),
        (
            "nested",
            "WITH ids AS (SELECT $1::oid AS id FROM pg_class WHERE relname = 'parameter_live') SELECT id FROM ids WHERE id IN (SELECT $1::oid FROM pg_namespace WHERE nspname = 'public')",
            vec![],
            vec![Some("17")],
            vec![Some("17")],
        ),
        (
            "null",
            "SELECT $1::oid AS id, $1::oid IS NULL AS empty FROM pg_class WHERE relname = 'parameter_live'",
            vec![],
            vec![None],
            vec![None, Some("t")],
        ),
        (
            "text",
            "SELECT relname FROM pg_class WHERE relname = $1",
            vec![25],
            vec![Some("parameter_live")],
            vec![Some("parameter_live")],
        ),
        (
            "boolean",
            "SELECT $1 AS value FROM pg_class WHERE relname = 'parameter_live'",
            vec![16],
            vec![Some("true")],
            vec![Some("t")],
        ),
    ] {
        parse(&mut socket, name, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]), "{name}");
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        let parameters = read(&mut socket);
        assert_eq!(parameters.0, b't');
        assert_eq!(
            i16::from_be_bytes(parameters.1[..2].try_into().unwrap()) as usize,
            values.len()
        );
        let expected_oids = if oids.is_empty() {
            vec![26; values.len()]
        } else {
            oids.iter()
                .map(|oid| if *oid == 0 { 26 } else { *oid })
                .collect()
        };
        for (bytes, oid) in parameters.1[2..].chunks_exact(4).zip(expected_oids) {
            assert_eq!(u32::from_be_bytes(bytes.try_into().unwrap()), oid);
        }
        let description = read(&mut socket);
        bind(&mut socket, "p", name, &values);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"Pp\0");
        assert_eq!(read(&mut socket), description);
        execute(&mut socket, "p", 0);
        assert_eq!(read(&mut socket), (b'D', row(&expected)), "{name}");
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    bind(&mut socket, "one", "explicit", &[Some("1")]);
    assert_eq!(read(&mut socket).0, b'2');
    bind(&mut socket, "two", "explicit", &[Some("2")]);
    assert_eq!(read(&mut socket).0, b'2');
    for (portal, value) in [("two", "2"), ("one", "1")] {
        execute(&mut socket, portal, 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some(value)])));
        assert_eq!(read(&mut socket).0, b'C');
    }
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    parse(
        &mut socket,
        "refresh",
        "SELECT relname FROM pg_class WHERE relnamespace = $1::oid AND relname = 'parameter_after_parse'",
        &[],
    );
    assert_eq!(read(&mut socket).0, b'1');
    native
        .execute("CREATE TABLE test.parameter_after_parse (id INT)")
        .unwrap();
    bind(&mut socket, "p", "refresh", &[Some(namespace.as_str())]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "p", 0);
    assert_eq!(
        read(&mut socket),
        (b'D', row(&[Some("parameter_after_parse")]))
    );
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    bind(
        &mut socket,
        "p",
        "text",
        &[Some("parameter_live' OR true --")],
    );
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "p", 0);
    assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    parse(
        &mut socket,
        "small",
        "SELECT $1 AS small, $1 = 1 AS matched FROM pg_class WHERE relname = 'parameter_live'",
        &[21],
    );
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "p", "small", &[Some("1")]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "p", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("1"), Some("t")])));
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    for (sql, oids, state) in [
        ("SELECT $0::oid FROM pg_class", vec![], "42P02"),
        ("SELECT $32768::oid FROM pg_class", vec![], "42P02"),
        ("SELECT $32767::oid FROM pg_class", vec![], "42P18"),
        ("SELECT $1 FROM pg_class", vec![], "42P18"),
        ("SELECT $2::oid FROM pg_class", vec![], "42P18"),
        ("SELECT $1::oid FROM pg_class", vec![26, 26], "08P01"),
        ("SELECT $1 FROM pg_class", vec![99999], "0A000"),
    ] {
        parse(&mut socket, "bad", sql, &oids);
        let response = read(&mut socket);
        assert_eq!(response.0, b'E', "{sql}: {response:?}");
        assert!(
            response
                .1
                .windows(6)
                .any(|w| w == [b"C", state.as_bytes()].concat()),
            "{sql}: {response:?}"
        );
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
        send(&mut socket, b'C', b"Sbad\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    for (values, state) in [
        (vec![Some("-1")], "22003"),
        (vec![Some("4294967296")], "22003"),
        (
            vec![Some("170141183460469231731687303715884105728")],
            "22003",
        ),
        (vec![Some("abc")], "22P02"),
        (vec![Some("1' OR true")], "22P02"),
        (vec![], "08P01"),
    ] {
        bind(&mut socket, "p", "oid", &values);
        let response = read(&mut socket);
        assert_eq!(response.0, b'E', "{values:?}: {response:?}");
        assert!(
            response
                .1
                .windows(6)
                .any(|w| w == [b"C", state.as_bytes()].concat()),
            "{values:?}: {response:?}"
        );
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    bind(&mut socket, "p", "oid", &[None]);
    assert_eq!(read(&mut socket).0, b'2');
    send(&mut socket, b'D', b"Pp\0");
    assert_eq!(read(&mut socket), description);
    execute(&mut socket, "p", 0);
    // NULL = NULL is unknown: no rows, but identical Describe.
    assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
    send(&mut socket, b'C', b"Pp\0");
    assert_eq!(read(&mut socket).0, b'3');
    send(&mut socket, b'C', b"Soid\0");
    assert_eq!(read(&mut socket).0, b'3');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

fn bind_formats(
    socket: &mut TcpStream,
    statement: &str,
    formats: &[i16],
    values: &[Option<&[u8]>],
) {
    let mut body = [
        b"\0".as_slice(),
        statement.as_bytes(),
        b"\0",
        &(formats.len() as i16).to_be_bytes(),
    ]
    .concat();
    for format in formats {
        body.extend_from_slice(&format.to_be_bytes());
    }
    body.extend_from_slice(&(values.len() as i16).to_be_bytes());
    for value in values {
        match value {
            Some(bytes) => {
                body.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                body.extend_from_slice(bytes);
            }
            None => body.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }
    body.extend_from_slice(&0i16.to_be_bytes());
    send(socket, b'B', &body);
}

#[test]
fn pg_introspection_parameters_array_metadata() {
    use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
    // Provider-owned array types must not become generic MySQL BLOB/text OIDs.
    let result = QueryResult {
        columns: vec![
            ColumnInfo {
                schema: String::new(),
                table: String::new(),
                org_table: String::new(),
                name: "array".into(),
                org_name: String::new(),
                charset: 45,
                column_length: 64,
                column_type: 253,
                flags: 0,
                decimals: 0,
                default_value: None
            };
            4
        ],
        native_types: [
            crate::pg_result::CatalogColumnType::Int2Array as u8,
            crate::pg_result::CatalogColumnType::Int4Array as u8,
            crate::pg_result::CatalogColumnType::OidArray as u8,
            crate::pg_result::CatalogColumnType::TextArray as u8,
        ]
        .into_iter()
        .map(|code| NativeType {
            code,
            flags: 0,
            length: 64,
            decimal: 0,
            postgresql_array_element: None,
            postgresql_record_fields: None,
        })
        .collect(),
        rows: vec![vec![
            Value::Text("{1,NULL,2}".into()),
            Value::Text("{1,2}".into()),
            Value::Text("{0,4294967295}".into()),
            Value::Text("{\"a,b\",NULL}".into()),
        ]],
        ..QueryResult::default()
    };
    let encoded = crate::pg_result::encode(&result, "SELECT").unwrap();
    let mut rest = &encoded[0].1[2..];
    for oid in [1005u32, 1007, 1028, 1009] {
        let end = rest.iter().position(|b| *b == 0).unwrap();
        rest = &rest[end + 1..];
        assert_eq!(u32::from_be_bytes(rest[6..10].try_into().unwrap()), oid);
        rest = &rest[18..];
    }
    assert!(rest.is_empty());
    assert_eq!(
        encoded[1],
        (
            b'D',
            row(&[
                Some("{1,NULL,2}"),
                Some("{1,2}"),
                Some("{0,4294967295}"),
                Some("{\"a,b\",NULL}")
            ])
        )
    );

    let binary = crate::pg_result::encode_formats(&result, "SELECT", &[1; 4]).unwrap();
    let mut fields = &binary[1].1[2..];
    for (element, count, nulls, elements) in [
        (
            21u32,
            3i32,
            1i32,
            vec![
                Some(1i16.to_be_bytes().to_vec()),
                None,
                Some(2i16.to_be_bytes().to_vec()),
            ],
        ),
        (
            23,
            2,
            0,
            vec![
                Some(1i32.to_be_bytes().to_vec()),
                Some(2i32.to_be_bytes().to_vec()),
            ],
        ),
        (
            26,
            2,
            0,
            vec![
                Some(0u32.to_be_bytes().to_vec()),
                Some(u32::MAX.to_be_bytes().to_vec()),
            ],
        ),
        (25, 2, 1, vec![Some(b"a,b".to_vec()), None]),
    ] {
        let mut expected = [
            1i32.to_be_bytes().as_slice(),
            &nulls.to_be_bytes(),
            &element.to_be_bytes(),
            &count.to_be_bytes(),
            &1i32.to_be_bytes(),
        ]
        .concat();
        for item in elements {
            match item {
                Some(bytes) => {
                    expected.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
                    expected.extend_from_slice(&bytes);
                }
                None => expected.extend_from_slice(&(-1i32).to_be_bytes()),
            }
        }
        let length = i32::from_be_bytes(fields[..4].try_into().unwrap()) as usize;
        assert_eq!(&fields[4..4 + length], expected);
        fields = &fields[4 + length..];
    }
    assert!(fields.is_empty());

    let empty = QueryResult {
        rows: vec![],
        ..result
    };
    assert_eq!(
        crate::pg_result::encode(&empty, "SELECT").unwrap()[0],
        encoded[0]
    );
}

#[test]
fn pg_introspection_binary_results_and_recovery() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE TABLE test.binary_live (id INT PRIMARY KEY)")
        .unwrap();
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
        .set_read_timeout(Some(Duration::from_secs(20)))
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
    while read(&mut socket).0 != b'Z' {}
    parse(
        &mut socket,
        "binary",
        "SELECT $1::oid AS id, relname, relname='binary_live' AS matched FROM pg_class WHERE relname='binary_live'",
        &[26],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    send(&mut socket, b'D', b"Sbinary\0");
    assert_eq!(read(&mut socket).0, b't');
    let statement_description = read(&mut socket);
    assert_eq!(statement_description.0, b'T');
    for formats in [vec![1i16], vec![1, 0, 1], vec![]] {
        let mut body = [
            b"\0binary\0".as_slice(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &4i32.to_be_bytes(),
            &u32::MAX.to_be_bytes(),
            &(formats.len() as i16).to_be_bytes(),
        ]
        .concat();
        for format in &formats {
            body.extend_from_slice(&format.to_be_bytes());
        }
        send(&mut socket, b'B', &body);
        assert_eq!(read(&mut socket), (b'2', vec![]));
        send(&mut socket, b'D', b"P\0");
        let portal_description = read(&mut socket);
        assert_eq!(portal_description.0, b'T');
        let mut offset = 2;
        for index in 0..3 {
            offset += portal_description.1[offset..]
                .iter()
                .position(|b| *b == 0)
                .unwrap()
                + 1;
            let format = if formats.is_empty() {
                0
            } else if formats.len() == 1 {
                formats[0]
            } else {
                formats[index]
            };
            assert_eq!(
                &portal_description.1[offset + 16..offset + 18],
                &format.to_be_bytes()
            );
            assert_eq!(&statement_description.1[offset + 16..offset + 18], &[0, 0]);
            offset += 18;
        }
        execute(&mut socket, "", 1);
        let message = read(&mut socket);
        assert_eq!(message.0, b'D');
        let expected = if formats.is_empty() {
            row(&[Some("4294967295"), Some("binary_live"), Some("t")])
        } else {
            [
                3i16.to_be_bytes().as_slice(),
                &4i32.to_be_bytes(),
                &u32::MAX.to_be_bytes(),
                &11i32.to_be_bytes(),
                b"binary_live",
                &1i32.to_be_bytes(),
                &[1],
            ]
            .concat()
        };
        assert_eq!(message.1, expected);
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    // An invalid format is a protocol error; Sync restores the same connection.
    send(
        &mut socket,
        b'B',
        &[
            b"\0binary\0".as_slice(),
            &0i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &1i16.to_be_bytes(),
            &2i16.to_be_bytes(),
        ]
        .concat(),
    );
    let failure = read(&mut socket);
    assert_eq!(failure.0, b'E');
    assert!(failure.1.windows(6).any(|w| w == b"08P01\0"));
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(
        query(&mut socket, "SELECT 1")
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"TDCZ"
    );
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_core_dialect_prepared_lifecycle() {
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _cleanup = Cleanup(service);
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_prepared.py"
        ))
        .arg(addr.port().to_string())
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // libpq uses unnamed portals. Exercise two explicitly named portals on
    // the same real canonical connection without a Sync between executions.
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
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
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "CREATE TABLE public.pg_prepared_portals (id BIGINT PRIMARY KEY)",
        "INSERT INTO public.pg_prepared_portals VALUES (17),(31),(73),(2147483648),(9223372036854775807)",
    ] {
        let response = query(&mut socket, sql);
        assert!(
            !response.iter().any(|message| message.0 == b'E'),
            "{response:?}"
        );
    }
    let mut other = TcpStream::connect(addr).unwrap();
    other
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    other
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    other.write_all(&body).unwrap();
    while read(&mut other).0 != b'Z' {}
    parse(
        &mut other,
        "a",
        "SELECT id FROM public.pg_prepared_portals WHERE id = $1",
        &[23],
    );
    assert_eq!(read(&mut other).0, b'1');
    bind(&mut other, "a", "a", &[Some("17")]);
    assert_eq!(read(&mut other).0, b'2');
    for (name, oid) in [("a", 23), ("b", 20)] {
        parse(
            &mut socket,
            name,
            "SELECT id FROM public.pg_prepared_portals WHERE id = $1",
            &[oid],
        );
        assert_eq!(read(&mut socket).0, b'1');
    }
    for (name, number) in [
        ("a", "17"),
        ("b", "2147483648"),
        ("a", "31"),
        ("b", "9223372036854775807"),
    ] {
        bind(&mut socket, name, name, &[Some(number)]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, name, 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some(number)])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'C', &[b"P", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket).0, b'3');
    }
    bind(&mut socket, "survivor", "b", &[Some("73")]);
    assert_eq!(read(&mut socket).0, b'2');
    send(&mut socket, b'C', b"Sa\0");
    assert_eq!(read(&mut socket).0, b'3');
    execute(&mut socket, "survivor", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("73")])));
    assert_eq!(read(&mut socket).0, b'C');
    // Closing a statement/portal on the first connection cannot affect an
    // identically named live portal on another connection.
    execute(&mut other, "a", 0);
    assert_eq!(read(&mut other), (b'D', row(&[Some("17")])));
    assert_eq!(read(&mut other).0, b'C');
    send(&mut other, b'C', b"Sa\0");
    assert_eq!(read(&mut other).0, b'3');
    send(&mut other, b'S', &[]);
    assert_eq!(read(&mut other).0, b'Z');
    send(&mut other, b'X', &[]);
    drop(other);
    send(&mut socket, b'C', b"Sb\0");
    assert_eq!(read(&mut socket).0, b'3');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_prepared_portals")[0].0,
        b'C'
    );
    send(&mut socket, b'X', &[]);
    drop(socket);
    _cleanup.0.close();
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn pg_core_integer_parameter_types() {
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
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
    while read(&mut socket).0 != b'Z' {}
    parse(&mut socket, "inferred", "SELECT ($1)::int4", &[]);
    assert_eq!(
        read(&mut socket),
        (b'1', vec![]),
        "explicit cast must infer omitted OID"
    );
    send(&mut socket, b'D', b"Sinferred\0");
    assert_eq!(
        read(&mut socket),
        (
            b't',
            [
                1i16.to_be_bytes().as_slice(),
                23u32.to_be_bytes().as_slice()
            ]
            .concat()
        )
    );
    assert_eq!(read(&mut socket).0, b'T');
    for value in [Some("17"), None] {
        bind(&mut socket, "", "inferred", &[value]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[value])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }

    for (name, sql, supplied, expected, inputs, outputs) in [
        (
            "reordered",
            "SELECT $2::int4 AS b, $1::int8 AS a, $2::int4 AS c",
            vec![],
            vec![20u32, 23],
            vec![Some("9007199254740993"), Some("17")],
            vec![Some("17"), Some("9007199254740993"), Some("17")],
        ),
        (
            "zero",
            "SELECT CAST($1 AS bigint)",
            vec![0],
            vec![20],
            vec![None],
            vec![None],
        ),
        (
            "source",
            "SELECT $1::int4",
            vec![20],
            vec![20],
            vec![Some("17")],
            vec![Some("17")],
        ),
        (
            "mixed",
            "SELECT $1::int4, $1::int8",
            vec![],
            vec![23],
            vec![Some("17")],
            vec![Some("17"), Some("17")],
        ),
        (
            "reverse",
            "SELECT $1::int8, $1::int4",
            vec![],
            vec![20],
            vec![Some("17")],
            vec![Some("17"), Some("17")],
        ),
        (
            "offset",
            "SELECT 'question ? $8', /* ? $3 */ ($1)::int4",
            vec![],
            vec![23],
            vec![Some("19")],
            vec![Some("question ? $8"), Some("19")],
        ),
    ] {
        parse(&mut socket, name, sql, &supplied);
        assert_eq!(read(&mut socket), (b'1', vec![]), "{sql}");
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        let mut description = (expected.len() as i16).to_be_bytes().to_vec();
        for oid in expected {
            description.extend_from_slice(&oid.to_be_bytes());
        }
        assert_eq!(read(&mut socket), (b't', description), "{sql}");
        assert_eq!(read(&mut socket).0, b'T');
        bind(&mut socket, "", name, &inputs);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&outputs)), "{sql}");
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    // Failed Parse leaves no statement; failed Bind leaves no portal.
    parse(&mut socket, "retry", "SELECT $2::int4", &[]);
    assert_state(&read(&mut socket), "42P18");
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    parse(&mut socket, "retry", "SELECT $1::int4", &[]);
    assert_eq!(read(&mut socket).0, b'1');
    for (value, state) in [("12x", "22P02"), ("2147483648", "22003")] {
        bind(&mut socket, "failed", "retry", &[Some(value)]);
        assert_state(&read(&mut socket), state);
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
        send(&mut socket, b'D', b"Pfailed\0");
        assert_state(&read(&mut socket), "34000");
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
        bind(&mut socket, "failed", "retry", &[Some("31")]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "failed", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some("31")])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }

    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE public.pg_parameter_writes (id INT)"
        )[0]
        .0,
        b'C'
    );
    parse(
        &mut socket,
        "writes",
        "INSERT INTO public.pg_parameter_writes VALUES ($1)",
        &[23],
    );
    assert_eq!(read(&mut socket).0, b'1');
    bind(&mut socket, "write_failed", "writes", &[Some("2147483648")]);
    assert_state(&read(&mut socket), "22003");
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    execute(&mut socket, "write_failed", 0);
    assert_state(&read(&mut socket), "34000");
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(
        query(
            &mut socket,
            "SELECT count(*) FROM public.pg_parameter_writes"
        )[1],
        (b'D', row(&[Some("0")]))
    );
    bind(&mut socket, "write_failed", "writes", &[Some("31")]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "write_failed", 0);
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(
        query(&mut socket, "SELECT id FROM public.pg_parameter_writes")[1],
        (b'D', row(&[Some("31")]))
    );
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_parameter_writes")[0].0,
        b'C'
    );
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_parameters.py"
        ))
        .arg(addr.port().to_string())
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    send(&mut socket, b'X', &[]);
    drop(socket);
    cleanup.0.close();
    assert!(TcpStream::connect(addr).is_err());
}

fn assert_state(message: &(u8, Vec<u8>), state: &str) {
    assert_eq!(message.0, b'E', "{message:?}");
    assert!(
        message
            .1
            .windows(6)
            .any(|bytes| bytes == [b"C", state.as_bytes()].concat()),
        "{message:?}"
    );
}

#[test]
fn pg_core_integer_result_metadata() {
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
    struct Cleanup(Arc<PgService>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(service);
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/pg_core_result_metadata.py"
        ))
        .arg(addr.port().to_string())
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    cleanup.0.close();
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn pg_dialect_26_source_parameter_contract() {
    let sql = "/*中 $3*/ INSERT INTO public.t(id) VALUES($2) ON CONFLICT(id) WHERE id>$1 DO NOTHING RETURNING $2::int4,'$1'";
    let contract = crate::pg_extended::indexed_parameter_contract(sql).unwrap();
    assert_eq!(
        contract,
        vec![
            (1, sql.find("VALUES($2").unwrap() + 7, true),
            (0, sql.find("id>$1").unwrap() + 3, false),
            (1, sql.find("RETURNING $2").unwrap() + 10, false)
        ]
    );
    let (engine, mapping) = crate::pg_extended::markers(sql).unwrap();
    assert_eq!(mapping, contract.iter().map(|v| v.0).collect::<Vec<_>>());
    assert!(engine.ends_with("RETURNING ?::int4,'$1'"));
}

pub(super) fn scalar_mysql_check(socket: &mut TcpStream) -> Vec<Vec<u8>> {
    use crate::pg_client_integration_test::{mysql_packet, mysql_query, mysql_read};
    mysql_packet(
        socket,
        0,
        b"\x03SELECT TRUE AS b, FALSE AS f, NULL AS n, 1 AS i",
    );
    assert_eq!(mysql_read(socket), vec![4]);
    let mut metadata = Vec::new();
    for _ in 0..4 {
        let column = mysql_read(socket);
        let mut offset = 0;
        for _ in 0..6 {
            let len = column[offset] as usize;
            offset += 1 + len;
        }
        assert_eq!(column[offset], 12);
        // Original numeric/NULL metadata; no PG OID or Boolean narrowing.
        let code = column[offset + 7];
        assert_eq!(code, 253, "{column:?}");
        metadata.push(column);
    }
    assert_eq!(mysql_read(socket)[0], 0xfe);
    assert_eq!(mysql_read(socket), b"\x011\x010\xfb\x011");
    let eof = mysql_read(socket);
    assert_eq!(eof[0], 0xfe);
    assert_eq!(&eof[1..3], &[0, 0]);
    assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
    metadata
}

fn scalar_types(message: &(u8, Vec<u8>), oids: &[u32]) {
    assert_eq!(message.0, b'T', "{message:?}");
    assert_eq!(
        u16::from_be_bytes(message.1[..2].try_into().unwrap()) as usize,
        oids.len()
    );
    let mut offset = 2;
    for oid in oids {
        offset += message.1[offset..].iter().position(|b| *b == 0).unwrap() + 1;
        assert_eq!(
            u32::from_be_bytes(message.1[offset + 6..offset + 10].try_into().unwrap()),
            *oid
        );
        assert_eq!(
            i16::from_be_bytes(message.1[offset + 10..offset + 12].try_into().unwrap()),
            match *oid {
                16 => 1,
                21 => 2,
                20 => 8,
                25 => -1,
                _ => 4,
            }
        );
        assert_eq!(
            i32::from_be_bytes(message.1[offset + 12..offset + 16].try_into().unwrap()),
            -1
        );
        offset += 18;
    }
}

#[test]
fn pg_scalar_01() {
    use crate::pg_client_integration_test::{mysql_handshake, mysql_query, start_client_server};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    let mysql_metadata = scalar_mysql_check(&mut mysql);
    let mut socket = TcpStream::connect(cleanup.0.postgres_listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0test\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "SELECT TRUE,FALSE,NULL::boolean,1::int4,0::int4",
        "SELECT TRUE,FALSE,CAST(NULL AS BOOL),1::int4,0::int4",
    ] {
        let messages = query(&mut socket, sql);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{messages:?}"
        );
        scalar_types(&messages[0], &[16, 16, 16, 23, 23]);
        assert_eq!(
            messages[1],
            (
                b'D',
                row(&[Some("t"), Some("f"), None, Some("1"), Some("0")])
            )
        );
    }
    for sql in [
        "CREATE TABLE public.pg_scalar_01 (id INT PRIMARY KEY,b BOOLEAN,i INT)",
        "INSERT INTO public.pg_scalar_01 VALUES (1,NULL,0),(2,TRUE,1),(3,FALSE,0)",
    ] {
        let messages = query(&mut socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
    }
    let messages = query(
        &mut socket,
        "SELECT b,i FROM public.pg_scalar_01 ORDER BY id",
    );
    scalar_types(&messages[0], &[16, 23]);
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        vec![
            row(&[None, Some("0")]),
            row(&[Some("t"), Some("1")]),
            row(&[Some("f"), Some("0")])
        ]
    );
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    for (name, sql, oids) in [
        (
            "empty",
            "SELECT b,i FROM public.pg_scalar_01 LIMIT 0",
            vec![16, 23],
        ),
        ("null", "SELECT NULL::bool LIMIT 0", vec![16]),
    ] {
        parse(&mut socket, name, sql, &[]);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket), (b't', 0i16.to_be_bytes().to_vec()));
        let description = read(&mut socket);
        scalar_types(&description, &oids);
        bind(&mut socket, "", name, &[]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), description);
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    for (name, sql, oids) in [
        ("supplied", "SELECT $1 AS b", vec![16]),
        ("inferred", "SELECT $1::boolean AS b", vec![]),
    ] {
        parse(&mut socket, name, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &16u32.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        scalar_types(&description, &[16]);
        for (input, expected) in [
            (Some("true"), Some("t")),
            (Some("false"), Some("f")),
            (None, None),
        ] {
            bind(&mut socket, "", name, &[input]);
            assert_eq!(read(&mut socket).0, b'2');
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), description);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[expected])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket).0, b'Z');
        }
    }
    parse(
        &mut socket,
        "write",
        "INSERT INTO public.pg_scalar_01 VALUES (4,$1,0)",
        &[16],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        if transaction {
            assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
        }
        bind(&mut socket, "rejected", "write", &[Some("invalid")]);
        assert_state(&read(&mut socket), "22P02");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            read(&mut socket),
            (b'Z', vec![if transaction { b'E' } else { b'I' }])
        );
        if transaction {
            assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
        }
        assert_eq!(
            query(&mut socket, "SELECT count(*) FROM public.pg_scalar_01")[1],
            (b'D', row(&[Some("3")]))
        );
    }
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_scalar_01")[0].0,
        b'C'
    );
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    assert!(mysql_query(&mut mysql, "SELECT 1") == vec![vec![Some(b"1".to_vec())]]);
    send(&mut socket, b'X', &[]);
    println!(
        "pg_scalar_01: 16 scenarios: literals/casts, declared rows, two zero-row descriptions, six parameter values, two rejected writes/recovery, MySQL before/during/after"
    );
}

#[test]
fn pg_scalar_02() {
    use crate::pg_client_integration_test::{mysql_handshake, mysql_query, start_client_server};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    let mysql_metadata = scalar_mysql_check(&mut mysql);
    let mut socket = TcpStream::connect(cleanup.0.postgres_listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0test\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "CREATE TABLE public.pg_scalar_02 (id INT PRIMARY KEY,b BOOLEAN,i INT)",
        "INSERT INTO public.pg_scalar_02 VALUES (1,NULL,0),(2,TRUE,1),(3,FALSE,0)",
    ] {
        let messages = query(&mut socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
    }
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    for (name, sql, oids) in [
        (
            "empty",
            "SELECT b,i FROM public.pg_scalar_02 LIMIT 0",
            vec![16, 23],
        ),
        ("null", "SELECT NULL::bool LIMIT 0", vec![16]),
    ] {
        parse(&mut socket, name, sql, &[]);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket), (b't', 0i16.to_be_bytes().to_vec()));
        let description = read(&mut socket);
        scalar_types(&description, &oids);
        bind(&mut socket, "", name, &[]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), description);
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    for (name, sql, oids) in [
        ("supplied", "SELECT $1 AS b", vec![16]),
        ("inferred", "SELECT $1::boolean AS b", vec![]),
    ] {
        parse(&mut socket, name, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &16u32.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        scalar_types(&description, &[16]);
        for (input, expected) in [
            (Some("true"), Some("t")),
            (Some("false"), Some("f")),
            (Some("yes"), Some("t")),
            (Some("no"), Some("f")),
            (Some("on"), Some("t")),
            (Some("off"), Some("f")),
            (Some("1"), Some("t")),
            (Some("0"), Some("f")),
            (Some("tr"), Some("t")),
            (Some("of"), Some("f")),
            (Some("n"), Some("f")),
            (Some("t"), Some("t")),
            (Some("f"), Some("f")),
            (Some("y"), Some("t")),
            (Some("tru"), Some("t")),
            (Some("fals"), Some("f")),
            (Some("ye"), Some("t")),
            (Some("fa"), Some("f")),
            (Some("fal"), Some("f")),
            (Some(" \tTrUe\n\r"), Some("t")),
            (Some("\x0bOFF\x0c"), Some("f")),
            (None, None),
        ] {
            bind(&mut socket, "", name, &[input]);
            assert_eq!(read(&mut socket).0, b'2', "{name}: {input:?}");
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), description);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[expected])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket).0, b'Z');
        }
    }
    parse(
        &mut socket,
        "write",
        "INSERT INTO public.pg_scalar_02 VALUES (4,$1,0)",
        &[16],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        for (input, state) in [
            (b"".as_slice(), "22P02"),
            (b"o", "22P02"),
            (b"2", "22P02"),
            (b"truth", "22P02"),
            (b"unknown", "22P02"),
            (b"\xff", "22021"),
            (b"t\0", "22021"),
            (b" \t\n", "22P02"),
            ("\u{a0}true".as_bytes(), "22P02"),
        ] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            bind_formats(&mut socket, "write", &[], &[Some(input)]);
            assert_state(&read(&mut socket), state);
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
            }
            assert_eq!(
                query(&mut socket, "SELECT count(*) FROM public.pg_scalar_02")[1],
                (b'D', row(&[Some("3")]))
            );
        }
    }
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_scalar_02")[0].0,
        b'C'
    );
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    assert!(mysql_query(&mut mysql, "SELECT 1") == vec![vec![Some(b"1".to_vec())]]);
    send(&mut socket, b'X', &[]);
    println!(
        "pg_scalar_02: 44 parameter values (21 spellings + NULL x 2), 18 rejected writes/recovery (9 inputs x 2), two zero-row descriptions, MySQL before/during/after"
    );
}

#[test]
fn pg_scalar_03() {
    use crate::pg_client_integration_test::{mysql_handshake, mysql_query, start_client_server};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    let mysql_metadata = scalar_mysql_check(&mut mysql);
    let mysql_cast_check = |socket: &mut TcpStream| {
        assert_eq!(
            mysql_query(socket, "SELECT CAST(2 AS SIGNED),CAST(TRUE AS CHAR)"),
            vec![vec![Some(b"2".to_vec()), Some(b"1".to_vec())]]
        );
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
    };
    mysql_cast_check(&mut mysql);
    let mut socket = TcpStream::connect(cleanup.0.postgres_listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0test\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}

    for sql in [
        "CREATE TABLE public.pg_scalar_03 (id INT PRIMARY KEY,b BOOLEAN,i INT)",
        "INSERT INTO public.pg_scalar_03 VALUES (1,NULL,0),(2,TRUE,-1),(3,FALSE,2)",
    ] {
        let messages = query(&mut socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
    }
    for (id, sql, oids, expected) in [
        ("zero", "SELECT 0::int4::boolean", vec![16], vec![Some("f")]),
        (
            "negative",
            "SELECT (-1)::int4::boolean",
            vec![16],
            vec![Some("t")],
        ),
        ("two", "SELECT 2::int4::boolean", vec![16], vec![Some("t")]),
        ("true_int", "SELECT TRUE::int4", vec![23], vec![Some("1")]),
        ("false_int", "SELECT FALSE::int4", vec![23], vec![Some("0")]),
        (
            "true_text",
            "SELECT TRUE::text",
            vec![25],
            vec![Some("true")],
        ),
        (
            "false_text",
            "SELECT FALSE::text",
            vec![25],
            vec![Some("false")],
        ),
        (
            "text_bool",
            "SELECT 'yes'::text::boolean",
            vec![16],
            vec![Some("t")],
        ),
        (
            "null_bool",
            "SELECT NULL::int4::boolean",
            vec![16],
            vec![None],
        ),
        (
            "null_int",
            "SELECT NULL::boolean::int4",
            vec![23],
            vec![None],
        ),
        (
            "null_text",
            "SELECT NULL::boolean::text",
            vec![25],
            vec![None],
        ),
        (
            "null_text_bool",
            "SELECT NULL::text::boolean",
            vec![16],
            vec![None],
        ),
        (
            "column",
            "SELECT b::int4,i::boolean,b::text FROM public.pg_scalar_03 WHERE id=2",
            vec![23, 16, 25],
            vec![Some("1"), Some("t"), Some("true")],
        ),
        (
            "column_null",
            "SELECT b::int4,i::boolean,b::text FROM public.pg_scalar_03 WHERE id=1",
            vec![23, 16, 25],
            vec![None, Some("f"), None],
        ),
        (
            "column_false",
            "SELECT b::int4,i::boolean,b::text FROM public.pg_scalar_03 WHERE id=3",
            vec![23, 16, 25],
            vec![Some("0"), Some("t"), Some("false")],
        ),
    ] {
        println!("pg_scalar_03 scenario {id}");
        parse(&mut socket, id, sql, &[]);
        assert_eq!(read(&mut socket), (b'1', vec![]), "{id}");
        send(&mut socket, b'D', &[b"S", id.as_bytes(), b"\0"].concat());
        assert_eq!(
            read(&mut socket),
            (b't', 0i16.to_be_bytes().to_vec()),
            "{id}"
        );
        let description = read(&mut socket);
        scalar_types(&description, &oids);
        bind(&mut socket, "", id, &[]);
        assert_eq!(read(&mut socket).0, b'2', "{id}");
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), description, "{id}");
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&expected)), "{id}");
        assert_eq!(read(&mut socket).0, b'C', "{id}");
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    mysql_cast_check(&mut mysql);
    for (name, sql, param_oid, result_oid, values) in [
        (
            "int_bool",
            "SELECT $1::int4::boolean",
            23,
            16,
            vec![
                (Some("0"), Some("f")),
                (Some("-1"), Some("t")),
                (Some("2"), Some("t")),
                (Some("-2147483648"), Some("t")),
                (Some("2147483647"), Some("t")),
                (None, None),
            ],
        ),
        (
            "bool_int",
            "SELECT $1::boolean::int4",
            16,
            23,
            vec![
                (Some("true"), Some("1")),
                (Some("false"), Some("0")),
                (None, None),
            ],
        ),
        (
            "bool_text",
            "SELECT $1::boolean::text",
            16,
            25,
            vec![
                (Some("true"), Some("true")),
                (Some("false"), Some("false")),
                (None, None),
            ],
        ),
        (
            "text_bool",
            "SELECT $1::text::boolean",
            25,
            16,
            vec![
                (Some("yes"), Some("t")),
                (Some("off"), Some("f")),
                (None, None),
            ],
        ),
    ] {
        for infer in [false, true] {
            let statement_name = format!("param_{name}_{infer}");
            let name = statement_name.as_str();
            let supplied = if infer { vec![] } else { vec![param_oid] };
            parse(&mut socket, name, sql, &supplied);
            assert_eq!(read(&mut socket), (b'1', vec![]), "{name}");
            send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
            assert_eq!(
                read(&mut socket),
                (
                    b't',
                    [1i16.to_be_bytes().as_slice(), &param_oid.to_be_bytes()].concat()
                )
            );
            let description = read(&mut socket);
            scalar_types(&description, &[result_oid]);
            for (input, expected) in values.iter().copied() {
                bind(&mut socket, "", name, &[input]);
                assert_eq!(read(&mut socket).0, b'2', "{name}: {input:?}");
                send(&mut socket, b'D', b"P\0");
                assert_eq!(read(&mut socket), description);
                execute(&mut socket, "", 0);
                assert_eq!(
                    read(&mut socket),
                    (b'D', row(&[expected])),
                    "{name}: {input:?}"
                );
                assert_eq!(read(&mut socket).0, b'C');
                send(&mut socket, b'S', &[]);
                assert_eq!(read(&mut socket).0, b'Z');
            }
        }
    }
    for (name, sql, oids) in [
        (
            "empty_cast",
            "SELECT b::int4,i::boolean,b::text FROM public.pg_scalar_03 LIMIT 0",
            vec![23, 16, 25],
        ),
        (
            "empty_null",
            "SELECT NULL::boolean::text,NULL::text::boolean LIMIT 0",
            vec![25, 16],
        ),
    ] {
        parse(&mut socket, name, sql, &[]);
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket), (b't', 0i16.to_be_bytes().to_vec()));
        let description = read(&mut socket);
        scalar_types(&description, &oids);
        bind(&mut socket, "", name, &[]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), description);
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'C', b"SELECT 0\0".to_vec()));
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    let messages = query(
        &mut socket,
        "INSERT INTO public.pg_scalar_03 VALUES (4,2::int4::boolean,FALSE::int4)",
    );
    assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
    assert_eq!(
        query(
            &mut socket,
            "SELECT b,i FROM public.pg_scalar_03 WHERE id=4"
        )[1],
        (b'D', row(&[Some("t"), Some("0")]))
    );
    assert_eq!(
        query(&mut socket, "DELETE FROM public.pg_scalar_03 WHERE id=4")[0].0,
        b'C'
    );
    for transaction in [false, true] {
        for (id, sql, state) in [
            ("where_int", "SELECT 1 WHERE 1", "42804"),
            ("where_paren", "SELECT 1 WHERE (1)", "42804"),
            ("where_negative", "SELECT 1 WHERE -1", "42804"),
            ("where_sum", "SELECT 1 WHERE 1+0", "42804"),
            (
                "insert_int",
                "INSERT INTO public.pg_scalar_03 VALUES (4,2,0)",
                "42804",
            ),
            ("text_invalid", "SELECT '2'::text::boolean", "22P02"),
            ("int8_bool", "SELECT 2::int8::boolean", "0A000"),
        ] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            let statement_name = format!("{id}_{transaction}");
            parse(&mut socket, &statement_name, sql, &[]);
            let response = read(&mut socket);
            if id == "text_invalid" {
                assert_eq!(response.0, b'1', "{id}: rejection must occur at Execute");
                bind(&mut socket, "", &statement_name, &[]);
                assert_eq!(read(&mut socket).0, b'2', "{id}");
                execute(&mut socket, "", 0);
                assert_state(&read(&mut socket), state);
            } else {
                assert_state(&response, state);
            }
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
            }
            assert_eq!(
                query(&mut socket, "SELECT count(*) FROM public.pg_scalar_03")[1],
                (b'D', row(&[Some("3")]))
            );
        }
    }
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    mysql_cast_check(&mut mysql);
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_scalar_03")[0].0,
        b'C'
    );
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    mysql_cast_check(&mut mysql);
    assert_eq!(
        mysql_query(&mut mysql, "SELECT CAST(2 AS SIGNED),CAST(TRUE AS CHAR)"),
        vec![vec![Some(b"2".to_vec()), Some(b"1".to_vec())]]
    );
    send(&mut socket, b'X', &[]);
    println!(
        "pg_scalar_03: 17 scalar/column, 30 parameter, 2 empty descriptions, 14 errors/recovery, MySQL before/during/after"
    );
}

#[test]
fn pg_scalar_05() {
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
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    let mysql_metadata = scalar_mysql_check(&mut mysql);
    let mysql_integer_check = |socket: &mut TcpStream| {
        mysql_packet(
            socket,
            0,
            b"\x03SELECT CAST(-32769 AS SIGNED),CAST(32768 AS SIGNED)",
        );
        assert_eq!(mysql_read(socket), vec![2]);
        let columns = vec![mysql_read(socket), mysql_read(socket)];
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x06-32769\x0532768");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_integer_metadata = mysql_integer_check(&mut mysql);
    let mut socket = TcpStream::connect(cleanup.0.postgres_listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0test\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "CREATE TABLE public.pg_scalar_05 (id INT PRIMARY KEY,s SMALLINT,i INT,b BIGINT)",
        "INSERT INTO public.pg_scalar_05 VALUES (1,-32768,-32768,-32768),(2,32767,32767,32767),(3,NULL,NULL,NULL)",
        "CREATE TABLE public.pg_scalar_05_alias (s INT2)",
        "DROP TABLE public.pg_scalar_05_alias",
    ] {
        let messages = query(&mut socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
    }
    for (id, sql, expected, empty) in [
        (
            "small_min",
            "SELECT (-32768)::smallint",
            Some("-32768"),
            false,
        ),
        ("int2_max", "SELECT 32767::int2", Some("32767"), false),
        (
            "cast_min",
            "SELECT CAST(-32768 AS smallint)",
            Some("-32768"),
            false,
        ),
        (
            "cast_max",
            "SELECT CAST(32767 AS int2)",
            Some("32767"),
            false,
        ),
        (
            "int4_narrow",
            "SELECT 32767::int4::int2",
            Some("32767"),
            false,
        ),
        (
            "int8_narrow",
            "SELECT (-32768)::int8::int2",
            Some("-32768"),
            false,
        ),
        ("typed_null", "SELECT NULL::int2", None, false),
        ("null_narrow", "SELECT NULL::int8::smallint", None, false),
        (
            "column_min",
            "SELECT i::int2 FROM public.pg_scalar_05 WHERE id=1",
            Some("-32768"),
            false,
        ),
        (
            "column_max",
            "SELECT b::smallint FROM public.pg_scalar_05 WHERE id=2",
            Some("32767"),
            false,
        ),
        (
            "column_null",
            "SELECT s FROM public.pg_scalar_05 WHERE id=3",
            None,
            false,
        ),
        (
            "empty_column",
            "SELECT i::int2 FROM public.pg_scalar_05 LIMIT 0",
            None,
            true,
        ),
        ("empty_null", "SELECT NULL::int2 LIMIT 0", None, true),
    ] {
        println!("pg_scalar_05 scenario {id}");
        parse(&mut socket, id, sql, &[]);
        assert_eq!(read(&mut socket), (b'1', vec![]), "{id}");
        send(&mut socket, b'D', &[b"S", id.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket), (b't', 0i16.to_be_bytes().to_vec()));
        let description = read(&mut socket);
        scalar_types(&description, &[21]);
        bind(&mut socket, "", id, &[]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), description);
        execute(&mut socket, "", 0);
        if !empty {
            assert_eq!(read(&mut socket), (b'D', row(&[expected])), "{id}");
        }
        assert_eq!(
            read(&mut socket),
            (
                b'C',
                if empty {
                    b"SELECT 0\0".to_vec()
                } else {
                    b"SELECT 1\0".to_vec()
                }
            ),
            "{id}"
        );
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(mysql_integer_check(&mut mysql), mysql_integer_metadata);
    for (name, sql, supplied, oid) in [
        ("inferred", "SELECT $1::smallint", vec![], 21u32),
        ("int2", "SELECT $1::int2", vec![21], 21),
        ("int4", "SELECT $1::int2", vec![23], 23),
        ("int8", "SELECT CAST($1 AS smallint)", vec![20], 20),
        ("direct", "SELECT $1", vec![21], 21),
    ] {
        parse(&mut socket, name, sql, &supplied);
        assert_eq!(read(&mut socket).0, b'1', "{name}");
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &oid.to_be_bytes()].concat()
            ),
            "{name}"
        );
        let description = read(&mut socket);
        scalar_types(&description, &[21]);
        for value in [Some("-32768"), Some("32767"), None] {
            bind(&mut socket, "", name, &[value]);
            assert_eq!(read(&mut socket).0, b'2', "{name} {value:?}");
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), description);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[value])), "{name} {value:?}");
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket).0, b'Z');
        }
        for transaction in [false, true] {
            for value in ["-32769", "32768"] {
                if transaction {
                    assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
                }
                bind(&mut socket, "", name, &[Some(value)]);
                let response = read(&mut socket);
                if oid != 21 {
                    assert_eq!(response.0, b'2', "{name}");
                    execute(&mut socket, "", 0);
                    assert_state(&read(&mut socket), "22003");
                } else {
                    assert_state(&response, "22003");
                }
                send(&mut socket, b'S', &[]);
                assert_eq!(
                    read(&mut socket),
                    (b'Z', vec![if transaction { b'E' } else { b'I' }])
                );
                if transaction {
                    assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
                }
                assert_eq!(
                    query(&mut socket, "SELECT count(*) FROM public.pg_scalar_05")[1],
                    (b'D', row(&[Some("3")]))
                );
            }
        }
    }
    for transaction in [false, true] {
        for (id, sql) in [
            ("lower", "SELECT (-32769)::smallint"),
            ("upper", "SELECT CAST(32768 AS int2)"),
            ("lower4", "SELECT (-32769)::int4::int2"),
            ("upper8", "SELECT 32768::int8::smallint"),
            (
                "write",
                "INSERT INTO public.pg_scalar_05 VALUES (4,32768::int2,0,0)",
            ),
        ] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            let name = format!("{id}_{transaction}");
            parse(&mut socket, &name, sql, &[]);
            assert_eq!(read(&mut socket).0, b'1', "{id}");
            bind(&mut socket, "", &name, &[]);
            assert_eq!(read(&mut socket).0, b'2');
            execute(&mut socket, "", 0);
            assert_state(&read(&mut socket), "22003");
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
            }
            assert_eq!(
                query(&mut socket, "SELECT count(*) FROM public.pg_scalar_05")[1],
                (b'D', row(&[Some("3")]))
            );
        }
    }
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_scalar_05")[0].0,
        b'C'
    );
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(mysql_integer_check(&mut mysql), mysql_integer_metadata);
    send(&mut socket, b'X', &[]);
    println!(
        "pg_scalar_05: 13 scalar/column/empty, 15 parameter values, 30 errors/recovery, MySQL before/during/after"
    );
}

#[test]
fn pg_scalar_06() {
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
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    let mysql_metadata = scalar_mysql_check(&mut mysql);
    let mysql_integer_check = |socket: &mut TcpStream| {
        mysql_packet(
            socket,
            0,
            b"\x03SELECT CAST(-32769 AS SIGNED),CAST(32768 AS SIGNED),CAST(' +00042 ' AS SIGNED)",
        );
        assert_eq!(mysql_read(socket), vec![3]);
        let columns = vec![mysql_read(socket), mysql_read(socket), mysql_read(socket)];
        assert_eq!(mysql_read(socket)[0], 0xfe);
        // Preserve the current MySQL text-CAST result as well as its metadata.
        assert_eq!(mysql_read(socket), b"\x06-32769\x0532768\x08 +00042 ");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_integer_metadata = mysql_integer_check(&mut mysql);
    let mut socket = TcpStream::connect(cleanup.0.postgres_listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0test\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}

    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE public.pg_scalar_06 (id INT PRIMARY KEY,v TEXT)"
        )[0]
        .0,
        b'C'
    );
    let mut success = 0;
    let mut errors = 0;
    for (ty, oid, low, high, below, above) in [
        ("int2", 21u32, "-32768", "32767", "-32769", "32768"),
        (
            "int4",
            23,
            "-2147483648",
            "2147483647",
            "-2147483649",
            "2147483648",
        ),
        (
            "int8",
            20,
            "-9223372036854775808",
            "9223372036854775807",
            "-9223372036854775809",
            "9223372036854775808",
        ),
    ] {
        let bits = match oid {
            21 => 16,
            23 => 32,
            _ => 64,
        };
        let magnitude = 1u64 << (bits - 1);
        let endpoints = [
            format!("-0x{magnitude:x}"),
            format!("0x{:x}", magnitude - 1),
            format!("-0o{magnitude:o}"),
            format!("0o{:o}", magnitude - 1),
            format!("-0b{magnitude:b}"),
            format!("0b{:b}", magnitude - 1),
        ];
        let overflow = [
            format!("0x{magnitude:x}"),
            format!("-0x{:x}", magnitude + 1),
            format!("0o{magnitude:o}"),
            format!("-0o{:o}", magnitude + 1),
            format!("0b{magnitude:b}"),
            format!("-0b{:b}", magnitude + 1),
        ];
        let mut values = vec![
            (Some(" \t+42\n"), Some("42")),
            (Some("-0"), Some("0")),
            (Some("0x2a"), Some("42")),
            (Some("-0X2A"), Some("-42")),
            (Some("+0o52"), Some("42")),
            (Some("0O_52"), Some("42")),
            (Some("0b10_1010"), Some("42")),
            (Some("0B_101010"), Some("42")),
            (Some("0x_2_a"), Some("42")),
            (Some("0_042"), Some("42")),
            (Some("00042"), Some("42")),
            (Some(low), Some(low)),
            (Some(high), Some(high)),
            (None, None),
        ];
        for (index, value) in endpoints.iter().enumerate() {
            values.push((
                Some(value.as_str()),
                Some(if index % 2 == 0 { low } else { high }),
            ));
        }
        if oid == 20 {
            values.push((Some("9_007_199_254_740_993"), Some("9007199254740993")));
        }
        for (value, expected) in values {
            for path in ["literal", "column", "inferred", "explicit"] {
                println!("pg_scalar_06 {ty} {path} {value:?}");
                let text = value
                    .map(|v| format!("'{}'", v.replace('\'', "''")))
                    .unwrap_or("NULL".into());
                let sql = match path {
                    "literal" => format!("SELECT {text}::{ty}"),
                    "column" => {
                        assert_eq!(
                            query(&mut socket, "DELETE FROM public.pg_scalar_06")[0].0,
                            b'C'
                        );
                        assert_eq!(
                            query(
                                &mut socket,
                                &format!("INSERT INTO public.pg_scalar_06 VALUES (1,{text})")
                            )[0]
                            .0,
                            b'C'
                        );
                        format!("SELECT v::{ty} FROM public.pg_scalar_06")
                    }
                    _ => format!("SELECT $1::{ty}"),
                };
                parse(
                    &mut socket,
                    "valid",
                    &sql,
                    if path == "explicit" {
                        std::slice::from_ref(&oid)
                    } else {
                        &[]
                    },
                );
                let parsed = read(&mut socket);
                assert_eq!(parsed.0, b'1', "{sql}: {parsed:?}");
                send(&mut socket, b'D', b"Svalid\0");
                let parameter_oids = read(&mut socket);
                let parameter_path = matches!(path, "inferred" | "explicit");
                assert_eq!(
                    parameter_oids,
                    (
                        b't',
                        if parameter_path {
                            [1i16.to_be_bytes().as_slice(), &oid.to_be_bytes()].concat()
                        } else {
                            0i16.to_be_bytes().to_vec()
                        }
                    )
                );
                let description = read(&mut socket);
                scalar_types(&description, &[oid]);
                bind(
                    &mut socket,
                    "",
                    "valid",
                    if parameter_path {
                        std::slice::from_ref(&value)
                    } else {
                        &[]
                    },
                );
                assert_eq!(read(&mut socket).0, b'2');
                send(&mut socket, b'D', b"P\0");
                assert_eq!(read(&mut socket), description);
                execute(&mut socket, "", 0);
                assert_eq!(read(&mut socket), (b'D', row(&[expected])));
                assert_eq!(read(&mut socket).0, b'C');
                send(&mut socket, b'S', &[]);
                assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
                send(&mut socket, b'C', b"Svalid\0");
                assert_eq!(read(&mut socket).0, b'3');
                success += 1;
            }
        }
        let empty = query(&mut socket, &format!("SELECT NULL::{ty} LIMIT 0"));
        scalar_types(&empty[0], &[oid]);
        assert!(!empty.iter().any(|m| matches!(m.0, b'D' | b'E')));
        assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
        assert_eq!(mysql_integer_check(&mut mysql), mysql_integer_metadata);
        let invalid = [
            "", " ", "+", "-", "_1", "1_", "1__0", "0x", "0x_", "0x__1", "0o8", "0b2", "0xg", "1x",
            "1.5", "+ 1", "1 2", "\u{a0}1",
        ];
        for (value, state) in invalid
            .into_iter()
            .map(|v| (v, "22P02"))
            .chain([below, above].into_iter().map(|v| (v, "22003")))
            .chain(overflow.iter().map(|v| (v.as_str(), "22003")))
        {
            for transaction in [false, true] {
                for path in ["parameter", "literal", "write"] {
                    if transaction {
                        assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
                    }
                    let sql = match path {
                        "parameter" => format!("SELECT $1::{ty}"),
                        "literal" => format!("SELECT '{value}'::{ty}"),
                        _ => format!(
                            "INSERT INTO public.pg_scalar_06 VALUES (2,('{value}'::{ty})::text)"
                        ),
                    };
                    parse(&mut socket, "bad", &sql, &[]);
                    assert_eq!(read(&mut socket).0, b'1', "{sql}");
                    let bad_values = [Some(value)];
                    bind(
                        &mut socket,
                        "",
                        "bad",
                        if path == "parameter" {
                            &bad_values
                        } else {
                            &[]
                        },
                    );
                    if path != "parameter" {
                        assert_eq!(read(&mut socket).0, b'2');
                        execute(&mut socket, "", 0);
                    }
                    assert_state(&read(&mut socket), state);
                    send(&mut socket, b'S', &[]);
                    assert_eq!(
                        read(&mut socket),
                        (b'Z', vec![if transaction { b'E' } else { b'I' }])
                    );
                    if transaction {
                        assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
                    }
                    assert_eq!(
                        query(
                            &mut socket,
                            "SELECT count(*) FROM public.pg_scalar_06 WHERE id=2"
                        )[1],
                        (b'D', row(&[Some("0")]))
                    );
                    send(&mut socket, b'C', b"Sbad\0");
                    assert_eq!(read(&mut socket).0, b'3');
                    errors += 1;
                }
            }
        }
    }
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_scalar_06")[0].0,
        b'C'
    );
    assert_eq!(scalar_mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(mysql_integer_check(&mut mysql), mysql_integer_metadata);
    send(&mut socket, b'X', &[]);
    println!(
        "pg_scalar_06: {success} value/description cases, {errors} SQLSTATE/recovery/no-write cases, 3 empty descriptions, MySQL before/during/after"
    );
}
