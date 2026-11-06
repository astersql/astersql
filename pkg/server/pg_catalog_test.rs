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
            ("Database Name".into(), 19),
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
        vec![("Latest Name".into(), 19), ("id".into(), 20)]
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

    assert_eq!(
        query(&mut socket, "CREATE DATABASE IF NOT EXISTS test")[0].0,
        b'C'
    );
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
            ("datname".into(), 19),
            ("description".into(), 25),
            ("datistemplate".into(), 16),
            ("datallowconn".into(), 16),
            ("owner".into(), 19),
            ("db".into(), 19),
            ("current_catalog".into(), 19)
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
                Some("root"),
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
            ("objoid".into(), 26),
            ("classoid".into(), 26),
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
        "SELECT oid, relname, relnamespace, relkind, NULL AS absent FROM pg_catalog.pg_class WHERE relname NOT IN ('relations_partitioned','relations_partition_idx') ORDER BY oid",
    );
    assert_eq!(
        result[0].0, b'T',
        "pg_class must provide real relations: {result:?}"
    );
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("oid".into(), 26),
            ("relname".into(), 19),
            ("relnamespace".into(), 26),
            ("relkind".into(), 18),
            ("absent".into(), 25)
        ]
    );
    for name in ["relations_partitioned", "relations_partition_idx"] {
        let denied = query(
            &mut socket,
            &format!("SELECT relkind FROM pg_catalog.pg_class WHERE relname='{name}'"),
        );
        assert_eq!(denied[0].0, b'E', "{denied:?}");
        assert!(denied[0].1.windows(5).any(|s| s == b"0A000"));
        let identity = query(
            &mut socket,
            &format!("SELECT oid,relname FROM pg_catalog.pg_class WHERE relname='{name}'"),
        );
        assert_eq!(identity.iter().filter(|m| m.0 == b'D').count(), 1);
    }
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
        if name == "relations_live" {
            let (index_name, index_kind) = ("relations_idx", "i");
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
    let xmin = query(&mut socket, "SELECT xmin FROM pg_catalog.pg_class");
    assert_eq!(xmin[0].0, b'E', "{xmin:?}");
    assert!(xmin[0].1.windows(5).any(|s| s == b"0A000"), "{xmin:?}");
    let unsupported_join = query(
        &mut socket,
        "SELECT C.relname FROM pg_catalog.pg_class C JOIN public.relations_seq B ON C.oid = B.id",
    );
    assert_eq!(unsupported_join[0].0, b'E', "{unsupported_join:?}");
    assert!(
        unsupported_join[0]
            .1
            .windows(5)
            .any(|window| window == b"0A000"),
        "{unsupported_join:?}"
    );
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
        (b'D', row(&[Some(&shadow_id), Some("pg_class")]))
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
        vec![("oid".into(), 26), ("relname".into(), 19)]
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
            ("attname".into(), 19),
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
    let unavailable_owner = query(&mut socket, &types_template.replace("$1", "11"));
    assert!(
        unavailable_owner
            .iter()
            .any(|m| m.0 == b'E' && m.1.windows(7).any(|w| w == b"C0A000\0")),
        "type owner facts are unavailable, not SQL NULL: {unavailable_owner:?}"
    );
    // Retain sorted nonempty builtin and empty user-namespace coverage without
    // claiming that the complete owner-bearing client template is supported.
    let supported_types = types_template.replace(
        ",\n       pg_catalog.pg_get_userbyid(T.typowner) as \"owner\"",
        "",
    );
    let types = query(&mut socket, &supported_types.replace("$1", "11"));
    assert_eq!(
        types[0].0, b'T',
        "supported builtin identity projection: {types:?}"
    );
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
        &supported_types.replace("$1", &namespace.to_string()),
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
                Some("6"),
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
    assert_eq!(after.iter().filter(|(tag, _)| *tag == b'D').count(), 6);
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
    let builtins = query(&mut socket, "SELECT typname FROM pg_type");
    assert_eq!(
        builtins[0].0, b'T',
        "static builtin identities are independent of native enums: {builtins:?}"
    );
    assert!(builtins.contains(&(b'D', row(&[Some("int4")]))));
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
    let (domain, native) =
        astersql_session::runtime::CreateUnbootstrappedAnalyzeSessionForTest().unwrap();
    native.execute("CREATE DATABASE test").unwrap();
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
            .any(|m| m.0 == b'D' && m.1 == row(&[Some("\"b\""), Some(""), Some("")]))
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
    let (domain, native) =
        astersql_session::runtime::CreateUnbootstrappedAnalyzeSessionForTest().unwrap();
    native.execute("CREATE DATABASE test").unwrap();
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
            ("relname".into(), 19),
            ("nspname".into(), 19),
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

#[test]
fn identity_catalog_column_origins() {
    let metadata = crate::pg_catalog::CatalogQuery::parse(
        "SELECT attidentity,attnotnull,atthasdef,atttypid FROM pg_attribute",
    )
    .unwrap()
    .unwrap()
    .metadata();
    assert_eq!(
        metadata.catalog_column_origins,
        vec![
            Some((1249, 15)),
            Some((1249, 12)),
            Some((1249, 13)),
            Some((1249, 3))
        ]
    );
    let metadata = crate::pg_catalog::CatalogQuery::parse("SELECT adnum FROM pg_attrdef")
        .unwrap()
        .unwrap()
        .metadata();
    assert_eq!(metadata.catalog_column_origins, vec![Some((2604, 3))]);
}

#[test]
fn identity_owned_catalog_column_origins() {
    for (sql, expected) in [
        (
            "SELECT seqstart,seqincrement,seqmin,seqmax,seqcache,seqcycle FROM pg_sequence",
            vec![
                (2224, 3),
                (2224, 4),
                (2224, 6),
                (2224, 5),
                (2224, 7),
                (2224, 8),
            ],
        ),
        (
            "SELECT deptype,refobjsubid FROM pg_depend",
            vec![(2608, 7), (2608, 6)],
        ),
        (
            "SELECT oid,relname,relnamespace,relkind FROM pg_class",
            vec![(1259, 1), (1259, 2), (1259, 3), (1259, 18)],
        ),
    ] {
        let metadata = crate::pg_catalog::CatalogQuery::parse(sql)
            .unwrap()
            .unwrap()
            .metadata();
        assert_eq!(
            metadata.catalog_column_origins,
            expected.into_iter().map(Some).collect::<Vec<_>>()
        );
    }
}

#[test]
fn pg12_ddl_visibility_catalog() {
    use crate::conn::{CancellationToken, SessionDriver, TiDBContext, Value};
    fn directory(
        ctx: &dyn TiDBContext,
        observations: &mut Vec<serde_json::Value>,
    ) -> Vec<Vec<Value>> {
        let sql = "SELECT n.nspname,c.relname,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='txddl_probe'";
        let result = crate::pg_catalog::CatalogQuery::parse(sql)
            .unwrap()
            .unwrap()
            .execute(ctx, &CancellationToken::new())
            .unwrap();
        eprintln!("real catalog SQL={sql} result={result:?}");
        let values = result
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|value| match value {
                        Value::Text(text) => Some(text.clone()),
                        Value::Null => None,
                        other => panic!("unexpected directory type: {other:?}"),
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        observations.push(
            serde_json::json!({"sql":sql,"rows":values,"native_result":format!("{result:?}")}),
        );
        result.rows
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let database = format!("pg12_ddl_visibility_catalog_{}", std::process::id());
    assert!(
        std::process::Command::new("python3")
            .arg(root.join("pkg/server/testdata/pg12_transaction_libpq.py"))
            .args([
                "--case",
                "ddl_visibility_reference",
                "--host",
                "127.0.0.1",
                "--port",
                "0",
                "--database",
                &database
            ])
            .status()
            .unwrap()
            .success(),
        "PG18.6 reference required"
    );
    let evidence = root
        .join("target/pg12-ddl_visibility_reference-evidence")
        .join(database);
    let reference: serde_json::Value =
        serde_json::from_slice(&std::fs::read(evidence.join("reference.json")).unwrap()).unwrap();
    let mut observations = Vec::new();
    let (domain, _bootstrap) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = ConcreteSessionDriver::new_for_test(domain, BootstrapAuthMode::InsecureRootOnly);
    let a = driver
        .open_ctx_with_dialect(
            98200,
            0,
            45,
            "test",
            None,
            astersql_parser_ast::SqlDialect::PostgreSQL,
        )
        .unwrap();
    let b = driver
        .open_ctx_with_dialect(
            98201,
            0,
            45,
            "test",
            None,
            astersql_parser_ast::SqlDialect::PostgreSQL,
        )
        .unwrap();
    let token = CancellationToken::new();
    let expected = vec![vec![
        Value::Text("public".into()),
        Value::Text("txddl_probe".into()),
        Value::Text("r".into()),
    ]];
    for (index, exit) in ["ROLLBACK", "COMMIT"].iter().enumerate() {
        assert!(directory(a.as_ref(), &mut observations).is_empty());
        assert!(directory(b.as_ref(), &mut observations).is_empty());
        // Deliberately enter through the internal shared session, keeping the
        // public PgService transaction-DDL rejection intact.
        for sql in [
            "BEGIN",
            "CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)",
            "INSERT INTO txddl_probe VALUES (1,20)",
        ] {
            a.execute_query(sql, false, &token).unwrap();
        }
        assert_eq!(directory(a.as_ref(), &mut observations), expected);
        assert_eq!(
            observations.last().unwrap()["rows"],
            reference["cases"][index]["before_a"]["directory"]
        );
        assert!(directory(b.as_ref(), &mut observations).is_empty());
        assert_eq!(
            observations.last().unwrap()["rows"],
            reference["cases"][index]["before_b"]["directory"]
        );
        a.execute_query(exit, false, &token).unwrap();
        for (i, ctx) in [&a, &b].iter().enumerate() {
            assert_eq!(
                directory(ctx.as_ref(), &mut observations),
                if *exit == "COMMIT" {
                    expected.clone()
                } else {
                    vec![]
                }
            );
            assert_eq!(
                observations.last().unwrap()["rows"],
                reference["cases"][index]["after"][i]["directory"]
            );
        }
        if *exit == "ROLLBACK" {
            a.execute_query(
                "CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)",
                false,
                &token,
            )
            .unwrap();
        }
        a.execute_query("DROP TABLE txddl_probe", false, &token)
            .unwrap();
        assert!(directory(b.as_ref(), &mut observations).is_empty());
    }
    a.close().unwrap();
    b.close().unwrap();
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    std::fs::write(
        evidence.join("aster-catalog.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "repository_sha":String::from_utf8_lossy(&sha.stdout).trim(),
            "observations":observations,"client":"real CatalogQuery over worker-backed TiDBContext",
            "cleanup":"connections closed; probe absent in both rounds",
            "wire":"internal provider; public gate separately tested"
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn pg12_ddl_failure_cleanup_catalog() {
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _bootstrap) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        driver.clone(),
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    struct CloseService(Arc<PgService>);
    impl Drop for CloseService {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close_service = CloseService(service.clone());
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
    until_ready(&mut socket);
    let directory = "SELECT n.nspname,c.relname,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname='txddl_probe'";
    let token = CancellationToken::new();
    for (index, close) in [false, true].iter().enumerate() {
        let a = driver
            .open_ctx_with_dialect(
                98300 + index as u64,
                0,
                45,
                "test",
                None,
                astersql_parser_ast::SqlDialect::PostgreSQL,
            )
            .unwrap();
        for sql in [
            "CREATE TABLE txddl_sentinel (id integer PRIMARY KEY,v integer NOT NULL)",
            "INSERT INTO txddl_sentinel VALUES (1,10)",
            "BEGIN",
            "UPDATE txddl_sentinel SET v=11 WHERE id=1",
            "CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)",
            "INSERT INTO txddl_probe VALUES (1,30)",
        ] {
            a.execute_query(sql, false, &token).unwrap();
        }
        assert!(
            a.execute_query("INSERT INTO txddl_probe VALUES (1,40)", false, &token)
                .is_err()
        );
        // The writer enters only through the internal worker, preserving the
        // public PG DDL gate; the observer uses a real TCP PostgreSQL session.
        for sql in [directory] {
            let messages = query(&mut socket, sql);
            eprintln!("cleanup close={close} before SQL={sql} wire={messages:?}");
            assert!(!messages.iter().any(|m| matches!(m.0, b'D' | b'E')));
            assert_eq!(messages.last().unwrap(), &(b'Z', vec![b'I']));
        }
        let messages = query(
            &mut socket,
            "SELECT id,v FROM public.txddl_sentinel ORDER BY id",
        );
        assert!(messages.contains(&(b'D', row(&[Some("1"), Some("10")]))));
        if !close {
            a.execute_query("ROLLBACK", false, &token).unwrap();
        }
        a.close().unwrap(); // Shutdown joins the worker and drops its actual session.
        for sql in [directory] {
            let messages = query(&mut socket, sql);
            eprintln!("cleanup close={close} after SQL={sql} wire={messages:?}");
            assert!(!messages.iter().any(|m| matches!(m.0, b'D' | b'E')));
        }
        let recovered = query(
            &mut socket,
            "SELECT id,v FROM public.txddl_sentinel ORDER BY id",
        );
        assert_eq!(
            recovered.iter().filter(|m| m.0 == b'D').collect::<Vec<_>>(),
            vec![&(b'D', row(&[Some("1"), Some("10")]))]
        );
        let missing = query(
            &mut socket,
            "SELECT id,v FROM public.txddl_probe ORDER BY id",
        );
        assert!(
            missing
                .iter()
                .any(|m| m.0 == b'E' && m.1.windows(6).any(|v| v == b"42P01\0"))
        );
        assert_eq!(missing.last().unwrap(), &(b'Z', vec![b'I']));
        let b = driver
            .open_ctx_with_dialect(
                98310 + index as u64,
                0,
                45,
                "test",
                None,
                astersql_parser_ast::SqlDialect::PostgreSQL,
            )
            .unwrap();
        for sql in [
            "CREATE TABLE txddl_probe (id integer PRIMARY KEY,v integer NOT NULL)",
            "INSERT INTO txddl_probe VALUES (2,50)",
            "UPDATE txddl_sentinel SET v=12 WHERE id=1",
        ] {
            b.execute_query(sql, false, &token).unwrap();
        }
        for (sql, expected) in [
            (
                "SELECT id,v FROM public.txddl_probe ORDER BY id",
                row(&[Some("2"), Some("50")]),
            ),
            (
                "SELECT id,v FROM public.txddl_sentinel ORDER BY id",
                row(&[Some("1"), Some("12")]),
            ),
        ] {
            let messages = query(&mut socket, sql);
            eprintln!("cleanup close={close} reuse SQL={sql} wire={messages:?}");
            assert_eq!(
                messages.iter().filter(|m| m.0 == b'D').collect::<Vec<_>>(),
                vec![&(b'D', expected)]
            );
        }
        for sql in ["DROP TABLE txddl_probe", "DROP TABLE txddl_sentinel"] {
            b.execute_query(sql, false, &token).unwrap();
        }
        b.close().unwrap();
        assert!(
            !query(&mut socket, directory)
                .iter()
                .any(|m| matches!(m.0, b'D' | b'E'))
        );
    }
    send(&mut socket, b'X', &[]);
    drop(socket);
    service.close();
    assert!(
        TcpListener::bind(addr).is_ok(),
        "owned listener must be released"
    );
}

#[test]
fn pg13_001_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_001_legacy").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_001_legacy")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_identity_user'@'%'")
            .unwrap();
    });
    let connect = || {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let body = [
            196608u32.to_be_bytes().as_slice(),
            b"user\0root\0database\0pg13_001_legacy\0\0",
        ]
        .concat();
        socket
            .write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&body).unwrap();
        while read(&mut socket).0 != b'Z' {}
        socket
    };
    let mut a = connect();
    let mut b = connect();
    let unmigrated = query(
        &mut a,
        "SELECT datdba FROM pg_catalog.pg_database WHERE datname = 'pg13_001_legacy'",
    );
    assert_eq!(
        unmigrated.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"EZ",
        "{unmigrated:?}"
    );
    assert!(unmigrated[0].1.windows(5).any(|w| w == b"0A000"));
    let names = query(
        &mut a,
        "SELECT oid, datname FROM pg_catalog.pg_database WHERE datname = 'pg13_001_legacy'",
    );
    assert_eq!(names.iter().map(|m| m.0).collect::<Vec<_>>(), b"TDCZ");
    let unmatched = query(
        &mut a,
        "SELECT D.datdba FROM pg_catalog.pg_database N LEFT JOIN pg_catalog.pg_database D ON D.datname = 'pg13_001_absent' WHERE N.datname = 'pg13_001_legacy'",
    );
    assert_eq!(
        unmatched.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ",
        "{unmatched:?}"
    );
    assert_eq!(unmatched[1], (b'D', row(&[None])));
    // Explicit admin migration, never an inferred root owner for a legacy DB.
    let migrated = query(&mut a, "ALTER DATABASE pg13_001_legacy OWNER TO root");
    assert_eq!(
        migrated.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"CZ",
        "{migrated:?}"
    );
    let sql = "SELECT oid, datname, datdba, datistemplate, datallowconn FROM pg_catalog.pg_database WHERE datname = 'pg13_001_legacy'";
    let result = query(&mut a, sql);
    assert_eq!(
        result.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"TDCZ",
        "{result:?}"
    );
    assert_eq!(
        columns(&result[0].1),
        vec![
            ("oid".into(), 26),
            ("datname".into(), 19),
            ("datdba".into(), 26),
            ("datistemplate".into(), 16),
            ("datallowconn".into(), 16)
        ]
    );
    let schema = domain
        .info_schema()
        .AllSchemas()
        .into_iter()
        .find(|s| s.name.original == "pg13_001_legacy")
        .unwrap();
    let fact = native
        .pg_database_facts(None)
        .unwrap()
        .into_iter()
        .find(|f| f.id == schema.id)
        .unwrap();
    let (owner_oid, owner_name) = fact.owner.unwrap();
    assert_eq!(owner_name, "root");
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
                Some("pg13_001_legacy"),
                Some(&owner_oid.to_string()),
                Some("f"),
                Some("t")
            ])
        )
    );
    domain.force_full_reload_for_test().unwrap();
    assert_eq!(
        query(
            &mut a,
            "SELECT pg_get_userbyid(datdba) FROM pg_catalog.pg_database WHERE datname = 'pg13_001_legacy'"
        )[1],
        (b'D', row(&[Some("root")]))
    );
    assert_eq!(
        query(&mut b, sql),
        result,
        "cross connection identities differ"
    );
    for suffix in ["WHERE datname = 'pg13_001_absent'", "LIMIT 0"] {
        let empty = query(
            &mut a,
            &format!(
                "SELECT oid, datname, datdba, datistemplate, datallowconn FROM pg_catalog.pg_database {suffix}"
            ),
        );
        assert_eq!(
            empty.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"TCZ",
            "{empty:?}"
        );
        assert_eq!(empty[0], result[0]);
        assert_eq!(empty[1], (b'C', b"SELECT 0\0".to_vec()));
        assert_eq!(empty[2], (b'Z', b"I".to_vec()));
    }
    assert_eq!(
        query(&mut a, "BEGIN").last().unwrap(),
        &(b'Z', b"T".to_vec())
    );
    assert_eq!(query(&mut a, sql).last().unwrap(), &(b'Z', b"T".to_vec()));
    let failed = query(&mut a, "SELECT unknown FROM pg_catalog.pg_database");
    assert_eq!(failed.last().unwrap(), &(b'Z', b"E".to_vec()));
    let aborted = query(&mut a, sql);
    assert!(aborted[0].1.windows(5).any(|w| w == b"25P02"));
    assert_eq!(
        query(&mut a, "ROLLBACK").last().unwrap(),
        &(b'Z', b"I".to_vec())
    );
    assert_eq!(query(&mut a, sql), result);
    native
        .execute("CREATE USER 'pg13_identity_user'@'%' IDENTIFIED WITH mysql_native_password BY ''")
        .unwrap();
    let first_role = native.pg_database_role_id("pg13_identity_user").unwrap();
    assert_eq!(
        query(
            &mut a,
            "ALTER DATABASE pg13_001_legacy OWNER TO pg13_identity_user"
        )[0]
        .0,
        b'C'
    );
    assert!(
        native
            .execute("DROP USER 'pg13_identity_user'@'%'")
            .is_err(),
        "dangling database owner allowed"
    );
    assert_eq!(
        query(&mut a, "ALTER DATABASE pg13_001_legacy OWNER TO root")[0].0,
        b'C'
    );
    native
        .execute("DROP USER 'pg13_identity_user'@'%'")
        .unwrap();
    native
        .execute("CREATE USER 'pg13_identity_user'@'%' IDENTIFIED WITH mysql_native_password BY ''")
        .unwrap();
    assert_ne!(
        native.pg_database_role_id("pg13_identity_user").unwrap(),
        first_role,
        "recreated role reused identity"
    );
    native
        .execute("DROP USER 'pg13_identity_user'@'%'")
        .unwrap();
    drop(a);
    drop(b);
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err(), "listener leaked");
    println!(
        "pg13_001_contract: BASE=1 EMPTY=2 cross_connection=1 reload=1 migration=2 role_recreation=1 transaction_recovery=1 cleanup=1"
    );
}

#[test]
fn pg13_002_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_002_policy").unwrap();
    native.execute("CREATE DATABASE pg13_002_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_002_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_002_admin")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_002_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_002_admin");
    let sql = "SELECT datistemplate, datallowconn FROM pg_catalog.pg_database WHERE datname = 'pg13_002_policy'";
    let initial = query(&mut a, sql);
    assert_eq!(initial[1], (b'D', row(&[Some("f"), Some("t")])));
    assert_eq!(
        columns(&initial[0].1),
        vec![("datistemplate".into(), 16), ("datallowconn".into(), 16)]
    );
    for (command, expected) in [
        ("ALLOW_CONNECTIONS false", ["f", "f"]),
        ("IS_TEMPLATE true", ["t", "f"]),
        ("ALLOW_CONNECTIONS true", ["t", "t"]),
        ("IS_TEMPLATE false", ["f", "t"]),
    ] {
        let out = query(&mut b, &format!("ALTER DATABASE pg13_002_policy {command}"));
        assert_eq!(out[0].0, b'C', "policy update must succeed: {out:?}");
        assert_eq!(
            query(&mut a, sql)[1],
            (b'D', row(&[Some(expected[0]), Some(expected[1])]))
        );
        let (fresh, messages) = connect("pg13_002_policy");
        if expected[1] == "f" {
            assert_eq!(messages[0].0, b'E', "global switch must reject even root");
            assert!(messages[0].1.windows(6).any(|w| w == b"55000\0"));
        } else {
            assert_eq!(messages.last().unwrap().0, b'Z');
        }
        drop(fresh);
    }
    let invalid = query(
        &mut b,
        "ALTER DATABASE pg13_002_policy ALLOW_CONNECTIONS nonsense",
    );
    assert_eq!(invalid[0].0, b'E');
    assert_eq!(query(&mut a, sql), initial);
    let empty = query(&mut a, &format!("{sql} LIMIT 0"));
    assert_eq!(empty[0], initial[0]);
    assert_eq!(empty[1], (b'C', b"SELECT 0\0".to_vec()));
    assert_eq!(
        query(&mut b, "SELECT 1 AS ok")[1],
        (b'D', row(&[Some("1")]))
    );
    // Remove only a persisted model fact; oid/name remain readable and no read repairs it.
    let schema = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("pg13_002_policy"))
        .unwrap();
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&astersql_kv::Context::default(), store, true, |_, txn| {
                txn.Delete(astersql_meta::transaction_meta_hash_key(
                    b"PgDatabasePolicy",
                    schema.id.to_string().as_bytes(),
                ))
            })
        })
        .unwrap();
    let missing = query(&mut a, sql);
    assert_eq!(missing.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(missing[0].1.windows(6).any(|w| w == b"0A000\0"));
    assert_eq!(
        query(
            &mut a,
            "SELECT oid, datname FROM pg_catalog.pg_database WHERE datname = 'pg13_002_policy'"
        )[1]
        .0,
        b'D'
    );
    // Updating one legacy fact must not silently invent its sibling.
    assert_eq!(
        query(&mut b, "ALTER DATABASE pg13_002_policy IS_TEMPLATE false")[0].0,
        b'C'
    );
    assert_eq!(
        query(
            &mut a,
            "SELECT datistemplate FROM pg_catalog.pg_database WHERE datname = 'pg13_002_policy'"
        )[1],
        (b'D', row(&[Some("f")]))
    );
    assert_eq!(query(&mut a, sql)[0].0, b'E');
    assert_eq!(
        query(
            &mut b,
            "ALTER DATABASE pg13_002_policy ALLOW_CONNECTIONS true"
        )[0]
        .0,
        b'C'
    );
    assert_eq!(query(&mut a, sql), initial);
    drop(a);
    drop(b);
    drop(cleanup);
    assert!(TcpListener::bind(addr).is_ok());
    println!(
        "pg13_002_contract: policy=4 cross_connection=4 startup=4 invalid=1 empty=1 recovery=1 missing_source=3 migration=2 cleanup=1"
    );
}

#[test]
fn pg13_003_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_003_policy").unwrap();
    native.execute("CREATE DATABASE pg13_003_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_003_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_003_admin")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_003_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_003_admin");
    let (mut c, _) = connect("pg13_003_policy");
    let sql = "SELECT oid,nspname FROM pg_catalog.pg_namespace WHERE nspname = 'ns_user'";
    assert_eq!(query(&mut a, sql).len(), 3);
    assert_eq!(query(&mut b, "CREATE SCHEMA ns_user")[0].0, b'C');
    let initial = query(&mut a, sql);
    assert_eq!(initial[0].0, b'T', "{initial:?}");
    assert_eq!(
        columns(&initial[0].1),
        vec![("oid".into(), 26), ("nspname".into(), 19)]
    );
    assert_eq!(initial.len(), 4, "{initial:?}");
    assert_eq!(
        query(&mut b, "ALTER SCHEMA ns_user RENAME TO ns_renamed")[0].0,
        b'C'
    );
    let old_oid = initial[1].1[2..].to_vec();
    assert_eq!(query(&mut c, sql).len(), 3);
    let renamed = query(
        &mut a,
        "SELECT oid,nspname FROM pg_catalog.pg_namespace WHERE nspname = 'ns_renamed'",
    );
    assert_eq!(renamed.len(), 4, "{renamed:?}");
    // Only the name changes; the first field retains the persistent OID.
    let width = i32::from_be_bytes(old_oid[..4].try_into().unwrap()) as usize;
    assert_eq!(&old_oid[..4 + width], &renamed[1].1[2..6 + width]);
    let empty = query(
        &mut a,
        "SELECT oid,nspname FROM pg_catalog.pg_namespace LIMIT 0",
    );
    assert_eq!(
        columns(&empty[0].1),
        vec![("oid".into(), 26), ("nspname".into(), 19)]
    );
    assert_eq!(empty[1], (b'C', b"SELECT 0\0".to_vec()));
    assert_eq!(query(&mut a, "SET search_path TO ns_renamed")[0].0, b'C');
    assert_eq!(
        query(
            &mut a,
            "SELECT nspname FROM pg_catalog.pg_namespace WHERE nspname='ns_renamed'"
        )[1],
        (b'D', row(&[Some("ns_renamed")]))
    );
    assert_eq!(query(&mut b, "DROP SCHEMA ns_renamed")[0].0, b'C');
    assert_eq!(query(&mut a, sql).len(), 3);
    drop(c);
    drop(a);
    drop(b);
    drop(cleanup);
    println!("pg13_003_contract: create=1 rename=1 drop=1 cross_connection=3 empty=2 cleanup=1");
}

#[test]
fn pg13_004_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_004_policy").unwrap();
    native.execute("CREATE DATABASE pg13_004_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_004_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_004_admin")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_004_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_004_admin");
    let (mut c, _) = connect("pg13_004_policy");
    let sql = "SELECT oid,nspowner,nspacl FROM pg_catalog.pg_namespace WHERE nspname = 'ns_acl'";
    assert_eq!(query(&mut b, "CREATE SCHEMA ns_acl")[0].0, b'C');
    let initial = query(&mut a, sql);
    assert_eq!(
        columns(&initial[0].1),
        vec![
            ("oid".into(), 26),
            ("nspowner".into(), 26),
            ("nspacl".into(), 1034)
        ]
    );
    assert_eq!(initial.len(), 4, "{initial:?}");
    assert!(initial[1].1.ends_with(&(-1i32).to_be_bytes()));
    assert_eq!(query(&mut c, sql).len(), 3);
    let empty = query(&mut a, &format!("{sql} LIMIT 0"));
    assert_eq!(columns(&empty[0].1), columns(&initial[0].1));
    assert_eq!(empty[1], (b'C', b"SELECT 0\0".to_vec()));
    // Corrupt only persisted metadata at its real model boundary, never provider rows.
    let db = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("pg13_004_admin"))
        .unwrap();
    let key =
        astersql_meta::transaction_meta_hash_key(b"PgNamespaces", db.id.to_string().as_bytes());
    let context = astersql_kv::Context::default();
    let mut original = Vec::new();
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&context, store, true, |_, tx| {
                original = tx.Get(&context, key.clone(), &[])?.Value;
                let mut document: serde_json::Value = serde_json::from_slice(&original).unwrap();
                document["ns_acl"]
                    .as_object_mut()
                    .unwrap()
                    .remove("explicit_acl");
                tx.Set(key.clone(), serde_json::to_vec(&document).unwrap())
            })
        })
        .unwrap();
    let missing = query(
        &mut a,
        "SELECT nspacl FROM pg_catalog.pg_namespace WHERE nspname='ns_acl'",
    );
    assert_eq!(missing[0].0, b'E', "{missing:?}");
    assert!(missing[0].1.windows(5).any(|w| w == b"0A000"));
    assert_eq!(missing.last().unwrap(), &(b'Z', vec![b'I']));
    assert_eq!(
        query(
            &mut a,
            "SELECT oid FROM pg_catalog.pg_namespace WHERE nspname='ns_acl'"
        )
        .len(),
        4
    );
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&context, store, true, |_, tx| {
                tx.Set(key.clone(), original.clone())
            })
        })
        .unwrap();
    assert_eq!(query(&mut a, sql), initial);
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&context, store, true, |_, tx| {
                let mut document: serde_json::Value = serde_json::from_slice(&original).unwrap();
                document["ns_acl"]["owner_id"] = serde_json::json!(2147483646);
                tx.Set(key.clone(), serde_json::to_vec(&document).unwrap())
            })
        })
        .unwrap();
    for field in ["nspowner", "nspacl"] {
        let missing = query(
            &mut a,
            &format!("SELECT {field} FROM pg_catalog.pg_namespace WHERE nspname='ns_acl'"),
        );
        assert_eq!(missing[0].0, b'E', "{missing:?}");
        assert!(missing[0].1.windows(5).any(|w| w == b"0A000"));
        assert_eq!(missing.last().unwrap(), &(b'Z', vec![b'I']));
    }
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&context, store, true, |_, tx| {
                tx.Set(key.clone(), original.clone())
            })
        })
        .unwrap();
    assert_eq!(query(&mut a, sql), initial);
    let invalid = query(
        &mut b,
        "GRANT USAGE ON SCHEMA ns_acl TO pg13_004_missing_role",
    );
    assert_eq!(invalid[0].0, b'E', "{invalid:?}");
    assert_eq!(query(&mut a, sql), initial);
    let revoked = query(&mut b, "REVOKE ALL ON SCHEMA ns_acl FROM root");
    assert_eq!(revoked[0].0, b'C', "{revoked:?}");
    let explicit = query(
        &mut a,
        "SELECT nspacl FROM pg_catalog.pg_namespace WHERE nspname='ns_acl'",
    );
    assert_eq!(explicit[1], (b'D', row(&[Some("{}")])), "{explicit:?}");
    assert_eq!(query(&mut b, "DROP SCHEMA ns_acl")[0].0, b'C');
    drop(c);
    drop(a);
    drop(b);
    drop(cleanup);
    println!(
        "pg13_004_contract: default_null=1 explicit_empty=1 cross_connection=2 empty_description=1 missing_source=3 invalid=1 recovery=3 cleanup=1"
    );
}

#[test]
fn pg13_005_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_005_policy").unwrap();
    native.execute("CREATE DATABASE pg13_005_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_005_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_005_admin")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_005_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_005_admin");
    let (mut c, _) = connect("pg13_005_policy");
    for sql in [
        "CREATE TABLE pg13_005_admin.rel_t(id integer, KEY rel_ix(id))",
        "CREATE VIEW pg13_005_admin.rel_v AS SELECT id FROM pg13_005_admin.rel_t",
        "CREATE SEQUENCE pg13_005_admin.rel_s START WITH 10",
    ] {
        native.execute(sql).unwrap();
    }
    let qr = "SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON c.relnamespace=n.oid WHERE n.nspname='public' AND c.relname IN ('rel_t','rel_ix','rel_v','rel_s','rel_renamed') ORDER BY c.relname";
    let initial = query(&mut a, qr);
    assert_eq!(initial[0].0, b'T', "{initial:?}");
    assert_eq!(
        columns(&initial[0].1),
        vec![
            ("oid".into(), 26),
            ("relname".into(), 19),
            ("relnamespace".into(), 26),
            ("relkind".into(), 18)
        ]
    );
    assert_eq!(
        initial.iter().filter(|m| m.0 == b'D').count(),
        4,
        "{initial:?}"
    );
    assert_eq!(query(&mut b, qr), initial);
    assert_eq!(query(&mut c, qr).iter().filter(|m| m.0 == b'D').count(), 0);
    for sql in [
        "SELECT i.indrelid=i_t.oid,i.indexrelid=x.oid FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class i_t ON i_t.oid=i.indrelid JOIN pg_catalog.pg_class x ON x.oid=i.indexrelid WHERE i_t.relname='rel_t' AND x.relname='rel_ix'",
        "SELECT s.seqrelid=c.oid FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid=s.seqrelid WHERE c.relname='rel_s'",
        "SELECT a.attrelid=c.oid FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid WHERE c.relname='rel_t' AND a.attname='id'",
    ] {
        let got = query(&mut a, sql);
        assert_eq!(
            got.iter().filter(|m| m.0 == b'D').count(),
            1,
            "{sql}: {got:?}"
        );
        assert!(!got.iter().any(|m| m.0 == b'E'), "{got:?}");
    }
    let ids = |messages: &Vec<(u8, Vec<u8>)>| {
        messages
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>()
    };
    let oid_sql = "SELECT oid FROM pg_catalog.pg_class WHERE relname IN ('rel_t','rel_renamed','rel_ix') ORDER BY oid";
    let before = ids(&query(&mut a, oid_sql));
    native
        .execute("RENAME TABLE pg13_005_admin.rel_t TO pg13_005_admin.rel_renamed")
        .unwrap();
    assert_eq!(ids(&query(&mut a, oid_sql)), before);
    assert_eq!(query(&mut a, qr), query(&mut b, qr));
    native.execute("DROP VIEW pg13_005_admin.rel_v").unwrap();
    native
        .execute("DROP TABLE pg13_005_admin.rel_renamed")
        .unwrap();
    assert!(ids(&query(&mut a, oid_sql)).is_empty());
    native
        .execute("CREATE TABLE pg13_005_admin.rel_t(id integer, KEY rel_ix(id))")
        .unwrap();
    let after = ids(&query(&mut a, oid_sql));
    assert_eq!(after.len(), 2);
    assert!(after.iter().all(|id| !before.contains(id)));
    assert_eq!(query(&mut a, qr), query(&mut b, qr));
    for id in [0, i64::MIN, i64::MAX] {
        assert!(crate::pg_oid::namespace_oid(id).is_err());
        assert!(crate::pg_oid::table_oid(id).is_err());
    }
    assert!(crate::pg_oid::index_oid(0, 1).is_err());
    assert!(crate::pg_oid::index_oid(1, i64::MAX).is_err());
    let pairs = [(42, 1), (43, 1), (42, -1)].map(|(t, i)| crate::pg_oid::index_oid(t, i).unwrap());
    assert_eq!(
        pairs
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    // Invalid canonical metadata is injected here, never precomputed catalog rows.
    let mut model = astersql_meta_model::TableInfo::default();
    model.ID = i64::MAX;
    model.Name = astersql_parser_ast::NewCIStr("bad_range");
    model.State = astersql_meta_model::StatePublic;
    let mut bad = astersql_infoschema::infoSchema::new(1);
    bad.add_schema(
        astersql_infoschema::DBInfo {
            id: 42,
            name: astersql_infoschema::CiString::new("invalid"),
            ..Default::default()
        },
        vec![astersql_infoschema::Table::new(
            astersql_infoschema::TableInfo {
                id: model.ID,
                db_id: 42,
                name: astersql_infoschema::CiString::new("bad_range"),
                model_meta: Some(Arc::new(model)),
                ..Default::default()
            },
        )],
    );
    let error = crate::pg_catalog::class_rows(&bad, "invalid").unwrap_err();
    assert_eq!(crate::pg_conn::sqlstate(&error), "0A000");
    drop(a);
    drop(b);
    drop(c);
    drop(cleanup);
    println!("pg13_005_contract: base=4 connections=3 key=3 lifecycle=3 range=9 cleanup=1");
}

