// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TiDB server 集成测试套件（testkit）。
//
// 对应 Go `servertestkit`：构造带临时端口的 Server/Domain/Driver，
// 提供 DDL schema lease（表结构租约）覆盖、TopSQL（Top SQL 采样）套件，
// 以及后台循环执行 SQL 的测试辅助。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_server::server::{Domain, Server, ServerConfig, ServerDriver, StatusConfig};
use astersql_server_internal_testserverclient::TestServerClient;
use astersql_server_internal_testutil::get_port_from_tcp_addr;
use astersql_sessionctx_vardef::SetSchemaLease;
use astersql_util_cpuprofile::{StartCPUProfiler, StopCPUProfiler};
use astersql_util_topsql_collector_mock::TopSQLCollector;
use astersql_util_topsql_reporter::report_ticker::SetReportTickerIntervalSecondsForTest;
use astersql_util_topsql_state::{DefTiDBTopSQLPrecisionSeconds, GlobalState};

/// TidbTestSuite is a test suite for tidb
/// TidbTestSuite 对应 Go 的同名结构体，聚合测试客户端、TiDB driver、server、domain 和 mock store。
pub struct TidbTestSuite {
    /// 封装 MySQL 协议客户端与 status HTTP 访问。
    pub test_server_client: TestServerClient,
    /// Server 驱动（查询执行入口的测试替身）。
    pub tidbdrv: Arc<dyn ServerDriver>,
    /// 已启动的 MySQL 协议 Server 实例。
    pub server: Arc<Server>,
    /// Domain：TiDB 中承载 schema/统计等元信息的域对象；此处为测试替身。
    pub domain: Arc<dyn Domain>,
    /// 逆序执行的清理回调列表（在 Drop 时调用）。
    cleanup: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

/// 测试用最小 ServerDriver，仅暴露固定驱动名。
struct SuiteDriver;

impl ServerDriver for SuiteDriver {
    fn name(&self) -> &str {
        "tidb-test-suite"
    }
}

/// 测试用 Domain：提供固定 server_id 与启动时间戳。
struct SuiteDomain {
    server_id: u64,
    start_timestamp: i64,
}

impl Domain for SuiteDomain {
    fn server_id(&self) -> u64 {
        self.server_id
    }

    fn start_timestamp(&self) -> i64 {
        self.start_timestamp
    }
}

impl Drop for TidbTestSuite {
    fn drop(&mut self) {
        // 逆序执行清理回调，再关闭 Server，避免资源泄漏。
        let cleanups = std::mem::take(&mut *self.cleanup.lock().expect("cleanup lock"));
        for cleanup in cleanups.into_iter().rev() {
            cleanup();
        }
        self.server.close();
    }
}

impl TidbTestSuite {
    /// 返回连接本套件 Server 的 DSN（数据源名）字符串。
    pub fn get_dsn(&self) -> String {
        self.test_server_client.get_dsn(&[])
    }

    /// 注册 Drop 时执行的清理回调。
    fn push_cleanup(&self, cleanup: impl FnOnce() + Send + 'static) {
        self.cleanup
            .lock()
            .expect("cleanup lock")
            .push(Box::new(cleanup));
    }
}

/// CreateTidbTestSuite creates a test suite for tidb
/// create_tidb_test_suite 对应 Go 的 CreateTidbTestSuite，使用默认测试配置启动套件。
pub fn create_tidb_test_suite() -> TidbTestSuite {
    create_tidb_test_suite_with_cfg(new_test_config())
}

/// CreateTidbTestSuiteWithDDLLease creates a test suite with DDL lease for tidb.
/// create_tidb_test_suite_with_ddl_lease 对应 Go 的 DDL lease 变体，先覆盖 schema lease 再复用通用启动流程。
pub fn create_tidb_test_suite_with_ddl_lease(ddl_lease: &str) -> TidbTestSuite {
    let ddl_lease_duration = parse_duration(ddl_lease).expect("ddl lease should parse");
    SetSchemaLease(ddl_lease_duration);
    create_tidb_test_suite_with_cfg(new_test_config())
}

/// new_test_config 对应 Go 的 newTestConfig，生成 server 测试所需的端口和 status 配置。
pub fn new_test_config() -> ServerConfig {
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
    }
}

