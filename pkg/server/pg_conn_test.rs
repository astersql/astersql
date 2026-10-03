// Copyright 2026 AsterSQL.
use crate::pg_conn::PgService;
use crate::runtime::{BootstrapAuthMode, CanonicalConnectionDomain, ConcreteSessionDriver};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

fn message(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut h = [0; 5];
    s.read_exact(&mut h).unwrap();
    let len = u32::from_be_bytes(h[1..].try_into().unwrap()) as usize;
    assert!((4..10000).contains(&len));
    let mut body = vec![0; len - 4];
    s.read_exact(&mut body).unwrap();
    (h[0], body)
}
fn send(s: &mut TcpStream, tag: u8, body: &[u8]) {
    s.write_all(&[tag]).unwrap();
    s.write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    s.write_all(body).unwrap();
}
fn startup(addr: std::net::SocketAddr, user: &str, version: u32) -> TcpStream {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut body = version.to_be_bytes().to_vec();
    body.extend_from_slice(format!("user\0{user}\0client_encoding\0UTF8\0\0").as_bytes());
    s.write_all(&((body.len() + 4) as u32).to_be_bytes())
        .unwrap();
    s.write_all(&body).unwrap();
    s
}
#[test]
fn startup_auth_roundtrip() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    let mut s = startup(addr, "root", 196610);
    assert_eq!(message(&mut s), (b'R', 0u32.to_be_bytes().to_vec()));
    let key = loop {
        let (tag, body) = message(&mut s);
        if tag == b'K' {
            break body;
        }
        assert_eq!(tag, b'S');
    };
    assert_eq!(key.len(), 36);
    assert_eq!(message(&mut s), (b'Z', b"I".to_vec()));
    let mut bad = startup(addr, "intruder", 196610);
    let e = message(&mut bad);
    assert_eq!(e.0, b'E');
    assert!(e.1.windows(5).any(|w| w == b"28P01"));
    let mut old = startup(addr, "root", 196609);
    let e = message(&mut old);
    assert_eq!(e.0, b'E');
    assert!(e.1.windows(5).any(|w| w == b"0A000"));
    send(&mut s, b'X', b"");
    let mut eof = [0];
    assert_eq!(s.read(&mut eof).unwrap(), 0);
    service.close();
}

#[test]
fn startup_protocol_30_roundtrip() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    let mut s = startup(addr, "root", 196608);
    assert_eq!(message(&mut s), (b'R', 0u32.to_be_bytes().to_vec()));
    let key = loop {
        let (tag, body) = message(&mut s);
        if tag == b'K' {
            break body;
        }
        assert_eq!(tag, b'S');
    };
    assert_eq!(key.len(), 8);
    assert_eq!(message(&mut s), (b'Z', b"I".to_vec()));
    let mut bad = startup(addr, "intruder", 196608);
    let e = message(&mut bad);
    assert_eq!(e.0, b'E');
    assert!(e.1.windows(5).any(|w| w == b"28P01"));
    send(&mut s, b'X', b"");
    let mut eof = [0];
    assert_eq!(s.read(&mut eof).unwrap(), 0);
    service.close();
}

#[test]
fn startup_server_version_parameter() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    for version in [196608, 196610] {
        let mut socket = startup(addr, "root", version);
        assert_eq!(message(&mut socket), (b'R', 0u32.to_be_bytes().to_vec()));
        for body in [
            b"client_encoding\0UTF8\0".as_slice(),
            b"server_encoding\0UTF8\0",
            b"DateStyle\0ISO, MDY\0",
            b"TimeZone\0UTC\0",
            b"server_version\018.0 (AsterSQL)\0",
        ] {
            assert_eq!(message(&mut socket), (b'S', body.to_vec()));
        }
        let (tag, key) = message(&mut socket);
        assert_eq!(tag, b'K');
        assert_eq!(key.len(), if version == 196608 { 8 } else { 36 });
        assert_eq!(message(&mut socket), (b'Z', b"I".to_vec()));
        send(&mut socket, b'X', b"");
        let mut bad = startup(addr, "intruder", version);
        let error = message(&mut bad);
        assert_eq!(error.0, b'E');
        assert!(error.1.windows(5).any(|w| w == b"28P01"));
    }
    service.close();
}