#[test]
fn pg13_006_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_006_policy").unwrap();
    native.execute("CREATE DATABASE pg13_006_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_006_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_006_admin")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_006_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_006_admin");
    let (mut c, _) = connect("pg13_006_policy");
    let checks = [
        (
            "SELECT pg_catalog.to_regclass('public.missing') AS absent",
            vec![Some("2205")],
            vec![None],
        ),
        (
            "SELECT '4294967295'::regclass AS maximum,NULL::regclass AS absent",
            vec![Some("2205"), Some("2205")],
            vec![Some("4294967295"), None],
        ),
    ];
    for (sql, types, values) in checks {
        let got = query(&mut a, sql);
        assert_eq!(got[0].0, b'T', "{sql}: {got:?}");
        assert_eq!(
            columns(&got[0].1)
                .iter()
                .map(|c| c.1.to_string())
                .collect::<Vec<_>>(),
            types
                .iter()
                .map(|t| t.unwrap().to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(got.iter().find(|m| m.0 == b'D').unwrap().1, row(&values));
    }
    for sql in [
        "CREATE TABLE public.pg_class(id integer)",
        "CREATE TABLE public.\"Rel Mixed\"(id integer)",
    ] {
        let got = query(&mut a, sql);
        assert!(!got.iter().any(|m| m.0 == b'E'), "{got:?}");
    }
    let got = query(&mut a, "SET search_path=public,pg_catalog");
    assert!(!got.iter().any(|m| m.0 == b'E'));
    let got = query(&mut a, "SELECT 1259::regclass::text AS name");
    assert_eq!(
        got.iter().find(|m| m.0 == b'D').unwrap().1,
        row(&[Some("pg_catalog.pg_class")])
    );
    let got = query(&mut b, "SELECT 'public.\"Rel Mixed\"'::regclass::oid AS id");
    assert!(!got.iter().any(|m| m.0 == b'E'), "{got:?}");
    let got = query(
        &mut c,
        "SELECT pg_catalog.to_regclass('public.\"Rel Mixed\"') AS missing",
    );
    assert_eq!(got.iter().find(|m| m.0 == b'D').unwrap().1, row(&[None]));
    for sql in [
        "SELECT 'public.missing'::regclass",
        "SELECT '\"unfinished'::regclass",
        "SELECT '4294967296'::regclass",
    ] {
        let got = query(&mut a, sql);
        assert!(got.iter().any(|m| m.0 == b'E'), "{got:?}");
        let got = query(&mut a, "SELECT 1259::oid AS recovered");
        assert!(!got.iter().any(|m| m.0 == b'E'), "{got:?}");
    }
    for table in ["legacy_one", "legacy_two"] {
        native
            .execute(&format!(
                "CREATE TABLE pg13_006_admin.{table}(id integer,KEY duplicate_local(id))"
            ))
            .unwrap();
    }
    let ambiguous = query(&mut a, "SELECT 'public.duplicate_local'::regclass");
    assert_eq!(ambiguous[0].0, b'E', "{ambiguous:?}");
    assert!(
        ambiguous[0].1.windows(5).any(|s| s == b"42725"),
        "{ambiguous:?}"
    );
    let ambiguous_display = query(
        &mut a,
        "SELECT c.oid::regclass AS relation FROM pg_catalog.pg_class c WHERE c.relname='duplicate_local'",
    );
    assert_eq!(ambiguous_display[0].0, b'E', "{ambiguous_display:?}");
    assert!(
        ambiguous_display[0].1.windows(5).any(|s| s == b"42725"),
        "{ambiguous_display:?}"
    );
    let recovered = query(&mut a, "SELECT 1259::oid AS recovered");
    assert!(!recovered.iter().any(|m| m.0 == b'E'));
    drop(a);
    drop(b);
    drop(c);
    drop(cleanup);
    println!(
        "pg13_006_contract: unknown=2 path=1 quoted=1 cross_database=1 errors_recovered=3 legacy_ambiguity=2 cleanup=1"
    );
}

#[test]
fn pg13_007_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_007_policy").unwrap();
    native.execute("CREATE DATABASE pg13_007_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_007_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_007_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_007_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_007_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_007_admin");
    let (mut c, _) = connect("pg13_007_policy");
    for sql in [
        "CREATE TABLE public.rel_t(id integer)",
        "CREATE INDEX rel_ix ON public.rel_t(id)",
    ] {
        let got = query(&mut a, sql);
        assert!(!got.iter().any(|m| m.0 == b'E'), "{sql}: {got:?}");
    }
    let sql = "SELECT c.relname,c.relkind,c.relowner,pg_catalog.pg_get_userbyid(c.relowner) AS owner FROM pg_catalog.pg_class c WHERE c.relname='rel_t'";
    let got = query(&mut a, sql);
    assert_eq!(got[0].0, b'T', "{got:?}");
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![19, 18, 26, 19]
    );
    let owner_row = got.iter().find(|m| m.0 == b'D').unwrap();
    assert!(
        !owner_row.1.windows(4).any(|w| w == [255, 255, 255, 255]),
        "owner must be non-NULL: {got:?}"
    );
    let second = query(&mut b, sql);
    assert_eq!(second.iter().find(|m| m.0 == b'D'), Some(owner_row));
    let absent = query(&mut c, sql);
    assert!(!absent.iter().any(|m| m.0 == b'D'));
    native
        .execute("CREATE USER 'pg13_007_owner'@'%' IDENTIFIED BY ''")
        .unwrap();
    let changed = query(&mut a, "ALTER TABLE public.rel_t OWNER TO pg13_007_owner");
    assert!(!changed.iter().any(|m| m.0 == b'E'), "{changed:?}");
    for socket in [&mut a, &mut b] {
        let got = query(socket, sql);
        let owner = got.iter().find(|m| m.0 == b'D').unwrap();
        assert!(
            owner
                .1
                .windows(b"pg13_007_owner".len())
                .any(|s| s == b"pg13_007_owner"),
            "{got:?}"
        );
    }
    for field in [
        "xmin",
        "relam",
        "relpartbound",
        "reltablespace",
        "relacl",
        "reloptions",
        "relpersistence",
        "relispartition",
    ] {
        let got = query(
            &mut a,
            &format!("SELECT c.{field} FROM pg_catalog.pg_class c WHERE c.relname='rel_t'"),
        );
        assert_eq!(got[0].0, b'E', "{field}: {got:?}");
        assert!(
            got[0].1.windows(5).any(|s| s == b"0A000"),
            "{field}: {got:?}"
        );
        let recovered = query(&mut a, sql);
        assert!(!recovered.iter().any(|m| m.0 == b'E'), "{recovered:?}");
    }
    native
        .execute("CREATE TABLE pg13_007_admin.legacy(id INT)")
        .unwrap();
    let legacy = query(
        &mut a,
        "SELECT c.oid,c.relowner FROM pg_class c WHERE c.relname='legacy'",
    );
    assert_eq!(legacy[0].0, b'E', "{legacy:?}");
    assert!(legacy[0].1.windows(5).any(|s| s == b"0A000"));
    let adopted = query(&mut a, "ALTER TABLE public.legacy OWNER TO root");
    assert!(!adopted.iter().any(|m| m.0 == b'E'), "{adopted:?}");
    let legacy = query(
        &mut b,
        "SELECT c.relowner FROM pg_class c WHERE c.relname='legacy'",
    );
    assert_eq!(
        legacy.iter().filter(|m| m.0 == b'D').count(),
        1,
        "{legacy:?}"
    );
    assert!(
        native.execute("DROP USER 'pg13_007_owner'@'%'").is_err(),
        "must not drop a referenced persistent owner"
    );
    drop(a);
    drop(b);
    drop(c);
    drop(cleanup);
    println!(
        "pg13_007_contract: kinds=2 owner_connections=2 owner_transfer=1 legacy_adoption=1 owner_drop_guard=1 cross_database=1 rejected_projections_recovered=8 cleanup=1"
    );
}

#[test]
fn pg13_008_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_008_policy").unwrap();
    native.execute("CREATE DATABASE pg13_008_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_008_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_008_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_008_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_008_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_008_admin");
    let (mut c, _) = connect("pg13_008_policy");
    let ddl = query(
        &mut a,
        "CREATE TABLE public.attr_life(id integer, label varchar(24), obsolete integer, stamp integer)",
    );
    assert!(!ddl.iter().any(|m| m.0 == b'E'), "{ddl:?}");
    let index = query(
        &mut a,
        "CREATE INDEX attr_life_stamp ON public.attr_life(stamp)",
    );
    assert!(!index.iter().any(|m| m.0 == b'E'), "{index:?}");
    let sql = "SELECT a.attnum,a.attname,a.atttypid,a.attisdropped FROM pg_catalog.pg_attribute a WHERE a.attrelid='public.attr_life'::regclass::oid AND a.attnum>0 ORDER BY a.attnum";
    let initial = query(&mut a, sql);
    assert_eq!(
        initial.iter().filter(|m| m.0 == b'D').count(),
        4,
        "{initial:?}"
    );
    for sql in [
        "ALTER TABLE public.attr_life RENAME COLUMN label TO \"Label\"",
        "ALTER TABLE public.attr_life DROP COLUMN obsolete",
        "ALTER TABLE public.attr_life ADD COLUMN added varchar(7)",
    ] {
        let got = query(&mut a, sql);
        assert!(!got.iter().any(|m| m.0 == b'E'), "{sql}: {got:?}");
    }
    for socket in [&mut a, &mut b] {
        let got = query(socket, sql);
        assert_eq!(
            got.iter().filter(|m| m.0 == b'D').count(),
            5,
            "DROP must retain a durable tombstone: {got:?}"
        );
        for values in [
            [Some("2"), Some("Label"), Some("1043"), Some("f")],
            [Some("4"), Some("stamp"), Some("23"), Some("f")],
            [Some("5"), Some("added"), Some("1043"), Some("f")],
        ] {
            assert!(got.contains(&(b'D', row(&values))), "{got:?}");
        }
    }
    let index = query(
        &mut a,
        "SELECT i.indkey FROM pg_catalog.pg_index i WHERE i.indrelid='public.attr_life'::regclass::oid",
    );
    assert!(
        index.contains(&(b'D', row(&[Some("4")]))),
        "index key must retain stamp's PG slot: {index:?}"
    );
    let isolated = query(
        &mut c,
        "SELECT a.attnum FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid WHERE c.relname='attr_life'",
    );
    assert_eq!(isolated.iter().filter(|m| m.0 == b'D').count(), 0);
    // Inject only canonical metadata, then exercise the real provider's gate.
    let meta = domain
        .info_schema()
        .TableByName(
            &astersql_infoschema::CiString::new("pg13_008_admin"),
            &astersql_infoschema::CiString::new("attr_life"),
        )
        .unwrap();
    let original = meta.Meta().model_meta.as_ref().unwrap().as_ref().clone();
    assert_eq!(original.PostgreSQLAttributeSlots.as_ref().unwrap().len(), 5);
    for scenario in 0..7 {
        let mut model = original.clone();
        match scenario {
            0 => model.PostgreSQLAttributeSlots.as_mut().unwrap()[0].0 = 0,
            1 => {
                model.PostgreSQLAttributeSlots.as_mut().unwrap()[1].0 =
                    model.PostgreSQLAttributeSlots.as_ref().unwrap()[0].0
            }
            2 => model.PostgreSQLAttributeSlots.as_mut().unwrap()[0].1 = true,
            3 => model.PostgreSQLAttributeSlots = Some(vec![(1, false); i16::MAX as usize + 1]),
            4 => model.PostgreSQLAttributeSlots = None,
            5 => model.Columns[0].ID = -1,
            _ => model.Columns[0].Hidden = true,
        }
        let mut bad = astersql_infoschema::infoSchema::new(1);
        bad.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("invalid"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: model.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new("attr_life"),
                    model_meta: Some(Arc::new(model)),
                    ..Default::default()
                },
            )],
        );
        let err = crate::pg_catalog::column_catalog_rows(
            "pg_attribute",
            &bad,
            "invalid",
            &crate::conn::CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(
            crate::pg_conn::sqlstate(&err),
            "0A000",
            "scenario={scenario}"
        );
    }
    // Hidden implementation columns do not acquire negative system attributes.
    let mut hidden = original.clone();
    let mut hidden_column = hidden.Columns[0].clone();
    hidden_column.ID = 999;
    hidden_column.Hidden = true;
    hidden.Columns.push(hidden_column);
    assert_eq!(hidden.postgresql_attribute_slots().unwrap().len(), 5);
    drop((a, b, c));
    drop(cleanup);
    println!(
        "pg13_008_contract: initial=1 rename=1 drop=1 add=1 second_connection=1 cross_database=1 invalid_metadata=7 hidden=1 cleanup=1"
    );
}

#[test]
fn pg13_009_contract() {
    let metadata =
        crate::pg_catalog::CatalogQuery::parse("SELECT attname,atttypmod FROM pg_attribute")
            .unwrap()
            .unwrap()
            .metadata();
    assert_eq!(
        metadata.catalog_column_origins,
        vec![Some((1249, 2)), Some((1249, 6))]
    );
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_009_policy").unwrap();
    native.execute("CREATE DATABASE pg13_009_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_009_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_009_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_009_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_009_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_009_admin");
    let (mut c, _) = connect("pg13_009_policy");
    let ddl = query(
        &mut a,
        "CREATE TABLE public.decl_types(i integer, v varchar(24), c char(7), n numeric(12,3), u numeric, neg numeric(8,-2), t timestamp, p timestamp(3), arr integer[], b boolean, txt text)",
    );
    assert!(!ddl.iter().any(|m| m.0 == b'E'), "{ddl:?}");
    let sql = "SELECT a.attname,a.atttypid,a.atttypmod FROM pg_catalog.pg_attribute a WHERE a.attrelid='public.decl_types'::regclass::oid ORDER BY a.attnum";
    let expected = [
        ("i", 23, -1),
        ("v", 1043, 28),
        ("c", 1042, 11),
        ("n", 1700, 786439),
        ("u", 1700, -1),
        ("neg", 1700, 526338),
        ("t", 1114, -1),
        ("p", 1114, 3),
        ("arr", 1007, -1),
        ("b", 16, -1),
        ("txt", 25, -1),
    ];
    for socket in [&mut a, &mut b] {
        let got = query(socket, sql);
        assert_eq!(got[0].0, b'T', "{got:?}");
        assert_eq!(
            columns(&got[0].1),
            vec![
                ("attname".into(), 19),
                ("atttypid".into(), 26),
                ("atttypmod".into(), 23)
            ]
        );
        for (name, oid, modifier) in expected {
            assert!(
                got.contains(&(
                    b'D',
                    row(&[
                        Some(name),
                        Some(&oid.to_string()),
                        Some(&modifier.to_string())
                    ])
                )),
                "{name}: {got:?}"
            );
        }
        let empty = query(socket, &format!("{sql} LIMIT 0"));
        assert_eq!(empty[0], got[0]);
        assert_eq!(empty.iter().filter(|m| m.0 == b'D').count(), 0);
    }
    let table = domain
        .info_schema()
        .TableByName(
            &astersql_infoschema::CiString::new("pg13_009_admin"),
            &astersql_infoschema::CiString::new("decl_types"),
        )
        .unwrap();
    for scenario in 0..3 {
        let mut model = table.Meta().model_meta.as_ref().unwrap().as_ref().clone();
        match scenario {
            0 => model.Columns[3].SetDecimal(1001),
            1 => model.Columns[6].SetDecimal(7),
            _ => model.Columns[1].SetFlen(i32::MAX as isize),
        }
        let mut bad = astersql_infoschema::infoSchema::new(1);
        bad.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("invalid"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: model.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new("decl_types"),
                    model_meta: Some(Arc::new(model)),
                    ..Default::default()
                },
            )],
        );
        let error = crate::pg_catalog::column_catalog_rows(
            "pg_attribute",
            &bad,
            "invalid",
            &crate::conn::CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(
            crate::pg_conn::sqlstate(&error),
            "0A000",
            "scenario={scenario}"
        );
    }
    native
        .execute("CREATE TABLE pg13_009_admin.native_tiny(x TINYINT(1) DEFAULT TRUE)")
        .unwrap();
    let tiny = query(
        &mut a,
        "SELECT a.atttypid FROM pg_catalog.pg_attribute a WHERE a.attrelid='public.native_tiny'::regclass::oid",
    );
    assert!(
        tiny.contains(&(b'D', row(&[Some("21")]))),
        "native width/default must not imply boolean: {tiny:?}"
    );
    drop((a, b, c));
    drop(cleanup);
    println!(
        "pg13_009_contract: declarations=11 connections=2 zero_rows=2 invalid_metadata=3 native_tiny=1 cleanup=1"
    );
}

#[test]
fn pg13_010_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_010_policy").unwrap();
    native.execute("CREATE DATABASE pg13_010_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_010_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_010_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_010_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_010_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_010_admin");
    let (mut c, _) = connect("pg13_010_policy");
    let ddl = query(
        &mut a,
        "CREATE TABLE public.null_contract(id integer NOT NULL, label varchar(24))",
    );
    assert!(!ddl.iter().any(|m| m.0 == b'E'), "{ddl:?}");
    let sql = "SELECT a.attname,a.attnotnull FROM pg_catalog.pg_attribute a WHERE a.attrelid='public.null_contract'::regclass::oid ORDER BY a.attnum";
    for (ddl, flag) in [
        ("SELECT 1", "f"),
        (
            "ALTER TABLE public.null_contract ALTER COLUMN label SET NOT NULL",
            "t",
        ),
        (
            "ALTER TABLE public.null_contract ALTER COLUMN label DROP NOT NULL",
            "f",
        ),
    ] {
        let changed = query(&mut a, ddl);
        assert!(!changed.iter().any(|m| m.0 == b'E'), "{changed:?}");
        for socket in [&mut a, &mut b] {
            let got = query(socket, sql);
            assert_eq!(
                columns(&got[0].1),
                vec![("attname".into(), 19), ("attnotnull".into(), 16)]
            );
            assert!(
                got.contains(&(b'D', row(&[Some("id"), Some("t")]))),
                "{got:?}"
            );
            assert!(
                got.contains(&(b'D', row(&[Some("label"), Some(flag)]))),
                "{got:?}"
            );
            let empty = query(socket, &format!("{sql} LIMIT 0"));
            assert_eq!(empty[0], got[0]);
            assert!(!empty.iter().any(|m| m.0 == b'D'));
        }
    }
    let begin = query(&mut a, "BEGIN");
    assert_eq!(begin.last().unwrap().1, b"T");
    let failed = query(&mut a, "INSERT INTO public.null_contract VALUES(NULL,NULL)");
    assert!(
        failed
            .iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"23502")),
        "{failed:?}"
    );
    assert_eq!(failed.last().unwrap().1, b"E");
    let rollback = query(&mut a, "ROLLBACK");
    assert_eq!(rollback.last().unwrap().1, b"I");
    let accepted = query(&mut a, "INSERT INTO public.null_contract VALUES(1,NULL)");
    assert!(!accepted.iter().any(|m| m.0 == b'E'), "{accepted:?}");
    let isolated = query(
        &mut c,
        "SELECT a.attname FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid WHERE c.relname='null_contract'",
    );
    assert!(!isolated.iter().any(|m| m.0 == b'D'));
    drop((a, b, c));
    drop(cleanup);
    println!(
        "pg13_010_contract: nullability_states=3 connections=2 zero_rows=6 enforcement=1 recovery=1 cross_database=1 cleanup=1"
    );
}

#[test]
fn pg13_011_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_011_policy").unwrap();
    native.execute("CREATE DATABASE pg13_011_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_011_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_011_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_011_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_011_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_011_admin");
    let (mut c, _) = connect("pg13_011_policy");
    let sql = "SELECT t.oid,t.typname,t.typnamespace,t.typtype,t.typcategory,t.typelem,t.typarray,t.typdelim,t.typlen,t.typbyval,t.typisdefined FROM pg_catalog.pg_type t WHERE t.oid IN (23,25,1007,1009) ORDER BY t.oid";
    let got = query(&mut a, sql);
    assert_eq!(
        got[0].0, b'T',
        "TYPE-BASE must return real builtin facts: {got:?}"
    );
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![26, 19, 26, 18, 18, 26, 26, 18, 21, 16, 16]
    );
    for values in [
        [
            "23", "int4", "11", "b", "N", "0", "1007", ",", "4", "t", "t",
        ],
        [
            "25", "text", "11", "b", "S", "0", "1009", ",", "-1", "f", "t",
        ],
        [
            "1007", "_int4", "11", "b", "A", "23", "0", ",", "-1", "f", "t",
        ],
        [
            "1009", "_text", "11", "b", "A", "25", "0", ",", "-1", "f", "t",
        ],
    ] {
        assert!(got.contains(&(b'D', row(&values.map(Some)))), "{got:?}");
    }
    assert_eq!(got.iter().filter(|m| m.0 == b'D').count(), 4);
    assert_eq!(query(&mut b, sql), got);
    assert_eq!(query(&mut c, sql), got);
    let empty = query(&mut a, &format!("{sql} LIMIT 0"));
    assert_eq!(empty[0], got[0]);
    assert!(!empty.iter().any(|m| m.0 == b'D'));
    // All result and declaration identities, including range/pseudo types, are unique.
    for oid in [
        16, 17, 18, 19, 20, 21, 22, 23, 25, 26, 30, 114, 199, 700, 701, 1000, 1001, 1002, 1003,
        1005, 1006, 1007, 1009, 1013, 1014, 1015, 1016, 1021, 1022, 1028, 1033, 1034, 1042, 1043,
        1082, 1083, 1114, 1115, 1182, 1183, 1184, 1185, 1186, 1187, 1231, 1700, 2205, 2206, 2210,
        2211, 2249, 2287, 2950, 2951, 3802, 3807, 3904, 3905, 3906, 3907,
    ] {
        let result = query(
            &mut a,
            &format!("SELECT t.oid FROM pg_catalog.pg_type t WHERE t.oid={oid}"),
        );
        assert_eq!(
            result.iter().filter(|m| m.0 == b'D').count(),
            1,
            "missing or duplicated OID {oid}: {result:?}"
        );
    }
    // Static builtin lookup must not validate unrelated native enum columns.
    native
        .execute("CREATE TABLE pg13_011_admin.bad_type (e ENUM('a','b'))")
        .unwrap();
    assert_eq!(query(&mut a, sql), got);
    for unsupported in [
        "CREATE DOMAIN public.type_domain AS integer CHECK (VALUE>0)",
        "CREATE TYPE public.type_composite AS (id integer)",
        "SELECT t.typowner,t.typacl FROM pg_catalog.pg_type t WHERE t.oid=23",
    ] {
        let result = query(&mut a, unsupported);
        assert!(
            result
                .iter()
                .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"0A000")),
            "{unsupported}: {result:?}"
        );
        assert_eq!(result.last().unwrap().1, b"I");
        assert_eq!(query(&mut a, sql), got);
    }
    drop((a, b, c));
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err());
    println!(
        "pg13_011_contract: base=4 closure=60 zero_rows=1 static_bad_model=1 refusals=3 recovery=3 connections=3 cleanup=1"
    );
}

#[test]
fn pg13_012_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_012_policy").unwrap();
    native.execute("CREATE DATABASE pg13_012_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_012_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_012_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_012_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_012_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_012_admin");
    let (mut c, _) = connect("pg13_012_policy");
    let sql = "SELECT t.oid,t.typname,t.typelem,t.typarray,t.typdelim FROM pg_catalog.pg_type t WHERE t.oid IN (21,22,23,25,26,30,1005,1006,1007,1009,1013,1028) ORDER BY t.oid";
    let got = query(&mut a, sql);
    assert_eq!(got[0].0, b'T', "{got:?}");
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![26, 19, 26, 26, 18]
    );
    for values in [
        ["21", "int2", "0", "1005", ","],
        ["22", "int2vector", "21", "1006", ","],
        ["23", "int4", "0", "1007", ","],
        ["25", "text", "0", "1009", ","],
        ["26", "oid", "0", "1028", ","],
        ["30", "oidvector", "26", "1013", ","],
        ["1005", "_int2", "21", "0", ","],
        ["1006", "_int2vector", "22", "0", ","],
        ["1007", "_int4", "23", "0", ","],
        ["1009", "_text", "25", "0", ","],
        ["1013", "_oidvector", "30", "0", ","],
        ["1028", "_oid", "26", "0", ","],
    ] {
        assert!(got.contains(&(b'D', row(&values.map(Some)))), "{got:?}");
    }
    assert_eq!(got.iter().filter(|m| m.0 == b'D').count(), 12);
    assert_eq!(query(&mut b, sql), got);
    assert_eq!(query(&mut c, sql), got);
    let empty = query(&mut a, &format!("{sql} LIMIT 0"));
    assert_eq!(empty[0], got[0]);
    assert_eq!(empty.iter().filter(|m| m.0 == b'D').count(), 0);
    drop((a, b, c));
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err());
    println!("pg13_012_contract: associations=12 connections=3 zero_rows=1 cleanup=1");
}

#[test]
fn pg13_013_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_013_policy").unwrap();
    native.execute("CREATE DATABASE pg13_013_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_013_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_013_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_013_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_013_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_013_admin");
    let (mut c, _) = connect("pg13_013_policy");
    let cases = [
        ("23,-1", Some("integer")),
        ("1043,28", Some("character varying(24)")),
        ("1700,786439", Some("numeric(12,3)")),
        ("1042,NULL", Some("character")),
        ("1042,-1", Some("bpchar")),
        ("1007,-1", Some("integer[]")),
        ("1015,28", Some("character varying(24)[]")),
        ("1185,3", Some("timestamp(3) with time zone[]")),
        ("NULL,-1", None),
        ("0,-1", Some("-")),
        ("4294967295,-1", Some("???")),
        ("2950,-1", Some("uuid")),
    ];
    for (args, expected) in cases {
        let sql = format!("SELECT pg_catalog.format_type({args}) AS type_name");
        let got = query(&mut a, &sql);
        assert_eq!(got[0].0, b'T', "{sql}: {got:?}");
        assert_eq!(columns(&got[0].1)[0].1, 25);
        assert!(got.contains(&(b'D', row(&[expected]))), "{sql}: {got:?}");
        assert_eq!(query(&mut b, &sql), got);
        assert_eq!(query(&mut c, &sql), got);
    }
    let empty = query(
        &mut a,
        "SELECT pg_catalog.format_type(23,-1) AS type_name LIMIT 0",
    );
    assert_eq!(empty[0].0, b'T');
    assert_eq!(empty.iter().filter(|m| m.0 == b'D').count(), 0);
    let got = query(&mut a, "SELECT pg_catalog.format_type(790,-1)");
    assert_eq!(got[0].0, b'E', "{got:?}");
    assert!(got[0].1.windows(6).any(|w| w == b"0A000\0"));
    assert_eq!(query(&mut a, "SELECT 1")[0].0, b'T');
    drop((a, b, c));
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err());
    println!(
        "pg13_013_contract: format=12 refusal=1 recovery=1 connections=3 zero_rows=1 cleanup=1"
    );
}

#[test]
fn pg13_014_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_014_policy").unwrap();
    native.execute("CREATE DATABASE pg13_014_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_014_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_014_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_014_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_014_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_014_admin");
    let (mut c, _) = connect("pg13_014_policy");
    let ddl = query(
        &mut a,
        "CREATE TABLE public.idx_contract (id integer, a integer, b integer, CONSTRAINT idx_contract_pk PRIMARY KEY (id))",
    );
    assert_eq!(ddl[0].0, b'C', "{ddl:?}");
    for sql in [
        "CREATE INDEX idx_contract_ba ON public.idx_contract (b,a)",
        "CREATE UNIQUE INDEX idx_contract_uq ON public.idx_contract (a)",
    ] {
        let got = query(&mut a, sql);
        assert_eq!(got[0].0, b'C', "{got:?}");
    }
    let sql = "SELECT c.relname,i.indexrelid,i.indrelid,i.indisunique,i.indisprimary FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE i.indrelid='public.idx_contract'::regclass::oid ORDER BY c.relname";
    let got = query(&mut a, sql);
    assert_eq!(got[0].0, b'T', "{got:?}");
    assert_eq!(got.iter().filter(|m| m.0 == b'D').count(), 3, "{got:?}");
    assert_eq!(query(&mut b, sql), got);
    let constraints = query(
        &mut a,
        "SELECT conname FROM pg_catalog.pg_constraint WHERE conrelid='public.idx_contract'::regclass::oid AND contype='u'",
    );
    assert_eq!(constraints[0].0, b'T', "{constraints:?}");
    assert_eq!(
        constraints.iter().filter(|m| m.0 == b'D').count(),
        0,
        "standalone unique index is not a constraint: {constraints:?}"
    );
    query(
        &mut a,
        "CREATE TABLE public.idx_handle (id integer PRIMARY KEY)",
    );
    let snapshot = domain.info_schema();
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_014_admin"))
        .unwrap();
    let handle = tables
        .iter()
        .find(|t| t.model_meta.as_ref().unwrap().Name.O == "idx_handle")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    assert!(handle.PKIsHandle);
    assert!(!handle.Indices.iter().any(|i| i.Primary));
    let indexes = crate::pg_catalog::catalog_indexes(handle).unwrap();
    assert_eq!(indexes.len(), 1);
    assert_eq!(indexes[0].ID, -1);
    let mut explicit = (**handle).clone();
    explicit.Indices.push(astersql_meta_model::IndexInfo {
        ID: 1,
        ..indexes[0].clone()
    });
    assert_eq!(
        crate::pg_catalog::catalog_indexes(&explicit).unwrap().len(),
        1
    );
    let mut invalid = (**handle).clone();
    invalid.Columns.push(invalid.Columns[0].clone());
    assert!(
        crate::pg_catalog::catalog_indexes(&invalid).is_err(),
        "multiple handle columns must be rejected"
    );
    let mut missing = (**handle).clone();
    missing.Columns.clear();
    assert!(crate::pg_catalog::catalog_indexes(&missing).is_err());
    let mut duplicate = explicit.clone();
    duplicate.Indices.push(duplicate.Indices[0].clone());
    assert!(crate::pg_catalog::catalog_indexes(&duplicate).is_err());
    duplicate.Indices[1].ID = 2;
    assert!(crate::pg_catalog::catalog_indexes(&duplicate).is_err());
    let mut non_integer = (**handle).clone();
    non_integer.Columns[0].SetType(astersql_parser_mysql::r#type::TypeVarchar);
    assert!(crate::pg_catalog::catalog_indexes(&non_integer).is_err());
    let mut invalid_id = explicit.clone();
    invalid_id.ID = 0;
    assert!(crate::pg_catalog::catalog_indexes(&invalid_id).is_err());
    invalid_id.ID = explicit.ID;
    invalid_id.Indices[0].ID = i64::MAX;
    assert!(crate::pg_catalog::catalog_indexes(&invalid_id).is_err());
    let null = query(
        &mut a,
        "SELECT pg_catalog.pg_get_indexdef(NULL) AS definition",
    );
    assert_eq!(null[0].0, b'T', "{null:?}");
    assert!(null.contains(&(b'D', row(&[None]))));
    let isolated = query(&mut c, "SELECT indexrelid FROM pg_catalog.pg_index");
    assert_eq!(isolated[0].0, b'T');
    assert_eq!(isolated.iter().filter(|m| m.0 == b'D').count(), 0);
    drop((a, b, c));
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err());
    println!(
        "pg13_014_contract: base=3 handle=1 explicit=1 invalid=7 standalone_unique=1 isolation=1 connections=3 cleanup=1"
    );
}

#[test]
fn pg13_015_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_015_policy").unwrap();
    native.execute("CREATE DATABASE pg13_015_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_015_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_015_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_015_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_015_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_015_admin");
    let (mut c, _) = connect("pg13_015_policy");
    for sql in [
        "CREATE TABLE public.idx_contract (id integer, obsolete integer, a integer, b integer)",
        "ALTER TABLE public.idx_contract DROP COLUMN obsolete",
        "CREATE INDEX idx_contract_ba ON public.idx_contract (b,a)",
    ] {
        let got = query(&mut a, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let sql = "SELECT i.indkey,i.indkey[0],i.indkey[1],i.indoption,i.indnatts,i.indnkeyatts FROM pg_catalog.pg_index i WHERE i.indrelid='public.idx_contract'::regclass::oid";
    let got = query(&mut a, sql);
    assert_eq!(got[0].0, b'T', "{got:?}");
    assert!(
        got.contains(&(
            b'D',
            row(&[
                Some("4 3"),
                Some("4"),
                Some("3"),
                Some("0 0"),
                Some("2"),
                Some("2")
            ])
        )),
        "{got:?}"
    );
    assert_eq!(query(&mut b, sql), got);
    for shape in [
        "(b DESC)",
        "(b DESC NULLS LAST,a ASC NULLS FIRST)",
        "(a) INCLUDE (b)",
        "((a+1))",
    ] {
        let ddl = format!("CREATE INDEX idx_rejected ON public.idx_contract {shape}");
        let denied = query(&mut a, &ddl);
        assert!(
            denied
                .iter()
                .any(|m| m.0 == b'E' && m.1.windows(7).any(|w| w == b"C0A000\0")),
            "{ddl}: {denied:?}"
        );
        assert_eq!(query(&mut b, sql), got, "rejection must not change indexes");
        assert_eq!(query(&mut a, "SELECT 1")[0].0, b'T');
    }
    let meta = domain
        .info_schema()
        .TableByName(
            &astersql_infoschema::CiString::new("pg13_015_admin"),
            &astersql_infoschema::CiString::new("idx_contract"),
        )
        .unwrap();
    let original = meta.Meta().model_meta.as_ref().unwrap().as_ref().clone();
    for scenario in 0..4 {
        let mut model = original.clone();
        match scenario {
            0 => model.Indices[0].Columns[0].Offset = 999,
            1 => model.Indices[0].Columns[0].Name = astersql_parser_ast::NewCIStr("missing"),
            2 => {
                model.PostgreSQLAttributeSlots.as_mut().unwrap()[3].0 =
                    model.PostgreSQLAttributeSlots.as_ref().unwrap()[2].0
            }
            _ => model.PostgreSQLAttributeSlots = Some(vec![(1, false); i16::MAX as usize + 1]),
        }
        let mut bad = astersql_infoschema::infoSchema::new(1);
        bad.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("invalid"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: model.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new("idx_contract"),
                    model_meta: Some(Arc::new(model)),
                    ..Default::default()
                },
            )],
        );
        let err = crate::pg_catalog::index_constraint_rows(
            "pg_index",
            &bad,
            "invalid",
            &crate::conn::CancellationToken::new(),
            &[],
        )
        .unwrap_err();
        assert_eq!(
            crate::pg_conn::sqlstate(&err),
            "0A000",
            "scenario={scenario}"
        );
    }
    let isolated = query(&mut c, "SELECT indkey,indoption FROM pg_catalog.pg_index");
    assert_eq!(isolated[0].0, b'T');
    assert_eq!(isolated.iter().filter(|m| m.0 == b'D').count(), 0);
    drop((a, b, c));
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err());
    println!(
        "pg13_015_contract: persistent_order=1 vector_subscripts=2 options=1 rejected_shapes=4 invalid_metadata=4 second_connection=1 isolation=1 cleanup=1"
    );
}

