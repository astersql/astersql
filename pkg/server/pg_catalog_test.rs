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
            "relkind IN (SELECT relkind FROM pg_catalog.pg_class)".to_string(),
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