#[test]
fn startup_datagrip_date_style() {
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
    for version in [196608u32, 196610] {
        for (style, zone, digits, error_code) in [
            ("ISO", "Asia/Shanghai", "3", None),
            ("iso", "Asia/Shanghai", "2", None),
            ("ISO, MDY", "Asia/Shanghai", "1", None),
            ("SQL, DMY", "Asia/Shanghai", "3", Some("0A000")),
            ("ISO", "NoSuch/Zone", "3", Some("22023")),
            ("ISO", "UTC'; SET sql_mode='", "3", Some("0A000")),
            ("ISO", "Asia/Shanghai", "-1", Some("0A000")),
        ] {
            let mut socket = TcpStream::connect(addr).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut body = version.to_be_bytes().to_vec();
            body.extend_from_slice(
                format!("user\0root\0client_encoding\0UTF8\0DateStyle\0{style}\0TimeZone\0{zone}\0extra_float_digits\0{digits}\0\0").as_bytes(),
            );
            socket
                .write_all(&((body.len() + 4) as u32).to_be_bytes())
                .unwrap();
            socket.write_all(&body).unwrap();
            if let Some(code) = error_code {
                let (tag, error) = message(&mut socket);
                assert_eq!(tag, b'E');
                assert!(error.windows(5).any(|w| w == code.as_bytes()));
                continue;
            }
            assert_eq!(message(&mut socket), (b'R', 0u32.to_be_bytes().to_vec()));
            let mut date_style = None;
            let mut time_zone = None;
            loop {
                let (tag, body) = message(&mut socket);
                if tag == b'S' && body.starts_with(b"DateStyle\0") {
                    date_style = Some(body.clone());
                }
                if tag == b'S' && body.starts_with(b"TimeZone\0") {
                    time_zone = Some(body.clone());
                }
                if tag == b'Z' {
                    break;
                }
                assert!(matches!(tag, b'S' | b'K'));
            }
            assert_eq!(date_style, Some(b"DateStyle\0ISO, MDY\0".to_vec()));
            assert_eq!(time_zone, Some(b"TimeZone\0Asia/Shanghai\0".to_vec()));
            send(&mut socket, b'Q', b"SELECT 1\0");
            assert_eq!(message(&mut socket).0, b'T');
            assert_eq!(message(&mut socket).0, b'D');
            assert_eq!(message(&mut socket).0, b'C');
            assert_eq!(message(&mut socket), (b'Z', b"I".to_vec()));
            send(&mut socket, b'X', b"");
        }
    }
    service.close();
}