#[test]
fn pg13_016_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_016_policy").unwrap();
    native.execute("CREATE DATABASE pg13_016_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_016_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_016_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_016_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_016_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_016_admin");
    let (mut c, _) = connect("pg13_016_policy");
    for sql in [
        "CREATE TABLE public.idx_contract (id integer, obsolete integer, a integer, b integer)",
        "ALTER TABLE public.idx_contract DROP COLUMN obsolete",
        "CREATE INDEX idx_contract_ba ON public.idx_contract (b,a)",
    ] {
        let got = query(&mut a, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let sql = "SELECT pg_catalog.pg_get_indexdef(i.indexrelid),pg_catalog.pg_get_indexdef(i.indexrelid,0,true),pg_catalog.pg_get_indexdef(i.indexrelid,1,true),pg_catalog.pg_get_indexdef(i.indexrelid,2,false),pg_catalog.pg_get_indexdef(i.indexrelid,99,false),pg_catalog.pg_get_indexdef(i.indexrelid,-1,true) FROM pg_catalog.pg_index i WHERE i.indrelid='public.idx_contract'::regclass::oid";
    let got = query(&mut a, sql);
    assert_eq!(got[0].0, b'T', "{got:?}");
    assert!(got.contains(&(b'D', row(&[Some("CREATE INDEX \"idx_contract_ba\" ON public.\"idx_contract\" USING btree (\"b\", \"a\")"),Some("CREATE INDEX \"idx_contract_ba\" ON public.\"idx_contract\" USING btree (\"b\", \"a\")"),Some("\"b\""),Some("\"a\""),Some(""),Some("")]))), "{got:?}");
    assert_eq!(query(&mut b, sql), got);
    let unknown = query(
        &mut a,
        "SELECT pg_catalog.pg_get_indexdef(0),pg_catalog.pg_get_indexdef(NULL),pg_catalog.pg_get_indexdef(0,99,true)",
    );
    assert!(
        unknown.contains(&(b'D', row(&[None, None, None]))),
        "{unknown:?}"
    );
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![25; 6]
    );
    assert_eq!(
        query(
            &mut c,
            "SELECT pg_catalog.pg_get_indexdef(indexrelid) FROM pg_catalog.pg_index"
        )
        .iter()
        .filter(|m| m.0 == b'D')
        .count(),
        0
    );
    let meta = domain
        .info_schema()
        .TableByName(
            &astersql_infoschema::CiString::new("pg13_016_admin"),
            &astersql_infoschema::CiString::new("idx_contract"),
        )
        .unwrap();
    let original = meta.Meta().model_meta.as_ref().unwrap().as_ref().clone();
    for scenario in 0..8 {
        let mut model = original.clone();
        match scenario {
            0 => model.Indices[0].Tp = astersql_meta_model::ast::IndexType::Invalid,
            1 => model.Indices[0].Tp = astersql_meta_model::ast::IndexType::Hash,
            2 => model.Indices[0].Invisible = true,
            3 => model.Indices[0].Global = true,
            4 => model.Indices[0].ConditionExprString = "a > 0".into(),
            5 => model.Indices[0].Columns[0].Length = 1,
            6 => model.Columns.last_mut().unwrap().Hidden = true,
            _ => model.Indices[0].Columns[0].UseChangingType = true,
        }
        let mut bad = astersql_infoschema::infoSchema::new(1);
        bad.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("invalid"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: model.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new("idx_contract"),
                    model_meta: Some(Arc::new(model)),
                    ..Default::default()
                },
            )],
        );
        let err = crate::pg_catalog::index_constraint_rows(
            "pg_index",
            &bad,
            "invalid",
            &crate::conn::CancellationToken::new(),
            &[],
        )
        .unwrap_err();
        assert_eq!(
            crate::pg_conn::sqlstate(&err),
            "0A000",
            "scenario={scenario}"
        );
    }
    drop((a, b, c));
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err());
    println!(
        "pg13_016_contract: full=2 key_order=2 out_of_range=2 unknown_null=3 invalid_model=8 second_connection=1 isolation=1 cleanup=1"
    );
}

#[test]
fn pg13_017_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_017_policy").unwrap();
    native.execute("CREATE DATABASE pg13_017_admin").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_017_policy")
            .unwrap();
        native
            .execute("DROP DATABASE IF EXISTS pg13_017_admin")
            .unwrap();
        native
            .execute("DROP USER IF EXISTS 'pg13_017_owner'@'%'")
            .unwrap();
    });
    let connect = |database: &str| {
        let mut socket = TcpStream::connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
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
        let mut messages = Vec::new();
        loop {
            let m = read(&mut socket);
            let done = m.0 == b'Z' || m.0 == b'E';
            messages.push(m);
            if done {
                break;
            }
        }
        (socket, messages)
    };
    let (mut a, startup) = connect("pg13_017_admin");
    assert_eq!(startup.last().unwrap().0, b'Z');
    let (mut b, _) = connect("pg13_017_admin");
    let (mut c, _) = connect("pg13_017_policy");
    for sql in [
        "CREATE TABLE public.idx_contract (id integer, obsolete integer, a integer, b integer)",
        "ALTER TABLE public.idx_contract DROP COLUMN obsolete",
        "CREATE INDEX idx_contract_ba ON public.idx_contract (b,a)",
    ] {
        let got = query(&mut a, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let sql = "SELECT i.indclass,i.indcollation,i.indexprs,i.indpred,i.indisvalid,i.indisready,i.indisexclusion,i.indimmediate,i.indisclustered,i.indcheckxmin,i.indislive,i.indisreplident FROM pg_catalog.pg_index i WHERE i.indrelid='public.idx_contract'::regclass::oid";
    let got = query(&mut a, sql);
    assert_eq!(got[0].0, b'T', "{got:?}");
    assert!(
        got.contains(&(
            b'D',
            row(&[
                Some("1978 1978"),
                Some("0 0"),
                None,
                None,
                Some("t"),
                Some("t"),
                Some("f"),
                Some("t"),
                Some("f"),
                Some("f"),
                Some("t"),
                Some("f")
            ])
        )),
        "{got:?}"
    );
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![30, 30, 194, 194, 16, 16, 16, 16, 16, 16, 16, 16]
    );
    assert_eq!(query(&mut b, sql), got);
    let empty = query(
        &mut c,
        "SELECT i.indclass,i.indexprs,i.indislive FROM pg_catalog.pg_index i",
    );
    assert_eq!(empty.iter().filter(|m| m.0 == b'D').count(), 0);
    assert_eq!(
        columns(&empty[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![30, 194, 16]
    );
    let meta = domain
        .info_schema()
        .TableByName(
            &astersql_infoschema::CiString::new("pg13_017_admin"),
            &astersql_infoschema::CiString::new("idx_contract"),
        )
        .unwrap();
    let original = meta.Meta().model_meta.as_ref().unwrap().as_ref().clone();
    for scenario in 0..11 {
        let mut model = original.clone();
        match scenario {
            0 => model.Indices[0].Tp = astersql_meta_model::ast::IndexType::Invalid,
            1 => model.Indices[0].Tp = astersql_meta_model::ast::IndexType::Hash,
            2 => model.Indices[0].Invisible = true,
            3 => model.Indices[0].Global = true,
            4 => model.Indices[0].ConditionExprString = "a > 0".into(),
            5 => model.Indices[0].Columns[0].Length = 1,
            6 => model.Columns.last_mut().unwrap().Hidden = true,
            7 => model.Indices[0].Columns[0].UseChangingType = true,
            8 => model.Indices[0].BackfillState = astersql_meta_model::BackfillStateRunning,
            9 => model.ID = 0,
            _ => model.Indices[0].State = astersql_meta_model::StateWriteOnly,
        }
        let mut bad = astersql_infoschema::infoSchema::new(1);
        bad.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("invalid"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: model.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new("idx_contract"),
                    model_meta: Some(Arc::new(model)),
                    ..Default::default()
                },
            )],
        );
        let err = crate::pg_catalog::index_constraint_rows(
            "pg_index",
            &bad,
            "invalid",
            &crate::conn::CancellationToken::new(),
            &[],
        )
        .unwrap_err();
        assert_eq!(
            crate::pg_conn::sqlstate(&err),
            if scenario == 9 { "22003" } else { "0A000" },
            "scenario={scenario}"
        );
    }
    assert!(original.Indices[0].PostgreSQLBuildComplete);
    let restored: astersql_meta_model::TableInfo =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(restored.Indices[0].PostgreSQLBuildComplete);
    // Legacy native publication has no PG build proof or string collation
    // equivalence. Related projections must refuse rather than emit NULL/true.
    native
        .execute("CREATE TABLE pg13_017_admin.legacy_aux (a VARCHAR(20),KEY legacy_key(a))")
        .unwrap();
    for field in ["indisvalid", "indclass", "indcollation"] {
        let refused = query(
            &mut a,
            &format!(
                "SELECT i.{field} FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indexrelid WHERE c.relname='legacy_key'"
            ),
        );
        assert_eq!(refused[0].0, b'E', "{refused:?}");
        assert!(refused[0].1.windows(6).any(|w| w == b"C0A000"));
        assert!(!refused.iter().any(|m| m.0 == b'D'));
        assert_eq!(query(&mut a, "SELECT 1").last(), Some(&(b'Z', vec![b'I'])));
    }
    drop((a, b, c));
    drop(cleanup);
    assert!(TcpStream::connect(addr).is_err());
    println!(
        "pg13_017_contract: aux_state_extra=12 null_shape=2 invalid_model=11 second_connection=1 isolation=1 cleanup=1"
    );
}

#[test]
fn pg13_018_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_018_contract").unwrap();
    native
        .execute("CREATE TABLE pg13_018_contract.native_unique (id INT)")
        .unwrap();
    native
        .execute("CREATE UNIQUE INDEX native_standalone ON pg13_018_contract.native_unique(id)")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_018_contract")
            .unwrap();
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_018_contract\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    let native_rows = query(
        &mut socket,
        "SELECT oid FROM pg_catalog.pg_constraint WHERE conrelid='public.native_unique'::regclass::oid",
    );
    assert_eq!(
        native_rows[0].0, b'E',
        "Unique flag alone is not constraint provenance: {native_rows:?}"
    );
    assert!(native_rows[0].1.windows(5).any(|w| w == b"0A000"));
    native
        .execute("DROP TABLE pg13_018_contract.native_unique")
        .unwrap();
    for sql in [
        "CREATE TABLE public.con_parent (a integer,b integer,code integer,CONSTRAINT con_pk PRIMARY KEY(a,b),CONSTRAINT con_uq UNIQUE(code))",
        "CREATE TABLE public.con_child (x integer,y integer,CONSTRAINT con_fk FOREIGN KEY(y,x) REFERENCES public.con_parent(a,b))",
        "CREATE UNIQUE INDEX con_standalone ON public.con_parent(b,a)",
        "CREATE TABLE public.con_handle (id integer CONSTRAINT con_handle_pk PRIMARY KEY)",
    ] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let sql = "SELECT c.oid,c.conname,c.contype,c.conrelid,c.connamespace,c.conindid FROM pg_catalog.pg_constraint c ORDER BY c.conname";
    let got = query(&mut socket, sql);
    // PG18 also exposes three primary-column NOT NULL declarations.
    assert_eq!(got.iter().filter(|m| m.0 == b'D').count(), 7, "{got:?}");
    assert_eq!(
        columns(&got[0].1).iter().map(|v| v.1).collect::<Vec<_>>(),
        vec![26, 19, 18, 26, 26, 26]
    );
    let snapshot = domain.info_schema();
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_018_contract"))
        .unwrap();
    let parent = tables
        .iter()
        .find(|t| t.name.lower == "con_parent")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    assert!(
        parent
            .Indices
            .iter()
            .any(|i| i.PostgreSQLConstraintName.as_deref() == Some("con_uq"))
    );
    assert!(
        parent
            .Indices
            .iter()
            .any(|i| i.Name.O == "con_standalone" && i.PostgreSQLConstraintName.is_none())
    );
    let before = query(
        &mut socket,
        "SELECT oid,conindid FROM pg_catalog.pg_constraint WHERE conname='con_uq'",
    );
    assert_eq!(
        query(
            &mut socket,
            "ALTER TABLE public.con_parent RENAME TO con_parent_new"
        )[0]
        .0,
        b'C'
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT oid,conindid FROM pg_catalog.pg_constraint WHERE conname='con_uq'"
        ),
        before
    );
    for sql in [
        "DROP TABLE public.con_child",
        "DROP TABLE public.con_parent_new",
        "DROP TABLE public.con_handle",
    ] {
        assert_eq!(query(&mut socket, sql)[0].0, b'C');
    }
    assert_eq!(
        query(
            &mut socket,
            "CREATE TABLE public.con_parent (a integer CONSTRAINT con_uq UNIQUE)"
        )[0]
        .0,
        b'C'
    );
    assert_ne!(
        query(
            &mut socket,
            "SELECT oid,conindid FROM pg_catalog.pg_constraint WHERE conname='con_uq'"
        ),
        before
    );
    send(&mut socket, b'X', &[]);
    println!(
        "pg13_018_contract: native_source=1 p_u_f=3 handle_dedup=1 rename=1 recreate=1 cleanup=1"
    );
}

#[test]
fn pg13_019_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_019_contract").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_019_contract")
            .unwrap();
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_019_contract\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "CREATE TABLE public.con_parent (a integer,b integer,CONSTRAINT con_pk PRIMARY KEY(b,a))",
        "CREATE TABLE public.con_child (x integer,y integer,CONSTRAINT con_fk FOREIGN KEY(y,x) REFERENCES public.con_parent(a,b) ON UPDATE RESTRICT ON DELETE CASCADE)",
    ] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let got = query(
        &mut socket,
        "SELECT conkey,confkey,confupdtype,confdeltype,confmatchtype FROM pg_catalog.pg_constraint WHERE conname='con_fk'",
    );
    assert_eq!(got[0].0, b'T', "{got:?}");
    assert!(
        got.iter().any(|m| m.0 == b'D'
            && m.1
                == row(&[
                    Some("{2,1}"),
                    Some("{1,2}"),
                    Some("r"),
                    Some("c"),
                    Some("s")
                ])),
        "{got:?}"
    );
    for sql in [
        "INSERT INTO public.con_parent VALUES (10,20)",
        "INSERT INTO public.con_child VALUES (20,10)",
        "INSERT INTO public.con_child VALUES (NULL,999)",
    ] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let bad = query(
        &mut socket,
        "UPDATE public.con_parent SET a=11 WHERE a=10 AND b=20",
    );
    assert_eq!(bad[0].0, b'E', "{bad:?}");
    let got = query(
        &mut socket,
        "DELETE FROM public.con_parent WHERE a=10 AND b=20",
    );
    assert_eq!(got[0].0, b'C', "{got:?}");
    let got = query(&mut socket, "SELECT x,y FROM public.con_child");
    assert!(
        got.iter()
            .any(|m| m.0 == b'D' && m.1 == row(&[None, Some("999")])),
        "{got:?}"
    );
    assert_eq!(got.iter().filter(|m| m.0 == b'D').count(), 1);
    let native_error = native
        .execute("INSERT INTO pg13_019_contract.con_child VALUES (777,666)")
        .err()
        .expect("native FK violation");
    assert!(
        native_error.to_string().contains("[kv:1452]"),
        "{native_error}"
    );
    // Corruption fixtures replace shared models, never provider output rows.
    let snapshot = domain.info_schema();
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_019_contract"))
        .unwrap();
    let parent = tables
        .iter()
        .find(|t| t.name.lower == "con_parent")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    let child = tables
        .iter()
        .find(|t| t.name.lower == "con_child")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    for scenario in 0..13 {
        let mut p = parent.as_ref().clone();
        let mut c = child.as_ref().clone();
        match scenario {
            0 => c.ForeignKeys[0].RefTable = astersql_meta_model::ast::NewCIStr("missing"),
            1 => {
                c.ForeignKeys[0].RefSchema = astersql_meta_model::ast::NewCIStr("another_database")
            }
            2 => {
                c.ForeignKeys[0].RefCols.pop();
            }
            3 => p.State = astersql_meta_model::StateWriteOnly,
            4 => c.ForeignKeys[0].OnDelete = 99,
            5 => c.ForeignKeys[0].OnUpdate = 99,
            6 => p.Columns[0].Hidden = true,
            7 => p.Columns[0].State = astersql_meta_model::StateWriteOnly,
            8 => c.ForeignKeys[0].Cols[0] = astersql_meta_model::ast::NewCIStr("missing"),
            9 => c.ForeignKeys[0].RefCols[0] = c.ForeignKeys[0].RefCols[1].clone(),
            10 => p.Indices[0].Columns[0].Offset = 999,
            11 => p.Indices.clear(),
            _ => p.Indices[0].Columns[0] = p.Indices[0].Columns[1].clone(),
        }
        let mut bad = astersql_infoschema::infoSchema::new(1);
        let models = [c, p]
            .into_iter()
            .map(|m| {
                astersql_infoschema::Table::new(astersql_infoschema::TableInfo {
                    id: m.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new(m.Name.L.clone()),
                    model_meta: Some(Arc::new(m)),
                    ..Default::default()
                })
            })
            .collect();
        bad.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("pg13_019_contract"),
                ..Default::default()
            },
            models,
        );
        let err = crate::pg_catalog::index_constraint_rows(
            "pg_constraint",
            &bad,
            "pg13_019_contract",
            &crate::conn::CancellationToken::new(),
            &[],
        )
        .unwrap_err();
        assert_eq!(
            crate::pg_conn::sqlstate(&err),
            "0A000",
            "scenario={scenario}"
        );
    }
    println!(
        "pg13_019_contract: order=1 permuted_parent=1 actions=2 match_null=1 corrupt_models=13 cleanup=1"
    );
}

#[test]
fn pg13_020_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_020_contract").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_020_contract")
            .unwrap();
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_020_contract\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "CREATE TABLE public.check_doc (v integer NOT NULL, x integer, CONSTRAINT positive CHECK (v > 0), CONSTRAINT constant_check CHECK (1=1))",
    ] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let got = query(
        &mut socket,
        "SELECT conname,contype,conkey,pg_catalog.pg_get_constraintdef(oid) FROM pg_catalog.pg_constraint WHERE conrelid='public.check_doc'::regclass::oid ORDER BY conname",
    );
    assert_eq!(
        got[0].0, b'T',
        "CHECK and NOT NULL positive catalog contract: {got:?}"
    );
    let rows: Vec<_> = got
        .iter()
        .filter(|(tag, _)| *tag == b'D')
        .map(|(_, b)| b.clone())
        .collect();
    assert_eq!(
        rows.len(),
        3,
        "CHECK constant is a constraint; NOT NULL has PG18 identity: {rows:?}"
    );
    assert_eq!(
        columns(&got[0].1)
            .iter()
            .map(|(_, oid)| *oid)
            .collect::<Vec<_>>(),
        vec![19, 18, 1005, 25]
    );
    assert_eq!(
        rows,
        vec![
            row(&[
                Some("check_doc_v_not_null"),
                Some("n"),
                Some("{1}"),
                Some("NOT NULL \"v\"")
            ]),
            row(&[
                Some("constant_check"),
                Some("c"),
                None,
                Some("CHECK (1 = 1)")
            ]),
            row(&[
                Some("positive"),
                Some("c"),
                Some("{1}"),
                Some("CHECK (\"v\" > 0)")
            ]),
        ]
    );
    let got = query(
        &mut socket,
        "SELECT pg_catalog.pg_get_constraintdef(0),pg_catalog.pg_get_constraintdef(NULL),pg_catalog.pg_get_constraintdef(0,true)",
    );
    assert!(
        got.iter()
            .any(|(tag, body)| *tag == b'D' && *body == row(&[None, None, None])),
        "{got:?}"
    );
    let got = query(
        &mut socket,
        "SELECT conbin FROM pg_catalog.pg_constraint WHERE oid=0",
    );
    assert_eq!(columns(&got[0].1)[0].1, 194);
    assert!(!got.iter().any(|(tag, _)| *tag == b'D'));
    let tables = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_020_contract"))
        .unwrap();
    let model = tables
        .iter()
        .find(|t| t.name.lower == "check_doc")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    let encoded = serde_json::to_vec(model.as_ref()).unwrap();
    let decoded: astersql_meta_model::TableInfo = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.PostgreSQLChecks.len(), 2);
    assert_eq!(
        decoded.Columns[0].PostgreSQLNotNull,
        model.Columns[0].PostgreSQLNotNull
    );
    for scenario in 0..6 {
        let mut m = decoded.clone();
        match scenario {
            0 => m.PostgreSQLChecks.clear(),
            1 => {
                m.PostgreSQLChecks.insert(
                    m.Constraints[0].ID,
                    astersql_meta_model::PostgreSQLCheckExpression::Column(999),
                );
            }
            2 => m.Columns[0].PostgreSQLNotNull = Some((0, "bad".into())),
            3 => m.Columns[0].Hidden = true,
            4 => m.Constraints[0].ID = i64::MAX,
            _ => m.Columns[0].PostgreSQLNotNull = Some((i64::MAX, "bad".into())),
        }
        let mut bad = astersql_infoschema::infoSchema::new(1);
        bad.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("pg13_020_contract"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: m.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new(m.Name.L.clone()),
                    model_meta: Some(Arc::new(m)),
                    ..Default::default()
                },
            )],
        );
        let err = crate::pg_catalog::index_constraint_rows(
            "pg_constraint",
            &bad,
            "pg13_020_contract",
            &crate::conn::CancellationToken::new(),
            &[],
        )
        .unwrap_err();
        assert_eq!(
            crate::pg_conn::sqlstate(&err),
            "0A000",
            "scenario={scenario}: {err}"
        );
    }
    println!(
        "pg13_020_contract: check=2 not_null=1 unknown_null=3 empty_describe=1 durable_roundtrip=1 corrupt_models=6 cleanup=1"
    );
}

#[test]
fn pg13_021_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_021_contract").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_021_contract")
            .unwrap();
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_021_contract\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    let got = query(
        &mut socket,
        "CREATE TABLE public.state_doc (v integer NOT NULL, CONSTRAINT state_u UNIQUE(v), CONSTRAINT state_c CHECK (v > 0))",
    );
    assert_eq!(got[0].0, b'C', "{got:?}");
    let sql = "SELECT conname,condeferrable,condeferred,convalidated,conenforced FROM pg_catalog.pg_constraint WHERE conrelid='public.state_doc'::regclass::oid ORDER BY conname";
    let got = query(&mut socket, sql);
    assert_eq!(got[0].0, b'T', "positive constraint state: {got:?}");
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![19, 16, 16, 16, 16]
    );
    let data: Vec<_> = got
        .iter()
        .filter(|(t, _)| *t == b'D')
        .map(|(_, b)| b.clone())
        .collect();
    assert_eq!(
        data,
        vec![
            row(&[Some("state_c"), Some("f"), Some("f"), Some("t"), Some("t")]),
            row(&[
                Some("state_doc_v_not_null"),
                Some("f"),
                Some("f"),
                Some("t"),
                Some("t")
            ]),
            row(&[Some("state_u"), Some("f"), Some("f"), Some("t"), Some("t")])
        ]
    );
    for sql in [
        "INSERT INTO public.state_doc VALUES (0)",
        "INSERT INTO public.state_doc VALUES (NULL)",
    ] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'E', "{got:?}");
    }
    for sql in ["BEGIN", "INSERT INTO public.state_doc VALUES (1)", "COMMIT"] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'C', "{got:?}");
    }
    let got = query(&mut socket, "INSERT INTO public.state_doc VALUES (1)");
    assert_eq!(got[0].0, b'E', "{got:?}");
    let got = query(
        &mut socket,
        &sql.replace("conrelid='public.state_doc'::regclass::oid", "oid=0"),
    );
    assert_eq!(columns(&got[0].1).len(), 5);
    assert!(!got.iter().any(|(t, _)| *t == b'D'));
    let models = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_021_contract"))
        .unwrap();
    let model = models
        .iter()
        .find(|t| t.name.lower == "state_doc")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    let encoded = serde_json::to_vec(model.as_ref()).unwrap();
    let decoded: astersql_meta_model::TableInfo = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.PostgreSQLImmediateConstraintValidation.len(), 3);
    for scenario in 0..3 {
        let mut model = decoded.clone();
        match scenario {
            0 => model.PostgreSQLImmediateConstraintValidation.clear(),
            1 => {
                for value in model.PostgreSQLImmediateConstraintValidation.values_mut() {
                    *value = false;
                }
            }
            _ => model.Partition = Some(Default::default()),
        }
        let mut schema = astersql_infoschema::infoSchema::new(1);
        schema.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("pg13_021_contract"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: model.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new("state_doc"),
                    model_meta: Some(Arc::new(model)),
                    ..Default::default()
                },
            )],
        );
        let rows = crate::pg_catalog::index_constraint_rows(
            "pg_constraint",
            &schema,
            "pg13_021_contract",
            &crate::conn::CancellationToken::new(),
            &[],
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|r| r[23] == crate::conn::Value::Null && r[24] == crate::conn::Value::Null)
        );
    }
    native
        .execute("CREATE TABLE pg13_021_contract.legacy_state (id INT PRIMARY KEY)")
        .unwrap();
    for field in [
        "convalidated",
        "conenforced",
        "condeferrable",
        "condeferred",
        "connoinherit",
        "conislocal",
        "coninhcount",
        "conparentid",
        "contypid",
        "conperiod",
    ] {
        let bad = query(
            &mut socket,
            &format!(
                "SELECT {field} FROM pg_catalog.pg_constraint WHERE conrelid='public.legacy_state'::regclass::oid"
            ),
        );
        assert_eq!(bad[0].0, b'E', "{field}: {bad:?}");
        assert!(bad[0].1.windows(6).any(|b| b == b"0A000\0"));
    }
    assert_eq!(query(&mut socket, "SELECT 1")[0].0, b'T');
    println!(
        "pg13_021_contract: states=3 violations=3 transaction_commit=1 empty_describe=1 durable_roundtrip=1 corrupt_models=3 legacy_reject_recover=1 cleanup=1"
    );
}

#[test]
fn pg13_022_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_022_contract").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_022_contract")
            .unwrap();
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_022_contract\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    for sql in [
        "CREATE TABLE public.def_doc (plain integer,n integer DEFAULT NULL,literal text DEFAULT 'NULL',qty integer DEFAULT 7,label text DEFAULT 'O''Reilly',flag boolean DEFAULT true)",
    ] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'C', "{got:?}");
    }
    let sql = "SELECT a.attnum,a.attname,a.atthasdef,d.oid,d.adrelid,d.adnum FROM pg_catalog.pg_attribute a LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid='public.def_doc'::regclass::oid ORDER BY a.attnum";
    let got = query(&mut socket, sql);
    assert_eq!(
        got[0].0, b'T',
        "default identity positive contract: {got:?}"
    );
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![21, 19, 16, 26, 26, 21]
    );
    let data: Vec<_> = got.iter().filter(|(t, _)| *t == b'D').collect();
    assert_eq!(data.len(), 6);
    assert_eq!(
        data[0].1,
        row(&[Some("1"), Some("plain"), Some("f"), None, None, None])
    );
    assert_eq!(
        data[1].1,
        row(&[Some("2"), Some("n"), Some("f"), None, None, None])
    );
    let before = query(
        &mut socket,
        "SELECT oid FROM pg_catalog.pg_attrdef WHERE adrelid='public.def_doc'::regclass::oid AND adnum=4",
    );
    for sql in [
        "ALTER TABLE public.def_doc ALTER COLUMN qty SET DEFAULT 9",
        "ALTER TABLE public.def_doc ALTER COLUMN label DROP DEFAULT",
        "ALTER TABLE public.def_doc RENAME COLUMN qty TO quantity",
    ] {
        let got = query(&mut socket, sql);
        assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
    }
    let after = query(
        &mut socket,
        "SELECT oid FROM pg_catalog.pg_attrdef WHERE adrelid='public.def_doc'::regclass::oid AND adnum=4",
    );
    assert_ne!(
        before.iter().find(|(t, _)| *t == b'D'),
        after.iter().find(|(t, _)| *t == b'D')
    );
    let empty = query(
        &mut socket,
        "SELECT oid,adrelid,adnum FROM pg_catalog.pg_attrdef WHERE adrelid=0",
    );
    assert_eq!(
        columns(&empty[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![26, 26, 21]
    );
    assert!(!empty.iter().any(|(t, _)| *t == b'D'));
    let models = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_022_contract"))
        .unwrap();
    let model = models
        .iter()
        .find(|t| t.name.lower == "def_doc")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    let encoded = serde_json::to_vec(model.as_ref()).unwrap();
    let decoded: astersql_meta_model::TableInfo = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        decoded
            .Columns
            .iter()
            .filter(|c| c.PostgreSQLHasDefault == Some(true))
            .count(),
        3
    );
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
    for scenario in 0..5 {
        let mut bad = decoded.clone();
        let column = &mut bad.Columns[3];
        match scenario {
            0 => column.PostgreSQLDefaultID = None,
            1 => column.PostgreSQLDefaultID = Some(0),
            2 => column.PostgreSQLDefaultID = Some(i64::MAX),
            3 => column.PostgreSQLHasDefault = Some(false),
            _ => bad.Columns[3].PostgreSQLDefaultID = bad.Columns[2].PostgreSQLDefaultID,
        }
        let mut schema = astersql_infoschema::infoSchema::new(1);
        schema.add_schema(
            astersql_infoschema::DBInfo {
                id: 42,
                name: astersql_infoschema::CiString::new("pg13_022_contract"),
                ..Default::default()
            },
            vec![astersql_infoschema::Table::new(
                astersql_infoschema::TableInfo {
                    id: bad.ID,
                    db_id: 42,
                    name: astersql_infoschema::CiString::new("def_doc"),
                    model_meta: Some(Arc::new(bad)),
                    ..Default::default()
                },
            )],
        );
        assert!(
            crate::pg_catalog::column_catalog_rows(
                "pg_attrdef",
                &schema,
                "pg13_022_contract",
                &crate::conn::CancellationToken::new()
            )
            .is_err()
        );
    }
    println!(
        "pg13_022_contract: base_columns=6 explicit_null_normalized=1 set_drop_rename=3 empty_describe=1 durable_roundtrip=1 corrupt_models=5 cleanup=1"
    );
}

#[test]
fn pg13_023_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_023_contract").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_023_contract")
            .unwrap();
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_023_contract\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    let created = query(
        &mut socket,
        "CREATE TABLE public.def_doc (qty integer DEFAULT 7, label text DEFAULT 'O''Reilly', flag boolean DEFAULT true)",
    );
    assert_eq!(created[0].0, b'C', "{created:?}");
    let got = query(
        &mut socket,
        "SELECT adbin,pg_get_expr(adbin,0),pg_get_expr(adbin,adrelid,true) FROM pg_attrdef WHERE adrelid='public.def_doc'::regclass::oid ORDER BY adnum",
    );
    assert_eq!(got[0].0, b'T', "safe deparse positive contract: {got:?}");
    assert_eq!(
        columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![194, 25, 25]
    );
    let data: Vec<_> = got.iter().filter(|(t, _)| *t == b'D').collect();
    assert_eq!(data.len(), 3);
    for expected in ["7", "'O''Reilly'::text", "true"] {
        assert!(
            data.iter()
                .any(|(_, body)| body.ends_with(&row(&[Some(expected), Some(expected)])[2..])),
            "{got:?}"
        );
    }
    let got = query(
        &mut socket,
        "SELECT pg_get_expr(NULL,0),pg_get_expr(NULL,0,false) FROM pg_type LIMIT 1",
    );
    assert_eq!(got[1], (b'D', row(&[None, None])));
    for sql in [
        "SELECT pg_get_expr('fake',0) FROM pg_type LIMIT 1",
        "SELECT pg_get_expr('0:7',0) FROM pg_type LIMIT 1",
        "SELECT pg_get_expr(NULL,0,false,0) FROM pg_type LIMIT 1",
    ] {
        let got = query(&mut socket, sql);
        assert!(got.iter().any(|(t, _)| *t == b'E'), "{sql}: {got:?}");
        assert!(!got.iter().any(|(t, _)| *t == b'D'));
        assert_eq!(got.last().unwrap(), &(b'Z', vec![b'I']));
    }
    assert_eq!(
        query(&mut socket, "SELECT oid FROM pg_type WHERE oid=23")[1],
        (b'D', row(&[Some("23")]))
    );
    native.execute("CREATE TABLE pg13_023_contract.def_native (x INT,y INT GENERATED ALWAYS AS (IFNULL(x,0)) STORED)").unwrap();
    let rejected = query(
        &mut socket,
        "SELECT adbin FROM pg_attrdef WHERE adrelid='public.def_native'::regclass::oid",
    );
    assert!(
        rejected
            .iter()
            .any(|(t, b)| *t == b'E' && b.windows(6).any(|w| w == b"0A000\0")),
        "{rejected:?}"
    );
    assert!(!rejected.iter().any(|(t, _)| *t == b'D'));
    native
        .execute("DROP TABLE pg13_023_contract.def_native")
        .unwrap();
    assert_eq!(
        query(&mut socket, "SELECT pg_get_expr(NULL,0)")[1],
        (b'D', row(&[None]))
    );
    send(&mut socket, b'X', b"");
    drop(socket);
    println!(
        "pg13_023_contract: constants=3 null=2 invalid_recovery=3 native_generated_reject=1 pg_node_tree_describe=1 cleanup=1"
    );
}

