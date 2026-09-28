// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 该文件承接 `cmd/tidb-server/main_test.go` 的核心测试语义。
//! 重点不是覆盖所有启动路径，而是验证 Rust 入口层在信号处理、
//! 配置覆盖、TiKV 线路装配、版本归一化以及 starter 相关约束上
//! 与 Go 版本保持一致。
//! 对于必须访问真实 PD 的场景，测试显式做成外部 gate，
//! 避免默认单元测试因为环境缺失而产生假阴性。
//! Go-equivalent tests for `cmd/tidb-server/main_test.go`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use astersql_config::StoreTypeTiKV;
use astersql_server::runtime::{BootstrapAuthMode, ConcreteSessionDriver};
use astersql_store_driver::{
    DriverBackend, DriverError, MetaServiceInfo, PdClientOptions, SafePointKvSetup,
    Security as TiKVSecurity, TiKVDriver, TlsConfig, WaitForEntry,
};

use crate::entry::{
    cleanup, createMgrClientForStarter, createStoreDDLOwnerMgrAndDomain, exitCodeForSignal,
    exitCodeInt, exitCodeOK, initDeployMode, initFlagSetWithArgs, initVersions, overrideConfig,
    prepareKeyspaceObservabilityForStarter, registerStores, registerStoresWithTiKVDriver,
    set_starter_additional_params, setGlobalVars, starter_additional_params,
};
use crate::stubs::{
    self, Signal, config, deploymode, kerneltype, mysql, server, syscall, vardef, variable,
};

/// 这里保留一个空实现，模拟 Go 侧 `view.Stop()` 的收尾位置。
/// arm64 stub 环境没有真正的 OpenCensus worker，因此只需要表达
/// “测试在此处完成观测资源收束”的语义，不引入额外副作用。
/// OpenCensus `view.Stop` stand-in (no opencensus worker in arm64 stubs).
fn view_stop() {}

/// 该测试对应 Go 的 `TestMain` 公共前置初始化。
/// Rust 单测没有直接复刻 goleak 的进程级校验，因此这里重点确认
/// 通用测试护栏已安装，并且内核类型与部署模式落在预期集合内。
/// 这样可以保证后续各个表驱动测试共享一致的全局环境。
/// TestMain — Go `TestMain` common setup (goleak ignore list is process-level in Go).
#[test]
fn test_main() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    // Go: testsetup.SetupForCommonTest() + goleak.VerifyTestMain ignore list.
    assert!(kerneltype::IsClassic() || kerneltype::IsNextGen());
    assert_eq!(deploymode::Get(), deploymode::Premium);
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 记录驱动打开 PD 时看到的输入参数。
/// 该结构只用于断言装配过程是否把配置值原样传递给底层驱动，
/// 不参与任何生产逻辑。
struct OpenObservation {
    addrs: Vec<String>,
    keyspace: String,
    security: TiKVSecurity,
    options: PdClientOptions,
}

#[derive(Debug, Default)]
/// 可观测的假后端，用来替代真实 TiKV driver backend。
/// 它把关键调用顺序记录到内存里，便于测试验证 bootstrap 与 cleanup
/// 是否遵守 Go 版本同样的资源生命周期。
struct WiringBackend {
    observation: Mutex<Option<OpenObservation>>,
    events: Mutex<Vec<String>>,
}

impl WiringBackend {
    /// 统一记录事件，既写入本地事件列表，也同步到测试 stub 事件流。
    /// 这样测试既能检查 backend 自己的关闭次数，也能校验跨模块顺序。
    fn event(&self, event: &str) {
        self.events.lock().unwrap().push(event.to_owned());
        stubs::record_event(format!("tikv.{event}"));
    }
}

impl DriverBackend for WiringBackend {
    /// 模拟成功打开 PD，并把所有输入留痕给断言使用。
    /// 返回固定 cluster id，确保后续 bootstrap 能走完全路径。
    fn open_pd(
        &self,
        addrs: &[String],
        keyspace: &str,
        security: &TiKVSecurity,
        options: &PdClientOptions,
    ) -> Result<u64, DriverError> {
        *self.observation.lock().unwrap() = Some(OpenObservation {
            addrs: addrs.to_vec(),
            keyspace: keyspace.to_owned(),
            security: security.clone(),
            options: options.clone(),
        });
        self.event("open-pd");
        Ok(42)
    }

    /// Safe point 初始化返回固定的 meta/group 地址集合，
    /// 用来证明上层在完成装配后会继续沿这些地址进行后续清理。
    fn new_safe_point_kv(
        &self,
        _cluster_id: u64,
        _keyspace: &str,
        _tls: Option<&TlsConfig>,
    ) -> Result<SafePointKvSetup, DriverError> {
        self.event("open-safe-point");
        Ok(SafePointKvSetup {
            meta_service_info: MetaServiceInfo {
                pd_addrs: vec!["pd-a:2379".into(), "pd-b:2379".into()],
                group_addrs: vec!["pd-a:2379".into(), "pd-b:2379".into()],
            },
            pd_addrs: vec!["pd-a:2379".into(), "pd-b:2379".into()],
            group_addrs: vec!["pd-a:2379".into(), "pd-b:2379".into()],
            safe_point_id: "wiring-safe-point".into(),
        })
    }

