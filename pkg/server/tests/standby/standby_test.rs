// Copyright 2026 AsterSQL.

// standby Server 生命周期单测。
//
// 覆盖待命场景下监听器初始化、关机标志（强制关闭 / 请求管理器释放）
// 以及关闭监听器后地址清空等行为。

/// Server 激活监听器后应能设置关机相关标志，并在 close_listeners 后清空地址。
#[test]
fn canonical_standby_server_activation_and_shutdown_flags_follow_lifecycle() {
    use astersql_server::server::{Server, ServerConfig, ServerDriver};
    use std::sync::Arc;

    /// 测试用最小 ServerDriver，仅提供驱动名。
    struct Driver;
    impl ServerDriver for Driver {
        fn name(&self) -> &str {
            "standby"
        }
    }

    let mut config = ServerConfig::default();
    config.host = "127.0.0.1".into();
    // port=0：由操作系统分配临时端口，避免测试端口冲突。
    config.port = 0;
    config.status.report_status = false;
    let server = Server::new(config, Arc::new(Driver)).unwrap();
    assert!(server.listener_addr().is_none());
    // 待命激活：显式初始化 TiDB 监听器。
    server.init_tidb_listener().unwrap();
    assert!(server.listener_addr().is_some());
    // 进入关机模式并设置强制关闭 / 请求管理器释放标志。
    server.enter_shutdown_mode();
    server.set_force_shutdown();
    server.set_need_request_manager_free();
    assert!(server.force_shutdown());
    assert!(server.need_request_manager_free());
    server.close_listeners();
    assert!(server.listener_addr().is_none());
}