#[test]
fn pg13_024_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_024_contract").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_024_contract")
            .unwrap();
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_024_contract\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    for (mode, flag) in [("STORED", "s"), ("VIRTUAL", "v")] {
        let name = format!("gen_{}", mode.to_ascii_lowercase());
        for sql in [
            format!(
                "CREATE TABLE public.{name} (x integer,y integer GENERATED ALWAYS AS (x+1) {mode}, d integer DEFAULT 7)"
            ),
            format!("INSERT INTO public.{name}(x) VALUES (2),(NULL)"),
        ] {
            let got = query(&mut socket, &sql);
            assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
        }
        let got = query(
            &mut socket,
            &format!("SELECT x,y,d FROM public.{name} ORDER BY x"),
        );
        assert_eq!(got[1], (b'D', row(&[Some("2"), Some("3"), Some("7")])));
        assert_eq!(got[2], (b'D', row(&[None, None, Some("7")])));
        let sql = format!(
            "SELECT a.attnum,a.attgenerated,a.atthasdef,d.oid,d.adnum,pg_get_expr(d.adbin,d.adrelid) FROM pg_attribute a LEFT JOIN pg_attrdef d ON a.attrelid=d.adrelid AND a.attnum=d.adnum WHERE a.attrelid='public.{name}'::regclass::oid ORDER BY a.attnum"
        );
        let got = query(&mut socket, &sql);
        assert_eq!(
            got[0].0, b'T',
            "generated catalog positive contract: {got:?}"
        );
        assert_eq!(
            columns(&got[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
            vec![21, 18, 16, 26, 21, 25]
        );
        let data: Vec<_> = got.iter().filter(|(t, _)| *t == b'D').collect();
        assert_eq!(data.len(), 3);
        assert_eq!(
            data[0].1,
            row(&[Some("1"), Some(""), Some("f"), None, None, None])
        );
        assert!(data[1].1.windows(flag.len()).any(|s| s == flag.as_bytes()));
        for sql in [
            format!("UPDATE public.{name} SET x=4 WHERE x=2"),
            format!("ALTER TABLE public.{name} RENAME COLUMN x TO renamed_x"),
            format!("INSERT INTO public.{name}(renamed_x) VALUES (6)"),
        ] {
            let got = query(&mut socket, &sql);
            assert_eq!(got[0].0, b'C', "{sql}: {got:?}");
        }
        let got = query(
            &mut socket,
            &format!("SELECT renamed_x,y FROM public.{name} WHERE renamed_x=6"),
        );
        assert_eq!(got[1], (b'D', row(&[Some("6"), Some("7")])));
        let got = query(&mut socket, &sql);
        assert_eq!(got[0].0, b'T', "{got:?}");
        let models = domain
            .info_schema()
            .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_024_contract"))
            .unwrap();
        let model = models
            .iter()
            .find(|t| t.name.lower == name)
            .unwrap()
            .model_meta
            .as_ref()
            .unwrap();
        assert!(model.Columns[1].PostgreSQLDefaultID.is_some());
        assert_eq!(model.Columns[1].PostgreSQLHasDefault, Some(true));
        let bytes = serde_json::to_vec(model.as_ref()).unwrap();
        let restored: astersql_meta_model::TableInfo = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(bytes, serde_json::to_vec(&restored).unwrap());
        assert_ne!(
            model.Columns[1].PostgreSQLDefaultID,
            model.Columns[2].PostgreSQLDefaultID
        );
        for scenario in 0..4 {
            let mut bad = restored.clone();
            match scenario {
                0 => bad.Columns[1].PostgreSQLDefaultID = None,
                1 => bad.Columns[1].PostgreSQLGenerated = None,
                2 => {
                    bad.Columns[1].PostgreSQLGenerated =
                        Some(astersql_meta_model::PostgreSQLCheckExpression::Column(999))
                }
                _ => bad.Columns[1].PostgreSQLHasDefault = Some(false),
            }
            let mut schema = astersql_infoschema::infoSchema::new(1);
            schema.add_schema(
                astersql_infoschema::DBInfo {
                    id: 42,
                    name: astersql_infoschema::CiString::new("pg13_024_contract"),
                    ..Default::default()
                },
                vec![astersql_infoschema::Table::new(
                    astersql_infoschema::TableInfo {
                        id: bad.ID,
                        db_id: 42,
                        name: astersql_infoschema::CiString::new(&name),
                        model_meta: Some(Arc::new(bad)),
                        ..Default::default()
                    },
                )],
            );
            assert!(
                crate::pg_catalog::column_catalog_rows(
                    "pg_attrdef",
                    &schema,
                    "pg13_024_contract",
                    &crate::conn::CancellationToken::new()
                )
                .is_err(),
                "corrupt generated model scenario {scenario}"
            );
        }
    }
    let rejected = query(
        &mut socket,
        "CREATE TABLE public.bad_gen (x integer,y integer GENERATED ALWAYS AS (IFNULL(x,0)) STORED)",
    );
    assert!(
        rejected
            .iter()
            .any(|(t, b)| *t == b'E' && b.windows(6).any(|b| b == b"42883\0")),
        "{rejected:?}"
    );
    assert_eq!(
        query(
            &mut socket,
            "SELECT relname FROM pg_class WHERE relname='bad_gen'"
        )
        .iter()
        .filter(|(t, _)| *t == b'D')
        .count(),
        0
    );
    let unsupported = query(
        &mut socket,
        "CREATE TABLE public.unsupported_gen(x integer,y integer GENERATED ALWAYS AS(x*2) STORED)",
    );
    assert!(
        unsupported
            .iter()
            .any(|(t, b)| *t == b'E' && b.windows(6).any(|b| b == b"0A000\0")),
        "{unsupported:?}"
    );
    assert!(
        !query(
            &mut socket,
            "SELECT oid FROM pg_class WHERE relname='unsupported_gen'"
        )
        .iter()
        .any(|(t, _)| *t == b'D')
    );
    native.execute("CREATE TABLE pg13_024_contract.native_gen(x INT,y INT GENERATED ALWAYS AS(IFNULL(x,0)) STORED)").unwrap();
    let rejected = query(
        &mut socket,
        "SELECT adbin FROM pg_attrdef WHERE adrelid='public.native_gen'::regclass::oid",
    );
    assert!(
        rejected
            .iter()
            .any(|(t, b)| *t == b'E' && b.windows(6).any(|b| b == b"0A000\0")),
        "{rejected:?}"
    );
    native
        .execute("DROP TABLE pg13_024_contract.native_gen")
        .unwrap();
    let empty = query(
        &mut socket,
        "SELECT oid,adrelid,adnum,adbin FROM pg_attrdef WHERE adrelid=0",
    );
    assert_eq!(
        columns(&empty[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![26, 26, 21, 194]
    );
    assert!(!empty.iter().any(|(t, _)| *t == b'D'));
    send(&mut socket, b'X', b"");
    drop(socket);
    println!(
        "pg13_024_contract: modes=2 insert_null=2 update=2 rename=2 durable_identity=2 serde=2 corrupt_models=8 native_generated_reject=1 unsupported_ddl=1 reject_recovery=1 empty_describe=1 cleanup=1"
    );
}

// Bound view creation, introspection, and execution share the same model.
#[test]
fn pg13_025_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_025_probe").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_025_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err(), "listener leaked");
        println!("TASK25_CLEANUP database_dropped=true listener_closed=true");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_025_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let got = query(socket, sql);
        assert!(!got.iter().any(|(t, _)| *t == b'E'), "{sql}: {got:?}");
        assert_eq!(got.last(), Some(&(b'Z', vec![b'I'])));
        got
    }
    fn definition(socket: &mut TcpStream, name: &str) -> String {
        let got = ok(
            socket,
            &format!("SELECT pg_get_viewdef(c.oid) FROM pg_class c WHERE c.relname='{name}'"),
        );
        assert_eq!(columns(&got[0].1)[0].1, 25);
        let rows: Vec<_> = got.iter().filter(|(t, _)| *t == b'D').collect();
        assert_eq!(rows.len(), 1);
        let len = i32::from_be_bytes(rows[0].1[2..6].try_into().unwrap());
        assert!(len > 0);
        String::from_utf8(rows[0].1[6..6 + len as usize].to_vec()).unwrap()
    }
    for sql in [
        "CREATE TABLE public.view_base (id integer,label text)",
        "INSERT INTO public.view_base VALUES (1,'AS WITH LOCAL CHECK OPTION'),(2,NULL)",
        "CREATE VIEW public.\"ViewAS\" (\"ID\",\"Label\") AS /* AS ignored */ SELECT id AS \"ID\",label AS \"Label\" FROM public.view_base WHERE id>0 WITH LOCAL CHECK OPTION",
        "CREATE VIEW public.view_simple AS SELECT id,label FROM public.view_base WHERE id>0",
    ] {
        ok(&mut socket, sql);
    }
    // Task 41: a non-public source is rewritten to a storage name plus its
    // logical range alias. Binding must keep that source's persistent identity.
    for sql in [
        "CREATE SCHEMA restart_view",
        "CREATE TABLE restart_view.base (id integer,qty integer)",
        "INSERT INTO restart_view.base VALUES (7,9)",
        "CREATE VIEW restart_view.v AS SELECT id,qty FROM restart_view.base",
        "CREATE VIEW restart_view.aliased AS SELECT p.id,p.qty FROM restart_view.base AS p",
    ] {
        ok(&mut socket, sql);
    }
    for name in ["v", "aliased"] {
        let got = ok(&mut socket, &format!("SELECT * FROM restart_view.{name}"));
        assert_eq!(got[1], (b'D', row(&[Some("7"), Some("9")])));
    }
    let original = ok(&mut socket, "SELECT * FROM public.\"ViewAS\"");
    assert_eq!(
        columns(&original[0].1),
        vec![("ID".into(), 23), ("Label".into(), 25)]
    );
    assert_eq!(
        original[1],
        (b'D', row(&[Some("1"), Some("AS WITH LOCAL CHECK OPTION")]))
    );
    assert_eq!(original[2], (b'D', row(&[Some("2"), None])));
    // Retain the original red input: view_simple's returned SELECT must create
    // an executable clone through the same PostgreSQL listener.
    let simple_definition = definition(&mut socket, "view_simple");
    ok(
        &mut socket,
        &format!("CREATE VIEW public.view_clone AS {simple_definition}"),
    );
    let simple = ok(&mut socket, "SELECT * FROM public.view_simple");
    let cloned_simple = ok(&mut socket, "SELECT * FROM public.view_clone");
    assert_eq!(columns(&simple[0].1), columns(&cloned_simple[0].1));
    assert_eq!(
        simple
            .iter()
            .filter(|(t, _)| *t == b'D')
            .collect::<Vec<_>>(),
        cloned_simple
            .iter()
            .filter(|(t, _)| *t == b'D')
            .collect::<Vec<_>>()
    );
    ok(&mut socket, "DROP VIEW public.view_clone");
    let mut scenarios = 2;
    for (name, sql, expected) in [
        (
            "as",
            "CREATE VIEW public.\"as\" AS SELECT 'AS WITH CASCADED CHECK OPTION' AS \"aS\"",
            25,
        ),
        (
            "view_cascade",
            "CREATE VIEW public.view_cascade AS SELECT id FROM public.view_base WHERE id>0 WITH CASCADED CHECK OPTION",
            23,
        ),
        (
            "view_default",
            "CREATE VIEW public.view_default AS SELECT id FROM public.view_base WHERE id>0 WITH CHECK OPTION",
            23,
        ),
    ] {
        ok(&mut socket, sql);
        let def = definition(&mut socket, name);
        ok(
            &mut socket,
            &format!("CREATE VIEW public.clone_lex AS {def}"),
        );
        let source = ok(&mut socket, &format!("SELECT * FROM public.\"{name}\""));
        let clone = ok(&mut socket, "SELECT * FROM public.clone_lex");
        assert_eq!(columns(&source[0].1), columns(&clone[0].1));
        assert_eq!(columns(&clone[0].1)[0].1, expected);
        assert_eq!(
            source
                .iter()
                .filter(|(t, _)| *t == b'D')
                .collect::<Vec<_>>(),
            clone.iter().filter(|(t, _)| *t == b'D').collect::<Vec<_>>()
        );
        ok(&mut socket, "DROP VIEW public.clone_lex");
        scenarios += 1;
    }
    for sql in [
        "CREATE SCHEMA view_shadow",
        "CREATE TABLE view_shadow.view_base(id integer,label text)",
        "INSERT INTO view_shadow.view_base VALUES(99,'shadow')",
        "SET search_path TO view_shadow,public,pg_catalog",
    ] {
        ok(&mut socket, sql);
    }
    let def = definition(&mut socket, "ViewAS");
    ok(
        &mut socket,
        &format!("CREATE VIEW public.view_clone AS {def}"),
    );
    let clone = ok(&mut socket, "SELECT * FROM public.view_clone");
    assert_eq!(columns(&original[0].1), columns(&clone[0].1));
    assert_eq!(
        original
            .iter()
            .filter(|(t, _)| *t == b'D')
            .collect::<Vec<_>>(),
        clone.iter().filter(|(t, _)| *t == b'D').collect::<Vec<_>>()
    );
    let shadow = ok(&mut socket, "SELECT * FROM view_shadow.view_base");
    assert_eq!(shadow[1], (b'D', row(&[Some("99"), Some("shadow")])));
    ok(&mut socket, "SET search_path TO public,pg_catalog");
    ok(
        &mut socket,
        "ALTER TABLE public.view_base RENAME COLUMN label TO renamed_label",
    );
    assert!(definition(&mut socket, "ViewAS").contains("renamed_label"));
    let after = ok(&mut socket, "SELECT * FROM public.\"ViewAS\"");
    assert_eq!(original, after);
    scenarios += 2;
    let models = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_025_probe"))
        .unwrap();
    let view = models
        .iter()
        .find(|t| t.name.original == "ViewAS")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    let base = models
        .iter()
        .find(|t| t.name.lower == "view_base")
        .unwrap()
        .model_meta
        .as_ref()
        .unwrap();
    let serialized = serde_json::to_vec(view.as_ref()).unwrap();
    let restored: astersql_meta_model::TableInfo = serde_json::from_slice(&serialized).unwrap();
    assert_eq!(serialized, serde_json::to_vec(&restored).unwrap());
    definition(&mut socket, "ViewAS");
    let fresh_models = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_025_probe"))
        .unwrap();
    let fresh_view = fresh_models
        .iter()
        .filter_map(|t| t.model_meta.as_ref())
        .find(|t| t.ID == restored.ID)
        .unwrap();
    assert_eq!(
        serialized,
        serde_json::to_vec(fresh_view.as_ref()).unwrap(),
        "introspection must not migrate or mutate shared view metadata"
    );
    assert!(
        restored
            .PostgreSQLView
            .as_ref()
            .unwrap()
            .render(&restored, Some(base), "public", false)
            .is_some()
    );
    for n in 0..5 {
        let mut bad = restored.clone();
        match n {
            0 => bad.View.as_mut().unwrap().SelectStmt = "DROP TABLE view_base".into(),
            1 => bad.View.as_mut().unwrap().SelectStmt = "SELECT 1; SELECT 2".into(),
            2 => bad.PostgreSQLView.as_mut().unwrap().RelationID = Some(-1),
            3 => bad.PostgreSQLView.as_mut().unwrap().RelationColumns[0].0 = -1,
            _ => bad.View.as_mut().unwrap().Cols[0] = astersql_meta_model::ast::NewCIStr("bad"),
        }
        assert!(
            bad.PostgreSQLView
                .as_ref()
                .unwrap()
                .render(&bad, Some(base), "public", false)
                .is_none(),
            "corrupt model {n}"
        );
    }
    native.execute("CREATE VIEW pg13_025_probe.view_native AS SELECT IFNULL(renamed_label,'x') AS label FROM pg13_025_probe.view_base").unwrap();
    let rejected = query(
        &mut socket,
        "SELECT pg_get_viewdef(c.oid) FROM pg_class c WHERE c.relname='view_native'",
    );
    assert!(
        rejected
            .iter()
            .any(|(t, b)| *t == b'E' && b.windows(6).any(|s| s == b"0A000\0")),
        "{rejected:?}"
    );
    assert!(!rejected.iter().any(|(t, _)| *t == b'D'));
    let empty = ok(
        &mut socket,
        "SELECT pg_get_viewdef(c.oid) FROM pg_class c WHERE c.oid=0",
    );
    assert_eq!(columns(&empty[0].1)[0].1, 25);
    assert!(!empty.iter().any(|(t, _)| *t == b'D'));
    let nulls = ok(
        &mut socket,
        "SELECT pg_get_viewdef(NULL::oid),pg_get_viewdef(0::oid)",
    );
    assert_eq!(nulls[1], (b'D', row(&[None, None])));
    // Removing a dependency must never retarget an old view to a same-name table.
    for sql in [
        "DROP TABLE public.view_base",
        "CREATE TABLE public.view_base(id integer,label text)",
        "INSERT INTO public.view_base VALUES(77,'replacement')",
    ] {
        ok(&mut socket, sql);
    }
    for sql in [
        "SELECT * FROM public.view_simple",
        "SELECT pg_get_viewdef(c.oid) FROM pg_class c WHERE c.relname='view_simple'",
    ] {
        let got = query(&mut socket, sql);
        assert!(
            got.iter()
                .any(|(t, b)| *t == b'E' && b.windows(6).any(|s| s == b"0A000\0")),
            "{sql}: {got:?}"
        );
        assert!(!got.iter().any(|(t, _)| *t == b'D'));
    }
    ok(&mut socket, "SELECT 1");
    send(&mut socket, b'X', b"");
    drop(socket);
    println!(
        "pg13_025_contract: positive_scenarios={scenarios} corrupt_models=5 native_reject=1 missing_dependency=2 null=2 empty_describe=1 recovery=1 serde=1 cleanup=1"
    );
}

// Task 26 exercises real bound view identity and refresh.
#[test]
fn pg13_026_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE pg13_026_probe").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_026_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err(), "listener leaked");
        println!("TASK26_CLEANUP database_dropped=true listener_closed=true");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_026_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ok(s: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let r = query(s, sql);
        assert!(r.iter().all(|m| m.0 != b'E'), "{sql}: {r:?}");
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        r
    }
    fn scalar(s: &mut TcpStream, sql: &str) -> Option<String> {
        let r = ok(s, sql);
        assert_eq!(r.iter().filter(|m| m.0 == b'D').count(), 1);
        let d = &r.iter().find(|m| m.0 == b'D').unwrap().1;
        let n = i32::from_be_bytes(d[2..6].try_into().unwrap());
        (n >= 0).then(|| String::from_utf8(d[6..6 + n as usize].to_vec()).unwrap())
    }
    fn execute(s: &mut TcpStream) -> Vec<(u8, Vec<u8>)> {
        send(
            s,
            b'B',
            &[
                b"\0refresh\0".as_slice(),
                &0u16.to_be_bytes(),
                &0u16.to_be_bytes(),
                &0u16.to_be_bytes(),
            ]
            .concat(),
        );
        send(s, b'D', b"P\0");
        send(s, b'E', &[b"\0".as_slice(), &0u32.to_be_bytes()].concat());
        send(s, b'S', b"");
        let r = until_ready(s);
        assert!(r.iter().all(|m| m.0 != b'E'), "{r:?}");
        r
    }
    ok(
        &mut socket,
        "CREATE TABLE public.view_base(id integer,label text)",
    );
    ok(
        &mut socket,
        "INSERT INTO public.view_base VALUES(1,'AS WITH LOCAL CHECK OPTION')",
    );
    ok(
        &mut socket,
        "CREATE VIEW public.\"ViewAS\" (\"ID\",\"Label\") AS SELECT id,label FROM public.view_base WHERE id>0 WITH LOCAL CHECK OPTION",
    );
    let model = || {
        domain
            .info_schema()
            .SchemaTableInfos(&astersql_infoschema::CiString::new("pg13_026_probe"))
            .unwrap()
            .into_iter()
            .filter_map(|t| t.model_meta.clone())
            .find(|t| t.View.is_some())
            .unwrap()
    };
    let original = model();
    let oid = scalar(
        &mut socket,
        "SELECT oid FROM pg_class WHERE relname='ViewAS'",
    )
    .unwrap();
    for option in ["", ",false", ",true", ",40"] {
        let def = scalar(
            &mut socket,
            &format!("SELECT pg_get_viewdef({oid}::oid{option})"),
        )
        .unwrap();
        ok(&mut socket, &format!("CREATE VIEW public.clone AS {def}"));
        let clone = ok(&mut socket, "SELECT * FROM public.clone");
        assert_eq!(
            columns(&clone[0].1),
            vec![("ID".into(), 23), ("Label".into(), 25)]
        );
        assert_eq!(
            clone[1],
            (b'D', row(&[Some("1"), Some("AS WITH LOCAL CHECK OPTION")]))
        );
        ok(&mut socket, "DROP VIEW public.clone");
    }
    assert_eq!(
        scalar(
            &mut socket,
            "SELECT pg_get_viewdef('public.\"ViewAS\"'::text,true)"
        ),
        scalar(&mut socket, &format!("SELECT pg_get_viewdef({oid}::oid)"))
    );
    for arg in ["0::oid", "NULL::oid", "'public.view_base'::regclass::oid"] {
        assert_eq!(
            scalar(&mut socket, &format!("SELECT pg_get_viewdef({arg})")),
            None
        );
    }
    assert_eq!(
        scalar(
            &mut socket,
            &format!("SELECT pg_get_viewdef({oid}::oid,NULL::boolean)")
        ),
        None
    );
    let sql = format!("SELECT pg_get_viewdef({oid}::oid) AS definition");
    send(
        &mut socket,
        b'P',
        &[
            b"refresh\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Srefresh\0");
    send(&mut socket, b'S', b"");
    let parsed = until_ready(&mut socket);
    assert_eq!(parsed.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ");
    assert_eq!(columns(&parsed[2].1), vec![("definition".into(), 25)]);
    ok(
        &mut socket,
        "CREATE OR REPLACE VIEW public.\"ViewAS\" (\"ID\",\"Label\") AS SELECT id,label FROM public.view_base WHERE id>1 WITH LOCAL CHECK OPTION",
    );
    let updated = model();
    assert_eq!(updated.ID, original.ID);
    assert_eq!(updated.PostgreSQLOwner, original.PostgreSQLOwner);
    assert_eq!(
        updated.PostgreSQLAttributeSlots,
        original.PostgreSQLAttributeSlots
    );
    let refreshed = execute(&mut socket);
    assert!(
        refreshed
            .iter()
            .any(|m| m.0 == b'D' && String::from_utf8_lossy(&m.1).contains("> 1")),
        "{refreshed:?}"
    );
    let renamed = ok(
        &mut socket,
        "ALTER VIEW public.\"ViewAS\" RENAME TO \"RenamedAS\"",
    );
    assert_eq!(renamed[0], (b'C', b"ALTER VIEW\0".to_vec()));
    assert_eq!(
        scalar(
            &mut socket,
            "SELECT oid FROM pg_class WHERE relname='RenamedAS'"
        ),
        Some(oid.clone())
    );
    ok(
        &mut socket,
        "ALTER TABLE public.view_base RENAME COLUMN label TO renamed_label",
    );
    assert!(
        scalar(&mut socket, &format!("SELECT pg_get_viewdef({oid}::oid)"))
            .unwrap()
            .contains("renamed_label")
    );
    for sql in [
        "CREATE OR REPLACE VIEW public.\"RenamedAS\" AS SELECT id AS changed,renamed_label AS \"Label\" FROM public.view_base",
        "CREATE OR REPLACE VIEW public.\"RenamedAS\" AS SELECT id AS \"ID\" FROM public.view_base",
    ] {
        let r = query(&mut socket, sql);
        assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
        assert!(r[0].1.windows(5).any(|b| b == b"42P16"), "{r:?}");
        assert_eq!(scalar(&mut socket, "SELECT 1"), Some("1".into()));
        assert!(
            scalar(&mut socket, &format!("SELECT pg_get_viewdef({oid}::oid)"))
                .unwrap()
                .contains("> 1")
        );
    }
    ok(
        &mut socket,
        "CREATE OR REPLACE VIEW public.\"RenamedAS\" AS SELECT id AS \"ID\",renamed_label AS \"Label\",id AS extra FROM public.view_base WHERE id>1",
    );
    let appended = model();
    assert_eq!(appended.Columns.len(), 3);
    assert_eq!(appended.Columns[0].ID, original.Columns[0].ID);
    ok(&mut socket, "DROP VIEW public.\"RenamedAS\"");
    assert_eq!(
        scalar(&mut socket, &format!("SELECT pg_get_viewdef({oid}::oid)")),
        None
    );
    ok(
        &mut socket,
        "CREATE VIEW public.\"RenamedAS\" AS SELECT 9 AS n",
    );
    assert_ne!(model().ID, original.ID);
    let gone = execute(&mut socket);
    assert!(gone.contains(&(b'D', row(&[None]))));
    let empty = ok(
        &mut socket,
        "SELECT pg_get_viewdef(oid) FROM pg_class WHERE oid=0",
    );
    assert_eq!(columns(&empty[0].1)[0].1, 25);
    assert!(empty.iter().all(|m| m.0 != b'D'));
    let r = query(&mut socket, "SELECT pg_get_viewdef(0::oid,true,false)");
    assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert_eq!(scalar(&mut socket, "SELECT 1"), Some("1".into()));
    send(&mut socket, b'X', b"");
    drop(socket);
    println!(
        "pg13_026_contract: signatures=5 null=4 refresh=1 rename=1 base_rename=1 replace_reject=2 append=1 drop_recreate=1 empty_describe=1 recovery=3 shared_model={} cleanup=guard",
        original.ID
    );
}

// Task 27: real shared metadata and TCP sequence declaration contract.
#[test]
fn pg13_027_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_027_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_027_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG27 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_027_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}
    let mut typed_created = 0;
    for (sql, typed) in [
        (
            "CREATE SEQUENCE public.probe_i AS integer START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE",
            true,
        ),
        (
            "CREATE SEQUENCE public.probe_b AS bigint START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE",
            true,
        ),
        (
            "CREATE SEQUENCE public.probe_default START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE",
            false,
        ),
        (
            "CREATE SEQUENCE public.probe_bad MINVALUE 10 MAXVALUE 1",
            false,
        ),
        (
            "CREATE SEQUENCE public.probe_big_edge AS bigint MINVALUE 1 MAXVALUE 9223372036854775807",
            false,
        ),
        (
            "CREATE SEQUENCE public.probe_small_range AS bigint START WITH 7 MINVALUE 1 MAXVALUE 2147483647 CACHE 2 CYCLE",
            false,
        ),
        (
            "CREATE TABLE public.probe_identity(id integer GENERATED ALWAYS AS IDENTITY)",
            false,
        ),
        (
            "SELECT c.oid,c.relname,c.relkind,s.seqtypid,s.seqstart,s.seqincrement,s.seqmin,s.seqmax,s.seqcache,s.seqcycle FROM pg_catalog.pg_class c JOIN pg_catalog.pg_sequence s ON c.oid=s.seqrelid ORDER BY c.relname",
            false,
        ),
        ("SELECT nextval('public.probe_default'::regclass)", false),
        (
            "SELECT seqtypid,seqstart,seqincrement,seqmin,seqmax,seqcache,seqcycle FROM pg_catalog.pg_sequence",
            false,
        ),
        ("SELECT currval('public.probe_default'::regclass)", false),
        ("SELECT nextval('public.probe_default'::regclass)", false),
    ] {
        let messages = query(&mut socket, sql);
        println!("PG27 SQL={sql}");
        for (tag, body) in &messages {
            println!(
                "PG27 wire={} bytes={body:?} text={}",
                *tag as char,
                String::from_utf8_lossy(body)
            );
        }
        assert_eq!(messages.last(), Some(&(b'Z', vec![b'I'])));
        if typed && messages.iter().all(|m| m.0 != b'E') {
            typed_created += 1;
        }
    }
    let tables = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new(
            "pg13_027_contract_probe",
        ))
        .unwrap();
    for table in tables {
        if let Some(model) = &table.model_meta {
            println!("PG27 MODEL={}", serde_json::to_string(model).unwrap());
        }
    }
    println!("PG27 scenarios=12 typed_create_success={typed_created}/2");
    assert_eq!(
        typed_created, 2,
        "required standalone AS integer/bigint capability missing"
    );
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let out = query(socket, sql);
        assert!(out.iter().all(|m| m.0 != b'E'), "{sql}: {out:?}");
        assert_eq!(out.last(), Some(&(b'Z', vec![b'I'])));
        out
    }
    fn data(out: &[(u8, Vec<u8>)]) -> Vec<Vec<Option<String>>> {
        out.iter()
            .filter(|m| m.0 == b'D')
            .map(|m| {
                let mut offset = 2;
                (0..u16::from_be_bytes(m.1[..2].try_into().unwrap()))
                    .map(|_| {
                        let size = i32::from_be_bytes(m.1[offset..offset + 4].try_into().unwrap());
                        offset += 4;
                        if size < 0 {
                            None
                        } else {
                            let v = String::from_utf8(m.1[offset..offset + size as usize].to_vec())
                                .unwrap();
                            offset += size as usize;
                            Some(v)
                        }
                    })
                    .collect()
            })
            .collect()
    }
    let projection = "seqrelid,seqtypid,seqstart,seqincrement,seqmin,seqmax,seqcache,seqcycle";
    for (name, typ, start, inc, min, max, cache, cycle) in [
        ("probe_i", "23", "7", "3", "1", "100", "1", "f"),
        ("probe_b", "20", "7", "3", "1", "100", "1", "f"),
        (
            "probe_identity__identity_1",
            "23",
            "1",
            "1",
            "1",
            "2147483647",
            "1",
            "f",
        ),
        (
            "probe_small_range",
            "20",
            "7",
            "1",
            "1",
            "2147483647",
            "2",
            "t",
        ),
    ] {
        let class = ok(
            &mut socket,
            &format!("SELECT oid FROM pg_class WHERE relname='{name}'"),
        );
        let oid = data(&class)[0][0].clone().unwrap();
        let result = ok(
            &mut socket,
            &format!("SELECT {projection} FROM pg_catalog.pg_sequence WHERE seqrelid={oid}"),
        );
        assert_eq!(
            data(&result),
            vec![vec![
                Some(oid.clone()),
                Some(typ.into()),
                Some(start.into()),
                Some(inc.into()),
                Some(min.into()),
                Some(max.into()),
                Some(cache.into()),
                Some(cycle.into())
            ]]
        );
        let desc = &result.iter().find(|m| m.0 == b'T').unwrap().1;
        assert_eq!(
            columns(desc).iter().map(|c| c.1).collect::<Vec<_>>(),
            vec![26, 26, 20, 20, 20, 20, 20, 16]
        );
        for predicate in ["0".to_string(), "NULL::oid".to_string()] {
            let empty = ok(
                &mut socket,
                &format!(
                    "SELECT {projection} FROM pg_catalog.pg_sequence WHERE seqrelid={predicate}"
                ),
            );
            assert!(data(&empty).is_empty());
            assert_eq!(&empty.iter().find(|m| m.0 == b'T').unwrap().1, desc);
        }
    }
    for (name, decl, typ, maximum) in [
        ("probe_h", "smallint", "21", "32767"),
        ("probe_def", "bigint", "20", "9223372036854775807"),
    ] {
        ok(
            &mut socket,
            &format!("CREATE SEQUENCE public.{name} AS {decl}"),
        );
        let r = ok(
            &mut socket,
            &format!(
                "SELECT seqtypid,seqmax FROM pg_sequence WHERE seqrelid='public.{name}'::regclass::oid"
            ),
        );
        assert_eq!(data(&r), vec![vec![Some(typ.into()), Some(maximum.into())]]);
    }
    // Invalid parameters never publish a shared relation and permit immediate recovery.
    for (name, options) in [
        ("range", "AS smallint MAXVALUE 32768"),
        ("start", "AS integer MINVALUE 1 MAXVALUE 10 START WITH 11"),
        ("zero", "INCREMENT BY 0"),
        ("cache", "CACHE 0"),
        ("bounds", "MINVALUE 10 MAXVALUE 1"),
    ] {
        let r = query(
            &mut socket,
            &format!("CREATE SEQUENCE public.bad_{name} {options}"),
        );
        let e = &r
            .iter()
            .find(|m| m.0 == b'E')
            .expect("invalid declaration succeeded")
            .1;
        assert!(e.windows(7).any(|s| s == b"C22023\0"), "{r:?}");
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        assert!(
            data(&ok(
                &mut socket,
                &format!("SELECT oid FROM pg_class WHERE relname='bad_{name}'")
            ))
            .is_empty()
        );
        ok(&mut socket, "SELECT 1");
    }
    let saved = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new(
            "pg13_027_contract_probe",
        ))
        .unwrap();
    let integer = saved
        .iter()
        .filter_map(|t| t.model_meta.as_ref())
        .find(|t| t.Name.O == "probe_i")
        .unwrap();
    assert_eq!(
        integer.PostgreSQLSequenceType,
        Some(astersql_meta_model::PostgreSQLSequenceType::Integer)
    );
    let encoded = serde_json::to_string(integer).unwrap();
    let restored: astersql_meta_model::TableInfo = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        restored.PostgreSQLSequenceType,
        integer.PostgreSQLSequenceType
    );
    // The full signed bigint endpoints must allocate once and exhaust without wrapping.
    for (name, start, increment, min, max) in [
        ("upper", i64::MAX, 1, i64::MAX - 2, i64::MAX),
        ("lower", i64::MIN, -1, i64::MIN, i64::MIN + 2),
    ] {
        ok(
            &mut socket,
            &format!(
                "CREATE SEQUENCE public.{name} AS bigint START WITH {start} INCREMENT BY {increment} MINVALUE {min} MAXVALUE {max} CACHE 1"
            ),
        );
        let first = ok(
            &mut socket,
            &format!("SELECT nextval('public.{name}'::regclass)"),
        );
        assert_eq!(data(&first), vec![vec![Some(start.to_string())]]);
        let end = query(
            &mut socket,
            &format!("SELECT nextval('public.{name}'::regclass)"),
        );
        assert!(
            end.iter()
                .any(|m| m.0 == b'E' && m.1.windows(7).any(|s| s == b"C2200H\0")),
            "{end:?}"
        );
    }
    println!(
        "pg13_027_contract: declaration=6 identity=1 parameter_rows=4 zero_rows=8 invalid_recovery=5 endpoints=2 model_roundtrip=1"
    );
}

