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
fn until_ready(socket: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
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
#[test]
fn catalog_projection_variants() {
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
    assert_eq!(
        query(&mut socket, "CREATE DATABASE catalog_projection_live")[0].0,
        b'C'
    );
    let response = query(
        &mut socket,
        "/* projection */ SELECT datname AS \"Database Name\", N.oid::bigint AS id, NULL AS absent FROM pg_catalog.pg_database N ORDER BY N.oid LIMIT 100",
    );
    assert_eq!(response[0].0, b'T', "{response:?}");
    assert!(response[0].1[2..].starts_with(b"Database Name\0"));
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "catalog_projection_live")
        .unwrap();
    assert!(response.iter().any(|m| {
        *m == (
            b'D',
            row(&[
                Some("catalog_projection_live"),
                Some(
                    &crate::pg_catalog::namespace_oid(schema.id)
                        .unwrap()
                        .to_string(),
                ),
                None,
            ]),
        )
    }));
    assert_eq!(
        columns(&response[0].1),
        vec![
            ("Database Name".into(), 25),
            ("id".into(), 20),
            ("absent".into(), 25)
        ]
    );
    let expected_id = crate::pg_catalog::namespace_oid(schema.id)
        .unwrap()
        .to_string();
    for sql in [
        "SELECT datname FROM pg_catalog.pg_database WHERE datname = 'catalog_projection_live'",
        "SELECT datname AS name FROM pg_catalog.pg_database WHERE datname = 'catalog_projection_live' ORDER BY name",
        "SELECT datname FROM pg_catalog.pg_database WHERE oid IS NOT NULL ORDER BY CASE WHEN datname = 'catalog_projection_live' THEN -1::bigint ELSE oid::bigint END LIMIT 1",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TDCZ",
            "{sql}: {result:?}"
        );
        assert_eq!(result[1], (b'D', row(&[Some("catalog_projection_live")])));
    }
    let cast = query(
        &mut socket,
        "SELECT oid::varchar AS text_id, oid::varchar::bigint AS id FROM pg_catalog.pg_database WHERE datname = 'catalog_projection_live'",
    );
    assert_eq!(
        columns(&cast[0].1),
        vec![("text_id".into(), 25), ("id".into(), 20)]
    );
    assert_eq!(
        cast[1],
        (b'D', row(&[Some(&expected_id), Some(&expected_id)]))
    );
    let empty = query(
        &mut socket,
        "SELECT oid FROM pg_catalog.pg_database LIMIT 0",
    );
    assert_eq!(empty.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    for (sql, state) in [
        (
            "SELECT oid FROM pg_catalog.pg_database UNION SELECT 1",
            b"0A000",
        ),
        ("SELECT oid FROM pg_catalog.pg_database; SELECT 1", b"0A000"),
        ("SELECT unknown FROM pg_catalog.pg_database", b"0A000"),
        ("SELECT oid, FROM pg_catalog.pg_database", b"42601"),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"EZ",
            "{sql}"
        );
        assert!(result[0].1.windows(5).any(|bytes| bytes == state));
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    }
    // A named catalog statement keeps its structure but observes metadata
    // created after Parse when its portal is first executed.
    let sql = "SELECT datname AS \"Latest Name\", oid::bigint AS id FROM pg_catalog.pg_database ORDER BY oid DESC LIMIT 1";
    send(
        &mut socket,
        b'P',
        &[
            b"catalog_variant\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scatalog_variant\0");
    send(&mut socket, b'S', b"");
    let parsed = until_ready(&mut socket);
    assert_eq!(parsed.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ");
    assert_eq!(
        columns(&parsed[2].1),
        vec![("Latest Name".into(), 25), ("id".into(), 20)]
    );
    assert_eq!(
        query(&mut socket, "CREATE DATABASE catalog_after_parse")[0].0,
        b'C'
    );
    let latest = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "catalog_after_parse")
        .unwrap();
    send(
        &mut socket,
        b'B',
        &[
            b"catalog_portal\0catalog_variant\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"catalog_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let executed = until_ready(&mut socket);
    assert_eq!(
        executed.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"2DCZ",
        "{executed:?}"
    );
    assert_eq!(
        executed[1],
        (
            b'D',
            row(&[
                Some("catalog_after_parse"),
                Some(
                    &crate::pg_catalog::namespace_oid(latest.id)
                        .unwrap()
                        .to_string()
                )
            ])
        )
    );
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn catalog_database_description_semantics() {
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

    for sql in [
        "SELECT objoid, classoid, objsubid, description FROM pg_catalog.pg_description WHERE classoid = 2615 AND objsubid = 0",
        "SELECT objoid, classoid, description FROM pg_catalog.pg_shdescription WHERE objoid = 1 AND classoid = 1262",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(
            result.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TCZ",
            "{sql}: {result:?}"
        );
    }
    assert_eq!(
        query(&mut socket, "CREATE DATABASE catalog_description_live")[0].0,
        b'C'
    );
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "catalog_description_live")
        .unwrap();
    let sql = "SELECT N.oid::bigint id, datname, D.description, datistemplate, datallowconn, pg_get_userbyid(datdba) owner, current_database() db, current_catalog FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_shdescription D ON N.oid = D.objoid AND D.classoid = 1262 WHERE datname = 'catalog_description_live'";
    let result = query(&mut socket, sql);
    assert_eq!(
        result.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ",
        "{result:?}"
    );
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("id".into(), 20),
            ("datname".into(), 25),
            ("description".into(), 25),
            ("datistemplate".into(), 16),
            ("datallowconn".into(), 16),
            ("owner".into(), 25),
            ("db".into(), 25),
            ("current_catalog".into(), 25)
        ]
    );
    assert_eq!(
        result[1],
        (
            b'D',
            row(&[
                Some(
                    &crate::pg_catalog::namespace_oid(schema.id)
                        .unwrap()
                        .to_string()
                ),
                Some("catalog_description_live"),
                None,
                Some("f"),
                Some("t"),
                None,
                Some("test"),
                Some("test")
            ])
        )
    );
    let namespace = query(
        &mut socket,
        "SELECT nspname, D.description FROM pg_catalog.pg_namespace N LEFT JOIN pg_catalog.pg_description D ON N.oid = D.objoid AND D.classoid = 2615 AND D.objsubid = 0 WHERE nspname = 'public'",
    );
    assert_eq!(
        namespace.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ",
        "{namespace:?}"
    );
    assert_eq!(namespace[1], (b'D', row(&[Some("public"), None])));
    let description_columns = query(
        &mut socket,
        "SELECT objoid, classoid, objsubid, description FROM pg_catalog.pg_description",
    );
    assert_eq!(
        columns(&description_columns[0].1),
        vec![
            ("objoid".into(), 20),
            ("classoid".into(), 20),
            ("objsubid".into(), 23),
            ("description".into(), 25)
        ]
    );
    let unknown_owner = query(
        &mut socket,
        "SELECT pg_get_userbyid(123) FROM pg_catalog.pg_database LIMIT 1",
    );
    assert_eq!(unknown_owner[1], (b'D', row(&[None])));
    let mut selected = TcpStream::connect(addr).unwrap();
    selected
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0catalog_description_live\0\0",
    ]
    .concat();
    selected
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    selected.write_all(&startup).unwrap();
    while read(&mut selected).0 != b'Z' {}
    let current = query(
        &mut selected,
        "SELECT current_database(), current_catalog FROM pg_catalog.pg_database LIMIT 1",
    );
    assert_eq!(
        current[1],
        (
            b'D',
            row(&[
                Some("catalog_description_live"),
                Some("catalog_description_live")
            ])
        )
    );
    send(&mut selected, b'X', b"");
    let filtered = query(
        &mut socket,
        "SELECT datname FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_shdescription D ON N.oid = D.objoid WHERE D.classoid = 1262",
    );
    assert_eq!(filtered.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    // Parse records the query structure; first execution reads the latest snapshot.
    let live_sql = "SELECT datname, D.description FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_shdescription D ON N.oid = D.objoid AND D.classoid = 1262 ORDER BY datname";
    send(
        &mut socket,
        b'P',
        &[
            b"description_live\0".as_slice(),
            live_sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', b"");
    assert_eq!(
        until_ready(&mut socket)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"1Z"
    );
    assert_eq!(
        query(&mut socket, "DROP DATABASE catalog_description_live")[0].0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "CREATE DATABASE catalog_description_after_parse"
        )[0]
        .0,
        b'C'
    );
    send(
        &mut socket,
        b'B',
        &[
            b"description_portal\0description_live\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"description_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let executed = until_ready(&mut socket);
    assert_eq!(executed.first().unwrap().0, b'2');
    assert!(
        executed
            .iter()
            .any(|m| *m == (b'D', row(&[Some("catalog_description_after_parse"), None])))
    );
    assert!(
        !executed
            .iter()
            .any(|m| *m == (b'D', row(&[Some("catalog_description_live"), None])))
    );
    assert_eq!(
        executed
            .iter()
            .rev()
            .take(2)
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"ZC"
    );
    let invalid = query(
        &mut socket,
        "SELECT objsubid FROM pg_catalog.pg_shdescription",
    );
    assert_eq!(invalid.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn pg_introspection_relations_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
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
    for sql in [
        "CREATE TABLE relations_live (id INT AUTO_INCREMENT PRIMARY KEY, KEY relations_idx(id))",
        "CREATE DATABASE relations_other",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'C', "{sql}: {result:?}");
    }
    native
        .execute("CREATE TABLE relations_other.hidden (id INT)")
        .unwrap();
    native
        .execute("CREATE VIEW test.relations_view AS SELECT id FROM test.relations_live")
        .unwrap();
    native
        .execute("CREATE SEQUENCE test.relations_seq START WITH 10")
        .unwrap();
    native.execute("CREATE TABLE test.relations_partitioned (id INT, KEY relations_partition_idx(id)) PARTITION BY RANGE (id) (PARTITION p0 VALUES LESS THAN (10), PARTITION p1 VALUES LESS THAN MAXVALUE)").unwrap();
    let result = query(
        &mut socket,
        "SELECT oid, relname, relnamespace, relkind, NULL AS absent FROM pg_catalog.pg_class ORDER BY oid",
    );
    assert_eq!(
        result[0].0, b'T',
        "pg_class must provide real relations: {result:?}"
    );
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("oid".into(), 26),
            ("relname".into(), 25),
            ("relnamespace".into(), 26),
            ("relkind".into(), 25),
            ("absent".into(), 25)
        ]
    );
    let snapshot = domain.info_schema();
    let schema = snapshot
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "test")
        .unwrap();
    let ns = crate::pg_catalog::namespace_oid(schema.id)
        .unwrap()
        .to_string();
    for (name, kind) in [
        ("relations_live", "r"),
        ("relations_view", "v"),
        ("relations_seq", "S"),
        ("relations_partitioned", "p"),
    ] {
        let table = snapshot
            .TableByName(
                &astersql_infoschema::CiString::new("test"),
                &astersql_infoschema::CiString::new(name),
            )
            .unwrap();
        let id = crate::pg_oid::table_oid(table.Meta().id)
            .unwrap()
            .to_string();
        assert!(
            result.contains(&(
                b'D',
                row(&[Some(&id), Some(name), Some(&ns), Some(kind), None])
            )),
            "{name}: {result:?}"
        );
        if name == "relations_live" || name == "relations_partitioned" {
            let (index_name, index_kind) = if name == "relations_live" {
                ("relations_idx", "i")
            } else {
                ("relations_partition_idx", "I")
            };
            let index = table
                .Meta()
                .indices
                .iter()
                .find(|i| i.name.original == index_name)
                .unwrap();
            let index_id = crate::pg_oid::index_oid(table.Meta().id, index.id)
                .unwrap()
                .to_string();
            assert!(result.contains(&(
                b'D',
                row(&[
                    Some(&index_id),
                    Some(index_name),
                    Some(&ns),
                    Some(index_kind),
                    None
                ])
            )));
        }
    }
    assert!(
        !result
            .iter()
            .any(|m| m.0 == b'D' && m.1.windows(6).any(|w| w == b"hidden"))
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT relname FROM pg_class WHERE relname = 'relations_live'"
        )[1],
        (b'D', row(&[Some("relations_live")]))
    );
    // Parse only stores the projection, not the snapshot or relation rows.
    send(
        &mut socket,
        b'P',
        &[
            b"relations_stmt\0".as_slice(),
            b"SELECT relname FROM pg_catalog.pg_class WHERE relname = 'relations_renamed'\0",
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', &[]);
    assert_eq!(until_ready(&mut socket)[0].0, b'1');
    native
        .execute("RENAME TABLE test.relations_live TO test.relations_renamed")
        .unwrap();
    send(
        &mut socket,
        b'B',
        &[
            b"relations_portal\0relations_stmt\0".as_slice(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"relations_portal\0".as_slice(), &0i32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', &[]);
    assert!(until_ready(&mut socket).contains(&(b'D', row(&[Some("relations_renamed")]))));
    assert_eq!(
        query(&mut socket, "DROP TABLE relations_renamed")[0].0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT relname FROM pg_class WHERE relname = 'relations_renamed'"
        )
        .iter()
        .filter(|m| m.0 == b'D')
        .count(),
        0
    );
    // New portals on the same statement must see both DROP and later CREATE.
    let bind_execute = |socket: &mut TcpStream, portal: &str| {
        send(
            socket,
            b'B',
            &[
                format!("{portal}\0relations_stmt\0").as_bytes(),
                &0i16.to_be_bytes(),
                &0i16.to_be_bytes(),
                &0i16.to_be_bytes(),
            ]
            .concat(),
        );
        send(
            socket,
            b'E',
            &[format!("{portal}\0").as_bytes(), &0i32.to_be_bytes()].concat(),
        );
        send(socket, b'S', &[]);
        until_ready(socket)
    };
    let dropped = bind_execute(&mut socket, "relations_dropped");
    assert_eq!(dropped.iter().map(|m| m.0).collect::<Vec<_>>(), b"2CZ");
    native
        .execute("CREATE TABLE test.relations_renamed (id INT)")
        .unwrap();
    assert!(
        bind_execute(&mut socket, "relations_created")
            .contains(&(b'D', row(&[Some("relations_renamed")])))
    );
    for sql in [
        "SELECT xmin FROM pg_catalog.pg_class",
        "SELECT C.relname FROM pg_catalog.pg_class C JOIN public.relations_seq B ON C.oid = B.id",
    ] {
        let response = query(&mut socket, sql);
        assert_eq!(response[0].0, b'E', "{response:?}");
        assert!(
            response[0].1.windows(5).any(|w| w == b"0A000"),
            "{response:?}"
        );
    }
    assert_eq!(
        query(&mut socket, "CREATE TABLE public.pg_class (marker INT)")[0].0,
        b'C'
    );
    assert_eq!(
        query(&mut socket, "INSERT INTO public.pg_class VALUES (7)")[0].0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT relname FROM pg_class WHERE relname = 'relations_view'"
        )[1],
        (b'D', row(&[Some("relations_view")]))
    );
    assert_eq!(
        query(&mut socket, "SET search_path = public, pg_catalog")[0].0,
        b'C'
    );
    assert_eq!(
        query(&mut socket, "SELECT marker FROM pg_class")[1],
        (b'D', row(&[Some("7")]))
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT relname FROM pg_catalog.pg_class WHERE relname = 'relations_view'"
        )[1],
        (b'D', row(&[Some("relations_view")]))
    );
    assert_eq!(
        query(&mut socket, "SELECT marker FROM public.pg_class")[1],
        (b'D', row(&[Some("7")]))
    );
    // Extended parsing must use the same search-path ownership as Query.
    send(
        &mut socket,
        b'P',
        &[
            b"shadow_stmt\0".as_slice(),
            b"SELECT marker FROM pg_class\0",
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'B',
        &[
            b"shadow_portal\0shadow_stmt\0".as_slice(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"shadow_portal\0".as_slice(), &0i32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', &[]);
    assert!(until_ready(&mut socket).contains(&(b'D', row(&[Some("7")]))));
    let shadow = domain
        .info_schema()
        .TableByName(
            &astersql_infoschema::CiString::new("test"),
            &astersql_infoschema::CiString::new("pg_class"),
        )
        .unwrap();
    let shadow_id = crate::pg_oid::table_oid(shadow.Meta().id)
        .unwrap()
        .to_string();
    assert_eq!(
        query(
            &mut socket,
            "SELECT 'pg_class'::regclass::oid, 'pg_class'::regclass FROM pg_catalog.pg_namespace WHERE nspname = 'public'"
        )[1],
        (b'D', row(&[Some(&shadow_id), Some("public.pg_class")]))
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT 'pg_catalog.pg_class'::regclass::oid FROM pg_catalog.pg_namespace WHERE nspname = 'public'"
        )[1],
        (b'D', row(&[Some("1259")]))
    );
    assert_eq!(query(&mut socket, "DROP TABLE public.pg_class")[0].0, b'C');
    // No public collision: explicitly later pg_catalog is still searched.
    assert_eq!(
        query(
            &mut socket,
            "SELECT relname FROM pg_class WHERE relname = 'relations_view'"
        )[1],
        (b'D', row(&[Some("relations_view")]))
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT 'pg_class'::regclass::oid FROM pg_catalog.pg_namespace WHERE nspname = 'public'"
        )[1],
        (b'D', row(&[Some("1259")]))
    );
    let empty = query(&mut socket, "SELECT oid, relname FROM pg_class LIMIT 0");
    assert_eq!(empty.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
    assert_eq!(
        columns(&empty[0].1),
        vec![("oid".into(), 26), ("relname".into(), 25)]
    );
    send(&mut socket, b'X', &[]);
    service.close();
    domain.close();
}

#[test]
fn pg_introspection_relations_ownership() {
    assert!(
        crate::pg_catalog::CatalogQuery::parse("SELECT relname FROM pg_class")
            .unwrap()
            .is_some(),
        "unqualified pg_class must be owned by the PG catalog"
    );
    assert!(
        crate::pg_catalog::CatalogQuery::parse("SELECT marker FROM public.pg_class")
            .unwrap()
            .is_none()
    );
    for sql in [
        "SELECT C.relname FROM public.business B JOIN pg_class C ON B.id = C.oid",
        "SELECT C.relname FROM public.business B, pg_catalog.pg_class C",
        "SELECT C.relname FROM public.business B, pg_class C",
    ] {
        assert_eq!(
            crate::pg_catalog::CatalogQuery::parse(sql).unwrap_err().0,
            "0A000"
        );
    }
}

#[test]
fn pg_introspection_predicates_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE TABLE test.predicate_live (id INT)")
        .unwrap();
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
    for (predicate, keep) in [
        ("relkind IN ('r','v')", true),
        ("relkind NOT IN ('v', NULL)", false),
        ("relkind IN (NULL, 'r')", true),
        ("NULL IN (1,2)", false),
        ("NULL NOT IN (1,2)", false),
        ("relkind NOT IN ('v')", true),
        ("relkind IN ('r'::varchar)", true),
        ("oid IN (0, NULL)", false),
        ("1 <= 1 AND 1 >= 1 AND NOT (1 < 1 OR 1 > 1 OR 1 <> 1)", true),
        ("'a' < 'b' AND 'b' > 'a'", true),
        ("relkind NOT IN ('r', NULL)", false),
        ("relkind IN ('v')", false),
        ("NOT (relkind = 'v')", true),
        ("NULL IS NULL", true),
        ("NOT NULL", false),
        ("NULL OR true", true),
        ("NULL AND false", false),
        ("true AND NOT false", true),
        ("NULL IS NOT NULL", false),
        ("oid = NULL", false),
        ("NOT (oid <> NULL)", false),
        ("(NULL = NULL OR 1 = 1) AND NOT (2 = 3)", true),
        ("1 = 1 OR 1 = 2 AND 2 = 3", true),
        ("NOT 1 = 1 OR 1 = 2", false),
        ("NULL = NULL AND 1 = 2", false),
        ("NULL = NULL OR 1 = 2", false),
        (
            "oid > 0 AND oid >= 0 AND 0 < oid AND 0 <= oid AND oid != 0",
            true,
        ),
        ("relkind <> 'v'", true),
    ] {
        let sql = format!(
            "SELECT relname FROM pg_catalog.pg_class WHERE relname = 'predicate_live' AND ({predicate})"
        );
        let result = query(&mut socket, &sql);
        assert_eq!(result[0].0, b'T', "{sql}: {result:?}");
        let rows: Vec<_> = result.iter().filter(|m| m.0 == b'D').collect();
        assert_eq!(rows.len(), usize::from(keep), "{sql}: {result:?}");
        if keep {
            assert_eq!(rows[0].1, row(&[Some("predicate_live")]));
        }
    }
    // Every combination of true, false and unknown is checked on a real catalog row.
    let truths = [
        ("1 = 1", Some(true)),
        ("1 = 2", Some(false)),
        ("NULL = 1", None),
    ];
    for (left_sql, left) in truths {
        for (right_sql, right) in truths {
            let and = match (left, right) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
            let or = match (left, right) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            };
            let sql = format!(
                "SELECT ({left_sql}) AND ({right_sql}) AS a, ({left_sql}) OR ({right_sql}) AS o FROM pg_catalog.pg_class WHERE relname = 'predicate_live'"
            );
            let result = query(&mut socket, &sql);
            let text = |v: Option<bool>| v.map(|v| if v { "t" } else { "f" });
            assert_eq!(result[1], (b'D', row(&[text(and), text(or)])), "{sql}");
        }
    }
    let joined = query(
        &mut socket,
        "SELECT nspname, D.description IS NULL AS absent FROM pg_catalog.pg_namespace N LEFT JOIN pg_catalog.pg_description D ON N.oid = D.objoid WHERE nspname = 'public' AND NOT (D.description IS NOT NULL)",
    );
    assert_eq!(joined[1], (b'D', row(&[Some("public"), Some("t")])));
    let sql = "SELECT relname FROM pg_catalog.pg_class WHERE relname IN ('predicate_live') AND NOT (oid <= 0 OR relkind <> 'r')";
    send(
        &mut socket,
        b'P',
        &[
            b"predicates\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'B',
        &[
            b"predicate_portal\0predicates\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"predicate_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let extended = until_ready(&mut socket);
    assert_eq!(extended.iter().map(|m| m.0).collect::<Vec<_>>(), b"12DCZ");
    assert_eq!(extended[2], (b'D', row(&[Some("predicate_live")])));
    let projected = query(
        &mut socket,
        "SELECT NULL = 1 AS unknown, NOT (NULL = 1) AS negated, NULL = 1 OR 1 = 1 AS yes, NULL = 1 AND 1 = 2 AS no FROM pg_catalog.pg_class WHERE relname = 'predicate_live'",
    );
    assert_eq!(
        projected[1],
        (b'D', row(&[None, None, Some("t"), Some("f")]))
    );
    for (predicate, state) in [
        ("relkind IN ()".to_string(), "42601"),
        ("relkind IN (relname)".to_string(), "0A000"),
        (
            "relkind IN (SELECT relkind, oid FROM pg_catalog.pg_class)".to_string(),
            "0A000",
        ),
        ("oid LIKE 'x'".to_string(), "0A000"),
        ("oid + 1 = 1".to_string(), "0A000"),
        (format!("oid IN ({})", vec!["1"; 129].join(",")), "0A000"),
        ("NOT oid".to_string(), "0A000"),
        ("oid IN ('x')".to_string(), "0A000"),
        (format!("{}oid = 1", "NOT ".repeat(70)), "0A000"),
        (
            format!("{}oid = 1{}", "(".repeat(70), ")".repeat(70)),
            "0A000",
        ),
        (vec!["1 = 1"; 140].join(" OR "), "0A000"),
    ] {
        let result = query(
            &mut socket,
            &format!("SELECT relname FROM pg_catalog.pg_class WHERE {predicate}"),
        );
        assert_eq!(result[0].0, b'E', "{predicate}: {result:?}");
        assert!(
            result[0]
                .1
                .windows(7)
                .any(|w| w == format!("C{state}\0").as_bytes()),
            "{predicate}: {result:?}"
        );
    }
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    send(&mut socket, b'X', b"");
    service.close();
    domain.close();
}

#[test]
fn pg_introspection_joins_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE TABLE test.joins_a (id INT)")
        .unwrap();
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

    native
        .execute("CREATE TABLE test.joins_b (id INT)")
        .unwrap();
    let sql = "SELECT A.relname, B.relname, D.description, E.description FROM pg_class A INNER JOIN pg_class B ON A.relnamespace = B.relnamespace LEFT OUTER JOIN pg_description D ON D.objoid = A.oid LEFT JOIN pg_shdescription E ON E.objoid = B.oid WHERE A.relname IN ('joins_a','joins_b') AND B.relname IN ('joins_a','joins_b') ORDER BY A.relname, B.relname";
    let result = query(&mut socket, sql);
    assert_eq!(result[0].0, b'T', "multi-join: {result:?}");
    let expected = vec![
        (b'D', row(&[Some("joins_a"), Some("joins_a"), None, None])),
        (b'D', row(&[Some("joins_a"), Some("joins_b"), None, None])),
        (b'D', row(&[Some("joins_b"), Some("joins_a"), None, None])),
        (b'D', row(&[Some("joins_b"), Some("joins_b"), None, None])),
    ];
    assert_eq!(result[1..5], expected);
    for (suffix, count) in [
        (
            "LEFT JOIN pg_class B ON A.oid = B.oid AND B.relname = 'joins_b' WHERE A.relname IN ('joins_a','joins_b')",
            2,
        ),
        (
            "LEFT JOIN pg_class B ON A.oid = B.oid AND B.relname = 'joins_b' WHERE A.relname IN ('joins_a','joins_b') AND B.relname IS NULL",
            1,
        ),
        (
            "JOIN pg_class B ON A.oid = B.oid AND B.relname = 'joins_b' WHERE A.relname IN ('joins_a','joins_b')",
            1,
        ),
        (
            "JOIN pg_description D ON A.oid = D.objoid WHERE A.relname = 'joins_a'",
            0,
        ),
        (
            "LEFT JOIN pg_class B ON NULL = B.oid WHERE A.relname = 'joins_a'",
            1,
        ),
    ] {
        let select = if suffix.contains("pg_description") {
            "A.relname"
        } else {
            "A.relname, B.relname"
        };
        let result = query(
            &mut socket,
            &format!("SELECT {select} FROM pg_class A {suffix} ORDER BY A.relname"),
        );
        assert_eq!(result[0].0, b'T', "{suffix}: {result:?}");
        assert_eq!(
            result.iter().filter(|m| m.0 == b'D').count(),
            count,
            "{suffix}"
        );
        if suffix.starts_with("LEFT JOIN") {
            assert_eq!(result[1], (b'D', row(&[Some("joins_a"), None])));
        }
    }
    // Later ON clauses see all earlier bindings, including NULL-extended rows.
    let result = query(
        &mut socket,
        "SELECT A.relname, B.relname, C.relname FROM pg_class A LEFT JOIN pg_class B ON B.oid = NULL LEFT JOIN pg_class C ON C.oid = B.oid WHERE A.relname = 'joins_a'",
    );
    assert_eq!(result[1], (b'D', row(&[Some("joins_a"), None, None])));
    let result = query(
        &mut socket,
        "SELECT A.relname, B.relname FROM pg_class A JOIN pg_namespace N ON relname = 'joins_a' AND N.oid = A.relnamespace JOIN pg_class B ON B.oid = A.oid WHERE A.relname = 'joins_a'",
    );
    assert_eq!(result[1], (b'D', row(&[Some("joins_a"), Some("joins_a")])));
    for (sql, state) in [
        (
            "SELECT oid FROM pg_class A JOIN pg_class B ON A.oid = B.oid",
            "42702",
        ),
        (
            "SELECT A.oid FROM pg_class A JOIN pg_class A ON A.oid = A.oid",
            "42712",
        ),
        (
            "SELECT A.oid FROM pg_class A JOIN pg_class B ON C.oid = B.oid JOIN pg_class C ON C.oid = A.oid",
            "0A000",
        ),
        (
            "SELECT A.oid FROM pg_class A RIGHT JOIN pg_class B ON A.oid = B.oid",
            "0A000",
        ),
        (
            "SELECT A.oid FROM pg_class A JOIN public.joins_b B ON A.oid = B.id",
            "0A000",
        ),
        (
            "SELECT A.oid FROM pg_class A JOIN pg_class B ON A.oid",
            "0A000",
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'E', "{sql}: {result:?}");
        assert!(
            result[0]
                .1
                .windows(7)
                .any(|w| w == format!("C{state}\0").as_bytes()),
            "{sql}: {result:?}"
        );
    }
    send(
        &mut socket,
        b'P',
        &[
            b"joins_stmt\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'B',
        &[
            b"joins_portal\0joins_stmt\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"joins_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let extended = until_ready(&mut socket);
    assert_eq!(
        extended
            .iter()
            .filter(|m| m.0 == b'D')
            .cloned()
            .collect::<Vec<_>>(),
        expected
    );
    let ninth = format!(
        "SELECT A.oid FROM pg_class A {}",
        (0..9)
            .map(|i| format!("JOIN pg_class J{i} ON A.oid = J{i}.oid"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let result = query(&mut socket, &ninth);
    assert!(result[0].1.windows(7).any(|w| w == b"C0A000\0"));
    // Real tables exercise both bounds, including LIMIT 0: no partial success.
    for i in 0..24 {
        native
            .execute(&format!("CREATE TABLE test.joins_work_{i} (id INT)"))
            .unwrap();
    }
    for (sql, message) in [
        (
            "SELECT A.oid FROM pg_class A JOIN pg_class B ON true JOIN pg_class C ON true JOIN pg_class D ON true LIMIT 0",
            "row limit exceeded (16384 rows)",
        ),
        (
            "SELECT A.oid FROM pg_class A JOIN pg_class B ON true JOIN pg_class C ON C.oid = B.oid JOIN pg_class D ON D.oid = B.oid JOIN pg_class E ON false LIMIT 0",
            "work limit exceeded (100000 comparisons)",
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'E', "{sql}: {result:?}");
        assert!(
            result[0].1.windows(7).any(|w| w == b"C54000\0"),
            "{sql}: {result:?}"
        );
        assert!(result.iter().all(|m| m.0 != b'D'));
        assert!(String::from_utf8_lossy(&result[0].1).contains(message));
    }
    // Cancellation must be checked before native metadata or join work starts.
    use crate::conn::{CancellationToken, SessionDriver};
    let driver =
        ConcreteSessionDriver::new_for_test(domain.clone(), BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(97007, 0, 45, "", None).unwrap();
    let token = CancellationToken::new();
    token.cancel();
    let catalog = crate::pg_catalog::CatalogQuery::parse(sql)
        .unwrap()
        .unwrap();
    let error = catalog.execute(context.as_ref(), &token).unwrap_err();
    assert_eq!(crate::pg_conn::sqlstate(&error), "57014");
    context.close().unwrap();
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    send(&mut socket, b'X', b"");
    service.close();
    domain.close();
}

#[test]
fn pg_introspection_cte_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE TABLE test.cte_live (id INT)")
        .unwrap();
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

    let sql = "WITH names AS (SELECT oid AS id, relname AS name FROM pg_catalog.pg_class WHERE relname = 'cte_live'), selected AS (SELECT id, name FROM names) SELECT C.relname FROM pg_catalog.pg_class C WHERE C.oid IN (SELECT id FROM selected) ORDER BY C.relname";
    let result = query(&mut socket, sql);
    assert_eq!(result[0].0, b'T', "WITH/IN: {result:?}");
    assert_eq!(result[1], (b'D', row(&[Some("cte_live")])));
    assert_eq!(result.iter().filter(|m| m.0 == b'D').count(), 1);
    for (sql, expected) in [
        (
            "WITH names AS (SELECT oid AS id, relname AS name FROM pg_class WHERE relname = 'cte_live') SELECT A.name, B.name FROM names A JOIN names B ON A.id = B.id",
            vec![Some("cte_live"), Some("cte_live")],
        ),
        (
            "WITH pg_class AS (SELECT relname AS name FROM pg_catalog.pg_class WHERE relname = 'cte_live') SELECT name FROM pg_class",
            vec![Some("cte_live")],
        ),
        (
            "WITH spaces AS (SELECT oid AS id FROM pg_namespace WHERE nspname = 'public') SELECT C.relname FROM pg_class C WHERE C.relname = 'cte_live' AND C.relnamespace IN (SELECT id FROM spaces)",
            vec![Some("cte_live")],
        ),
        (
            "WITH names(id) AS (SELECT oid FROM pg_class WHERE relname = 'cte_live') SELECT C.relname FROM pg_class C WHERE C.oid IN (SELECT id FROM names)",
            vec![Some("cte_live")],
        ),
        (
            "SELECT relname FROM pg_class WHERE relname = 'cte_live' AND oid NOT IN (SELECT oid FROM pg_class WHERE false)",
            vec![Some("cte_live")],
        ),
        (
            "SELECT NULL IN (SELECT oid FROM pg_class WHERE false), NULL NOT IN (SELECT oid FROM pg_class WHERE false), oid IN (SELECT NULL FROM pg_class WHERE relname = 'cte_live'), oid NOT IN (SELECT NULL FROM pg_class WHERE relname = 'cte_live') FROM pg_class WHERE relname = 'cte_live'",
            vec![Some("f"), Some("t"), None, None],
        ),
        (
            "SELECT oid IN (SELECT oid FROM pg_class WHERE relname = 'cte_live'), oid NOT IN (SELECT oid FROM pg_class WHERE relname = 'cte_live') FROM pg_class WHERE relname = 'cte_live'",
            vec![Some("t"), Some("f")],
        ),
        (
            "WITH x AS (SELECT oid AS id FROM pg_class WHERE relname = 'cte_live') SELECT id IN (WITH x AS (SELECT id FROM x) SELECT id FROM x) FROM x",
            vec![Some("t")],
        ),
        (
            "SELECT true IN (SELECT true FROM pg_class WHERE relname = 'cte_live'), 'cte_live' IN (SELECT relname FROM pg_class WHERE relname = 'cte_live') FROM pg_class WHERE relname = 'cte_live'",
            vec![Some("t"), Some("t")],
        ),
        (
            "WITH nulls AS (SELECT NULL::oid AS id FROM pg_class WHERE relname = 'cte_live') SELECT C.relname, N.id FROM pg_class C LEFT JOIN nulls N ON C.oid IN (SELECT id FROM nulls) WHERE C.relname = 'cte_live'",
            vec![Some("cte_live"), None],
        ),
        (
            "WITH x AS (SELECT oid AS id FROM pg_class WHERE relname = 'cte_live') SELECT id IN (WITH x AS (SELECT NULL::oid AS id FROM pg_class WHERE relname = 'cte_live') SELECT id FROM x) FROM x",
            vec![None],
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'T', "{sql}: {result:?}");
        assert_eq!(result[1], (b'D', row(&expected)), "{sql}");
        assert_eq!(result.iter().filter(|m| m.0 == b'D').count(), 1, "{sql}");
    }
    for (sql, state) in [
        (
            "WITH RECURSIVE x AS (SELECT oid FROM pg_class) SELECT oid FROM x",
            "0A000",
        ),
        (
            "WITH x AS (DELETE FROM pg_class RETURNING oid) SELECT oid FROM x",
            "0A000",
        ),
        (
            "WITH x AS (SELECT oid FROM x) SELECT oid FROM pg_class",
            "42P01",
        ),
        (
            "WITH x AS (SELECT oid FROM y), y AS (SELECT oid FROM pg_class) SELECT oid FROM x",
            "42P01",
        ),
        (
            "WITH x AS (SELECT oid FROM pg_class), x AS (SELECT oid FROM pg_class) SELECT oid FROM x",
            "42712",
        ),
        (
            "SELECT oid FROM pg_class WHERE oid IN (SELECT oid, relname FROM pg_class)",
            "0A000",
        ),
        (
            "SELECT C.oid FROM pg_class C WHERE C.oid IN (SELECT B.oid FROM pg_class B WHERE B.oid = C.oid)",
            "0A000",
        ),
        (
            "SELECT oid FROM pg_class WHERE oid IN (SELECT relname FROM pg_class)",
            "0A000",
        ),
        (
            "WITH x AS (SELECT oid AS id, oid AS id FROM pg_class) SELECT id FROM x",
            "42702",
        ),
        (
            "WITH x(id, extra) AS (SELECT oid FROM pg_class) SELECT id FROM x",
            "42601",
        ),
        (
            "WITH pg_class AS (SELECT oid FROM pg_catalog.pg_class) SELECT pg_catalog.pg_class.oid FROM pg_class",
            "0A000",
        ),
        (
            "WITH x AS (SELECT oid FROM pg_class) SELECT oid FROM public.cte_live",
            "0A000",
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'E', "{sql}: {result:?}");
        assert!(
            result[0]
                .1
                .windows(7)
                .any(|w| w == format!("C{state}\0").as_bytes()),
            "{sql}: {result:?}"
        );
    }
    // CTE lookup precedes public/catalog search_path collisions; explicit
    // pg_catalog qualification still addresses the provider, never the CTE.
    native
        .execute("CREATE TABLE test.pg_class (id INT)")
        .unwrap();
    assert_eq!(
        query(&mut socket, "SET search_path TO public, pg_catalog")[0].0,
        b'C'
    );
    let result = query(
        &mut socket,
        "WITH pg_class AS (SELECT relname AS name FROM pg_catalog.pg_class WHERE relname = 'cte_live') SELECT name FROM pg_class",
    );
    assert_eq!(result[1], (b'D', row(&[Some("cte_live")])));
    assert_eq!(query(&mut socket, "RESET search_path")[0].0, b'C');
    let result = query(
        &mut socket,
        "WITH pg_class AS (SELECT NULL::oid AS oid FROM pg_catalog.pg_class WHERE relname = 'cte_live') SELECT oid IN (SELECT oid FROM pg_class) FROM pg_catalog.pg_class WHERE relname = 'cte_live'",
    );
    assert_eq!(result[1], (b'D', row(&[None])));
    let result = query(
        &mut socket,
        "WITH objects AS (SELECT oid::regclass AS id FROM pg_class WHERE relname = 'cte_live') SELECT id::varchar, id FROM objects",
    );
    assert_eq!(
        columns(&result[0].1),
        vec![("id".into(), 25), ("id".into(), 2205)]
    );
    assert_eq!(
        result[1],
        (b'D', row(&[Some("cte_live"), Some("cte_live")]))
    );
    for i in 0..7 {
        native
            .execute(&format!("CREATE TABLE test.cte_budget_{i} (id INT)"))
            .unwrap();
    }
    // Materialization is bounded across the entire query; unused definitions
    // and LIMIT 0 cannot bypass the budget. IN shares the join work budget.
    for sql in [
        "WITH x AS (SELECT A.oid FROM pg_class A JOIN pg_class B ON true JOIN pg_class C ON true JOIN pg_class D ON true) SELECT oid FROM x LIMIT 0",
        "WITH x AS (SELECT A.oid FROM pg_class A JOIN pg_class B ON true JOIN pg_class C ON true) SELECT oid IN (SELECT oid FROM x) FROM pg_class WHERE oid = 0 LIMIT 0",
        "SELECT oid FROM pg_class WHERE 0 IN (SELECT A.oid FROM pg_class A JOIN pg_class B ON true JOIN pg_class C ON true) LIMIT 0",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'E', "{sql}: {result:?}");
        assert!(
            result[0].1.windows(7).any(|w| w == b"C54000\0"),
            "{sql}: {result:?}"
        );
        assert!(result.iter().all(|m| m.0 != b'D'));
    }
    send(
        &mut socket,
        b'P',
        &[
            b"cte_stmt\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scte_stmt\0");
    send(&mut socket, b'S', b"");
    let described = until_ready(&mut socket);
    assert!(described.iter().any(|m| m.0 == b'T'));
    send(
        &mut socket,
        b'B',
        &[
            b"cte_before\0cte_stmt\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"cte_before\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let before = until_ready(&mut socket);
    assert!(before.iter().all(|m| m.0 != b'E'), "{before:?}");
    assert_eq!(
        before
            .iter()
            .filter(|m| m.0 == b'D')
            .cloned()
            .collect::<Vec<_>>(),
        vec![(b'D', row(&[Some("cte_live")]))]
    );
    native
        .execute("RENAME TABLE test.cte_live TO test.cte_changed")
        .unwrap();
    send(
        &mut socket,
        b'B',
        &[
            b"cte_portal\0cte_stmt\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"cte_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let extended = until_ready(&mut socket);
    assert!(extended.iter().all(|m| m.0 != b'E'), "{extended:?}");
    assert_eq!(
        extended.iter().filter(|m| m.0 == b'D').count(),
        0,
        "Execute must reread metadata"
    );
    use crate::conn::{CancellationToken, SessionDriver};
    let driver =
        ConcreteSessionDriver::new_for_test(domain.clone(), BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(97008, 0, 45, "", None).unwrap();
    let token = CancellationToken::new();
    token.cancel();
    let catalog = crate::pg_catalog::CatalogQuery::parse(sql)
        .unwrap()
        .unwrap();
    let error = catalog.execute(context.as_ref(), &token).unwrap_err();
    assert_eq!(crate::pg_conn::sqlstate(&error), "57014");
    context.close().unwrap();
    assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    send(&mut socket, b'X', b"");
    service.close();
    domain.close();
}

#[test]
fn pg_introspection_columns_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE TABLE test.columns_live (id INT PRIMARY KEY, amount DECIMAL(12,3) DEFAULT 1.250, label VARCHAR(24) DEFAULT 'hello', day DATE, stamp DATETIME(3) DEFAULT CURRENT_TIMESTAMP(3))").unwrap();
    native.execute("CREATE DATABASE columns_other").unwrap();
    native
        .execute("CREATE TABLE columns_other.hidden (id INT)")
        .unwrap();
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
    let sql = "SELECT a.attnum, a.attname, a.atttypid, pg_catalog.format_type(a.atttypid,a.atttypmod) AS spec, a.attnotnull, pg_catalog.pg_get_expr(d.adbin,a.attrelid) AS def FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_attrdef d ON a.attrelid=d.adrelid AND a.attnum=d.adnum WHERE a.attrelid='public.columns_live'::regclass::oid ORDER BY a.attnum";
    let result = query(&mut socket, sql);
    assert_eq!(
        result[0].0, b'T',
        "column provider must return real metadata: {result:?}"
    );
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("attnum".into(), 21),
            ("attname".into(), 25),
            ("atttypid".into(), 26),
            ("spec".into(), 25),
            ("attnotnull".into(), 16),
            ("def".into(), 25)
        ]
    );
    for expected in [
        row(&[
            Some("1"),
            Some("id"),
            Some("23"),
            Some("integer"),
            Some("t"),
            None,
        ]),
        row(&[
            Some("2"),
            Some("amount"),
            Some("1700"),
            Some("numeric(12,3)"),
            Some("f"),
            Some("1.250"),
        ]),
        row(&[
            Some("3"),
            Some("label"),
            Some("1043"),
            Some("character varying(24)"),
            Some("f"),
            Some("'hello'::character varying"),
        ]),
        row(&[
            Some("4"),
            Some("day"),
            Some("1082"),
            Some("date"),
            Some("f"),
            None,
        ]),
        row(&[
            Some("5"),
            Some("stamp"),
            Some("1114"),
            Some("timestamp(3) without time zone"),
            Some("f"),
            Some("CURRENT_TIMESTAMP(3)"),
        ]),
    ] {
        assert!(result.contains(&(b'D', expected)), "{result:?}");
    }
    let result = query(
        &mut socket,
        "SELECT t.oid,t.typname FROM pg_catalog.pg_type t JOIN pg_catalog.pg_attribute a ON t.oid=a.atttypid WHERE a.attrelid='public.columns_live'::regclass::oid ORDER BY a.attnum",
    );
    assert_eq!(result[0].0, b'T', "{result:?}");
    assert!(result.contains(&(b'D', row(&[Some("1700"), Some("numeric")]))));
    let source = include_str!("../../docs/postgresql-protocol-first-phase.md");
    let template = source
        .split("#### RetrieveColumns")
        .nth(1)
        .unwrap()
        .split("```sql")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.lower == "test")
        .unwrap();
    let namespace = crate::pg_oid::namespace_oid(schema.id).unwrap();
    let full = query(&mut socket, &template.replace("$1", &namespace.to_string()));
    assert_eq!(full[0].0, b'T', "complete PG18 column template: {full:?}");
    assert_eq!(full.iter().filter(|(tag, _)| *tag == b'D').count(), 5);
    assert_eq!(columns(&full[0].1)[11].1, 1009);
    let types_template = source
        .split("#### RetrieveDataTypes")
        .nth(1)
        .unwrap()
        .split("```sql")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let types = query(&mut socket, &types_template.replace("$1", "11"));
    assert_eq!(types[0].0, b'T', "complete PG18 type template: {types:?}");
    let mut previous = 0u32;
    for (_, body) in types.iter().filter(|(tag, _)| *tag == b'D') {
        let length = i32::from_be_bytes(body[2..6].try_into().unwrap()) as usize;
        let id = std::str::from_utf8(&body[6..6 + length])
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(
            id > previous,
            "ORDER BY 1 must sort projected type OID: {types:?}"
        );
        previous = id;
    }
    assert!(previous > 0, "builtin types must be real nonempty rows");
    let types = query(
        &mut socket,
        &types_template.replace("$1", &namespace.to_string()),
    );
    assert_eq!(types[0].0, b'T', "{types:?}");
    assert_eq!(types.iter().filter(|(tag, _)| *tag == b'D').count(), 0);
    // Parse/Describe does not freeze catalog rows; reuse the same Bind/Execute
    // after native DROP/ADD, then verify offset differs from persistent ID.
    send(
        &mut socket,
        b'P',
        &[
            b"cols\0".as_slice(),
            template.as_bytes(),
            b"\0",
            &1i16.to_be_bytes(),
            &26u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scols\0");
    send(&mut socket, b'S', b"");
    let described = until_ready(&mut socket);
    assert_eq!(described[0].0, b'1', "{described:?}");
    assert_eq!(
        columns(&described.iter().find(|(tag, _)| *tag == b'T').unwrap().1),
        columns(&full[0].1)
    );
    let bind = |socket: &mut TcpStream| {
        let text = namespace.to_string();
        send(
            socket,
            b'B',
            &[
                b"\0cols\0".as_slice(),
                &0i16.to_be_bytes(),
                &1i16.to_be_bytes(),
                &(text.len() as i32).to_be_bytes(),
                text.as_bytes(),
                &0i16.to_be_bytes(),
            ]
            .concat(),
        );
        send(
            socket,
            b'E',
            &[b"\0".as_slice(), &0i32.to_be_bytes()].concat(),
        );
        send(socket, b'S', b"");
        until_ready(socket)
    };
    let before = bind(&mut socket);
    assert_eq!(
        before
            .iter()
            .filter(|(tag, _)| *tag == b'D')
            .collect::<Vec<_>>(),
        full.iter()
            .filter(|(tag, _)| *tag == b'D')
            .collect::<Vec<_>>()
    );
    native
        .execute("ALTER TABLE test.columns_live DROP COLUMN label")
        .unwrap();
    native
        .execute("ALTER TABLE test.columns_live ADD COLUMN added VARCHAR(7) DEFAULT 'new'")
        .unwrap();
    let result = query(&mut socket, sql);
    assert_eq!(result[0].0, b'T', "{result:?}");
    assert!(
        result.contains(&(
            b'D',
            row(&[
                Some("5"),
                Some("added"),
                Some("1043"),
                Some("character varying(7)"),
                Some("f"),
                Some("'new'::character varying")
            ])
        )),
        "{result:?}"
    );
    native
        .execute(
            "ALTER TABLE test.columns_live MODIFY COLUMN amount DECIMAL(8,2) NOT NULL DEFAULT 2.50",
        )
        .unwrap();
    let changed = query(&mut socket, sql);
    assert!(
        changed.contains(&(
            b'D',
            row(&[
                Some("2"),
                Some("amount"),
                Some("1700"),
                Some("numeric(8,2)"),
                Some("t"),
                Some("2.50")
            ])
        )),
        "ALTER must reread type/null/default: {changed:?}"
    );
    let table = domain
        .info_schema()
        .TableByName(
            &astersql_infoschema::CiString::new("test"),
            &astersql_infoschema::CiString::new("columns_live"),
        )
        .unwrap();
    let model = table.Meta().model_meta.as_ref().unwrap();
    let added = model.Columns.iter().find(|c| c.Name.O == "added").unwrap();
    assert_ne!(
        added.ID,
        added.Offset as i64 + 1,
        "fixture must distinguish native ID from offset"
    );
    let after = bind(&mut socket);
    assert_eq!(after.iter().filter(|(tag, _)| *tag == b'D').count(), 5);
    assert_ne!(
        before
            .iter()
            .filter(|(tag, _)| *tag == b'D')
            .collect::<Vec<_>>(),
        after
            .iter()
            .filter(|(tag, _)| *tag == b'D')
            .collect::<Vec<_>>()
    );
    for sql in [
        "SELECT attname FROM pg_attribute WHERE attrelid=0",
        "SELECT adbin FROM pg_attrdef WHERE adrelid=0",
        "SELECT typname FROM pg_type WHERE typnamespace=0",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'T', "{sql}: {result:?}");
        assert_eq!(result.iter().filter(|(tag, _)| *tag == b'D').count(), 0);
    }
    let result = query(
        &mut socket,
        "SELECT xmin,attfdwoptions,attidentity,attgenerated FROM pg_attribute WHERE attrelid='public.columns_live'::regclass::oid AND attname='id'",
    );
    assert_eq!(result[1], (b'D', row(&[None, None, Some(""), Some("")])));
    let result = query(
        &mut socket,
        "SELECT format_type(NULL,NULL), format_type(0,-1), format_type(999999,-1), format_type(1042,NULL), format_type(1042,-1), format_type(23,7), format_type(1009,NULL), pg_get_expr(NULL,0) FROM pg_type LIMIT 1",
    );
    assert_eq!(
        result[1],
        (
            b'D',
            row(&[
                None,
                Some("-"),
                Some("???"),
                Some("character"),
                Some("bpchar"),
                Some("integer"),
                Some("text[]"),
                None
            ])
        )
    );
    for (expression, expected) in [
        ("(1,2)=(1,2)", Some("t")),
        ("(NULL,1)=(NULL,2)", Some("f")),
        ("(NULL,1)=(NULL,1)", None),
        ("(1,NULL)=(2,NULL)", Some("f")),
    ] {
        let result = query(
            &mut socket,
            &format!("SELECT {expression} FROM pg_type LIMIT 1"),
        );
        assert_eq!(
            result[1],
            (b'D', row(&[expected])),
            "{expression}: {result:?}"
        );
    }
    let result = query(
        &mut socket,
        "SELECT 'c'::\"char\" AS kind FROM pg_type LIMIT 1",
    );
    assert_eq!(columns(&result[0].1), vec![("kind".into(), 18)]);
    assert_eq!(result[1], (b'D', row(&[Some("c")])));
    for (sql, state) in [
        ("SELECT 'wide'::\"char\" FROM pg_type LIMIT 1", "0A000"),
        ("SELECT oid FROM pg_type ORDER BY 0", "42P10"),
        ("SELECT oid FROM pg_type ORDER BY 2", "42P10"),
        (
            "SELECT format_type(4294967296,NULL) FROM pg_type LIMIT 1",
            "22003",
        ),
        (
            "SELECT format_type(1700,2147483648) FROM pg_type LIMIT 1",
            "22003",
        ),
        ("SELECT format_type(1114,7) FROM pg_type LIMIT 1", "0A000"),
        ("SELECT (1,2,3)=(1,2,3) FROM pg_type LIMIT 1", "0A000"),
        ("SELECT (1,2)<(1,3) FROM pg_type LIMIT 1", "0A000"),
        ("SELECT (1,2)=(1,'two') FROM pg_type LIMIT 1", "0A000"),
        ("SELECT format_type('bad',-1) FROM pg_type LIMIT 1", "0A000"),
        ("SELECT pg_get_expr('fake',0) FROM pg_type LIMIT 1", "0A000"),
        (
            "SELECT attname FROM pg_attribute WHERE (attrelid,attnum)=(0)",
            "0A000",
        ),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'E', "{sql}: {result:?}");
        assert!(
            result[0]
                .1
                .windows(7)
                .any(|w| w == format!("C{state}\0").as_bytes()),
            "{sql}: {result:?}"
        );
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    }
    native
        .execute("CREATE TABLE test.columns_unsupported (value ENUM('a','b'))")
        .unwrap();
    let rejected = query(&mut socket, "SELECT attname FROM pg_attribute");
    assert_eq!(
        rejected[0].0, b'E',
        "unsupported native enum must not be guessed: {rejected:?}"
    );
    assert!(rejected[0].1.windows(7).any(|w| w == b"C0A000\0"));
    let rejected = query(&mut socket, "SELECT typname FROM pg_type");
    assert_eq!(
        rejected[0].0, b'E',
        "type catalog cannot silently hide unmapped native enum: {rejected:?}"
    );
    assert!(rejected[0].1.windows(7).any(|w| w == b"C0A000\0"));
    native
        .execute("DROP TABLE test.columns_unsupported")
        .unwrap();
    native.execute("CREATE TABLE test.columns_auto (id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY, tiny TINYINT(1), nullable INT DEFAULT NULL)").unwrap();
    let result = query(
        &mut socket,
        "SELECT attname,atttypid,attidentity FROM pg_attribute WHERE attrelid='public.columns_auto'::regclass::oid ORDER BY attnum",
    );
    assert_eq!(
        result[1],
        (b'D', row(&[Some("id"), Some("1700"), Some("")]))
    );
    assert_eq!(
        result[2],
        (b'D', row(&[Some("tiny"), Some("21"), Some("")]))
    );

    native
        .execute(
            "CREATE TABLE test.columns_generated (a INT, b INT GENERATED ALWAYS AS (a+1) STORED)",
        )
        .unwrap();
    let rejected = query(&mut socket, "SELECT attname FROM pg_attribute");
    assert_eq!(
        rejected[0].0, b'E',
        "native generated expression cannot be copied as PG SQL: {rejected:?}"
    );
    assert!(rejected[0].1.windows(7).any(|w| w == b"C0A000\0"));
    native.execute("DROP TABLE test.columns_generated").unwrap();
    let result = query(
        &mut socket,
        "SELECT a.attname FROM pg_attribute a JOIN pg_type b ON true JOIN pg_type c ON true JOIN pg_type d ON true",
    );
    assert_eq!(result[0].0, b'E', "{result:?}");
    assert!(result[0].1.windows(7).any(|w| w == b"C54000\0"));
    assert!(result.iter().all(|(tag, _)| *tag != b'D'));
    use crate::conn::{CancellationToken, SessionDriver};
    let driver =
        ConcreteSessionDriver::new_for_test(domain.clone(), BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(97010, 0, 45, "", None).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let catalog = crate::pg_catalog::CatalogQuery::parse("SELECT attname FROM pg_attribute")
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::pg_conn::sqlstate(&catalog.execute(context.as_ref(), &cancel).unwrap_err()),
        "57014"
    );
    context.close().unwrap();
    send(&mut socket, b'X', &[]);
    service.close();
    domain.close();
}

#[test]
fn pg_introspection_constraints_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE TABLE test.constraints_parent (a INT, b INT, label VARCHAR(20), PRIMARY KEY(a,b), UNIQUE KEY uq_label(label), KEY ix_b(b))").unwrap();
    native.execute("CREATE TABLE test.constraints_child (a INT, b INT, CONSTRAINT fk_parent FOREIGN KEY(a,b) REFERENCES test.constraints_parent(a,b) ON DELETE CASCADE ON UPDATE RESTRICT)").unwrap();
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
    let result = query(
        &mut socket,
        "SELECT i.indnkeyatts,i.indisunique,i.indisprimary,i.indkey,pg_get_indexdef(i.indexrelid) AS definition FROM pg_index i WHERE i.indrelid='public.constraints_parent'::regclass::oid ORDER BY i.indexrelid",
    );
    assert_eq!(result[0].0, b'T', "real index catalog: {result:?}");
    assert_eq!(
        result.iter().filter(|m| m.0 == b'D').count(),
        3,
        "{result:?}"
    );
    assert!(result.iter().any(|m| m.0==b'D' && m.1==row(&[Some("2"),Some("t"),Some("t"),Some("1 2"),Some("CREATE UNIQUE INDEX \"PRIMARY\" ON public.\"constraints_parent\" USING btree (\"a\", \"b\")")])));
    assert!(result.iter().any(|m| m.0 == b'D'
        && m.1
            == row(&[
                Some("1"),
                Some("t"),
                Some("f"),
                Some("3"),
                Some(
                    "CREATE UNIQUE INDEX \"uq_label\" ON public.\"constraints_parent\" USING btree (\"label\")"
                )
            ])));
    assert!(result.iter().any(|m| m.0 == b'D'
        && m.1
            == row(&[
                Some("1"),
                Some("f"),
                Some("f"),
                Some("2"),
                Some("CREATE INDEX \"ix_b\" ON public.\"constraints_parent\" USING btree (\"b\")")
            ])));
    let result = query(
        &mut socket,
        "SELECT conname,contype,conkey,confkey,confupdtype,confdeltype,pg_get_constraintdef(oid),xmin FROM pg_constraint WHERE conrelid='public.constraints_child'::regclass::oid",
    );
    assert_eq!(result[0].0, b'T', "real FK catalog: {result:?}");
    assert!(result.iter().any(|m| m.0==b'D' && m.1==row(&[Some("fk_parent"),Some("f"),Some("{1,2}"),Some("{1,2}"),Some("r"),Some("c"),Some("FOREIGN KEY (\"a\", \"b\") REFERENCES public.\"constraints_parent\" (\"a\", \"b\") ON UPDATE RESTRICT ON DELETE CASCADE"),None])),"{result:?}");
    let result = query(
        &mut socket,
        "SELECT conname,contype,conkey,pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid='public.constraints_parent'::regclass::oid ORDER BY conname",
    );
    assert_eq!(
        result.iter().filter(|m| m.0 == b'D').count(),
        2,
        "{result:?}"
    );
    let linked = query(
        &mut socket,
        "SELECT c.conname FROM pg_constraint c JOIN pg_index i ON c.conindid=i.indexrelid JOIN pg_class t ON c.confrelid=t.oid WHERE c.contype='f' AND i.indisprimary AND t.relname='constraints_parent'",
    );
    assert!(
        linked
            .iter()
            .any(|m| m.0 == b'D' && m.1 == row(&[Some("fk_parent")]))
    );
    let typed = query(
        &mut socket,
        "SELECT conkey,confkey,conexclop,contype FROM pg_constraint WHERE contype='f'",
    );
    assert_eq!(
        columns(&typed[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![1005, 1005, 1028, 18]
    );
    let part = query(
        &mut socket,
        "SELECT pg_get_indexdef(indexrelid,2,true),pg_get_indexdef(indexrelid,99,false),pg_get_indexdef(indexrelid,-1,true) FROM pg_index WHERE indisprimary",
    );
    assert!(
        part.iter()
            .any(|m| m.0 == b'D' && m.1 == row(&[Some("\"b\""), None, None]))
    );
    let primary_type = query(
        &mut socket,
        "SELECT indkey,indoption FROM pg_index WHERE indisprimary",
    );
    assert_eq!(
        columns(&primary_type[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![22, 22]
    );
    // Same parsed statement must re-read current metadata at Execute.
    let statement_sql = "SELECT conname FROM pg_constraint WHERE contype='f'";
    send(
        &mut socket,
        b'P',
        &[
            b"constraint_stmt\0",
            statement_sql.as_bytes(),
            b"\0",
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', &[]);
    assert_eq!(until_ready(&mut socket)[0].0, b'1');
    send(
        &mut socket,
        b'B',
        &[
            b"\0constraint_stmt\0".as_slice(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"\0".as_slice(), &0i32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', &[]);
    let before = until_ready(&mut socket);
    assert!(
        before
            .iter()
            .any(|m| m.0 == b'D' && m.1 == row(&[Some("fk_parent")])),
        "{before:?}"
    );

    native
        .execute("ALTER TABLE test.constraints_child DROP FOREIGN KEY fk_parent")
        .unwrap();
    assert!(
        !query(
            &mut socket,
            "SELECT oid FROM pg_constraint WHERE conrelid='public.constraints_child'::regclass::oid"
        )
        .iter()
        .any(|m| m.0 == b'D')
    );
    send(
        &mut socket,
        b'B',
        &[
            b"\0constraint_stmt\0".as_slice(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"\0".as_slice(), &0i32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', &[]);
    let refreshed = until_ready(&mut socket);
    assert_eq!(refreshed[0].0, b'2', "{refreshed:?}");
    assert!(!refreshed.iter().any(|m| m.0 == b'D'));
    native
        .execute("ALTER TABLE test.constraints_parent DROP INDEX ix_b")
        .unwrap();
    assert_eq!(query(&mut socket,"SELECT indexrelid FROM pg_index WHERE indrelid='public.constraints_parent'::regclass::oid").iter().filter(|m|m.0==b'D').count(),2);
    assert!(
        query(
            &mut socket,
            "SELECT pg_get_indexdef(NULL),pg_get_constraintdef(0) FROM pg_index"
        )
        .iter()
        .filter(|m| m.0 == b'D')
        .all(|m| m.1 == row(&[None, None]))
    );
    let rejected = query(
        &mut socket,
        "SELECT pg_get_indexdef(indexrelid,1,true,0) FROM pg_index",
    );
    assert_eq!(rejected[0].0, b'E');
    assert!(rejected[0].1.windows(6).any(|w| w == b"0A000\0"));
    native
        .execute("CREATE TABLE test.constraints_handle (id INT PRIMARY KEY)")
        .unwrap();
    let handle = query(
        &mut socket,
        "SELECT pg_get_indexdef(i.indexrelid),pg_get_constraintdef(c.oid) FROM pg_index i JOIN pg_constraint c ON i.indexrelid=c.conindid JOIN pg_class t ON t.oid=i.indexrelid WHERE i.indrelid='public.constraints_handle'::regclass::oid AND t.relkind='i'",
    );
    assert!(handle.iter().any(|m|m.0==b'D' && m.1==row(&[Some("CREATE UNIQUE INDEX \"PRIMARY\" ON public.\"constraints_handle\" USING btree (\"id\")"),Some("PRIMARY KEY (\"id\")")])) ,"{handle:?}");
    native
        .execute("DROP TABLE test.constraints_handle")
        .unwrap();
    native
        .execute(r#"CREATE TABLE test.`order` (`select` INT, KEY `ix"quote`(`select`))"#)
        .unwrap();
    let quoted = query(
        &mut socket,
        "SELECT pg_get_indexdef(indexrelid) FROM pg_index WHERE indrelid='public.order'::regclass::oid",
    );
    assert!(
        quoted.iter().any(|m| m.0 == b'D'
            && m.1
                == row(&[Some(
                    r#"CREATE INDEX "ix""quote" ON public."order" USING btree ("select")"#
                )])),
        "{quoted:?}"
    );
    native.execute("DROP TABLE test.`order`").unwrap();
    native
        .execute(
            "CREATE TABLE test.constraints_prefix (label VARCHAR(20), KEY ix_prefix(label(3)))",
        )
        .unwrap();
    let rejected = query(&mut socket, "SELECT indkey FROM pg_index");
    assert_eq!(rejected[0].0, b'E');
    assert!(rejected[0].1.windows(6).any(|w| w == b"0A000\0"));
    native
        .execute("DROP TABLE test.constraints_prefix")
        .unwrap();
    native.execute("DROP TABLE test.constraints_child").unwrap();
    native
        .execute("DROP TABLE test.constraints_parent")
        .unwrap();
    let empty = query(&mut socket, "SELECT indexrelid FROM pg_index");
    assert_eq!(empty[0].0, b'T', "{empty:?}");
    assert!(!empty.iter().any(|m| m.0 == b'D'));
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
}

#[test]
fn pg_introspection_functions_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    assert!(
        native
            .execute("SELECT ROUTINE_NAME FROM information_schema.routines")
            .unwrap()
            .remove(0)
            .next_row()
            .unwrap()
            .is_none()
    );
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

    let source_sql = r#"with system_languages as ( select oid as lang
                           from pg_catalog.pg_language
                           where lanname in ('c','internal') )
select oid as id,
       pg_catalog.pg_get_function_arguments(oid) as arguments_def,
       pg_catalog.pg_get_function_result(oid) as result_def,
       pg_catalog.pg_get_function_sqlbody(oid) /* null */ as sqlbody_def,
       prosrc as source_text
from pg_catalog.pg_proc
where pronamespace = ?::oid
  --  and pg_proc.proname in ( :[*f_names] )
  --  and pg_catalog.age(xmin) <= #SRCTXAGE
  and not (prokind = 'a') /* proisagg */
  and prolang not in (select lang from system_languages)
  and prosrc is not null
"#;

    let namespace = query(
        &mut socket,
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    );
    assert_eq!(namespace[1].0, b'D', "{namespace:?}");
    let length = i32::from_be_bytes(namespace[1].1[2..6].try_into().unwrap()) as usize;
    let namespace = std::str::from_utf8(&namespace[1].1[6..6 + length])
        .unwrap()
        .to_owned();
    for oid in [&namespace, "11"] {
        let result = query(&mut socket, &source_sql.replace("?", oid));
        assert_eq!(result[0].0, b'T', "complete function WITH: {result:?}");
        assert_eq!(
            columns(&result[0].1),
            vec![
                ("id".into(), 26),
                ("arguments_def".into(), 25),
                ("result_def".into(), 25),
                ("sqlbody_def".into(), 25),
                ("source_text".into(), 25)
            ]
        );
        assert!(
            result.iter().all(|m| m.0 != b'D'),
            "no user routines; internal routines excluded: {result:?}"
        );
    }
    let result = query(
        &mut socket,
        "SELECT p.proname,p.prokind,l.lanname,pg_get_function_arguments(p.oid),pg_get_function_result(p.oid),pg_get_function_sqlbody(p.oid),p.prosrc FROM pg_proc p JOIN pg_language l ON p.prolang=l.oid WHERE p.pronamespace=11 ORDER BY p.proname",
    );
    assert_eq!(result[0].0, b'T', "{result:?}");
    let data = result
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| m.1.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        data,
        [
            "pg_get_function_arguments",
            "pg_get_function_result",
            "pg_get_function_sqlbody"
        ]
        .map(|name| row(&[
            Some(name),
            Some("f"),
            Some("internal"),
            Some("function_oid oid"),
            Some("text"),
            None,
            Some(name)
        ]))
    );
    let result = query(
        &mut socket,
        "SELECT pg_get_function_arguments(NULL),pg_get_function_result(NULL::oid),pg_get_function_sqlbody(NULL) FROM pg_language",
    );
    assert_eq!(result[1], (b'D', row(&[None, None, None])));
    for sql in [
        "SELECT pg_get_function_arguments(0::oid) FROM pg_language",
        "SELECT pg_get_function_result(4294967295::oid) FROM pg_language",
        "SELECT pg_get_function_sqlbody('bad') FROM pg_language",
        "SELECT pg_get_function_arguments() FROM pg_language",
        "SELECT pg_get_function_result(oid,oid) FROM pg_proc",
        "SELECT unsupported_user_function(oid) FROM pg_proc",
        "SELECT oid FROM pg_proc WHERE unsupported_user_function(oid) IS NOT NULL AND pronamespace=0",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'E', "{sql}: {result:?}");
        assert!(
            result[0].1.windows(7).any(|w| w == b"C0A000\0"),
            "{sql}: {result:?}"
        );
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    }
    // The exact frozen JDBC SQL uses ?, while PG Parse binds $1 with OID 26.
    let sql = source_sql.replace("?", "$1");
    send(
        &mut socket,
        b'P',
        &[
            b"functions\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &1u16.to_be_bytes(),
            &26u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Sfunctions\0");
    send(&mut socket, b'S', b"");
    let described = until_ready(&mut socket);
    assert!(described.iter().all(|m| m.0 != b'E'), "{described:?}");
    assert!(described.iter().any(|m|m.0==b't' && m.1==[&1u16.to_be_bytes()[..],&26u32.to_be_bytes()].concat()),"{described:?}");
    for value in [Some(namespace.as_str()), None, Some("11")] {
        let parameter = match value {
            Some(value) => [&(value.len() as i32).to_be_bytes()[..], value.as_bytes()].concat(),
            None => (-1i32).to_be_bytes().to_vec(),
        };
        send(
            &mut socket,
            b'B',
            &[
                b"\0functions\0".as_slice(),
                &0u16.to_be_bytes(),
                &1u16.to_be_bytes(),
                &parameter,
                &0u16.to_be_bytes(),
            ]
            .concat(),
        );
        send(
            &mut socket,
            b'E',
            &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
        );
        send(&mut socket, b'S', b"");
        let result = until_ready(&mut socket);
        assert!(
            result.iter().all(|m| m.0 != b'E' && m.0 != b'D'),
            "{result:?}"
        );
        assert!(result.iter().any(|m| m.0 == b'C'), "{result:?}");
    }
    send(&mut socket, b'X', b"");
    service.close();
}

#[test]
fn pg_introspection_dependencies_live() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE SEQUENCE test.dependency_seq START WITH 7")
        .unwrap();
    native.execute("CREATE TABLE test.dependency_auto (id BIGINT PRIMARY KEY AUTO_INCREMENT, source BIGINT DEFAULT 7)").unwrap();
    // The native default builder cannot represent a durable sequence reference.
    let error = native
        .execute(
            "CREATE TABLE test.dependency_reference (id BIGINT DEFAULT (NEXTVAL(dependency_seq)))",
        )
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("expression form is not valid in CREATE TABLE metadata"),
        "{error}"
    );
    let (_, metadata) = domain.stats_table("test", "dependency_auto").unwrap();
    assert!(metadata.Sequence.is_none());
    assert!(!metadata.Columns[1].DefaultIsExpr);
    assert!(metadata.Columns[1].GetDefaultValue().is_some());
    let (_, sequence) = domain.stats_table("test", "dependency_seq").unwrap();
    assert_eq!(sequence.Sequence.as_ref().unwrap().Start, 7);
    native.execute("CREATE DATABASE dependency_other").unwrap();
    native
        .execute("CREATE SEQUENCE dependency_other.hidden_seq")
        .unwrap();
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
    let namespace = query(
        &mut socket,
        "SELECT oid FROM pg_namespace WHERE nspname='public'",
    );
    assert_eq!(namespace[1].0, b'D', "{namespace:?}");
    let length = i32::from_be_bytes(namespace[1].1[2..6].try_into().unwrap()) as usize;
    let namespace = std::str::from_utf8(&namespace[1].1[6..6 + length])
        .unwrap()
        .to_owned();
    // Frozen original statement 1869279760: both class guards are essential.
    let source_sql = r#"select D.objid as dependent_id,
       D.refobjid as owner_id,
       D.refobjsubid as owner_subobject_id
from pg_depend D
  join pg_class C_SEQ on D.objid    = C_SEQ.oid and D.classid    = 'pg_class'::regclass::oid
  join pg_class C_TAB on D.refobjid = C_TAB.oid and D.refclassid = 'pg_class'::regclass::oid
where C_SEQ.relkind = 'S'
  and C_TAB.relkind = 'r'
  and D.refobjsubid <> 0
  and (D.deptype = 'a' or D.deptype = 'i')
  and C_TAB.relnamespace = ?::oid
order by owner_id
"#;
    let result = query(&mut socket, &source_sql.replace("?", &namespace));
    assert_eq!(
        result[0].0, b'T',
        "complete sequence dependency query: {result:?}"
    );
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("dependent_id".into(), 26),
            ("owner_id".into(), 26),
            ("owner_subobject_id".into(), 23)
        ]
    );
    assert!(
        result.iter().all(|m| m.0 != b'D'),
        "native sequences have no column ownership: {result:?}"
    );
    // Actual sequences are S; AUTO_INCREMENT and default expressions do not create more sequences.
    let result = query(
        &mut socket,
        "SELECT relname,relkind FROM pg_class WHERE relname IN ('dependency_seq','dependency_auto','hidden_seq') ORDER BY relname",
    );
    assert_eq!(
        result
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        vec![
            row(&[Some("dependency_auto"), Some("r")]),
            row(&[Some("dependency_seq"), Some("S")])
        ]
    );
    // Persistent schema membership is the representable normal dependency.
    let membership_sql = "SELECT C.relname,N.nspname,D.classid,D.refclassid,D.refobjsubid,D.deptype FROM pg_depend D JOIN pg_class C ON D.objid=C.oid AND D.classid='pg_class'::regclass::oid JOIN pg_namespace N ON D.refobjid=N.oid AND D.refclassid='pg_namespace'::regclass::oid ORDER BY C.relname";
    let result = query(&mut socket, membership_sql);
    assert_eq!(result[0].0, b'T', "{result:?}");
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("relname".into(), 25),
            ("nspname".into(), 25),
            ("classid".into(), 26),
            ("refclassid".into(), 26),
            ("refobjsubid".into(), 23),
            ("deptype".into(), 18)
        ]
    );
    assert_eq!(
        result
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        vec![row(&[
            Some("dependency_seq"),
            Some("public"),
            Some("1259"),
            Some("2615"),
            Some("0"),
            Some("n")
        ])]
    );
    for sql in [
        "SELECT objid FROM pg_depend WHERE refobjid=NULL::oid OR deptype=NULL",
        "SELECT objid FROM pg_depend WHERE refobjid=4294967295::oid",
        "SELECT D.objid FROM pg_depend D JOIN pg_class C ON D.refobjid=C.oid AND D.refclassid='pg_class'::regclass::oid",
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'T', "{sql}: {result:?}");
        assert!(result.iter().all(|m| m.0 != b'D'), "{sql}: {result:?}");
    }
    for (sql, state) in [
        ("SELECT missing_field FROM pg_depend WHERE objid=0", "0A000"),
        (
            "SELECT objid FROM pg_depend WHERE unsupported_dependency(objid) IS NOT NULL",
            "0A000",
        ),
        ("DELETE FROM pg_depend", "0A000"),
    ] {
        let result = query(&mut socket, sql);
        assert_eq!(result[0].0, b'E', "{sql}: {result:?}");
        assert!(
            result[0]
                .1
                .windows(7)
                .any(|w| w == format!("C{state}\0").as_bytes()),
            "{sql}: {result:?}"
        );
        assert_eq!(query(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
    }
    // Exercise the original multi-JOIN statement with explicit OID parameters and NULL.
    let sql = source_sql.replace("?", "$1");
    send(
        &mut socket,
        b'P',
        &[
            b"dependencies\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &1u16.to_be_bytes(),
            &26u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Sdependencies\0");
    send(&mut socket, b'S', b"");
    let described = until_ready(&mut socket);
    assert!(described.iter().all(|m| m.0 != b'E'), "{described:?}");
    assert!(described.iter().any(|m|m.0==b't' && m.1==[&1u16.to_be_bytes()[..],&26u32.to_be_bytes()].concat()),"{described:?}");
    for value in [Some(namespace.as_str()), None, Some("4294967295")] {
        let parameter = match value {
            Some(value) => [&(value.len() as i32).to_be_bytes()[..], value.as_bytes()].concat(),
            None => (-1i32).to_be_bytes().to_vec(),
        };
        send(
            &mut socket,
            b'B',
            &[
                b"\0dependencies\0".as_slice(),
                &0u16.to_be_bytes(),
                &1u16.to_be_bytes(),
                &parameter,
                &0u16.to_be_bytes(),
            ]
            .concat(),
        );
        send(
            &mut socket,
            b'E',
            &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
        );
        send(&mut socket, b'S', b"");
        let result = until_ready(&mut socket);
        assert!(
            result.iter().all(|m| m.0 != b'E' && m.0 != b'D'),
            "{result:?}"
        );
        assert!(result.iter().any(|m| m.0 == b'C'), "{result:?}");
    }
    // Parse freezes syntax and metadata, not rows. A later replacement must
    // replace the original sequence in this nonempty dependency join.
    send(
        &mut socket,
        b'P',
        &[
            b"membership\0".as_slice(),
            membership_sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', b"");
    let parsed = until_ready(&mut socket);
    assert!(parsed.iter().all(|m| m.0 != b'E'), "{parsed:?}");
    // Independent native sequences can be dropped while tables remain:
    // neither AUTO_INCREMENT nor a default expression establishes ownership.
    native.execute("DROP SEQUENCE test.dependency_seq").unwrap();
    let result = query(&mut socket, membership_sql);
    assert_eq!(result[0].0, b'T', "{result:?}");
    assert!(
        result.iter().all(|m| m.0 != b'D'),
        "DROP must remove membership: {result:?}"
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT relname FROM pg_class WHERE relname='dependency_auto'"
        )[1],
        (b'D', row(&[Some("dependency_auto")]))
    );
    native
        .execute("CREATE SEQUENCE test.replacement_seq")
        .unwrap();
    let result = query(&mut socket, membership_sql);
    assert_eq!(
        result[1],
        (
            b'D',
            row(&[
                Some("replacement_seq"),
                Some("public"),
                Some("1259"),
                Some("2615"),
                Some("0"),
                Some("n")
            ])
        )
    );
    send(
        &mut socket,
        b'B',
        &[
            b"\0membership\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'E',
        &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let result = until_ready(&mut socket);
    assert!(result.iter().all(|m| m.0 != b'E'), "{result:?}");
    assert_eq!(
        result
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        vec![row(&[
            Some("replacement_seq"),
            Some("public"),
            Some("1259"),
            Some("2615"),
            Some("0"),
            Some("n")
        ])]
    );
    send(&mut socket, b'X', b"");
    service.close();
}
