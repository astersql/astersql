// Copyright 2026 AsterSQL.
use crate::pg_catalog_query::{CastType, Expr, parse};

#[test]
fn catalog_select_structure() {
    let select = parse(crate::pg_catalog::DATABASES_SQL).unwrap().unwrap();
    assert_eq!(select.projections.len(), 6);
    assert_eq!(select.from.name, "pg_database");
    assert_eq!(select.from.alias, "n");
    assert_eq!(select.joins[0].relation.name, "pg_shdescription");
    assert!(matches!(select.order[0].expr, Expr::Case { .. }));
    let transactions = parse(crate::pg_catalog::TRANSACTIONS_SQL).unwrap().unwrap();
    assert!(matches!(
        transactions.projections[0].expr,
        Expr::Cast(_, CastType::Bigint)
    ));
    assert!(matches!(transactions.filter, Some(Expr::NotNull(_))));
    assert!(transactions.order[0].descending);
    assert_eq!(transactions.limit, Some(1));
    let namespace = parse("select N.oid::bigint as id, N.xmin as state_number, nspname as name, D.description, pg_catalog.pg_get_userbyid(N.nspowner) as \"owner\" from pg_catalog.pg_namespace N left join pg_catalog.pg_description D on N.oid = D.objoid order by case when nspname = pg_catalog.current_schema() then -1::bigint else N.oid::bigint end").unwrap().unwrap();
    assert_eq!(namespace.from.name, "pg_namespace");
    assert_eq!(namespace.projections.len(), 5);
    assert_eq!(
        parse("select oid, spcname from pg_catalog.pg_tablespace")
            .unwrap()
            .unwrap()
            .from
            .name,
        "pg_tablespace"
    );
}

#[test]
fn catalog_projection_syntax_and_boundaries() {
    let select = parse("/* outer /* nested */ */ SeLeCt datname AS \"Database Name\", NULL missing, 'pg_catalog.pg_database' literal, oid::varchar::bigint id FROM \"pg_catalog\".\"pg_database\" AS n ORDER BY oid DESC LIMIT 2; -- done").unwrap().unwrap();
    assert_eq!(select.projections[0].name, "Database Name");
    assert_eq!(select.projections[1].name, "missing");
    assert_eq!(select.limit, Some(2));
    assert_eq!(parse("select 'from pg_catalog.pg_database'").unwrap(), None);
    assert_eq!(
        parse("select 1 /* from pg_catalog.pg_database */").unwrap(),
        None
    );
    for sql in [
        "select oid from pg_catalog.pg_database; select 1",
        "select * from pg_catalog.pg_database",
        "select distinct oid from pg_catalog.pg_database",
        "select oid::numeric from pg_catalog.pg_database",
        "select oid + 1 from pg_catalog.pg_database",
        "select oid from pg_catalog.pg_database union select 1",
    ] {
        assert_eq!(parse(sql).unwrap_err().0, "0A000", "{sql}");
    }
    for sql in [
        "select from pg_catalog.pg_database",
        "select oid, from pg_catalog.pg_database",
        "select oid from pg_catalog.pg_database limit",
        "select oid from pg_catalog.pg_database /*",
        "select oid as \"unfinished from pg_catalog.pg_database",
    ] {
        assert_eq!(parse(sql).unwrap_err().0, "42601", "{sql}");
    }
}