/// Go TestStandby 的主场景：激活前等待，激活后启动 status 服务并挂载控制器路由。
#[test]
fn canonical_standby_activation_starts_status_server_and_controller_route() {
    use astersql_server::http_status::{Response, Router};
    use astersql_server::runtime::{
        BootstrapAuthMode, CanonicalConnectionDomain, CanonicalServerDomain, ConcreteSessionDriver,
    };
    use astersql_server::server::{Domain, Server, ServerConfig, ServerDriver, StatusConfig};
    use astersql_server::standby::{StandbyController, StandbyReadyServer, StandbyShutdownServer};
    use astersql_server_internal_testserverclient::TestServerClient;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    struct Driver;
    impl ServerDriver for Driver {
        fn name(&self) -> &str {
            "standby"
        }
    }

    struct TestDomain;
    impl Domain for TestDomain {
        fn server_id(&self) -> u64 {
            1
        }

        fn start_timestamp(&self) -> i64 {
            1
        }
    }

    struct Controller {
        waiting: AtomicBool,
        activated: AtomicBool,
        changed: Condvar,
        lock: Mutex<()>,
        prepare_calls: AtomicUsize,
        end_standby_calls: AtomicUsize,
        server_created_calls: AtomicUsize,
        server_shutdown_calls: AtomicUsize,
        connection_active_calls: AtomicUsize,
    }

    impl Controller {
        fn new() -> Self {
            Self {
                waiting: AtomicBool::new(false),
                activated: AtomicBool::new(false),
                changed: Condvar::new(),
                lock: Mutex::new(()),
                prepare_calls: AtomicUsize::new(0),
                end_standby_calls: AtomicUsize::new(0),
                server_created_calls: AtomicUsize::new(0),
                server_shutdown_calls: AtomicUsize::new(0),
                connection_active_calls: AtomicUsize::new(0),
            }
        }

        fn activate(&self) {
            self.activated.store(true, Ordering::Release);
            self.changed.notify_all();
        }

        fn wait_until_waiting(&self) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.waiting.load(Ordering::Acquire) {
                assert!(
                    Instant::now() < deadline,
                    "standby server did not wait for activation"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl StandbyController for Controller {
        fn wait_for_activate(&self) {
            self.waiting.store(true, Ordering::Release);
            let mut guard = self.lock.lock().expect("standby lock");
            while !self.activated.load(Ordering::Acquire) {
                guard = self.changed.wait(guard).expect("standby lock");
            }
        }

        fn end_standby(&self, result: Result<(), String>) {
            self.end_standby_calls.fetch_add(1, Ordering::AcqRel);
            assert!(result.is_ok(), "standby activation failed: {result:?}");
        }

        fn handler(&self, _server: Arc<dyn StandbyShutdownServer>) -> Option<(String, Router)> {
            let router = Router::default();
            router.add(
                "/status",
                Arc::new(|_| Response::text(200, "standby-active")),
            );
            Some(("/mock-standby/".into(), router))
        }

        fn on_connection_active(&self) {
            self.connection_active_calls.fetch_add(1, Ordering::AcqRel);
        }

        fn prepare_for_activation(&self, server: &dyn StandbyReadyServer) -> Result<(), String> {
            self.prepare_calls.fetch_add(1, Ordering::AcqRel);
            server.init_tidb_listener()
        }

        fn on_server_created(&self, _server: &dyn StandbyReadyServer) {
            self.server_created_calls.fetch_add(1, Ordering::AcqRel);
        }

        fn on_server_shutdown(&self, _server: &dyn StandbyShutdownServer) {
            self.server_shutdown_calls.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn read_packet(stream: &mut TcpStream) -> Vec<u8> {
        let mut header = [0_u8; 4];
        stream
            .read_exact(&mut header)
            .expect("read MySQL packet header");
        let length =
            usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
        let mut payload = vec![0_u8; length];
        stream
            .read_exact(&mut payload)
            .expect("read MySQL packet payload");
        payload
    }

    fn write_packet(stream: &mut TcpStream, sequence: u8, payload: &[u8]) {
        let length = payload.len();
        let header = [
            length as u8,
            (length >> 8) as u8,
            (length >> 16) as u8,
            sequence,
        ];
        stream
            .write_all(&header)
            .and_then(|_| stream.write_all(payload))
            .and_then(|_| stream.flush())
            .expect("write MySQL packet");
    }

    fn complete_root_handshake(stream: &mut TcpStream) {
        let initial = read_packet(stream);
        assert_eq!(initial.first(), Some(&10), "server must emit protocol v10");
        const CLIENT_PROTOCOL_41: u32 = 1 << 9;
        const CLIENT_SECURE_CONNECTION: u32 = 1 << 15;
        const CLIENT_PLUGIN_AUTH: u32 = 1 << 19;
        let capability = CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH;
        let mut response = Vec::new();
        response.extend_from_slice(&capability.to_le_bytes());
        response.extend_from_slice(&(64_u32 << 20).to_le_bytes());
        response.push(45);
        response.extend_from_slice(&[0; 23]);
        response.extend_from_slice(b"root\0");
        response.push(0);
        response.extend_from_slice(b"mysql_native_password\0");
        write_packet(stream, 1, &response);
        assert_eq!(
            read_packet(stream).first(),
            Some(&0),
            "root handshake must succeed"
        );
    }

    let controller = Arc::new(Controller::new());
    let server = Server::with_standby(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..StatusConfig::default()
            },
            ..ServerConfig::default()
        },
        Arc::new(Driver),
        Arc::clone(&controller) as Arc<dyn StandbyController>,
    )
    .expect("create standby server");
    assert_eq!(controller.server_created_calls.load(Ordering::Acquire), 1);
    let (store, domain) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    server
        .set_connection_runtime(
            Arc::new(ConcreteSessionDriver::from_initialized_domain(
                Arc::clone(&domain),
                BootstrapAuthMode::InsecureRootOnly,
            )),
            Arc::new(CanonicalConnectionDomain::new(Arc::clone(&domain))),
        )
        .expect("install canonical server connection runtime");
    let run_server = Arc::clone(&server);
    let run = thread::spawn(move || run_server.run(Arc::new(CanonicalServerDomain::new(domain))));

    controller.wait_until_waiting();
    assert!(
        server.status_listener_addr().is_none(),
        "status service must remain unavailable before standby activation"
    );
    controller.activate();
    run.join()
        .expect("standby server thread")
        .expect("run standby server");
    assert_eq!(controller.prepare_calls.load(Ordering::Acquire), 1);
    assert_eq!(controller.end_standby_calls.load(Ordering::Acquire), 1);

    let status_addr = server
        .status_listener_addr()
        .expect("status listener after activation");
    let mut client = TestServerClient::new();
    client.host = status_addr.ip().to_string();
    client.port = server
        .listener_addr()
        .expect("MySQL listener after activation")
        .port();
    client.status_port = status_addr.port();
    client
        .wait_until_server_online(Duration::from_secs(5))
        .expect("standby server becomes online");
    let response = client
        .fetch_status("/mock-standby/status")
        .expect("fetch standby status");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text().expect("UTF-8 status body"),
        "standby-active"
    );

    let mysql_addr = server.listener_addr().expect("MySQL listener stays online");
    let mut stream = TcpStream::connect(mysql_addr).expect("connect activated MySQL listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set MySQL client timeout");
    complete_root_handshake(&mut stream);
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(5);
    while controller.connection_active_calls.load(Ordering::Acquire) == 0 {
        assert!(
            Instant::now() < deadline,
            "standby controller did not observe active connection"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(controller.connection_active_calls.load(Ordering::Acquire) > 0);

    server.close();
    assert_eq!(controller.server_shutdown_calls.load(Ordering::Acquire), 1);
    assert!(server.listener_addr().is_none());
    assert!(server.status_listener_addr().is_none());
    drop(store);
}
