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
    let mut old = startup(addr, "root", 196608);
    let e = message(&mut old);
    assert_eq!(e.0, b'E');
    assert!(e.1.windows(5).any(|w| w == b"0A000"));
    send(&mut s, b'X', b"");
    let mut eof = [0];
    assert_eq!(s.read(&mut eof).unwrap(), 0);
    service.close();
}

fn authenticated(addr: std::net::SocketAddr) -> (TcpStream, Vec<u8>) {
    let mut socket = startup(addr, "root", 196610);
    assert_eq!(message(&mut socket).0, b'R');
    let key = loop {
        let (tag, body) = message(&mut socket);
        if tag == b'K' {
            break body;
        }
    };
    assert_eq!(message(&mut socket).0, b'Z');
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
    let mut eof = [0];
    assert_eq!(socket.read(&mut eof).unwrap(), 0);
}
#[test]
fn startup_cancel_is_scoped_and_consumed() {
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
    let (mut a, key) = authenticated(addr);
    let (mut b, other) = authenticated(addr);
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