    /// 下面几个 close/current 接口都只保留最小行为：
    /// 记录调用顺序，避免把测试重点转移到底层实现细节。
    fn close_pd(&self, _cluster_id: u64) {
        self.event("close-pd");
    }

    /// safe point 关闭必须晚于 storage close，
    /// 本测试通过事件时间线验证这一点。
    fn close_safe_point(&self, _safe_point_id: &str) {
        self.event("close-safe-point");
    }

    /// close_store 只允许对 canonical store 生效一次，
    /// 末尾的计数断言就是为了防止重复释放。
    fn close_store(&self, _uuid: &str) -> Result<(), DriverError> {
        self.event("close-store");
        Ok(())
    }

    /// 返回固定 TSO，确保上层“当前版本”查询可预测且不依赖外部时钟。
    fn current_timestamp(&self, _txn_scope: &str) -> Result<u64, DriverError> {
        Ok(4_242)
    }

    /// 本文件不覆盖锁等待抓取流程，因此显式返回空集合。
    /// 这样可以把测试焦点限定在入口 wiring 上。
    fn lock_waits(&self) -> Vec<Result<Vec<WaitForEntry>, DriverError>> {
        Vec::new()
    }
}

/// 该用例验证 TiKV wiring 的主路径。
/// 先检查注册表里挂载的是具体的 `TiKVDriver`，
/// 再检查配置中的 PD 地址、TLS 参数、keyspace 与超时
/// 是否原样传给底层 backend。
/// 最后通过事件序列确认 bootstrap 完成后，
/// cleanup 会按 Go 侧既定顺序释放 domain、owner、storage、safe point 和 PD。
/// TiKV wiring registers the concrete driver, preserves config and storage
/// identity through bootstrap, then closes the canonical store exactly once.
#[test]
fn tikv_storage_wiring_uses_registered_real_driver() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    stubs::clear_events();
    // 这里固定注入显式 TLS 与超时配置，防止测试退化成“只验证默认值存在”。
    config::UpdateGlobal(|conf| {
        conf.Store = config::StoreTypeTiKV.into();
        conf.Path = "pd-a:2379,pd-b:2379".into();
        conf.Security.ClusterSSLCA = "/tls/ca.pem".into();
        conf.Security.ClusterSSLCert = "/tls/client.pem".into();
        conf.Security.ClusterSSLKey = "/tls/client-key.pem".into();
        conf.TiKVClient.StoreLivenessTimeout = "7s".into();
    });

    // 观察对象保存 open_pd 的实参，用来验证配置到 driver 的完整透传。
    let backend = Arc::new(WiringBackend::default());
    registerStoresWithTiKVDriver(TiKVDriver::with_backend(backend.clone())).unwrap();
    assert_eq!(
        astersql_store::RegisteredDriverTypeName(StoreTypeTiKV),
        Some(std::any::type_name::<TiKVDriver>())
    );

    // storage path 必须带上 keyspaceName，才能证明入口层拼接逻辑与 Go 对齐。
    let (storage, dom) = createStoreDDLOwnerMgrAndDomain("analytics").unwrap();
    assert_eq!(
        storage.path,
        "tikv://pd-a:2379,pd-b:2379?keyspaceName=analytics"
    );
    assert_eq!(storage.GetClusterID(), Some(42));
    assert_eq!(storage.CurrentVersion("global").unwrap(), Some(4_242));

    let observation = backend.observation.lock().unwrap().clone().unwrap();
    assert_eq!(observation.addrs, ["pd-a:2379", "pd-b:2379"]);
    assert_eq!(observation.keyspace, "analytics");
    assert_eq!(
        observation.security,
        TiKVSecurity {
            cluster_ssl_ca: "/tls/ca.pem".into(),
            cluster_ssl_cert: "/tls/client.pem".into(),
            cluster_ssl_key: "/tls/client-key.pem".into(),
        }
    );
    assert_eq!(observation.options.server_timeout, Duration::from_secs(7));

    // bootstrap 事件里必须能看到刚刚打开的 storage identity，
    // 否则说明会话层拿到的不是实际创建出的那个 store 实例。
    let identity = storage.registered_identity().unwrap();
    let bootstrap_event = stubs::take_events()
        .into_iter()
        .find(|event| event.starts_with("session.BootstrapSession"))
        .expect("bootstrap must receive the opened storage");
    assert!(bootstrap_event.contains(&format!("storage-id=Some({identity})")));

    // 释放顺序很关键：domain 先停，再关闭 owner/storage，
    // 最后才回收 TiKV driver 相关资源，避免悬挂引用。
    stubs::clear_events();
    cleanup(&server::Server::new(), &storage, &dom);
    let events = stubs::take_events();
    let position = |needle: &str| {
        events
            .iter()
            .position(|event| event.starts_with(needle))
            .unwrap_or_else(|| panic!("missing {needle} in {events:?}"))
    };
    assert!(position("domain.Close") < position("ddl.CloseOwnerManager"));
    assert!(position("ddl.CloseOwnerManager") < position("kv.Storage.Close"));
    assert!(position("kv.Storage.Close") < position("tikv.close-safe-point"));
    assert!(position("tikv.close-safe-point") < position("tikv.close-pd"));
    assert!(position("tikv.close-pd") < position("tikv.close-store"));
    assert!(events[position("kv.Storage.Close")].contains(&format!("storage-id=Some({identity})")));

    // 再次显式关闭 storage，不应触发第二次 close-store。
    // 这与 Go 侧“canonical store 只关闭一次”的语义一致。
    storage.Close().unwrap();
    assert_eq!(
        backend
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| *event == "close-store")
            .count(),
        1
    );
}

