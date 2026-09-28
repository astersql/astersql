// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Mock TiDB cluster matching `br/pkg/mock/mock_cluster.go`.
//!
//! Heavy TiDB/PD/TiKV types are local stand-ins in [`crate::stubs`] so this
//! package stays free of the grpcio rebuild path on darwin arm64.
//!
//! 模拟 TiDB 集群门面：对齐 Go `mock_cluster.go` 的 NewCluster / Start / Stop
//! 生命周期，供 BR 单测在无真实 PD/TiKV 时拉起可连接的 SQL/HTTP 探针环境。
//! TiDB/PD/TiKV 重型类型由 [`crate::stubs`] 本地替身承担，避免 darwin arm64
//! 上走 grpcio 重建路径；行为语义以 Go 为准，桩实现只保证端口就绪与资源关闭。
//! 进程级 pprof 监听（12235）用 `Once` 只启动一次，与 Go `pprofOnce` 一致。
//! Start 与 Stop 必须成对出现，否则 Domain/Server 句柄与 online 标志会泄漏到后续用例。

use std::sync::{Arc, Once};

use crate::stubs::{
    self, BootstrapSession, BootstrapWithSingleStore, Config, Domain, Error, HttpServer,
    MysqlConfig, NewMockStoreWithoutBootstrap, NewServer, NewTiDBDriver, PDClient, PDHTTPClient,
    RUN_IN_GO_TEST, Result, Server, Storage, StoreType, TiDBDriver, TiKVCluster, view_Stop,
};

// 进程级 pprof：与 Go `var pprofOnce sync.Once` 对应，保证 ListenAndServe 只跑一次。
static PPROF_ONCE: Once = Once::new();

/// Cluster is mock tidb cluster, includes tikv and pd.
///
/// 聚合 mock 集群运行时句柄：Server/驱动/Domain/Storage、PD 客户端与 DSN。
/// 字段均为 Option 或空串，因 NewCluster 与 Start 分两阶段填充（Go 嵌入指针同理）。
pub struct Cluster {
    pub Server: Option<Server>,
    pub Cluster: Option<TiKVCluster>,
    pub Storage: Option<Storage>,
    pub TiDBDriver: Option<TiDBDriver>,
    pub Domain: Option<Domain>,
    pub DSN: String,
    pub PDClient: Option<PDClient>,
    pub PDHTTPCli: Option<PDHTTPClient>,
    pub HttpServer: Option<HttpServer>,
}

impl Default for Cluster {
    fn default() -> Self {
        // 全空初始态：对应 Go `cluster := &Cluster{}`，随后由 NewCluster/Start 写入。
        Self {
            Server: None,
            Cluster: None,
            Storage: None,
            TiDBDriver: None,
            Domain: None,
            DSN: String::new(),
            PDClient: None,
            PDHTTPCli: None,
            HttpServer: None,
        }
    }
}

