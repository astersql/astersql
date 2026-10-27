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
    assert_eq!(parameter(1082, Some(b"2026-02-29")).unwrap_err().0, "22008");
    assert!(parameter(1082, Some(b"2024-02-29")).is_ok());
    assert!(parameter(1083, Some(b"24:00:00")).is_ok());
    assert!(parameter(1114, Some(b"2026-09-30 12:30:00.123456")).is_ok());
    let rounded = parameter(1114, Some(b"2026-09-30 12:30:00.1234567")).unwrap();
    assert_eq!(rounded.tp, 253);
    assert_eq!(rounded.value, b"2026-09-30 12:30:00.123457");
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
        ("SELECT NULL::timestamp(-1)", vec![1114], "42601"),
        ("SELECT CAST(NULL AS TIMESTAMP(-1))", vec![1114], "42601"),
        ("SELECT NULL::timestamp(0)", vec![1114], "08P01"),
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
            postgresql_text_oid: None,
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
                20 | 701 => 8,
                25 | 1042 | 1043 | 1700 => -1,
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

#[test]
fn pg_scalar_07() {
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
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT CAST('12.340' AS DECIMAL(6,3)),NULL");
        assert_eq!(mysql_read(socket), vec![2]);
        let columns = vec![mysql_read(socket), mysql_read(socket)];
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x0612.340\xfb");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_metadata = mysql_check(&mut mysql);
    let mut socket = TcpStream::connect(cleanup.0.postgres_listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
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
    // Set up shared storage through MySQL: PG TEXT currently has a 64KiB
    // storage bound, which belongs to the separate text-family task.
    mysql_query(&mut mysql, "CREATE DATABASE IF NOT EXISTS test");
    mysql_query(
        &mut mysql,
        "CREATE TABLE test.pg_scalar_07 (id INT PRIMARY KEY,v LONGTEXT)",
    );
    let start = std::time::Instant::now();
    let mut successes = 0;
    for value in [
        Some("12.340".to_owned()),
        Some("-12.340".to_owned()),
        Some("-9007199254740993".to_owned()),
        Some("9007199254740993".to_owned()),
        Some("18446744073709551615".into()),
        Some("9".repeat(82)),
        Some("9".repeat(1000)),
        Some("9".repeat(131072)),
        Some(format!("{}.{}", "9".repeat(131072), "9".repeat(16383))),
        Some(format!("0.{}1", "0".repeat(16382))),
        None,
    ] {
        let value = value.as_deref();
        let text = value.map(|v| format!("'{v}'")).unwrap_or("NULL".into());
        // Keep the initial red inputs and assertions at the simple SQL entrance.
        let messages = query(&mut socket, &format!("SELECT {text}::numeric"));
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "numeric length {:?}: {:?}",
            value.map(str::len),
            messages.iter().filter(|m| m.0 == b'E').collect::<Vec<_>>()
        );
        scalar_types(&messages[0], &[1700]);
        assert_eq!(messages[1], (b'D', row(&[value])));
        successes += 1;
        if let Some(integer) = value.and_then(|v| v.parse::<i64>().ok()) {
            let actual = query(&mut socket, &format!("SELECT {integer}::numeric"));
            scalar_types(&actual[0], &[1700]);
            assert_eq!(actual[1], (b'D', row(&[value])));
            successes += 1;
        }

        for path in [
            "literal", "column", "inferred", "explicit", "cast", "decimal",
        ] {
            let sql = match path {
                "literal" => format!("SELECT {text}::numeric"),
                "cast" => format!("SELECT CAST({text} AS numeric)"),
                "decimal" => format!("SELECT {text}::decimal"),
                "column" => {
                    assert_eq!(
                        query(&mut socket, "DELETE FROM public.pg_scalar_07")[0].0,
                        b'C'
                    );
                    assert_eq!(
                        query(
                            &mut socket,
                            &format!("INSERT INTO public.pg_scalar_07 VALUES(1,{text})")
                        )[0]
                        .0,
                        b'C'
                    );
                    "SELECT v::numeric FROM public.pg_scalar_07".into()
                }
                _ => "SELECT $1::numeric".into(),
            };
            let parameter = matches!(path, "inferred" | "explicit");
            println!("pg_scalar_07 {path} length={:?}", value.map(str::len));
            parse(
                &mut socket,
                "valid",
                &sql,
                if path == "explicit" { &[1700] } else { &[] },
            );
            assert_eq!(read(&mut socket).0, b'1');
            send(&mut socket, b'D', b"Svalid\0");
            assert_eq!(
                read(&mut socket),
                (
                    b't',
                    if parameter {
                        [1i16.to_be_bytes().as_slice(), &1700u32.to_be_bytes()].concat()
                    } else {
                        0i16.to_be_bytes().to_vec()
                    }
                )
            );
            let description = read(&mut socket);
            scalar_types(&description, &[1700]);
            bind(
                &mut socket,
                "",
                "valid",
                if parameter {
                    std::slice::from_ref(&value)
                } else {
                    &[]
                },
            );
            assert_eq!(read(&mut socket).0, b'2');
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), description);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[value])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
            send(&mut socket, b'C', b"Svalid\0");
            assert_eq!(read(&mut socket).0, b'3');
            successes += 1;
        }
        assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    }
    let empty = query(&mut socket, "SELECT NULL::numeric LIMIT 0");
    scalar_types(&empty[0], &[1700]);
    assert!(!empty.iter().any(|m| matches!(m.0, b'D' | b'E')));
    let mut errors = 0;
    for value in ["9".repeat(131073), format!("0.{}1", "0".repeat(16383))] {
        for transaction in [false, true] {
            for path in ["parameter", "literal", "write"] {
                if transaction {
                    assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
                }
                let sql = match path {
                    "parameter" => "SELECT $1::numeric".into(),
                    "literal" => format!("SELECT '{value}'::numeric"),
                    _ => format!(
                        "INSERT INTO public.pg_scalar_07 VALUES(2,('{value}'::numeric)::text)"
                    ),
                };
                parse(&mut socket, "bad", &sql, &[]);
                assert_eq!(read(&mut socket).0, b'1');
                let bad_values = [Some(value.as_str())];
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
                    query(
                        &mut socket,
                        "SELECT count(*) FROM public.pg_scalar_07 WHERE id=2"
                    )[1],
                    (b'D', row(&[Some("0")]))
                );
                send(&mut socket, b'C', b"Sbad\0");
                assert_eq!(read(&mut socket).0, b'3');
                errors += 1;
            }
        }
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_scalar_07")[0].0,
        b'C'
    );
    send(&mut socket, b'X', &[]);
    println!(
        "pg_scalar_07: {successes} value/metadata cases, {errors} range/recovery/no-write cases, 1 empty description; endpoint sizes 131072+16383 digits; elapsed {:?}",
        start.elapsed()
    );
}

#[test]
fn pg_scalar_08() {
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
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT CAST('12.340' AS DECIMAL(6,3)),NULL");
        assert_eq!(mysql_read(socket), vec![2]);
        let columns = vec![mysql_read(socket), mysql_read(socket)];
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x0612.340\xfb");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_metadata = mysql_check(&mut mysql);
    let mut socket = TcpStream::connect(cleanup.0.postgres_listener_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
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
    // Set up shared storage through MySQL: PG TEXT currently has a 64KiB
    // storage bound, which belongs to the separate text-family task.
    mysql_query(&mut mysql, "CREATE DATABASE IF NOT EXISTS test");
    mysql_query(
        &mut mysql,
        "CREATE TABLE test.pg_scalar_08 (id INT PRIMARY KEY,v LONGTEXT)",
    );
    let start = std::time::Instant::now();
    let mut successes = 0;
    for (value, expected) in [
        (Some(" +12.340 "), Some("12.340")),
        (Some("\t-12.340\n"), Some("-12.340")),
        (Some("1e3"), Some("1000")),
        (Some("1e-3"), Some("0.001")),
        (Some("12.340e+1"), Some("123.40")),
        (Some("+.5"), Some("0.5")),
        (Some("1."), Some("1")),
        (Some("+0"), Some("0")),
        (Some("-0.000"), Some("0.000")),
        (Some("00012.3400"), Some("12.3400")),
        (None, None),
    ] {
        let text = value.map(|v| format!("'{v}'")).unwrap_or("NULL".into());
        // Exercise normalization at the simple SQL entrance before prepared paths.
        let messages = query(&mut socket, &format!("SELECT {text}::numeric"));
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "numeric length {:?}: {:?}",
            value.map(str::len),
            messages.iter().filter(|m| m.0 == b'E').collect::<Vec<_>>()
        );
        scalar_types(&messages[0], &[1700]);
        assert_eq!(messages[1], (b'D', row(&[expected])));
        let canonical = expected.map(|v| format!("'{v}'")).unwrap_or("NULL".into());
        let reread = query(&mut socket, &format!("SELECT {canonical}::numeric"));
        scalar_types(&reread[0], &[1700]);
        assert_eq!(reread[1], (b'D', row(&[expected])));
        successes += 1;
        if let Some(integer) = value.and_then(|v| v.parse::<i64>().ok()) {
            let actual = query(&mut socket, &format!("SELECT {integer}::numeric"));
            scalar_types(&actual[0], &[1700]);
            assert_eq!(actual[1], (b'D', row(&[expected])));
            successes += 1;
        }

        for path in [
            "literal", "column", "inferred", "explicit", "cast", "decimal",
        ] {
            let sql = match path {
                "literal" => format!("SELECT {text}::numeric"),
                "cast" => format!("SELECT CAST({text} AS numeric)"),
                "decimal" => format!("SELECT {text}::decimal"),
                "column" => {
                    assert_eq!(
                        query(&mut socket, "DELETE FROM public.pg_scalar_08")[0].0,
                        b'C'
                    );
                    assert_eq!(
                        query(
                            &mut socket,
                            &format!("INSERT INTO public.pg_scalar_08 VALUES(1,{text})")
                        )[0]
                        .0,
                        b'C'
                    );
                    "SELECT v::numeric FROM public.pg_scalar_08".into()
                }
                _ => "SELECT $1::numeric".into(),
            };
            let parameter = matches!(path, "inferred" | "explicit");
            println!("pg_scalar_08 {path} length={:?}", value.map(str::len));
            parse(
                &mut socket,
                "valid",
                &sql,
                if path == "explicit" { &[1700] } else { &[] },
            );
            assert_eq!(read(&mut socket).0, b'1');
            send(&mut socket, b'D', b"Svalid\0");
            assert_eq!(
                read(&mut socket),
                (
                    b't',
                    if parameter {
                        [1i16.to_be_bytes().as_slice(), &1700u32.to_be_bytes()].concat()
                    } else {
                        0i16.to_be_bytes().to_vec()
                    }
                )
            );
            let description = read(&mut socket);
            scalar_types(&description, &[1700]);
            bind(
                &mut socket,
                "",
                "valid",
                if parameter {
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
            successes += 1;
        }
        assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    }
    let empty = query(&mut socket, "SELECT NULL::numeric LIMIT 0");
    scalar_types(&empty[0], &[1700]);
    assert!(!empty.iter().any(|m| matches!(m.0, b'D' | b'E')));
    let mut errors = 0;
    for (value, state) in [
        ("".to_owned(), "22P02"),
        ("12bad".to_owned(), "22P02"),
        ("1e".to_owned(), "22P02"),
        ("1e+".to_owned(), "22P02"),
        ("1e131072".to_owned(), "22003"),
        ("1e-16384".to_owned(), "22003"),
    ] {
        for transaction in [false, true] {
            for path in ["parameter", "literal", "write"] {
                if transaction {
                    assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
                }
                let sql = match path {
                    "parameter" => "SELECT $1::numeric".into(),
                    "literal" => format!("SELECT '{value}'::numeric"),
                    _ => format!(
                        "INSERT INTO public.pg_scalar_08 VALUES(2,('{value}'::numeric)::text)"
                    ),
                };
                parse(&mut socket, "bad", &sql, &[]);
                assert_eq!(read(&mut socket).0, b'1');
                let bad_values = [Some(value.as_str())];
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
                        "SELECT count(*) FROM public.pg_scalar_08 WHERE id=2"
                    )[1],
                    (b'D', row(&[Some("0")]))
                );
                send(&mut socket, b'C', b"Sbad\0");
                assert_eq!(read(&mut socket).0, b'3');
                errors += 1;
            }
        }
    }
    parse(
        &mut socket,
        "encoding",
        "INSERT INTO public.pg_scalar_08 VALUES(2,$1)",
        &[1700],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        for input in [b"\xff".as_slice(), b"1\0"] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            bind_formats(&mut socket, "encoding", &[], &[Some(input)]);
            assert_state(&read(&mut socket), "22021");
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
                    "SELECT count(*) FROM public.pg_scalar_08 WHERE id=2"
                )[1],
                (b'D', row(&[Some("0")]))
            );
            errors += 1;
        }
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(
        query(&mut socket, "DROP TABLE public.pg_scalar_08")[0].0,
        b'C'
    );
    send(&mut socket, b'X', &[]);
    println!(
        "pg_scalar_08: {successes} value/metadata cases, {errors} range/recovery/no-write cases, 1 empty description; finite text grammar and normalization; elapsed {:?}",
        start.elapsed()
    );
}

#[test]
fn pg_scalar_09() {
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
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT CAST('12.340' AS DECIMAL(6,3)),NULL");
        assert_eq!(mysql_read(socket), vec![2]);
        let columns = vec![mysql_read(socket), mysql_read(socket)];
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x0612.340\xfb");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    fn description(message: &(u8, Vec<u8>), p: i32, scale: i32) {
        let mut unconstrained = message.clone();
        let offset = 2 + message.1[2..].iter().position(|b| *b == 0).unwrap() + 1;
        let typmod = ((p << 16) | (scale & 0x7ff)) + 4;
        assert_eq!(&message.1[offset + 12..offset + 16], &typmod.to_be_bytes());
        unconstrained.1[offset + 12..offset + 16].copy_from_slice(&(-1i32).to_be_bytes());
        scalar_types(&unconstrained, &[1700]);
    }
    let cases = vec![
        (Some("1.25".to_owned()), 3, 1, Some("1.3".to_owned())),
        (Some("-1.25".into()), 3, 1, Some("-1.3".into())),
        (Some("1250".into()), 2, -2, Some("1300".into())),
        (Some("-1250".into()), 2, -2, Some("-1300".into())),
        (Some("0.000995".into()), 2, 4, Some("0.0010".into())),
        (Some("9.4".into()), 1, 0, Some("9".into())),
        (Some("0".into()), 1, -1000, Some("0".into())),
        (
            Some("5e999".into()),
            1,
            -1000,
            Some(format!("1{}", "0".repeat(1000))),
        ),
        (
            Some("1e-1000".into()),
            1,
            1000,
            Some(format!("0.{}1", "0".repeat(999))),
        ),
        (Some("9".repeat(1000)), 1000, 0, Some("9".repeat(1000))),
        (None, 3, -1, None),
    ];
    let mut count = 0;
    for (value, p, scale, expected) in &cases {
        let target = format!("numeric({p},{scale})");
        let literal = value
            .as_ref()
            .map(|v| format!("'{v}'"))
            .unwrap_or("NULL".into());
        for sql in [
            format!("SELECT {literal}::{target}"),
            format!("SELECT CAST({literal} AS {target})"),
        ] {
            let messages = query(&mut socket, &sql);
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"TDCZ",
                "{sql}: {messages:?}"
            );
            description(&messages[0], *p, *scale);
            assert_eq!(messages[1], (b'D', row(&[expected.as_deref()])), "{sql}");
            count += 1;
        }
        for explicit in [false, true] {
            parse(
                &mut socket,
                "n9",
                &format!("SELECT $1::{target}"),
                if explicit { &[1700] } else { &[] },
            );
            assert_eq!(read(&mut socket).0, b'1');
            send(&mut socket, b'D', b"Sn9\0");
            assert_eq!(
                read(&mut socket),
                (
                    b't',
                    [1i16.to_be_bytes().as_slice(), &1700u32.to_be_bytes()].concat()
                )
            );
            let desc = read(&mut socket);
            description(&desc, *p, *scale);
            bind(&mut socket, "", "n9", &[value.as_deref()]);
            assert_eq!(read(&mut socket).0, b'2');
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), desc);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[expected.as_deref()])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
            send(&mut socket, b'C', b"Sn9\0");
            assert_eq!(read(&mut socket).0, b'3');
            count += 1;
        }
        let empty = query(
            &mut socket,
            &format!("SELECT {literal}::{target} WHERE FALSE"),
        );
        assert_eq!(
            empty.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TCZ",
            "{empty:?}"
        );
        description(&empty[0], *p, *scale);
        count += 1;
    }
    for sql in ["SELECT '1.5'::numeric(2)", "SELECT '-1.5'::numeric(2)"] {
        let m = query(&mut socket, sql);
        description(&m[0], 2, 0);
        assert_eq!(
            m[1],
            (
                b'D',
                row(&[Some(if sql.contains("-1") { "-2" } else { "2" })])
            )
        );
        count += 1;
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "CREATE DATABASE IF NOT EXISTS test");
    mysql_query(
        &mut mysql,
        "CREATE TABLE test.pg_scalar_09 (id INT PRIMARY KEY,v LONGTEXT)",
    );
    let mut errors = 0;
    for (expression, state) in [
        ("'99.95'::numeric(3,1)", "22003"),
        ("'-99.95'::numeric(3,1)", "22003"),
        ("NULL::numeric(0,0)", "22023"),
        ("NULL::numeric(1001,0)", "22023"),
        ("NULL::numeric(1,-1001)", "22023"),
        ("NULL::numeric(1,1001)", "22023"),
    ] {
        for sql in [
            format!("SELECT {expression}"),
            format!("SELECT {expression} WHERE FALSE"),
            format!("INSERT INTO public.pg_scalar_09 VALUES (1,({expression})::text)"),
        ] {
            // Empty scans only validate the declaration, not an unevaluated value.
            if state == "22003" && sql.ends_with("FALSE") {
                continue;
            }
            let m = query(&mut socket, &sql);
            assert_eq!(m[0].0, b'E', "{sql}: {m:?}");
            assert!(
                m[0].1
                    .windows(7)
                    .any(|v| v == format!("C{state}\0").as_bytes()),
                "{sql}: {m:?}"
            );
            assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
            errors += 1;
        }
        if state == "22023" {
            parse(
                &mut socket,
                "invalid9",
                &format!("SELECT {expression} WHERE FALSE"),
                &[],
            );
            assert_state(&read(&mut socket), "22023");
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
            errors += 1;
        }
    }
    for value in ["99.95", "-99.95"] {
        for transaction in [false, true] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            parse(&mut socket, "overflow9", "SELECT $1::numeric(3,1)", &[1700]);
            assert_eq!(read(&mut socket).0, b'1');
            bind(&mut socket, "", "overflow9", &[Some(value)]);
            let bound = read(&mut socket);
            let error = if bound.0 == b'2' {
                execute(&mut socket, "", 0);
                read(&mut socket)
            } else {
                bound
            };
            assert_state(&error, "22003");
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                query(&mut socket, "ROLLBACK");
            }
            assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
            send(&mut socket, b'C', b"Soverflow9\0");
            assert_eq!(read(&mut socket).0, b'3');
            errors += 1;
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_09")[1],
        (b'D', row(&[Some("0")]))
    );
    assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
    assert_eq!(
        query(&mut socket, "SELECT '99.95'::numeric(3,1)")[0].0,
        b'E'
    );
    assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
    assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
    query(&mut socket, "DROP TABLE public.pg_scalar_09");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    println!(
        "pg_scalar_09: {count} value/metadata/NULL/empty cases, {errors} error/recovery/no-write cases"
    );
}