/// parseDuration parses lease argument string.
/// parse_duration 对应 Go 的 parseDuration：先按完整 duration 解析，失败后追加秒单位再试。
pub fn parse_duration(lease: &str) -> Result<Duration, String> {
    match parse_go_duration(lease).or_else(|_| parse_go_duration(&format!("{lease}s"))) {
        Ok(duration) => Ok(duration),
        Err(_) => Err(format!("invalid lease duration: {lease}")),
    }
}

/// Parse the positive subset of Go `time.ParseDuration` accepted by `parseDuration`.
///
/// `time.Duration` is signed while Rust's `Duration` is not; negative values are
/// rejected by the Go wrapper, so parsing them here as an error preserves the
/// wrapper's public contract.  Component parsing deliberately accepts Go's
/// compound, decimal and microsecond forms instead of only the lease examples.
fn parse_go_duration(value: &str) -> Result<Duration, String> {
    const MAX_GO_DURATION_NANOS: u128 = i64::MAX as u128;

    if value == "0" {
        return Ok(Duration::from_secs(0));
    }
    let negative = value.starts_with('-');
    let mut rest = value.strip_prefix(['+', '-']).unwrap_or(value);
    if rest.is_empty() {
        return Err("empty duration".into());
    }

    let mut total_nanos = 0_u128;
    while !rest.is_empty() {
        let number_len = rest
            .bytes()
            .take_while(|byte| byte.is_ascii_digit() || *byte == b'.')
            .count();
        let number = &rest[..number_len];
        if number.is_empty() || number.bytes().filter(|byte| *byte == b'.').count() > 1 {
            return Err(format!("invalid duration {value}"));
        }
        let (integer, fraction) = number.split_once('.').unwrap_or((number, ""));
        if integer.is_empty() && fraction.is_empty()
            || !integer.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(format!("invalid duration {value}"));
        }

        rest = &rest[number_len..];
        let (unit_nanos, unit_len) = if rest.starts_with("ns") {
            (1_u128, 2)
        } else if rest.starts_with("us") {
            (1_000_u128, 2)
        } else if rest.starts_with("µs") || rest.starts_with("μs") {
            (1_000_u128, "µs".len())
        } else if rest.starts_with("ms") {
            (1_000_000_u128, 2)
        } else if rest.starts_with('s') {
            (1_000_000_000_u128, 1)
        } else if rest.starts_with('m') {
            (60_000_000_000_u128, 1)
        } else if rest.starts_with('h') {
            (3_600_000_000_000_u128, 1)
        } else {
            return Err(format!("invalid duration {value}"));
        };
        rest = &rest[unit_len..];

        let whole = if integer.is_empty() {
            0
        } else {
            integer
                .parse::<u128>()
                .map_err(|_| format!("invalid duration {value}"))?
        };
        let whole_nanos = whole
            .checked_mul(unit_nanos)
            .ok_or_else(|| format!("invalid duration {value}"))?;
        // Eighteen decimal places are sufficient: a one-hour unit contributes
        // less than one nanosecond beyond that precision.
        let fraction = &fraction[..fraction.len().min(18)];
        let fraction_nanos = if fraction.is_empty() {
            0
        } else {
            let numerator = fraction
                .parse::<u128>()
                .map_err(|_| format!("invalid duration {value}"))?;
            let denominator = 10_u128.pow(fraction.len() as u32);
            numerator * unit_nanos / denominator
        };
        total_nanos = total_nanos
            .checked_add(whole_nanos)
            .and_then(|total| total.checked_add(fraction_nanos))
            .filter(|total| *total <= MAX_GO_DURATION_NANOS)
            .ok_or_else(|| format!("invalid duration {value}"))?;
    }

    if negative && total_nanos != 0 {
        return Err(format!("invalid duration {value}"));
    }
    Ok(Duration::from_nanos(total_nanos as u64))
}