/// NewCluster create a new mock cluster.
///
/// 创建未 Start 的集群：启动（或复用）pprof、bootstrap mock store、挂 Domain/PD。
/// 此时尚无 SQL 监听端口；调用方必须再 `Start` 才能得到可用 DSN。
pub fn NewCluster() -> Result<Cluster> {
    let mut cluster = Cluster::default();

    // Go: pprofOnce.Do — start pprof listener once per process.
    // 首次调用的实例登记 0.0.0.0:12235；桩版 ListenAndServe 不真实 bind。
    PPROF_ONCE.call_once(|| {
        let addr = "0.0.0.0:12235".to_string();
        let http = HttpServer {
            Addr: addr.clone(),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            listening: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        // Stand-in for ListenAndServe (no real bind on arm64 unit tests).
        let _ = http.ListenAndServe();
        // Go's closure captures this invocation's `cluster`; later calls do
        // not inherit the first cluster's server handle.
        cluster.HttpServer = Some(http);
    });

    // 捕获 BootstrapWithSingleStore 注入的 TiKVCluster，对齐 Go inspector 闭包赋值。
    let mut captured: Option<TiKVCluster> = None;
    let storage = NewMockStoreWithoutBootstrap(|c| {
        BootstrapWithSingleStore(c);
        captured = Some(c.clone());
    })
    .map_err(Error::Trace)?;
    cluster.Cluster = captured;
    cluster.Storage = Some(storage.clone());

    // 测试路径禁统计，减少 Domain bootstrap 开销（Go session.DisableStats4Test）。
    stubs::DisableStats4Test();
    let dom = BootstrapSession(&storage).map_err(Error::Trace)?;
    cluster.Domain = Some(dom);

    // Go: storage.(tikv.Storage).GetRegionCache().PDClient() / GetPDHTTPClient()
    // 从 mock storage 取出 PD 客户端，供连接层单测直接使用。
    cluster.PDClient = Some(storage.GetRegionCache().PDClient());
    cluster.PDHTTPCli = Some(storage.GetPDHTTPClient());
    Ok(cluster)
}

impl Cluster {
    /// Start runs a mock cluster.
    ///
    /// 拉起 mock TiDB Server：Port/StatusPort 置 0 让实现选临时端口，
    /// 后台 `Run` 后阻塞等待 `RunInGoTestChan`，再探针 SQL/HTTP 并写入 DSN。
    pub fn Start(&mut self) -> Result<()> {
        // 标记 Go 测试模式，使 Server.Run 走 channel 就绪通知而非常驻阻塞。
        RUN_IN_GO_TEST.store(true, std::sync::atomic::Ordering::SeqCst);
        let ready_rx = stubs::make_run_in_go_test_chan();

        let storage = self
            .Storage
            .as_ref()
            .ok_or_else(|| Error::new("nil storage"))?;
        self.TiDBDriver = Some(NewTiDBDriver(storage));

        let mut cfg = Config::NewConfig();
        // let tidb random select a port
        // Port/StatusPort=0：与 Go 一致，避免固定端口冲突。
        cfg.Port = 0;
        cfg.Store = StoreType::TiKV;
        cfg.Status.StatusPort = 0;
        cfg.Status.ReportStatus = true;
        // Unix socket 路径带纳秒后缀，降低并行单测撞名概率。
        cfg.Socket = format!(
            "/tmp/tidb-mock-{}.sock",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        let driver = self.TiDBDriver.as_ref().unwrap();
        let svr = NewServer(&cfg, driver).map_err(Error::Trace)?;
        // NewServer may assign ephemeral status port when 0.
        let status_port = svr.status_port;
        self.Server = Some(svr.clone());

        // 独立线程跑 Server，主线程等就绪信号（Go: go svr.Run() + <-chan）。
        let svr_run = svr.clone();
        std::thread::spawn(move || {
            if let Err(err1) = svr_run.Run(None) {
                panic!("{err1}");
            }
        });
        // Go: <-server.RunInGoTestChan
        let _ = ready_rx.recv();

        // Mark online so default SQL/HTTP probes succeed (mirrors live server).
        // 桩探针依赖 online 标志；先置真再 wait，避免空转重试耗尽。
        stubs::set_cluster_online(true);
        self.DSN = waitUntilServerOnline("127.0.0.1", status_port as u32);
        Ok(())
    }

    /// Stop stops a mock cluster.
    ///
    /// 按 Domain → Storage → Server → HttpServer 顺序关闭，再停 view 与 online 标志。
    /// 与 Go 一致：各 Close 容错调用，不因单步失败中断后续清理。
    pub fn Stop(&mut self) {
        if let Some(domain) = &self.Domain {
            domain.Close();
        }
        if let Some(storage) = &self.Storage {
            let _ = storage.Close();
        }
        if let Some(server) = &self.Server {
            server.Close();
        }
        if let Some(http_server) = &self.HttpServer {
            let _ = http_server.Close();
        }
        // OpenCensus view 后台 worker 收尾，对齐 Go view.Stop()。
        view_Stop();
        stubs::set_cluster_online(false);
    }
}

/// configOverrider matches Go `func(*mysql.Config)`.
///
/// 可变闭包覆盖默认 MysqlConfig，供 getDSN 组合多条覆写（含 None 槽位跳过）。
pub type ConfigOverrider = Box<dyn FnMut(&mut MysqlConfig)>;

// Go 默认重试次数常量；桩侧实际次数由 stubs::retry_time() 覆盖以便单测加速。
const RETRY_TIME_DEFAULT: i32 = 100;

/// defaultDSNConfig matches Go package-level default.
///
/// 默认 root@tcp(127.0.0.1:4001)，与 Go 包级 defaultDSNConfig 字段一致。
pub fn default_dsn_config() -> MysqlConfig {
    MysqlConfig {
        User: "root".into(),
        Net: "tcp".into(),
        Addr: "127.0.0.1:4001".into(),
        ..MysqlConfig::default()
    }
}

/// getDSN generates a DSN string for MySQL connection.
///
/// 依次应用 overrider（跳过 None），再 FormatDSN；空切片返回默认 DSN。
pub fn getDSN(overriders: Vec<Option<ConfigOverrider>>) -> String {
    let mut cfg = default_dsn_config();
    for mut overrider in overriders {
        if let Some(ref mut overrider) = overrider {
            overrider(&mut cfg);
        }
    }
    cfg.FormatDSN()
}

/// waitUntilServerOnline waits for MySQL and HTTP status, returns DSN prefix.
///
/// 先轮询 SQL Open，再 GET `/status`；任一路耗尽重试则 panic（与 Go 相同）。
/// 成功后返回 `strings.SplitAfter(dsn, "/")[0]` 形态的前缀，供 Cluster.DSN 使用。
pub fn waitUntilServerOnline(addr: &str, status_port: u32) -> String {
    let retry_time = stubs::retry_time();
    let mut retry = 0;
    // 把探测地址写进 DSN，覆盖默认 4001。
    let dsn = getDSN(vec![Some(Box::new({
        let addr = addr.to_string();
        move |cfg: &mut MysqlConfig| {
            cfg.Addr = addr.clone();
        }
    }))]);

    // SQL 就绪：失败则 sleep 后重试，成功 Close 连接即退出循环。
    while retry < retry_time {
        stubs::sleep_retry();
        match stubs::sql_open("mysql", &dsn) {
            Ok(db) => {
                db.Close();
                break;
            }
            Err(_) => {
                retry += 1;
            }
        }
    }
    if retry == retry_time {
        panic!(
            "failed to connect DB in every 10 ms retryTime={}",
            retry_time
        );
    }

    // HTTP status 就绪：与 Go 一样访问 127.0.0.1:statusPort/status。
    let status_url = format!("http://127.0.0.1:{status_port}/status");
    retry = 0;
    for attempt in 0..retry_time {
        // Go uses `for retry = range retryTime`; after natural exhaustion the
        // loop variable remains `retryTime - 1`, rather than `retryTime`.
        retry = attempt;
        match stubs::http_get(&status_url) {
            Ok(resp) => {
                let _ = resp.Body();
                break;
            }
            Err(_) => {
                stubs::sleep_retry();
            }
        }
    }
    if retry == retry_time {
        panic!(
            "failed to connect HTTP status in every 10 ms retryTime={retry_time} url={status_url}"
        );
    }

    // Go: strings.SplitAfter(dsn, "/")[0]
    // 保留首个 '/' 及之前部分作为 DSN 前缀。
    split_after_first(&dsn, '/').to_string()
}

/// 等价 Go `strings.SplitAfter(s, sep)[0]`：找不到分隔符则返回整串。
fn split_after_first(s: &str, sep: char) -> &str {
    match s.find(sep) {
        Some(i) => &s[..=i],
        None => s,
    }
}

#[allow(dead_code)]
const _RETRY_TIME_DEFAULT: i32 = RETRY_TIME_DEFAULT;