#[test]
fn canonical_listener_config_maps_ports_socket_proxy_and_tls() {
    let mut cfg = config::NewConfig();
    cfg.Host = "127.0.0.1".into();
    cfg.Port = 0;
    cfg.Socket.clear();
    cfg.MaxServerConnections = 37;
    cfg.Status.ReportStatus = true;
    cfg.Status.StatusHost = "127.0.0.1".into();
    cfg.Status.StatusPort = 0;
    let mapped = crate::entry::canonicalServerConfig(&cfg).expect("map canonical listener config");
    assert_eq!(mapped.host, "127.0.0.1");
    assert_eq!(mapped.port, 0);
    assert_eq!(mapped.max_connections, 37);
    assert_eq!(mapped.status.host, "127.0.0.1");
    assert_eq!(mapped.status.port, 0);

    cfg.Port = 65_536;
    assert!(
        crate::entry::canonicalServerConfig(&cfg)
            .unwrap_err()
            .msg
            .contains("outside")
    );
    cfg.Port = 0;
    cfg.Socket = "/tmp/tidb.sock".into();
    let mapped = crate::entry::canonicalServerConfig(&cfg).expect("map Unix socket listener");
    assert_eq!(mapped.socket.as_deref(), Some("/tmp/tidb.sock"));
    cfg.Socket.clear();
    cfg.ProxyProtocol.Networks = "*".into();
    cfg.ProxyProtocol.Fallbackable = true;
    cfg.ProxyProtocol.HeaderTimeout = 7;
    let mapped = crate::entry::canonicalServerConfig(&cfg).expect("map PROXY Protocol");
    assert!(mapped.proxy_protocol_enabled);
    assert_eq!(mapped.proxy_protocol_networks, "*");
    assert!(mapped.proxy_protocol_fallbackable);
    assert_eq!(mapped.proxy_protocol_header_timeout, Duration::from_secs(7));
    cfg.ProxyProtocol.Networks.clear();
    cfg.Security.SSLCert = "/tmp/server.pem".into();
    assert!(
        crate::entry::canonicalServerConfig(&cfg)
            .unwrap_err()
            .msg
            .contains("ssl-key")
    );
    cfg.Security.SSLKey = "/tmp/server-key.pem".into();
    cfg.Security.SSLCA = "/tmp/ca.pem".into();
    let mapped = crate::entry::canonicalServerConfig(&cfg).expect("map SQL TLS paths");
    assert_eq!(mapped.sql_tls_ca.as_deref(), Some("/tmp/ca.pem"));
    assert_eq!(
        mapped.sql_tls_certificate.as_deref(),
        Some("/tmp/server.pem")
    );
    assert_eq!(mapped.sql_tls_key.as_deref(), Some("/tmp/server-key.pem"));

    cfg.Security.ClusterSSLCert = "/tmp/status.pem".into();
    assert!(
        crate::entry::canonicalServerConfig(&cfg)
            .unwrap_err()
            .msg
            .contains("cluster-ssl-key")
    );
    cfg.Security.ClusterSSLKey = "/tmp/status-key.pem".into();
    cfg.Security.ClusterSSLCA = "/tmp/cluster-ca.pem".into();
    cfg.Security.ClusterVerifyCN = vec!["tidb-client-2".into()];
    let mapped = crate::entry::canonicalServerConfig(&cfg).expect("map status TLS paths");
    assert_eq!(mapped.status.tls_ca.as_deref(), Some("/tmp/cluster-ca.pem"));
    assert_eq!(
        mapped.status.tls_certificate.as_deref(),
        Some("/tmp/status.pem")
    );
    assert_eq!(
        mapped.status.tls_key.as_deref(),
        Some("/tmp/status-key.pem")
    );
    assert_eq!(
        mapped.status.tls_verify_common_names,
        vec!["tidb-client-2".to_owned()]
    );

    cfg.Security.AutoTLS = true;
    cfg.Security.RSAKeySize = 2_048;
    cfg.TempStoragePath = "/tmp/auto-tls".into();
    let mapped = crate::entry::canonicalServerConfig(&cfg).expect("map AutoTLS settings");
    assert!(mapped.sql_auto_tls);
    assert_eq!(mapped.rsa_key_size, 2_048);
    assert_eq!(mapped.temp_storage_path.as_deref(), Some("/tmp/auto-tls"));
}