#[test]
fn pg13_028_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_028_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_028_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG28 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_028_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    for sql in [
        "CREATE TABLE public.owned_table (gone integer, id bigint, plain bigint)",
        "CREATE SEQUENCE public.owned_seq START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 OWNED BY public.owned_table.id",
    ] {
        let messages = query(&mut socket, sql);
        assert!(messages.iter().all(|m| m.0 != b'E'), "{sql}: {messages:?}");
        assert_eq!(messages.last(), Some(&(b'Z', vec![b'I'])));
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    let snapshot = domain.info_schema();
    let table = snapshot
        .ModelTableInfoByExactName(
            &astersql_infoschema::CiString::new("pg13_028_contract_probe"),
            "owned_table",
        )
        .unwrap();
    let seq = snapshot
        .ModelTableInfoByExactName(
            &astersql_infoschema::CiString::new("pg13_028_contract_probe"),
            "owned_seq",
        )
        .unwrap();
    assert_eq!(
        seq.PostgreSQLSequenceOwner,
        Some((table.ID, table.Columns[1].ID))
    );
    let restored: astersql_meta_model::TableInfo =
        serde_json::from_slice(&serde_json::to_vec(&*seq).unwrap()).unwrap();
    assert_eq!(
        restored.PostgreSQLSequenceOwner,
        seq.PostgreSQLSequenceOwner
    );
    let sid = crate::pg_oid::table_oid(seq.ID).unwrap().to_string();
    let tid = crate::pg_oid::table_oid(table.ID).unwrap().to_string();
    let sql = format!(
        "SELECT classid,objid,objsubid,refclassid,refobjid,refobjsubid,deptype FROM pg_catalog.pg_depend WHERE objid={sid} ORDER BY refclassid"
    );
    let m = ok(&mut socket, &sql);
    assert_eq!(
        columns(&m[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![26, 26, 23, 26, 26, 23, 18]
    );
    assert!(m.contains(&(
        b'D',
        row(&[
            Some("1259"),
            Some(&sid),
            Some("0"),
            Some("1259"),
            Some(&tid),
            Some("2"),
            Some("a")
        ])
    )));
    assert_eq!(m.iter().filter(|m| m.0 == b'D').count(), 2);
    ok(
        &mut socket,
        "ALTER TABLE public.owned_table DROP COLUMN gone",
    );
    let m = ok(&mut socket, &sql);
    assert!(m.contains(&(
        b'D',
        row(&[
            Some("1259"),
            Some(&sid),
            Some("0"),
            Some("1259"),
            Some(&tid),
            Some("2"),
            Some("a")
        ])
    )));
    ok(
        &mut socket,
        "ALTER TABLE public.owned_table RENAME COLUMN id TO renamed_id",
    );
    ok(
        &mut socket,
        "ALTER TABLE public.owned_table RENAME TO renamed_table",
    );
    ok(
        &mut socket,
        "ALTER SEQUENCE public.owned_seq RENAME TO renamed_seq",
    );
    assert_eq!(
        ok(&mut socket, &sql).iter().filter(|m| m.0 == b'D').count(),
        2
    );
    ok(
        &mut socket,
        "ALTER SEQUENCE public.renamed_seq OWNED BY NONE",
    );
    assert_eq!(
        ok(&mut socket, &sql).iter().filter(|m| m.0 == b'D').count(),
        1
    );
    ok(
        &mut socket,
        "ALTER SEQUENCE public.renamed_seq OWNED BY public.renamed_table.renamed_id",
    );
    for invalid in [
        "ALTER SEQUENCE public.renamed_seq OWNED BY public.renamed_table.absent",
        "SELECT missing_field FROM pg_catalog.pg_depend",
        "DELETE FROM pg_catalog.pg_depend",
    ] {
        let m = query(&mut socket, invalid);
        assert!(m.iter().any(|m| m.0 == b'E'), "{invalid}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        assert_eq!(
            ok(&mut socket, &sql).iter().filter(|m| m.0 == b'D').count(),
            2
        );
    }
    for (q, expected) in [
        ("SELECT nextval('public.renamed_seq'::regclass)", "7"),
        ("SELECT currval('public.renamed_seq'::regclass)", "7"),
        ("SELECT nextval('public.renamed_seq'::regclass)", "10"),
    ] {
        assert!(ok(&mut socket, q).contains(&(b'D', row(&[Some(expected)]))));
        ok(&mut socket, &sql);
    }
    ok(
        &mut socket,
        "ALTER TABLE public.renamed_table DROP COLUMN renamed_id",
    );
    assert_eq!(
        ok(&mut socket, &sql).iter().filter(|m| m.0 == b'D').count(),
        0
    );
    ok(
        &mut socket,
        "CREATE TABLE public.identity_owner(id integer GENERATED ALWAYS AS IDENTITY, plain integer)",
    );
    let snapshot = domain.info_schema();
    let identity = snapshot
        .ModelTableInfoByExactName(
            &astersql_infoschema::CiString::new("pg13_028_contract_probe"),
            "identity_owner",
        )
        .unwrap();
    let internal = identity.Columns[0]
        .PostgreSQLIdentity
        .as_ref()
        .unwrap()
        .SequenceID;
    let seq = snapshot.TableByID(internal).unwrap().ModelMeta().unwrap();
    let m = query(&mut socket, &format!("DROP SEQUENCE public.{}", seq.Name.O));
    assert!(m.iter().any(|m| m.0 == b'E'));
    ok(
        &mut socket,
        "ALTER TABLE public.identity_owner DROP COLUMN id",
    );
    assert!(domain.info_schema().TableByID(internal).is_none());
    ok(
        &mut socket,
        "CREATE SEQUENCE public.drop_independent OWNED BY public.renamed_table.plain",
    );
    ok(&mut socket, "DROP SEQUENCE public.drop_independent");
    ok(
        &mut socket,
        "CREATE SEQUENCE public.drop_with_table OWNED BY public.renamed_table.plain",
    );
    ok(&mut socket, "DROP TABLE public.renamed_table");
    let m = ok(
        &mut socket,
        "SELECT objid FROM pg_catalog.pg_depend WHERE deptype='a'",
    );
    assert_eq!(m.iter().filter(|m| m.0 == b'D').count(), 0);
    println!(
        "pg13_028_contract: owned=1 stable_slots=1 rename=3 detach_attach=2 invalid_recovery=3 no_allocation=1 drop_column=2 drop_sequence=1 drop_table=1 model_roundtrip=1"
    );
}

#[test]
fn pg13_029_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_029_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_029_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG29 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_029_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    for sql in [
        "CREATE SCHEMA seqdep",
        "CREATE TABLE seqdep.ia (gone integer, id integer GENERATED ALWAYS AS IDENTITY, plain integer)",
        "CREATE TABLE seqdep.ib (id bigint GENERATED BY DEFAULT AS IDENTITY)",
        "CREATE TABLE seqdep.t (id bigint, plain bigint)",
        "CREATE SEQUENCE seqdep.s START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 OWNED BY seqdep.t.id",
    ] {
        ok(&mut socket, sql);
    }
    let serial = "SELECT pg_catalog.pg_get_serial_sequence('seqdep.t','id')";
    let m = ok(&mut socket, serial);
    assert!(
        m.contains(&(b'D', row(&[Some("seqdep.s")]))),
        "OWNED BY lookup: {m:?}"
    );
    assert_eq!(
        columns(&m[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![25]
    );
    for (name, mode, kind, slot) in [("ia", "a", "23", "2"), ("ib", "d", "20", "1")] {
        let model = domain
            .info_schema()
            .SchemaTableInfos(&astersql_infoschema::CiString::new(
                "pg13_029_contract_probe",
            ))
            .unwrap()
            .into_iter()
            .filter_map(|table| table.model_meta.clone())
            .find(|model| {
                model
                    .PostgreSQLNamespace
                    .as_ref()
                    .is_some_and(|(_, logical)| logical == name)
            })
            .unwrap();
        let col = model.Columns.iter().find(|c| c.Name.O == "id").unwrap();
        let id = col.PostgreSQLIdentity.as_ref().unwrap().SequenceID;
        println!(
            "PG29 shared model: table={} column={} sequence={id}",
            model.ID, col.ID
        );
        let sid = crate::pg_oid::table_oid(id).unwrap().to_string();
        let tid = crate::pg_oid::table_oid(model.ID).unwrap().to_string();
        let q = format!(
            "SELECT attname,attidentity,atthasdef,atttypid FROM pg_catalog.pg_attribute WHERE attrelid='seqdep.{name}'::regclass::oid AND attname='id'"
        );
        let m = ok(&mut socket, &q);
        assert_eq!(
            columns(&m[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
            vec![19, 18, 16, 26]
        );
        assert!(
            m.contains(&(b'D', row(&[Some("id"), Some(mode), Some("f"), Some(kind)]))),
            "{m:?}"
        );
        let q = format!(
            "SELECT classid,objid,objsubid,refclassid,refobjid,refobjsubid,deptype FROM pg_catalog.pg_depend WHERE objid={sid} AND deptype='i'"
        );
        let m = ok(&mut socket, &q);
        assert!(m.contains(&(
            b'D',
            row(&[
                Some("1259"),
                Some(&sid),
                Some("0"),
                Some("1259"),
                Some(&tid),
                Some(slot),
                Some("i")
            ])
        )));
        let q = format!("SELECT seqtypid FROM pg_catalog.pg_sequence WHERE seqrelid={sid}");
        assert!(ok(&mut socket, &q).contains(&(b'D', row(&[Some(kind)]))));
        let seq = domain
            .info_schema()
            .TableByID(id)
            .unwrap()
            .ModelMeta()
            .unwrap();
        let q = format!("SELECT pg_catalog.pg_get_serial_sequence('seqdep.{name}','id')");
        let returned = format!(
            "seqdep.{}",
            crate::pg_oid::quoted(
                seq.PostgreSQLNamespace
                    .as_ref()
                    .map_or(seq.Name.O.as_str(), |(_, name)| name.as_str())
            )
        );
        assert!(ok(&mut socket, &q).contains(&(b'D', row(&[Some(&returned)]))));
        let denied = query(
            &mut socket,
            &format!(
                "DROP SEQUENCE seqdep.{}",
                crate::pg_oid::quoted(
                    seq.PostgreSQLNamespace
                        .as_ref()
                        .map_or(seq.Name.O.as_str(), |(_, name)| name.as_str())
                )
            ),
        );
        assert!(
            denied
                .iter()
                .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"2BP01")),
            "{denied:?}"
        );
        ok(
            &mut socket,
            &format!("ALTER TABLE seqdep.{name} DROP COLUMN id"),
        );
        assert!(domain.info_schema().TableByID(id).is_none());
        assert_eq!(
            ok(
                &mut socket,
                &format!("SELECT objid FROM pg_catalog.pg_depend WHERE objid={sid}")
            )
            .iter()
            .filter(|m| m.0 == b'D')
            .count(),
            0
        );
    }
    for sql in [
        "SELECT pg_catalog.pg_get_serial_sequence('seqdep.t','plain')",
        "SELECT pg_catalog.pg_get_serial_sequence(NULL,'id')",
        "SELECT pg_catalog.pg_get_serial_sequence('seqdep.t',NULL)",
    ] {
        assert!(ok(&mut socket, sql).contains(&(b'D', row(&[None]))));
    }
    ok(&mut socket, "ALTER SEQUENCE seqdep.s OWNED BY NONE");
    assert!(ok(&mut socket, serial).contains(&(b'D', row(&[None]))));
    ok(&mut socket, "ALTER SEQUENCE seqdep.s OWNED BY seqdep.t.id");
    ok(
        &mut socket,
        "ALTER TABLE seqdep.t RENAME COLUMN id TO renamed",
    );
    ok(&mut socket, "ALTER SEQUENCE seqdep.s RENAME TO renamed_s");
    let m = ok(
        &mut socket,
        "SELECT pg_catalog.pg_get_serial_sequence('seqdep.t','renamed')",
    );
    assert!(m.contains(&(b'D', row(&[Some("seqdep.renamed_s")]))));
    assert!(
        ok(&mut socket, "SELECT nextval('seqdep.renamed_s'::regclass)")
            .contains(&(b'D', row(&[Some("7")])))
    );
    ok(
        &mut socket,
        "SELECT pg_catalog.pg_get_serial_sequence('seqdep.t','renamed')",
    );
    assert!(
        ok(&mut socket, "SELECT currval('seqdep.renamed_s'::regclass)")
            .contains(&(b'D', row(&[Some("7")])))
    );
    assert!(
        ok(&mut socket, "SELECT nextval('seqdep.renamed_s'::regclass)")
            .contains(&(b'D', row(&[Some("10")])))
    );
    let bad = query(
        &mut socket,
        "SELECT pg_catalog.pg_get_serial_sequence('seqdep.t','absent')",
    );
    assert!(
        bad.iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703")),
        "{bad:?}"
    );
    ok(&mut socket, "SELECT 1");
    println!(
        "pg13_029_contract: identity_modes=2 internal_edges=2 sequence_types=2 internal_drop_recovery=2 drop_columns=2 owned_lookup=1 nulls=3 detach_attach=2 rename=2 no_allocation=1 invalid_recovery=1 cleanup=1"
    );
}

#[test]
fn pg13_030_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_030_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_030_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG30 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_030_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    for sql in [
        "CREATE SCHEMA empty_schema",
        "CREATE TABLE public.schemata(id integer)",
    ] {
        ok(&mut socket, sql);
    }
    let sql = "SELECT catalog_name,schema_name,schema_owner,default_character_set_catalog,default_character_set_schema,default_character_set_name,sql_path FROM information_schema.schemata WHERE schema_name='empty_schema'";
    let result = ok(&mut socket, sql);
    assert!(
        result.contains(&(
            b'D',
            row(&[
                Some("pg13_030_contract_probe"),
                Some("empty_schema"),
                Some("root"),
                None,
                None,
                None,
                None
            ])
        )),
        "{result:?}"
    );
    assert_eq!(
        columns(&result[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![19, 19, 19, 19, 19, 19, 1043]
    );
    let zero = ok(&mut socket, &sql.replace("empty_schema", "absent_schema"));
    assert_eq!(zero[0], result[0]);
    assert!(!zero.iter().any(|m| m.0 == b'D'));
    let error = query(
        &mut socket,
        "SELECT absent FROM information_schema.schemata",
    );
    assert!(
        error
            .iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703")),
        "{error:?}"
    );
    ok(&mut socket, "SELECT id FROM public.schemata");
    ok(&mut socket, "DROP TABLE public.schemata");
    ok(&mut socket, "DROP SCHEMA empty_schema");
    let database = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new(
            "pg13_030_contract_probe",
        ))
        .unwrap();
    domain.storage_handle().with_storage(|store| {
        let ctx = astersql_kv::Context::default();
        astersql_kv::RunInNewTxn(&ctx, store, true, |_, txn| {
            txn.Set(
                astersql_meta::transaction_meta_hash_key(
                    b"PgNamespaces",
                    database.id.to_string().as_bytes(),
                ),
                br#"{"legacy":{"id":42,"owner":"missing_role","usage":[],"explicit_acl":null}}"#
                    .to_vec(),
            )
        })
        .unwrap();
    });
    let missing = query(&mut socket, sql);
    assert!(
        missing
            .iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"0A000")),
        "{missing:?}"
    );
    ok(&mut socket, "SELECT 1");
    println!(
        "pg13_030_contract: seven_fields=1 nulls=4 empty_schema=1 zero_rows=1 unknown_column_recovery=1 public_isolation=1 cleanup=1"
    );
}

#[test]
fn pg13_031_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_031_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_031_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG31 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_031_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    for sql in [
        "CREATE SCHEMA other_schema",
        "CREATE TABLE public.is_map_t(id integer)",
        "CREATE TABLE other_schema.is_map_t(id integer)",
        "CREATE VIEW public.is_map_v AS SELECT id FROM public.is_map_t",
        "CREATE SEQUENCE public.is_map_s",
        "CREATE INDEX is_map_i ON public.is_map_t(id)",
    ] {
        ok(&mut socket, sql);
    }
    let sql = "SELECT table_catalog,table_schema,table_name,table_type,self_referencing_column_name,reference_generation,user_defined_type_catalog,user_defined_type_schema,user_defined_type_name,is_insertable_into,is_typed,commit_action FROM information_schema.tables WHERE table_schema='public' AND table_name='is_map_t'";
    let result = ok(&mut socket, sql);
    assert!(
        result.contains(&(
            b'D',
            row(&[
                Some("pg13_031_contract_probe"),
                Some("public"),
                Some("is_map_t"),
                Some("BASE TABLE"),
                None,
                None,
                None,
                None,
                None,
                Some("YES"),
                Some("NO"),
                None
            ])
        )),
        "{result:?}"
    );
    assert_eq!(
        columns(&result[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![19, 19, 19, 1043, 19, 1043, 19, 19, 19, 1043, 1043, 1043]
    );
    let zero = ok(&mut socket, &sql.replace("is_map_t", "absent_table"));
    assert_eq!(zero[0], result[0]);
    assert!(!zero.iter().any(|m| m.0 == b'D'));
    let kinds = ok(
        &mut socket,
        "SELECT table_name,table_type FROM information_schema.tables WHERE table_schema='public' AND table_name IN ('is_map_t','is_map_v','is_map_s','is_map_i') ORDER BY table_name",
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        vec![
            row(&[Some("is_map_t"), Some("BASE TABLE")]),
            row(&[Some("is_map_v"), Some("VIEW")])
        ]
    );
    ok(&mut socket, "SET search_path TO pg_catalog");
    assert_eq!(ok(&mut socket, sql), result);
    let both = ok(
        &mut socket,
        "SELECT table_schema FROM information_schema.tables WHERE table_name='is_map_t' ORDER BY table_schema",
    );
    assert_eq!(both.iter().filter(|m| m.0 == b'D').count(), 2);
    for command in [
        "SELECT absent FROM information_schema.tables",
        "BEGIN",
        "SELECT absent FROM information_schema.tables",
        "ROLLBACK",
        "SELECT 1",
    ] {
        let m = query(&mut socket, command);
        if command.contains("absent") {
            assert!(
                m.iter()
                    .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703")),
                "{m:?}"
            );
        }
    }
    // Native CREATE VIEW deliberately lacks the durable PG binding. Its name
    // stays readable, while only the unsupported insertability flag refuses.
    native.execute("CREATE VIEW pg13_031_contract_probe.legacy_v AS SELECT id FROM pg13_031_contract_probe.is_map_t").unwrap();
    let legacy = ok(
        &mut socket,
        "SELECT table_name FROM information_schema.tables WHERE table_name='legacy_v'",
    );
    assert!(legacy.contains(&(b'D', row(&[Some("legacy_v")]))));
    let unsupported = query(
        &mut socket,
        "SELECT is_insertable_into FROM information_schema.tables WHERE table_name='legacy_v'",
    );
    assert!(
        unsupported
            .iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"0A000")),
        "{unsupported:?}"
    );
    ok(&mut socket, sql);
    native
        .execute("DROP VIEW pg13_031_contract_probe.legacy_v")
        .unwrap();
    native.execute("CREATE GLOBAL TEMPORARY TABLE pg13_031_contract_probe.legacy_global(id INT) ON COMMIT DELETE ROWS").unwrap();
    let global = ok(
        &mut socket,
        "SELECT table_name FROM information_schema.tables WHERE table_name='legacy_global'",
    );
    assert!(global.contains(&(b'D', row(&[Some("legacy_global")]))));
    let kind = query(
        &mut socket,
        "SELECT table_type FROM information_schema.tables WHERE table_name='legacy_global'",
    );
    assert!(
        kind.iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"0A000")),
        "{kind:?}"
    );
    ok(&mut socket, sql);
    native
        .execute("DROP TABLE pg13_031_contract_probe.legacy_global")
        .unwrap();
    ok(&mut socket, "DROP VIEW public.is_map_v");
    ok(&mut socket, "DROP TABLE public.is_map_t");
    ok(&mut socket, "DROP TABLE other_schema.is_map_t");
    ok(&mut socket, "DROP SEQUENCE public.is_map_s");
    ok(&mut socket, "DROP SCHEMA other_schema");
    println!(
        "pg13_031_contract: scenarios=18 twelve_fields typed_null zero_rows relation_kinds path schemas recovery cleanup"
    );
}

#[test]
fn pg13_032_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_032_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_032_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG32 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_032_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    for sql in [
        "CREATE SCHEMA other_schema",
        "CREATE TABLE public.is_map_t(id integer)",
        "CREATE TABLE other_schema.is_map_t(id integer)",
        "CREATE VIEW public.is_map_v AS SELECT id FROM public.is_map_t",
        "CREATE SEQUENCE public.is_map_s",
        "CREATE INDEX is_map_i ON public.is_map_t(id)",
    ] {
        ok(&mut socket, sql);
    }
    let standard = ok(
        &mut socket,
        "SELECT table_name FROM information_schema.views WHERE table_schema='information_schema' AND table_name IN ('schemata','tables','views') ORDER BY table_name",
    );
    assert_eq!(
        standard.iter().filter(|m| m.0 == b'D').count(),
        3,
        "{standard:?}"
    );
    let sql = "SELECT table_catalog,table_schema,table_name,view_definition,check_option,is_updatable,is_insertable_into,is_trigger_updatable,is_trigger_deletable,is_trigger_insertable_into FROM information_schema.views WHERE table_schema='public' AND table_name='is_map_v'";
    let result = ok(&mut socket, sql);
    assert_eq!(
        columns(&result[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![19, 19, 19, 1043, 1043, 1043, 1043, 1043, 1043, 1043]
    );
    let definition = ok(
        &mut socket,
        "SELECT pg_catalog.pg_get_viewdef('public.is_map_v'::regclass)",
    );
    let def = definition.iter().find(|m| m.0 == b'D').unwrap();
    let data = &def.1;
    let len = i32::from_be_bytes(data[2..6].try_into().unwrap()) as usize;
    let definition = std::str::from_utf8(&data[6..6 + len]).unwrap();
    assert!(
        result.contains(&(
            b'D',
            row(&[
                Some("pg13_032_contract_probe"),
                Some("public"),
                Some("is_map_v"),
                Some(definition),
                Some("NONE"),
                Some("YES"),
                Some("YES"),
                Some("NO"),
                Some("NO"),
                Some("NO")
            ])
        )),
        "{result:?}"
    );
    let zero = ok(&mut socket, &sql.replace("is_map_v", "absent"));
    assert_eq!(result[0], zero[0]);
    assert!(!zero.iter().any(|m| m.0 == b'D'));
    ok(
        &mut socket,
        "CREATE VIEW public.literal_v AS SELECT 32 AS id",
    );
    let flags = ok(
        &mut socket,
        "SELECT is_updatable,is_insertable_into FROM information_schema.views WHERE table_name='literal_v'",
    );
    assert!(flags.contains(&(b'D', row(&[Some("NO"), Some("NO")]))));
    ok(
        &mut socket,
        "CREATE VIEW public.checked_v AS SELECT id FROM public.is_map_t WHERE id>0 WITH LOCAL CHECK OPTION",
    );
    let checked = ok(
        &mut socket,
        "SELECT check_option FROM information_schema.views WHERE table_name='checked_v'",
    );
    assert!(checked.contains(&(b'D', row(&[Some("LOCAL")]))));
    ok(&mut socket, "SET search_path TO pg_catalog");
    assert_eq!(ok(&mut socket, sql), result);
    native.execute("CREATE VIEW pg13_032_contract_probe.legacy_v AS SELECT id FROM pg13_032_contract_probe.is_map_t").unwrap();
    assert!(
        ok(
            &mut socket,
            "SELECT table_name FROM information_schema.views WHERE table_name='legacy_v'"
        )
        .contains(&(b'D', row(&[Some("legacy_v")])))
    );
    for field in [
        "view_definition",
        "check_option",
        "is_updatable",
        "is_trigger_updatable",
    ] {
        let error = query(
            &mut socket,
            &format!("SELECT {field} FROM information_schema.views WHERE table_name='legacy_v'"),
        );
        assert!(
            error
                .iter()
                .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"0A000")),
            "{error:?}"
        );
    }
    for command in [
        "SELECT absent FROM information_schema.views",
        "BEGIN",
        "SELECT absent FROM information_schema.views",
        "ROLLBACK",
        "SELECT 1",
    ] {
        let m = query(&mut socket, command);
        if command.contains("absent") {
            assert!(
                m.iter()
                    .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703")),
                "{m:?}"
            );
        }
    }
    println!(
        "pg13_032_contract: scenarios=14 definition flags literal check_option zero_describe path legacy_projection_reject recovery cleanup"
    );
}

#[test]
fn pg13_033_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_033_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_033_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG33 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_033_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.is_col_t(id integer NOT NULL, label varchar(7), plain integer)",
    );
    let sql = "SELECT table_catalog,table_schema,table_name,column_name,ordinal_position,is_nullable,data_type,udt_catalog,udt_schema,udt_name,domain_catalog,domain_schema,domain_name,dtd_identifier FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_t' ORDER BY ordinal_position";
    let result = ok(&mut socket, sql);
    assert_eq!(
        columns(&result[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![19, 19, 19, 19, 23, 1043, 1043, 19, 19, 19, 19, 19, 19, 19]
    );
    assert_eq!(
        result
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| cells(&m.1))
            .collect::<Vec<_>>(),
        vec![
            vec![
                Some("pg13_033_contract_probe"),
                Some("public"),
                Some("is_col_t"),
                Some("id"),
                Some("1"),
                Some("NO"),
                Some("integer"),
                Some("pg13_033_contract_probe"),
                Some("pg_catalog"),
                Some("int4"),
                None,
                None,
                None,
                Some("1")
            ],
            vec![
                Some("pg13_033_contract_probe"),
                Some("public"),
                Some("is_col_t"),
                Some("label"),
                Some("2"),
                Some("YES"),
                Some("character varying"),
                Some("pg13_033_contract_probe"),
                Some("pg_catalog"),
                Some("varchar"),
                None,
                None,
                None,
                Some("2")
            ],
            vec![
                Some("pg13_033_contract_probe"),
                Some("public"),
                Some("is_col_t"),
                Some("plain"),
                Some("3"),
                Some("YES"),
                Some("integer"),
                Some("pg13_033_contract_probe"),
                Some("pg_catalog"),
                Some("int4"),
                None,
                None,
                None,
                Some("3")
            ],
        ]
    );
    for ddl in [
        "ALTER TABLE public.is_col_t DROP COLUMN label",
        "ALTER TABLE public.is_col_t ADD COLUMN added integer[]",
        "ALTER TABLE public.is_col_t RENAME COLUMN added TO renamed",
    ] {
        ok(&mut socket, ddl);
    }
    let out = ok(&mut socket, sql);
    let rows = out
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect::<Vec<_>>();
    assert_eq!(
        rows.iter()
            .map(|r| (r[3], r[4], r[6], r[9]))
            .collect::<Vec<_>>(),
        vec![
            (Some("id"), Some("1"), Some("integer"), Some("int4")),
            (Some("plain"), Some("3"), Some("integer"), Some("int4")),
            (Some("renamed"), Some("4"), Some("ARRAY"), Some("_int4"))
        ]
    );
    let empty = ok(&mut socket, &sql.replace("is_col_t'", "missing'"));
    assert_eq!(columns(&empty[0].1), columns(&result[0].1));
    assert!(!empty.iter().any(|m| m.0 == b'D'));
    let bad = query(&mut socket, "SELECT absent FROM information_schema.columns");
    assert!(
        bad.iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703"))
    );
    ok(&mut socket, "SELECT 1");
    println!(
        "pg13_033_contract: scenarios=7 identity nullability types arrays stable_slots zero_describe error recovery cleanup"
    );
}

#[test]
fn pg13_034_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_034_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_034_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG34 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_034_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.is_size_t(id integer, amount numeric(12,3), label varchar(7), stamp timestamp(3), plain text, f real, d double precision, dt date, n numeric, neg numeric(5,-2), iv interval day to second(2))",
    );
    let sql = "SELECT column_name,character_maximum_length,character_octet_length,numeric_precision,numeric_precision_radix,numeric_scale,datetime_precision,interval_type,interval_precision FROM information_schema.columns WHERE table_schema='public' AND table_name='is_size_t' ORDER BY ordinal_position";
    let result = ok(&mut socket, sql);
    assert_eq!(
        columns(&result[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![19, 23, 23, 23, 23, 23, 23, 1043, 23]
    );
    let expected = vec![
        vec![
            Some("id"),
            None,
            None,
            Some("32"),
            Some("2"),
            Some("0"),
            None,
            None,
            None,
        ],
        vec![
            Some("amount"),
            None,
            None,
            Some("12"),
            Some("10"),
            Some("3"),
            None,
            None,
            None,
        ],
        vec![
            Some("label"),
            Some("7"),
            Some("28"),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        vec![
            Some("stamp"),
            None,
            None,
            None,
            None,
            None,
            Some("3"),
            None,
            None,
        ],
        vec![
            Some("plain"),
            None,
            Some("1073741824"),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        vec![
            Some("f"),
            None,
            None,
            Some("24"),
            Some("2"),
            None,
            None,
            None,
            None,
        ],
        vec![
            Some("d"),
            None,
            None,
            Some("53"),
            Some("2"),
            None,
            None,
            None,
            None,
        ],
        vec![
            Some("dt"),
            None,
            None,
            None,
            None,
            None,
            Some("0"),
            None,
            None,
        ],
        vec![
            Some("n"),
            None,
            None,
            None,
            Some("10"),
            None,
            None,
            None,
            None,
        ],
        vec![
            Some("neg"),
            None,
            None,
            Some("5"),
            Some("10"),
            Some("2046"),
            None,
            None,
            None,
        ],
        vec![
            Some("iv"),
            None,
            None,
            None,
            None,
            None,
            Some("2"),
            Some("DAY TO SECOND(2)"),
            None,
        ],
    ];
    assert_eq!(
        result
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| cells(&m.1))
            .collect::<Vec<_>>(),
        expected
    );
    let empty = ok(&mut socket, &sql.replace("is_size_t'", "absent'"));
    assert_eq!(columns(&empty[0].1), columns(&result[0].1));
    assert!(!empty.iter().any(|m| m.0 == b'D'));
    let extra = ok(
        &mut socket,
        "SELECT character_set_catalog,character_set_schema,character_set_name,scope_catalog,scope_schema,scope_name,maximum_cardinality,is_self_referencing FROM information_schema.columns WHERE table_name='is_size_t' ORDER BY ordinal_position",
    );
    assert_eq!(
        columns(&extra[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![19, 19, 19, 19, 19, 19, 23, 1043]
    );
    for row in extra.iter().filter(|m| m.0 == b'D') {
        assert_eq!(
            cells(&row.1),
            vec![None, None, None, None, None, None, None, Some("NO")]
        );
    }
    let bad = query(&mut socket, "SELECT absent FROM information_schema.columns");
    assert!(
        bad.iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703"))
    );
    ok(&mut socket, "SELECT 1");
    println!(
        "pg13_034_contract: scenarios=14 types=11 lengths numeric temporal interval null zero_describe error recovery cleanup"
    );
}

#[test]
fn pg13_035_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_035_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_035_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG35 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_035_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.is_def_t(a integer DEFAULT 7, b integer GENERATED ALWAYS AS (a+1) STORED, plain integer)",
    );
    let sql = "SELECT column_name,column_default,is_generated,generation_expression,is_updatable FROM information_schema.columns WHERE table_schema='public' AND table_name='is_def_t' ORDER BY ordinal_position";
    let result = ok(&mut socket, sql);
    assert_eq!(
        columns(&result[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![19, 1043, 1043, 1043, 1043]
    );
    let values: Vec<_> = result
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect();
    assert_eq!(
        values,
        vec![
            vec![Some("a"), Some("7"), Some("NEVER"), None, Some("YES")],
            vec![
                Some("b"),
                None,
                Some("ALWAYS"),
                Some("(\"a\" + 1)"),
                Some("YES")
            ],
            vec![Some("plain"), None, Some("NEVER"), None, Some("YES")]
        ]
    );
    ok(&mut socket, "INSERT INTO public.is_def_t(plain) VALUES(3)");
    let inserted = ok(&mut socket, "SELECT a,b,plain FROM public.is_def_t");
    assert_eq!(
        cells(&inserted.iter().find(|m| m.0 == b'D').unwrap().1),
        vec![Some("7"), Some("8"), Some("3")]
    );
    ok(&mut socket, "UPDATE public.is_def_t SET a=9");
    let updated = ok(&mut socket, "SELECT a,b FROM public.is_def_t");
    assert_eq!(
        cells(&updated.iter().find(|m| m.0 == b'D').unwrap().1),
        vec![Some("9"), Some("10")]
    );
    ok(
        &mut socket,
        "CREATE VIEW public.is_def_v AS SELECT a,a+1 AS calculated FROM public.is_def_t",
    );
    let view = ok(&mut socket, &sql.replace("is_def_t", "is_def_v"));
    assert_eq!(
        view.iter()
            .filter(|m| m.0 == b'D')
            .map(|m| cells(&m.1))
            .collect::<Vec<_>>(),
        vec![
            vec![Some("a"), None, Some("NEVER"), None, Some("YES")],
            vec![Some("calculated"), None, Some("NEVER"), None, Some("NO")]
        ]
    );
    for ddl in [
        "ALTER TABLE public.is_def_t ALTER COLUMN a SET DEFAULT 11",
        "ALTER TABLE public.is_def_t ALTER COLUMN a DROP DEFAULT",
        "ALTER TABLE public.is_def_t ALTER COLUMN plain SET DEFAULT NULL",
    ] {
        ok(&mut socket, ddl);
    }
    let refreshed = domain
        .info_schema()
        .SchemaTableInfos(&astersql_infoschema::CiString::new(
            "pg13_035_contract_probe",
        ))
        .unwrap();
    let base = refreshed
        .iter()
        .filter_map(|t| t.model_meta.as_ref())
        .find(|m| m.Name.O == "is_def_t")
        .unwrap();
    let view = refreshed
        .iter()
        .filter_map(|t| t.model_meta.as_ref())
        .find(|m| m.Name.O == "is_def_v")
        .unwrap();
    let bound = view.PostgreSQLView.as_ref().unwrap();
    assert!(
        bound.render(view, Some(base), "public", false).is_some(),
        "base={:?} dependency={:?}",
        base.Columns[0].FieldType,
        bound.RelationColumns[0].1
    );
    let empty = ok(&mut socket, &sql.replace("is_def_t", "missing"));
    assert_eq!(columns(&empty[0].1), columns(&result[0].1));
    assert!(!empty.iter().any(|m| m.0 == b'D'));
    let bad = query(&mut socket, "SELECT absent FROM information_schema.columns");
    assert!(
        bad.iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703"))
    );
    ok(&mut socket, "SELECT 1");
    // Corruption is injected only at the stored model boundary, never as provider rows.
    let snapshot = domain.info_schema();
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new(
            "pg13_035_contract_probe",
        ))
        .unwrap();
    let model = tables
        .iter()
        .filter_map(|t| t.model_meta.as_ref())
        .find(|m| m.Name.O == "is_def_t")
        .unwrap();
    for scenario in 0..4 {
        let mut bad = model.as_ref().clone();
        match scenario {
            0 => bad.Columns[1].PostgreSQLGenerated = None,
            1 => bad.Columns[1].PostgreSQLDefaultID = None,
            2 => bad.Columns[1].PostgreSQLHasDefault = Some(false),
            _ => {
                bad.Columns[1].PostgreSQLGenerated =
                    Some(astersql_meta_model::PostgreSQLCheckExpression::Column(999))
            }
        }
        for field in [5, 41, 42] {
            assert!(
                crate::pg_catalog::standard_column_definition(
                    snapshot.as_ref(),
                    "pg13_035_contract_probe",
                    &bad,
                    &bad.Columns[1],
                    field
                )
                .is_err(),
                "corrupt model {scenario} field {field}"
            );
        }
    }
    println!(
        "pg13_035_contract: scenarios=13 corrupt_models=4 defaults generated stored insert update view column_analysis null zero_describe recovery cleanup"
    );
}

#[test]
fn pg13_036_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_036_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_036_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG36 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_036_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.is_col_i(id bigint GENERATED BY DEFAULT AS IDENTITY (START WITH 7 INCREMENT BY 2 MINVALUE 1 MAXVALUE 99 NO CYCLE), plain integer)",
    );
    ok(
        &mut socket,
        "CREATE TABLE public.is_col_a(id smallint GENERATED ALWAYS AS IDENTITY (START WITH 3 INCREMENT BY 1 MINVALUE 1 MAXVALUE 9 CYCLE))",
    );
    let sql = "SELECT column_name,is_identity,identity_generation,identity_start,identity_increment,identity_minimum,identity_maximum,identity_cycle,column_default FROM information_schema.columns WHERE table_schema='public' AND table_name='is_col_i' ORDER BY ordinal_position";
    let result = ok(&mut socket, sql);
    assert_eq!(
        columns(&result[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![19, 1043, 1043, 1043, 1043, 1043, 1043, 1043, 1043]
    );
    assert_eq!(
        result
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| cells(&m.1))
            .collect::<Vec<_>>(),
        vec![
            vec![
                Some("id"),
                Some("YES"),
                Some("BY DEFAULT"),
                Some("7"),
                Some("2"),
                Some("1"),
                Some("99"),
                Some("NO"),
                None
            ],
            vec![
                Some("plain"),
                Some("NO"),
                None,
                None,
                None,
                None,
                None,
                Some("NO"),
                None
            ]
        ]
    );
    let always = ok(&mut socket, &sql.replace("is_col_i", "is_col_a"));
    assert_eq!(
        cells(&always.iter().find(|m| m.0 == b'D').unwrap().1),
        vec![
            Some("id"),
            Some("YES"),
            Some("ALWAYS"),
            Some("3"),
            Some("1"),
            Some("1"),
            Some("9"),
            Some("YES"),
            None
        ]
    );
    // The real allocator must still start at the declaration after introspection.
    ok(&mut socket, "INSERT INTO public.is_col_i(plain) VALUES(1)");
    let inserted = ok(&mut socket, "SELECT id FROM public.is_col_i");
    assert_eq!(
        cells(&inserted.iter().find(|m| m.0 == b'D').unwrap().1),
        vec![Some("7")]
    );
    let after = ok(&mut socket, sql);
    assert_eq!(result, after);
    for (ddl, index, expected) in [
        ("SET GENERATED ALWAYS", 2, "ALWAYS"),
        ("SET INCREMENT BY 3", 4, "3"),
        ("SET START WITH 11", 3, "11"),
        ("SET MINVALUE 2", 5, "2"),
        ("SET MAXVALUE 101", 6, "101"),
        ("SET CYCLE", 7, "YES"),
        ("SET NO CYCLE", 7, "NO"),
        ("SET GENERATED BY DEFAULT", 2, "BY DEFAULT"),
    ] {
        ok(
            &mut socket,
            &format!("ALTER TABLE public.is_col_i ALTER COLUMN id {ddl}"),
        );
        let updated = ok(&mut socket, sql);
        assert_eq!(
            cells(&updated.iter().find(|m| m.0 == b'D').unwrap().1)[index],
            Some(expected)
        );
    }
    ok(&mut socket, "INSERT INTO public.is_col_i(plain) VALUES(2)");
    let next = ok(&mut socket, "SELECT id FROM public.is_col_i WHERE plain=2");
    assert_eq!(
        cells(&next.iter().find(|m| m.0 == b'D').unwrap().1),
        vec![Some("10")]
    );
    let snapshot = domain.info_schema();
    let tables = snapshot
        .SchemaTableInfos(&astersql_infoschema::CiString::new(
            "pg13_036_contract_probe",
        ))
        .unwrap();
    let model = tables
        .iter()
        .filter_map(|t| t.model_meta.as_ref())
        .find(|t| t.Name.O == "is_col_i")
        .unwrap();
    println!(
        "PG36 shared model: table={} column={} sequence={}",
        model.ID,
        model.Columns[0].ID,
        model.Columns[0]
            .PostgreSQLIdentity
            .as_ref()
            .unwrap()
            .SequenceID
    );
    for scenario in 0..3 {
        let mut bad = model.as_ref().clone();
        match scenario {
            0 => {
                bad.Columns[0]
                    .PostgreSQLIdentity
                    .as_mut()
                    .unwrap()
                    .SequenceID = i64::MAX
            }
            1 => {
                bad.Columns[0].PostgreSQLGenerated =
                    Some(astersql_meta_model::PostgreSQLCheckExpression::Column(99))
            }
            _ => bad.PostgreSQLNamespace = Some((i64::MAX, "invalid".into())),
        }
        for field in 34..=40 {
            assert!(
                crate::pg_catalog::standard_column_identity(
                    snapshot.as_ref(),
                    "pg13_036_contract_probe",
                    &bad,
                    &bad.Columns[0],
                    field
                )
                .is_err(),
                "scenario={scenario} field={field}"
            );
        }
    }
    let empty = ok(&mut socket, &sql.replace("is_col_i", "missing"));
    assert_eq!(columns(&empty[0].1), columns(&result[0].1));
    assert!(!empty.iter().any(|m| m.0 == b'D'));
    let bad = query(&mut socket, "SELECT absent FROM information_schema.columns");
    assert!(
        bad.iter()
            .any(|m| m.0 == b'E' && m.1.windows(5).any(|w| w == b"42703"))
    );
    ok(&mut socket, "SELECT 1");
    println!(
        "pg13_036_contract: scenarios=20 modes=2 alterations=8 corrupt_models=3 cycle null zero_describe no_allocation allocator_continuity error recovery cleanup"
    );
}

#[test]
fn pg13_037_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_037_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_037_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG37 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_037_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.tc_p(a integer, b integer, CONSTRAINT tc_pk PRIMARY KEY(a,b), CONSTRAINT tc_u UNIQUE(b))",
    );
    ok(
        &mut socket,
        "CREATE TABLE public.tc_c(x integer NOT NULL, v integer, CONSTRAINT tc_f FOREIGN KEY(x) REFERENCES public.tc_p(b), CONSTRAINT tc_ck CHECK(v > 0))",
    );
    let sql = "SELECT constraint_catalog,constraint_schema,constraint_name,table_catalog,table_schema,table_name,constraint_type,is_deferrable,initially_deferred,enforced,nulls_distinct FROM information_schema.table_constraints WHERE table_schema='public' AND table_name='tc_c' ORDER BY constraint_name";
    let m = ok(&mut socket, sql);
    assert_eq!(
        columns(&m[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![19, 19, 19, 19, 19, 19, 1043, 1043, 1043, 1043, 1043]
    );
    let rows: Vec<_> = m
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(&row[7..], &[Some("NO"), Some("NO"), Some("YES"), None]);
    }
    assert!(
        rows.iter()
            .any(|r| r[2] == Some("tc_f") && r[6] == Some("FOREIGN KEY"))
    );
    assert!(
        rows.iter()
            .any(|r| r[2] == Some("tc_ck") && r[6] == Some("CHECK"))
    );
    let zero = ok(&mut socket, &sql.replace("tc_c'", "missing'"));
    assert_eq!(columns(&zero[0].1), columns(&m[0].1));
    assert!(!zero.iter().any(|m| m.0 == b'D'));
    ok(&mut socket, "ALTER TABLE public.tc_c RENAME COLUMN x TO xx");
    ok(&mut socket, "ALTER TABLE public.tc_p RENAME COLUMN b TO bb");
    let after = ok(&mut socket, sql);
    assert_eq!(after.iter().filter(|m| m.0 == b'D').count(), 3);
    let keys = ok(
        &mut socket,
        "SELECT conkey,confkey FROM pg_catalog.pg_constraint WHERE conname='tc_f'",
    );
    assert!(
        keys.iter()
            .any(|m| m.0 == b'D' && m.1 == row(&[Some("{1}"), Some("{2}")]))
    );
    ok(&mut socket, "ALTER TABLE public.tc_c DROP CONSTRAINT tc_f");
    assert_eq!(
        ok(&mut socket, sql).iter().filter(|m| m.0 == b'D').count(),
        2
    );
    ok(&mut socket, "DROP TABLE public.tc_c");
    ok(&mut socket, "DROP TABLE public.tc_p");
    println!(
        "pg13_037_contract: scenarios=5 drop_constraint rename_child_parent p/u/f/c/n state null zero_describe cleanup"
    );
}

#[test]
fn pg13_038_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_038_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_038_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG38 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_038_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.kp(a integer,b integer,CONSTRAINT kp_pk PRIMARY KEY(a,b))",
    );
    ok(
        &mut socket,
        "CREATE TABLE public.kc(pad integer,x integer,y integer,v integer,CONSTRAINT kc_fk FOREIGN KEY(y,x) REFERENCES public.kp(b,a),CONSTRAINT kc_ck CHECK(v>0))",
    );
    let sql = "SELECT constraint_catalog,constraint_schema,constraint_name,table_catalog,table_schema,table_name,column_name,ordinal_position,position_in_unique_constraint FROM information_schema.key_column_usage WHERE table_schema='public' AND table_name='kc' ORDER BY ordinal_position";
    let m = ok(&mut socket, sql);
    assert_eq!(
        columns(&m[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![19, 19, 19, 19, 19, 19, 19, 23, 23]
    );
    let data: Vec<_> = m
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect();
    assert_eq!(data.len(), 2);
    assert_eq!(&data[0][6..], &[Some("y"), Some("1"), Some("2")]);
    assert_eq!(&data[1][6..], &[Some("x"), Some("2"), Some("1")]);
    let parent = ok(&mut socket, &sql.replace("'kc'", "'kp'"));
    assert_eq!(parent.iter().filter(|m| m.0 == b'D').count(), 2);
    assert!(
        parent
            .iter()
            .filter(|m| m.0 == b'D')
            .all(|m| cells(&m.1)[8].is_none())
    );
    let zero = ok(&mut socket, &sql.replace("'kc'", "'missing'"));
    assert_eq!(columns(&zero[0].1), columns(&m[0].1));
    assert!(!zero.iter().any(|m| m.0 == b'D'));
    ok(&mut socket, "ALTER TABLE public.kc DROP COLUMN pad");
    ok(&mut socket, "ALTER TABLE public.kc RENAME COLUMN y TO yy");
    ok(&mut socket, "ALTER TABLE public.kp RENAME COLUMN b TO bb");
    let fresh = ok(&mut socket, sql);
    let data: Vec<_> = fresh
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect();
    assert_eq!(&data[0][6..], &[Some("yy"), Some("1"), Some("2")]);
    ok(&mut socket, "DROP TABLE public.kc");
    ok(&mut socket, "DROP TABLE public.kp");
    println!(
        "pg13_038_contract: scenarios=5 reverse_fk key_order non_fk_null zero_describe dropped_slot rename cleanup"
    );
}

#[test]
fn pg13_039_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_039_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_039_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG39 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_039_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.rp(a integer,b integer,CONSTRAINT rp_pk PRIMARY KEY(a,b))",
    );
    ok(
        &mut socket,
        "CREATE TABLE public.rc(x integer,y integer,CONSTRAINT rc_fk FOREIGN KEY(x,y) REFERENCES public.rp(b,a) ON UPDATE RESTRICT ON DELETE CASCADE)",
    );
    let sql = "SELECT constraint_catalog,constraint_schema,constraint_name,unique_constraint_catalog,unique_constraint_schema,unique_constraint_name,match_option,update_rule,delete_rule FROM information_schema.referential_constraints WHERE constraint_name='rc_fk'";
    let m = ok(&mut socket, sql);
    assert_eq!(
        columns(&m[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![19, 19, 19, 19, 19, 19, 1043, 1043, 1043]
    );
    let data: Vec<_> = m
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect();
    assert_eq!(data.len(), 1);
    assert_eq!(
        &data[0][1..],
        &[
            Some("public"),
            Some("rc_fk"),
            Some("pg13_039_contract_probe"),
            Some("public"),
            Some("rp_pk"),
            Some("NONE"),
            Some("RESTRICT"),
            Some("CASCADE")
        ]
    );
    let zero = ok(&mut socket, &sql.replace("'rc_fk'", "'missing'"));
    assert_eq!(columns(&zero[0].1), columns(&m[0].1));
    assert!(!zero.iter().any(|m| m.0 == b'D'));
    ok(&mut socket, "CREATE TABLE public.ri(a integer,b integer)");
    ok(&mut socket, "CREATE UNIQUE INDEX ri_ix ON public.ri(a,b)");
    ok(
        &mut socket,
        "CREATE TABLE public.rr(x integer,y integer,CONSTRAINT rr_fk FOREIGN KEY(x,y) REFERENCES public.ri(a,b))",
    );
    let independent = ok(&mut socket, &sql.replace("'rc_fk'", "'rr_fk'"));
    let row = independent.iter().find(|m| m.0 == b'D').unwrap();
    assert_eq!(
        &cells(&row.1)[3..],
        &[
            None,
            None,
            None,
            Some("NONE"),
            Some("NO ACTION"),
            Some("NO ACTION")
        ]
    );
    ok(&mut socket, "CREATE SCHEMA r_s");
    ok(
        &mut socket,
        "CREATE TABLE r_s.rp(a integer,CONSTRAINT rp_pk UNIQUE(a))",
    );
    ok(
        &mut socket,
        "CREATE TABLE r_s.rc(x integer,CONSTRAINT rc_fk FOREIGN KEY(x) REFERENCES r_s.rp(a))",
    );
    let names = ok(&mut socket, &format!("{sql} ORDER BY constraint_schema"));
    let rows: Vec<_> = names
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][4], Some("public"));
    assert_eq!(rows[1][4], Some("r_s"));
    ok(&mut socket, "ALTER TABLE public.rp RENAME COLUMN b TO bb");
    assert_eq!(
        ok(&mut socket, &format!("{sql} ORDER BY constraint_schema")),
        names
    );
    ok(&mut socket, "ALTER TABLE public.rc DROP CONSTRAINT rc_fk");
    assert_eq!(
        ok(&mut socket, sql).iter().filter(|m| m.0 == b'D').count(),
        1
    );
    let unsupported = query(
        &mut socket,
        "CREATE TEMPORARY TABLE rc_unsupported(x integer,y integer,FOREIGN KEY(x,y) REFERENCES public.ri(a,b))",
    );
    let error = unsupported.iter().find(|m| m.0 == b'E').unwrap();
    assert!(error.1.windows(6).any(|w| w == b"0A000\0"));
    assert_eq!(unsupported.last(), Some(&(b'Z', vec![b'I'])));
    assert!(
        !ok(
            &mut socket,
            "SELECT relname FROM pg_catalog.pg_class WHERE relname='rc_unsupported'"
        )
        .iter()
        .any(|m| m.0 == b'D')
    );
    let deferred = query(
        &mut socket,
        "CREATE TABLE rc_deferred(x integer,y integer,FOREIGN KEY(x,y) REFERENCES public.ri(a,b) DEFERRABLE INITIALLY DEFERRED)",
    );
    let error = deferred.iter().find(|m| m.0 == b'E').unwrap();
    assert!(error.1.windows(6).any(|w| w == b"0A000\0"));
    assert_eq!(deferred.last(), Some(&(b'Z', vec![b'I'])));
    assert!(
        !ok(
            &mut socket,
            "SELECT relname FROM pg_catalog.pg_class WHERE relname='rc_deferred'"
        )
        .iter()
        .any(|m| m.0 == b'D')
    );
    println!(
        "pg13_039_contract: scenarios=8 base reverse_fk independent_index_null zero_describe cross_schema rename drop cleanup"
    );
}