#[test]
fn pg_scalar_10() {
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
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT CAST('12.340' AS DECIMAL(6,3)),NULL");
        assert_eq!(mysql_read(socket), vec![2]);
        let columns = vec![mysql_read(socket), mysql_read(socket)];
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x0612.340\xfb");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    fn description(message: &(u8, Vec<u8>), p: i32, scale: i32) {
        let mut unconstrained = message.clone();
        let offset = 2 + message.1[2..].iter().position(|b| *b == 0).unwrap() + 1;
        let typmod = if p == -1 {
            -1
        } else {
            ((p << 16) | (scale & 0x7ff)) + 4
        };
        assert_eq!(&message.1[offset + 12..offset + 16], &typmod.to_be_bytes());
        unconstrained.1[offset + 12..offset + 16].copy_from_slice(&(-1i32).to_be_bytes());
        scalar_types(&unconstrained, &[1700]);
    }
    let cases = vec![
        (Some("NaN".to_owned()), -1, -1, Some("NaN".to_owned())),
        (Some(" nan ".into()), 3, 1, Some("NaN".into())),
        (Some("NaN".into()), 1, -1000, Some("NaN".into())),
        (Some("NaN".into()), 1, 1000, Some("NaN".into())),
        (Some("Infinity".into()), -1, -1, Some("Infinity".into())),
        (Some("+iNf".into()), -1, -1, Some("Infinity".into())),
        (Some("-Infinity".into()), -1, -1, Some("-Infinity".into())),
        (Some(" -inf ".into()), -1, -1, Some("-Infinity".into())),
        (None, -1, -1, None),
        (None, 3, 1, None),
    ];
    let mut count = 0;
    for (value, p, scale, expected) in &cases {
        let target = if *p == -1 {
            "numeric".into()
        } else {
            format!("numeric({p},{scale})")
        };
        let literal = value
            .as_ref()
            .map(|v| format!("'{v}'"))
            .unwrap_or("NULL".into());
        for sql in [
            format!("SELECT {literal}::{target}"),
            format!("SELECT CAST({literal} AS {target})"),
        ] {
            let messages = query(&mut socket, &sql);
            assert_eq!(
                messages.iter().map(|m| m.0).collect::<Vec<_>>(),
                b"TDCZ",
                "{sql}: {messages:?}"
            );
            description(&messages[0], *p, *scale);
            assert_eq!(messages[1], (b'D', row(&[expected.as_deref()])), "{sql}");
            count += 1;
        }
        for explicit in [false, true] {
            parse(
                &mut socket,
                "n10",
                &format!("SELECT $1::{target}"),
                if explicit { &[1700] } else { &[] },
            );
            assert_eq!(read(&mut socket).0, b'1');
            send(&mut socket, b'D', b"Sn10\0");
            assert_eq!(
                read(&mut socket),
                (
                    b't',
                    [1i16.to_be_bytes().as_slice(), &1700u32.to_be_bytes()].concat()
                )
            );
            let desc = read(&mut socket);
            description(&desc, *p, *scale);
            bind(&mut socket, "", "n10", &[value.as_deref()]);
            assert_eq!(read(&mut socket).0, b'2');
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), desc);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[expected.as_deref()])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
            send(&mut socket, b'C', b"Sn10\0");
            assert_eq!(read(&mut socket).0, b'3');
            count += 1;
        }
        let roundtrip = query(
            &mut socket,
            &format!("SELECT ({literal}::{target})::text::numeric"),
        );
        scalar_types(&roundtrip[0], &[1700]);
        assert_eq!(roundtrip[1], (b'D', row(&[expected.as_deref()])));
        count += 1;
        let empty = query(
            &mut socket,
            &format!("SELECT {literal}::{target} WHERE FALSE"),
        );
        assert_eq!(
            empty.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TCZ",
            "{empty:?}"
        );
        description(&empty[0], *p, *scale);
        count += 1;
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "CREATE DATABASE IF NOT EXISTS test");
    mysql_query(
        &mut mysql,
        "CREATE TABLE test.pg_scalar_10 (id INT PRIMARY KEY,v LONGTEXT)",
    );
    let mut errors = 0;
    for (expression, state) in [
        ("'Infinity'::numeric(3,1)", "22003"),
        ("'-Infinity'::numeric(3,1)", "22003"),
        ("'+NaN'::numeric", "22P02"),
        ("'-NaN'::numeric", "22P02"),
    ] {
        for sql in [
            format!("SELECT {expression}"),
            format!("SELECT {expression} WHERE FALSE"),
            format!("INSERT INTO public.pg_scalar_10 VALUES (1,({expression})::text)"),
        ] {
            // Empty scans do not evaluate value conversion errors.
            if sql.ends_with("FALSE") {
                continue;
            }
            let m = query(&mut socket, &sql);
            assert_eq!(m[0].0, b'E', "{sql}: {m:?}");
            assert!(
                m[0].1
                    .windows(7)
                    .any(|v| v == format!("C{state}\0").as_bytes()),
                "{sql}: {m:?}"
            );
            assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
            errors += 1;
        }
    }
    for value in ["Infinity", "-Infinity"] {
        for transaction in [false, true] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            parse(
                &mut socket,
                "overflow10",
                "SELECT $1::numeric(3,1)",
                &[1700],
            );
            assert_eq!(read(&mut socket).0, b'1');
            bind(&mut socket, "", "overflow10", &[Some(value)]);
            let bound = read(&mut socket);
            let error = if bound.0 == b'2' {
                execute(&mut socket, "", 0);
                read(&mut socket)
            } else {
                bound
            };
            assert_state(&error, "22003");
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                query(&mut socket, "ROLLBACK");
            }
            assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
            send(&mut socket, b'C', b"Soverflow10\0");
            assert_eq!(read(&mut socket).0, b'3');
            errors += 1;
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_10")[1],
        (b'D', row(&[Some("0")]))
    );
    assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
    assert_eq!(
        query(&mut socket, "SELECT 'Infinity'::numeric(3,1)")[0].0,
        b'E'
    );
    assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
    assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
    query(&mut socket, "DROP TABLE public.pg_scalar_10");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    println!(
        "pg_scalar_10: {count} value/metadata/NULL/empty cases, {errors} error/recovery/no-write cases"
    );
}