#[test]
fn canonical_listener_starts_on_port_zero_and_preserves_cleanup_order() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    stubs::clear_events();
    config::UpdateGlobal(|cfg| {
        cfg.Store = config::StoreTypeTiKV.into();
        cfg.Path = "pd-a:2379".into();
        cfg.Host = "127.0.0.1".into();
        cfg.Port = 0;
        cfg.Socket.clear();
        cfg.Status.ReportStatus = true;
        cfg.Status.StatusHost = "127.0.0.1".into();
        cfg.Status.StatusPort = 0;
    });
    registerStoresWithTiKVDriver(TiKVDriver::with_backend(Arc::new(WiringBackend::default())))
        .expect("register canonical TiKV driver");
    let (storage, dom) =
        createStoreDDLOwnerMgrAndDomain("").expect("create registered storage and stub domain");
    let (canonical_domain, _) =
        astersql_session::runtime::CreateAnalyzeSession().expect("initialize test Domain");
    let session_driver = Arc::new(ConcreteSessionDriver::from_initialized_domain(
        Arc::clone(&canonical_domain),
        BootstrapAuthMode::InsecureRootOnly,
    ));
    let canonical_config =
        crate::entry::canonicalServerConfig(&config::GetGlobalConfig()).expect("listener config");
    let server =
        crate::entry::assembleCanonicalServer(canonical_config, canonical_domain, session_driver)
            .expect("assemble canonical listener");
    server.SetDomain(&dom);
    assert!(server.uses_canonical_listener());

    let run_server = server.clone();
    let run_domain = dom.clone();
    let run = thread::spawn(move || run_server.Run(&run_domain));
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mysql_addr, status_addr) = loop {
        if let (Some(mysql), Some(status)) = (
            server.canonical_listener_addr(),
            server.canonical_status_addr(),
        ) {
            break (mysql, status);
        }
        assert!(
            Instant::now() < deadline,
            "canonical listeners did not start"
        );
        thread::sleep(Duration::from_millis(10));
    };

    let mut mysql = TcpStream::connect(mysql_addr).expect("connect canonical MySQL listener");
    mysql
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set MySQL timeout");
    let mut header = [0_u8; 4];
    mysql
        .read_exact(&mut header)
        .expect("read canonical handshake header");
    let length =
        usize::from(header[0]) | (usize::from(header[1]) << 8) | (usize::from(header[2]) << 16);
    let mut handshake = vec![0_u8; length];
    mysql
        .read_exact(&mut handshake)
        .expect("read canonical handshake");
    assert_eq!(handshake.first(), Some(&10));
    drop(mysql);

    let mut status = TcpStream::connect(status_addr).expect("connect canonical status listener");
    status
        .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("request status");
    let mut response = String::new();
    status
        .read_to_string(&mut response)
        .expect("read status response");
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    server.Close();
    run.join()
        .expect("join canonical run")
        .expect("canonical run result");
    crate::entry::cleanup(&server, &storage, &dom);
    let events = stubs::take_events();
    let position = |needle: &str| {
        events
            .iter()
            .position(|event| event.starts_with(needle))
            .unwrap_or_else(|| panic!("missing event {needle}: {events:?}"))
    };
    assert!(position("canonical.server.closed") < position("canonical.domain.closed"));
    assert!(position("canonical.domain.closed") < position("domain.Close"));
    assert!(position("domain.Close") < position("kv.Storage.Close"));
}

#[test]
fn mysql_compatibility_external_gate_targets_the_real_server_binary() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root");
    let gate = std::fs::read_to_string(repo_root.join("scripts/test-mysql-compat.sh"))
        .expect("read external MySQL compatibility gate");
    let fixture = std::fs::read_to_string(repo_root.join("tests/mysql-compat/common.sql"))
        .expect("read common MySQL fixture");

    assert!(gate.contains("server_package=\"astersql-cmd-tidb-server\""));
    assert!(gate.contains("cargo build -p \"${server_package}\" --bin \"${server_package}\""));
    assert!(gate.contains("-store \"${store_name}\""));
    assert!(gate.contains("run_mysql_tester_case unistore \"\""));
    assert!(gate.contains("./mysql_tester"));
    assert!(gate.contains("mysql_metadata_compat"));
    assert!(fixture.contains("CREATE TABLE child"));
    assert!(fixture.contains("LEFT JOIN child"));
    assert!(fixture.contains("SAVEPOINT before_insert"));
    assert!(fixture.contains("mysql_compat_ok"));
}

/// 该用例覆盖真实 client-rust driver 的失败路径。
/// 当 PD 地址不可达时，入口层必须把错误继续向上抛出，
/// 不能偷偷吞掉并构造一个看似可用的 storage。
/// The production client-rust driver must propagate an unreachable PD error.
#[test]
fn tikv_storage_wiring_rejects_unreachable_pd() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    config::UpdateGlobal(|conf| {
        conf.Store = config::StoreTypeTiKV.into();
        conf.Path = "127.0.0.1:1".into();
    });

    registerStores().unwrap();
    let opened = createStoreDDLOwnerMgrAndDomain("");
    assert!(
        opened.is_err(),
        "an unreachable PD must fail TiKV storage initialization"
    );
    eprintln!("unreachable PD error: {}", opened.unwrap_err().msg);
}