/// CreateTidbTestSuiteWithCfg creates a test suite for tidb with config
/// create_tidb_test_suite_with_cfg 对应 Go 的核心启动流程，按顺序创建 driver、domain 和 server。
pub fn create_tidb_test_suite_with_cfg(cfg: ServerConfig) -> TidbTestSuite {
    let mut client = TestServerClient::new();
    let tidbdrv: Arc<dyn ServerDriver> = Arc::new(SuiteDriver);
    let server = Server::new(cfg, Arc::clone(&tidbdrv)).expect("server should be created");
    let start_timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs() as i64;
    let domain: Arc<dyn Domain> = Arc::new(SuiteDomain {
        server_id: 1,
        start_timestamp,
    });

    // Go 通过 RunInGoTestChan 等待 server.Run 完成监听初始化；这里直接完成 listener/status 启动。
    server.run(Arc::clone(&domain)).expect("server should run");
    server
        .start_status_http()
        .expect("status server should start");

    // 回填客户端实际绑定的 SQL 端口与 status HTTP 端口。
    let listen_addr = server
        .listener_addr()
        .expect("listener address should exist");
    let status_addr = server
        .status_listener_addr()
        .expect("status listener address should exist");
    client.host = status_addr.ip().to_string();
    client.port = get_port_from_tcp_addr(listen_addr);
    client.status_port = get_port_from_tcp_addr(status_addr);
    // 轮询直到 Server 对外可连接，超时 5 秒。
    client
        .wait_until_server_online(Duration::from_secs(5))
        .expect("server should become online");

    TidbTestSuite {
        test_server_client: client,
        tidbdrv,
        server,
        domain,
        cleanup: Mutex::new(Vec::new()),
    }
}

/// tidbTestTopSQLSuite 对应 Go 的 tidbTestTopSQLSuite，复用基础 TiDB suite 并附加 TopSQL 初始化。
pub struct TidbTestTopSqlSuite {
    /// 内嵌的基础 TiDB 测试套件。
    pub base: TidbTestSuite,
    /// Drop 时恢复 report ticker 间隔的回调。
    restore_ticker: Option<Box<dyn FnOnce() + Send>>,
}

/// CreateTidbTestTopSQLSuite creates a test suite for top-sql test.
/// create_tidb_test_top_sql_suite 对应 Go 的 TopSQL 套件创建逻辑。
pub fn create_tidb_test_top_sql_suite() -> TidbTestTopSqlSuite {
    let base = create_tidb_test_suite();

    // Initialize global variable for top-sql test.
    // 缩短 TopSQL 精度与上报间隔，便于单测尽快观测采样结果。
    GlobalState.PrecisionSeconds.store(1, Ordering::SeqCst);
    // Go 通过 `set @@global.tidb_top_sql_max_time_series_count=5` 更新同一
    // 进程级 TopSQL 状态；Rust 测试服务器没有 SQL system-variable bridge，
    // 因此直接写入其规范状态字段以保持相同副作用。
    GlobalState.MaxStatementCount.store(5, Ordering::SeqCst);
    let restore_ticker = SetReportTickerIntervalSecondsForTest(2);
    StartCPUProfiler().expect("cpu profiler should start");

    TidbTestTopSqlSuite {
        base,
        restore_ticker: Some(restore_ticker),
    }
}

impl Drop for TidbTestTopSqlSuite {
    fn drop(&mut self) {
        // 停止 CPU profiler，并恢复 TopSQL 全局精度与 ticker 间隔。
        StopCPUProfiler();
        GlobalState
            .PrecisionSeconds
            .store(DefTiDBTopSQLPrecisionSeconds, Ordering::SeqCst);
        if let Some(restore) = self.restore_ticker.take() {
            restore();
        }
    }
}

impl TidbTestTopSqlSuite {
    /// TestCase is to run the test case for top-sql test.
    /// test_case 对应 Go 的 TestCase：后台循环执行 SQL，前台执行检查函数，随后 cancel 并清理 collector。
    pub fn test_case<E, C>(&self, mc: &TopSQLCollector, exec_fn: E, check_fn: C)
    where
        E: Fn() + Send + 'static,
        C: FnOnce(),
    {
        // 后台线程循环执行 SQL，直到 cancel 置位。
        let cancel = Arc::new(AtomicI64::new(0));
        let worker_cancel = Arc::clone(&cancel);
        let handle = thread::spawn(move || {
            while worker_cancel.load(Ordering::SeqCst) == 0 {
                exec_fn();
            }
        });

        // 前台跑断言；完成后发取消信号并重置 collector。
        check_fn();
        cancel.store(1, Ordering::SeqCst);
        handle.join().expect("exec worker should join");
        mc.Reset();
    }

    /// loop_exec 对应 Go 的 loopExec：持续执行传入函数直到取消信号出现。
    pub fn loop_exec<F>(&self, cancel: &AtomicI64, f: F)
    where
        F: Fn(),
    {
        while cancel.load(Ordering::SeqCst) == 0 {
            f();
        }
    }
}