fn authenticated(addr: std::net::SocketAddr) -> (TcpStream, Vec<u8>) {
    authenticated_version(addr, 196610)
}
fn authenticated_version(addr: std::net::SocketAddr, version: u32) -> (TcpStream, Vec<u8>) {
    let mut socket = startup(addr, "root", version);
    assert_eq!(message(&mut socket).0, b'R');
    let key = loop {
        let (tag, body) = message(&mut socket);
        if tag == b'K' {
            break body;
        }
    };
    assert_eq!(key.len(), if version == 196608 { 8 } else { 36 });
    assert_eq!(message(&mut socket), (b'Z', b"I".to_vec()));
    (socket, key)
}
fn cancel_packet(addr: std::net::SocketAddr, key: &[u8]) {
    let mut socket = TcpStream::connect(addr).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    socket
        .write_all(&((key.len() + 8) as u32).to_be_bytes())
        .unwrap();
    socket.write_all(&80877102u32.to_be_bytes()).unwrap();
    socket.write_all(key).unwrap();
    if key.len() < 8 || key.len() > 260 {
        let error = message(&mut socket);
        assert_eq!(error.0, b'E');
        assert!(error.1.windows(5).any(|w| w == b"08P01"));
    }
    let mut eof = [0];
    assert_eq!(socket.read(&mut eof).unwrap(), 0);
}
#[test]
fn startup_cancel_is_scoped_and_consumed() {
    for version in [196608, 196610] {
        cancel_is_scoped_and_consumed(version);
    }
}
fn cancel_is_scoped_and_consumed(version: u32) {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let service = PgService::start(
        listener,
        driver,
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    let (mut a, key) = authenticated_version(addr, version);
    let (mut b, other) = authenticated_version(addr, version);
    let (mut opposite, opposite_key) =
        authenticated_version(addr, if version == 196608 { 196610 } else { 196608 });
    let pid = u32::from_be_bytes(key[..4].try_into().unwrap());
    let other_pid = u32::from_be_bytes(other[..4].try_into().unwrap());
    for should_cancel in [false, true] {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let owner = service.clone();
        let worker = std::thread::spawn(move || {
            owner
                .with_query(pid, |context| {
                    entered_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                    context.execute_query("select 1", false, &crate::conn::CancellationToken::new())
                })
                .unwrap()
        });
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        if should_cancel {
            cancel_packet(addr, &key);
        } else {
            let mut wrong = key.clone();
            wrong[4] ^= 1;
            cancel_packet(addr, &wrong);
            let mut cross = key[..4].to_vec();
            cross.extend_from_slice(&other[4..]);
            cancel_packet(addr, &cross);
            // Keep malformed lengths inside the initial frame's bounds so the
            // registry, rather than transport framing, rejects the secret.
            let mut oversized = key.clone();
            oversized.push(0);
            cancel_packet(addr, &oversized);
            cancel_packet(addr, &key[..key.len() - 1]);
            let mut different_version = key[..4].to_vec();
            different_version.extend_from_slice(&opposite_key[4..]);
            cancel_packet(addr, &different_version);
            let mut beyond_protocol_maximum = key[..4].to_vec();
            beyond_protocol_maximum.resize(261, 0);
            cancel_packet(addr, &beyond_protocol_maximum);
        }
        assert!(
            service
                .with_query(other_pid, |c| c.execute_query(
                    "select 2",
                    false,
                    &crate::conn::CancellationToken::new()
                ))
                .unwrap()
                .is_ok()
        );
        resume_tx.send(()).unwrap();
        let result = worker.join().unwrap();
        if should_cancel {
            assert!(result.unwrap_err().to_string().contains("interrupted"));
        } else {
            assert!(result.is_ok(), "incorrect key must not cancel target");
        }
    }
    assert!(
        service
            .with_query(pid, |c| c.execute_query(
                "select 3",
                false,
                &crate::conn::CancellationToken::new()
            ))
            .unwrap()
            .is_ok()
    );
    cancel_packet(addr, &key); // idle cancellation must not poison next query
    assert!(
        service
            .with_query(pid, |c| c.execute_query(
                "select 4",
                false,
                &crate::conn::CancellationToken::new()
            ))
            .unwrap()
            .is_ok()
    );
    send(&mut a, b'X', b"");
    let mut eof = [0];
    assert_eq!(a.read(&mut eof).unwrap(), 0);
    cancel_packet(addr, &key); // stale key is harmless
    assert!(
        service
            .with_query(other_pid, |c| c.execute_query(
                "select 5",
                false,
                &crate::conn::CancellationToken::new()
            ))
            .unwrap()
            .is_ok()
    );
    send(&mut b, b'X', b"");
    send(&mut opposite, b'X', b"");
    service.close();
}

#[test]
fn startup_negotiation_and_secure_mode_reject() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    for secure in [false, true] {
        let driver = Arc::new(ConcreteSessionDriver::new_for_test(
            domain.clone(),
            BootstrapAuthMode::SecureUnsupported,
        ));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let service = PgService::start(
            listener,
            driver,
            Arc::new(CanonicalConnectionDomain::new(domain.clone())),
            secure,
        )
        .unwrap();
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        for code in [80877103u32, 80877104] {
            s.write_all(&8u32.to_be_bytes()).unwrap();
            s.write_all(&code.to_be_bytes()).unwrap();
            let mut byte = [0];
            s.read_exact(&mut byte).unwrap();
            assert_eq!(&byte, b"N");
        }
        let body = b"user\0root\0\0";
        s.write_all(&((body.len() + 8) as u32).to_be_bytes())
            .unwrap();
        s.write_all(&196610u32.to_be_bytes()).unwrap();
        s.write_all(body).unwrap();
        assert_eq!(message(&mut s).0, b'E');
        service.close();
    }
}