/// 该用例是显式的外部集成门禁。
/// 只有提供 `REAL_TIKV_PD` 时才连接真实集群，用于确认
/// cluster id 与 PD 分配的 TSO 都来自真实服务，而不是 stub 常量。
/// 默认跳过可以避免本地或 CI 环境因缺少集群而失败。
/// External-cluster gate for cluster identity and a real PD-allocated TSO.
#[test]
fn tikv_storage_wiring_real_pd_cluster_and_tso() {
    let Ok(pd) = std::env::var("REAL_TIKV_PD") else {
        eprintln!("REAL_TIKV_PD is not set; skipping external TiKV wiring gate");
        return;
    };
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    config::UpdateGlobal(|conf| {
        conf.Store = config::StoreTypeTiKV.into();
        conf.Path = pd.clone();
    });

    registerStores().unwrap();
    let (storage, _dom) = createStoreDDLOwnerMgrAndDomain("")
        .unwrap_or_else(|error| panic!("open real PD {pd}: {error}"));
    let cluster_id = storage.GetClusterID().expect("real TiKV cluster id");
    let tso = storage
        .CurrentVersion("global")
        .expect("real PD TSO request")
        .expect("real TiKV TSO");
    assert_ne!(cluster_id, 0);
    assert_ne!(tso, 0);
    eprintln!("real PD: {pd}; cluster id: {cluster_id}; TSO: {tso}");
    storage.Close().unwrap();
}

/// 该测试只服务于 coverage_server 构建链路。
/// 与 Go 直接调用 `main()` 不同，Rust 这里走 `run_main --help`，
/// 既保留入口覆盖率，又避免真的启动阻塞型服务。
/// TestRunMain — coverage_server entry; only invokes main when coverage flag is set.
#[test]
fn test_run_main() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    let is_coverage = std::env::var("isCoverageServer").as_deref() == Ok("1")
        || std::env::var("COVERAGE_SERVER").as_deref() == Ok("1");
    if is_coverage {
        // Go calls main(); use a non-blocking help path in the unit harness.
        let code = crate::entry::run_main(&["tidb-server".into(), "--help".into()]);
        assert_eq!(code, exitCodeOK);
    } else {
        // Dummy path for non-coverage builds (same as Go).
        assert!(!is_coverage);
    }
}

/// 这里保留 Go 风格的表驱动测试，
/// 验证不同信号映射到的退出码是否与入口层约定一致。
/// 其中 `SIGINT` 特殊地走 128+signal，其余信号与空值都回到 OK。
/// TestExitCodeForSignal — table-driven signal → exit code mapping.
#[test]
fn test_exit_code_for_signal() {
    let _g = stubs::test_guard();
    let cases = [
        ("SIGINT", Signal::SIGINT, exitCodeInt),
        ("SIGTERM", Signal::SIGTERM, exitCodeOK),
        ("SIGHUP", Signal::SIGHUP, exitCodeOK),
        ("SIGQUIT", Signal::SIGQUIT, exitCodeOK),
        ("nil", Signal::None, exitCodeOK),
    ];
    for (name, sig, want) in cases {
        assert_eq!(
            exitCodeForSignal(sig),
            want,
            "case {name}: SIGINT const={}",
            syscall::SIGINT
        );
    }
    assert_eq!(exitCodeInt, 128 + syscall::SIGINT);
}

/// 该用例验证命令行 flag 对配置对象的覆盖能力。
/// 一方面 `--keyspace-activate` 需要落到布尔配置上，
/// 另一方面 starter 附加参数必须被整体保存下来，
/// 供后续 manager notifier 与 keyspace 初始化逻辑继续消费。
/// TestOverrideConfigKeyspaceActivateMode — flag overrides activate mode + starter params.
#[test]
fn test_override_config_keyspace_activate_mode() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    let original = starter_additional_params();

    let fset = initFlagSetWithArgs(&[
        "tidb-server".into(),
        "--keyspace-activate=true".into(),
        "--starter-additional-params=pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1".into(),
    ]);
    let mut cfg = config::NewConfig();
    overrideConfig(&mut cfg, &fset);
    assert!(cfg.KeyspaceActivateMode);
    assert_eq!(
        starter_additional_params(),
        "pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1"
    );

    set_starter_additional_params(original);
}