#[test]
fn pg13_040_contract() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native
        .execute("CREATE DATABASE pg13_040_contract_probe")
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS pg13_040_contract_probe")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
        println!("PG40 cleanup: database dropped; listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0pg13_040_contract_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&body).unwrap();
    while read(&mut socket).0 != b'Z' {}

    fn cells(body: &[u8]) -> Vec<Option<&str>> {
        let count = u16::from_be_bytes(body[..2].try_into().unwrap());
        let mut pos = 2;
        let mut out = Vec::new();
        for _ in 0..count {
            let len = i32::from_be_bytes(body[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len < 0 {
                out.push(None);
            } else {
                let end = pos + len as usize;
                out.push(Some(std::str::from_utf8(&body[pos..end]).unwrap()));
                pos = end;
            }
        }
        out
    }
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let m = query(socket, sql);
        assert!(m.iter().all(|m| m.0 != b'E'), "{sql}: {m:?}");
        assert_eq!(m.last(), Some(&(b'Z', vec![b'I'])));
        m
    }
    ok(
        &mut socket,
        "CREATE TABLE public.cp(a integer,b integer,CONSTRAINT cp_pk PRIMARY KEY(a,b))",
    );
    ok(
        &mut socket,
        "CREATE TABLE public.cc(x integer,y integer,v integer,CONSTRAINT cc_fk FOREIGN KEY(x,y) REFERENCES public.cp(b,a),CONSTRAINT cc_ck CHECK(v > x AND v > 0),CONSTRAINT cc_const CHECK(1=1))",
    );
    let sql = "SELECT table_catalog,table_schema,table_name,column_name,constraint_catalog,constraint_schema,constraint_name FROM information_schema.constraint_column_usage WHERE constraint_name IN ('cp_pk','cc_fk','cc_ck','cc_const') ORDER BY constraint_name,column_name";
    let m = ok(&mut socket, sql);
    assert_eq!(
        columns(&m[0].1).iter().map(|c| c.1).collect::<Vec<_>>(),
        vec![19; 7]
    );
    let rows: Vec<_> = m
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| cells(&m.1))
        .collect();
    assert_eq!(rows.len(), 6);
    assert_eq!(
        rows.iter().map(|r| (r[2], r[3], r[6])).collect::<Vec<_>>(),
        vec![
            (Some("cc"), Some("v"), Some("cc_ck")),
            (Some("cc"), Some("x"), Some("cc_ck")),
            (Some("cp"), Some("a"), Some("cc_fk")),
            (Some("cp"), Some("b"), Some("cc_fk")),
            (Some("cp"), Some("a"), Some("cp_pk")),
            (Some("cp"), Some("b"), Some("cp_pk"))
        ]
    );
    let zero = ok(
        &mut socket,
        &sql.replace("'cp_pk','cc_fk','cc_ck','cc_const'", "'cc_const'"),
    );
    assert_eq!(columns(&zero[0].1), columns(&m[0].1));
    assert!(!zero.iter().any(|m| m.0 == b'D'));
    ok(&mut socket, "ALTER TABLE public.cc RENAME COLUMN v TO vv");
    let renamed = ok(&mut socket, sql);
    assert!(
        renamed
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| cells(&m.1))
            .any(|r| r[3] == Some("vv"))
    );
    println!(
        "pg13_040_contract: scenarios=6 p fk check_deduplicated constant_zero zero_describe rename; shared domain TCP cleanup"
    );
}

#[test]
fn pg13_041_virtual_system_schema_is_not_a_database() {
    use crate::conn::{CancellationToken, SessionDriver};
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver =
        ConcreteSessionDriver::new_for_test(domain.clone(), BootstrapAuthMode::InsecureRootOnly);
    let context = driver.open_ctx(98441, 0, 45, "test", None).unwrap();
    let query = crate::pg_catalog::CatalogQuery::parse(
        "SELECT oid,datname FROM pg_catalog.pg_database WHERE datname='test'",
    )
    .unwrap()
    .unwrap();
    let token = CancellationToken::new();
    let before = query.execute(context.as_ref(), &token).unwrap();
    assert_eq!(before.rows.len(), 1);
    // Inject the actual registered virtual schema identity at the shared model
    // boundary, as a serving InfoSchema reload does; do not mock provider rows.
    let virtual_schema = astersql_infoschema::tables::information_schema_db();
    domain
        .ddl_drop_database("information_schema", true)
        .unwrap();
    domain
        .ddl_create_database_with_id("information_schema", false, Some(virtual_schema.id))
        .unwrap();
    assert_eq!(
        query.execute(context.as_ref(), &token).unwrap().rows,
        before.rows
    );
    let virtual_query = crate::pg_catalog::CatalogQuery::parse(
        "SELECT oid FROM pg_catalog.pg_database WHERE datname='information_schema'",
    )
    .unwrap()
    .unwrap();
    assert!(
        virtual_query
            .execute(context.as_ref(), &token)
            .unwrap()
            .rows
            .is_empty()
    );
    domain
        .ddl_create_database_with_id("invalid_native_id", false, Some(0))
        .unwrap();
    let error = query.execute(context.as_ref(), &token).unwrap_err();
    assert_eq!(crate::pg_conn::sqlstate(&error), "0A000");
    context.close().unwrap();
    domain.close();
}