#[test]
fn startup_real_dual_listener_shutdown_closes_pending_and_authenticated_clients() {
    struct Driver;
    impl crate::server::ServerDriver for Driver {
        fn name(&self) -> &str {
            "tidb"
        }
    }
    struct Domain;
    impl crate::server::Domain for Domain {
        fn server_id(&self) -> u64 {
            1
        }
        fn start_timestamp(&self) -> i64 {
            1
        }
    }
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let driver = Arc::new(ConcreteSessionDriver::new_for_test(
        domain.clone(),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let server = crate::server::Server::new_test(
        crate::server::ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            postgres_port: Some(0),
            status: crate::server::StatusConfig {
                report_status: false,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(Driver),
    );
    server
        .set_connection_runtime(driver, Arc::new(CanonicalConnectionDomain::new(domain)))
        .unwrap();
    server.run(Arc::new(Domain)).unwrap();
    let addr = server.postgres_listener_addr().unwrap();
    let (mut authenticated, _) = authenticated(addr);
    let mut pending = TcpStream::connect(addr).unwrap();
    pending
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    // TCP connect can complete while the socket is still in the listener's
    // backlog. Confirm acceptance before asserting graceful worker shutdown;
    // closing an unaccepted backlog socket may legitimately send a TCP reset.
    pending.write_all(&8u32.to_be_bytes()).unwrap();
    pending.write_all(&80877103u32.to_be_bytes()).unwrap();
    let mut ssl_response = [0];
    pending.read_exact(&mut ssl_response).unwrap();
    assert_eq!(ssl_response, [b'N']); // Still waiting for the actual startup packet.
    let mut mysql = TcpStream::connect(server.listener_addr().unwrap()).unwrap();
    mysql
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut mysql_header = [0; 4];
    mysql.read_exact(&mut mysql_header).unwrap();
    assert_eq!(mysql_header[3], 0); // MySQL listener still emits its normal initial handshake.
    server.close();
    let mut eof = [0];
    assert_eq!(authenticated.read(&mut eof).unwrap(), 0);
    assert_eq!(pending.read(&mut eof).unwrap(), 0);
    assert!(server.postgres_listener_addr().is_none());
    assert!(TcpStream::connect(addr).is_err());
}

/// Exercise recovery on the wire and observe ownership after disconnect/shutdown.
#[test]
fn catalog_error_recovery_and_cleanup() {
    fn select_one(socket: &mut TcpStream) {
        send(socket, b'Q', b"SELECT 1\0");
        assert_eq!(message(socket).0, b'T');
        assert_eq!(message(socket), (b'D', vec![0, 1, 0, 0, 0, 1, b'1']));
        assert_eq!(message(socket), (b'C', b"SELECT 1\0".to_vec()));
        assert_eq!(message(socket), (b'Z', b"I".to_vec()));
    }
    fn assert_error(socket: &mut TcpStream, state: &str) {
        let (tag, body) = message(socket);
        assert_eq!(tag, b'E');
        let field = [b"C", state.as_bytes(), b"\0"].concat();
        assert!(
            body.windows(field.len()).any(|part| part == field),
            "{body:?}"
        );
    }
    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(
                std::time::Instant::now() < deadline,
                "resources were not released"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
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
        Arc::new(CanonicalConnectionDomain::new(domain)),
        false,
    )
    .unwrap();
    let (mut socket, key) = authenticated(addr);
    // The canonical malformed-input syntax gate can take over seven seconds
    // on a cold run. Keep a bounded read while allowing its 42601 response;
    // resource cleanup deadlines and all protocol assertions stay unchanged.
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();

    let pid = u32::from_be_bytes(key[..4].try_into().unwrap());
    let context = service.with_query(pid, Arc::downgrade).unwrap();
    let (mut unaffected, _) = authenticated(addr);
    for (sql, state) in [
        ("SELECT oid FROM pg_catalog.pg_missing", "42P01"),
        (
            "SELECT oid FROM pg_catalog.pg_namespace GROUP BY oid",
            "0A000",
        ),
        ("SELECT (", "42601"),
    ] {
        send(&mut socket, b'Q', &[sql.as_bytes(), b"\0"].concat());
        assert_error(&mut socket, state);
        assert_eq!(message(&mut socket), (b'Z', b"I".to_vec()));
        select_one(&mut socket);
        select_one(&mut unaffected);
    }
    // Parse fails; queued Query/Execute are ignored until Sync, with no output.
    send(
        &mut socket,
        b'P',
        b"bad\0SELECT oid FROM pg_catalog.pg_missing\0\0\0",
    );
    assert_error(&mut socket, "42P01");
    send(&mut socket, b'Q', b"SELECT 99\0");
    send(&mut socket, b'E', b"missing\0\0\0\0\0");
    send(&mut socket, b'S', b"");
    assert_eq!(message(&mut socket), (b'Z', b"I".to_vec()));
    select_one(&mut socket);
    // A catalog portal is valid at Parse/Bind; Execute errors on a negative limit.
    send(
        &mut socket,
        b'P',
        b"catalog\0SELECT oid FROM pg_catalog.pg_namespace\0\0\0",
    );
    assert_eq!(message(&mut socket), (b'1', vec![]));
    send(&mut socket, b'B', b"portal\0catalog\0\0\0\0\0\0\0");
    assert_eq!(message(&mut socket), (b'2', vec![]));
    send(&mut socket, b'E', b"portal\0\xff\xff\xff\xff");
    assert_error(&mut socket, "08P01");
    send(&mut socket, b'Q', b"SELECT 99\0");
    send(&mut socket, b'S', b"");
    assert_eq!(message(&mut socket), (b'Z', b"I".to_vec()));
    select_one(&mut socket);
    drop(socket);
    wait_until(|| {
        context.upgrade().is_none()
            && service.resource_counts().0 == 1
            && service.resource_counts().1 == 1
    });
    // Partial startup and authenticated frames terminate only their own worker.
    let mut partial = TcpStream::connect(addr).unwrap();
    partial.write_all(&[0, 0]).unwrap();
    drop(partial);
    for _ in 0..12 {
        let (mut client, key) = authenticated(addr);
        let pid = u32::from_be_bytes(key[..4].try_into().unwrap());
        let context = service.with_query(pid, Arc::downgrade).unwrap();
        client.write_all(&[b'Q', 0, 0, 0, 20, b'S']).unwrap();
        drop(client);
        wait_until(|| {
            context.upgrade().is_none()
                && service.resource_counts().0 == 1
                && service.resource_counts().1 == 1
        });
        select_one(&mut unaffected);
    }
    // Close interrupts an accepted client still waiting for a startup body.
    let mut pending = TcpStream::connect(addr).unwrap();
    pending
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    pending.write_all(&8u32.to_be_bytes()).unwrap();
    pending.write_all(&80877103u32.to_be_bytes()).unwrap();
    let mut byte = [0];
    pending.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [b'N']);
    service.close();
    service.close(); // Idempotent shutdown.
    assert_eq!(service.resource_counts(), (0, 0, 0));
    assert_eq!(unaffected.read(&mut byte).unwrap(), 0);
    assert_eq!(pending.read(&mut byte).unwrap(), 0);
    assert!(TcpStream::connect(addr).is_err());
}