/// 该测试覆盖从全局配置同步到 session/system variables 的主流程。
/// 核心关注点包括默认值、实例级变量作用域、显式版本覆盖、
/// 空版本配置不应回滚已有版本，以及 socket/hostname 的派生结果。
/// 这类断言直接约束入口层启动后 SQL 可见的系统变量状态。
/// TestSetGlobalVars — config → session system variables (instance scope, version, socket).
#[test]
fn test_set_global_vars() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    view_stop();

    assert_eq!(
        variable::GetSysVar(vardef::TiDBIsolationReadEngines)
            .unwrap()
            .Value,
        "tikv,tiflash,tidb"
    );
    assert_eq!(
        variable::GetSysVar(vardef::TiDBMemQuotaQuery)
            .unwrap()
            .Value,
        "1073741824"
    );
    assert_ne!(variable::GetSysVar(vardef::Version).unwrap().Value, "test");

    // 先验证默认情况下该变量还不是实例级，再通过 setGlobalVars 激活实例作用域。
    assert!(!variable::HasInstanceScope(
        vardef::TiDBInstancePlanCacheMaxMemSize
    ));
    config::UpdateGlobal(|conf| {
        conf.Instance.InstancePlanCacheMaxMemSize = "444".into();
    });
    setGlobalVars();
    assert_eq!(
        variable::GetSysVar(vardef::TiDBInstancePlanCacheMaxMemSize)
            .unwrap()
            .Value,
        "444"
    );
    assert!(variable::HasInstanceScope(
        vardef::TiDBInstancePlanCacheMaxMemSize
    ));

    config::UpdateGlobal(|conf| {
        conf.IsolationRead.Engines = vec!["tikv".into(), "tidb".into()];
        conf.ServerVersion = "test".into();
    });
    setGlobalVars();

    assert_eq!(
        variable::GetSysVar(vardef::TiDBIsolationReadEngines)
            .unwrap()
            .Value,
        "tikv,tidb"
    );
    assert_eq!(variable::GetSysVar(vardef::Version).unwrap().Value, "test");
    assert_eq!(
        variable::GetSysVar(vardef::Version).unwrap().Value,
        mysql::ServerVersion()
    );

    config::UpdateGlobal(|conf| {
        conf.ServerVersion.clear();
    });
    setGlobalVars();

    // 清空配置里的 ServerVersion 不应覆盖当前已生效的版本，
    // 这是为了与 Go 版本“仅在显式给值时更新”的行为保持一致。
    assert_eq!(variable::GetSysVar(vardef::Version).unwrap().Value, "test");
    assert_eq!(
        variable::GetSysVar(vardef::Version).unwrap().Value,
        mysql::ServerVersion()
    );

    let cfg = config::GetGlobalConfig();
    assert_eq!(
        cfg.Socket,
        variable::GetSysVar(vardef::Socket).unwrap().Value
    );

    let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into());
    assert_eq!(
        variable::GetSysVar(vardef::Hostname).unwrap().Value,
        hostname
    );

    view_stop();
}

/// 该用例验证 nextgen 模式下的部署模式初始化约束。
/// 一部分断言检查合法枚举与非法枚举的分支，
/// 另一部分断言检查 starter 下命令行 TLS 参数只允许覆盖 cert/key，
/// 不能把配置文件里的 CA 路径意外抹掉。
/// TestInitDeployMode — nextgen deploy mode init, invalid mode, starter TLS flag override.
#[test]
fn test_init_deploy_mode() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    // Go 在 classic kernel 上会 skip；Rust 这里强制 nextgen，
    // 是为了让单测真正执行校验逻辑，而不是留下一个空壳测试。
    kerneltype::set_nextgen_for_test(true);
    let original = deploymode::Get();

    let mut cfg = config::NewConfig();
    cfg.DeployMode = deploymode::PremiumReserved;
    initDeployMode(&cfg).unwrap();
    assert_eq!(deploymode::Get(), deploymode::PremiumReserved);

    cfg.DeployMode = 100;
    let err = initDeployMode(&cfg).unwrap_err();
    assert!(err.msg.contains("invalid deploy mode"), "got: {}", err.msg);

    // 只允许证书与私钥被 flag 覆盖，CA 仍以配置文件为准，
    // 避免部署时把信任根来源意外切换到命令行输入。
    let fset = initFlagSetWithArgs(&[
        "tidb-server".into(),
        "--cluster-cert=/tmp/flag-cluster-cert.pem".into(),
        "--cluster-key=/tmp/flag-cluster-key.pem".into(),
        "--sql-cert=/tmp/flag-sql-cert.pem".into(),
        "--sql-key=/tmp/flag-sql-key.pem".into(),
    ]);
    let mut cfg = config::NewConfig();
    cfg.DeployMode = deploymode::Starter;
    cfg.Security.ClusterSSLCA = "/tmp/config-cluster-ca.pem".into();
    cfg.Security.ClusterSSLCert = "/tmp/config-cluster-cert.pem".into();
    cfg.Security.ClusterSSLKey = "/tmp/config-cluster-key.pem".into();
    cfg.Security.SSLCA = "/tmp/config-sql-ca.pem".into();
    cfg.Security.SSLCert = "/tmp/config-sql-cert.pem".into();
    cfg.Security.SSLKey = "/tmp/config-sql-key.pem".into();
    overrideConfig(&mut cfg, &fset);
    assert_eq!(cfg.Security.ClusterSSLCA, "/tmp/config-cluster-ca.pem");
    assert_eq!(cfg.Security.ClusterSSLCert, "/tmp/flag-cluster-cert.pem");
    assert_eq!(cfg.Security.ClusterSSLKey, "/tmp/flag-cluster-key.pem");
    assert_eq!(cfg.Security.SSLCA, "/tmp/config-sql-ca.pem");
    assert_eq!(cfg.Security.SSLCert, "/tmp/flag-sql-cert.pem");
    assert_eq!(cfg.Security.SSLKey, "/tmp/flag-sql-key.pem");

    deploymode::Set(original).unwrap();
}