#[test]
fn pg_scalar_11() {
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
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT CAST('12.340' AS DECIMAL(6,3)),NULL");
        assert_eq!(mysql_read(socket), vec![2]);
        let columns = vec![mysql_read(socket), mysql_read(socket)];
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x0612.340\xfb");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    let mut count = 0;
    for (target, oid, min, max) in [
        ("int2", 21, "-32768", "32767"),
        ("int4", 23, "-2147483648", "2147483647"),
        ("int8", 20, "-9223372036854775808", "9223372036854775807"),
    ] {
        for (input, expected) in [
            (Some("1.5"), Some("2")),
            (Some("2.5"), Some("3")),
            (Some("-1.5"), Some("-2")),
            (Some("-2.5"), Some("-3")),
            (Some(min), Some(min)),
            (Some(max), Some(max)),
            (None, None),
        ] {
            let literal = input.map(|v| format!("'{v}'")).unwrap_or("NULL".into());
            for sql in [
                format!("SELECT ({literal}::numeric)::{target}"),
                format!("SELECT CAST(CAST({literal} AS numeric) AS {target})"),
            ] {
                let m = query(&mut socket, &sql);
                assert_eq!(
                    m.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"TDCZ",
                    "{sql}: {m:?}"
                );
                scalar_types(&m[0], &[oid]);
                assert_eq!(m[1], (b'D', row(&[expected])), "{sql}");
                count += 1;
            }
            parse(
                &mut socket,
                "n11",
                &format!("SELECT ($1::numeric)::{target}"),
                &[1700],
            );
            assert_eq!(read(&mut socket).0, b'1');
            send(&mut socket, b'D', b"Sn11\0");
            assert_eq!(
                read(&mut socket),
                (
                    b't',
                    [1i16.to_be_bytes().as_slice(), &1700u32.to_be_bytes()].concat()
                )
            );
            let desc = read(&mut socket);
            scalar_types(&desc, &[oid]);
            bind(&mut socket, "", "n11", &[input]);
            assert_eq!(read(&mut socket).0, b'2');
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), desc);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[expected])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
            send(&mut socket, b'C', b"Sn11\0");
            assert_eq!(read(&mut socket).0, b'3');
            let empty = query(
                &mut socket,
                &format!("SELECT ({literal}::numeric)::{target} WHERE FALSE"),
            );
            assert_eq!(empty.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
            scalar_types(&empty[0], &[oid]);
            count += 2;
        }
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "CREATE DATABASE IF NOT EXISTS test");
    mysql_query(
        &mut mysql,
        "CREATE TABLE test.pg_scalar_11 (id INT PRIMARY KEY,v LONGTEXT)",
    );
    let mut errors = 0;
    for (target, upper, lower, inside_upper, inside_lower) in [
        (
            "int2",
            "32767.5",
            "-32768.5",
            "32767.499999999999999999",
            "-32768.499999999999999999",
        ),
        (
            "int4",
            "2147483647.5",
            "-2147483648.5",
            "2147483647.499999999999999999",
            "-2147483648.499999999999999999",
        ),
        (
            "int8",
            "9223372036854775807.5",
            "-9223372036854775808.5",
            "9223372036854775807.499999999999999999",
            "-9223372036854775808.499999999999999999",
        ),
    ] {
        for (input, expected) in [
            (inside_upper, upper.split('.').next().unwrap()),
            (inside_lower, lower.split('.').next().unwrap()),
        ] {
            let m = query(
                &mut socket,
                &format!("SELECT ('{input}'::numeric)::{target}"),
            );
            assert_eq!(m[1], (b'D', row(&[Some(expected)])));
            count += 1;
            let m = query(
                &mut socket,
                &format!("SELECT ('{expected}'::{target})::numeric"),
            );
            scalar_types(&m[0], &[1700]);
            assert_eq!(m[1], (b'D', row(&[Some(expected)])));
            count += 1;
        }
        for input in [upper, lower, "NaN", "Infinity", "-Infinity"] {
            for sql in [
                format!("SELECT ('{input}'::numeric)::{target}"),
                format!(
                    "INSERT INTO public.pg_scalar_11 VALUES (1,(('{input}'::numeric)::{target})::text)"
                ),
            ] {
                let m = query(&mut socket, &sql);
                assert_state(&m[0], "22003");
                assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
                send(&mut socket, b'C', b"Sbad11\0");
                assert_eq!(read(&mut socket).0, b'3');
                errors += 1;
            }
            for transaction in [false, true] {
                if transaction {
                    query(&mut socket, "BEGIN");
                }
                parse(
                    &mut socket,
                    "bad11",
                    &format!("SELECT ($1::numeric)::{target}"),
                    &[1700],
                );
                assert_eq!(read(&mut socket).0, b'1');
                bind(&mut socket, "", "bad11", &[Some(input)]);
                let bound = read(&mut socket);
                let error = if bound.0 == b'2' {
                    execute(&mut socket, "", 0);
                    read(&mut socket)
                } else {
                    bound
                };
                assert_state(&error, "22003");
                send(&mut socket, b'S', &[]);
                assert_eq!(
                    read(&mut socket),
                    (b'Z', vec![if transaction { b'E' } else { b'I' }])
                );
                if transaction {
                    query(&mut socket, "ROLLBACK");
                }
                assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
                send(&mut socket, b'C', b"Sbad11\0");
                assert_eq!(read(&mut socket).0, b'3');
                errors += 1;
            }
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_11")[1],
        (b'D', row(&[Some("0")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_11");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    println!(
        "pg_scalar_11: {count} value/NULL/metadata/parameter/empty cases; {errors} error/recovery/no-write cases"
    );
}

#[test]
fn pg_scalar_12() {
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
    let addr = cleanup.0.postgres_listener_addr().unwrap();
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT 1,2147483648,1.5,1e0");
        assert_eq!(mysql_read(socket), vec![4]);
        let columns = (0..4).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x011\x0a2147483648\x031.5\x011");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_metadata = mysql_check(&mut mysql);
    let mut socket = TcpStream::connect(addr).unwrap();
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
    let cases = [
        ("1", 23, "1"),
        ("2147483647", 23, "2147483647"),
        ("2147483648", 20, "2147483648"),
        ("9223372036854775807", 20, "9223372036854775807"),
        ("9223372036854775808", 1700, "9223372036854775808"),
        ("1.5", 1700, "1.5"),
        ("1e0", 1700, "1"),
        ("-2147483648", 23, "-2147483648"),
        ("-2147483649", 20, "-2147483649"),
        ("-9223372036854775808", 20, "-9223372036854775808"),
        ("-9223372036854775809", 1700, "-9223372036854775809"),
        ("18446744073709551616", 1700, "18446744073709551616"),
        ("1.2300e0", 1700, "1.2300"),
        (
            "-123456789012345678901234567890.123456789",
            1700,
            "-123456789012345678901234567890.123456789",
        ),
        ("+(1e0)", 1700, "1"),
    ];
    for (input, oid, expected) in cases {
        let sql = format!("SELECT {input}");
        let messages = query(&mut socket, &sql);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {messages:?}"
        );
        scalar_types(&messages[0], &[oid]);
        assert_eq!(messages[1], (b'D', row(&[Some(expected)])), "{sql}");
        parse(&mut socket, "n12", &sql, &[]);
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', b"Sn12\0");
        assert_eq!(read(&mut socket), (b't', vec![0, 0]));
        let desc = read(&mut socket);
        scalar_types(&desc, &[oid]);
        bind(&mut socket, "", "n12", &[]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), desc);
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some(expected)])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        send(&mut socket, b'C', b"Sn12\0");
        assert_eq!(read(&mut socket).0, b'3');
        let empty = query(&mut socket, &format!("{sql} WHERE FALSE"));
        assert_eq!(empty.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
        scalar_types(&empty[0], &[oid]);
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    for (literal, oid) in [("1", 23), ("2147483648", 20), ("1.5", 1700)] {
        let target = match oid {
            23 => "int4",
            20 => "int8",
            _ => "numeric",
        };
        let m = query(&mut socket, &format!("SELECT {literal},NULL::{target}"));
        scalar_types(&m[0], &[oid, oid]);
        assert_eq!(m[1], (b'D', row(&[Some(literal), None])));
    }
    mysql_query(&mut mysql, "CREATE DATABASE IF NOT EXISTS test");
    mysql_query(
        &mut mysql,
        "CREATE TABLE test.pg_scalar_12 (id INT PRIMARY KEY,v LONGTEXT)",
    );
    for sql in [
        "SELECT 1e131072",
        "INSERT INTO public.pg_scalar_12 VALUES (1,(1e131072)::text)",
    ] {
        let m = query(&mut socket, sql);
        assert_state(&m[0], "22003");
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    }
    for transaction in [false, true] {
        if transaction {
            query(&mut socket, "BEGIN");
        }
        parse(&mut socket, "bad12", "SELECT 1e131072", &[]);
        let parsed = read(&mut socket);
        let error = if parsed.0 == b'1' {
            bind(&mut socket, "", "bad12", &[]);
            let bound = read(&mut socket);
            if bound.0 == b'2' {
                execute(&mut socket, "", 0);
                read(&mut socket)
            } else {
                bound
            }
        } else {
            parsed
        };
        assert_state(&error, "22003");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            read(&mut socket),
            (b'Z', vec![if transaction { b'E' } else { b'I' }])
        );
        if transaction {
            query(&mut socket, "ROLLBACK");
        }
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
        send(&mut socket, b'C', b"Sbad12\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_12")[1],
        (b'D', row(&[Some("0")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_12");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    println!(
        "pg_scalar_12: 15 domains x simple/extended/empty, 3 typed NULL, 4 overflow/recovery/no-write cases, 3 MySQL reads"
    );
}

#[test]
fn pg_scalar_13() {
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
    let addr = cleanup.0.postgres_listener_addr().unwrap();
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT 1,2147483648,1.5,1e0");
        assert_eq!(mysql_read(socket), vec![4]);
        let columns = (0..4).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x011\x0a2147483648\x031.5\x011");
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let mysql_metadata = mysql_check(&mut mysql);
    let mut socket = TcpStream::connect(addr).unwrap();
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

    mysql_query(&mut mysql, "CREATE DATABASE IF NOT EXISTS test");
    mysql_query(
        &mut mysql,
        "CREATE TABLE test.pg_scalar_13 (a TINYINT UNSIGNED,b SMALLINT UNSIGNED,c MEDIUMINT UNSIGNED,d INT UNSIGNED,e BIGINT UNSIGNED)",
    );
    mysql_query(
        &mut mysql,
        "INSERT INTO test.pg_scalar_13 VALUES (0,0,0,0,0),(255,65535,16777215,4294967295,18446744073709551615),(NULL,NULL,NULL,NULL,NULL)",
    );
    let unsigned_mysql = |socket: &mut TcpStream| {
        mysql_packet(
            socket,
            0,
            b"\x03SELECT a,b,c,d,e FROM test.pg_scalar_13 ORDER BY a",
        );
        assert_eq!(mysql_read(socket), vec![5]);
        let columns = (0..5).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        let rows = (0..3).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        let eof = mysql_read(socket);
        assert_eq!(eof[0], 0xfe);
        assert_eq!(&eof[1..3], &[0, 0]);
        assert_eq!(rows[0], vec![0xfb; 5]);
        assert_eq!(rows[1], b"\x010\x010\x010\x010\x010");
        assert_eq!(
            rows[2],
            b"\x03255\x0565535\x0816777215\x0a4294967295\x1418446744073709551615"
        );
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        columns
    };
    let unsigned_metadata = unsigned_mysql(&mut mysql);
    let m = query(
        &mut socket,
        "SELECT a,b,c,d,e FROM public.pg_scalar_13 ORDER BY a",
    );
    assert_eq!(
        m.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDDDCZ",
        "{m:?}"
    );
    scalar_types(&m[0], &[21, 23, 23, 20, 1700]);
    assert_eq!(m[1], (b'D', row(&[None, None, None, None, None])));
    assert_eq!(m[2], (b'D', row(&[Some("0"); 5])));
    assert_eq!(
        m[3],
        (
            b'D',
            row(&[
                Some("255"),
                Some("65535"),
                Some("16777215"),
                Some("4294967295"),
                Some("18446744073709551615")
            ])
        )
    );
    let sql = "SELECT a,b,c,d,e FROM public.pg_scalar_13 WHERE a=255";
    parse(&mut socket, "u13", sql, &[]);
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'D', b"Su13\0");
    assert_eq!(read(&mut socket), (b't', vec![0, 0]));
    let desc = read(&mut socket);
    scalar_types(&desc, &[21, 23, 23, 20, 1700]);
    bind(&mut socket, "", "u13", &[]);
    assert_eq!(read(&mut socket).0, b'2');
    send(&mut socket, b'D', b"P\0");
    assert_eq!(read(&mut socket), desc);
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), m[3]);
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
    let empty = query(
        &mut socket,
        "SELECT a,b,c,d,e FROM public.pg_scalar_13 WHERE FALSE",
    );
    assert_eq!(empty.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    scalar_types(&empty[0], &[21, 23, 23, 20, 1700]);
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(unsigned_mysql(&mut mysql), unsigned_metadata);
    for (column, upper) in [
        ("a", "256"),
        ("b", "65536"),
        ("c", "16777216"),
        ("d", "4294967296"),
        ("e", "18446744073709551616"),
    ] {
        for input in ["-1", upper, "1.5"] {
            let sql = format!("INSERT INTO public.pg_scalar_13 ({column}) VALUES ({input})");
            let messages = query(&mut socket, &sql);
            assert_state(&messages[0], "22003");
            for transaction in [false, true] {
                if transaction {
                    query(&mut socket, "BEGIN");
                }
                parse(&mut socket, "bad13", &sql, &[]);
                let parsed = read(&mut socket);
                assert_eq!(parsed.0, b'1', "{sql}: {parsed:?}");
                bind(&mut socket, "", "bad13", &[]);
                assert_eq!(read(&mut socket).0, b'2');
                execute(&mut socket, "", 0);
                assert_state(&read(&mut socket), "22003");
                send(&mut socket, b'S', &[]);
                assert_eq!(
                    read(&mut socket),
                    (b'Z', vec![if transaction { b'E' } else { b'I' }])
                );
                if transaction {
                    query(&mut socket, "ROLLBACK");
                }
                assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
                send(&mut socket, b'C', b"Sbad13\0");
                assert_eq!(read(&mut socket).0, b'3');
            }
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_13")[1],
        (b'D', row(&[Some("3")]))
    );
    assert_eq!(unsigned_mysql(&mut mysql), unsigned_metadata);
    // A PG numeric/signed OID does not recreate a source unsigned declaration.
    for (oid, input) in [(23, "-1"), (1700, "-1.5")] {
        let sql = if oid == 23 {
            "SELECT $1::int4"
        } else {
            "SELECT $1::numeric"
        };
        parse(&mut socket, "signed13", sql, &[oid]);
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', b"Ssigned13\0");
        let mut params = 1i16.to_be_bytes().to_vec();
        params.extend_from_slice(&oid.to_be_bytes());
        assert_eq!(read(&mut socket), (b't', params));
        scalar_types(&read(&mut socket), &[oid]);
        bind(&mut socket, "", "signed13", &[Some(input)]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some(input)])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
        send(&mut socket, b'C', b"Ssigned13\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    query(&mut socket, "DROP TABLE public.pg_scalar_13");
    println!(
        "pg_scalar_13: 15 source value/NULL cells, extended and empty metadata, 15 simple + 30 extended reverse rejections/recovery/no-write, 2 signed parameter identities, 3 unsigned MySQL reads"
    );
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
}

#[test]
fn pg_scalar_16() {
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
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT 1.5,1e0");
        assert_eq!(mysql_read(socket), vec![2]);
        let metadata = (0..2).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x031.5\x011");
        let eof = mysql_read(socket);
        assert_eq!(&eof[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    parse(&mut socket, "f16", "SELECT $1", &[700]);
    let parsed = read(&mut socket);
    assert_eq!(parsed.0, b'1', "{parsed:?}");
    send(&mut socket, b'D', b"Sf16\0");
    assert_eq!(read(&mut socket), (b't', vec![0, 1, 0, 0, 2, 188]));
    let desc = read(&mut socket);
    scalar_types(&desc, &[700]);
    let good = [
        ("7.038531e-26", f32::from_bits(0x15ae43fd)),
        ("3.4028234663852886e38", f32::MAX),
        ("-3.4028234663852886e38", -f32::MAX),
        ("1.1754943508222875e-38", f32::MIN_POSITIVE),
        ("-1.1754943508222875e-38", -f32::MIN_POSITIVE),
        ("1.401298464324817e-45", f32::from_bits(1)),
        ("-1.401298464324817e-45", -f32::from_bits(1)),
        ("1.1754942106924411e-38", f32::from_bits(0x007fffff)),
        ("-1.1754942106924411e-38", -f32::from_bits(0x007fffff)),
        ("7.006492321624086e-46", f32::from_bits(1)),
        ("-7.006492321624086e-46", -f32::from_bits(1)),
        ("0", 0.0),
        ("-0", -0.0),
        ("0e-9999", 0.0),
        ("-0e9999", -0.0),
        (" \t1.5\r\n", 1.5),
        ("nAn", f32::NAN),
        ("+INFINITY", f32::INFINITY),
        ("-iNf", f32::NEG_INFINITY),
    ];
    assert_ne!(
        good[0].1.to_bits(),
        ("7.038531e-26".parse::<f64>().unwrap() as f32).to_bits()
    );
    for (input, expected) in good {
        bind(&mut socket, "", "f16", &[Some(input)]);
        let bound = read(&mut socket);
        assert_eq!(bound.0, b'2', "{input}: {bound:?}");
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), desc);
        execute(&mut socket, "", 0);
        let data = read(&mut socket);
        assert_eq!(data.0, b'D', "{input}: {data:?}");
        let actual = std::str::from_utf8(&data.1[6..])
            .unwrap()
            .parse::<f32>()
            .unwrap();
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits(), "{input}");
        }
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
    }
    bind(&mut socket, "", "f16", &[None]);
    assert_eq!(read(&mut socket).0, b'2');
    send(&mut socket, b'D', b"P\0");
    assert_eq!(read(&mut socket), desc);
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[None])));
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    parse(&mut socket, "empty16", "SELECT $1 WHERE FALSE", &[700]);
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'D', b"Sempty16\0");
    assert_eq!(read(&mut socket).0, b't');
    scalar_types(&read(&mut socket), &[700]);
    bind(&mut socket, "", "empty16", &[Some("1")]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    parse(&mut socket, "inferred16", "SELECT $1::float4", &[]);
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'D', b"Sinferred16\0");
    assert_eq!(read(&mut socket), (b't', vec![0, 1, 0, 0, 2, 188]));
    scalar_types(&read(&mut socket), &[700]);
    bind(&mut socket, "", "inferred16", &[Some("-0")]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[Some("-0")])));
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    for (sql, state) in [
        ("SELECT '1e39'::float4", "22003"),
        ("SELECT 'bad'::float4", "22P02"),
    ] {
        let messages = query(&mut socket, sql);
        assert_state(&messages[0], state);
        assert_eq!(messages.last().unwrap(), &(b'Z', vec![b'I']));
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(
        query(&mut socket, "CREATE TABLE public.pg_scalar_16 (v INT)")[0].0,
        b'C'
    );
    parse(
        &mut socket,
        "write16",
        "INSERT INTO public.pg_scalar_16 VALUES ($1)",
        &[700],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        for (input, state) in [
            (b"1e39".as_slice(), "22003"),
            (b"-1e39", "22003"),
            (b"1e-100", "22003"),
            (b"-1e-100", "22003"),
            (b"3.4028236e38", "22003"),
            (b"-3.4028236e38", "22003"),
            (b"7.006492321624085e-46", "22003"),
            (b"-7.006492321624085e-46", "22003"),
            (b"", "22P02"),
            (b"1bad", "22P02"),
            (b"1e", "22P02"),
            (b"Infinityx", "22P02"),
            (b"\xff", "22021"),
            (b"1\0", "22021"),
        ] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            bind_formats(&mut socket, "write16", &[], &[Some(input)]);
            assert_state(&read(&mut socket), state);
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
            }
            assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_16")[1],
        (b'D', row(&[Some("0")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_16");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    println!(
        "pg_scalar_16: 19 binary32 inputs + inferred cast, typed NULL, empty metadata, 2 SQL errors, 28 Bind errors/recovery/no-write, 3 MySQL value/metadata/warning reads"
    );
}

#[test]
fn pg_scalar_17() {
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
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT 1.5,1e0");
        assert_eq!(mysql_read(socket), vec![2]);
        let metadata = (0..2).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x031.5\x011");
        let eof = mysql_read(socket);
        assert_eq!(&eof[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    parse(&mut socket, "f17", "SELECT $1", &[701]);
    let parsed = read(&mut socket);
    assert_eq!(parsed.0, b'1', "{parsed:?}");
    send(&mut socket, b'D', b"Sf17\0");
    assert_eq!(read(&mut socket), (b't', vec![0, 1, 0, 0, 2, 189]));
    let desc = read(&mut socket);
    scalar_types(&desc, &[701]);
    let good = [
        ("1.7976931348623157e308", f64::MAX),
        ("-1.7976931348623157e308", -f64::MAX),
        ("2.2250738585072014e-308", f64::MIN_POSITIVE),
        ("-2.2250738585072014e-308", -f64::MIN_POSITIVE),
        ("4.9406564584124654e-324", f64::from_bits(1)),
        ("-4.9406564584124654e-324", -f64::from_bits(1)),
        ("2.225073858507201e-308", f64::from_bits(0x000fffffffffffff)),
        (
            "-2.225073858507201e-308",
            -f64::from_bits(0x000fffffffffffff),
        ),
        ("2.4703282292062328e-324", f64::from_bits(1)),
        ("-2.4703282292062328e-324", -f64::from_bits(1)),
        ("0", 0.0),
        ("-0", -0.0),
        ("0e-9999", 0.0),
        ("-0e9999", -0.0),
        (" \t1.5\r\n\x0b\x0c", 1.5),
        ("nAn", f64::NAN),
        ("+INFINITY", f64::INFINITY),
        ("-iNf", f64::NEG_INFINITY),
    ];
    for (input, expected) in good {
        bind(&mut socket, "", "f17", &[Some(input)]);
        let bound = read(&mut socket);
        assert_eq!(bound.0, b'2', "{input}: {bound:?}");
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), desc);
        execute(&mut socket, "", 0);
        let data = read(&mut socket);
        assert_eq!(data.0, b'D', "{input}: {data:?}");
        let actual = std::str::from_utf8(&data.1[6..])
            .unwrap()
            .parse::<f64>()
            .unwrap();
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.to_bits(), expected.to_bits(), "{input}");
        }
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
    }
    bind(&mut socket, "", "f17", &[None]);
    assert_eq!(read(&mut socket).0, b'2');
    send(&mut socket, b'D', b"P\0");
    assert_eq!(read(&mut socket), desc);
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket), (b'D', row(&[None])));
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    parse(&mut socket, "empty17", "SELECT $1 WHERE FALSE", &[701]);
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'D', b"Sempty17\0");
    assert_eq!(read(&mut socket).0, b't');
    scalar_types(&read(&mut socket), &[701]);
    bind(&mut socket, "", "empty17", &[Some("1")]);
    assert_eq!(read(&mut socket).0, b'2');
    execute(&mut socket, "", 0);
    assert_eq!(read(&mut socket).0, b'C');
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(
        query(&mut socket, "CREATE TABLE public.pg_scalar_17 (v INT)")[0].0,
        b'C'
    );
    parse(
        &mut socket,
        "write17",
        "INSERT INTO public.pg_scalar_17 VALUES ($1)",
        &[701],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        for (input, state) in [
            (b"1e309".as_slice(), "22003"),
            (b"-1e309", "22003"),
            (b"1e-9999", "22003"),
            (b"-1e-9999", "22003"),
            (b"1.7976931348623159e308", "22003"),
            (b"-1.7976931348623159e308", "22003"),
            (b"2.4703282292062327e-324", "22003"),
            (b"-2.4703282292062327e-324", "22003"),
            (b"", "22P02"),
            (b" \t", "22P02"),
            (b"1bad", "22P02"),
            (b"1e", "22P02"),
            (b"Infinityx", "22P02"),
            (b"NaN(1)", "22P02"),
            (b"\xc2\xa01", "22P02"),
            (b"\xff", "22021"),
            (b"1\0", "22021"),
        ] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            bind_formats(&mut socket, "write17", &[], &[Some(input)]);
            assert_state(&read(&mut socket), state);
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
            }
            assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_17")[1],
        (b'D', row(&[Some("0")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_17");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    println!(
        "pg_scalar_17: 18 binary64 inputs, typed NULL, empty metadata, 34 Bind errors/recovery/no-write, 3 MySQL value/metadata/warning reads"
    );
}

#[test]
fn pg_scalar_18() {
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
    mysql_query(
        &mut mysql,
        "CREATE TABLE pg_scalar_18_mysql (f FLOAT, d DOUBLE)",
    );
    mysql_query(
        &mut mysql,
        "INSERT INTO pg_scalar_18_mysql VALUES (1.5,1.5)",
    );
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT f,d FROM pg_scalar_18_mysql");
        assert_eq!(mysql_read(socket), vec![2]);
        let metadata = (0..2).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        for (column, expected) in metadata.iter().zip([4, 5]) {
            let mut offset = 0;
            for _ in 0..6 {
                offset += 1 + usize::from(column[offset]);
            }
            assert_eq!(column[offset], 12);
            assert_eq!(column[offset + 7], expected);
        }
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x031.5\x031.5");
        let eof = mysql_read(socket);
        assert_eq!(&eof[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    for (oid, input, expected) in [
        (700, "0.1", "0.1"),
        (700, "3.4028235e38", "3.4028235e+38"),
        (700, "1.1754944e-38", "1.1754944e-38"),
        (700, "1e-45", "1e-45"),
        (700, "0.0001", "0.0001"),
        (700, "0.00001", "1e-05"),
        (700, "100000", "100000"),
        (700, "1000000", "1e+06"),
        (701, "1.7976931348623157e308", "1.7976931348623157e+308"),
        (701, "5e-324", "5e-324"),
        (701, "100000000000000", "100000000000000"),
        (701, "1000000000000000", "1e+15"),
        (700, "-0", "-0"),
        (700, "NaN", "NaN"),
        (700, "Infinity", "Infinity"),
        (700, "-Infinity", "-Infinity"),
        (701, "0.1", "0.1"),
        (701, "1e23", "9.999999999999999e+22"),
        (701, "-0", "-0"),
        (701, "NaN", "NaN"),
        (701, "Infinity", "Infinity"),
        (701, "-Infinity", "-Infinity"),
    ] {
        parse(&mut socket, "f18", "SELECT $1", &[oid]);
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', b"Sf18\0");
        assert_eq!(
            read(&mut socket),
            (b't', [vec![0, 1], oid.to_be_bytes().to_vec()].concat())
        );
        scalar_types(&read(&mut socket), &[oid]);
        for value in [Some(input), None] {
            bind(&mut socket, "", "f18", &[value]);
            assert_eq!(read(&mut socket).0, b'2');
            execute(&mut socket, "", 0);
            assert_eq!(
                read(&mut socket),
                (b'D', row(&[value.map(|_| expected)])),
                "oid={oid} input={input}"
            );
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        }
        send(&mut socket, b'C', b"Sf18\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    for (ty, oid) in [
        ("float(1)", 700),
        ("float(24)", 700),
        ("float(25)", 701),
        ("float(53)", 701),
        ("real", 700),
        ("double precision", 701),
        ("float", 701),
        ("float4", 700),
        ("float8", 701),
    ] {
        parse(&mut socket, "alias18", &format!("SELECT $1::{ty}"), &[]);
        let parsed = read(&mut socket);
        assert_eq!(parsed.0, b'1', "{ty}: {parsed:?}");
        send(&mut socket, b'D', b"Salias18\0");
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [vec![0, 1], (oid as u32).to_be_bytes().to_vec()].concat()
            )
        );
        scalar_types(&read(&mut socket), &[oid]);
        bind(&mut socket, "", "alias18", &[Some("0.1")]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some("0.1")])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        send(&mut socket, b'C', b"Salias18\0");
        assert_eq!(read(&mut socket).0, b'3');
        let cast = query(&mut socket, &format!("SELECT CAST('0.1' AS {ty})"));
        scalar_types(&cast[0], &[oid]);
        assert_eq!(cast[1], (b'D', row(&[Some("0.1")])));
        for expr in ["'0.1'", "NULL"] {
            let messages = query(&mut socket, &format!("SELECT {expr}::{ty}"));
            scalar_types(&messages[0], &[oid]);
            assert_eq!(
                messages[1],
                (
                    b'D',
                    row(&[if expr == "NULL" { None } else { Some("0.1") }])
                )
            );
        }
        let messages = query(&mut socket, &format!("SELECT NULL::{ty} WHERE FALSE"));
        scalar_types(&messages[0], &[oid]);
        assert_eq!(messages[1].0, b'C');
    }
    assert_eq!(
        query(&mut socket, "CREATE TABLE public.pg_scalar_18 (v INT)")[0].0,
        b'C'
    );
    for transaction in [false, true] {
        for ty in ["float(0)", "float(54)"] {
            if transaction {
                assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
            }
            parse(
                &mut socket,
                "bad18",
                &format!("INSERT INTO public.pg_scalar_18 VALUES (NULL::{ty})"),
                &[],
            );
            assert_state(&read(&mut socket), "22023");
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
            }
            assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_18")[1],
        (b'D', row(&[Some("0")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_18");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "DROP TABLE pg_scalar_18_mysql");
    println!(
        "pg_scalar_18: 22 parameter values + typed NULL, 9 declarations with inferred parameters/CAST/values/NULL/zero rows, 4 invalid precision Parse errors/Sync/ROLLBACK/no-write, 3 MySQL reads"
    );
}

#[test]
fn pg_scalar_19() {
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
    mysql_query(
        &mut mysql,
        "CREATE TABLE pg_scalar_19_mysql (f FLOAT, d DOUBLE)",
    );
    mysql_query(
        &mut mysql,
        "INSERT INTO pg_scalar_19_mysql VALUES (1.5,1.5)",
    );
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT f,d FROM pg_scalar_19_mysql");
        assert_eq!(mysql_read(socket), vec![2]);
        let metadata = (0..2).map(|_| mysql_read(socket)).collect::<Vec<_>>();
        for (column, expected) in metadata.iter().zip([4, 5]) {
            let mut offset = 0;
            for _ in 0..6 {
                offset += 1 + usize::from(column[offset]);
            }
            assert_eq!(column[offset], 12);
            assert_eq!(column[offset + 7], expected);
        }
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x031.5\x031.5");
        let eof = mysql_read(socket);
        assert_eq!(&eof[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    for (expression, oid, expected) in [
        ("'0.1'::float4::float8", 701, "0.10000000149011612"),
        ("'2.5'::float8::int4", 23, "2"),
        ("'-2.5'::float8::int4", 23, "-2"),
        ("'3.5'::float4::int8", 20, "4"),
        ("'32767'::float8::int2", 21, "32767"),
        ("'-32768.5'::float8::int2", 21, "-32768"),
        ("'2147483647'::float8::int4", 23, "2147483647"),
        ("'-2147483648'::float8::int4", 23, "-2147483648"),
        (
            "'9223372036854774784'::float8::int8",
            20,
            "9223372036854774784",
        ),
        (
            "'-9223372036854775808'::float8::int8",
            20,
            "-9223372036854775808",
        ),
        ("'-0'::float8::float4", 700, "-0"),
        ("'1e-45'::float8::float4", 700, "1e-45"),
        ("'1.0000000596046448'::float8::float4", 700, "1"),
        ("'Infinity'::float8::float4", 700, "Infinity"),
        ("'-Infinity'::float4::float8", 701, "-Infinity"),
        ("'NaN'::float8::float4", 700, "NaN"),
    ] {
        let messages = query(&mut socket, &format!("SELECT {expression}"));
        scalar_types(&messages[0], &[oid]);
        assert_eq!(messages[1], (b'D', row(&[Some(expected)])), "{expression}");
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    for (target, oid) in [
        ("float4", 700),
        ("float8", 701),
        ("int2", 21),
        ("int4", 23),
        ("int8", 20),
    ] {
        let messages = query(&mut socket, &format!("SELECT NULL::float8::{target}"));
        scalar_types(&messages[0], &[oid]);
        assert_eq!(messages[1], (b'D', row(&[None])));
        let messages = query(
            &mut socket,
            &format!("SELECT NULL::float8::{target} WHERE FALSE"),
        );
        scalar_types(&messages[0], &[oid]);
        assert_eq!(messages[1].0, b'C');
        parse(&mut socket, "f19", &format!("SELECT $1::{target}"), &[701]);
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', b"Sf19\0");
        assert_eq!(
            read(&mut socket),
            (b't', [vec![0, 1], 701u32.to_be_bytes().to_vec()].concat())
        );
        scalar_types(&read(&mut socket), &[oid]);
        bind(&mut socket, "", "f19", &[Some("2.5")]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(
            read(&mut socket),
            (b'D', row(&[Some(if oid < 100 { "2" } else { "2.5" })]))
        );
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        send(&mut socket, b'C', b"Sf19\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    query(&mut socket, "CREATE TABLE public.pg_scalar_19 (v INT)");
    let mut error_case = 0;
    for transaction in [false, true] {
        for expression in [
            "'1e39'::float8::float4",
            "'1e-100'::float8::float4",
            "'NaN'::float8::int4",
            "'Infinity'::float8::int8",
            "'-Infinity'::float4::int2",
            "'32767.5'::float8::int2",
            "'2147483647.5'::float8::int4",
            "'9223372036854775808'::float8::int8",
        ] {
            if transaction {
                query(&mut socket, "BEGIN");
            }
            error_case += 1;
            let statement = format!("bad19_{error_case}");
            parse(
                &mut socket,
                &statement,
                &format!("INSERT INTO public.pg_scalar_19 VALUES ({expression})"),
                &[],
            );
            let parsed = read(&mut socket);
            if parsed.0 == b'1' {
                bind(&mut socket, "", &statement, &[]);
                let bound = read(&mut socket);
                if bound.0 == b'2' {
                    execute(&mut socket, "", 0);
                    assert_state(&read(&mut socket), "22003");
                } else {
                    assert_state(&bound, "22003");
                }
            } else {
                assert_state(&parsed, "22003");
            }
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                query(&mut socket, "ROLLBACK");
            }
            assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
        }
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_19")[1],
        (b'D', row(&[Some("0")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_19");
    println!(
        "pg_scalar_19: 16 values, 5 typed NULL/empty/extended parameter targets, 16 errors/Sync/ROLLBACK/no-write, MySQL isolation"
    );
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "DROP TABLE pg_scalar_19_mysql");
}

#[test]
fn pg_scalar_21() {
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
    mysql_query(
        &mut mysql,
        "CREATE TABLE pg_scalar_21_mysql (v VARCHAR(30))",
    );
    mysql_query(&mut mysql, "INSERT INTO pg_scalar_21_mysql VALUES ('é')");
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT v FROM pg_scalar_21_mysql");
        assert_eq!(mysql_read(socket), vec![1]);
        let metadata = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), "\u{2}é".as_bytes());
        assert_eq!(&mysql_read(socket)[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    query(
        &mut socket,
        "CREATE TABLE public.pg_scalar_21 (v VARCHAR(30))",
    );
    for oid in [25u32, 1043, 1042] {
        parse(&mut socket, "text21", "SELECT $1", &[oid]);
        let parsed = read(&mut socket);
        assert_eq!(parsed.0, b'1', "{oid}: {parsed:?}");
        send(&mut socket, b'D', b"Stext21\0");
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &oid.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        scalar_types(&description, &[oid]);
        for input in [Some(""), Some("e\u{301}é😀"), None] {
            bind(&mut socket, "", "text21", &[input]);
            assert_eq!(read(&mut socket).0, b'2');
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), description);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[input])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        }
        // Both wire formats retain UTF8 bytes and reject invalid encodings at Bind.
        for format in [0, 1] {
            bind_formats(&mut socket, "text21", &[format], &[Some("é😀".as_bytes())]);
            assert_eq!(read(&mut socket).0, b'2');
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[Some("é😀")])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        }
        parse(
            &mut socket,
            "write21",
            "INSERT INTO public.pg_scalar_21 VALUES ($1)",
            &[oid],
        );
        assert_eq!(read(&mut socket).0, b'1');
        for transaction in [false, true] {
            for input in [b"\xff".as_slice(), b"a\0b", b"\xc3"] {
                if transaction {
                    assert_eq!(query(&mut socket, "BEGIN")[0].0, b'C');
                }
                bind_formats(&mut socket, "write21", &[], &[Some(input)]);
                assert_state(&read(&mut socket), "22021");
                send(&mut socket, b'S', &[]);
                assert_eq!(
                    read(&mut socket),
                    (b'Z', vec![if transaction { b'E' } else { b'I' }])
                );
                if transaction {
                    assert_eq!(query(&mut socket, "ROLLBACK")[0].0, b'C');
                }
                assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
            }
        }
        send(&mut socket, b'C', b"Swrite21\0");
        assert_eq!(read(&mut socket).0, b'3');
        send(&mut socket, b'C', b"Stext21\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    for (ty, oid, typmod) in [
        ("text", 25, -1),
        ("varchar", 1043, -1),
        ("bpchar", 1042, -1),
        ("char", 1042, 5),
    ] {
        for expr in [
            format!("'é'::{ty}"),
            format!("CAST('é' AS {ty})"),
            format!("NULL::{ty}"),
        ] {
            let messages = query(&mut socket, &format!("SELECT {expr}"));
            assert_eq!(messages[0].0, b'T', "{expr}: {messages:?}");
            let mut normalized = messages[0].clone();
            let offset = 3 + normalized.1[2..].iter().position(|b| *b == 0).unwrap();
            assert_eq!(
                i32::from_be_bytes(normalized.1[offset + 12..offset + 16].try_into().unwrap()),
                typmod,
                "{expr}"
            );
            normalized.1[offset + 12..offset + 16].copy_from_slice(&(-1i32).to_be_bytes());
            scalar_types(&normalized, &[oid]);
            assert_eq!(
                messages[1],
                (
                    b'D',
                    row(&[if expr.starts_with("NULL") {
                        None
                    } else {
                        Some("é")
                    }])
                )
            );
        }
        parse(&mut socket, "inferred21", &format!("SELECT $1::{ty}"), &[]);
        let parsed = read(&mut socket);
        assert_eq!(parsed.0, b'1', "{ty}: {parsed:?}");
        send(&mut socket, b'D', b"Sinferred21\0");
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &oid.to_be_bytes()].concat()
            )
        );
        let inferred_description = read(&mut socket);
        bind(&mut socket, "", "inferred21", &[Some("é")]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), inferred_description);
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some("é")])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        send(&mut socket, b'C', b"Sinferred21\0");
        assert_eq!(read(&mut socket).0, b'3');
        let empty = query(&mut socket, &format!("SELECT NULL::{ty} WHERE FALSE"));
        assert_eq!(empty[0].0, b'T');
        assert_eq!(empty[1].0, b'C');
        // Parse and both Describe variants are stable even without any rows.
        parse(
            &mut socket,
            "empty21",
            &format!("SELECT NULL::{ty} WHERE FALSE"),
            &[],
        );
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', b"Sempty21\0");
        assert_eq!(read(&mut socket), (b't', vec![0, 0]));
        assert_eq!(read(&mut socket), empty[0]);
        bind(&mut socket, "", "empty21", &[]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), empty[0]);
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        send(&mut socket, b'C', b"Sempty21\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_21")[1],
        (b'D', row(&[Some("0")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_21");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "DROP TABLE pg_scalar_21_mysql");
    println!(
        "pg_scalar_21: 9 value/NULL parameter cases, 6 binary/text inputs, 18 encoding Bind errors/Sync/ROLLBACK/no-write, 12 cast/NULL cases, 4 inferred parameters, 4 empty/Describe lifecycles, 3 MySQL value/metadata/warning reads"
    );
}

#[test]
fn pg_scalar_22() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain.clone(), Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    mysql_query(
        &mut mysql,
        "CREATE TABLE pg_scalar_22_mysql (v VARCHAR(30))",
    );
    mysql_query(&mut mysql, "INSERT INTO pg_scalar_22_mysql VALUES ('é')");
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT v FROM pg_scalar_22_mysql");
        assert_eq!(mysql_read(socket), vec![1]);
        let metadata = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), "\u{2}é".as_bytes());
        assert_eq!(&mysql_read(socket)[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    for (input, n, expected) in [
        ("é😀x", 2, "é😀"),
        ("e\u{301}x", 2, "e\u{301}"),
        ("a  ", 2, "a "),
        ("é", 1, "é"),
    ] {
        let sql = format!("SELECT CAST('{input}' AS varchar({n}))");
        let messages = query(&mut socket, &sql);
        assert_eq!(messages[0].0, b'T', "{sql}: {messages:?}");
        assert_eq!(messages[1], (b'D', row(&[Some(expected)])), "{sql}");
    }

    let describe = |message: &(u8, Vec<u8>), length: i32| {
        assert_eq!(message.0, b'T', "{message:?}");
        let mut normalized = message.clone();
        let offset = 3 + normalized.1[2..].iter().position(|b| *b == 0).unwrap();
        assert_eq!(
            i32::from_be_bytes(normalized.1[offset + 12..offset + 16].try_into().unwrap()),
            length + 4
        );
        normalized.1[offset + 12..offset + 16].copy_from_slice(&(-1i32).to_be_bytes());
        scalar_types(&normalized, &[1043]);
    };
    for n in [1, 10485760] {
        for null in [false, true] {
            let sql = format!("SELECT {}::varchar({n})", if null { "NULL" } else { "'é'" });
            let messages = query(&mut socket, &sql);
            describe(&messages[0], n);
            assert_eq!(
                messages[1],
                (b'D', row(&[if null { None } else { Some("é") }]))
            );
        }
        let empty = query(
            &mut socket,
            &format!("SELECT NULL::varchar({n}) WHERE FALSE"),
        );
        describe(&empty[0], n);
        assert_eq!(empty[1].0, b'C');
        parse(
            &mut socket,
            "length22",
            &format!("SELECT $1::varchar({n})"),
            &[],
        );
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', b"Slength22\0");
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1043u32.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        describe(&description, n);
        bind(&mut socket, "", "length22", &[Some("éx")]);
        assert_eq!(read(&mut socket).0, b'2');
        send(&mut socket, b'D', b"P\0");
        assert_eq!(read(&mut socket), description);
        execute(&mut socket, "", 0);
        assert_eq!(
            read(&mut socket),
            (b'D', row(&[Some(if n == 1 { "é" } else { "éx" })]))
        );
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        send(&mut socket, b'C', b"Slength22\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    for n in [0, 10485761] {
        for suffix in ["", " WHERE FALSE"] {
            let messages = query(&mut socket, &format!("SELECT NULL::varchar({n}){suffix}"));
            assert_state(&messages[0], "22023");
        }
    }
    for n in [0, 10485761] {
        let messages = query(
            &mut socket,
            &format!("CREATE TABLE public.pg_scalar_22_invalid (v varchar({n}))"),
        );
        assert_state(&messages[0], "22023");
    }
    let created = query(
        &mut socket,
        "CREATE TABLE public.pg_scalar_22_one (v varchar(1))",
    );
    assert_eq!(created[0].0, b'C');
    query(&mut socket, "DROP TABLE public.pg_scalar_22_one");
    // Construct the maximum value in SQL, within the listener's input limit.
    let maximum = "é".repeat(10485760);
    send(
        &mut socket,
        b'Q',
        b"SELECT repeat('\xc3\xa9',10485760)::varchar(10485760)\0",
    );
    describe(&read(&mut socket), 10485760);
    // The generic fixture reader caps allocations at 1 MiB; this one result has
    // an exact known size, so read it locally without weakening other tests.
    let expected = row(&[Some(&maximum)]);
    let mut header = [0; 5];
    socket.read_exact(&mut header).unwrap();
    assert_eq!(header[0], b'D');
    assert_eq!(
        u32::from_be_bytes(header[1..].try_into().unwrap()) as usize,
        expected.len() + 4
    );
    let mut actual = vec![0; expected.len()];
    socket.read_exact(&mut actual).unwrap();
    assert!(
        actual == expected,
        "maximum varchar value must retain every UTF8 character"
    );
    assert_eq!(read(&mut socket).0, b'C');
    assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
    let created = query(
        &mut socket,
        "CREATE TABLE public.pg_scalar_22_max (v VARCHAR(10485760))",
    );
    assert_eq!(created[0].0, b'C');
    query(&mut socket, "DROP TABLE public.pg_scalar_22_max");
    query(
        &mut socket,
        "CREATE TABLE public.pg_scalar_22 (v varchar(2))",
    );
    for (input, expected) in [
        (Some("é😀"), Some("é😀")),
        (Some("e\u{301} "), Some("e\u{301}")),
        (Some("a  "), Some("a ")),
        (None, None),
    ] {
        parse(
            &mut socket,
            "write22",
            "INSERT INTO public.pg_scalar_22 VALUES ($1)",
            &[1043],
        );
        assert_eq!(read(&mut socket).0, b'1');
        bind(&mut socket, "", "write22", &[input]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        let written = read(&mut socket);
        assert_eq!(written.0, b'C', "{input:?}: {written:?}");
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        let result = query(&mut socket, "SELECT v FROM public.pg_scalar_22");
        describe(&result[0], 2);
        assert_eq!(result[1], (b'D', row(&[expected])));
        query(&mut socket, "DELETE FROM public.pg_scalar_22");
        send(&mut socket, b'C', b"Swrite22\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    for transaction in [false, true] {
        for input in ["é😀x", "a \t", "a \u{a0}"] {
            if transaction {
                query(&mut socket, "BEGIN");
            }
            parse(
                &mut socket,
                "bad22",
                "INSERT INTO public.pg_scalar_22 VALUES ($1)",
                &[1043],
            );
            assert_eq!(read(&mut socket).0, b'1');
            bind(&mut socket, "", "bad22", &[Some(input)]);
            assert_eq!(read(&mut socket).0, b'2');
            execute(&mut socket, "", 0);
            assert_state(&read(&mut socket), "22001");
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                query(&mut socket, "ROLLBACK");
            }
            assert_eq!(
                query(&mut socket, "SELECT count(*) FROM public.pg_scalar_22")[1],
                (b'D', row(&[Some("0")]))
            );
            send(&mut socket, b'C', b"Sbad22\0");
            assert_eq!(read(&mut socket).0, b'3');
        }
    }
    assert_eq!(
        query(
            &mut socket,
            "INSERT INTO public.pg_scalar_22 VALUES ('é😀x'::varchar(2))"
        )[0]
        .0,
        b'C'
    );
    assert_eq!(
        query(&mut socket, "SELECT v FROM public.pg_scalar_22")[1],
        (b'D', row(&[Some("é😀")]))
    );
    assert_state(
        &query(&mut socket, "UPDATE public.pg_scalar_22 SET v='abc'")[0],
        "22001",
    );
    assert_eq!(
        query(&mut socket, "SELECT v FROM public.pg_scalar_22")[1],
        (b'D', row(&[Some("é😀")]))
    );
    assert_eq!(
        query(&mut socket, "UPDATE public.pg_scalar_22 SET v='a  '")[0].0,
        b'C'
    );
    assert_eq!(
        query(&mut socket, "SELECT v FROM public.pg_scalar_22")[1],
        (b'D', row(&[Some("a ")]))
    );
    query(&mut socket, "DELETE FROM public.pg_scalar_22");
    assert_state(
        &query(
            &mut socket,
            "INSERT INTO public.pg_scalar_22 VALUES ('ok'),('abc')",
        )[0],
        "22001",
    );
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_22")[1],
        (b'D', row(&[Some("0")]))
    );
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    query(&mut socket, "DROP TABLE public.pg_scalar_22");
    println!(
        "pg_scalar_22: 4 explicit casts; 8 endpoint/NULL/empty/Describe cases; 6 invalid modifiers; 2 declaration endpoints; 4 DML atomicity/cast/update checks; 1 actual 10485760-character value (20 MiB UTF8); 4 assignments; 6 Execute errors/Sync/ROLLBACK/no-write; 3 MySQL checkpoints"
    );
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "DROP TABLE pg_scalar_22_mysql");
}

#[test]
fn pg_scalar_23() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain.clone(), Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    mysql_query(
        &mut mysql,
        "CREATE TABLE pg_scalar_23_mysql (v VARCHAR(30))",
    );
    mysql_query(&mut mysql, "INSERT INTO pg_scalar_23_mysql VALUES ('é')");
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT v FROM pg_scalar_23_mysql");
        assert_eq!(mysql_read(socket), vec![1]);
        let metadata = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), "\u{2}é".as_bytes());
        assert_eq!(&mysql_read(socket)[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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
    let describe = |message: &(u8, Vec<u8>), oid: u32, length: i32| {
        assert_eq!(message.0, b'T', "{message:?}");
        let mut normalized = message.clone();
        let offset = 3 + normalized.1[2..].iter().position(|b| *b == 0).unwrap();
        assert_eq!(
            i32::from_be_bytes(normalized.1[offset + 12..offset + 16].try_into().unwrap()),
            if length < 0 { -1 } else { length + 4 }
        );
        normalized.1[offset + 12..offset + 16].copy_from_slice(&(-1i32).to_be_bytes());
        scalar_types(&normalized, &[oid]);
    };
    for (sql, expected, oid, length) in [
        ("SELECT 'é'::char(3)", "é  ", 1042, 3),
        ("SELECT CAST('é' AS char(3))", "é  ", 1042, 3),
        ("SELECT ''::char", " ", 1042, 1),
        ("SELECT 'ab'::char", "a", 1042, 1),
        ("SELECT 'é😀x'::char(2)", "é😀", 1042, 2),
        ("SELECT 'éx'::char(2)", "é", 1042, 2),
        ("SELECT 'a  '::bpchar", "a  ", 1042, -1),
        ("SELECT ('é'::char(3))::text", "é", 25, -1),
        ("SELECT ('é'::char(3))::varchar", "é", 1043, -1),
        ("SELECT ('é'::char(3))::varchar(2)", "é", 1043, 2),
        ("SELECT ('a  '::bpchar)::text", "a", 25, -1),
        ("SELECT ('   '::char(3))::text", "", 25, -1),
        ("SELECT ('a　'::char(3))::text", "a　", 25, -1),
        ("SELECT ('a  '::text)::varchar", "a  ", 1043, -1),
        ("SELECT ('a  '::varchar)::text", "a  ", 25, -1),
    ] {
        let messages = query(&mut socket, sql);
        describe(&messages[0], oid, length);
        assert_eq!(
            messages[1],
            (b'D', row(&[Some(expected)])),
            "{sql}: {messages:?}"
        );
    }
    for (target, length) in [("char", 1), ("char(3)", 3), ("bpchar", -1)] {
        let messages = query(&mut socket, &format!("SELECT NULL::{target}"));
        describe(&messages[0], 1042, length);
        assert_eq!(messages[1], (b'D', row(&[None])));
        let messages = query(&mut socket, &format!("SELECT NULL::{target} WHERE FALSE"));
        describe(&messages[0], 1042, length);
        assert_eq!(messages[1].0, b'C');
        parse(&mut socket, "char23", &format!("SELECT $1::{target}"), &[]);
        assert_eq!(read(&mut socket).0, b'1');
        send(&mut socket, b'D', b"Schar23\0");
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1042u32.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        describe(&description, 1042, length);
        for (input, expected) in [
            (Some("é"), Some(if length == 3 { "é  " } else { "é" })),
            (None, None),
        ] {
            bind(&mut socket, "", "char23", &[input]);
            assert_eq!(read(&mut socket).0, b'2');
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), description);
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[expected])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        }
        send(&mut socket, b'C', b"Schar23\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    for n in [0, 10485761] {
        for suffix in ["", " WHERE FALSE"] {
            assert_state(
                &query(&mut socket, &format!("SELECT NULL::char({n}){suffix}"))[0],
                "22023",
            );
        }
        assert_state(
            &query(
                &mut socket,
                &format!("CREATE TABLE public.pg_scalar_23_bad (v char({n}))"),
            )[0],
            "22023",
        );
    }
    send(&mut socket, b'Q', "SELECT 'é'::char(10485760)\0".as_bytes());
    describe(&read(&mut socket), 1042, 10485760);
    // This exact bounded endpoint exceeds the general fixture's 1 MiB cap.
    let mut header = [0; 5];
    socket.read_exact(&mut header).unwrap();
    assert_eq!(header[0], b'D');
    assert_eq!(
        u32::from_be_bytes(header[1..].try_into().unwrap()),
        10485771
    );
    let mut endpoint = vec![0; 10485767];
    socket.read_exact(&mut endpoint).unwrap();
    assert_eq!(&endpoint[..8], &[0, 1, 0, 160, 0, 1, 195, 169]);
    assert!(endpoint[8..].iter().all(|b| *b == b' '));
    assert_eq!(read(&mut socket).0, b'C');
    assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    assert_eq!(
        query(&mut socket, "CREATE TABLE public.pg_scalar_23 (v char(3))")[0].0,
        b'C'
    );
    for (input, expected) in [
        ("é", "é  "),
        ("é😀x", "é😀x"),
        ("a    ", "a  "),
        ("", "   "),
    ] {
        parse(
            &mut socket,
            "assign23",
            "INSERT INTO public.pg_scalar_23 VALUES ($1)",
            &[25],
        );
        assert_eq!(read(&mut socket).0, b'1');
        bind(&mut socket, "", "assign23", &[Some(input)]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        let result = query(&mut socket, "SELECT v FROM public.pg_scalar_23");
        describe(&result[0], 1042, 3);
        assert_eq!(result[1], (b'D', row(&[Some(expected)])));
        let result = query(&mut socket, "SELECT v::text FROM public.pg_scalar_23");
        assert_eq!(
            result[1],
            (b'D', row(&[Some(expected.trim_end_matches(' '))]))
        );
        let result = query(&mut socket, "SELECT v::varchar FROM public.pg_scalar_23");
        assert_eq!(
            result[1],
            (b'D', row(&[Some(expected.trim_end_matches(' '))]))
        );
        query(&mut socket, "DELETE FROM public.pg_scalar_23");
        send(&mut socket, b'C', b"Sassign23\0");
        assert_eq!(read(&mut socket).0, b'3');
    }
    for transaction in [false, true] {
        if transaction {
            query(&mut socket, "BEGIN");
        }
        parse(
            &mut socket,
            "reject23",
            "INSERT INTO public.pg_scalar_23 VALUES ($1)",
            &[25],
        );
        assert_eq!(read(&mut socket).0, b'1');
        bind(&mut socket, "", "reject23", &[Some("abcd")]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_state(&read(&mut socket), "22001");
        send(&mut socket, b'S', &[]);
        assert_eq!(
            read(&mut socket),
            (b'Z', vec![if transaction { b'E' } else { b'I' }])
        );
        if transaction {
            query(&mut socket, "ROLLBACK");
        }
        send(&mut socket, b'C', b"Sreject23\0");
        assert_eq!(read(&mut socket).0, b'3');
        assert_eq!(
            query(&mut socket, "SELECT count(*) FROM public.pg_scalar_23")[1],
            (b'D', row(&[Some("0")]))
        );
    }
    assert_state(
        &query(
            &mut socket,
            "INSERT INTO public.pg_scalar_23 VALUES ('ok'),('abcd')",
        )[0],
        "22001",
    );
    assert_eq!(
        query(&mut socket, "SELECT count(*) FROM public.pg_scalar_23")[1],
        (b'D', row(&[Some("0")]))
    );
    query(
        &mut socket,
        "INSERT INTO public.pg_scalar_23 VALUES ('abcd'::char(3))",
    );
    assert_state(
        &query(&mut socket, "UPDATE public.pg_scalar_23 SET v='abcd'")[0],
        "22001",
    );
    assert_eq!(
        query(&mut socket, "SELECT v FROM public.pg_scalar_23")[1],
        (b'D', row(&[Some("abc")]))
    );
    query(&mut socket, "DROP TABLE public.pg_scalar_23");
    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE public.pg_scalar_23_default (v char)"
        )[0]
        .0,
        b'C'
    );
    query(
        &mut socket,
        "INSERT INTO public.pg_scalar_23_default VALUES ('')",
    );
    let result = query(&mut socket, "SELECT v FROM public.pg_scalar_23_default");
    describe(&result[0], 1042, 1);
    assert_eq!(result[1], (b'D', row(&[Some(" ")])));
    query(&mut socket, "DROP TABLE public.pg_scalar_23_default");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    mysql_query(&mut mysql, "DROP TABLE pg_scalar_23_mysql");
    eprintln!(
        "pg_scalar_23: 15 casts, 3 NULL/zero-row/statement/portal lifecycles with 6 parameter executions, 6 invalid modifier cases, 4 assignments with column text/varchar conversion, 2 Execute errors/Sync/ROLLBACK/no-write, atomic insert and update rejection, default char declaration, 3 MySQL checkpoints"
    );
}

#[test]
fn pg_scalar_25() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain.clone(), Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    mysql_query(&mut mysql, "CREATE TABLE hex25_mysql (v VARBINARY(4))");
    mysql_query(&mut mysql, "INSERT INTO hex25_mysql VALUES (X'00aaff00')");
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT v FROM hex25_mysql");
        assert_eq!(mysql_read(socket), vec![1]);
        let metadata = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x04\x00\xaa\xff\x00");
        assert_eq!(&mysql_read(socket)[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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

    let describe = |m: &(u8, Vec<u8>)| {
        assert_eq!(m.0, b'T', "{m:?}");
        let offset = 3 + m.1[2..].iter().position(|b| *b == 0).unwrap();
        assert_eq!(&m.1[offset + 6..offset + 10], &17u32.to_be_bytes());
        assert_eq!(&m.1[offset + 10..offset + 12], &(-1i16).to_be_bytes());
        assert_eq!(&m.1[offset + 12..offset + 16], &(-1i32).to_be_bytes());
    };
    query(&mut socket, "CREATE TABLE public.hex25 (v bytea)");
    describe(&query(&mut socket, "SELECT v FROM public.hex25 WHERE FALSE")[0]);
    parse(&mut socket, "hex25", "SELECT $1", &[17]);
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'D', b"Shex25\0");
    assert_eq!(
        read(&mut socket),
        (
            b't',
            [1i16.to_be_bytes().as_slice(), &17u32.to_be_bytes()].concat()
        )
    );
    describe(&read(&mut socket));
    for (input, expected) in [
        (Some("\\x"), Some("\\x")),
        (Some("\\x00aAff00"), Some("\\x00aaff00")),
        (Some("\\x 00\tAA\nff\r00 "), Some("\\x00aaff00")),
        (Some("\\x \t\n\r"), Some("\\x")),
        (None, None),
    ] {
        bind(&mut socket, "", "hex25", &[input]);
        assert_eq!(read(&mut socket).0, b'2', "{input:?}");
        send(&mut socket, b'D', b"P\0");
        describe(&read(&mut socket));
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[expected])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    parse(
        &mut socket,
        "write25",
        "INSERT INTO public.hex25 VALUES ($1)",
        &[17],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        for (input, state) in [
            (b"\\x0 0".as_slice(), "22023"),
            (b"\\x0", "22023"),
            (b"\\xgg", "22023"),
            (b"\\x00\0", "22021"),
            (b"\\x\xff", "22021"),
        ] {
            if transaction {
                query(&mut socket, "BEGIN");
            }
            bind_formats(&mut socket, "write25", &[0], &[Some(input)]);
            assert_state(&read(&mut socket), state);
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                query(&mut socket, "ROLLBACK");
            }
            assert_eq!(
                query(&mut socket, "SELECT count(*) FROM public.hex25")[1],
                (b'D', row(&[Some("0")]))
            );
        }
    }
    query(&mut socket, "DROP TABLE public.hex25");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    eprintln!(
        "pg_scalar_25: 5 value/NULL cases, zero-row and statement/portal metadata, 10 Bind rejection/Sync/ROLLBACK/no-write cases, 3 MySQL checkpoints"
    );
}

#[test]
fn pg_scalar_26() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain.clone(), Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    mysql_query(&mut mysql, "CREATE TABLE escape26_mysql (v VARBINARY(4))");
    mysql_query(
        &mut mysql,
        "INSERT INTO escape26_mysql VALUES (X'00aaff00')",
    );
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT v FROM escape26_mysql");
        assert_eq!(mysql_read(socket), vec![1]);
        let metadata = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x04\x00\xaa\xff\x00");
        assert_eq!(&mysql_read(socket)[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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

    let describe = |m: &(u8, Vec<u8>)| {
        assert_eq!(m.0, b'T', "{m:?}");
        let offset = 3 + m.1[2..].iter().position(|b| *b == 0).unwrap();
        assert_eq!(&m.1[offset + 6..offset + 10], &17u32.to_be_bytes());
        assert_eq!(&m.1[offset + 10..offset + 12], &(-1i16).to_be_bytes());
        assert_eq!(&m.1[offset + 12..offset + 16], &(-1i32).to_be_bytes());
    };
    query(&mut socket, "CREATE TABLE public.escape26 (v bytea)");
    describe(&query(&mut socket, "SELECT v FROM public.escape26 WHERE FALSE")[0]);
    parse(&mut socket, "escape26", "SELECT $1", &[17]);
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'D', b"Sescape26\0");
    assert_eq!(
        read(&mut socket),
        (
            b't',
            [1i16.to_be_bytes().as_slice(), &17u32.to_be_bytes()].concat()
        )
    );
    describe(&read(&mut socket));
    for (input, expected) in [
        (Some("abc"), Some("\\x616263")),
        (Some("\\\\"), Some("\\x5c")),
        (Some("\\000\\377\\\\"), Some("\\x00ff5c")),
        (Some("\\303\\050"), Some("\\xc328")),
        (Some("a\\000b\\377c"), Some("\\x610062ff63")),
        (Some(""), Some("\\x")),
        (Some("\\x00ff5c"), Some("\\x00ff5c")),
        (None, None),
    ] {
        bind(&mut socket, "", "escape26", &[input]);
        assert_eq!(read(&mut socket).0, b'2', "{input:?}");
        send(&mut socket, b'D', b"P\0");
        describe(&read(&mut socket));
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[expected])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    parse(
        &mut socket,
        "write26",
        "INSERT INTO public.escape26 VALUES ($1)",
        &[17],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        for (input, state) in [
            (b"\\400".as_slice(), "22P02"),
            (b"\\08", "22P02"),
            (b"\\q", "22P02"),
            (b"\\", "22P02"),
            (b"\\00", "22P02"),
            (b"\\378", "22P02"),
            (b"a\0b", "22021"),
            (b"\xff", "22021"),
        ] {
            if transaction {
                query(&mut socket, "BEGIN");
            }
            bind_formats(&mut socket, "write26", &[0], &[Some(input)]);
            assert_state(&read(&mut socket), state);
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                query(&mut socket, "ROLLBACK");
            }
            assert_eq!(
                query(&mut socket, "SELECT count(*) FROM public.escape26")[1],
                (b'D', row(&[Some("0")]))
            );
        }
    }
    query(&mut socket, "DROP TABLE public.escape26");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    eprintln!(
        "pg_scalar_26: 8 value/NULL cases, zero-row and statement/portal metadata, 16 Bind rejection/Sync/ROLLBACK/no-write cases, 3 MySQL checkpoints"
    );
}

#[test]
fn pg_scalar_27() {
    use crate::pg_client_integration_test::{
        mysql_handshake, mysql_packet, mysql_query, mysql_read, start_client_server,
    };
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain.clone(), Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
    let mut mysql = TcpStream::connect(cleanup.0.listener_addr().unwrap()).unwrap();
    mysql_handshake(&mut mysql);
    mysql_query(&mut mysql, "CREATE TABLE output27_mysql (v VARBINARY(4))");
    mysql_query(
        &mut mysql,
        "INSERT INTO output27_mysql VALUES (X'00aaff00')",
    );
    let mysql_check = |socket: &mut TcpStream| {
        mysql_packet(socket, 0, b"\x03SELECT v FROM output27_mysql");
        assert_eq!(mysql_read(socket), vec![1]);
        let metadata = mysql_read(socket);
        assert_eq!(mysql_read(socket)[0], 0xfe);
        assert_eq!(mysql_read(socket), b"\x04\x00\xaa\xff\x00");
        assert_eq!(&mysql_read(socket)[..3], &[0xfe, 0, 0]);
        assert!(mysql_query(socket, "SHOW WARNINGS").is_empty());
        metadata
    };
    let mysql_metadata = mysql_check(&mut mysql);
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

    let describe = |m: &(u8, Vec<u8>)| {
        assert_eq!(m.0, b'T', "{m:?}");
        let offset = 3 + m.1[2..].iter().position(|b| *b == 0).unwrap();
        assert_eq!(&m.1[offset + 6..offset + 10], &17u32.to_be_bytes());
        assert_eq!(&m.1[offset + 10..offset + 12], &(-1i16).to_be_bytes());
        assert_eq!(&m.1[offset + 12..offset + 16], &(-1i32).to_be_bytes());
    };
    query(&mut socket, "CREATE TABLE public.output27 (v bytea)");
    describe(&query(&mut socket, "SELECT v FROM public.output27 WHERE FALSE")[0]);
    parse(&mut socket, "output27", "SELECT $1", &[17]);
    assert_eq!(read(&mut socket).0, b'1');
    send(&mut socket, b'D', b"Soutput27\0");
    assert_eq!(
        read(&mut socket),
        (
            b't',
            [1i16.to_be_bytes().as_slice(), &17u32.to_be_bytes()].concat()
        )
    );
    describe(&read(&mut socket));
    for (input, expected) in [
        (Some(""), Some("\\x")),
        (Some("\\000\\377\\\\"), Some("\\x00ff5c")),
        (Some("\\x0000ff5c00"), Some("\\x0000ff5c00")),
        (Some("\\303\\050"), Some("\\xc328")),
        (None, None),
    ] {
        // Rebind the canonical output to prove text round trips without UTF-8 loss.
        for value in [input, expected] {
            bind(&mut socket, "", "output27", &[value]);
            assert_eq!(read(&mut socket).0, b'2', "{value:?}");
            send(&mut socket, b'D', b"P\0");
            describe(&read(&mut socket));
            execute(&mut socket, "", 0);
            let message = read(&mut socket);
            assert_eq!(message, (b'D', row(&[expected])));
            let length = i32::from_be_bytes(message.1[2..6].try_into().unwrap());
            assert_eq!(length, expected.map_or(-1, |v| v.len() as i32));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', vec![b'I']));
        }
    }
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    parse(
        &mut socket,
        "write27",
        "INSERT INTO public.output27 VALUES ($1)",
        &[17],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for transaction in [false, true] {
        for (input, state) in [
            (b"\\400".as_slice(), "22P02"),
            (b"\\08", "22P02"),
            (b"\\q", "22P02"),
            (b"\\", "22P02"),
            (b"\\00", "22P02"),
            (b"\\378", "22P02"),
            (b"a\0b", "22021"),
            (b"\xff", "22021"),
        ] {
            if transaction {
                query(&mut socket, "BEGIN");
            }
            bind_formats(&mut socket, "write27", &[0], &[Some(input)]);
            assert_state(&read(&mut socket), state);
            send(&mut socket, b'S', &[]);
            assert_eq!(
                read(&mut socket),
                (b'Z', vec![if transaction { b'E' } else { b'I' }])
            );
            if transaction {
                query(&mut socket, "ROLLBACK");
            }
            assert_eq!(
                query(&mut socket, "SELECT count(*) FROM public.output27")[1],
                (b'D', row(&[Some("0")]))
            );
        }
    }
    query(&mut socket, "DROP TABLE public.output27");
    assert_eq!(mysql_check(&mut mysql), mysql_metadata);
    eprintln!(
        "pg_scalar_27: 5 value/NULL roundtrip cases (10 executions), zero-row and statement/portal metadata, 16 Bind rejection/Sync/ROLLBACK/no-write cases, 3 MySQL checkpoints"
    );
}

#[test]
fn pg05_uuid_input_text_codec() {
    use crate::pg_extended::parameter;
    let canonical = b"00112233-4455-6677-8899-aabbccddeeff";
    for input in [
        canonical.as_slice(),
        b"00112233445566778899AABBCCDDEEFF",
        b"{00112233445566778899AABBCCDDEEFF}",
        b"0011-2233-4455-6677-8899-aabb-ccdd-eeff",
    ] {
        let value = parameter(2950, Some(input)).unwrap();
        assert!(!value.is_null);
        assert_eq!(value.tp, 253);
        assert_eq!(value.value, canonical);
    }
    for input in [
        b"".as_slice(),
        b"00112233445566778899aabbccddeef",
        b"00112233445566778899aabbccddeeff0",
        b"00112233-4455-6677-8899-aabbccddeefg",
        b"00112233-4455-6677-8899-aabbccddeeff-",
        b"{00112233445566778899aabbccddeeff",
        b"00112233445566778899aabbccddeeff}",
        b"urn:uuid:00112233-4455-6677-8899-aabbccddeeff",
        b" 00112233-4455-6677-8899-aabbccddeeff",
        b"00112233-4455-6677-8899-aabbccddeeff ",
        b"0-0112233445566778899aabbccddeeff",
    ] {
        assert_eq!(
            parameter(2950, Some(input)).unwrap_err().0,
            "22P02",
            "{input:?}"
        );
    }
    for input in [
        b"\xff".as_slice(),
        b"00112233\0-4455-6677-8899-aabbccddeeff",
    ] {
        assert_eq!(parameter(2950, Some(input)).unwrap_err().0, "22021");
    }
    assert!(parameter(2950, None).unwrap().is_null);
}

#[test]
fn pg05_uuid_input_parse_bind_execute_and_recovery() {
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
    // Explicit OID, cast inference and an OID-zero declaration use the real engine.
    for (name, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![2950]),
        ("inferred", "SELECT $1::uuid AS v", vec![]),
        ("zero", "SELECT $1::uuid AS v", vec![0]),
    ] {
        parse(&mut socket, name, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]), "{name}");
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &2950u32.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        assert_eq!(description.0, b'T');
        let mut expected_description = 1i16.to_be_bytes().to_vec();
        expected_description.extend_from_slice(b"v\0");
        expected_description.extend_from_slice(&0u32.to_be_bytes());
        expected_description.extend_from_slice(&0i16.to_be_bytes());
        expected_description.extend_from_slice(&2950u32.to_be_bytes());
        expected_description.extend_from_slice(&16i16.to_be_bytes());
        expected_description.extend_from_slice(&(-1i32).to_be_bytes());
        expected_description.extend_from_slice(&0i16.to_be_bytes());
        assert_eq!(description.1, expected_description);
        let bits = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        for (format, bytes, expected) in [
            (
                0,
                Some(b"{00112233445566778899AABBCCDDEEFF}".as_slice()),
                Some("00112233-4455-6677-8899-aabbccddeeff"),
            ),
            (
                1,
                Some(bits.as_slice()),
                Some("00112233-4455-6677-8899-aabbccddeeff"),
            ),
            (
                1,
                Some([0u8; 16].as_slice()),
                Some("00000000-0000-0000-0000-000000000000"),
            ),
            (
                1,
                Some([255u8; 16].as_slice()),
                Some("ffffffff-ffff-ffff-ffff-ffffffffffff"),
            ),
            (
                1,
                Some([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128].as_slice()),
                Some("01000000-0000-0000-0000-000000000080"),
            ),
            (0, None, None),
            (1, None, None),
        ] {
            bind_formats(&mut socket, name, &[format], &[bytes]);
            assert_eq!(read(&mut socket), (b'2', vec![]), "{name}/{format}");
            send(&mut socket, b'D', b"P\0");
            assert_eq!(read(&mut socket), description);
            execute(&mut socket, "", 0);
            assert_eq!(
                read(&mut socket),
                (b'D', row(&[expected])),
                "{name}/{format}"
            );
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        }
    }
    for (format, bytes, state) in [
        (1, b"".as_slice(), "08P01"),
        (1, &[0; 15], "08P01"),
        (1, &[0; 17], "22P03"),
        (0, b"bad", "22P02"),
        (0, b"\xff", "22021"),
        (0, b"\0", "22021"),
    ] {
        bind_formats(&mut socket, "declared", &[format], &[Some(bytes)]);
        let response = read(&mut socket);
        assert_eq!(response.0, b'E');
        assert!(
            response
                .1
                .windows(6)
                .any(|w| w == [b"C", state.as_bytes()].concat()),
            "{response:?}"
        );
        // Ignore Execute until Sync, then reuse the same statement successfully.
        execute(&mut socket, "", 0);
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        bind_formats(
            &mut socket,
            "declared",
            &[0],
            &[Some(b"00112233445566778899aabbccddeeff")],
        );
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(
            read(&mut socket),
            (b'D', row(&[Some("00112233-4455-6677-8899-aabbccddeeff")]))
        );
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    for name in ["declared", "inferred", "zero"] {
        send(&mut socket, b'C', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket).0, b'3');
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_uuid_output_portal_formats_rebinding_and_empty_rows() {
    fn description(formats: &[i16]) -> Vec<u8> {
        let mut body = (formats.len() as i16).to_be_bytes().to_vec();
        for (name, format) in ["v", "n"].into_iter().zip(formats) {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            body.extend_from_slice(&[0; 6]);
            body.extend_from_slice(&2950u32.to_be_bytes());
            body.extend_from_slice(&16i16.to_be_bytes());
            body.extend_from_slice(&(-1i32).to_be_bytes());
            body.extend_from_slice(&format.to_be_bytes());
        }
        body
    }
    fn bind_output(
        socket: &mut TcpStream,
        portal: &str,
        statement: &str,
        value: Option<&str>,
        formats: &[i16],
    ) {
        let mut body = [
            portal.as_bytes(),
            b"\0",
            statement.as_bytes(),
            b"\0",
            &0i16.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        if let Some(value) = value {
            body.extend_from_slice(&(value.len() as i32).to_be_bytes());
            body.extend_from_slice(value.as_bytes());
        } else {
            body.extend_from_slice(&(-1i32).to_be_bytes());
        }
        body.extend_from_slice(&(formats.len() as i16).to_be_bytes());
        for format in formats {
            body.extend_from_slice(&format.to_be_bytes());
        }
        send(socket, b'B', &body);
        assert_eq!(read(socket), (b'2', vec![]));
        send(socket, b'D', &[b"P", portal.as_bytes(), b"\0"].concat());
        assert_eq!(read(socket), (b'T', description(formats)));
    }
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
    let text = "00112233-4455-6677-8899-aabbccddeeff";
    let bits = [
        0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255,
    ];
    for (statement, prefix, suffix, row_count) in [
        ("values", "", "", 1),
        ("empty", "", " WHERE false", 0),
        (
            "first_null",
            "SELECT NULL::uuid AS v, NULL::uuid AS n UNION ALL ",
            "",
            2,
        ),
    ] {
        parse(
            &mut socket,
            statement,
            &format!("{prefix}SELECT $1::uuid AS v, NULL::uuid AS n{suffix}"),
            if row_count == 2 { &[2950] } else { &[] },
        );
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &2950u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), (b'T', description(&[0, 0])));
        // Two live portals retain independent per-column result formats.
        for (portal, formats) in [("a", [0, 1]), ("b", [1, 0])] {
            bind_output(&mut socket, portal, statement, Some(text), &formats);
        }
        for (portal, format) in [("b", 1), ("a", 0)] {
            for execution in 0..2 {
                execute(&mut socket, portal, 0);
                if row_count > 0 && execution == 0 {
                    if row_count == 2 {
                        assert_eq!(read(&mut socket), (b'D', row(&[None, None])));
                    }
                    let payload = if format == 1 {
                        bits.as_slice()
                    } else {
                        text.as_bytes()
                    };
                    let expected = [
                        2i16.to_be_bytes().as_slice(),
                        &(payload.len() as i32).to_be_bytes(),
                        payload,
                        &(-1i32).to_be_bytes(),
                    ]
                    .concat();
                    assert_eq!(read(&mut socket), (b'D', expected));
                }
                assert_eq!(
                    read(&mut socket),
                    (b'C', format!("SELECT {row_count}\0").into_bytes())
                );
            }
        }
        // Unnamed portal rebinding changes formats and value/NULL, never identity.
        for (formats, value) in [([1, 1], None), ([0, 0], Some(text)), ([1, 0], None)] {
            bind_output(&mut socket, "", statement, value, &formats);
            execute(&mut socket, "", 0);
            if row_count > 0 {
                if row_count == 2 {
                    assert_eq!(read(&mut socket), (b'D', row(&[None, None])));
                }
                assert_eq!(read(&mut socket), (b'D', row(&[value, None])));
            }
            assert_eq!(read(&mut socket).0, b'C');
        }
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        for portal in ["a", "b"] {
            send(
                &mut socket,
                b'C',
                &[b"P", portal.as_bytes(), b"\0"].concat(),
            );
            assert_eq!(read(&mut socket), (b'3', vec![]));
        }
        send(
            &mut socket,
            b'C',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(read(&mut socket), (b'3', vec![]));
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_json_input_text_binary_codec() {
    pg05_json_input_codec(crate::pg_extended::parameter);
    pg05_json_input_codec(crate::pg_extended::binary_parameter);
}

fn pg05_json_input_codec(
    parameter: fn(
        u32,
        Option<&[u8]>,
    ) -> Result<crate::conn_stmt::BinaryParam, (&'static str, String)>,
) {
    for input in [
        r#" { "b":1, "a":2,"b":3 } "#,
        "\t[null, true, false, {\"x\": []}]\n",
        "1.230e+02",
        "9007199254740993",
        "18446744073709551616",
        "1e1000000",
        "0.1234567890123456789012345678901234567890",
        "42",
        "true",
        "false",
        r#""x""#,
        "[]",
        "{}",
        "null",
        r#""null""#,
        r#""中""#,
        r#""\u4e2d""#,
        r#""\u0000""#,
        r#""\uD800""#,
    ] {
        let value = parameter(114, Some(input.as_bytes())).unwrap();
        assert!(!value.is_null);
        assert_eq!(value.tp, 253);
        assert_eq!(value.value, input.as_bytes());
    }
    for input in [
        b"".as_slice(),
        b" \t\n",
        b"{} []",
        b"[1,]",
        b"\"x",
        b"/*x*/{}",
        b"01",
        b"+1",
        b"NaN",
        b"Infinity",
        b"\"\\uZZZZ\"",
        b"\"\n\"",
        b"\x01{}",
    ] {
        assert_eq!(
            parameter(114, Some(input)).unwrap_err().0,
            "22P02",
            "{input:?}"
        );
    }
    for input in [b"\xff".as_slice(), b"\"\0\""] {
        assert_eq!(parameter(114, Some(input)).unwrap_err().0, "22021");
    }
    assert!(parameter(114, None).unwrap().is_null);
}

#[test]
fn pg05_json_input_parse_bind_execute_and_recovery() {
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
    for (name, sql, oids) in [
        ("json_declared", "SELECT $1 AS v", vec![114]),
        ("json_inferred", "SELECT $1::json AS v", vec![]),
        ("json_zero", "SELECT $1::json AS v", vec![0]),
    ] {
        parse(&mut socket, name, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]), "{name}");
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &114u32.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        let expected = [
            1i16.to_be_bytes().as_slice(),
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &114u32.to_be_bytes(),
            &(-1i16).to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat();
        assert_eq!(description, (b'T', expected));
        for format in [0, 1] {
            for input in [
                None,
                Some("null"),
                Some(r#""null""#),
                Some(r#" { "b":1, "a":2,"b":3 } "#),
                Some("1e1000000"),
                Some("9007199254740993"),
                Some("18446744073709551616"),
                Some("1.230e+02"),
                Some("0.1234567890123456789012345678901234567890"),
                Some("\t[true,false,null]\n"),
                Some(r#""x""#),
                Some("42"),
                Some(r#""中""#),
                Some(r#""\u4e2d""#),
                Some(r#""\u0000""#),
                Some(r#""\uD800""#),
            ] {
                bind_formats(&mut socket, name, &[format], &[input.map(str::as_bytes)]);
                assert_eq!(
                    read(&mut socket),
                    (b'2', vec![]),
                    "{name}/{format}/{input:?}"
                );
                send(&mut socket, b'D', b"P\0");
                assert_eq!(read(&mut socket), description);
                execute(&mut socket, "", 0);
                assert_eq!(
                    read(&mut socket),
                    (b'D', row(&[input])),
                    "{name}/{format}/{input:?}"
                );
                assert_eq!(read(&mut socket).0, b'C');
                send(&mut socket, b'S', &[]);
                assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
            }
        }
    }
    for format in [0, 1] {
        for (input, state) in [
            (b"".as_slice(), "22P02"),
            (b" \t\n", "22P02"),
            (b"{} []", "22P02"),
            (b"[1,]", "22P02"),
            (b"\x01{}", "22P02"),
            (b"\"\\uZZZZ\"", "22P02"),
            (b"\"\n\"", "22P02"),
            (b"\xff", "22021"),
            (b"\0", "22021"),
        ] {
            bind_formats(&mut socket, "json_declared", &[format], &[Some(input)]);
            assert_state(&read(&mut socket), state);
            execute(&mut socket, "", 0);
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
            bind_formats(&mut socket, "json_declared", &[format], &[Some(b"null")]);
            assert_eq!(read(&mut socket).0, b'2');
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[Some("null")])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket).0, b'Z');
        }
    }
    for name in ["json_declared", "json_inferred", "json_zero"] {
        send(&mut socket, b'C', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket).0, b'3');
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_json_input_shared_prepared_identity_and_rebinding() {
    use astersql_session::runtime::{
        ConcretePreparedArgument, ConcreteSession, CreateAnalyzeSession,
    };
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg =
        ConcreteSession::new_with_dialect(domain, astersql_expression::ast::SqlDialect::PostgreSQL);
    for (sql, oid) in [("SELECT ? AS v", 114), ("SELECT ?::json AS v", 0)] {
        assert_eq!(pg.protocol_parameter_types(sql, &[oid]).unwrap(), [114]);
        let (id, _, fields) = pg
            .prepare_protocol_statement_with_parameter_types(sql, &[oid])
            .unwrap();
        assert_eq!(
            fields[0].column.FieldType.GetType(),
            astersql_parser_mysql::r#type::TypePgJson
        );
        for decode in [
            crate::pg_extended::parameter,
            crate::pg_extended::binary_parameter,
        ] {
            for input in [
                None,
                Some("null"),
                Some(r#""null""#),
                Some(r#" { "b":1, "a":2,"b":3 } "#),
                Some("1e1000000"),
                Some(r#""\uD800""#),
            ] {
                let value = decode(114, input.map(str::as_bytes)).unwrap();
                let argument = if value.is_null {
                    ConcretePreparedArgument::Null
                } else {
                    assert_eq!(value.tp, 253);
                    ConcretePreparedArgument::Text(String::from_utf8(value.value).unwrap())
                };
                let mut rows = pg.execute_protocol_statement(id, &[argument]).unwrap();
                assert_eq!(
                    rows[0].result_fields()[0]
                        .as_ref()
                        .unwrap()
                        .column
                        .FieldType
                        .GetType(),
                    astersql_parser_mysql::r#type::TypePgJson
                );
                assert_eq!(
                    rows[0].next_row().unwrap().unwrap(),
                    [input.unwrap_or("<nil>")]
                );
            }
        }
        pg.close_protocol_statement(id).unwrap();
    }
}

#[test]
fn pg05_json_output_portal_formats_rebinding_and_empty_rows() {
    fn description(formats: &[i16]) -> Vec<u8> {
        let mut body = (formats.len() as i16).to_be_bytes().to_vec();
        for (name, format) in ["v", "n"].into_iter().zip(formats) {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            body.extend_from_slice(&[0; 6]);
            body.extend_from_slice(&114u32.to_be_bytes());
            body.extend_from_slice(&(-1i16).to_be_bytes());
            body.extend_from_slice(&(-1i32).to_be_bytes());
            body.extend_from_slice(&format.to_be_bytes());
        }
        body
    }
    fn bind_output(
        socket: &mut TcpStream,
        portal: &str,
        statement: &str,
        value: Option<&str>,
        formats: &[i16],
    ) {
        let mut body = [
            portal.as_bytes(),
            b"\0",
            statement.as_bytes(),
            b"\0",
            &0i16.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        if let Some(value) = value {
            body.extend_from_slice(&(value.len() as i32).to_be_bytes());
            body.extend_from_slice(value.as_bytes());
        } else {
            body.extend_from_slice(&(-1i32).to_be_bytes());
        }
        body.extend_from_slice(&(formats.len() as i16).to_be_bytes());
        for format in formats {
            body.extend_from_slice(&format.to_be_bytes());
        }
        send(socket, b'B', &body);
        assert_eq!(read(socket), (b'2', vec![]));
        send(socket, b'D', &[b"P", portal.as_bytes(), b"\0"].concat());
        assert_eq!(read(socket), (b'T', description(formats)));
    }
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
    let text = r#" { "b":1, "a":2,"b":3, "x":"\u0000" } "#;
    for (statement, prefix, suffix, row_count) in [
        ("values", "", "", 1),
        ("empty", "", " WHERE false", 0),
        (
            "first_null",
            "SELECT NULL::json AS v, NULL::json AS n UNION ALL ",
            "",
            2,
        ),
    ] {
        parse(
            &mut socket,
            statement,
            &format!("{prefix}SELECT $1::json AS v, NULL::json AS n{suffix}"),
            if row_count == 2 { &[114] } else { &[] },
        );
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &114u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), (b'T', description(&[0, 0])));
        // Two live portals retain independent per-column result formats.
        for (portal, formats) in [("a", [0, 1]), ("b", [1, 0])] {
            bind_output(&mut socket, portal, statement, Some(text), &formats);
        }
        for portal in ["b", "a"] {
            for execution in 0..2 {
                execute(&mut socket, portal, 0);
                if row_count > 0 && execution == 0 {
                    if row_count == 2 {
                        assert_eq!(read(&mut socket), (b'D', row(&[None, None])));
                    }
                    let payload = text.as_bytes();
                    let expected = [
                        2i16.to_be_bytes().as_slice(),
                        &(payload.len() as i32).to_be_bytes(),
                        payload,
                        &(-1i32).to_be_bytes(),
                    ]
                    .concat();
                    assert_eq!(read(&mut socket), (b'D', expected));
                }
                assert_eq!(
                    read(&mut socket),
                    (b'C', format!("SELECT {row_count}\0").into_bytes())
                );
            }
        }
        // Unnamed portal rebinding changes formats and value/NULL, never identity.
        for (formats, value) in [
            ([1, 1], None),
            ([0, 0], Some(text)),
            ([1, 0], None),
            ([1, 1], Some("null")),
            ([0, 1], Some(r#""null""#)),
        ] {
            bind_output(&mut socket, "", statement, value, &formats);
            execute(&mut socket, "", 0);
            if row_count > 0 {
                if row_count == 2 {
                    assert_eq!(read(&mut socket), (b'D', row(&[None, None])));
                }
                assert_eq!(read(&mut socket), (b'D', row(&[value, None])));
            }
            assert_eq!(read(&mut socket).0, b'C');
        }
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        for portal in ["a", "b"] {
            send(
                &mut socket,
                b'C',
                &[b"P", portal.as_bytes(), b"\0"].concat(),
            );
            assert_eq!(read(&mut socket), (b'3', vec![]));
        }
        send(
            &mut socket,
            b'C',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(read(&mut socket), (b'3', vec![]));
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_jsonb_input_text_binary_codec() {
    for (input, expected) in [
        (r#" { "b":1, "a":2,"b":3 } "#, r#"{"a": 2, "b": 3}"#),
        ("18446744073709551617", "18446744073709551617"),
        ("1.230e-5", "0.00001230"),
        ("[null,true,1,1]", "[null, true, 1, 1]"),
        (r#""\ud83d\ude00""#, "\"😀\""),
        ("null", "null"),
        (r#""null""#, r#""null""#),
    ] {
        for binary in [false, true] {
            let bytes = if binary {
                [b"\x01".as_slice(), input.as_bytes()].concat()
            } else {
                input.as_bytes().to_vec()
            };
            let decode = if binary {
                crate::pg_extended::binary_parameter
            } else {
                crate::pg_extended::parameter
            };
            let value = decode(3802, Some(&bytes)).unwrap();
            assert!(!value.is_null);
            assert_eq!(value.tp, 253);
            assert_eq!(value.value, expected.as_bytes());
        }
    }
    for (input, state) in [
        (b"".as_slice(), "22P02"),
        (b"{} []", "22P02"),
        (b"[1,]", "22P02"),
        (b"{}garbage", "22P02"),
        (b"\"\\u0000\"", "22P05"),
        (b"\"\\ud800\"", "22P02"),
        (b"1e131072", "22003"),
        (b"\xff", "22021"),
        (b"\0", "22021"),
    ] {
        assert_eq!(
            crate::pg_extended::parameter(3802, Some(input))
                .unwrap_err()
                .0,
            state
        );
        let bytes = [b"\x01".as_slice(), input].concat();
        assert_eq!(
            crate::pg_extended::binary_parameter(3802, Some(&bytes))
                .unwrap_err()
                .0,
            state
        );
    }
    assert_eq!(
        crate::pg_extended::binary_parameter(3802, Some(b""))
            .unwrap_err()
            .0,
        "08P01"
    );
    // PostgreSQL 18.6 jsonb_recv, captured using a separate reference cluster.
    for input in [b"\x00{}".as_slice(), b"\x02{}", b"\xff{}"] {
        assert_eq!(
            crate::pg_extended::binary_parameter(3802, Some(input))
                .unwrap_err()
                .0,
            "XX000"
        );
    }
    for decode in [
        crate::pg_extended::parameter,
        crate::pg_extended::binary_parameter,
    ] {
        assert!(decode(3802, None).unwrap().is_null);
    }
}

#[test]
fn pg05_jsonb_input_parse_bind_execute_and_recovery() {
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
    for (name, sql, oids) in [
        ("jsonb_declared", "SELECT $1 AS v", vec![3802]),
        ("jsonb_inferred", "SELECT $1::jsonb AS v", vec![]),
        ("jsonb_zero", "SELECT $1::jsonb AS v", vec![0]),
    ] {
        parse(&mut socket, name, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]), "{name}");
        send(&mut socket, b'D', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &3802u32.to_be_bytes()].concat()
            )
        );
        let description = read(&mut socket);
        let expected = [
            1i16.to_be_bytes().as_slice(),
            b"v\0",
            &0u32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &3802u32.to_be_bytes(),
            &(-1i16).to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat();
        assert_eq!(description, (b'T', expected));
        for format in [0, 1] {
            for (input, expected) in [
                (None, None),
                (Some("null"), Some("null")),
                (Some(r#""null""#), Some(r#""null""#)),
                (
                    Some(r#" { "b":1, "a":2,"b":3 } "#),
                    Some(r#"{"a": 2, "b": 3}"#),
                ),
                (Some("18446744073709551617"), Some("18446744073709551617")),
                (Some("1.230e-5"), Some("0.00001230")),
                (Some("[true,false,null]"), Some("[true, false, null]")),
            ] {
                let payload = input.map(|v| {
                    if format == 1 {
                        [b"\x01".as_slice(), v.as_bytes()].concat()
                    } else {
                        v.as_bytes().to_vec()
                    }
                });
                bind_formats(&mut socket, name, &[format], &[payload.as_deref()]);
                assert_eq!(
                    read(&mut socket),
                    (b'2', vec![]),
                    "{name}/{format}/{input:?}"
                );
                send(&mut socket, b'D', b"P\0");
                assert_eq!(read(&mut socket), description);
                execute(&mut socket, "", 0);
                assert_eq!(
                    read(&mut socket),
                    (b'D', row(&[expected])),
                    "{name}/{format}/{input:?}"
                );
                assert_eq!(read(&mut socket).0, b'C');
                send(&mut socket, b'S', &[]);
                assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
            }
        }
    }
    for format in [0, 1] {
        for (input, state) in [
            (b"".as_slice(), "22P02"),
            (b" \t\n", "22P02"),
            (b"{} []", "22P02"),
            (b"[1,]", "22P02"),
            (b"\x01{}", "22P02"),
            (b"\"\\uZZZZ\"", "22P02"),
            (b"\"\n\"", "22P02"),
            (b"\xff", "22021"),
            (b"\0", "22021"),
        ] {
            let payload = if format == 1 {
                [b"\x01".as_slice(), input].concat()
            } else {
                input.to_vec()
            };
            bind_formats(&mut socket, "jsonb_declared", &[format], &[Some(&payload)]);
            assert_state(&read(&mut socket), state);
            execute(&mut socket, "", 0);
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
            bind_formats(
                &mut socket,
                "jsonb_declared",
                &[format],
                &[Some(if format == 1 { b"\x01null" } else { b"null" })],
            );
            assert_eq!(read(&mut socket).0, b'2');
            execute(&mut socket, "", 0);
            assert_eq!(read(&mut socket), (b'D', row(&[Some("null")])));
            assert_eq!(read(&mut socket).0, b'C');
            send(&mut socket, b'S', &[]);
            assert_eq!(read(&mut socket).0, b'Z');
        }
    }
    for (input, state) in [
        (b"".as_slice(), "08P01"),
        (b"\x01", "22P02"),
        (b"\x00{}", "XX000"),
        (b"\x02{}", "XX000"),
        (b"\xff{}", "XX000"),
        (b"\x01{}garbage", "22P02"),
        (b"\x01\"\\ud800\"", "22P02"),
        (b"\x01\"\\u0000\"", "22P05"),
        (b"\x011e131072", "22003"),
    ] {
        bind_formats(&mut socket, "jsonb_declared", &[1], &[Some(input)]);
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        bind_formats(&mut socket, "jsonb_declared", &[1], &[Some(b"\x01null")]);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), (b'D', row(&[Some("null")])));
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
    // One version byte is present, but the parameter declares two bytes.
    let truncated = [
        b"\0jsonb_declared\0".as_slice(),
        &1i16.to_be_bytes(),
        &1i16.to_be_bytes(),
        &1i16.to_be_bytes(),
        &2i32.to_be_bytes(),
        b"\x01",
    ]
    .concat();
    send(&mut socket, b'B', &truncated);
    assert_state(&read(&mut socket), "08P01");
    send(&mut socket, b'S', &[]);
    assert_eq!(read(&mut socket).0, b'Z');
    for name in ["jsonb_declared", "jsonb_inferred", "jsonb_zero"] {
        send(&mut socket, b'C', &[b"S", name.as_bytes(), b"\0"].concat());
        assert_eq!(read(&mut socket).0, b'3');
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_jsonb_output_portal_formats_rebinding_and_empty_rows() {
    fn output_row(value: Option<&str>, format: i16) -> Vec<u8> {
        let payload = value.map(|v| {
            let canonical = astersql_types::pg_jsonb::PgJsonb::from_bytes(v.as_bytes()).unwrap();
            [
                if format == 1 { b"\x01".as_slice() } else { b"" },
                canonical.as_bytes(),
            ]
            .concat()
        });
        let mut body = 2i16.to_be_bytes().to_vec();
        if let Some(payload) = payload {
            body.extend_from_slice(&(payload.len() as i32).to_be_bytes());
            body.extend_from_slice(&payload);
        } else {
            body.extend_from_slice(&(-1i32).to_be_bytes());
        }
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body
    }
    fn description(formats: &[i16]) -> Vec<u8> {
        let mut body = (formats.len() as i16).to_be_bytes().to_vec();
        for (name, format) in ["v", "n"].into_iter().zip(formats) {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
            body.extend_from_slice(&[0; 6]);
            body.extend_from_slice(&3802u32.to_be_bytes());
            body.extend_from_slice(&(-1i16).to_be_bytes());
            body.extend_from_slice(&(-1i32).to_be_bytes());
            body.extend_from_slice(&format.to_be_bytes());
        }
        body
    }
    fn bind_output(
        socket: &mut TcpStream,
        portal: &str,
        statement: &str,
        value: Option<&str>,
        formats: &[i16],
    ) {
        let mut body = [
            portal.as_bytes(),
            b"\0",
            statement.as_bytes(),
            b"\0",
            &0i16.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        if let Some(value) = value {
            body.extend_from_slice(&(value.len() as i32).to_be_bytes());
            body.extend_from_slice(value.as_bytes());
        } else {
            body.extend_from_slice(&(-1i32).to_be_bytes());
        }
        body.extend_from_slice(&(formats.len() as i16).to_be_bytes());
        for format in formats {
            body.extend_from_slice(&format.to_be_bytes());
        }
        send(socket, b'B', &body);
        assert_eq!(read(socket), (b'2', vec![]));
        send(socket, b'D', &[b"P", portal.as_bytes(), b"\0"].concat());
        assert_eq!(read(socket), (b'T', description(formats)));
    }
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
    let text = r#" { "b":1, "a":2,"b":3, "x":"\u4e2d" } "#;
    for (statement, prefix, suffix, row_count) in [
        ("values", "", "", 1),
        ("empty", "", " WHERE false", 0),
        (
            "first_null",
            "SELECT NULL::jsonb AS v, NULL::jsonb AS n UNION ALL ",
            "",
            2,
        ),
    ] {
        parse(
            &mut socket,
            statement,
            &format!("{prefix}SELECT $1::jsonb AS v, NULL::jsonb AS n{suffix}"),
            if row_count == 2 { &[3802] } else { &[] },
        );
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &3802u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), (b'T', description(&[0, 0])));
        // Two live portals retain independent per-column result formats.
        for (portal, formats) in [("a", [0, 1]), ("b", [1, 0])] {
            bind_output(&mut socket, portal, statement, Some(text), &formats);
        }
        for portal in ["b", "a"] {
            for execution in 0..2 {
                execute(&mut socket, portal, 0);
                if row_count > 0 && execution == 0 {
                    if row_count == 2 {
                        assert_eq!(read(&mut socket), (b'D', row(&[None, None])));
                    }
                    let expected = output_row(Some(text), if portal == "b" { 1 } else { 0 });
                    assert_eq!(read(&mut socket), (b'D', expected));
                }
                assert_eq!(
                    read(&mut socket),
                    (b'C', format!("SELECT {row_count}\0").into_bytes())
                );
            }
        }
        // Unnamed portal rebinding changes formats and value/NULL, never identity.
        for (formats, value) in [
            ([1, 1], None),
            ([0, 0], Some(text)),
            ([1, 0], None),
            ([1, 1], Some("null")),
            ([0, 1], Some(r#""null""#)),
        ] {
            bind_output(&mut socket, "", statement, value, &formats);
            execute(&mut socket, "", 0);
            if row_count > 0 {
                if row_count == 2 {
                    assert_eq!(read(&mut socket), (b'D', row(&[None, None])));
                }
                assert_eq!(read(&mut socket), (b'D', output_row(value, formats[0])));
            }
            assert_eq!(read(&mut socket).0, b'C');
        }
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket), (b'Z', b"I".to_vec()));
        for portal in ["a", "b"] {
            send(
                &mut socket,
                b'C',
                &[b"P", portal.as_bytes(), b"\0"].concat(),
            );
            assert_eq!(read(&mut socket), (b'3', vec![]));
        }
        send(
            &mut socket,
            b'C',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(read(&mut socket), (b'3', vec![]));
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_int4_array_required_parser_context() {
    let mut parser =
        astersql_parser::Parser::with_dialect(astersql_parser_ast::SqlDialect::PostgreSQL);
    for sql in [
        "SELECT $1::integer[] AS v",
        "SELECT NULL::integer[] AS v",
        "SELECT ARRAY[]::integer[] AS v",
        "SELECT CAST($1 AS integer[]) AS v",
    ] {
        assert!(
            parser.ParseOneStmt(sql, "", "").is_ok(),
            "missing required parser context: {sql}"
        );
    }
}

#[test]
fn pg05_int4_array_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1007u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1007]),
        ("inferred", "SELECT $1::integer[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS int4[]) AS v", vec![]),
        ("empty", "SELECT $1::integer[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::integer[] AS v UNION ALL SELECT $1::integer[] AS v",
            vec![1007],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1007u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (Some("{1,2}"), Some(words(&[1, 0, 23, 2, 1, 4, 1, 4, 2]))),
            (
                Some("{{1,2},{3,4}}"),
                Some(words(&[2, 0, 23, 2, 1, 2, 1, 4, 1, 4, 2, 4, 3, 4, 4])),
            ),
            (
                Some("[0:1][-2:-1]={{1,NULL},{-3,2147483647}}"),
                Some(words(&[
                    2, 1, 23, 2, 0, 2, -2, 4, 1, -1, 4, -3, 4, 2147483647,
                ])),
            ),
            (Some("{}"), Some(words(&[0, 0, 23]))),
            (Some("{NULL}"), Some(words(&[1, 1, 23, 1, 1, -1]))),
            (Some("{NULL,NULL}"), Some(words(&[1, 1, 23, 2, 1, -1, -1]))),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        ("SELECT ARRAY[[1,2],[3,4]] AS v", Some("{{1,2},{3,4}}")),
        ("SELECT ARRAY[NULL,1] AS v", Some("{NULL,1}")),
        ("SELECT ARRAY[]::integer[] AS v", Some("{}")),
        (
            "SELECT ARRAY[NULL,NULL]::integer[] AS v",
            Some("{NULL,NULL}"),
        ),
        ("SELECT NULL::integer[] AS v", None),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{x}".to_vec(), "22P02"),
        (0, b"{2147483648}".to_vec(), "22003"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 25]), "42804"),
        (1, words(&[-1, 0, 23]), "22P03"),
        (1, words(&[0, 2, 23]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 23, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 23, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{7,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{7,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_cast_wire_four_formats_metadata_and_recovery() {
    // Independent expected wire transformation of these fixed int4 fixtures.
    fn widen(bytes: &[u8]) -> Vec<u8> {
        let words = bytes
            .chunks_exact(4)
            .map(|b| i32::from_be_bytes(b.try_into().unwrap()))
            .collect::<Vec<_>>();
        let header = 3 + 2 * words[0] as usize;
        let mut output = words[..header]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect::<Vec<_>>();
        output[8..12].copy_from_slice(&20u32.to_be_bytes());
        let mut pos = header;
        while pos < words.len() {
            if words[pos] == -1 {
                output.extend_from_slice(&(-1i32).to_be_bytes());
                pos += 1;
            } else {
                assert_eq!(words[pos], 4);
                output.extend_from_slice(&8i32.to_be_bytes());
                output.extend_from_slice(&(words[pos + 1] as i64).to_be_bytes());
                pos += 2;
            }
        }
        output
    }
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1016u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1::int8[] AS v", vec![1007]),
        ("inferred", "SELECT $1::integer[]::int8[] AS v", vec![0]),
        (
            "cast",
            "SELECT CAST(CAST($1 AS int4[]) AS int8[]) AS v",
            vec![],
        ),
        (
            "empty",
            "SELECT $1::integer[]::int8[] AS v WHERE false",
            vec![],
        ),
        (
            "first_null",
            "SELECT NULL::int8[] AS v UNION ALL SELECT $1::integer[]::int8[] AS v",
            vec![1007],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1007u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (Some("{1,2}"), Some(words(&[1, 0, 23, 2, 1, 4, 1, 4, 2]))),
            (
                Some("{{1,2},{3,4}}"),
                Some(words(&[2, 0, 23, 2, 1, 2, 1, 4, 1, 4, 2, 4, 3, 4, 4])),
            ),
            (
                Some("[0:1][-2:-1]={{1,NULL},{-3,2147483647}}"),
                Some(words(&[
                    2, 1, 23, 2, 0, 2, -2, 4, 1, -1, 4, -3, 4, 2147483647,
                ])),
            ),
            (Some("{}"), Some(words(&[0, 0, 23]))),
            (Some("{NULL}"), Some(words(&[1, 1, 23, 1, 1, -1]))),
            (Some("{NULL,NULL}"), Some(words(&[1, 1, 23, 2, 1, -1, -1]))),
            (None, None),
        ];
        for (text, binary) in cases {
            let widened = binary.as_ref().map(|b| widen(b));
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            widened.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[[1,2],[3,4]]::int8[] AS v",
            Some("{{1,2},{3,4}}"),
        ),
        ("SELECT ARRAY[NULL,1]::int8[] AS v", Some("{NULL,1}")),
        ("SELECT ARRAY[]::int8[] AS v", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::int8[] AS v", Some("{NULL,NULL}")),
        ("SELECT NULL::int8[] AS v", None),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{x}".to_vec(), "22P02"),
        (0, b"{2147483648}".to_vec(), "22003"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 25]), "42804"),
        (1, words(&[-1, 0, 23]), "22P03"),
        (1, words(&[0, 2, 23]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 23, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 23, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{7,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{7,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(
        &mut socket,
        "narrow",
        "SELECT $1::text::int8[]::int4[] AS v",
        &[25],
    );
    assert_eq!(read(&mut socket).0, b'1');
    for payload in [b"{1,2147483648}".as_slice(), b"{1,-2147483649}"] {
        bind_value(&mut socket, "narrow", 0, 1, Some(payload));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_state(&read(&mut socket), "22003");
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{7,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{7,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    // Task28 opens this former gate; preserve the original boundary check
    // as an ordinary int8 Bind/portal/result assertion.
    parse(
        &mut socket,
        "int8_parameter_opened",
        "SELECT $1 AS v",
        &[1016],
    );
    assert_eq!(read(&mut socket), (b'1', vec![]));
    bind_value(
        &mut socket,
        "int8_parameter_opened",
        0,
        1,
        Some(b"{7,NULL}"),
    );
    assert_eq!(read(&mut socket), (b'2', vec![]));
    send(&mut socket, b'D', b"P\0");
    assert_eq!(read(&mut socket), description(1));
    execute(&mut socket, "", 0);
    let binary = [
        words(&[1, 1, 20, 2, 1, 8]),
        7i64.to_be_bytes().to_vec(),
        words(&[-1]),
    ]
    .concat();
    assert_eq!(read(&mut socket), data(Some(&binary)));
    assert_eq!(read(&mut socket).0, b'C');
    sync(&mut socket);
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_text_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1009u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1009]),
        ("inferred", "SELECT $1::text[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS text[]) AS v", vec![]),
        ("empty", "SELECT $1::text[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::text[] AS v UNION ALL SELECT $1::text[] AS v",
            vec![1009],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1009u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{a,b}"),
                Some(
                    [
                        words(&[1, 0, 25, 2, 1, 1]),
                        b"a".to_vec(),
                        words(&[1]),
                        b"b".to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (
                Some("[0:1][-2:-1]={{a,NULL},{b,c}}"),
                Some(
                    [
                        words(&[2, 1, 25, 2, 0, 2, -2, 1]),
                        b"a".to_vec(),
                        words(&[-1, 1]),
                        b"b".to_vec(),
                        words(&[1]),
                        b"c".to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (Some("{}"), Some(words(&[0, 0, 25]))),
            (Some("{NULL}"), Some(words(&[1, 1, 25, 1, 1, -1]))),
            (Some("{NULL,NULL}"), Some(words(&[1, 1, 25, 2, 1, -1, -1]))),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[['a','b'],['c','d']] AS v",
            Some("{{a,b},{c,d}}"),
        ),
        (
            "SELECT ARRAY[NULL,'NULL',''] AS v",
            Some(r#"{NULL,"NULL",""}"#),
        ),
        ("SELECT ARRAY[]::text[] AS v", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::text[] AS v", Some("{NULL,NULL}")),
        ("SELECT NULL::text[] AS v", None),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 25]), "22P03"),
        (1, words(&[0, 2, 25]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 25, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 25, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{7,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{7,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_bool_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1000u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1000]),
        ("inferred", "SELECT $1::boolean[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS boolean[]) AS v", vec![]),
        ("empty", "SELECT $1::boolean[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::boolean[] AS v UNION ALL SELECT $1::boolean[] AS v",
            vec![1000],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1000u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{t,f}"),
                Some([words(&[1, 0, 16, 2, 1, 1]), vec![1], words(&[1]), vec![0]].concat()),
            ),
            (
                Some("[0:1][-2:-1]={{t,NULL},{f,t}}"),
                Some(
                    [
                        words(&[2, 1, 16, 2, 0, 2, -2, 1]),
                        vec![1],
                        words(&[-1, 1]),
                        vec![0],
                        words(&[1]),
                        vec![1],
                    ]
                    .concat(),
                ),
            ),
            (Some("{}"), Some(words(&[0, 0, 16]))),
            (Some("{NULL}"), Some(words(&[1, 1, 16, 1, 1, -1]))),
            (Some("{NULL,NULL}"), Some(words(&[1, 1, 16, 2, 1, -1, -1]))),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[[true,false],[false,true]] AS v",
            Some("{{t,f},{f,t}}"),
        ),
        ("SELECT ARRAY[]::boolean[] AS v", Some("{}")),
        (
            "SELECT ARRAY[NULL,NULL]::boolean[] AS v",
            Some("{NULL,NULL}"),
        ),
        ("SELECT NULL::boolean[] AS v", None),
        (
            "SELECT '{yes,off,TRUE,0}'::boolean[] AS v",
            Some("{t,f,t,f}"),
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 16]), "22P03"),
        (1, words(&[0, 2, 16]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 16, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 16, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{t,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{t,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_bool_scalar_binary_receive() {
    for byte in 0..=255u8 {
        let value = crate::pg_extended::binary_parameter(16, Some(&[byte])).unwrap();
        assert!(!value.is_null);
        assert_eq!(value.tp, 1);
        assert_eq!(value.value, vec![u8::from(byte != 0)]);
    }
    let null = crate::pg_extended::binary_parameter(16, None).unwrap();
    assert!(null.is_null);
    assert_eq!(null.tp, 1);
    for (payload, state) in [
        (vec![], "08P01"),
        (vec![0, 0], "22P03"),
        (vec![1, 255], "22P03"),
    ] {
        assert_eq!(
            crate::pg_extended::binary_parameter(16, Some(&payload))
                .unwrap_err()
                .0,
            state
        );
    }
}

#[test]
fn pg05_array_int2_scalar_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for (text, number) in [("-32768", i16::MIN), ("32767", i16::MAX)] {
        let expected = number.to_le_bytes().to_vec();
        assert_eq!(
            parameter(21, Some(text.as_bytes())).unwrap().value,
            expected
        );
        assert_eq!(
            binary_parameter(21, Some(&number.to_be_bytes()))
                .unwrap()
                .value,
            expected
        );
    }
    assert!(binary_parameter(21, None).unwrap().is_null);
    for text in ["-32769", "32768"] {
        assert_eq!(parameter(21, Some(text.as_bytes())).unwrap_err().0, "22003");
    }
    assert_eq!(
        binary_parameter(21, Some(&[0, 1, 0])).unwrap_err().0,
        "22P03"
    );
    for payload in [b"".as_slice(), b"\0".as_slice()] {
        let error = binary_parameter(21, Some(payload)).unwrap_err();
        assert_eq!(error.0, "08P01", "int2 short payload {payload:?}");
    }
}
#[test]
fn pg05_array_int2_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1005u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1005]),
        ("inferred", "SELECT $1::int2[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS int2[]) AS v", vec![]),
        ("empty", "SELECT $1::int2[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::int2[] AS v UNION ALL SELECT $1::int2[] AS v",
            vec![1005],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1005u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{-32768,32767}"),
                Some(
                    [
                        words(&[1, 0, 21, 2, 1, 2]),
                        (-32768i16).to_be_bytes().to_vec(),
                        words(&[2]),
                        32767i16.to_be_bytes().to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (
                Some("[0:1][-2:-1]={{-32768,NULL},{0,32767}}"),
                Some(
                    [
                        words(&[2, 1, 21, 2, 0, 2, -2, 2]),
                        (-32768i16).to_be_bytes().to_vec(),
                        words(&[-1, 2]),
                        0i16.to_be_bytes().to_vec(),
                        words(&[2]),
                        32767i16.to_be_bytes().to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (Some("{}"), Some(words(&[0, 0, 21]))),
            (Some("{NULL}"), Some(words(&[1, 1, 21, 1, 1, -1]))),
            (Some("{NULL,NULL}"), Some(words(&[1, 1, 21, 2, 1, -1, -1]))),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[[-32768,32767],[0,1]]::int2[] AS v",
            Some("{{-32768,32767},{0,1}}"),
        ),
        ("SELECT ARRAY[]::int2[] AS v", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::int2[] AS v", Some("{NULL,NULL}")),
        ("SELECT NULL::int2[] AS v", None),
        (
            "SELECT '{-32768,32767,0,1}'::int2[] AS v",
            Some("{-32768,32767,0,1}"),
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 16]), "22P03"),
        (1, words(&[0, 2, 16]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 21, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 21, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{1,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{1,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

// Scalar contract reused by int8 array Bind decoding.
#[test]
fn pg05_array_int8_scalar_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for number in [
        i64::MIN,
        -9007199254740993,
        -1,
        0,
        1,
        9007199254740993,
        i64::MAX,
    ] {
        let text = number.to_string();
        let expected = number.to_le_bytes().to_vec();
        assert_eq!(
            parameter(20, Some(text.as_bytes())).unwrap().value,
            expected
        );
        assert_eq!(
            binary_parameter(20, Some(&number.to_be_bytes()))
                .unwrap()
                .value,
            expected
        );
        println!("int8 exact text/binary pass: {number}");
    }
    assert!(binary_parameter(20, None).unwrap().is_null);
    for text in ["-9223372036854775809", "9223372036854775808"] {
        assert_eq!(parameter(20, Some(text.as_bytes())).unwrap_err().0, "22003");
    }
    assert_eq!(binary_parameter(20, Some(&[0; 9])).unwrap_err().0, "22P03");
    let observed: Vec<_> = (0..8)
        .map(|n| {
            let state = binary_parameter(20, Some(&vec![0; n])).unwrap_err().0;
            println!("int8 binary length={n}: actual={state}, reference-required=08P01");
            state
        })
        .collect();
    assert_eq!(observed, vec!["08P01"; 8]);
}

#[test]
fn pg05_array_int8_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1016u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    struct CloseOnDrop<'a>(&'a PgService);
    impl Drop for CloseOnDrop<'_> {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = CloseOnDrop(&service);
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1016]),
        ("inferred", "SELECT $1::int8[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS int8[]) AS v", vec![]),
        ("empty", "SELECT $1::int8[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::int8[] AS v UNION ALL SELECT $1::int8[] AS v",
            vec![1016],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1016u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{-9223372036854775808,9223372036854775807}"),
                Some(
                    [
                        words(&[1, 0, 20, 2, 1, 8]),
                        (-9223372036854775808i64).to_be_bytes().to_vec(),
                        words(&[8]),
                        9223372036854775807i64.to_be_bytes().to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (
                Some("[0:1][-2:-1]={{-9223372036854775808,NULL},{0,9223372036854775807}}"),
                Some(
                    [
                        words(&[2, 1, 20, 2, 0, 2, -2, 8]),
                        (-9223372036854775808i64).to_be_bytes().to_vec(),
                        words(&[-1, 8]),
                        0i64.to_be_bytes().to_vec(),
                        words(&[8]),
                        9223372036854775807i64.to_be_bytes().to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (Some("{}"), Some(words(&[0, 0, 20]))),
            (Some("{NULL}"), Some(words(&[1, 1, 20, 1, 1, -1]))),
            (Some("{NULL,NULL}"), Some(words(&[1, 1, 20, 2, 1, -1, -1]))),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[[-9223372036854775808,9223372036854775807],[0,1]]::int8[] AS v",
            Some("{{-9223372036854775808,9223372036854775807},{0,1}}"),
        ),
        ("SELECT ARRAY[]::int8[] AS v", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::int8[] AS v", Some("{NULL,NULL}")),
        ("SELECT NULL::int8[] AS v", None),
        (
            "SELECT '{-9223372036854775808,9223372036854775807,0,1}'::int8[] AS v",
            Some("{-9223372036854775808,9223372036854775807,0,1}"),
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 16]), "22P03"),
        (1, words(&[0, 2, 16]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 20, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 20, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{1,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{1,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_bytea_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1001u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    struct CloseOnDrop<'a>(&'a PgService);
    impl Drop for CloseOnDrop<'_> {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = CloseOnDrop(&service);
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let startup = [196610u32.to_be_bytes().as_slice(), b"user\0root\0\0"].concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1001]),
        ("inferred", "SELECT $1::bytea[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS bytea[]) AS v", vec![]),
        ("empty", "SELECT $1::bytea[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::bytea[] AS v UNION ALL SELECT $1::bytea[] AS v",
            vec![1001],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1001u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some(r#"{"\\x00ff","\\x",NULL}"#),
                Some([words(&[1, 1, 17, 3, 1, 2]), vec![0, 255], words(&[0, -1])].concat()),
            ),
            (
                Some(r#"[0:1][-2:-1]={{"\\x00ff",NULL},{"\\x","\\xc328"}}"#),
                Some(
                    [
                        words(&[2, 1, 17, 2, 0, 2, -2, 2]),
                        vec![0, 255],
                        words(&[-1, 0, 2]),
                        vec![195, 40],
                    ]
                    .concat(),
                ),
            ),
            (Some("{}"), Some(words(&[0, 0, 17]))),
            (Some("{NULL}"), Some(words(&[1, 1, 17, 1, 1, -1]))),
            (Some("{NULL,NULL}"), Some(words(&[1, 1, 17, 2, 1, -1, -1]))),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        ("SELECT ARRAY[]::bytea[] AS v", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::bytea[] AS v", Some("{NULL,NULL}")),
        ("SELECT NULL::bytea[] AS v", None),
        (
            "SELECT '{abc,NULL}'::bytea[] AS v",
            Some(r#"{"\\x616263",NULL}"#),
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 16]), "22P03"),
        (1, words(&[0, 2, 16]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 17, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 17, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(br#"{"\\x31",NULL}"#));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(br#"{"\\x31",NULL}"#)));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_bytea_scalar_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for bytes in [
        vec![],
        vec![0, 255, 195, 40, 92],
        (0..=255).collect::<Vec<u8>>(),
    ] {
        let text = format!(
            "\\x{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        assert_eq!(parameter(17, Some(text.as_bytes())).unwrap().value, bytes);
        assert_eq!(binary_parameter(17, Some(&bytes)).unwrap().value, bytes);
    }
    assert_eq!(
        parameter(17, Some(br"\000\377\\")).unwrap().value,
        [0, 255, 92]
    );
    assert!(parameter(17, None).unwrap().is_null);
    assert!(binary_parameter(17, None).unwrap().is_null);
    for (input, state) in [
        (br"\x0".as_slice(), "22023"),
        (br"\xgg", "22023"),
        (br"\400", "22P02"),
        (br"\q", "22P02"),
        (b"a\0b", "22021"),
        (b"\xff", "22021"),
    ] {
        assert_eq!(parameter(17, Some(input)).unwrap_err().0, state);
    }
}

#[test]
fn pg05_array_numeric_scalar_binary_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for (text, hex) in [
        ("12.340", "0002000000000003000c0d48"),
        ("NaN", "00000000c0000000"),
        ("Infinity", "00000000d0000000"),
        ("-Infinity", "00000000f0000000"),
    ] {
        let bytes: Vec<u8> = hex
            .as_bytes()
            .chunks(2)
            .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(
            parameter(1700, Some(text.as_bytes())).unwrap().value,
            text.as_bytes()
        );
        assert_eq!(
            binary_parameter(1700, Some(&bytes)).unwrap().value,
            text.as_bytes()
        );
    }
}

#[test]
fn pg05_array_float4_scalar_length_prerequisite() {
    for payload in [&b""[..], &b"\0"[..], &b"\0\0\0"[..]] {
        assert_eq!(
            crate::pg_extended::binary_parameter(700, Some(payload))
                .unwrap_err()
                .0,
            "08P01"
        );
    }
}

#[test]
fn pg05_array_float4_scalar_length_and_null_edges() {
    use crate::pg_extended::{binary_parameter, parameter};
    for bytes in [vec![0; 5], vec![0; 8]] {
        assert_eq!(binary_parameter(700, Some(&bytes)).unwrap_err().0, "22P03");
    }
    assert!(binary_parameter(700, None).unwrap().is_null);
    assert!(parameter(700, None).unwrap().is_null);
    assert_eq!(
        u32::from_le_bytes(
            parameter(700, Some(b"-0"))
                .unwrap()
                .value
                .try_into()
                .unwrap()
        ),
        0x80000000
    );
}

#[test]
fn pg05_array_float4_scalar_nan_bits_prerequisite() {
    use crate::pg_client_integration_test::start_client_server;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
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
    parse(&mut socket, "bits", "SELECT $1 AS v", &[700]);
    assert_eq!(read(&mut socket).0, b'1');
    for bits in [0x7fc00000u32, 0x7fc12345, 0xffc12345, 0x7fa12345] {
        let body = [
            b"\0bits\0".as_slice(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &4i32.to_be_bytes(),
            &bits.to_be_bytes(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        send(&mut socket, b'B', &body);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        let actual = read(&mut socket);
        println!("float4 binary input={bits:08x} output={actual:?}");
        assert_eq!(
            actual,
            (
                b'D',
                [
                    1i16.to_be_bytes().as_slice(),
                    &4i32.to_be_bytes(),
                    &bits.to_be_bytes()
                ]
                .concat()
            )
        );
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
}

#[test]
fn pg05_array_float8_scalar_length_prerequisite() {
    for payload in [&b""[..], &b"\0"[..], &b"\0\0\0"[..]] {
        assert_eq!(
            crate::pg_extended::binary_parameter(701, Some(payload))
                .unwrap_err()
                .0,
            "08P01"
        );
    }
}

#[test]
fn pg05_array_float8_scalar_length_and_null_edges() {
    use crate::pg_extended::{binary_parameter, parameter};
    for bytes in [vec![0; 9], vec![0; 16]] {
        assert_eq!(binary_parameter(701, Some(&bytes)).unwrap_err().0, "22P03");
    }
    assert!(binary_parameter(701, None).unwrap().is_null);
    assert!(parameter(701, None).unwrap().is_null);
    assert_eq!(
        u64::from_le_bytes(
            parameter(701, Some(b"-0"))
                .unwrap()
                .value
                .try_into()
                .unwrap()
        ),
        0x8000000000000000
    );
}

#[test]
fn pg05_array_float8_scalar_nan_bits_prerequisite() {
    use crate::pg_client_integration_test::start_client_server;
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let server = start_client_server(domain, Some(0));
    struct Cleanup(Arc<crate::server::Server>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let cleanup = Cleanup(server);
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
    parse(&mut socket, "bits", "SELECT $1 AS v", &[701]);
    assert_eq!(read(&mut socket).0, b'1');
    for bits in [
        0x7ff8000000000000u64,
        0x7ff812345678abcd,
        0xfff812345678abcd,
        0x7ff012345678abcd,
    ] {
        let body = [
            b"\0bits\0".as_slice(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
            &8i32.to_be_bytes(),
            &bits.to_be_bytes(),
            &1i16.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        send(&mut socket, b'B', &body);
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        let actual = read(&mut socket);
        println!("float8 binary input={bits:016x} output={actual:?}");
        assert_eq!(
            actual,
            (
                b'D',
                [
                    1i16.to_be_bytes().as_slice(),
                    &8i32.to_be_bytes(),
                    &bits.to_be_bytes()
                ]
                .concat()
            )
        );
        assert_eq!(read(&mut socket).0, b'C');
        send(&mut socket, b'S', &[]);
        assert_eq!(read(&mut socket).0, b'Z');
    }
}

#[test]
fn pg05_array_varchar_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1015u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1015]),
        ("inferred", "SELECT $1::varchar[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS varchar[]) AS v", vec![]),
        ("empty", "SELECT $1::varchar[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::varchar[] AS v UNION ALL SELECT $1::varchar[] AS v",
            vec![1015],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1015u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{a,b}"),
                Some(
                    [
                        words(&[1, 0, 1043, 2, 1, 1]),
                        b"a".to_vec(),
                        words(&[1]),
                        b"b".to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (
                Some("[0:1][-2:-1]={{a,NULL},{b,c}}"),
                Some(
                    [
                        words(&[2, 1, 1043, 2, 0, 2, -2, 1]),
                        b"a".to_vec(),
                        words(&[-1, 1]),
                        b"b".to_vec(),
                        words(&[1]),
                        b"c".to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (Some("{}"), Some(words(&[0, 0, 1043]))),
            (Some("{NULL}"), Some(words(&[1, 1, 1043, 1, 1, -1]))),
            (
                Some("{NULL,NULL}"),
                Some(words(&[1, 1, 1043, 2, 1, -1, -1])),
            ),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[['a','b'],['c','d']]::varchar[] AS v",
            Some("{{a,b},{c,d}}"),
        ),
        (
            "SELECT ARRAY[NULL,'NULL','']::varchar[] AS v",
            Some(r#"{NULL,"NULL",""}"#),
        ),
        ("SELECT ARRAY[]::varchar[] AS v", Some("{}")),
        (
            "SELECT ARRAY[NULL,NULL]::varchar[] AS v",
            Some("{NULL,NULL}"),
        ),
        ("SELECT NULL::varchar[] AS v", None),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 1043]), "22P03"),
        (1, words(&[0, 2, 1043]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 1043, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 1043, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{7,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{7,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_varchar_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for decode in [parameter, binary_parameter] {
        for input in ["", "é😀", "a  "] {
            assert_eq!(
                decode(1043, Some(input.as_bytes())).unwrap().value,
                input.as_bytes()
            );
        }
        assert!(decode(1043, None).unwrap().is_null);
        for input in [b"a\0b".as_slice(), b"\xff", b"\xc3"] {
            assert_eq!(decode(1043, Some(input)).unwrap_err().0, "22021");
        }
    }
}

#[test]
fn pg05_array_bpchar_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1014u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1014]),
        ("inferred", "SELECT $1::bpchar[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS bpchar[]) AS v", vec![]),
        ("empty", "SELECT $1::bpchar[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::bpchar[] AS v UNION ALL SELECT $1::bpchar[] AS v",
            vec![1014],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1014u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{a,b}"),
                Some(
                    [
                        words(&[1, 0, 1042, 2, 1, 1]),
                        b"a".to_vec(),
                        words(&[1]),
                        b"b".to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (
                Some("[0:1][-2:-1]={{a,NULL},{b,c}}"),
                Some(
                    [
                        words(&[2, 1, 1042, 2, 0, 2, -2, 1]),
                        b"a".to_vec(),
                        words(&[-1, 1]),
                        b"b".to_vec(),
                        words(&[1]),
                        b"c".to_vec(),
                    ]
                    .concat(),
                ),
            ),
            (Some("{}"), Some(words(&[0, 0, 1042]))),
            (Some("{NULL}"), Some(words(&[1, 1, 1042, 1, 1, -1]))),
            (
                Some("{NULL,NULL}"),
                Some(words(&[1, 1, 1042, 2, 1, -1, -1])),
            ),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[['a','b'],['c','d']]::bpchar[] AS v",
            Some("{{a,b},{c,d}}"),
        ),
        (
            "SELECT ARRAY[NULL,'NULL','']::bpchar[] AS v",
            Some(r#"{NULL,"NULL",""}"#),
        ),
        ("SELECT ARRAY[]::bpchar[] AS v", Some("{}")),
        (
            "SELECT ARRAY[NULL,NULL]::bpchar[] AS v",
            Some("{NULL,NULL}"),
        ),
        ("SELECT NULL::bpchar[] AS v", None),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 1042]), "22P03"),
        (1, words(&[0, 2, 1042]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 1042, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 1042, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{7,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{7,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_bpchar_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for decode in [parameter, binary_parameter] {
        for input in ["", "é😀", "a  "] {
            assert_eq!(
                decode(1042, Some(input.as_bytes())).unwrap().value,
                input.as_bytes()
            );
        }
        assert!(decode(1042, None).unwrap().is_null);
        for input in [b"a\0b".as_slice(), b"\xff", b"\xc3"] {
            assert_eq!(decode(1042, Some(input)).unwrap_err().0, "22021");
        }
    }
}

#[test]
fn pg05_array_date_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1182u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1182]),
        ("inferred", "SELECT $1::date[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS date[]) AS v", vec![]),
        ("empty", "SELECT $1::date[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::date[] AS v UNION ALL SELECT $1::date[] AS v",
            vec![1182],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1182u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{2000-01-01,2000-01-02}"),
                Some(words(&[1, 0, 1082, 2, 1, 4, 0, 4, 1])),
            ),
            (
                Some("[0:1][-2:-1]={{2000-01-01,NULL},{infinity,-infinity}}"),
                Some(words(&[
                    2,
                    1,
                    1082,
                    2,
                    0,
                    2,
                    -2,
                    4,
                    0,
                    -1,
                    4,
                    i32::MAX,
                    4,
                    i32::MIN,
                ])),
            ),
            (Some("{}"), Some(words(&[0, 0, 1082]))),
            (Some("{NULL}"), Some(words(&[1, 1, 1082, 1, 1, -1]))),
            (
                Some("{NULL,NULL}"),
                Some(words(&[1, 1, 1082, 2, 1, -1, -1])),
            ),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[['2000-01-01','2000-01-02'],['infinity','-infinity']]::date[] AS v",
            Some("{{2000-01-01,2000-01-02},{infinity,-infinity}}"),
        ),
        (
            "SELECT ARRAY[NULL,'2000-01-01']::date[] AS v",
            Some("{NULL,2000-01-01}"),
        ),
        ("SELECT ARRAY[]::date[] AS v", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::date[] AS v", Some("{NULL,NULL}")),
        ("SELECT NULL::date[] AS v", None),
        (
            "SELECT ARRAY['2000-01-01'::date,NULL::date] AS v",
            Some("{2000-01-01,NULL}"),
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 1082]), "22P03"),
        (1, words(&[0, 2, 1082]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 1082, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 1082, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{2000-01-01,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{2000-01-01,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_date_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for (text, days) in [
        ("4714-11-24 BC", -2451545i32),
        ("0001-12-31 BC", -730120),
        ("2000-01-01", 0),
        ("5874897-12-31", 2145031948),
        ("infinity", i32::MAX),
        ("-infinity", i32::MIN),
    ] {
        assert_eq!(
            parameter(1082, Some(text.as_bytes())).unwrap().value,
            text.as_bytes()
        );
        assert_eq!(
            binary_parameter(1082, Some(&days.to_be_bytes()))
                .unwrap()
                .value,
            text.as_bytes()
        );
    }
}

#[test]
fn pg05_array_time_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for (text, micros) in [
        ("00:00:00", 0i64),
        ("24:00:00", 86400000000),
        ("12:34:56.123456", 45296123456),
    ] {
        assert_eq!(
            parameter(1083, Some(text.as_bytes())).unwrap().value,
            text.as_bytes()
        );
        assert_eq!(
            binary_parameter(1083, Some(&micros.to_be_bytes()))
                .unwrap()
                .value,
            text.as_bytes()
        );
    }
}
#[test]
fn pg05_array_time_registered_input() {
    use crate::pg_extended::{binary_parameter, parameter};
    let value = parameter(1183, Some(b"[0:1]={24:00:00,NULL}")).unwrap();
    assert_eq!(value.value, b"[0:1]={24:00:00,NULL}");
    let binary = [1i32, 1, 1083, 2, 0, 8]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .chain(86400000000i64.to_be_bytes())
        .chain((-1i32).to_be_bytes())
        .collect::<Vec<_>>();
    assert_eq!(
        binary_parameter(1183, Some(&binary)).unwrap().value,
        value.value
    );
}

#[test]
fn pg05_array_time_wire_four_formats_metadata_and_recovery() {
    fn words(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    fn description(format: i16) -> (u8, Vec<u8>) {
        (
            b'T',
            [
                1i16.to_be_bytes().as_slice(),
                b"v\0",
                &[0; 6],
                &1183u32.to_be_bytes(),
                &(-1i16).to_be_bytes(),
                &(-1i32).to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        )
    }
    fn bind_value(
        socket: &mut TcpStream,
        statement: &str,
        input: i16,
        output: i16,
        value: Option<&[u8]>,
    ) {
        let mut body = [
            b"\0".as_slice(),
            statement.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &input.to_be_bytes(),
            &1i16.to_be_bytes(),
        ]
        .concat();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        body.extend_from_slice(&1i16.to_be_bytes());
        body.extend_from_slice(&output.to_be_bytes());
        send(socket, b'B', &body);
    }
    fn data(value: Option<&[u8]>) -> (u8, Vec<u8>) {
        let mut body = 1i16.to_be_bytes().to_vec();
        body.extend_from_slice(&value.map_or(-1, |v| v.len() as i32).to_be_bytes());
        if let Some(v) = value {
            body.extend_from_slice(v);
        }
        (b'D', body)
    }
    fn sync(socket: &mut TcpStream) {
        send(socket, b'S', &[]);
        assert_eq!(read(socket), (b'Z', b"I".to_vec()));
    }
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
    while read(&mut socket).0 != b'Z' {}

    for (statement, sql, oids) in [
        ("declared", "SELECT $1 AS v", vec![1183]),
        ("inferred", "SELECT $1::time[] AS v", vec![0]),
        ("cast", "SELECT CAST($1 AS time[]) AS v", vec![]),
        ("empty", "SELECT $1::time[] AS v WHERE false", vec![]),
        (
            "first_null",
            "SELECT NULL::time[] AS v UNION ALL SELECT $1::time[] AS v",
            vec![1183],
        ),
    ] {
        parse(&mut socket, statement, sql, &oids);
        assert_eq!(read(&mut socket), (b'1', vec![]));
        send(
            &mut socket,
            b'D',
            &[b"S", statement.as_bytes(), b"\0"].concat(),
        );
        assert_eq!(
            read(&mut socket),
            (
                b't',
                [1i16.to_be_bytes().as_slice(), &1183u32.to_be_bytes()].concat()
            )
        );
        assert_eq!(read(&mut socket), description(0));
        let cases = [
            (
                Some("{00:00:00,00:00:01}"),
                Some(words(&[1, 0, 1083, 2, 1, 8, 0, 0, 8, 0, 1000000])),
            ),
            (
                Some("[0:1][-2:-1]={{00:00:00,NULL},{00:00:03,00:00:02}}"),
                Some(words(&[
                    2, 1, 1083, 2, 0, 2, -2, 8, 0, 0, -1, 8, 0, 3000000, 8, 0, 2000000,
                ])),
            ),
            (Some("{}"), Some(words(&[0, 0, 1083]))),
            (Some("{NULL}"), Some(words(&[1, 1, 1083, 1, 1, -1]))),
            (
                Some("{NULL,NULL}"),
                Some(words(&[1, 1, 1083, 2, 1, -1, -1])),
            ),
            (None, None),
        ];
        for (text, binary) in cases {
            for input in [0, 1] {
                for output in [0, 1] {
                    let payload = if input == 0 {
                        text.map(str::as_bytes)
                    } else {
                        binary.as_deref()
                    };
                    bind_value(&mut socket, statement, input, output, payload);
                    assert_eq!(
                        read(&mut socket),
                        (b'2', vec![]),
                        "{statement} input={input} output={output} {text:?}"
                    );
                    send(&mut socket, b'D', b"P\0");
                    assert_eq!(read(&mut socket), description(output));
                    execute(&mut socket, "", 0);
                    if statement == "first_null" {
                        assert_eq!(read(&mut socket), data(None));
                    }
                    if statement != "empty" {
                        let expected = if output == 0 {
                            text.map(str::as_bytes)
                        } else {
                            binary.as_deref()
                        };
                        assert_eq!(
                            read(&mut socket),
                            data(expected),
                            "{statement} input={input} output={output} {text:?}"
                        );
                    }
                    assert_eq!(read(&mut socket).0, b'C');
                    sync(&mut socket);
                }
            }
        }
    }
    for (sql, text) in [
        (
            "SELECT ARRAY[['00:00:00','00:00:01'],['00:00:03','00:00:02']]::time[] AS v",
            Some("{{00:00:00,00:00:01},{00:00:03,00:00:02}}"),
        ),
        (
            "SELECT ARRAY[NULL,'00:00:00']::time[] AS v",
            Some("{NULL,00:00:00}"),
        ),
        ("SELECT ARRAY[]::time[] AS v", Some("{}")),
        ("SELECT ARRAY[NULL,NULL]::time[] AS v", Some("{NULL,NULL}")),
        ("SELECT NULL::time[] AS v", None),
        (
            "SELECT ARRAY['00:00:00'::time,NULL::time] AS v",
            Some("{00:00:00,NULL}"),
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0], description(0), "{sql}: {result:?}");
        assert_eq!(result[1], data(text.map(str::as_bytes)), "{sql}");
    }
    for (format, payload, state) in [
        (0, b"{{1,2},{3}}".to_vec(), "22P02"),
        (0, b"[0:2]={1,2}".to_vec(), "22P02"),
        (0, b"{1,}".to_vec(), "22P02"),
        (0, b"{\xff}".to_vec(), "22021"),
        (1, words(&[0, 0, 23]), "42804"),
        (1, words(&[-1, 0, 1083]), "22P03"),
        (1, words(&[0, 2, 1083]), "22P03"),
        (1, vec![0], "08P01"),
        (1, words(&[1, 0, 1083, 1, 1, -2]), "22P03"),
        (1, words(&[0, 0, 1083, 0]), "22P03"),
    ] {
        bind_value(&mut socket, "declared", format, 1, Some(&payload));
        assert_state(&read(&mut socket), state);
        execute(&mut socket, "", 0);
        sync(&mut socket);
        bind_value(&mut socket, "declared", 0, 0, Some(b"{00:00:00,NULL}"));
        assert_eq!(read(&mut socket).0, b'2');
        execute(&mut socket, "", 0);
        assert_eq!(read(&mut socket), data(Some(b"{00:00:00,NULL}")));
        assert_eq!(read(&mut socket).0, b'C');
        sync(&mut socket);
    }
    parse(&mut socket, "unknown", "SELECT $1 AS v", &[0]);
    assert_state(&read(&mut socket), "42P18");
    sync(&mut socket);
    parse(&mut socket, "unopened", "SELECT $1 AS v", &[3807]);
    assert_state(&read(&mut socket), "0A000");
    sync(&mut socket);
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg05_array_timestamp_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for (text, micros) in [
        ("2000-01-01 00:00:00", 0i64),
        ("1999-12-31 23:59:59.999999", -1),
        ("infinity", i64::MAX),
        ("-infinity", i64::MIN),
    ] {
        assert_eq!(
            parameter(1114, Some(text.as_bytes())).unwrap().value,
            text.as_bytes()
        );
        assert_eq!(
            binary_parameter(1114, Some(&micros.to_be_bytes()))
                .unwrap()
                .value,
            text.as_bytes()
        );
    }
}
#[test]
fn pg05_array_timestamp_registered_input() {
    use crate::pg_extended::{binary_parameter, parameter};
    let text = br#"[0:1]={"2000-01-01 00:00:00",NULL}"#;
    let value = parameter(1115, Some(text)).unwrap();
    assert_eq!(value.value, text);
    let binary = [1i32, 1, 1114, 2, 0, 8]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .chain(0i64.to_be_bytes())
        .chain((-1i32).to_be_bytes())
        .collect::<Vec<_>>();
    assert_eq!(
        binary_parameter(1115, Some(&binary)).unwrap().value,
        value.value
    );
}

#[test]
fn pg05_array_timestamptz_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for (text, micros) in [
        ("2000-01-01 00:00:00+00", 0i64),
        ("1999-12-31 23:59:59.999999+00", -1),
        ("infinity", i64::MAX),
        ("-infinity", i64::MIN),
    ] {
        assert_eq!(
            parameter(1184, Some(text.as_bytes())).unwrap().value,
            text.as_bytes()
        );
        assert_eq!(
            binary_parameter(1184, Some(&micros.to_be_bytes()))
                .unwrap()
                .value,
            text.as_bytes()
        );
    }
}
#[test]
fn pg05_array_timestamptz_registered_input() {
    use crate::pg_extended::{binary_parameter, parameter};
    let text = br#"[0:1]={"2000-01-01 00:00:00+00",NULL}"#;
    let value = parameter(1185, Some(text)).unwrap();
    assert_eq!(value.value, text);
    let binary = [1i32, 1, 1184, 2, 0, 8]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .chain(0i64.to_be_bytes())
        .chain((-1i32).to_be_bytes())
        .collect::<Vec<_>>();
    assert_eq!(
        binary_parameter(1185, Some(&binary)).unwrap().value,
        value.value
    );
}

#[test]
fn pg05_array_interval_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    use astersql_types::pg_temporal::PgInterval;
    for parts in [
        (1, 0, 0),
        (0, 30, 0),
        (0, 1, 0),
        (0, 0, 86_400_000_000),
        (-1, 2, -11_045_123_456),
        (i32::MAX, i32::MAX, i64::MAX),
        (i32::MIN, i32::MIN, i64::MIN),
    ] {
        let v = PgInterval::from_components(parts.0, parts.1, parts.2);
        let text = v.to_string();
        assert_eq!(
            parameter(1186, Some(text.as_bytes())).unwrap().value,
            text.as_bytes()
        );
        assert_eq!(
            binary_parameter(1186, Some(&v.to_binary())).unwrap().value,
            text.as_bytes()
        );
    }
}
#[test]
fn pg05_array_interval_registered_input() {
    use crate::pg_extended::{binary_parameter, parameter};
    let text = br#"[-1:0]={"1 mon",NULL}"#;
    let value = parameter(1187, Some(text)).unwrap();
    assert_eq!(value.value, text);
    let binary = [1i32, 1, 1186, 2, -1, 16]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .chain(0i64.to_be_bytes())
        .chain(0i32.to_be_bytes())
        .chain(1i32.to_be_bytes())
        .chain((-1i32).to_be_bytes())
        .collect::<Vec<_>>();
    assert_eq!(
        binary_parameter(1187, Some(&binary)).unwrap().value,
        value.value
    );
}

#[test]
fn pg05_array_uuid_scalar_codec_prerequisite() {
    use crate::pg_extended::{binary_parameter, parameter};
    for bytes in [[0; 16], [255; 16], std::array::from_fn(|i| i as u8)] {
        let text = astersql_types::pg_uuid::PgUuid::from_bytes(bytes).to_string();
        assert_eq!(
            parameter(2950, Some(text.as_bytes())).unwrap().value,
            text.as_bytes()
        );
        assert_eq!(
            binary_parameter(2950, Some(&bytes)).unwrap().value,
            text.as_bytes()
        );
    }
}
#[test]
fn pg05_array_uuid_registered_input() {
    use crate::pg_extended::{binary_parameter, parameter};
    let text = b"[-1:0]={00000000-0000-0000-0000-000000000000,NULL}";
    let value = parameter(2951, Some(text)).unwrap();
    assert_eq!(value.value, text);
    let binary = [1i32, 1, 2950, 2, -1, 16]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .chain([0u8; 16])
        .chain((-1i32).to_be_bytes())
        .collect::<Vec<_>>();
    assert_eq!(
        binary_parameter(2951, Some(&binary)).unwrap().value,
        value.value
    );
    assert_eq!(
        parameter(2951, Some(b"{00000000-0000-0000-0000-000000000000,bad}"))
            .unwrap_err()
            .0,
        "22P02"
    );
}

#[test]
fn pg05_array_json_registered_input() {
    use crate::pg_extended::{binary_parameter, parameter};
    let scalar = br#" { "k":1, "k":2, "n":1e100000 } "#;
    for codec in [parameter, binary_parameter] {
        assert_eq!(codec(114, Some(scalar)).unwrap().value, scalar);
        assert_eq!(codec(114, Some(b"null")).unwrap().value, b"null");
        assert!(codec(114, None).unwrap().is_null);
    }
    let text = br#"[-1:1]={" { \"k\":1, \"k\":2, \"n\":1e100000 } ","null",NULL}"#;
    assert_eq!(parameter(199, Some(text)).unwrap().value, text);
    let binary = [1i32, 1, 114, 3, -1, scalar.len() as i32]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .chain(scalar.iter().copied())
        .chain(4i32.to_be_bytes())
        .chain(*b"null")
        .chain((-1i32).to_be_bytes())
        .collect::<Vec<_>>();
    assert_eq!(binary_parameter(199, Some(&binary)).unwrap().value, text);
    assert_eq!(parameter(199, Some(b"{null,bad}")).unwrap_err().0, "22P02");
}