#[test]
fn cat14_02() {
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP TABLE IF EXISTS test.cat14_02_unrelated")
            .unwrap();
        assert!(TcpStream::connect(addr).is_err());
    });
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
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let messages = query(socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
        assert_eq!(messages.last(), Some(&(b'Z', vec![b'I'])));
        messages
    }
    fn description(body: &[u8]) -> Vec<(u32, i16, i32, i16)> {
        let mut offset = 2;
        (0..u16::from_be_bytes(body[..2].try_into().unwrap()))
            .map(|_| {
                offset += body[offset..].iter().position(|b| *b == 0).unwrap() + 1;
                let value = (
                    u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 10..offset + 12].try_into().unwrap()),
                    i32::from_be_bytes(body[offset + 12..offset + 16].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 16..offset + 18].try_into().unwrap()),
                );
                offset += 18;
                value
            })
            .collect()
    }
    let sql = "SELECT oid,proname,pronamespace,proargtypes,prorettype,proacl FROM pg_catalog.pg_proc WHERE pronamespace=11 ORDER BY oid";
    let baseline = ok(&mut socket, sql);
    let types = vec![
        (26, 4, -1, 0),
        (19, 64, -1, 0),
        (26, 4, -1, 0),
        (30, -1, -1, 0),
        (26, 4, -1, 0),
        (1034, -1, -1, 0),
    ];
    assert_eq!(description(&baseline[0].1), types);
    let names = [
        "pg_get_function_arguments",
        "pg_get_function_result",
        "pg_get_function_sqlbody",
    ];
    let rows: Vec<_> = baseline
        .iter()
        .filter(|m| m.0 == b'D')
        .map(|m| m.1.clone())
        .collect();
    assert_eq!(
        rows,
        names
            .iter()
            .enumerate()
            .map(|(i, name)| row(&[
                Some(&(16000 + i).to_string()),
                Some(name),
                Some("11"),
                Some("26"),
                Some("25"),
                None,
            ]))
            .collect::<Vec<_>>()
    );
    let empty = ok(
        &mut socket,
        &sql.replace("pronamespace=11", "pronamespace=0"),
    );
    assert_eq!(description(&empty[0].1), types);
    assert!(!empty.iter().any(|m| m.0 == b'D'));
    let detail_sql = "SELECT proargnames,proargmodes,proallargtypes,proargdefaults,pronargs,pronargdefaults,provariadic,prokind,proretset,provolatile,proisstrict,prosecdef,proleakproof,proparallel,prolang,prosrc,proconfig FROM pg_proc ORDER BY oid";
    let detail = ok(&mut socket, detail_sql);
    let expected_oids = [
        1009, 1002, 1028, 194, 21, 21, 26, 18, 16, 18, 16, 16, 16, 18, 26, 25, 1009,
    ];
    assert_eq!(
        columns(&detail[0].1)
            .iter()
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        expected_oids
    );
    assert_eq!(
        detail
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        names
            .iter()
            .map(|name| row(&[
                Some("{function_oid}"),
                None,
                None,
                None,
                Some("1"),
                Some("0"),
                Some("0"),
                Some("f"),
                Some("f"),
                Some("s"),
                Some("f"),
                Some("f"),
                Some("f"),
                Some("u"),
                Some("12"),
                Some(name),
                None
            ]))
            .collect::<Vec<_>>()
    );
    let joined = ok(
        &mut socket,
        "SELECT p.oid,l.oid,l.lanname,t.oid,t.typname FROM pg_proc p JOIN pg_language l ON p.prolang=l.oid JOIN pg_type t ON p.prorettype=t.oid WHERE p.pronamespace=11 ORDER BY p.oid",
    );
    assert_eq!(
        joined
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        (16000..16003)
            .map(|oid| row(&[
                Some(&oid.to_string()),
                Some("12"),
                Some("internal"),
                Some("25"),
                Some("text")
            ]))
            .collect::<Vec<_>>()
    );
    let input = ok(
        &mut socket,
        "SELECT t.oid,t.typname FROM pg_type t WHERE t.oid=26",
    );
    assert_eq!(
        input.iter().find(|m| m.0 == b'D').unwrap().1,
        row(&[Some("26"), Some("oid")])
    );
    let inputs = ok(
        &mut socket,
        "SELECT p.oid,t.oid,t.typname FROM pg_proc p JOIN pg_type t ON p.proargtypes[0]=t.oid WHERE p.pronamespace=11 ORDER BY p.oid",
    );
    assert_eq!(
        inputs
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        (16000..16003)
            .map(|oid| row(&[Some(&oid.to_string()), Some("26"), Some("oid")]))
            .collect::<Vec<_>>()
    );
    // Missing cost/version facts must fail at binding, even for an empty relation.
    for field in ["procost", "prorows", "xmin"] {
        let sql = format!("SELECT {field} FROM pg_proc WHERE pronamespace=0");
        let error = query(&mut socket, &sql);
        assert!(
            error
                .iter()
                .any(|m| m.0 == b'E' && m.1.windows(7).any(|w| w == b"C0A000\0")),
            "{error:?}"
        );
        assert!(!error.iter().any(|m| m.0 == b'T' || m.0 == b'D'));
        assert_eq!(error.last(), Some(&(b'Z', vec![b'I'])));
        ok(&mut socket, "SELECT 1");
        send(
            &mut socket,
            b'P',
            &[
                b"bad\0".as_slice(),
                sql.as_bytes(),
                b"\0",
                &0u16.to_be_bytes(),
            ]
            .concat(),
        );
        send(&mut socket, b'S', b"");
        let error = until_ready(&mut socket);
        assert!(
            error
                .iter()
                .any(|m| m.0 == b'E' && m.1.windows(7).any(|w| w == b"C0A000\0")),
            "{error:?}"
        );
        assert!(!error.iter().any(|m| m.0 == b'1'));
        ok(&mut socket, "SELECT 1");
    }
    let ddl = query(
        &mut socket,
        "CREATE FUNCTION public.cat14_02_f(integer) RETURNS integer LANGUAGE SQL AS 'SELECT $1'",
    );
    assert!(
        ddl.iter()
            .any(|m| m.0 == b'E' && m.1.windows(7).any(|w| w == b"C0A000\0")),
        "{ddl:?}"
    );
    ok(&mut socket, "SELECT 1");
    send(&mut socket,b'P',&[b"no_user_ddl\0".as_slice(),b"/* bounded capability */ CREATE OR REPLACE FUNCTION public.cat14_02_f(bigint) RETURNS bigint LANGUAGE SQL AS 'SELECT $1'\0",&0u16.to_be_bytes()].concat());
    send(&mut socket, b'S', b"");
    let error = until_ready(&mut socket);
    assert!(
        error
            .iter()
            .any(|m| m.0 == b'E' && m.1.windows(7).any(|w| w == b"C0A000\0")),
        "{error:?}"
    );
    assert!(!error.iter().any(|m| m.0 == b'1'));
    ok(&mut socket, "SELECT 'CREATE FUNCTION' AS literal");
    let nonexistent = ok(
        &mut socket,
        "SELECT proname FROM pg_proc WHERE proname='cat14_02_f'",
    );
    assert!(!nonexistent.iter().any(|m| m.0 == b'D'));
    assert_eq!(
        query(&mut socket, "BEGIN").last(),
        Some(&(b'Z', vec![b'T']))
    );
    let error = query(&mut socket, "SELECT procost FROM pg_proc");
    assert!(
        error
            .iter()
            .any(|m| m.0 == b'E' && m.1.windows(7).any(|w| w == b"C0A000\0")),
        "{error:?}"
    );
    assert_eq!(error.last(), Some(&(b'Z', vec![b'E'])));
    ok(&mut socket, "ROLLBACK");
    ok(&mut socket, "SELECT 1");
    native
        .execute("CREATE TABLE test.cat14_02_unrelated(a int)")
        .unwrap();
    let after = ok(&mut socket, sql);
    assert_eq!(after, baseline);
    native
        .execute("DROP TABLE test.cat14_02_unrelated")
        .unwrap();
    // The extended path must retain the same descriptions with typed NULL and
    // binary parameter/result formats, and recover to idle after every Sync.
    let parameter_sql = sql.replace("pronamespace=11", "pronamespace=$1");
    send(
        &mut socket,
        b'P',
        &[
            b"cat14_02\0".as_slice(),
            parameter_sql.as_bytes(),
            b"\0",
            &1u16.to_be_bytes(),
            &26u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scat14_02\0");
    send(&mut socket, b'S', b"");
    let described = until_ready(&mut socket);
    assert!(!described.iter().any(|m| m.0 == b'E'), "{described:?}");
    assert_eq!(
        description(&described.iter().find(|m| m.0 == b'T').unwrap().1),
        types
    );
    for (value, binary) in [
        (Some(11u32), false),
        (None, false),
        (Some(0), true),
        (Some(11), true),
    ] {
        let parameter = match value {
            Some(v) if binary => [&4i32.to_be_bytes()[..], &v.to_be_bytes()].concat(),
            Some(v) => {
                let v = v.to_string();
                [&(v.len() as i32).to_be_bytes()[..], v.as_bytes()].concat()
            }
            None => (-1i32).to_be_bytes().to_vec(),
        };
        send(
            &mut socket,
            b'B',
            &[
                b"\0cat14_02\0".as_slice(),
                &1u16.to_be_bytes(),
                &i16::from(binary).to_be_bytes(),
                &1u16.to_be_bytes(),
                &parameter,
                &1u16.to_be_bytes(),
                &i16::from(binary).to_be_bytes(),
            ]
            .concat(),
        );
        send(&mut socket, b'D', b"P\0");
        send(
            &mut socket,
            b'E',
            &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
        );
        send(&mut socket, b'S', b"");
        let messages = until_ready(&mut socket);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{messages:?}");
        let mut expected = types.clone();
        for t in &mut expected {
            t.3 = i16::from(binary);
        }
        assert_eq!(
            description(&messages.iter().find(|m| m.0 == b'T').unwrap().1),
            expected
        );
        let data: Vec<_> = messages.iter().filter(|m| m.0 == b'D').collect();
        assert_eq!(data.len(), if value == Some(11) { 3 } else { 0 });
        if value == Some(11) && !binary {
            assert_eq!(data.iter().map(|m| m.1.clone()).collect::<Vec<_>>(), rows);
        }
        if value == Some(11) && binary {
            for (i, m) in data.iter().enumerate() {
                let vector = [
                    &1i32.to_be_bytes()[..],
                    &0i32.to_be_bytes(),
                    &26u32.to_be_bytes(),
                    &1i32.to_be_bytes(),
                    &0i32.to_be_bytes(),
                    &4i32.to_be_bytes(),
                    &26u32.to_be_bytes(),
                ]
                .concat();
                let values = [
                    Some((16000u32 + i as u32).to_be_bytes().to_vec()),
                    Some(names[i].as_bytes().to_vec()),
                    Some(11u32.to_be_bytes().to_vec()),
                    Some(vector),
                    Some(25u32.to_be_bytes().to_vec()),
                    None,
                ];
                let mut expected = 6u16.to_be_bytes().to_vec();
                for v in values {
                    match v {
                        Some(v) => {
                            expected.extend_from_slice(&(v.len() as i32).to_be_bytes());
                            expected.extend(v);
                        }
                        None => expected.extend_from_slice(&(-1i32).to_be_bytes()),
                    }
                }
                assert_eq!(m.1, expected);
            }
        }
        assert_eq!(messages.last(), Some(&(b'Z', vec![b'I'])));
    }
    send(
        &mut socket,
        b'P',
        &[
            b"cat14_detail\0".as_slice(),
            detail_sql.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(
        &mut socket,
        b'B',
        &[
            b"\0cat14_detail\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &1u16.to_be_bytes(),
            &1u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"P\0");
    send(
        &mut socket,
        b'E',
        &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut socket, b'S', b"");
    let detail_binary = until_ready(&mut socket);
    assert!(
        !detail_binary.iter().any(|m| m.0 == b'E'),
        "{detail_binary:?}"
    );
    let lengths = [-1, -1, -1, -1, 2, 2, 4, 1, 1, 1, 1, 1, 1, 1, 4, -1, -1];
    assert_eq!(
        description(&detail_binary.iter().find(|m| m.0 == b'T').unwrap().1),
        expected_oids
            .iter()
            .zip(lengths)
            .map(|(oid, len)| (*oid, len, -1, 1))
            .collect::<Vec<_>>()
    );
    assert_eq!(detail_binary.iter().filter(|m| m.0 == b'D').count(), 3);
    for message in detail_binary.iter().filter(|m| m.0 == b'D') {
        let mut pos = 2;
        for field in 0..17 {
            let len = i32::from_be_bytes(message.1[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if [1, 2, 3, 16].contains(&field) {
                assert_eq!(len, -1);
            } else {
                assert!(len >= 0);
            }
            if len >= 0 {
                pos += len as usize;
            }
        }
        assert_eq!(pos, message.1.len());
    }
    send(&mut socket, b'C', b"Scat14_detail\0");
    send(&mut socket, b'C', b"Scat14_02\0");
    send(&mut socket, b'S', b"");
    assert!(!until_ready(&mut socket).iter().any(|m| m.0 == b'E'));
    send(&mut socket, b'X', b"");
    println!(
        "cat14_02: three signatures; Q/extended text/binary/NULL/zero describe; language/type joins; cost/version Parse gates; DDL refusal; stable after unrelated DDL; cleanup"
    );
}

#[test]
fn cat14_03() {
    let (domain, _native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        assert!(TcpStream::connect(addr).is_err());
    });
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
    fn ok(socket: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let messages = query(socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
        assert_eq!(messages.last(), Some(&(b'Z', vec![b'I'])));
        messages
    }
    fn description(body: &[u8]) -> Vec<(u32, i16, i32, i16)> {
        let mut offset = 2;
        (0..u16::from_be_bytes(body[..2].try_into().unwrap()))
            .map(|_| {
                offset += body[offset..].iter().position(|b| *b == 0).unwrap() + 1;
                let value = (
                    u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 10..offset + 12].try_into().unwrap()),
                    i32::from_be_bytes(body[offset + 12..offset + 16].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 16..offset + 18].try_into().unwrap()),
                );
                offset += 18;
                value
            })
            .collect()
    }

    let sql = "SELECT pg_catalog.pg_get_function_arguments(oid),pg_catalog.pg_get_function_result(oid),pg_catalog.pg_get_function_sqlbody(oid) FROM pg_catalog.pg_proc WHERE oid=$1";
    let expected = row(&[Some("function_oid oid"), Some("text"), None]);
    let types = vec![(25, -1, -1, 0); 3];
    send(
        &mut socket,
        b'P',
        &[
            b"cat14_03\0".as_slice(),
            sql.as_bytes(),
            b"\0",
            &1u16.to_be_bytes(),
            &26u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scat14_03\0");
    send(&mut socket, b'S', b"");
    let described = until_ready(&mut socket);
    assert_eq!(described.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ");
    assert_eq!(
        described[1].1,
        [&1u16.to_be_bytes()[..], &26u32.to_be_bytes()].concat()
    );
    assert_eq!(description(&described[2].1), types);
    for binary in [false, true] {
        for oid in [
            Some(16000u32),
            Some(16001),
            Some(16002),
            None,
            Some(0),
            Some(u32::MAX),
        ] {
            let parameter = match oid {
                Some(v) if binary => [&4i32.to_be_bytes()[..], &v.to_be_bytes()].concat(),
                Some(v) => {
                    let v = v.to_string();
                    [&(v.len() as i32).to_be_bytes()[..], v.as_bytes()].concat()
                }
                None => (-1i32).to_be_bytes().to_vec(),
            };
            send(
                &mut socket,
                b'B',
                &[
                    b"\0cat14_03\0".as_slice(),
                    &1u16.to_be_bytes(),
                    &i16::from(binary).to_be_bytes(),
                    &1u16.to_be_bytes(),
                    &parameter,
                    &1u16.to_be_bytes(),
                    &i16::from(binary).to_be_bytes(),
                ]
                .concat(),
            );
            send(&mut socket, b'D', b"P\0");
            send(
                &mut socket,
                b'E',
                &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
            );
            send(&mut socket, b'S', b"");
            let messages = until_ready(&mut socket);
            assert!(
                !messages.iter().any(|m| m.0 == b'E'),
                "oid={oid:?} binary={binary}: {messages:?}"
            );
            assert_eq!(
                description(&messages.iter().find(|m| m.0 == b'T').unwrap().1),
                vec![(25, -1, -1, i16::from(binary)); 3]
            );
            let data = messages
                .iter()
                .filter(|m| m.0 == b'D')
                .map(|m| m.1.clone())
                .collect::<Vec<_>>();
            // Unknown identities filtered out before projection do not invoke a primitive.
            assert_eq!(
                data,
                if oid.is_some_and(|v| (16000..=16002).contains(&v)) {
                    vec![expected.clone()]
                } else {
                    vec![]
                }
            );
            assert_eq!(messages.last(), Some(&(b'Z', vec![b'I'])));
            send(&mut socket, b'C', b"P\0");
            send(&mut socket, b'S', b"");
            assert_eq!(
                until_ready(&mut socket)
                    .iter()
                    .map(|m| m.0)
                    .collect::<Vec<_>>(),
                b"3Z"
            );
        }
    }
    let joined = ok(
        &mut socket,
        "SELECT p.oid,p.proargtypes,p.prorettype,l.lanname,pg_get_function_arguments(p.oid),pg_get_function_result(p.oid),pg_get_function_sqlbody(p.oid) FROM pg_proc p JOIN pg_language l ON p.prolang=l.oid WHERE p.pronamespace=11 ORDER BY p.oid",
    );
    assert_eq!(
        joined
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        (16000..=16002)
            .map(|oid| row(&[
                Some(&oid.to_string()),
                Some("26"),
                Some("25"),
                Some("internal"),
                Some("function_oid oid"),
                Some("text"),
                None
            ]))
            .collect::<Vec<_>>()
    );
    let excluded = ok(
        &mut socket,
        "WITH system_languages AS (SELECT oid AS lang FROM pg_language WHERE lanname IN ('c','internal')) SELECT pg_get_function_arguments(oid),pg_get_function_result(oid),pg_get_function_sqlbody(oid) FROM pg_proc WHERE prolang NOT IN (SELECT lang FROM system_languages)",
    );
    assert_eq!(description(&excluded[0].1), types);
    assert!(!excluded.iter().any(|m| m.0 == b'D'));
    let nulls = ok(
        &mut socket,
        "SELECT pg_get_function_arguments(NULL::oid),pg_get_function_result(NULL::oid),pg_get_function_sqlbody(NULL::oid) FROM pg_language",
    );
    assert_eq!(description(&nulls[0].1), types);
    assert_eq!(
        nulls
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>(),
        vec![row(&[None, None, None])]
    );
    // Reject unknown names during Parse even when a predicate would yield no rows.
    for name in [
        "pg_get_function_arguments",
        "pg_get_function_result",
        "pg_get_function_sqlbody",
        "unsupported_user_function",
    ] {
        for arguments in ["", "oid,oid"] {
            let invalid = format!("SELECT {name}({arguments}) FROM pg_proc WHERE pronamespace=0");
            let rejected = query(&mut socket, &invalid);
            assert_eq!(rejected[0].0, b'E', "{invalid}: {rejected:?}");
            assert!(rejected[0].1.windows(7).any(|w| w == b"C0A000\0"));
            assert_eq!(rejected.last(), Some(&(b'Z', vec![b'I'])));
            send(
                &mut socket,
                b'P',
                &[
                    b"invalid\0".as_slice(),
                    invalid.as_bytes(),
                    b"\0",
                    &0u16.to_be_bytes(),
                ]
                .concat(),
            );
            send(&mut socket, b'S', b"");
            let rejected = until_ready(&mut socket);
            assert_eq!(rejected.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
            assert!(rejected[0].1.windows(7).any(|w| w == b"C0A000\0"));
            assert_eq!(ok(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
        }
    }
    let unknown = ok(&mut socket, "SELECT oid FROM pg_proc WHERE pronamespace=0");
    assert!(!unknown.iter().any(|m| m.0 == b'D'));
    let invalid = "SELECT unsupported_user_function(oid) FROM pg_proc WHERE pronamespace=0";
    let rejected = query(&mut socket, invalid);
    assert_eq!(rejected[0].0, b'E');
    assert!(rejected[0].1.windows(7).any(|w| w == b"C0A000\0"));
    assert_eq!(rejected.last(), Some(&(b'Z', vec![b'I'])));
    send(
        &mut socket,
        b'P',
        &[
            b"invalid\0".as_slice(),
            invalid.as_bytes(),
            b"\0",
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'S', b"");
    let rejected = until_ready(&mut socket);
    assert_eq!(rejected.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(rejected[0].1.windows(7).any(|w| w == b"C0A000\0"));
    // Direct invocation evaluates the OID and refuses unknown identities, unlike WHERE oid=$1.
    for name in [
        "pg_get_function_arguments",
        "pg_get_function_result",
        "pg_get_function_sqlbody",
    ] {
        let direct = format!("SELECT {name}($1) FROM pg_language");
        send(
            &mut socket,
            b'P',
            &[
                b"direct\0".as_slice(),
                direct.as_bytes(),
                b"\0",
                &1u16.to_be_bytes(),
                &26u32.to_be_bytes(),
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
        for oid in [0u32, u32::MAX] {
            let begun = query(&mut socket, "BEGIN");
            assert_eq!(begun.last(), Some(&(b'Z', vec![b'T'])));
            send(
                &mut socket,
                b'B',
                &[
                    b"\0direct\0".as_slice(),
                    &1u16.to_be_bytes(),
                    &1u16.to_be_bytes(),
                    &1u16.to_be_bytes(),
                    &4i32.to_be_bytes(),
                    &oid.to_be_bytes(),
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
            let rejected = until_ready(&mut socket);
            assert_eq!(
                rejected[0].0, b'2',
                "Bind must complete before Execute refusal: {rejected:?}"
            );
            let error = rejected
                .iter()
                .find(|m| m.0 == b'E')
                .expect("unknown OID must be refused");
            assert!(error.1.windows(7).any(|w| w == b"C0A000\0"), "{rejected:?}");
            assert!(!rejected.iter().any(|m| m.0 == b'D'));
            assert_eq!(rejected.last(), Some(&(b'Z', vec![b'E'])));
            assert_eq!(
                ok(&mut socket, "ROLLBACK").last(),
                Some(&(b'Z', vec![b'I']))
            );
            assert_eq!(ok(&mut socket, "SELECT 1")[1], (b'D', row(&[Some("1")])));
            println!(
                "{name}({oid}): Parse accepted; execution refused 0A000; T -> E -> I; recovery passed"
            );
        }
        send(&mut socket, b'C', b"Sdirect\0");
        send(&mut socket, b'S', b"");
        assert_eq!(
            until_ready(&mut socket)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"3Z"
        );
    }
    send(&mut socket, b'C', b"Scat14_03\0");
    send(&mut socket, b'S', b"");
    assert_eq!(
        until_ready(&mut socket)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"3Z"
    );
    send(&mut socket, b'X', b"");
    drop(socket);
    println!(
        "cat14_03: 26 -> 25/25/25, all three identities text/binary, NULL/filter empty, language JOIN/internal exclusion, arity/name Parse refusal, unknown OID execution refusal, recovery and cleanup"
    );
}

#[test]
fn cat14_04() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    astersql_session::runtime::EnableMockDDLForTest(&domain).unwrap();
    native.execute("CREATE DATABASE cat14_04_probe").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS cat14_04_probe")
            .unwrap();
        domain.close();
        assert!(TcpStream::connect(addr).is_err());
        println!("cat14_04 cleanup: database dropped, listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0cat14_04_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ok(s: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let r = query(s, sql);
        assert!(r.iter().all(|m| m.0 != b'E'), "{sql}: {r:?}");
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        r
    }
    fn data(r: &[(u8, Vec<u8>)]) -> Vec<Vec<u8>> {
        r.iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect()
    }
    fn scalar(s: &mut TcpStream, sql: &str) -> String {
        let r = data(&ok(s, sql));
        assert_eq!(r.len(), 1, "{sql}: {r:?}");
        let size = i32::from_be_bytes(r[0][2..6].try_into().unwrap());
        assert!(size >= 0);
        String::from_utf8(r[0][6..6 + size as usize].to_vec()).unwrap()
    }
    fn description(body: &[u8]) -> Vec<(u32, i16, i32, i16)> {
        let mut offset = 2;
        (0..u16::from_be_bytes(body[..2].try_into().unwrap()))
            .map(|_| {
                offset += body[offset..].iter().position(|b| *b == 0).unwrap() + 1;
                let v = (
                    u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 10..offset + 12].try_into().unwrap()),
                    i32::from_be_bytes(body[offset + 12..offset + 16].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 16..offset + 18].try_into().unwrap()),
                );
                offset += 18;
                v
            })
            .collect()
    }
    // Native DDL writes the shared Comment fields. The assertions below consume
    // the real persisted schema through the PG service, independently of the
    // separate PG COMMENT ON syntax/authorization capability.
    native.execute("CREATE TABLE cat14_04_probe.comment_contract (a int, b text COMMENT '列 中文', INDEX comment_idx(a) COMMENT '索引 é') COMMENT='表 é ''引号''' ").unwrap();
    ok(
        &mut socket,
        "ALTER TABLE public.comment_contract OWNER TO root",
    );
    let oid = scalar(
        &mut socket,
        "SELECT oid FROM pg_class WHERE relname='comment_contract'",
    );
    let oid_value = oid.parse::<u32>().unwrap();
    let index_oid = scalar(
        &mut socket,
        "SELECT oid FROM pg_class WHERE relname='comment_idx'",
    );
    let key = "SELECT c.oid,a.attnum FROM pg_class c JOIN pg_attribute a ON a.attrelid=c.oid WHERE c.relname='comment_contract' AND a.attname='b'";
    assert_eq!(
        data(&ok(&mut socket, key)),
        vec![row(&[Some(&oid), Some("2")])]
    );
    let sql = format!(
        "SELECT objoid,classoid,objsubid,description FROM pg_description WHERE objoid={oid} AND classoid=1259 ORDER BY objsubid"
    );
    let r = ok(&mut socket, &sql);
    assert_eq!(
        description(&r[0].1),
        vec![
            (26, 4, -1, 0),
            (26, 4, -1, 0),
            (23, 4, -1, 0),
            (25, -1, -1, 0)
        ]
    );
    assert_eq!(
        data(&r),
        vec![
            row(&[Some(&oid), Some("1259"), Some("0"), Some("表 é '引号'")]),
            row(&[Some(&oid), Some("1259"), Some("2"), Some("列 中文")])
        ]
    );
    let index_rows = ok(
        &mut socket,
        &format!(
            "SELECT objoid,classoid,objsubid,description FROM pg_description WHERE objoid={index_oid}"
        ),
    );
    assert_eq!(
        data(&index_rows),
        vec![row(&[
            Some(&index_oid),
            Some("1259"),
            Some("0"),
            Some("索引 é")
        ])]
    );
    // Reuse the same persistent factory as CREATE SEQUENCE. Its SQL table
    // options are not yet wired to SequenceInfo.Comment in the shared session.
    domain
        .ddl_create_table(
            "cat14_04_probe",
            astersql_meta_model::TableInfo {
                Name: astersql_parser_ast::NewCIStr("comment_seq"),
                State: astersql_meta_model::StatePublic,
                Sequence: Some(astersql_meta_model::SequenceInfo {
                    Start: 1,
                    Increment: 1,
                    MinValue: 1,
                    MaxValue: i64::MAX,
                    Cache: false,
                    CacheValue: 1,
                    Cycle: false,
                    Comment: "序列 😀".into(),
                }),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let sequence_oid = scalar(
        &mut socket,
        "SELECT oid FROM pg_class WHERE relname='comment_seq'",
    );
    assert_eq!(
        data(&ok(
            &mut socket,
            &format!(
                "SELECT objoid,classoid,objsubid,description FROM pg_description WHERE objoid={sequence_oid}"
            )
        )),
        vec![row(&[
            Some(&sequence_oid),
            Some("1259"),
            Some("0"),
            Some("序列 😀")
        ])]
    );
    let text = "第二行\n中文 é 😀 '引号' \\";
    // The shared SQL facade does not submit ALTER TABLE COMMENT yet. Use the
    // existing durable comment job, with the serving owner/worker installed by
    // the standard test factory, and wait for its real publication.
    let set_table_comment = |comment: &str| {
        let table = domain
            .table_by_name("cat14_04_probe", "comment_contract")
            .unwrap();
        let mut job = astersql_meta_model::group_3::Job {
            tp: 17,
            schema_id: table.DBID,
            table_id: table.ID,
            schema_name: "cat14_04_probe".into(),
            table_name: "comment_contract".into(),
            version: astersql_meta_model::group_3::JobVersion::V2,
            raw_args: serde_json::to_vec(&serde_json::json!({"comment":comment})).unwrap(),
            ..Default::default()
        };
        domain
            .ddl()
            .unwrap()
            .submit_persistent_job(&mut job)
            .unwrap();
        assert_eq!(job.state, astersql_meta_model::group_3::JobState::Synced);
        domain.reload().unwrap();
        assert_eq!(
            domain
                .table_by_name("cat14_04_probe", "comment_contract")
                .unwrap()
                .Comment,
            comment
        );
    };
    set_table_comment(text);
    // Both parameter and result formats must preserve every UTF-8 byte; the
    // dynamic OID is obtained above from this endpoint, never a fixture constant.
    let prepared = "SELECT objoid,classoid,objsubid,description FROM pg_description WHERE objoid=$1 AND classoid=1259 AND objsubid=0 AND description=$2";
    send(
        &mut socket,
        b'P',
        &[
            b"cat14_04\0".as_slice(),
            prepared.as_bytes(),
            b"\0",
            &2u16.to_be_bytes(),
            &26u32.to_be_bytes(),
            &25u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scat14_04\0");
    send(&mut socket, b'S', b"");
    let r = until_ready(&mut socket);
    assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ");
    assert_eq!(
        r[1].1,
        [
            &2u16.to_be_bytes()[..],
            &26u32.to_be_bytes(),
            &25u32.to_be_bytes()
        ]
        .concat()
    );
    assert_eq!(
        description(&r[2].1),
        vec![
            (26, 4, -1, 0),
            (26, 4, -1, 0),
            (23, 4, -1, 0),
            (25, -1, -1, 0)
        ]
    );
    for binary in [false, true] {
        let oid_parameter = if binary {
            oid_value.to_be_bytes().to_vec()
        } else {
            oid.as_bytes().to_vec()
        };
        let parameters = [
            &(oid_parameter.len() as i32).to_be_bytes()[..],
            &oid_parameter,
            &(text.len() as i32).to_be_bytes(),
            text.as_bytes(),
        ]
        .concat();
        send(
            &mut socket,
            b'B',
            &[
                b"\0cat14_04\0".as_slice(),
                &1u16.to_be_bytes(),
                &i16::from(binary).to_be_bytes(),
                &2u16.to_be_bytes(),
                &parameters,
                &1u16.to_be_bytes(),
                &i16::from(binary).to_be_bytes(),
            ]
            .concat(),
        );
        send(&mut socket, b'D', b"P\0");
        send(
            &mut socket,
            b'E',
            &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
        );
        send(&mut socket, b'S', b"");
        let r = until_ready(&mut socket);
        assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{r:?}");
        assert_eq!(
            description(&r[1].1),
            vec![
                (26, 4, -1, i16::from(binary)),
                (26, 4, -1, i16::from(binary)),
                (23, 4, -1, i16::from(binary)),
                (25, -1, -1, i16::from(binary))
            ]
        );
        let expected = if binary {
            [
                &4i16.to_be_bytes()[..],
                &4i32.to_be_bytes(),
                &oid_value.to_be_bytes(),
                &4i32.to_be_bytes(),
                &1259u32.to_be_bytes(),
                &4i32.to_be_bytes(),
                &0i32.to_be_bytes(),
                &(text.len() as i32).to_be_bytes(),
                text.as_bytes(),
            ]
            .concat()
        } else {
            row(&[Some(&oid), Some("1259"), Some("0"), Some(text)])
        };
        assert_eq!(data(&r), vec![expected]);
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
    }
    // NULL input is not an entity description and does not match any OID.
    send(
        &mut socket,
        b'B',
        &[
            b"\0cat14_04\0".as_slice(),
            &0u16.to_be_bytes(),
            &2u16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &(text.len() as i32).to_be_bytes(),
            text.as_bytes(),
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
    let r = until_ready(&mut socket);
    assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"2CZ");
    native
        .execute("ALTER TABLE cat14_04_probe.comment_contract MODIFY COLUMN b text COMMENT ''")
        .unwrap();
    assert_eq!(data(&ok(&mut socket, &sql)).len(), 1);
    native
        .execute(
            "ALTER TABLE cat14_04_probe.comment_contract MODIFY COLUMN b text COMMENT '列 中文'",
        )
        .unwrap();
    assert_eq!(data(&ok(&mut socket, &sql)).len(), 2);
    // Omitting COMMENT from a new native column declaration clears its field.
    native
        .execute("ALTER TABLE cat14_04_probe.comment_contract MODIFY COLUMN b text")
        .unwrap();
    assert_eq!(data(&ok(&mut socket, &sql)).len(), 1);
    set_table_comment("");
    assert!(data(&ok(&mut socket, &sql)).is_empty());
    assert_eq!(
        data(&ok(
            &mut socket,
            "SELECT c.relname,d.description FROM pg_class c LEFT JOIN pg_description d ON d.objoid=c.oid AND d.classoid=1259 AND d.objsubid=0 WHERE c.relname='comment_contract'"
        )),
        vec![row(&[Some("comment_contract"), None])]
    );
    set_table_comment("恢复表");
    native
        .execute(
            "ALTER TABLE cat14_04_probe.comment_contract MODIFY COLUMN b text COMMENT '列 中文'",
        )
        .unwrap();
    ok(
        &mut socket,
        "ALTER TABLE public.comment_contract RENAME COLUMN b TO renamed",
    );
    // The index belongs to a; remove it first so DROP proves the column slot
    // lifecycle without relying on unrelated implicit-index-drop behavior.
    native
        .execute("DROP INDEX comment_idx ON cat14_04_probe.comment_contract")
        .unwrap();
    assert!(
        data(&ok(
            &mut socket,
            &format!("SELECT description FROM pg_description WHERE objoid={index_oid}")
        ))
        .is_empty()
    );
    ok(
        &mut socket,
        "ALTER TABLE public.comment_contract DROP COLUMN a",
    );
    let key = key.replace("a.attname='b'", "a.attname='renamed'");
    assert_eq!(
        data(&ok(&mut socket, &key)),
        vec![row(&[Some(&oid), Some("2")])]
    );
    assert_eq!(
        data(&ok(&mut socket, &sql)),
        vec![
            row(&[Some(&oid), Some("1259"), Some("0"), Some("恢复表")]),
            row(&[Some(&oid), Some("1259"), Some("2"), Some("列 中文")])
        ]
    );
    ok(&mut socket, "DROP TABLE public.comment_contract");
    let old = ok(&mut socket, &sql);
    assert!(data(&old).is_empty());
    assert_eq!(
        description(&old[0].1),
        vec![
            (26, 4, -1, 0),
            (26, 4, -1, 0),
            (23, 4, -1, 0),
            (25, -1, -1, 0)
        ]
    );
    ok(
        &mut socket,
        "CREATE TABLE public.comment_contract(a integer,b text)",
    );
    let new_oid = scalar(
        &mut socket,
        "SELECT oid FROM pg_class WHERE relname='comment_contract'",
    );
    assert_ne!(new_oid, oid);
    assert!(
        data(&ok(
            &mut socket,
            &format!("SELECT description FROM pg_description WHERE objoid={new_oid}")
        ))
        .is_empty()
    );
    native
        .execute("DROP SEQUENCE cat14_04_probe.comment_seq")
        .unwrap();
    assert!(
        data(&ok(
            &mut socket,
            &format!("SELECT description FROM pg_description WHERE objoid={sequence_oid}")
        ))
        .is_empty()
    );
    // The deprecated one-argument signature stays unsupported; no fake NULL result.
    let rejected = query(
        &mut socket,
        "SELECT obj_description(oid) FROM pg_class WHERE relname='comment_contract'",
    );
    assert_eq!(rejected[0].0, b'E');
    assert!(rejected[0].1.windows(5).any(|v| v == b"0A000"));
    assert_eq!(rejected.last(), Some(&(b'Z', vec![b'I'])));
    assert_eq!(data(&ok(&mut socket, "SELECT 1")), vec![row(&[Some("1")])]);
    send(&mut socket, b'C', b"Scat14_04\0");
    send(&mut socket, b'S', b"");
    assert_eq!(
        until_ready(&mut socket)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"3Z"
    );
    send(&mut socket, b'X', b"");
    drop(socket);
    drop(cleanup);
    println!(
        "cat14_04: four relation kinds, OID26/26/int4/text descriptions, Unicode bytes text/binary, NULL/empty, LEFT JOIN, stable attnum rename/drop, old OID empty, recreation, refusal/recovery, cleanup"
    );
}

#[test]
fn cat14_05() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    astersql_session::runtime::EnableMockDDLForTest(&domain).unwrap();
    native.execute("CREATE DATABASE cat14_05_probe").unwrap();
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS cat14_05_probe")
            .unwrap();
        domain.close();
        assert!(TcpStream::connect(addr).is_err());
        println!("cat14_05 cleanup: database dropped, listener closed");
    });
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let startup = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0root\0database\0cat14_05_probe\0\0",
    ]
    .concat();
    socket
        .write_all(&((startup.len() + 4) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&startup).unwrap();
    while read(&mut socket).0 != b'Z' {}
    fn ok(s: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let r = query(s, sql);
        assert!(r.iter().all(|m| m.0 != b'E'), "{sql}: {r:?}");
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        r
    }
    fn data(r: &[(u8, Vec<u8>)]) -> Vec<Vec<u8>> {
        r.iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect()
    }
    fn scalar(s: &mut TcpStream, sql: &str) -> String {
        let r = data(&ok(s, sql));
        assert_eq!(r.len(), 1, "{sql}: {r:?}");
        let size = i32::from_be_bytes(r[0][2..6].try_into().unwrap());
        assert!(size >= 0);
        String::from_utf8(r[0][6..6 + size as usize].to_vec()).unwrap()
    }
    fn description(body: &[u8]) -> Vec<(u32, i16, i32, i16)> {
        let mut offset = 2;
        (0..u16::from_be_bytes(body[..2].try_into().unwrap()))
            .map(|_| {
                offset += body[offset..].iter().position(|b| *b == 0).unwrap() + 1;
                assert_eq!(
                    &body[offset..offset + 6],
                    &[0; 6],
                    "function expressions have no source attribute"
                );
                let v = (
                    u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 10..offset + 12].try_into().unwrap()),
                    i32::from_be_bytes(body[offset + 12..offset + 16].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 16..offset + 18].try_into().unwrap()),
                );
                offset += 18;
                v
            })
            .collect()
    }
    native.execute("CREATE TABLE cat14_05_probe.comment_contract (a int, b text COMMENT '列 中文', INDEX comment_idx(a) COMMENT '索引 é') COMMENT='表 é ''引号''' ").unwrap();
    let oid = scalar(
        &mut socket,
        "SELECT oid FROM pg_class WHERE relname='comment_contract'",
    );
    let sql = format!(
        "SELECT pg_catalog.obj_description(oid,'pg_class'),pg_catalog.col_description(oid,2) FROM pg_class WHERE oid={oid}"
    );
    let r = ok(&mut socket, &sql);
    assert_eq!(description(&r[0].1), vec![(25, -1, -1, 0); 2]);
    assert_eq!(data(&r), vec![row(&[Some("表 é '引号'"), Some("列 中文")])]);
    // Object identity includes its catalog; object descriptions exclude columns.
    for category in [
        "pg_proc",
        "pg_type",
        "pg_namespace",
        "pg_database",
        "missing",
        "PG_CLASS",
        "pg_catalog.pg_class",
    ] {
        let r = ok(
            &mut socket,
            &format!("SELECT obj_description({oid},'{category}') FROM pg_class WHERE oid={oid}"),
        );
        assert_eq!(data(&r), vec![row(&[None])], "{category}");
    }
    for expression in [
        "obj_description(NULL,'pg_class')".into(),
        format!("obj_description({oid},NULL)"),
        "obj_description(0,'pg_class')".into(),
        "col_description(NULL,2)".into(),
        format!("col_description({oid},NULL)"),
        format!("col_description({oid},-1)"),
        format!("col_description({oid},2147483647)"),
        "col_description(0,2)".into(),
    ] {
        assert_eq!(
            data(&ok(
                &mut socket,
                &format!("SELECT {expression} FROM pg_class WHERE oid={oid}")
            )),
            vec![row(&[None])],
            "{expression}"
        );
    }
    assert_eq!(
        data(&ok(
            &mut socket,
            "SELECT obj_description(oid,'pg_class') FROM pg_class WHERE relname='comment_idx'"
        )),
        vec![row(&[Some("索引 é")])]
    );
    assert_eq!(
        data(&ok(
            &mut socket,
            "SELECT col_description(c.oid,a.attnum) FROM pg_class c JOIN pg_attribute a ON a.attrelid=c.oid WHERE c.relname='comment_contract' AND a.attname='b'"
        )),
        vec![row(&[Some("列 中文")])]
    );
    // PG18.4/18.6 match objsubid literally: slot zero returns the object comment.
    assert_eq!(
        data(&ok(
            &mut socket,
            &format!("SELECT col_description({oid},0) FROM pg_class WHERE oid={oid}")
        )),
        vec![row(&[Some("表 é '引号'")])]
    );
    domain
        .ddl_create_table(
            "cat14_05_probe",
            astersql_meta_model::TableInfo {
                Name: astersql_parser_ast::NewCIStr("comment_seq"),
                State: astersql_meta_model::StatePublic,
                Sequence: Some(astersql_meta_model::SequenceInfo {
                    Start: 1,
                    Increment: 1,
                    MinValue: 1,
                    MaxValue: i64::MAX,
                    Cache: false,
                    CacheValue: 1,
                    Cycle: false,
                    Comment: "序列 😀".into(),
                }),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    assert_eq!(
        data(&ok(
            &mut socket,
            "SELECT obj_description(oid,'pg_class') FROM pg_class WHERE relname='comment_seq'"
        )),
        vec![row(&[Some("序列 😀")])]
    );
    // Update through the existing durable DDL job, not provider mutation.
    let set_table_comment = |comment: &str| {
        let table = domain
            .table_by_name("cat14_05_probe", "comment_contract")
            .unwrap();
        let mut job = astersql_meta_model::group_3::Job {
            tp: 17,
            schema_id: table.DBID,
            table_id: table.ID,
            schema_name: "cat14_05_probe".into(),
            table_name: "comment_contract".into(),
            version: astersql_meta_model::group_3::JobVersion::V2,
            raw_args: serde_json::to_vec(&serde_json::json!({"comment":comment})).unwrap(),
            ..Default::default()
        };
        domain
            .ddl()
            .unwrap()
            .submit_persistent_job(&mut job)
            .unwrap();
        assert_eq!(job.state, astersql_meta_model::group_3::JobState::Synced);
        domain.reload().unwrap();
        assert_eq!(
            domain
                .table_by_name("cat14_05_probe", "comment_contract")
                .unwrap()
                .Comment,
            comment
        );
    };
    set_table_comment("");
    assert_eq!(
        data(&ok(&mut socket, &sql)),
        vec![row(&[None, Some("列 中文")])]
    );
    let text = "第二行\n中文 é 😀 '引号' \\";
    set_table_comment(text);
    let prepared = format!(
        "SELECT obj_description($1,$2),col_description($1,$3) FROM pg_class WHERE oid={oid}"
    );
    send(
        &mut socket,
        b'P',
        &[
            b"cat14_05\0".as_slice(),
            prepared.as_bytes(),
            b"\0",
            &3u16.to_be_bytes(),
            &26u32.to_be_bytes(),
            &19u32.to_be_bytes(),
            &23u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut socket, b'D', b"Scat14_05\0");
    send(&mut socket, b'S', b"");
    let r = until_ready(&mut socket);
    assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ");
    assert_eq!(
        r[1].1,
        [
            &3u16.to_be_bytes()[..],
            &26u32.to_be_bytes(),
            &19u32.to_be_bytes(),
            &23u32.to_be_bytes()
        ]
        .concat()
    );
    assert_eq!(description(&r[2].1), vec![(25, -1, -1, 0); 2]);
    fn probe(s: &mut TcpStream, params: [Option<&[u8]>; 3], binary: bool) -> Vec<(u8, Vec<u8>)> {
        let mut bind = b"\0cat14_05\0".to_vec();
        bind.extend(1u16.to_be_bytes());
        bind.extend(i16::from(binary).to_be_bytes());
        bind.extend(3u16.to_be_bytes());
        for p in params {
            match p {
                Some(p) => {
                    bind.extend((p.len() as i32).to_be_bytes());
                    bind.extend(p);
                }
                None => bind.extend((-1i32).to_be_bytes()),
            }
        }
        bind.extend(1u16.to_be_bytes());
        bind.extend(i16::from(binary).to_be_bytes());
        send(s, b'B', &bind);
        send(s, b'D', b"P\0");
        send(s, b'E', &[b"\0".as_slice(), &0u32.to_be_bytes()].concat());
        send(s, b'S', b"");
        until_ready(s)
    }
    for binary in [false, true] {
        let params = if binary {
            vec![
                oid.parse::<u32>().unwrap().to_be_bytes().to_vec(),
                b"pg_class".to_vec(),
                2i32.to_be_bytes().to_vec(),
            ]
        } else {
            vec![oid.as_bytes().to_vec(), b"pg_class".to_vec(), b"2".to_vec()]
        };
        let values = [
            Some(params[0].as_slice()),
            Some(params[1].as_slice()),
            Some(params[2].as_slice()),
        ];
        let r = probe(&mut socket, values, binary);
        assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{r:?}");
        assert_eq!(
            description(&r[1].1),
            vec![(25, -1, -1, i16::from(binary)); 2]
        );
        assert_eq!(data(&r), vec![row(&[Some(text), Some("列 中文")])]);
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        for null in 0..3 {
            let mut nullable = values;
            nullable[null] = None;
            let r = probe(&mut socket, nullable, binary);
            assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{r:?}");
            assert_eq!(
                description(&r[1].1),
                vec![(25, -1, -1, i16::from(binary)); 2]
            );
            assert_eq!(
                data(&r),
                vec![row(&[
                    if null == 0 || null == 1 {
                        None
                    } else {
                        Some(text)
                    },
                    if null == 0 || null == 2 {
                        None
                    } else {
                        Some("列 中文")
                    }
                ])]
            );
            assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        }
        let long_name = "é".repeat(40);
        let r = probe(
            &mut socket,
            [values[0], Some(long_name.as_bytes()), values[2]],
            binary,
        );
        if binary {
            assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
            assert!(r[0].1.windows(6).any(|w| w == b"42622\0"));
        } else {
            assert_eq!(data(&r), vec![row(&[None, Some("列 中文")])]);
        }
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        for invalid in [b"pg_class\0".as_slice(), &[0xff][..]] {
            let r = probe(&mut socket, [values[0], Some(invalid), values[2]], binary);
            assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
            assert!(r[0].1.windows(6).any(|w| w == b"22021\0"));
            assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
            ok(&mut socket, "SELECT 1");
        }
    }
    native
        .execute("ALTER TABLE cat14_05_probe.comment_contract MODIFY COLUMN b text COMMENT ''")
        .unwrap();
    assert_eq!(data(&ok(&mut socket, &sql)), vec![row(&[Some(text), None])]);
    native
        .execute(
            "ALTER TABLE cat14_05_probe.comment_contract MODIFY COLUMN b text COMMENT '列 中文'",
        )
        .unwrap();
    ok(
        &mut socket,
        "ALTER TABLE public.comment_contract OWNER TO root",
    );
    ok(
        &mut socket,
        "ALTER TABLE public.comment_contract RENAME COLUMN b TO renamed",
    );
    native
        .execute("ALTER TABLE cat14_05_probe.comment_contract DROP INDEX comment_idx")
        .unwrap();
    ok(
        &mut socket,
        "ALTER TABLE public.comment_contract DROP COLUMN a",
    );
    assert_eq!(
        data(&ok(&mut socket, &sql)),
        vec![row(&[Some(text), Some("列 中文")])]
    );
    ok(&mut socket, "DROP TABLE public.comment_contract");
    assert_eq!(
        data(&ok(
            &mut socket,
            &format!(
                "SELECT obj_description({oid},'pg_class'),col_description({oid},2) FROM pg_database WHERE datname='cat14_05_probe'"
            )
        )),
        vec![row(&[None, None])]
    );
    native
        .execute("CREATE TABLE cat14_05_probe.comment_contract (a int,b text)")
        .unwrap();
    assert_ne!(
        scalar(
            &mut socket,
            "SELECT oid FROM pg_class WHERE relname='comment_contract'"
        ),
        oid
    );
    assert_eq!(
        data(&ok(
            &mut socket,
            "SELECT obj_description(oid,'pg_class'),col_description(oid,2) FROM pg_class WHERE relname='comment_contract'"
        )),
        vec![row(&[None, None])]
    );
    for expression in [
        "obj_description(oid)",
        "obj_description(oid,2)",
        "col_description(oid,'bad')",
    ] {
        let r = query(
            &mut socket,
            &format!("SELECT {expression} FROM pg_class WHERE relname='comment_contract'"),
        );
        let error = &r
            .iter()
            .find(|m| m.0 == b'E')
            .expect("binding must reject unsupported signature")
            .1;
        assert!(error.windows(6).any(|w| w == b"0A000\0"), "{r:?}");
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        ok(&mut socket, "SELECT 1");
    }
    send(&mut socket, b'C', b"Scat14_05\0");
    send(&mut socket, b'S', b"");
    assert_eq!(
        until_ready(&mut socket)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"3Z"
    );
    send(&mut socket, b'X', b"");
    drop(socket);
    drop(cleanup);
}

#[test]
fn cat14_06() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    astersql_session::runtime::EnableMockDDLForTest(&domain).unwrap();
    for db in ["cat14_06_db", "cat14_06_other"] {
        native.execute(&format!("CREATE DATABASE {db}")).unwrap();
        native
            .execute(&format!(
                "CREATE TABLE {db}.local_comment(a int) COMMENT='local only'"
            ))
            .unwrap();
    }
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
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let cleanup = Cleanup(|| {
        service.close();
        for db in ["cat14_06_db", "cat14_06_other"] {
            native
                .execute(&format!("DROP DATABASE IF EXISTS {db}"))
                .unwrap();
        }
        domain.close();
        assert!(TcpStream::connect(addr).is_err());
        println!("cat14_06 cleanup: own databases and listener removed");
    });
    fn ok(s: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let r = query(s, sql);
        assert!(r.iter().all(|m| m.0 != b'E'), "{sql}: {r:?}");
        assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
        r
    }
    fn data(r: &[(u8, Vec<u8>)]) -> Vec<Vec<u8>> {
        r.iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect()
    }
    fn description(body: &[u8]) -> Vec<(u32, i16, i32, i16)> {
        let mut offset = 2;
        (0..u16::from_be_bytes(body[..2].try_into().unwrap()))
            .map(|_| {
                offset += body[offset..].iter().position(|b| *b == 0).unwrap() + 1;
                let v = (
                    u32::from_be_bytes(body[offset + 6..offset + 10].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 10..offset + 12].try_into().unwrap()),
                    i32::from_be_bytes(body[offset + 12..offset + 16].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 16..offset + 18].try_into().unwrap()),
                );
                offset += 18;
                v
            })
            .collect()
    }
    fn origins(body: &[u8]) -> Vec<(u32, i16)> {
        let mut offset = 2;
        (0..u16::from_be_bytes(body[..2].try_into().unwrap()))
            .map(|_| {
                offset += body[offset..].iter().position(|b| *b == 0).unwrap() + 1;
                let value = (
                    u32::from_be_bytes(body[offset..offset + 4].try_into().unwrap()),
                    i16::from_be_bytes(body[offset + 4..offset + 6].try_into().unwrap()),
                );
                offset += 18;
                value
            })
            .collect()
    }
    let mut saved_oid = None;
    for db in ["cat14_06_db", "cat14_06_other"] {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let startup = [
            196608u32.to_be_bytes().as_slice(),
            format!("user\0root\0database\0{db}\0\0").as_bytes(),
        ]
        .concat();
        s.write_all(&((startup.len() + 4) as u32).to_be_bytes())
            .unwrap();
        s.write_all(&startup).unwrap();
        while read(&mut s).0 != b'Z' {}
        let sql = "SELECT objoid,classoid,description FROM pg_catalog.pg_shdescription";
        let r = ok(&mut s, sql);
        assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"TCZ");
        assert_eq!(
            description(&r[0].1),
            vec![(26, 4, -1, 0), (26, 4, -1, 0), (25, -1, -1, 0)]
        );
        assert_eq!(origins(&r[0].1), vec![(2396, 1), (2396, 2), (2396, 3)]);
        let r = ok(
            &mut s,
            "SELECT oid FROM pg_database WHERE datname='cat14_06_db'",
        );
        let rows = data(&r);
        assert_eq!(rows.len(), 1);
        let oid = String::from_utf8(rows[0][6..].to_vec()).unwrap();
        assert_eq!(saved_oid.get_or_insert(oid.clone()), &oid);
        let join = "SELECT d.oid,s.classoid,s.description FROM pg_database d LEFT JOIN pg_shdescription s ON s.objoid=d.oid AND s.classoid=1262 WHERE d.datname='cat14_06_db'";
        assert_eq!(
            data(&ok(&mut s, join)),
            vec![row(&[Some(&oid), None, None])]
        );
        // A real local description must never leak into the shared function.
        assert_eq!(
            data(&ok(
                &mut s,
                "SELECT description FROM pg_description WHERE description='local only'"
            )),
            vec![row(&[Some("local only")])]
        );
        for expr in [
            format!("shobj_description({oid},'pg_database')"),
            "shobj_description(0,'pg_database')".into(),
            "shobj_description(4294967295,'pg_authid')".into(),
            "shobj_description(NULL,'pg_database')".into(),
            format!("shobj_description({oid},NULL)"),
            format!("shobj_description({oid},'missing')"),
            "shobj_description(1259,'pg_class')".into(),
        ] {
            let r = ok(
                &mut s,
                &format!("SELECT pg_catalog.{expr} FROM pg_database WHERE datname='cat14_06_db'"),
            );
            assert_eq!(data(&r), vec![row(&[None])]);
            assert_eq!(description(&r[0].1), vec![(25, -1, -1, 0)]);
            assert_eq!(origins(&r[0].1), vec![(0, 0)]);
        }
        for target in ["DATABASE cat14_06_db", "ROLE root", "TABLESPACE pg_default"] {
            for value in ["'库 中文'", "''", "NULL"] {
                let sql = format!("COMMENT ON {target} IS {value}");
                let r = query(&mut s, &sql);
                assert_eq!(
                    r.iter().map(|m| m.0).collect::<Vec<_>>(),
                    b"EZ",
                    "{sql}: {r:?}"
                );
                assert!(r[0].1.windows(7).any(|v| v == b"C0A000\0"), "{sql}: {r:?}");
                assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
                assert!(data(&ok(&mut s, "SELECT description FROM pg_shdescription")).is_empty());
                assert_eq!(
                    data(&ok(&mut s, join)),
                    vec![row(&[Some(&oid), None, None])]
                );
            }
        }
        let sql = "SELECT shobj_description($1,$2) FROM pg_database WHERE datname='cat14_06_db'";
        send(
            &mut s,
            b'P',
            &[
                b"shared\0".as_slice(),
                sql.as_bytes(),
                b"\0",
                &2u16.to_be_bytes(),
                &26u32.to_be_bytes(),
                &19u32.to_be_bytes(),
            ]
            .concat(),
        );
        send(&mut s, b'D', b"Sshared\0");
        send(&mut s, b'S', b"");
        let r = until_ready(&mut s);
        assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ", "{r:?}");
        assert_eq!(
            r[1].1,
            [
                &2u16.to_be_bytes()[..],
                &26u32.to_be_bytes(),
                &19u32.to_be_bytes()
            ]
            .concat()
        );
        for binary in [false, true] {
            let bytes = if binary {
                oid.parse::<u32>().unwrap().to_be_bytes().to_vec()
            } else {
                oid.as_bytes().to_vec()
            };
            for null in [None, Some(0), Some(1)] {
                let mut bind = b"\0shared\0".to_vec();
                bind.extend(1u16.to_be_bytes());
                bind.extend(i16::from(binary).to_be_bytes());
                bind.extend(2u16.to_be_bytes());
                for (i, p) in [bytes.as_slice(), b"pg_database"].iter().enumerate() {
                    if null == Some(i) {
                        bind.extend((-1i32).to_be_bytes());
                    } else {
                        bind.extend((p.len() as i32).to_be_bytes());
                        bind.extend(*p);
                    }
                }
                bind.extend(1u16.to_be_bytes());
                bind.extend(i16::from(binary).to_be_bytes());
                send(&mut s, b'B', &bind);
                send(&mut s, b'D', b"P\0");
                send(
                    &mut s,
                    b'E',
                    &[b"\0".as_slice(), &0u32.to_be_bytes()].concat(),
                );
                send(&mut s, b'S', b"");
                let r = until_ready(&mut s);
                assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{r:?}");
                assert_eq!(description(&r[1].1), vec![(25, -1, -1, i16::from(binary))]);
                assert_eq!(data(&r), vec![row(&[None])]);
                assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
            }
        }
        // Bind failures recover at Sync without running a portal.
        for (value, state) in [("bad", "22P02"), ("4294967296", "22003")] {
            let mut bind = b"\0shared\0".to_vec();
            bind.extend(0u16.to_be_bytes());
            bind.extend(2u16.to_be_bytes());
            for p in [value.as_bytes(), b"pg_database"] {
                bind.extend((p.len() as i32).to_be_bytes());
                bind.extend(p);
            }
            bind.extend(0u16.to_be_bytes());
            send(&mut s, b'B', &bind);
            send(&mut s, b'S', b"");
            let r = until_ready(&mut s);
            assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
            assert!(
                r[0].1
                    .windows(7)
                    .any(|v| v == format!("C{state}\0").as_bytes())
            );
            assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
            ok(
                &mut s,
                "SELECT oid FROM pg_database WHERE datname='cat14_06_db'",
            );
        }
        let r = query(&mut s, "BEGIN");
        assert_eq!(r.last(), Some(&(b'Z', vec![b'T'])));
        let r = query(&mut s, "COMMENT ON DATABASE cat14_06_db IS 'rejected'");
        assert_eq!(r[0].0, b'E');
        assert!(r[0].1.windows(7).any(|v| v == b"C0A000\0"));
        assert_eq!(r.last(), Some(&(b'Z', vec![b'E'])));
        ok(&mut s, "ROLLBACK");
        assert_eq!(
            data(&ok(&mut s, join)),
            vec![row(&[Some(&oid), None, None])]
        );
        for bad in [
            "SELECT objsubid FROM pg_shdescription",
            "SELECT shobj_description(1) FROM pg_database",
            "SELECT shobj_description(1,2) FROM pg_database",
        ] {
            let r = query(&mut s, bad);
            assert_eq!(r[0].0, b'E');
            assert!(r[0].1.windows(7).any(|v| v == b"C0A000\0"));
            assert_eq!(r.last(), Some(&(b'Z', vec![b'I'])));
            ok(
                &mut s,
                "SELECT oid FROM pg_database WHERE datname='cat14_06_db'",
            );
        }
        send(&mut s, b'C', b"Sshared\0");
        send(&mut s, b'S', b"");
        until_ready(&mut s);
        send(&mut s, b'X', b"");
    }
    drop(cleanup);
}

#[test]
fn cat14_07() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE cat14_07_a").unwrap();
    native.execute("CREATE DATABASE cat14_07_b").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        Arc::new(ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::BootstrapRootAndNativeAccounts,
        )),
        Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        false,
    )
    .unwrap();
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        let mut cleaned = true;
        for db in ["cat14_07_a", "cat14_07_b", "cat14_07_forbidden"] {
            cleaned &= native
                .execute(&format!("DROP DATABASE IF EXISTS {db}"))
                .is_ok();
        }
        cleaned &= native
            .execute("DROP USER IF EXISTS 'cat14_07_reader'@'127.0.0.1'")
            .is_ok();
        for name in [
            "cat14_07_reader",
            "cat14_07_group",
            "cat14_07@quoted'role",
            "root@cat14_07",
            "cat14_07_locked",
            "cat14_07_legacy",
            "cat14_07_forbidden",
        ] {
            cleaned &= native
                .execute(&format!(
                    "DROP USER IF EXISTS '{}'@'%'",
                    name.replace('\'', "''")
                ))
                .is_ok();
        }
        assert!(
            cleaned,
            "all task-owned databases and accounts must be removed"
        );
        assert!(TcpStream::connect(address).is_err());
    });
    let connect = |user: &str, db: &str| {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let body = [
            196608u32.to_be_bytes().as_slice(),
            format!("user\0{user}\0database\0{db}\0\0").as_bytes(),
        ]
        .concat();
        socket
            .write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&body).unwrap();
        let messages = until_ready(&mut socket);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "login: {messages:?}");
        socket
    };
    let ok = |socket: &mut TcpStream, sql: &str| {
        let result = query(socket, sql);
        assert!(!result.iter().any(|m| m.0 == b'E'), "{sql}: {result:?}");
        assert_eq!(result.last().unwrap(), &(b'Z', b"I".to_vec()));
        result
    };
    let data = |messages: &[(u8, Vec<u8>)]| {
        messages
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>()
    };
    let mut admin = connect("root", "cat14_07_a");
    ok(&mut admin, "CREATE ROLE cat14_07_reader LOGIN");
    ok(&mut admin, "CREATE ROLE cat14_07_group NOLOGIN");
    ok(&mut admin, "CREATE ROLE \"cat14_07@quoted'role\" LOGIN");
    let mut reader = connect("cat14_07_reader", "cat14_07_a");
    let sql = "SELECT usename,usesysid,usesuper,passwd FROM pg_catalog.pg_user WHERE usename='cat14_07_reader'";
    let users = ok(&mut reader, sql);
    assert_eq!(
        columns(&users[0].1),
        vec![
            ("usename".into(), 19),
            ("usesysid".into(), 26),
            ("usesuper".into(), 16),
            ("passwd".into(), 25)
        ]
    );
    let roles = ok(
        &mut reader,
        "SELECT oid,rolname,rolcanlogin FROM pg_roles WHERE rolname IN ('cat14_07_reader','cat14_07_group') ORDER BY rolname",
    );
    assert_eq!(
        columns(&roles[0].1),
        vec![
            ("oid".into(), 26),
            ("rolname".into(), 19),
            ("rolcanlogin".into(), 16)
        ]
    );
    assert_eq!(data(&roles).len(), 2);
    let group_oid = native
        .pg_database_role_id("cat14_07_group")
        .unwrap()
        .to_string();
    assert_eq!(
        data(&roles)[0],
        row(&[Some(&group_oid), Some("cat14_07_group"), Some("f")])
    );

    let oid = native
        .pg_database_role_id("cat14_07_reader")
        .unwrap()
        .to_string();
    assert_eq!(
        data(&users),
        vec![row(&[
            Some("cat14_07_reader"),
            Some(&oid),
            Some("f"),
            Some("********")
        ])]
    );
    assert_eq!(
        data(&roles)[1],
        row(&[Some(&oid), Some("cat14_07_reader"), Some("t")])
    );
    assert!(
        data(&ok(
            &mut reader,
            "SELECT usename FROM pg_user WHERE usename='cat14_07_group'"
        ))
        .is_empty()
    );
    let mut other = connect("cat14_07_reader", "cat14_07_b");
    assert_eq!(ok(&mut other, sql), users);
    domain.force_full_reload_for_test().unwrap();
    assert_eq!(ok(&mut reader, sql), users);
    let denied = query(&mut reader, "SELECT rolpassword FROM pg_authid");
    assert_eq!(denied.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(denied[0].1.windows(5).any(|w| w == b"42501"), "{denied:?}");
    assert_eq!(data(&ok(&mut reader, "SELECT 1")), vec![row(&[Some("1")])]);
    let denied = query(&mut reader, "SELECT User FROM mysql.user");
    assert!(
        denied.iter().any(|m| m.0 == b'E'),
        "native user table must remain private: {denied:?}"
    );
    assert_eq!(ok(&mut reader, sql), users);
    for promote in [false, true] {
        if promote {
            native
                .execute("UPDATE mysql.user SET Super_priv='Y' WHERE User='root'")
                .unwrap();
        }
        let legacy = query(
            &mut admin,
            "SELECT usesuper FROM pg_user WHERE usename='root'",
        );
        assert_eq!(legacy.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
        assert!(legacy[0].1.windows(5).any(|w| w == b"0A000"));
    }
    // The native SUPER bit is not evidence of a PG role policy.
    native
        .execute("CREATE USER 'cat14_07_legacy'@'%' IDENTIFIED WITH mysql_native_password BY ''")
        .unwrap();
    native
        .execute("GRANT SUPER ON *.* TO 'cat14_07_legacy'@'%'")
        .unwrap();
    let old = query(
        &mut reader,
        "SELECT usesuper FROM pg_user WHERE usename='cat14_07_legacy'",
    );
    assert_eq!(old.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(old[0].1.windows(5).any(|w| w == b"0A000"));
    assert_eq!(ok(&mut reader, sql), users);

    native.execute("CREATE USER 'cat14_07_locked'@'%' IDENTIFIED WITH mysql_native_password BY '' ACCOUNT LOCK").unwrap();
    assert_eq!(
        data(&ok(
            &mut reader,
            "SELECT usename FROM pg_user WHERE usename='cat14_07_locked'"
        )),
        vec![row(&[Some("cat14_07_locked")])]
    );
    let quoted = native
        .pg_database_role_id("cat14_07@quoted'role")
        .unwrap()
        .to_string();
    assert_eq!(
        data(&ok(
            &mut reader,
            "SELECT oid,rolname FROM pg_roles WHERE rolname='cat14_07@quoted''role'"
        )),
        vec![row(&[Some(&quoted), Some("cat14_07@quoted'role")])]
    );
    let mut quoted_login = connect("cat14_07@quoted'role", "cat14_07_a");
    assert_eq!(
        data(&ok(
            &mut quoted_login,
            "SELECT usename FROM pg_user WHERE usename='cat14_07@quoted''role'"
        )),
        vec![row(&[Some("cat14_07@quoted'role")])]
    );
    send(&mut quoted_login, b'X', &[]);

    ok(&mut admin, "CREATE ROLE \"root@cat14_07\" LOGIN");
    let mut impersonator = connect("root@cat14_07", "cat14_07_a");
    let denied = query(&mut impersonator, "SELECT oid FROM pg_authid");
    assert_eq!(denied.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(denied[0].1.windows(5).any(|w| w == b"42501"));
    send(&mut impersonator, b'X', &[]);
    let denied = query(&mut reader, "CREATE ROLE cat14_07_forbidden LOGIN");
    assert!(denied[0].1.windows(5).any(|w| w == b"42501"));
    assert!(
        data(&ok(
            &mut reader,
            "SELECT rolname FROM pg_roles WHERE rolname='cat14_07_forbidden'"
        ))
        .is_empty()
    );

    // A reported NOCREATEDB declaration must also reject the actual operation.
    let denied = query(&mut reader, "CREATE DATABASE cat14_07_forbidden");
    assert_eq!(
        denied.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"EZ",
        "{denied:?}"
    );
    assert!(denied[0].1.windows(5).any(|w| w == b"42501"));

    // Temporary account locks must not change LOGIN membership, and unlocking
    // a native backing account must not grant a PG NOLOGIN role login rights.
    native
        .execute("ALTER USER 'cat14_07_group'@'%' ACCOUNT UNLOCK")
        .unwrap();
    let mut rejected = TcpStream::connect(address).unwrap();
    rejected
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let body = [
        196608u32.to_be_bytes().as_slice(),
        b"user\0cat14_07_group\0database\0cat14_07_a\0\0",
    ]
    .concat();
    rejected
        .write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    rejected.write_all(&body).unwrap();
    let fatal = read(&mut rejected);
    assert_eq!(fatal.0, b'E');
    assert!(fatal.1.windows(5).any(|w| w == b"28P01"));
    drop(rejected);

    let old_group = native.pg_database_role_id("cat14_07_group").unwrap();
    ok(&mut admin, "DROP ROLE cat14_07_group");
    assert!(
        data(&ok(
            &mut reader,
            "SELECT oid FROM pg_roles WHERE rolname='cat14_07_group'"
        ))
        .is_empty()
    );
    ok(&mut admin, "CREATE ROLE cat14_07_group NOLOGIN");
    assert_ne!(
        native.pg_database_role_id("cat14_07_group").unwrap(),
        old_group
    );

    // Ambiguous native identities refuse the entire read before returning rows.
    native
        .execute(
            "CREATE USER 'cat14_07_reader'@'127.0.0.1' IDENTIFIED WITH mysql_native_password BY ''",
        )
        .unwrap();
    let ambiguous = query(&mut reader, "SELECT usename FROM pg_user");
    assert_eq!(ambiguous.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(ambiguous[0].1.windows(5).any(|w| w == b"0A000"));
    native
        .execute("DROP USER 'cat14_07_reader'@'127.0.0.1'")
        .unwrap();
    assert_eq!(
        ok(&mut reader, sql),
        users,
        "dropping the conflicting Host must preserve the original OID"
    );

    native
        .execute("GRANT SELECT ON mysql.user TO 'cat14_07_reader'@'%'")
        .unwrap();
    assert_eq!(ok(&mut reader, sql), users);
    native
        .execute("REVOKE SELECT ON mysql.user FROM 'cat14_07_reader'@'%'")
        .unwrap();
    assert_eq!(ok(&mut reader, sql), users);

    let safe = "SELECT oid,rolname,rolcanlogin,rolpassword,rolconfig,rolvaliduntil FROM pg_roles WHERE rolname=$1";
    let parse = [
        b"cat14_07_stmt\0".as_slice(),
        safe.as_bytes(),
        b"\0",
        &1u16.to_be_bytes(),
        &25u32.to_be_bytes(),
    ]
    .concat();
    send(&mut reader, b'P', &parse);
    let bind = [
        b"cat14_07_portal\0cat14_07_stmt\0".as_slice(),
        &0u16.to_be_bytes(),
        &1u16.to_be_bytes(),
        &(b"cat14_07_reader".len() as i32).to_be_bytes(),
        b"cat14_07_reader",
        &1u16.to_be_bytes(),
        &1u16.to_be_bytes(),
    ]
    .concat();
    send(&mut reader, b'B', &bind);
    send(&mut reader, b'D', b"Pcat14_07_portal\0");
    send(
        &mut reader,
        b'E',
        &[b"cat14_07_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut reader, b'S', &[]);
    let binary = until_ready(&mut reader);
    assert_eq!(
        binary.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"12TDCZ",
        "{binary:?}"
    );
    assert_eq!(
        columns(&binary[2].1),
        vec![
            ("oid".into(), 26),
            ("rolname".into(), 19),
            ("rolcanlogin".into(), 16),
            ("rolpassword".into(), 25),
            ("rolconfig".into(), 1009),
            ("rolvaliduntil".into(), 1184)
        ]
    );
    let mut expected = 6i16.to_be_bytes().to_vec();
    for value in [
        Some((oid.parse::<u32>().unwrap()).to_be_bytes().to_vec()),
        Some(b"cat14_07_reader".to_vec()),
        Some(vec![1]),
        Some(b"********".to_vec()),
        None,
        None,
    ] {
        match value {
            Some(v) => {
                expected.extend_from_slice(&(v.len() as i32).to_be_bytes());
                expected.extend(v);
            }
            None => expected.extend_from_slice(&(-1i32).to_be_bytes()),
        }
    }
    assert_eq!(data(&binary), vec![expected]);
    let mut cursor = 2;
    let mut descriptors = Vec::new();
    for _ in 0..6 {
        cursor += binary[2].1[cursor..].iter().position(|b| *b == 0).unwrap() + 1;
        let field = &binary[2].1[cursor..cursor + 18];
        descriptors.push((
            u32::from_be_bytes(field[6..10].try_into().unwrap()),
            i16::from_be_bytes(field[10..12].try_into().unwrap()),
            i32::from_be_bytes(field[12..16].try_into().unwrap()),
            i16::from_be_bytes(field[16..18].try_into().unwrap()),
        ));
        cursor += 18;
    }
    assert_eq!(
        descriptors,
        vec![
            (26, 4, -1, 1),
            (19, 64, -1, 1),
            (16, 1, -1, 1),
            (25, -1, -1, 1),
            (1009, -1, -1, 1),
            (1184, 8, -1, 1)
        ]
    );
    send(&mut reader, b'C', b"Pcat14_07_portal\0");
    send(&mut reader, b'S', &[]);
    assert_eq!(
        until_ready(&mut reader)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"3Z"
    );
    let null_bind = [
        b"cat14_07_portal\0cat14_07_stmt\0".as_slice(),
        &0u16.to_be_bytes(),
        &1u16.to_be_bytes(),
        &(-1i32).to_be_bytes(),
        &1u16.to_be_bytes(),
        &0u16.to_be_bytes(),
    ]
    .concat();
    send(&mut reader, b'B', &null_bind);
    send(&mut reader, b'D', b"Pcat14_07_portal\0");
    send(
        &mut reader,
        b'E',
        &[b"cat14_07_portal\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut reader, b'S', &[]);
    let empty = until_ready(&mut reader);
    assert_eq!(
        empty.iter().map(|m| m.0).collect::<Vec<_>>(),
        b"2TCZ",
        "{empty:?}"
    );
    assert_eq!(columns(&empty[1].1), columns(&binary[2].1));
    assert_eq!(empty.last().unwrap(), &(b'Z', b"I".to_vec()));

    send(&mut reader, b'C', b"Pcat14_07_portal\0");
    send(&mut reader, b'C', b"Scat14_07_stmt\0");
    send(&mut reader, b'S', &[]);
    assert_eq!(
        until_ready(&mut reader)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"33Z"
    );
    assert_eq!(
        query(&mut reader, "BEGIN").last().unwrap(),
        &(b'Z', b"T".to_vec())
    );
    let denied = query(&mut reader, "SELECT passwd FROM pg_shadow");
    assert_eq!(denied.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(denied[0].1.windows(5).any(|w| w == b"42501"));
    assert_eq!(denied.last().unwrap(), &(b'Z', b"E".to_vec()));
    assert_eq!(
        query(&mut reader, "ROLLBACK").last().unwrap(),
        &(b'Z', b"I".to_vec())
    );
    assert_eq!(ok(&mut reader, sql), users);

    native
        .execute("ALTER USER 'cat14_07_reader'@'%' PASSWORD EXPIRE")
        .unwrap();
    // Native expiration has no verified absolute PG valuntil mapping. Refuse that
    // field without discarding the independent, explicitly declared PG attributes.
    assert_eq!(ok(&mut reader, sql), users);
    let expired = query(
        &mut reader,
        "SELECT valuntil FROM pg_user WHERE usename='cat14_07_reader'",
    );
    assert_eq!(expired.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(expired[0].1.windows(5).any(|w| w == b"0A000"));
    let raw = query(&mut admin, "SELECT passwd FROM pg_shadow");
    assert_eq!(raw.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
    assert!(raw[0].1.windows(5).any(|w| w == b"0A000"));
    // A migrated stable OID can be known while old role declarations remain unknown.
    let legacy_oid = native.pg_database_role_id("cat14_07_legacy").unwrap();
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&astersql_kv::Context::default(), store, true, |_, txn| {
                txn.Delete(astersql_meta::transaction_meta_hash_key(
                    b"PgDatabaseRoleProperties",
                    legacy_oid.to_string().as_bytes(),
                ))
            })
        })
        .unwrap();
    assert_eq!(
        data(&ok(
            &mut reader,
            "SELECT oid,rolname FROM pg_roles WHERE rolname='cat14_07_legacy'"
        )),
        vec![row(&[
            Some(&legacy_oid.to_string()),
            Some("cat14_07_legacy")
        ])]
    );
    for sql in [
        "SELECT rolcanlogin FROM pg_roles WHERE rolname='cat14_07_legacy'",
        "SELECT usename FROM pg_user",
    ] {
        let unknown = query(&mut reader, sql);
        assert_eq!(unknown.iter().map(|m| m.0).collect::<Vec<_>>(), b"EZ");
        assert!(unknown[0].1.windows(5).any(|w| w == b"0A000"));
    }
    assert_eq!(
        native.pg_database_role_id("cat14_07_legacy").unwrap(),
        legacy_oid
    );
    native.execute("DROP USER 'cat14_07_legacy'@'%'").unwrap();
    assert_eq!(ok(&mut reader, sql), users);
    send(&mut reader, b'X', &[]);
    send(&mut other, b'X', &[]);
    send(&mut admin, b'X', &[]);
}

#[test]
fn cat14_08() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE cat14_08_db").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        Arc::new(ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::BootstrapRootAndNativeAccounts,
        )),
        Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        false,
    )
    .unwrap();
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        let mut cleaned = native
            .execute("DROP DATABASE IF EXISTS cat14_08_db")
            .is_ok();
        for name in ["cat14_08_reader", "cat14_08_group", "cat14_08_forbidden"] {
            cleaned &= native
                .execute(&format!("DROP USER IF EXISTS '{name}'@'%'"))
                .is_ok();
        }
        assert!(cleaned, "task-owned objects must be removed");
        assert!(TcpStream::connect(address).is_err());
    });
    let connect = |name: &str| {
        let mut socket = TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let body = [
            196608u32.to_be_bytes().as_slice(),
            format!("user\0{name}\0database\0cat14_08_db\0\0").as_bytes(),
        ]
        .concat();
        socket
            .write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(&body).unwrap();
        let messages = until_ready(&mut socket);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "login: {messages:?}");
        socket
    };
    let ok = |socket: &mut TcpStream, sql: &str| {
        let messages = query(socket, sql);
        assert!(!messages.iter().any(|m| m.0 == b'E'), "{sql}: {messages:?}");
        assert_eq!(messages.last(), Some(&(b'Z', vec![b'I'])));
        messages
    };
    let data = |messages: &[(u8, Vec<u8>)]| {
        messages
            .iter()
            .filter(|m| m.0 == b'D')
            .map(|m| m.1.clone())
            .collect::<Vec<_>>()
    };
    let denied = |messages: &[(u8, Vec<u8>)], state: &[u8], status: u8| {
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"EZ",
            "{messages:?}"
        );
        assert!(
            messages[0].1.windows(state.len()).any(|w| w == state),
            "{messages:?}"
        );
        assert_eq!(messages.last(), Some(&(b'Z', vec![status])));
    };
    let mut admin = connect("root");
    ok(&mut admin, "CREATE ROLE cat14_08_reader LOGIN");
    ok(&mut admin, "CREATE ROLE cat14_08_group NOLOGIN");
    let mut reader = connect("cat14_08_reader");
    let projection = "r.rolname,r.rolsuper,r.rolinherit,r.rolcreaterole,r.rolcreatedb,r.rolcanlogin,r.rolconnlimit,r.rolvaliduntil,r.rolreplication,r.rolbypassrls,r.rolpassword,r.rolconfig";
    let sql =
        format!("SELECT {projection} FROM pg_catalog.pg_roles r WHERE r.rolname='cat14_08_reader'");
    let expected_text = row(&[
        Some("cat14_08_reader"),
        Some("f"),
        Some("t"),
        Some("f"),
        Some("f"),
        Some("t"),
        Some("-1"),
        None,
        Some("f"),
        Some("f"),
        Some("********"),
        None,
    ]);
    let text = ok(&mut reader, &sql);
    assert_eq!(data(&text), vec![expected_text.clone()]);
    let expected_types = [
        (19, 64),
        (16, 1),
        (16, 1),
        (16, 1),
        (16, 1),
        (16, 1),
        (23, 4),
        (1184, 8),
        (16, 1),
        (16, 1),
        (25, -1),
        (1009, -1),
    ];
    let descriptors = |body: &[u8]| {
        let mut offset = 2;
        let count = i16::from_be_bytes(body[..2].try_into().unwrap());
        (0..count)
            .map(|_| {
                offset += body[offset..].iter().position(|b| *b == 0).unwrap() + 1;
                let field = &body[offset..offset + 18];
                offset += 18;
                (
                    u32::from_be_bytes(field[6..10].try_into().unwrap()),
                    i16::from_be_bytes(field[10..12].try_into().unwrap()),
                    i32::from_be_bytes(field[12..16].try_into().unwrap()),
                    i16::from_be_bytes(field[16..18].try_into().unwrap()),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        descriptors(&text[0].1),
        expected_types
            .iter()
            .map(|(oid, len)| (*oid, *len, -1, 0))
            .collect::<Vec<_>>()
    );
    // The same persisted declaration must survive metadata reload and independent connections.
    domain.force_full_reload_for_test().unwrap();
    let mut other = connect("cat14_08_reader");
    assert_eq!(data(&ok(&mut other, &sql)), vec![expected_text.clone()]);
    let group = ok(
        &mut reader,
        "SELECT rolinherit,rolcreaterole,rolcanlogin,rolconnlimit FROM pg_roles WHERE rolname='cat14_08_group'",
    );
    assert_eq!(
        data(&group),
        vec![row(&[Some("t"), Some("f"), Some("f"), Some("-1")])]
    );
    // Persisted NOCREATEROLE has to agree with the actual operation, including after native grants.
    for grant in [false, true, false] {
        if grant {
            native
                .execute("GRANT CREATE ROLE, SUPER ON *.* TO 'cat14_08_reader'@'%'")
                .unwrap();
        } else {
            native
                .execute("REVOKE CREATE ROLE, SUPER ON *.* FROM 'cat14_08_reader'@'%'")
                .unwrap();
        }
        denied(
            &query(&mut reader, "CREATE ROLE cat14_08_forbidden LOGIN"),
            b"42501",
            b'I',
        );
        denied(
            &query(&mut reader, "DROP ROLE cat14_08_group"),
            b"42501",
            b'I',
        );
        assert_eq!(data(&ok(&mut reader, &sql)), vec![expected_text.clone()]);
    }
    assert!(
        data(&ok(
            &mut reader,
            "SELECT rolname FROM pg_roles WHERE rolname='cat14_08_forbidden'"
        ))
        .is_empty()
    );
    // Text and binary Execute share one parameterized declaration and retain NULLs.
    let prepared = format!("SELECT {projection} FROM pg_roles r WHERE r.rolname=$1");
    send(
        &mut reader,
        b'P',
        &[
            b"attrs\0".as_slice(),
            prepared.as_bytes(),
            b"\0",
            &1u16.to_be_bytes(),
            &25u32.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut reader, b'S', &[]);
    assert_eq!(
        until_ready(&mut reader)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"1Z"
    );
    for format in [0i16, 1] {
        send(
            &mut reader,
            b'B',
            &[
                b"p\0attrs\0".as_slice(),
                &0u16.to_be_bytes(),
                &1u16.to_be_bytes(),
                &(b"cat14_08_reader".len() as i32).to_be_bytes(),
                b"cat14_08_reader",
                &1u16.to_be_bytes(),
                &format.to_be_bytes(),
            ]
            .concat(),
        );
        send(&mut reader, b'D', b"Pp\0");
        send(
            &mut reader,
            b'E',
            &[b"p\0".as_slice(), &0u32.to_be_bytes()].concat(),
        );
        send(&mut reader, b'S', &[]);
        let messages = until_ready(&mut reader);
        assert_eq!(
            messages.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"2TDCZ",
            "{messages:?}"
        );
        assert_eq!(
            descriptors(&messages[1].1),
            expected_types
                .iter()
                .map(|(oid, len)| (*oid, *len, -1, format))
                .collect::<Vec<_>>()
        );
        if format == 0 {
            assert_eq!(data(&messages), vec![expected_text.clone()]);
        } else {
            let mut expected = 12i16.to_be_bytes().to_vec();
            for value in [
                Some(b"cat14_08_reader".to_vec()),
                Some(vec![0]),
                Some(vec![1]),
                Some(vec![0]),
                Some(vec![0]),
                Some(vec![1]),
                Some((-1i32).to_be_bytes().to_vec()),
                None,
                Some(vec![0]),
                Some(vec![0]),
                Some(b"********".to_vec()),
                None,
            ] {
                if let Some(v) = value {
                    expected.extend_from_slice(&(v.len() as i32).to_be_bytes());
                    expected.extend(v);
                } else {
                    expected.extend_from_slice(&(-1i32).to_be_bytes());
                }
            }
            assert_eq!(data(&messages), vec![expected]);
        }
        send(&mut reader, b'C', b"Pp\0");
        send(&mut reader, b'S', &[]);
        assert_eq!(
            until_ready(&mut reader)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            b"3Z"
        );
    }
    send(
        &mut reader,
        b'B',
        &[
            b"p\0attrs\0".as_slice(),
            &0u16.to_be_bytes(),
            &1u16.to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0u16.to_be_bytes(),
        ]
        .concat(),
    );
    send(&mut reader, b'D', b"Pp\0");
    send(
        &mut reader,
        b'E',
        &[b"p\0".as_slice(), &0u32.to_be_bytes()].concat(),
    );
    send(&mut reader, b'S', &[]);
    let nulls = until_ready(&mut reader);
    assert_eq!(nulls.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TCZ");
    assert_eq!(descriptors(&nulls[1].1), descriptors(&text[0].1));
    send(&mut reader, b'C', b"Pp\0");
    send(&mut reader, b'C', b"Sattrs\0");
    send(&mut reader, b'S', &[]);
    assert_eq!(
        until_ready(&mut reader)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        b"33Z"
    );
    // Simulate an older persisted declaration; do not infer missing fields on read.
    let oid = native.pg_database_role_id("cat14_08_reader").unwrap();
    let key = astersql_meta::transaction_meta_hash_key(
        b"PgDatabaseRoleProperties",
        oid.to_string().as_bytes(),
    );
    let saved = domain
        .storage_handle()
        .with_storage(|store| {
            let mut txn = store.Begin(&[])?;
            let value = txn.Get(&astersql_kv::Context::default(), key.clone(), &[])?;
            txn.Rollback()?;
            Ok::<_, astersql_kv::errors::SharedError>(value.Value)
        })
        .unwrap();
    let mut old: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    for field in ["inherit", "create_role", "connection_limit"] {
        assert!(
            old["attributes"]
                .as_object_mut()
                .unwrap()
                .remove(field)
                .is_some(),
            "new defaults must be persisted"
        );
    }
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&astersql_kv::Context::default(), store, true, |_, txn| {
                txn.Set(key.clone(), serde_json::to_vec(&old).unwrap())
            })
        })
        .unwrap();
    for field in ["rolinherit", "rolcreaterole", "rolconnlimit"] {
        denied(
            &query(
                &mut reader,
                &format!("SELECT {field} FROM pg_roles WHERE rolname='cat14_08_reader'"),
            ),
            b"0A000",
            b'I',
        );
        // Unknown attributes in filters/order must also refuse; LIMIT is not an escape.
        denied(
            &query(
                &mut reader,
                &format!(
                    "SELECT rolname FROM pg_roles WHERE rolname='cat14_08_reader' AND {field} IS NOT NULL"
                ),
            ),
            b"0A000",
            b'I',
        );
        denied(
            &query(
                &mut reader,
                &format!(
                    "SELECT rolname FROM pg_roles WHERE rolname='cat14_08_reader' ORDER BY {field} LIMIT 1"
                ),
            ),
            b"0A000",
            b'I',
        );
        assert_eq!(
            data(&ok(
                &mut reader,
                "SELECT oid,rolsuper FROM pg_roles WHERE rolname='cat14_08_reader'"
            )),
            vec![row(&[Some(&oid.to_string()), Some("f")])]
        );
    }
    domain
        .storage_handle()
        .with_storage(|store| {
            astersql_kv::RunInNewTxn(&astersql_kv::Context::default(), store, true, |_, txn| {
                txn.Set(key.clone(), saved.clone())
            })
        })
        .unwrap();
    assert_eq!(data(&ok(&mut reader, &sql)), vec![expected_text]);
    // Non-default policies have no supported declaration/executor yet. Reject
    // atomically rather than store a display-only value or change another role.
    for option in [
        "CREATEROLE",
        "NOINHERIT",
        "CONNECTION LIMIT 1",
        "VALID UNTIL '2030-01-01'",
        "PASSWORD 'cat14_08_rejected_secret'",
    ] {
        let failure = query(
            &mut admin,
            &format!("CREATE ROLE cat14_08_forbidden {option}"),
        );
        denied(&failure, b"0A000", b'I');
        assert!(
            !format!(
                "{:?}",
                failure
                    .iter()
                    .map(|m| String::from_utf8_lossy(&m.1))
                    .collect::<Vec<_>>()
            )
            .contains("cat14_08_rejected_secret")
        );
    }
    assert!(
        data(&ok(
            &mut reader,
            "SELECT rolname FROM pg_roles WHERE rolname='cat14_08_forbidden'"
        ))
        .is_empty()
    );
    // Secrets planted at the real native credential boundary never enter public or error output.
    let marker = "cat14_08_private_credential";
    native
        .execute(&format!(
            "UPDATE mysql.user SET authentication_string='{marker}' WHERE User='cat14_08_group'"
        ))
        .unwrap();
    assert_eq!(
        data(&ok(
            &mut reader,
            "SELECT rolpassword FROM pg_roles WHERE rolname='cat14_08_group'"
        )),
        vec![row(&[Some("********")])]
    );
    for sensitive in [
        "SELECT rolpassword FROM pg_authid",
        "SELECT passwd FROM pg_shadow",
    ] {
        let failure = query(&mut reader, sensitive);
        denied(&failure, b"42501", b'I');
        assert!(
            !failure
                .iter()
                .any(|m| m.1.windows(marker.len()).any(|w| w == marker.as_bytes()))
        );
        denied(&query(&mut admin, sensitive), b"0A000", b'I');
    }
    assert_eq!(
        query(&mut reader, "BEGIN").last(),
        Some(&(b'Z', vec![b'T']))
    );
    denied(
        &query(&mut reader, "SELECT passwd FROM pg_shadow"),
        b"42501",
        b'E',
    );
    assert_eq!(
        query(&mut reader, "ROLLBACK").last(),
        Some(&(b'Z', vec![b'I']))
    );
    assert_eq!(
        data(&ok(
            &mut reader,
            "SELECT passwd FROM pg_user WHERE usename='cat14_08_reader'"
        )),
        vec![row(&[Some("********")])]
    );
    assert_eq!(data(&ok(&mut reader, "SELECT 1")), vec![row(&[Some("1")])]);
    for logger in [
        astersql_util_logutil::log::background_logger(),
        astersql_util_logutil::log::general_logger(),
        astersql_util_logutil::log::slow_query_logger(),
    ] {
        assert!(
            !format!("{:?}", logger.entries()).contains(marker)
                && !format!("{:?}", logger.entries()).contains("cat14_08_rejected_secret"),
            "credential appeared in error logs"
        );
    }
    send(&mut reader, b'X', &[]);
    send(&mut other, b'X', &[]);
    send(&mut admin, b'X', &[]);
    println!(
        "cat14_08: 12 fields Q/text/binary, NULL, persisted defaults/reload, legacy per-field gates, native grant/revoke isolation, sensitive 42501/0A000, I/T/E recovery, secret-free logs, cleanup"
    );
}

#[test]
fn cat14_09() {
    let (domain, native) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    native.execute("CREATE DATABASE cat14_09_db").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let service = PgService::start(
        listener,
        Arc::new(ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::BootstrapRootAndNativeAccounts,
        )),
        Arc::new(CanonicalConnectionDomain::new(domain.clone())),
        false,
    )
    .unwrap();
    struct Cleanup<F: FnMut()>(F);
    impl<F: FnMut()> Drop for Cleanup<F> {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let _cleanup = Cleanup(|| {
        service.close();
        native
            .execute("DROP DATABASE IF EXISTS cat14_09_db")
            .unwrap();
        for name in ["cat14_09@reader'一", "cat14_09@group'二", "cat14_09_denied"] {
            native
                .execute(&format!(
                    "DROP USER IF EXISTS '{}'@'%'",
                    name.replace('\'', "''")
                ))
                .unwrap();
        }
        domain.close();
        assert!(TcpStream::connect(address).is_err());
    });
    let connect = |user: &str| {
        let mut s = TcpStream::connect(address).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let body = [
            196608u32.to_be_bytes().as_slice(),
            format!("user\0{user}\0database\0cat14_09_db\0\0").as_bytes(),
        ]
        .concat();
        s.write_all(&((body.len() + 4) as u32).to_be_bytes())
            .unwrap();
        s.write_all(&body).unwrap();
        let r = until_ready(&mut s);
        assert!(!r.iter().any(|m| m.0 == b'E'), "{r:?}");
        s
    };
    fn ok(s: &mut TcpStream, sql: &str) -> Vec<(u8, Vec<u8>)> {
        let r = query(s, sql);
        assert!(!r.iter().any(|m| m.0 == b'E'), "{sql}: {r:?}");
        r
    }
    fn identity(r: &[(u8, Vec<u8>)], login: &str, actor: &str, binary: bool, status: u8) {
        let t = &r.iter().find(|m| m.0 == b'T').expect("description").1;
        assert_eq!(
            columns(t),
            vec![
                ("session_user".into(), 19),
                ("current_user".into(), 19),
                ("current_role".into(), 19)
            ]
        );
        let mut offset = 2;
        for _ in 0..3 {
            offset += t[offset..].iter().position(|b| *b == 0).unwrap() + 1;
            assert_eq!(&t[offset..offset + 6], &[0; 6]);
            assert_eq!(
                i16::from_be_bytes(t[offset + 10..offset + 12].try_into().unwrap()),
                64
            );
            assert_eq!(
                i32::from_be_bytes(t[offset + 12..offset + 16].try_into().unwrap()),
                -1
            );
            assert_eq!(
                i16::from_be_bytes(t[offset + 16..offset + 18].try_into().unwrap()),
                i16::from(binary)
            );
            offset += 18;
        }
        assert_eq!(
            r.iter()
                .filter(|m| m.0 == b'D')
                .map(|m| m.1.clone())
                .collect::<Vec<_>>(),
            vec![row(&[Some(login), Some(actor), Some(actor)])]
        );
        assert_eq!(r.last(), Some(&(b'Z', vec![status])));
    }
    fn denied(s: &mut TcpStream, sql: &str, state: &str, status: u8) {
        let r = query(s, sql);
        assert_eq!(
            r.iter().map(|m| m.0).collect::<Vec<_>>(),
            b"EZ",
            "{sql}: {r:?}"
        );
        assert!(
            r[0].1
                .windows(7)
                .any(|w| w == format!("C{state}\0").as_bytes()),
            "{sql}: {r:?}"
        );
        assert_eq!(r.last(), Some(&(b'Z', vec![status])));
    }
    let login = "cat14_09@reader'一";
    let actor = "cat14_09@group'二";
    let mut admin = connect("root");
    ok(&mut admin, "CREATE ROLE \"cat14_09@reader'一\" LOGIN");
    ok(&mut admin, "CREATE ROLE \"cat14_09@group'二\" NOLOGIN");
    ok(&mut admin, "CREATE ROLE cat14_09_denied NOLOGIN");
    native
        .execute("GRANT 'cat14_09@group''二'@'%' TO 'cat14_09@reader''一'@'%'")
        .unwrap();
    ok(
        &mut admin,
        "CREATE SCHEMA cat14_09_private AUTHORIZATION \"cat14_09@group'二\"",
    );
    let mut a = connect(login);
    let mut b = connect(login);
    ok(&mut a, "SET search_path TO cat14_09_private");
    let schema_sql = "SELECT current_schema() FROM pg_namespace WHERE nspname='public'";
    assert_eq!(
        ok(&mut a, schema_sql)
            .iter()
            .find(|m| m.0 == b'D')
            .unwrap()
            .1,
        row(&[None])
    );
    let sql = "SELECT session_user,current_user,current_role";
    identity(&ok(&mut a, sql), login, login, false, b'I');
    identity(&ok(&mut b, sql), login, login, false, b'I');
    // Parse once, execute after SET/RESET: no identity is frozen into a statement.
    send(
        &mut a,
        b'P',
        &[b"identity\0".as_slice(), sql.as_bytes(), b"\0\0\0"].concat(),
    );
    send(&mut a, b'D', b"Sidentity\0");
    send(&mut a, b'S', b"");
    let r = until_ready(&mut a);
    assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"1tTZ", "{r:?}");
    for binary in [false, true] {
        if binary {
            send(
                &mut a,
                b'P',
                "\0SET ROLE \"cat14_09@group'二\"\0\0\0".as_bytes(),
            );
            send(&mut a, b'B', &[0; 8]);
            send(&mut a, b'E', &[0; 5]);
            send(&mut a, b'S', b"");
            let r = until_ready(&mut a);
            assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"12CZ", "{r:?}");
        } else {
            ok(&mut a, "SET ROLE \"cat14_09@group'二\"");
        }
        identity(&ok(&mut a, sql), login, actor, false, b'I');
        assert_eq!(
            ok(&mut a, schema_sql)
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[Some("cat14_09_private")])
        );
        identity(&ok(&mut b, sql), login, login, false, b'I');
        let bind = [
            b"\0identity\0".as_slice(),
            &0u16.to_be_bytes(),
            &0u16.to_be_bytes(),
            &1u16.to_be_bytes(),
            &i16::from(binary).to_be_bytes(),
        ]
        .concat();
        send(&mut a, b'B', &bind);
        send(&mut a, b'D', b"P\0");
        send(&mut a, b'E', b"\0\0\0\0\0");
        send(&mut a, b'S', b"");
        let r = until_ready(&mut a);
        assert_eq!(r.iter().map(|m| m.0).collect::<Vec<_>>(), b"2TDCZ", "{r:?}");
        identity(&r, login, actor, binary, b'I');
        denied(&mut a, "SET ROLE cat14_09_denied", "42501", b'I');
        denied(&mut a, "SET ROLE root", "42501", b'I');
        denied(&mut a, "SET ROLE cat14_09_missing", "22023", b'I');
        identity(&ok(&mut a, sql), login, actor, false, b'I');
        // The effective identity must drive actual authority, not only its label.
        denied(
            &mut a,
            "CREATE ROLE cat14_09_forbidden LOGIN",
            "42501",
            b'I',
        );
        assert_eq!(ok(&mut a, "BEGIN").last(), Some(&(b'Z', vec![b'T'])));
        denied(&mut a, "SET ROLE cat14_09_denied", "42501", b'E');
        denied(&mut a, sql, "25P02", b'E');
        ok(&mut a, "ROLLBACK");
        identity(&ok(&mut a, sql), login, actor, false, b'I');
        ok(&mut a, "RESET ROLE");
        assert_eq!(
            ok(&mut a, schema_sql)
                .iter()
                .find(|m| m.0 == b'D')
                .unwrap()
                .1,
            row(&[None])
        );
        identity(&ok(&mut a, sql), login, login, false, b'I');
        // The same prepared statement now observes the restored actor.
        send(&mut a, b'B', &bind);
        send(&mut a, b'D', b"P\0");
        send(&mut a, b'E', b"\0\0\0\0\0");
        send(&mut a, b'S', b"");
        identity(&until_ready(&mut a), login, login, binary, b'I');
    }
    native
        .execute("REVOKE 'cat14_09@group''二'@'%' FROM 'cat14_09@reader''一'@'%'")
        .unwrap();
    denied(&mut a, "SET ROLE \"cat14_09@group'二\"", "42501", b'I');
    identity(&ok(&mut a, sql), login, login, false, b'I');
    ok(&mut a, "SET ROLE NONE");
    ok(&mut a, "SET ROLE \"cat14_09@reader'一\"");
    identity(&ok(&mut a, sql), login, login, false, b'I');
    send(&mut a, b'C', b"Sidentity\0");
    send(&mut a, b'S', b"");
    assert_eq!(
        until_ready(&mut a).iter().map(|m| m.0).collect::<Vec<_>>(),
        b"3Z"
    );
    for s in [&mut a, &mut b, &mut admin] {
        send(s, b'X', b"");
    }
}