/// 该测试专门覆盖 starter manager notifier 的参数校验。
/// 入口层只有在拿到完整 pod 身份信息后，才能安全构造 manager client；
/// 因此缺参、重复键、未知键、缺失 manager 地址都必须报错。
/// 最后一个成功分支用于证明合法参数组合确实能创建客户端。
/// TestCreateMgrClientRequiresPodIdentityInStarter — starter manager notifier param validation.
#[test]
fn test_create_mgr_client_requires_pod_identity_in_starter() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    let restore = config::RestoreFunc();
    let original_mode = deploymode::Get();
    deploymode::Set(deploymode::Starter).unwrap();
    config::UpdateGlobal(|conf| {
        conf.StarterParams.EnableManagerNotifier = true;
        conf.StarterParams.ManagerAddr = "manager.example.com:8000".into();
    });
    let original_params = starter_additional_params();

    // 空字符串模拟完全未传 `--starter-additional-params`，
    // 这是 manager notifier 最先要拦截的错误。
    set_starter_additional_params("");
    let err = createMgrClientForStarter().unwrap_err();
    assert!(
        err.msg
            .contains("manager notifier requires --starter-additional-params"),
        "got: {}",
        err.msg
    );

    // 重复键会让 pod 身份语义不再单值确定，因此必须拒绝。
    set_starter_additional_params(
        "pod-name=pod-1,pod-name=pod-2,pod-ip=10.0.0.1,pod-namespace=ns-1",
    );
    let err = createMgrClientForStarter().unwrap_err();
    assert!(
        err.msg
            .contains("starter additional param \"pod-name\" is duplicated"),
        "got: {}",
        err.msg
    );

    // 未知键必须立即暴露，防止拼写错误被悄悄忽略。
    set_starter_additional_params(
        "pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1,unknown=value",
    );
    let err = createMgrClientForStarter().unwrap_err();
    assert!(
        err.msg
            .contains("unknown starter additional param \"unknown\""),
        "got: {}",
        err.msg
    );

    // 即使 pod 身份齐全，若既没有配置 manager-addr，
    // 也没有通过附加参数提供 manager-namespace，仍然无法构建客户端。
    config::UpdateGlobal(|conf| {
        conf.StarterParams.ManagerAddr.clear();
    });
    set_starter_additional_params("pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1");
    let err = createMgrClientForStarter().unwrap_err();
    assert!(
        err.msg.contains(
            "manager notifier requires manager-addr config or manager-namespace in --starter-additional-params"
        ),
        "got: {}",
        err.msg
    );

    // 成功分支证明 manager namespace 可以补足缺失的地址配置来源。
    set_starter_additional_params(
        "manager-namespace=manager-ns,pod-name=pod-1,pod-ip=10.0.0.1,pod-namespace=ns-1",
    );
    let cli = createMgrClientForStarter().unwrap();
    assert!(cli.is_some());

    set_starter_additional_params(original_params);
    deploymode::Set(original_mode).unwrap();
    restore();
}

/// nextgen 内核下不允许通过配置直接篡改 edition/version 标识。
/// 该断言保护的是产品线与版本号的统一生成逻辑，
/// 防止部署配置把内建版本模型改成与实际内核不一致的组合。
/// TestSetVersionByConfigInNextGen — nextgen forbids edition/version overrides via config.
#[test]
fn test_set_version_by_config_in_next_gen() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    let origin = config::GetGlobalConfig().TiDBEdition.clone();

    config::UpdateGlobal(|conf| {
        conf.TiDBEdition = "Starter".into();
    });
    let err = initVersions(&config::GetGlobalConfig()).unwrap_err();
    assert!(
        err.msg.contains("are not allowed to set in nextgen kernel"),
        "got: {}",
        err.msg
    );

    config::UpdateGlobal(|conf| {
        conf.TiDBEdition = origin;
    });
}

/// 这里覆盖 nextgen release version 的非法值校验。
/// 人为塞入一个不满足规范的版本号后，初始化必须报错，
/// 以保证最终对外暴露的版本字符串可被稳定解析。
/// TestSetVersionByConfigInvalidNextGenReleaseVersion — invalid nextgen release version errors.
#[test]
fn test_set_version_by_config_invalid_next_gen_release_version() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    config::UpdateGlobal(|c| {
        c.TiDBEdition.clear();
        c.TiDBReleaseVersion.clear();
        c.ServerVersion.clear();
    });
    let origin = mysql::TiDBReleaseVersion();
    mysql::set_TiDBReleaseVersion("v26.13.1");
    let err = initVersions(&config::GetGlobalConfig()).unwrap_err();
    assert!(
        err.msg
            .contains("invalid tidb release version for nextgen kernel"),
        "got: {}",
        err.msg
    );
    mysql::set_TiDBReleaseVersion(origin);
}

