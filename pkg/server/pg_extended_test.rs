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
    assert!(unsupported.1.windows(5).any(|w| w == b"0A000"));
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

    let empty = QueryResult {
        rows: vec![],
        ..result
    };
    assert_eq!(
        crate::pg_result::encode(&empty, "SELECT").unwrap()[0],
        encoded[0]
    );
}
