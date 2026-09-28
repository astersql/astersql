// Copyright 2026 AsterSQL.

/// Repeatedly bootstrapping and closing the canonical Domain must not retain an
/// unbounded amount of allocator-owned heap. This is the Rust counterpart of
/// Go's `TestMemoryLeak`: `CreateAnalyzeSession` creates the real in-memory
/// mock KV store and bootstraps the production session/Domain stack.
#[test]
fn test_memory_leak() {
    const MIB: u64 = 1024 * 1024;

    let old_heap_in_use = astersql_util_memory::memstats::ForceReadMemStats().heap_inuse;

    for _ in 0..20 {
        let (domain, session) = astersql_session::runtime::CreateAnalyzeSession()
            .expect("create mock store and bootstrap session");
        drop(session);
        domain.close();
        drop(domain);
    }

    let heap_in_use = astersql_util_memory::memstats::ForceReadMemStats().heap_inuse;
    let growth = heap_in_use.saturating_sub(old_heap_in_use);
    let limit = if astersql_util_syncutil::EnableDeadlock {
        5_400 * MIB
    } else {
        900 * MIB
    };
    assert!(
        growth < limit,
        "20 bootstrap/close cycles retained {growth} bytes of heap (limit {limit})"
    );
}

// server 库级 canonical 单测：重复创建/关闭监听器。
//
// 验证多次 `new` → `init_tidb_listener` → `close` 循环不会泄漏监听地址，
// 每次 close 后 `listener_addr` 应为空。

/// 连续四次创建并关闭 Server，监听器地址应在 close 后清空。
#[test]
fn canonical_server_library_repeated_create_close_releases_listeners() {
    use crate::server::{Server, ServerConfig, ServerDriver};
    use std::sync::Arc;

    /// 测试用最小 ServerDriver。
    struct Driver;
    impl ServerDriver for Driver {
        fn name(&self) -> &str {
            "canonical"
        }
    }

    for _ in 0..4 {
        let mut config = ServerConfig::default();
        config.host = "127.0.0.1".into();
        // 临时端口，避免与并行测试抢占固定端口。
        config.port = 0;
        config.status.report_status = false;
        let server = Server::new(config, Arc::new(Driver)).unwrap();
        server.init_tidb_listener().unwrap();
        assert!(server.listener_addr().is_some());
        server.close();
        assert!(server.listener_addr().is_none());
    }
}

#[cfg(unix)]
#[test]
fn canonical_server_binds_and_removes_configured_unix_socket() {
    use crate::server::{Server, ServerConfig, ServerDriver};
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    struct Driver;
    impl ServerDriver for Driver {
        fn name(&self) -> &str {
            "canonical"
        }
    }

    let socket = std::path::PathBuf::from(format!(
        "/tmp/astersql-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut config = ServerConfig::default();
    config.host = "127.0.0.1".into();
    config.port = 0;
    config.socket = Some(socket.to_string_lossy().into_owned());
    config.status.report_status = false;

    let server = Server::new(config, Arc::new(Driver)).unwrap();
    server.init_tidb_listener().unwrap();
    assert!(std::fs::metadata(&socket).unwrap().file_type().is_socket());
    let client = UnixStream::connect(&socket).unwrap();
    drop(client);

    server.close();
    assert!(!socket.exists());
}

#[test]
fn proxy_protocol_v1_v2_and_fallback_preserve_source_address() {
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    fn exchange(payload: Vec<u8>, fallbackable: bool) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(&payload).unwrap();
        });
        let (mut stream, peer) = listener.accept().unwrap();
        let source = crate::server::proxy_source_addr(
            &mut stream,
            peer,
            "127.0.0.0/8",
            fallbackable,
            Duration::from_secs(1),
        )
        .unwrap()
        .unwrap();
        client.join().unwrap();
        source
    }

    assert_eq!(
        exchange(
            b"PROXY TCP4 192.0.2.10 127.0.0.1 4567 4000\r\n".to_vec(),
            false,
        ),
        "192.0.2.10:4567".parse().unwrap()
    );
    let mut v2 = b"\r\n\r\n\0\r\nQUIT\n\x21\x11\0\x0c".to_vec();
    v2.extend_from_slice(&[198, 51, 100, 8, 127, 0, 0, 1, 0x17, 0x70, 0x0f, 0xa0]);
    assert_eq!(exchange(v2, false), "198.51.100.8:6000".parse().unwrap());
    assert!(exchange(b"mysql".to_vec(), true).ip().is_loopback());
}

#[test]
fn invalid_proxy_network_is_rejected_before_listening() {
    use crate::server::{Server, ServerConfig, ServerDriver};
    use std::sync::Arc;

    struct Driver;
    impl ServerDriver for Driver {
        fn name(&self) -> &str {
            "canonical"
        }
    }
    let mut config = ServerConfig::default();
    config.proxy_protocol_networks = "127.0.0.1/99".into();
    let error = match Server::new(config, Arc::new(Driver)) {
        Ok(_) => panic!("invalid PROXY network must be rejected"),
        Err(error) => error,
    };
    assert!(error.contains("invalid PROXY"));
}

#[test]
fn tcp_packet_io_performs_real_rustls_handshake() {
    use crate::conn::PacketIo;
    use crate::runtime::TcpPacketIo;
    use rustls::pki_types::ServerName;
    use rustls::{ClientConnection, StreamOwned};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::Arc;
    use std::thread;

    let fixtures = std::env::temp_dir().join(format!(
        "astersql-server-runtime-tls-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&fixtures).unwrap();
    let certificate = fixtures.join("server-cert.pem");
    let key = fixtures.join("server-key.pem");
    astersql_util::misc::CreateCertificates(
        &certificate,
        &key,
        2048,
        astersql_util::misc::PublicKeyAlgorithm::Rsa,
        astersql_util::misc::SignatureAlgorithm::Unspecified,
    )
    .unwrap();
    let path = |path: &Path| path.to_str().unwrap().to_owned();
    let server_config =
        astersql_util::security::NewTLSConfig(vec![astersql_util::security::WithCertAndKeyPath(
            path(&certificate),
            path(&key),
        )])
        .unwrap()
        .unwrap()
        .server_config()
        .unwrap();
    let client_config =
        astersql_util::security::NewTLSConfig(vec![astersql_util::security::WithCAPath(path(
            &certificate,
        ))])
        .unwrap()
        .unwrap()
        .client_config()
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut packet =
            TcpPacketIo::new_with_options(stream, 64 << 20, Some(server_config), None, None)
                .unwrap();
        packet.upgrade_to_tls().unwrap()
    });
    let stream = TcpStream::connect(address).unwrap();
    let connection = ClientConnection::new(
        Arc::clone(&client_config),
        ServerName::try_from("localhost").unwrap().to_owned(),
    )
    .unwrap();
    let mut stream = StreamOwned::new(connection, stream);
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock).unwrap();
    }
    let state = server.join().unwrap();
    assert!(matches!(state.version, 0x0303 | 0x0304));
    assert_ne!(state.cipher_suite, 0);
    std::fs::remove_dir_all(fixtures).unwrap();
}