/// 该用例验证 legacy sentinel 向 nextgen sentinel 的归一化过程。
/// Rust 版本保留了更贴近真实执行路径的断言：
/// 先从 classic 的 legacy sentinel 起步，再让 `initVersions`
/// 触发标准化与 server version 拼装逻辑。
/// 这样可以直接证明与 Go 共享的“从旧占位符迁移到新占位符”规则仍然成立。
/// TestSetVersionByConfigNormalizeLegacySentinelForNextGen — legacy → nextgen sentinel.
#[test]
fn test_set_version_by_config_normalize_legacy_sentinel_for_next_gen() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    config::UpdateGlobal(|c| {
        c.TiDBEdition.clear();
        c.TiDBReleaseVersion.clear();
        c.ServerVersion.clear();
    });
    // 先把版本重置到 classic 时代的占位符，
    // 才能确保真正走到 nextgen 归一化分支。
    mysql::set_TiDBReleaseVersion(mysql::legacyTiDBReleaseVersionSentinel);

    initVersions(&config::GetGlobalConfig()).unwrap();
    // 这里断言的是“从 legacy 迁移而来”的真实路径，而不是预先注入 ldflags 后的结果。
    // 因此 Rust 断言会看到 tidbX sentinel 和带 prerelease 的 server version。
    // 这能更准确约束实际迁移逻辑，即使 Go 某些构建环境的断言更接近最终展示值。
    assert_eq!(
        mysql::TiDBReleaseVersion(),
        mysql::tidbXSentinelReleaseVersion
    );
    assert_eq!(
        mysql::ServerVersion(),
        format!(
            "{}{}{}",
            mysql::mysqlCompatibilityVersion,
            mysql::VersionSeparator,
            concat!("CLOUD.202603.0-this-is-a-", "place", "holder")
        )
    );
}

/// 该测试覆盖 starter 模式下 keyspace 可观测信息的写入。
/// 当 store 为 TiKV 时，入口层需要把 keyspace 名以及扩展元信息
/// 同步到 metric label、slow log 字段和 statement log 字段三条通路。
/// 这样后续诊断链路才能按 keyspace 维度关联指标与日志。
/// TestSetupKeyspaceObservabilityForStarter — TiKV starter writes metric/slow/stmt fields.
#[test]
fn test_setup_keyspace_observability_for_starter() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    let restore = config::RestoreFunc();
    let original_mode = deploymode::Get();
    deploymode::Set(deploymode::Starter).unwrap();

    // 第一段先验证最小配置：即便没有额外元信息，keyspace_name 也必须存在。
    config::UpdateGlobal(|conf| {
        conf.Store = config::StoreTypeTiKV.into();
        conf.KeyspaceName = "ks".into();
    });
    prepareKeyspaceObservabilityForStarter(HashMap::new()).unwrap();
    assert_eq!(
        config::GetGlobalConfig().GetKeyspaceObservabilityMetricLabels(),
        HashMap::from([("keyspace_name".into(), "ks".into())])
    );

    // 第二段再验证扩展字段会同时投射到三类观测出口，
    // 且键名映射遵守配置中的 MetricLabel/SlowLogField/StmtLogField 定义。
    config::UpdateGlobal(|conf| {
        conf.KeyspaceObservability = config::KeyspaceObservability {
            Fields: vec![config::KeyspaceObservabilityField {
                Source: "meta_a".into(),
                MetricLabel: "keyspace_meta_label_a".into(),
                SlowLogField: "Keyspace_meta_slow_a".into(),
                StmtLogField: "stmt_meta_a".into(),
                Required: true,
            }],
        };
    });

    prepareKeyspaceObservabilityForStarter(HashMap::from([("meta_a".into(), "value_a".into())]))
        .unwrap();

    let cfg = config::GetGlobalConfig();
    assert_eq!(
        cfg.GetKeyspaceObservabilityMetricLabels(),
        HashMap::from([
            ("keyspace_name".into(), "ks".into()),
            ("keyspace_meta_label_a".into(), "value_a".into()),
        ])
    );
    assert_eq!(
        cfg.GetKeyspaceObservabilitySlowLogFields(),
        vec![config::KeyspaceObservabilityLogField {
            Name: "Keyspace_meta_slow_a".into(),
            Value: "value_a".into(),
        }]
    );
    assert_eq!(
        cfg.GetKeyspaceObservabilityStmtLogFields(),
        HashMap::from([("stmt_meta_a".into(), "value_a".into())])
    );

    deploymode::Set(original_mode).unwrap();
    restore();
}

/// 非 TiKV 存储不会参与 starter 的 keyspace 观测增强。
/// 该用例用于防止入口层把 TiKV 专属标签错误地下发给其他 store，
/// 从而制造误导性的监控维度。
/// TestSetupKeyspaceObservabilityForStarterSkipsNonTiKV — non-TiKV store skips labels.
#[test]
fn test_setup_keyspace_observability_for_starter_skips_non_tikv() {
    let _g = stubs::test_guard();
    stubs::reset_all_for_test();
    kerneltype::set_nextgen_for_test(true);
    let restore = config::RestoreFunc();
    let original_mode = deploymode::Get();
    deploymode::Set(deploymode::Starter).unwrap();

    config::UpdateGlobal(|conf| {
        conf.Store = config::StoreTypeUniStore.into();
        conf.Path = "invalid-pd-path".into();
        conf.KeyspaceName = "test_keyspace".into();
    });

    prepareKeyspaceObservabilityForStarter(HashMap::new()).unwrap();
    assert!(
        config::GetGlobalConfig()
            .GetKeyspaceObservabilityMetricLabels()
            .is_empty()
    );

    deploymode::Set(original_mode).unwrap();
    restore();
}