#[test]
fn catalog_binding_has_explicit_support() {
    use crate::pg_catalog::CatalogQuery;
    for sql in [
        "select unknown from pg_catalog.pg_database",
        "select x.oid from pg_catalog.pg_database n",
        "select pg_catalog.unknown(oid) from pg_catalog.pg_database",
        "select oid from pg_catalog.pg_database where oid",
        "select datistemplate::bigint from pg_catalog.pg_database",
        "select current_database() from pg_catalog.pg_locks",
        "select pg_catalog.age(transactionid) from pg_catalog.pg_locks",
    ] {
        assert_eq!(CatalogQuery::parse(sql).unwrap_err().0, "0A000", "{sql}");
    }
    assert!(
        CatalogQuery::parse(
            "select datname name, oid id from pg_catalog.pg_database order by oid limit 2"
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn catalog_parser_bounds_expression_depth() {
    let sql = format!(
        "select {}oid{} from pg_catalog.pg_database",
        "(".repeat(100),
        ")".repeat(100)
    );
    assert_eq!(parse(&sql).unwrap_err().0, "0A000");
    let sql = format!(
        "select oid{} from pg_catalog.pg_database",
        "::bigint".repeat(100)
    );
    assert_eq!(parse(&sql).unwrap_err().0, "0A000");
}

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
fn columns(body: &[u8]) -> Vec<(String, u32)> {
    let count = i16::from_be_bytes(body[..2].try_into().unwrap());
    let mut offset = 2;
    (0..count)
        .map(|_| {
            let end = offset + body[offset..].iter().position(|b| *b == 0).unwrap();
            let name = String::from_utf8(body[offset..end].to_vec()).unwrap();
            offset = end + 1;
            let oid = u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap());
            offset += 18;
            (name, oid)
        })
        .collect()
}
#[test]
fn pg_introspection_oid_live_casts() {
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
    let response = query(
        &mut socket,
        "select 'pg_class'::regclass::oid as id, NULL::oid as absent, '4294967295'::oid as max from pg_catalog.pg_namespace where nspname = 'public'",
    );
    assert_eq!(
        response[0].0, b'T',
        "OID/regclass query must execute: {response:?}"
    );
    assert_eq!(
        columns(&response[0].1),
        vec![("id".into(), 26), ("absent".into(), 26), ("max".into(), 26)]
    );
    assert!(response.contains(&(b'D', row(&[Some("1259"), None, Some("4294967295")]))));
    let namespace = query(
        &mut socket,
        "select oid from pg_catalog.pg_namespace where nspname = 'public'",
    );
    assert_eq!(columns(&namespace[0].1), vec![("oid".into(), 26)]);
    let cast_sql = "select 'pg_class'::regclass::oid as id from pg_catalog.pg_namespace where nspname = 'public'";
    send(
        &mut socket,
        b'P',
        &[
            b"oid_statement\0".as_slice(),
            cast_sql.as_bytes(),
            b"\0",
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Soid_statement\0");
    send(&mut socket, b'S', &[]);
    let mut described = Vec::new();
    loop {
        let m = read(&mut socket);
        let ready = m.0 == b'Z';
        described.push(m);
        if ready {
            break;
        }
    }
    let metadata = described
        .iter()
        .find(|m| m.0 == b'T')
        .expect("extended Describe must expose an oid field");
    assert_eq!(columns(&metadata.1), vec![("id".into(), 26)]);
    let direct = query(
        &mut socket,
        "select 'pg_class'::regclass as relation from pg_catalog.pg_namespace where nspname = 'public'",
    );
    assert_eq!(columns(&direct[0].1), vec![("relation".into(), 2205)]);
    assert!(direct.contains(&(b'D', row(&[Some("pg_class")]))));
    let text = query(
        &mut socket,
        "select 'pg_class'::regclass::varchar as relation, '1259'::regclass::oid as id from pg_catalog.pg_namespace where nspname = 'public'",
    );
    assert_eq!(
        columns(&text[0].1),
        vec![("relation".into(), 25), ("id".into(), 26)]
    );
    assert!(text.contains(&(b'D', row(&[Some("pg_class"), Some("1259")]))));
    for (value, state) in [
        ("missing_relation", "42P01"),
        ("public.missing_relation", "42P01"),
        ("other.missing_relation", "0A000"),
        ("test.public.missing_relation", "0A000"),
        ("\"unfinished", "22P02"),
    ] {
        let response = query(
            &mut socket,
            &format!(
                "select '{value}'::regclass::oid from pg_catalog.pg_namespace where nspname = 'public'"
            ),
        );
        assert_eq!(response[0].0, b'E', "{response:?}");
        assert!(
            response[0]
                .1
                .windows(state.len())
                .any(|w| w == state.as_bytes()),
            "{response:?}"
        );
    }
    for (value, state) in [
        ("-1", "22003"),
        ("4294967296", "22003"),
        ("invalid", "22P02"),
    ] {
        let response = query(
            &mut socket,
            &format!("select '{value}'::oid from pg_catalog.pg_namespace where nspname = 'public'"),
        );
        assert_eq!(response[0].0, b'E', "{response:?}");
        assert!(
            response[0]
                .1
                .windows(state.len())
                .any(|w| w == state.as_bytes()),
            "{response:?}"
        );
    }
    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE oid_live_one (id INT, KEY oid_idx_one(id))"
        )[0]
        .0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE oid_live_two (id INT, KEY oid_idx_two(id))"
        )[0]
        .0,
        b'C'
    );
    let snapshot = domain.info_schema();
    let one = snapshot
        .TableByName(
            &astersql_infoschema::CiString::new("test"),
            &astersql_infoschema::CiString::new("oid_live_one"),
        )
        .unwrap();
    let two = snapshot
        .TableByName(
            &astersql_infoschema::CiString::new("test"),
            &astersql_infoschema::CiString::new("oid_live_two"),
        )
        .unwrap();
    assert_eq!(
        one.Meta().indices[0].id,
        two.Meta().indices[0].id,
        "index IDs are local to each table"
    );
    let expected = [
        crate::pg_oid::table_oid(one.Meta().id).unwrap().to_string(),
        crate::pg_oid::index_oid(one.Meta().id, one.Meta().indices[0].id)
            .unwrap()
            .to_string(),
        crate::pg_oid::index_oid(two.Meta().id, two.Meta().indices[0].id)
            .unwrap()
            .to_string(),
    ];
    let sql = "select 'oid_live_one'::regclass::oid, 'public.oid_idx_one'::regclass::oid, 'oid_idx_two'::regclass::oid from pg_catalog.pg_namespace where nspname = 'public'";
    let expected_row = (
        b'D',
        row(&expected
            .iter()
            .map(|s| Some(s.as_str()))
            .collect::<Vec<_>>()),
    );
    assert!(query(&mut socket, sql).contains(&expected_row));
    assert!(
        query(
            &mut socket,
            "ALTER TABLE oid_live_one RENAME TO oid_live_renamed"
        )
        .iter()
        .all(|m| m.0 != b'E')
    );
    assert!(
        query(
            &mut socket,
            &sql.replace("'oid_live_one'", "'public.oid_live_renamed'")
        )
        .contains(&expected_row)
    );
    let mut second = TcpStream::connect(addr).unwrap();
    second
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    second
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    second.write_all(&body).unwrap();
    while read(&mut second).0 != b'Z' {}
    assert!(
        query(
            &mut second,
            &sql.replace("'oid_live_one'", "'oid_live_renamed'")
        )
        .contains(&expected_row)
    );
    send(&mut second, b'X', &[]);
    send(&mut socket, b'X', &[]);
    service.close();
    // Restart the PG service over the same authoritative native metadata.
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
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    assert!(
        query(
            &mut socket,
            &sql.replace("'oid_live_one'", "'oid_live_renamed'")
        )
        .contains(&expected_row)
    );
    assert_eq!(query(&mut socket, "DROP TABLE oid_live_renamed")[0].0, b'C');
    assert_eq!(
        query(
            &mut socket,
            &sql.replace("'oid_live_one'", "'oid_live_renamed'")
        )[0]
        .0,
        b'E'
    );
    send(&mut socket, b'X', &[]);
    service.close();
}

#[test]
fn pg_introspection_joins_parameter_rejection() {
    for sql in [
        "SELECT A.oid FROM pg_class A JOIN pg_namespace N ON N.oid = A.relnamespace WHERE N.oid = $1::oid",
        "SELECT A.oid FROM pg_class A JOIN pg_namespace N ON N.oid = $1::oid",
        "SELECT $1::oid FROM pg_class A JOIN pg_namespace N ON N.oid = A.relnamespace",
    ] {
        assert_eq!(parse(sql).unwrap_err().0, "0A000", "{sql}");
    }
    assert!(
        parse("SELECT '$1' FROM pg_class A JOIN pg_namespace N ON N.oid = A.relnamespace")
            .unwrap()
            .is_some()
    );
}
