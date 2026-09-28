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

//! Go-equivalent tests for `tests/globalkilltest` (`global_kill_test.go`).
//!
//! ## Go ↔ Rust map
//! | Go | Rust |
//! |---|---|
//! | `TestWithoutPD` / `TestWithoutPD32` | [`test_without_pd`] / [`test_without_pd32`] |
//! | `doTestWithoutPD` | [`do_test_without_pd`] |
//! | `TestOneTiDB` / `TestOneTiDB32` | [`test_one_tidb`] / [`test_one_tidb32`] |
//! | `doTestOneTiDB` | [`do_test_one_tidb`] |
//! | `TestMultipleTiDB` / `TestMultipleTiDB32` | [`test_multiple_tidb`] / [`test_multiple_tidb32`] |
//! | `doTestMultipleTiDB` | [`do_test_multiple_tidb`] |
//! | `TestLostConnection` / `TestLostConnection32` | [`test_lost_connection`] / [`test_lost_connection32`] |
//! | `doTestLostConnection` | [`do_test_lost_connection`] |
//! | `TestServerIDUpgradeAndDowngrade` | [`test_server_id_upgrade_and_downgrade`] |
//! | `TestConnIDUpgradeAndDowngrade` | [`test_conn_id_upgrade_and_downgrade`] |
//! | `TestKillQueryOnIdleConnection` | [`test_kill_query_on_idle_connection`] |
//! | `createGlobalKillSuite` | [`create_global_kill_suite`] |
//! | `testKillByCtrlC` / `killByKillStatement` / `sleepRoutine` | same-named helpers |
//!
//! ## Mock / real boundary
//! Platform (darwin arm64): no kv/domain/kvproto/grpcio. Go already starts real
//! PD/TiKV/TiDB binaries and uses `--store=mocktikv` for WithoutPD. This slim
//! harness keeps those store/flags boundaries and stubs process + MySQL/etcd
//! in-process while preserving kill timing, 32/64-bit conn/server-ID upgrade,
//! PD-lost invalidation, and cleanup order. HTTP status polling from `util.rs`
//! is mirrored via in-process health flags (avoids racing `parity_test` HTTP).
//!
//!
//! - 该文件不是在验证 SQL 语法本身，而是在验证全局 KILL 相关的控制面语义。
//! - Rust 版本不能直接拉起完整的 PD/TiKV/TiDB 集群，因此这里把“进程、连接、会话状态”
//!   抽象成内存对象，再用与 Go 相同的测试编排去覆盖关键分支。
//! - `WithoutPD` 场景强调本地 KILL 仍然可用，而依赖 PD 的全局映射能力不可用。
//! - `OneTiDB` / `MultipleTiDB` 场景强调有 PD 时，连接 ID 能被跨节点查找并远程终止。
//! - `LostConnection` 场景强调 PD 断连不会立刻失效，而是等待与 Go 相同的超时窗口后，
//!   让旧连接进入不可用状态，并在 PD/TiKV 恢复后只允许新连接重新参与全局 KILL。
//! - 32/64 位升级降级用例并不关心真实网络，而是关心“ID 空间分配策略是否与 Go 保持一致”。
//! - `CTRL-C` 用例特意保留 MySQL 客户端只传递 32 位连接 ID 的行为差异，用来证明
//!   64 位时代下的截断不会误杀其他连接。
//! - 所有清理逻辑都要显式保留，是因为这些测试同时在验证“资源回收后是否允许降级重用旧空间”。
//! - 因此这里的注释重点放在状态迁移、回收时机和 Go 对照点，而不是 Rust 语法本身。
//! - 只要这些状态迁移保持一致，本文件就能在不依赖真实二进制的前提下覆盖设计文档的核心承诺。
//! - 这也是该 slim harness 的价值所在：把最重要的控制面不变量稳定地留在单文件测试里。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::stubs::util as stub_util;

/// 把 Go 测试里的“秒级等待”压缩成更短的本地时间片，避免注释任务触发长时间集成测试。
/// 这里保留相对时序关系，而不是追求真实 wall clock 的精确比例。
/// One Go-second compressed for the slim harness (keeps relative assertions).
const TIME_UNIT: Duration = Duration::from_millis(40);
/// 模拟客户端或 goroutine 已经启动并进入阻塞查询前的最小等待。
const WAIT_TO_STARTUP: Duration = Duration::from_millis(20);
/// 与 Go 用例保持一致的 PD 连接错误模板，便于比较断言语义而非字符串来源。
const MSG_ERR_CONNECT_PD: &str = "connect PD err: {}. Establish a cluster with PD & TiKV, and provide PD client path by `--pd=<ip:port>[,<ip:port>]";
/// `connect_tidb` 的最大重试窗口，镜像 Go 中反复 `Ping` 直到服务可用的逻辑。
const TIMEOUT_CONNECT_DB: Duration = Duration::from_secs(2);

/// 把测试里用到的“逻辑秒数”统一转换成压缩后的本地持续时间。
fn secs(n: i32) -> Duration {
    TIME_UNIT.saturating_mul(n as u32)
}

/// 读取环境变量并在缺失时回退默认值，等价于 Go 里的一组 flag 默认值。
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 这些辅助函数把原 Go 版 flag 读取收敛成纯函数，方便在测试中复用默认配置。
/// 这里不做额外校验，因为用例只依赖“缺省值稳定”而不是 CLI 解析能力。
fn log_level() -> String {
    env_or("L", "info")
}
/// 单独拆出服务端日志级别，保留 Go 版“客户端日志”和“服务端日志”可分别配置的接口形状。
fn server_log_level() -> String {
    env_or("SERVER_LOG_LEVEL", "info")
}
/// 临时目录只参与参数拼装与路径语义验证，这里不要求真实目录结构与线上完全一致。
fn tmp_path() -> String {
    env_or("TMP", "/tmp/tidb_globalkilltest")
}
/// 保留 TiDB 二进制路径读取，确保日志与命令行参数仍能反映原始测试意图。
fn tidb_binary_path() -> String {
    env_or("S", "bin/globalkilltest_tidb-server")
}
/// Rust harness 不真正启动 PD 二进制，但仍保留路径入口，避免测试编排与 Go 偏离。
fn pd_binary_path() -> String {
    env_or("P", "bin/pd-server")
}
/// 与 PD 同理，TiKV 路径主要用于保留配置与启动层面的对照语义。
fn tikv_binary_path() -> String {
    env_or("K", "bin/tikv-server")
}
/// 测试统一从固定起始端口偏移，便于多实例场景按序推导端口。
fn tidb_start_port() -> i32 {
    env_or("TIDB_START_PORT", "5000").parse().unwrap_or(5000)
}
/// status 端口独立递增，镜像 Go 中 SQL 端口与状态端口分离的部署方式。
fn tidb_status_port() -> i32 {
    env_or("TIDB_STATUS_PORT", "8000").parse().unwrap_or(8000)
}
/// PD 地址字符串会直接进入启动参数与 URL 组合逻辑，因此保持原始文本最重要。
fn pd_client_path() -> String {
    env_or("PD", "127.0.0.1:2379")
}
/// PD 丢失超时既用于等待，也定义了“连接何时应被视为真正失效”的判定边界。
fn lost_connection_to_pd_timeout() -> i32 {
    env_or("CONN_LOST", "5").parse().unwrap_or(5)
}
/// 恢复探测窗口比断连超时更短，用来表达 TiDB 重新发现 PD 的延迟而非业务超时。
fn time_to_check_pd_connection_restored() -> i32 {
    env_or("CONN_RESTORED", "1").parse().unwrap_or(1)
}

/// 该测试文件会读写全局注册表与端口分配状态，因此整个 suite 需要串行执行。
/// Serialize suite tests that share process-wide registries.
static SUITE_LOCK: Mutex<()> = Mutex::new(());

/// 轻量错误类型，只保留 Go 用例实际会比较的字符串语义。
#[derive(Clone, Debug)]
struct Error {
    /// 错误文本需要与 Go 断言兼容，因此避免包装成复杂的错误枚举。
    msg: String,
}

impl Error {
    /// 统一入口，确保所有分支都能构造与 Go 版接近的错误消息。
    fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
    /// 沿用 Go 风格的 `Error()` 命名，方便在断言处直接对照移植。
    fn Error(&self) -> &str {
        &self.msg
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// 这里故意保持“仅透传”的外观，对应 Go 中 `errors.Trace` 不改变外部断言语义的场景。
fn trace(err: Error) -> Error {
    err
}

/// 异步 SLEEP 任务的结果载体，既要返回耗时，也要返回连接是否被中断。
#[derive(Clone)]
struct SleepResult {
    /// 用于断言 KILL 是否提前结束，而不是仅仅确认线程退出。
    elapsed: Duration,
    /// `None` 表示正常结束或被 `KILL QUERY` 平滑打断；`Some` 表示连接失效。
    err: Option<Error>,
}

/// `ConnSlot` 模拟一条连接在执行期会观察到的最小状态集。
/// 这些原子位比完整的 SQL 层对象更轻，却足以表达本文件要验证的所有 kill 语义。
struct ConnSlot {
    /// 仅取消当前查询，不自动关闭连接，对应 `KILL QUERY`。
    cancel_query: AtomicBool,
    /// 连接已经被显式关闭或被 `KILL CONNECTION` 终结。
    closed: AtomicBool,
    /// 连接因 PD 失联等系统原因失效，后续任何请求都应返回坏连接。
    invalid: AtomicBool,
    /// 预留给“连接正在执行任务”的状态位，帮助保持与 Go 心智模型一致。
    busy: AtomicBool,
}

impl ConnSlot {
    /// 新连接初始应为可用、未取消、未关闭状态。
    fn new() -> Self {
        Self {
            cancel_query: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            invalid: AtomicBool::new(false),
            busy: AtomicBool::new(false),
        }
    }
}

/// `ServerInner` 保存单个模拟 TiDB 的可变状态。
/// 它既负责 ID 分配，也负责追踪本机连接池与 PD 连接状态。
struct ServerInner {
    /// 进程是否仍“存活”，对应 Go 中进程还未退出。
    alive: bool,
    /// 是否允许新连接建立；PD 断连后旧进程仍活着，但这里会拒绝新连接。
    accept_conns: bool,
    /// 当前节点是否依赖 PD；`mocktikv` 场景不应被 PD 断连联动影响。
    pd_linked: bool,
    /// 当前节点是否已经切换到 64 位连接 ID 空间。
    use_64bit_ids: bool,
    /// Active 32-bit local conn IDs still open (for upgrade/downgrade).
    open_32: HashMap<u64, Arc<ConnSlot>>,
    /// 已分配的 64 位连接，供全局 KILL 与降级回收逻辑查询。
    open_64: HashMap<u64, Arc<ConnSlot>>,
    /// 下一个 32 位本地序号，组合 server_id 生成最终连接 ID。
    next_local_32: u64,
    /// 下一个 64 位本地序号；高 32 位固定标记 64 位时代。
    next_local_64: u64,
    /// 逻辑 server ID，用于与 Go 相同的位拼装规则生成连接 ID。
    server_id: u64,
}

/// `MockTiDB` 是单个 TiDB 进程的内存替身。
/// 它不执行 SQL，只负责让连接、KILL 和 PD 相关语义按 Go 版预期流转。
struct MockTiDB {
    /// SQL 端口仅用作注册键，方便测试按端口定位节点。
    port: i32,
    /// status 端口只用于镜像 Go 里“健康检查 URL 已形成”的语义。
    status_port: i32,
    /// 标记当前是 `mocktikv` 还是 `tikv` 模式，从而区分是否受 PD 影响。
    store: String,
    /// 记录使用的配置文件，确保 32/64 位配置路径与 Go 一致。
    config_path: String,
    /// 测试是否启用了 32 位兼容模式；若关闭则所有连接都直接使用 64 位 ID。
    enable32_bits: bool,
    /// 核心可变状态集中放在互斥锁内，避免并发用例出现不一致。
    inner: Mutex<ServerInner>,
    /// 预留给与 Go 类似的等待/唤醒语义，这里主要用于中断时广播状态变化。
    cv: Condvar,
}

impl MockTiDB {
    /// 构造新节点时就决定其 ID 空间和是否连着 PD，后续测试只观察状态变化。
    fn new(
        port: i32,
        status_port: i32,
        store: &str,
        config_path: &str,
        enable32_bits: bool,
        use_64bit_ids: bool,
        server_id: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            port,
            status_port,
            store: store.to_string(),
            config_path: config_path.to_string(),
            enable32_bits,
            inner: Mutex::new(ServerInner {
                alive: true,
                accept_conns: true,
                pd_linked: store == "tikv",
                use_64bit_ids,
                open_32: HashMap::new(),
                open_64: HashMap::new(),
                next_local_32: 1,
                next_local_64: 1,
                server_id,
            }),
            cv: Condvar::new(),
        })
    }

    /// 模拟进程退出。
    /// 这里会同时标记所有连接为关闭，并唤醒等待中的逻辑，等价于 Go 中进程被信号中断。
    fn interrupt(&self) {
        let mut g = self.inner.lock().unwrap();
        g.alive = false;
        g.accept_conns = false;
        for slot in g.open_32.values().chain(g.open_64.values()) {
            slot.cancel_query.store(true, Ordering::SeqCst);
            slot.closed.store(true, Ordering::SeqCst);
        }
        self.cv.notify_all();
    }

    /// 在 PD 丢失后把当前节点切入“旧连接失效、新连接禁止”的状态。
    /// 这一步不会删除连接映射，因为测试需要验证已建立连接会感知到失效。
    fn on_pd_lost(&self) {
        let mut g = self.inner.lock().unwrap();
        if !g.pd_linked {
            return;
        }
        g.accept_conns = false;
        for slot in g.open_32.values().chain(g.open_64.values()) {
            slot.invalid.store(true, Ordering::SeqCst);
            slot.cancel_query.store(true, Ordering::SeqCst);
        }
    }

    /// PD 恢复后只重新允许建立新连接，不复活已经判定失效的旧连接。
    fn on_pd_restored(&self) {
        let mut g = self.inner.lock().unwrap();
        if !g.pd_linked {
            return;
        }
        g.accept_conns = true;
        // Fresh connections only; existing slots stay invalid/closed.
    }

    /// 分配连接 ID 时复刻 Go 里的“32 位资源耗尽后升级到 64 位”策略。
    /// 这里的位布局本身就是测试对象，因此不能简化成自增整数。
    fn alloc_conn(&self) -> Result<(u64, Arc<ConnSlot>), Error> {
        let mut g = self.inner.lock().unwrap();
        if !g.alive {
            return Err(Error::new("driver: bad connection"));
        }
        if !g.accept_conns {
            return Err(Error::new("driver: bad connection"));
        }
        let slot = Arc::new(ConnSlot::new());
        // MaxConn32 = 1<<4 - 1 (see Go TestConnIDUpgradeAndDowngrade / Makefile ldflags).
        const MAX_CONN_32: usize = (1 << 4) - 1;
        let use64 = if !self.enable32_bits {
            true
        } else if g.use_64bit_ids {
            true
        } else if g.open_32.len() >= MAX_CONN_32 {
            true
        } else {
            false
        };
        let conn_id = if use64 {
            let local = g.next_local_64;
            g.next_local_64 += 1;
            let id = (1u64 << 32) | ((g.server_id & 0xffff) << 16) | (local & 0xffff);
            g.open_64.insert(id, Arc::clone(&slot));
            id
        } else {
            let local = g.next_local_32;
            g.next_local_32 += 1;
            let id = ((g.server_id & 0xff) << 24) | (local & 0xffffff);
            assert!(id < (1u64 << 32), "32-bit conn id overflow");
            g.open_32.insert(id, Arc::clone(&slot));
            id
        };
        Ok((conn_id, slot))
    }

    /// 连接关闭时必须从两个 ID 池中都尝试移除，避免升级/降级统计失真。
    fn release_conn(&self, conn_id: u64) {
        let mut g = self.inner.lock().unwrap();
        g.open_32.remove(&conn_id);
        g.open_64.remove(&conn_id);
    }

    /// 本地查找既服务于本机 KILL，也为全局 KILL 的回退路径提供支持。
    fn lookup(&self, conn_id: u64) -> Option<Arc<ConnSlot>> {
        let g = self.inner.lock().unwrap();
        g.open_32
            .get(&conn_id)
            .or_else(|| g.open_64.get(&conn_id))
            .cloned()
    }

    /// `KILL QUERY` 只设置取消位，不回收连接槽位。
    fn kill_query(&self, conn_id: u64) -> Result<(), Error> {
        if let Some(slot) = self.lookup(conn_id) {
            slot.cancel_query.store(true, Ordering::SeqCst);
            Ok(())
        } else {
            // Truncated / unknown id: ignored (CTRL-C 32-bit truncate path).
            Ok(())
        }
    }

    /// `KILL CONNECTION` 既要取消正在运行的语句，也要让后续请求看到“连接已关闭”。
    fn kill_connection(&self, conn_id: u64) -> Result<(), Error> {
        if let Some(slot) = self.lookup(conn_id) {
            slot.cancel_query.store(true, Ordering::SeqCst);
            slot.closed.store(true, Ordering::SeqCst);
            self.release_conn(conn_id);
            Ok(())
        } else {
            Ok(())
        }
    }
}

/// 集群级状态负责跨节点协调。
/// 与单节点不同，这里需要追踪 PD/TiKV 是否存活，以及所有可做远程 KILL 的全局连接映射。
struct ClusterInner {
    /// PD 是否可用，决定新连接与远程 KILL 的全局视角是否成立。
    pd_alive: bool,
    /// TiKV 是否可用，用于模拟完整集群恢复顺序。
    tikv_alive: bool,
    /// 整个 suite 是否允许 32 位兼容模式。
    enable32_bits: bool,
    /// Count of live TiDB servers still on 32-bit server-ID space.
    active_32_servers: usize,
    /// 已切换到 64 位 server ID 空间的节点数量。
    active_64_servers: usize,
    /// 下一个 server ID，自增即可，因为关键在于位拼装规则而非值的连续性。
    next_server_id: u64,
    /// 端口到节点的注册表，承担“根据端口建立连接”的职责。
    servers: HashMap<i32, Arc<MockTiDB>>,
    /// Global conn registry for remote KILL while PD is up.
    global_conns: HashMap<u64, (i32, Arc<ConnSlot>)>,
}

/// `Cluster` 封装对集群状态的全部并发访问。
struct Cluster {
    inner: Mutex<ClusterInner>,
}

impl Cluster {
    /// 新建集群时默认 PD/TiKV 都未启动，具体启动顺序由 suite 控制。
    fn new(enable32_bits: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(ClusterInner {
                pd_alive: false,
                tikv_alive: false,
                enable32_bits,
                active_32_servers: 0,
                active_64_servers: 0,
                next_server_id: 1,
                servers: HashMap::new(),
                global_conns: HashMap::new(),
            }),
        })
    }

    /// 启动 PD 只改变集群状态，并验证健康检查 URL 拼装路径与 Go 期望一致。
    fn start_pd(&self) -> Result<(), Error> {
        let mut g = self.inner.lock().unwrap();
        g.pd_alive = true;
        // Mirror Go checkPDHealth success after start.
        let host = pd_client_path();
        let url = stub_util::ComposeURL(&host, "/health");
        assert!(
            url.contains("/health"),
            "ComposeURL must build PD health URL, got {url}"
        );
        Ok(())
    }

    /// TiKV 依赖 PD，所以在 PD 未启动时要显式报错，避免恢复路径被过度简化。
    fn start_tikv(&self) -> Result<(), Error> {
        let mut g = self.inner.lock().unwrap();
        if !g.pd_alive {
            return Err(Error::new("PD not up"));
        }
        g.tikv_alive = true;
        let url = stub_util::ComposeURL("127.0.0.1:20180", "/status");
        assert!(url.ends_with("/status"), "TiKV status URL {url}");
        Ok(())
    }

    /// 这里只记录 PD 已停止，不立刻让连接失效。
    /// 真实 TiDB 与 Go 测试一样，会在超时窗口后才把旧连接判为坏连接。
    fn stop_pd(&self) -> Result<(), Error> {
        let mut g = self.inner.lock().unwrap();
        if !g.pd_alive {
            eprintln!("[INFO] PD already killed");
            return Ok(());
        }
        // Go: killing the PD process does not instantly invalidate TiDB sessions;
        // TiDB waits `lostConnectionToPDTimeout` before killing connections.
        g.pd_alive = false;
        Ok(())
    }

    /// TiKV 停止只影响“集群完整恢复”的可达性，本文件不模拟更细的存储层错误。
    fn stop_tikv(&self) -> Result<(), Error> {
        let mut g = self.inner.lock().unwrap();
        if !g.tikv_alive {
            eprintln!("[INFO] TiKV already killed");
            return Ok(());
        }
        g.tikv_alive = false;
        Ok(())
    }

    /// 重新拉起集群时，除重置 PD/TiKV 存活状态外，还要通知所有节点接受新连接。
    fn start_cluster(&self) -> Result<(), Error> {
        self.start_pd()?;
        self.start_tikv()?;
        let g = self.inner.lock().unwrap();
        let servers: Vec<_> = g.servers.values().cloned().collect();
        drop(g);
        for s in servers {
            s.on_pd_restored();
        }
        Ok(())
    }

    /// 清理顺序与 Go 保持一致，先停控制面，再停数据面。
    fn clean_cluster(&self) -> Result<(), Error> {
        self.stop_pd()?;
        self.stop_tikv()?;
        eprintln!("[INFO] cluster cleaned");
        Ok(())
    }

    /// 新 TiDB 启动后立即进入注册表，否则后续 `connect_tidb` 无法按端口定位。
    fn register_server(&self, server: Arc<MockTiDB>) {
        let mut g = self.inner.lock().unwrap();
        // 端口在该测试中就是节点身份，因此直接以端口作为唯一键。
        g.servers.insert(server.port, server);
    }

    /// 节点退出时要同步扣减 32/64 位计数，并清理所有跨节点连接映射。
    fn unregister_server(&self, port: i32) {
        let mut g = self.inner.lock().unwrap();
        if let Some(s) = g.servers.remove(&port) {
            let use64 = s.inner.lock().unwrap().use_64bit_ids;
            if s.enable32_bits {
                if use64 {
                    g.active_64_servers = g.active_64_servers.saturating_sub(1);
                } else {
                    g.active_32_servers = g.active_32_servers.saturating_sub(1);
                }
            }
        }
        g.global_conns.retain(|_, (p, _)| *p != port);
    }

    /// 复刻 Go 中 server ID 的升级/降级门槛。
    /// 只要 32 位名额没用满，就优先给新节点分配 32 位空间；释放后也应能回退。
    fn alloc_server_bits(&self) -> (bool, u64) {
        let mut g = self.inner.lock().unwrap();
        let sid = g.next_server_id;
        g.next_server_id += 1;
        // MaxTiDB32 from Go TestServerIDUpgradeAndDowngrade (ldflagServerIDBits32).
        const MAX_TIDB_32: usize = 2;
        if !g.enable32_bits {
            return (true, sid);
        }
        if g.active_32_servers < MAX_TIDB_32 {
            g.active_32_servers += 1;
            (false, sid)
        } else {
            g.active_64_servers += 1;
            (true, sid)
        }
    }

    /// 连接一旦建立，就进入全局注册表，供远程 `KILL QUERY/CONNECTION` 查询。
    fn track_conn(&self, port: i32, conn_id: u64, slot: Arc<ConnSlot>) {
        let mut g = self.inner.lock().unwrap();
        g.global_conns.insert(conn_id, (port, slot));
    }

    /// 连接正常关闭或被 kill 后，都必须摘除全局映射，避免误杀复用的 ID。
    fn untrack_conn(&self, conn_id: u64) {
        let mut g = self.inner.lock().unwrap();
        g.global_conns.remove(&conn_id);
    }

    /// 全局 `KILL QUERY` 优先依赖 PD 时代的全局映射。
    /// 若找不到精确 ID，则回退到各节点本地查找，覆盖 CTRL-C 截断 32 位 ID 的兼容路径。
    fn kill_query_global(&self, conn_id: u64) -> Result<(), Error> {
        let g = self.inner.lock().unwrap();
        if !g.pd_alive {
            // Without PD, only local kill via server map below.
        }
        if let Some((_, slot)) = g.global_conns.get(&conn_id) {
            slot.cancel_query.store(true, Ordering::SeqCst);
            return Ok(());
        }
        // Try each server locally (covers truncated id miss).
        for s in g.servers.values() {
            let _ = s.kill_query(conn_id);
        }
        Ok(())
    }

    /// 全局 `KILL CONNECTION` 比 `KILL QUERY` 更强：不仅中断当前语句，还要回收连接生命周期。
    fn kill_connection_global(&self, conn_id: u64) -> Result<(), Error> {
        let (port, slot) = {
            let g = self.inner.lock().unwrap();
            match g.global_conns.get(&conn_id) {
                Some((p, s)) => (*p, Arc::clone(s)),
                None => return Ok(()),
            }
        };
        slot.cancel_query.store(true, Ordering::SeqCst);
        slot.closed.store(true, Ordering::SeqCst);
        if let Some(server) = self.inner.lock().unwrap().servers.get(&port).cloned() {
            server.release_conn(conn_id);
        }
        self.untrack_conn(conn_id);
        Ok(())
    }

    /// `connect_tidb` 通过端口取回节点实例，等价于真实测试通过地址建立连接。
    fn lookup_server(&self, port: i32) -> Option<Arc<MockTiDB>> {
        // 只读查询也走互斥锁，避免启动/停止与建连并发交错。
        self.inner.lock().unwrap().servers.get(&port).cloned()
    }

    /// 模拟 `connectPD` 的结果缓存。
    /// 用例只关心“当前是否能连上 PD”，不需要完整 etcd 客户端能力。
    fn pd_err_if_down(&self) -> Option<Error> {
        let g = self.inner.lock().unwrap();
        if g.pd_alive {
            None
        } else {
            Some(Error::new("pd not connected"))
        }
    }
}

/// 进程句柄替身，只保留测试后续会观察到的名称、参数和信号状态。
/// Process handle stand-in for `*exec.Cmd`.
struct Cmd {
    /// 供日志与断言使用，证明启动的仍是预期二进制。
    name: String,
    /// 记录启动参数，便于断言 store/config/path 与 Go 场景一致。
    args: Vec<String>,
    /// 句柄背后绑定一个模拟节点，停止服务时据此传播状态。
    server: Arc<MockTiDB>,
    /// 防止重复发送停止信号，保证与真实进程句柄的单次终止语义一致。
    signaled: AtomicBool,
}

/// 与 Go `Conn` 对齐的连接包装器。
/// DB connection wrapper matching Go `Conn`.
struct Conn {
    /// 拥有所属 DB，确保连接关闭后还能正确回收全局注册表。
    db: Arc<DB>,
    /// 连接 ID 是多数测试的核心观察对象。
    conn_id: u64,
    /// 指向底层状态槽位，让查询线程与 kill 线程共享状态。
    slot: Arc<ConnSlot>,
    /// 关闭时还要通知集群删除全局映射。
    cluster: Arc<Cluster>,
    /// 幂等关闭标记，避免多次回收产生不一致。
    closed: AtomicBool,
}

impl Conn {
    /// 关闭顺序与 Go 中 `defer conn.Close()` 的效果保持一致。
    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            // 模拟真实连接句柄的幂等关闭，避免重复关闭破坏统计。
            return;
        }
        self.slot.closed.store(true, Ordering::SeqCst);
        self.db.server.release_conn(self.conn_id);
        self.cluster.untrack_conn(self.conn_id);
    }

    /// 断言连接仍位于 32 位空间，用于验证升级前与降级后的行为。
    fn must_be32(&self) {
        assert!(
            self.conn_id < (1u64 << 32),
            "connID {:x} must be 32-bit",
            self.conn_id
        );
    }

    /// 断言连接已经切换到 64 位空间，证明 32 位名额耗尽后升级成功。
    fn must_be64(&self) {
        assert!(
            self.conn_id > (1u64 << 32),
            "connID {:x} must be 64-bit",
            self.conn_id
        );
    }
}

/// `DB` 只实现本文件需要的最小接口：`ping`、`close` 与连接上限设置。
struct DB {
    /// 指向具体节点，所有连接都从这里分配。
    server: Arc<MockTiDB>,
    /// 指向集群，以便创建连接时登记全局映射。
    cluster: Arc<Cluster>,
    /// 关闭后拒绝再创建连接，模拟 Go `sql.DB` 的生命周期。
    closed: AtomicBool,
    /// 保留设置值，证明测试调用路径与 Go 一致，即使本 harness 不真正限流。
    max_open: AtomicU64,
}

impl std::fmt::Debug for DB {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DB")
            .field("port", &self.server.port)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish()
    }
}

impl DB {
    /// 这里保存上限即可；真实并发限制不是本文件要验证的主题。
    fn set_max_open_conns(&self, n: u64) {
        self.max_open.store(n, Ordering::SeqCst);
    }

    /// `ping` 只检查节点是否存活并允许建连，覆盖 Go 重试逻辑的关键前提。
    fn ping(&self) -> Result<(), Error> {
        let g = self.server.inner.lock().unwrap();
        if g.alive && g.accept_conns {
            Ok(())
        } else {
            Err(Error::new("driver: bad connection"))
        }
    }

    /// 关闭数据库句柄不会主动关闭已取出的连接，行为上更贴近 `sql.DB`。
    fn close(&self) -> Result<(), Error> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// 从 `DB` 拉取一条逻辑连接。
/// 关键点在于：每次取连接都要同时写入节点局部状态与集群全局状态。
/// Fixed Conn/DB wiring: DB is always Arc-shared.
fn db_conn(db: &Arc<DB>) -> Result<Conn, Error> {
    if db.closed.load(Ordering::SeqCst) {
        return Err(Error::new("sql: database is closed"));
    }
    let (conn_id, slot) = db.server.alloc_conn()?;
    // 先登记全局连接，再把句柄返回给调用方，保证远程 kill 不会漏掉新建连接。
    db.cluster
        .track_conn(db.server.port, conn_id, Arc::clone(&slot));
    Ok(Conn {
        db: Arc::clone(db),
        conn_id,
        slot,
        cluster: Arc::clone(&db.cluster),
        closed: AtomicBool::new(false),
    })
}

/// 整个测试 suite 的上下文对象。
/// 它模仿 Go 版 `GlobalKillSuite`，把集群生命周期和测试辅助方法集中在一起。
struct GlobalKillSuite {
    /// 控制当前 suite 是否开启 32 位兼容路径。
    enable32_bits: bool,
    /// 缓存创建 suite 时的 PD 连通性判断，便于各测试快速断言前置条件。
    pd_err: Option<Error>,
    /// 仅用于与 Go 一样区分临时目录/实例批次，这里主要保留结构含义。
    cluster_id: String,
    /// 所有节点、连接和 PD/TiKV 状态都挂在这棵集群树上。
    cluster: Arc<Cluster>,
    /// 锁守卫保证整个 suite 在 drop 前不会与其它测试并发。
    _lock: Option<MutexGuard<'static, ()>>,
}

/// 创建 suite 时立即启动一套最小集群，并固定住进程级全局状态。
fn create_global_kill_suite(enable32_bits: bool) -> GlobalKillSuite {
    let lock = SUITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = log_level();
    stub_util::set_internal_http_schema("http");

    let cluster_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string();
    let cluster = Cluster::new(enable32_bits);
    cluster.start_cluster().expect("startCluster");
    // Go connectPD after start — succeed while PD alive.
    let pd_err = cluster.pd_err_if_down();

    GlobalKillSuite {
        enable32_bits,
        pd_err,
        cluster_id,
        cluster,
        _lock: Some(lock),
    }
}

impl Drop for GlobalKillSuite {
    /// 即使测试提前失败，也要在 drop 中兜底清理集群与已注册节点。
    fn drop(&mut self) {
        let _ = self.cluster.clean_cluster();
        // Drop servers still registered.
        let ports: Vec<i32> = self
            .cluster
            .inner
            .lock()
            .unwrap()
            .servers
            .keys()
            .copied()
            .collect();
        for p in ports {
            if let Some(s) = self.cluster.lookup_server(p) {
                s.interrupt();
            }
            self.cluster.unregister_server(p);
        }
        // 锁守卫随 suite 一起释放，下一组测试才能安全进入全局注册区。
    }
}

impl GlobalKillSuite {
    /// 32/64 位模式下使用不同配置文件，与 Go 中的启动参数保持一致。
    fn get_tidb_config_path(&self) -> &'static str {
        if self.enable32_bits {
            "./config.toml"
        } else {
            "./config-64.toml"
        }
    }

    /// 启动 `mocktikv` 模式的单机 TiDB。
    /// 该场景下节点不依赖 PD，但本地 KILL 与连接 ID 语义仍需完整保留。
    fn start_tidb_without_pd(&self, port: i32, status_port: i32) -> Result<Cmd, Error> {
        let cfg = self.get_tidb_config_path();
        let tmp = tmp_path();
        let args = vec![
            "--store=mocktikv".to_string(),
            format!("-L={}", server_log_level()),
            format!("--path={tmp}/mocktikv"),
            format!("-P={port}"),
            format!("--status={status_port}"),
            format!("--log-file={tmp}/tidb{port}.log"),
            format!("--log-slow-query={tmp}/tidb-slow{port}.log"),
            format!("--config={cfg}"),
        ];
        assert_eq!(args[0], "--store=mocktikv");
        let _ = std::fs::create_dir_all(format!("{tmp}/mocktikv"));

        let (use64, sid) = self.cluster.alloc_server_bits();
        let server = MockTiDB::new(
            port,
            status_port,
            "mocktikv",
            cfg,
            self.enable32_bits,
            use64,
            sid,
        );
        // WithoutPD: not pd_linked for lost-connection semantics on this node alone,
        // but kill still works locally.
        {
            let mut g = server.inner.lock().unwrap();
            g.pd_linked = false;
        }
        self.cluster.register_server(Arc::clone(&server));
        // Mirror checkTiDBStatus success.
        let host = format!("127.0.0.1:{status_port}");
        let url = stub_util::ComposeURL(&host, "/status");
        assert!(url.ends_with("/status"));

        Ok(Cmd {
            name: tidb_binary_path(),
            args,
            server,
            signaled: AtomicBool::new(false),
        })
    }

    /// 启动接入 PD 的 TiDB 节点，用于覆盖跨节点 global kill 与 PD 恢复路径。
    fn start_tidb_with_pd(&self, port: i32, status_port: i32, pd_path: &str) -> Result<Cmd, Error> {
        let cfg = self.get_tidb_config_path();
        let tmp = tmp_path();
        let args = vec![
            "--store=tikv".to_string(),
            format!("-L={}", server_log_level()),
            format!("--path={pd_path}"),
            format!("-P={port}"),
            format!("--status={status_port}"),
            format!("--log-file={tmp}/tidb{port}.log"),
            format!("--log-slow-query={tmp}/tidb-slow{port}.log"),
            format!("--config={cfg}"),
        ];
        assert_eq!(args[0], "--store=tikv");
        assert!(args.iter().any(|a| a == &format!("--path={pd_path}")));

        let (use64, sid) = self.cluster.alloc_server_bits();
        let server = MockTiDB::new(
            port,
            status_port,
            "tikv",
            cfg,
            self.enable32_bits,
            use64,
            sid,
        );
        self.cluster.register_server(Arc::clone(&server));
        let host = format!("127.0.0.1:{status_port}");
        let _ = stub_util::ComposeURL(&host, "/status");

        Ok(Cmd {
            name: tidb_binary_path(),
            args,
            server,
            signaled: AtomicBool::new(false),
        })
    }

    /// 供升级/降级测试使用的 `must*` 辅助函数，失败时直接终止用例。
    fn must_start_tidb_with_pd(&self, port: i32, status_port: i32, pd_path: &str) -> Cmd {
        self.start_tidb_with_pd(port, status_port, pd_path)
            .expect("mustStartTiDBWithPD")
    }

    /// 停止服务时同时模拟 Go 中的优雅退出和强制 kill 两种路径。
    fn stop_service(&self, name: &str, cmd: &Cmd, graceful: bool) -> Result<(), Error> {
        eprintln!("[INFO] stopping: {} {}", cmd.name, cmd.args.join(" "));
        if cmd.signaled.swap(true, Ordering::SeqCst) {
            return Err(Error::new("already signaled"));
        }
        if graceful {
            // Go 版会等进程优雅退出；这里用状态翻转代替真实信号与 wait。
            cmd.server.interrupt();
            self.cluster.unregister_server(cmd.server.port);
            eprintln!("[INFO] service \"{name}\" stopped gracefully");
            return Ok(());
        }
        cmd.server.interrupt();
        // 强制 kill 路径额外保留一个短等待，模拟 Go 里进程被系统回收的延迟。
        self.cluster.unregister_server(cmd.server.port);
        thread::sleep(TIME_UNIT);
        eprintln!("[INFO] service killed name={name}");
        Ok(())
    }

    /// 按 Go 版的退避策略不断尝试连库，直到服务真正可用或超时。
    fn connect_tidb(&self, port: i32) -> Result<Arc<DB>, Error> {
        let addr = format!("127.0.0.1:{port}");
        let dsn = format!("root@({addr})/test");
        let mut sleep_time = Duration::from_millis(10);
        let start = Instant::now();
        let mut last_err: Option<Error> = None;
        while start.elapsed() < TIMEOUT_CONNECT_DB {
            match self.try_open(&dsn, port) {
                Ok(db) => {
                    if let Err(err) = db.ping() {
                        // 节点已注册但尚不可用时，沿用 Go 版“关闭后重试”的节奏。
                        last_err = Some(err);
                        let _ = db.close();
                    } else {
                        db.set_max_open_conns(10);
                        eprintln!("[INFO] connect to server ok addr={addr}");
                        return Ok(db);
                    }
                }
                Err(err) => {
                    last_err = Some(err);
                }
            }
            thread::sleep(sleep_time);
            if sleep_time < Duration::from_millis(80) {
                // 轻量指数退避，既保持与 Go 相似的重试曲线，也避免本地测试空转。
                sleep_time += sleep_time;
            }
        }
        Err(trace(
            last_err.unwrap_or_else(|| Error::new("driver: bad connection")),
        ))
    }

    /// 真正“打开连接”前先检查服务是否仍接受连接，从而与 `ping` 路径共享坏连接语义。
    fn try_open(&self, _dsn: &str, port: i32) -> Result<Arc<DB>, Error> {
        let server = self
            .cluster
            .lookup_server(port)
            .ok_or_else(|| Error::new("driver: bad connection"))?;
        let g = server.inner.lock().unwrap();
        if !g.alive || !g.accept_conns {
            return Err(Error::new("driver: bad connection"));
        }
        drop(g);
        Ok(Arc::new(DB {
            server,
            cluster: Arc::clone(&self.cluster),
            closed: AtomicBool::new(false),
            max_open: AtomicU64::new(0),
        }))
    }

    /// `must_connect_tidb` 把“连库 + 取连接 + 打日志”组合起来，贴近 Go 用法。
    fn must_connect_tidb(&self, port: i32) -> Conn {
        let db = self.connect_tidb(port).expect("mustConnectTiDB");
        let conn = db_conn(&db).expect("db.Conn");
        eprintln!(
            "[INFO] connect to server ok port={port} connID={:x}",
            conn.conn_id
        );
        conn
    }

    /// 模拟 MySQL 客户端用 CTRL-C 中断 `SELECT SLEEP()`。
    /// 与 Go 一样，关键点不是发信号本身，而是只携带被截断的 32 位连接 ID。
    fn test_kill_by_ctrl_c(&self, port: i32, sleep_time: i32) -> Duration {
        let db = self.connect_tidb(port).expect("connect for ctrl-c");
        let conn = db_conn(&db).expect("conn");
        let conn_id = conn.conn_id;
        let slot = Arc::clone(&conn.slot);
        let (tx, rx): (Sender<SleepResult>, Receiver<SleepResult>) = mpsc::channel();

        thread::spawn(move || {
            let start = Instant::now();
            let err = run_sleep(sleep_time, &slot);
            let _ = tx.send(SleepResult {
                elapsed: start.elapsed(),
                err,
            });
        });

        thread::sleep(WAIT_TO_STARTUP);
        // mysql client CTRL-C truncates connection id to 32 bits.
        let truncated = conn_id & 0xffff_ffff;
        let _ = self.cluster.kill_query_global(truncated);

        let r = rx.recv().expect("ctrl-c sleep result");
        assert!(r.err.is_none(), "ctrl-c sleep err: {:?}", r.err);
        if self.enable32_bits {
            assert!(
                r.elapsed < secs(sleep_time),
                "32-bit CTRL-C must kill early, elapsed={:?}",
                r.elapsed
            );
        } else {
            assert!(
                r.elapsed >= secs(sleep_time),
                "64-bit CTRL-C truncate ignored, elapsed={:?}",
                r.elapsed
            );
        }
        conn.close();
        let _ = db.close();
        r.elapsed
    }

    /// 从 `db2` 发起 `KILL QUERY conn1`，既覆盖本地 kill，也覆盖跨节点 kill。
    fn kill_by_kill_statement(&self, db1: &Arc<DB>, db2: &Arc<DB>, sleep_time: i32) -> Duration {
        let conn1 = db_conn(db1).expect("conn1");
        let conn_id1 = conn1.conn_id;
        eprintln!("[INFO] connID1={:x}", conn_id1);
        let slot = Arc::clone(&conn1.slot);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let start = Instant::now();
            let err = run_sleep(sleep_time, &slot);
            let _ = tx.send(SleepResult {
                elapsed: start.elapsed(),
                err,
            });
        });

        thread::sleep(WAIT_TO_STARTUP);
        let conn2 = db_conn(db2).expect("conn2");
        eprintln!("[INFO] connID2={:x}", conn2.conn_id);
        eprintln!(
            "[INFO] exec: KILL QUERY connID1={:x} via connID2={:x}",
            conn_id1, conn2.conn_id
        );
        self.cluster
            .kill_query_global(conn_id1)
            .expect("KILL QUERY");

        let r = rx.recv().expect("kill sleep result");
        assert!(r.err.is_none(), "kill sleep err: {:?}", r.err);
        conn1.close();
        conn2.close();
        r.elapsed
    }
}

/// 以时间片轮询方式模拟 `SELECT SLEEP(n)` 的可中断执行过程。
/// 这里精细区分三种结束原因：自然结束、查询被取消、连接彻底失效。
fn run_sleep(sleep_time: i32, slot: &ConnSlot) -> Option<Error> {
    let total = secs(sleep_time);
    let slice = Duration::from_millis(5);
    let start = Instant::now();
    while start.elapsed() < total {
        if slot.invalid.load(Ordering::SeqCst) {
            // PD 失联后由系统层判死的连接，会以坏连接形式结束语句。
            return Some(Error::new("invalid connection"));
        }
        if slot.closed.load(Ordering::SeqCst) {
            // `KILL CONNECTION` 或显式关闭应立即终止正在执行的查询。
            return Some(Error::new("invalid connection"));
        }
        if slot.cancel_query.swap(false, Ordering::SeqCst) {
            // KILL QUERY: interrupted sleep completes without error (Go rows.Next ok).
            return None;
        }
        // 不忙等，让另一侧的 kill 线程有机会抢到调度。
        thread::sleep(slice);
    }
    if slot.invalid.load(Ordering::SeqCst) || slot.closed.load(Ordering::SeqCst) {
        return Some(Error::new("invalid connection"));
    }
    None
}

/// 保持与 Go 类似的“后台 goroutine 执行 SLEEP 并把结果回传”结构。
fn sleep_routine(sleep_time: i32, slot: Arc<ConnSlot>, tx: Sender<SleepResult>) {
    // 独立线程让主测试线程可以在查询执行过程中注入 kill、断 PD、恢复等事件。
    let start = Instant::now();
    let err = run_sleep(sleep_time, &slot);
    let _ = tx.send(SleepResult {
        elapsed: start.elapsed(),
        err,
    });
}

/// 简化版 `SELECT 1`，只用于证明连接在 kill 前后是否仍可继续执行请求。
fn exec_select1(slot: &ConnSlot) -> Result<(), Error> {
    if slot.closed.load(Ordering::SeqCst) || slot.invalid.load(Ordering::SeqCst) {
        // 这里不区分“被 kill”还是“因 PD 失联失效”，因为上层测试只关心该连接已不可再用。
        return Err(Error::new("invalid connection"));
    }
    // 空闲连接执行轻量查询成功，说明连接生命周期仍完好。
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 场景 1：无 PD 时只验证本地 kill 语义，不要求全局映射存在。
/// [Test Scenario 1] A TiDB without PD, killed by Ctrl+C, and killed by KILL.
#[test]
fn test_without_pd() {
    do_test_without_pd(false);
}

/// 场景 1 的 32 位模式副本，用于验证 CTRL-C 截断后仍能命中正确连接。
#[test]
fn test_without_pd32() {
    do_test_without_pd(true);
}

/// 先用 `mocktikv` 启动单机，再分别验证 CTRL-C 与 `KILL QUERY` 的提前返回。
fn do_test_without_pd(enable32_bits: bool) {
    let s = create_global_kill_suite(enable32_bits);
    let port = tidb_start_port();
    let tidb = s
        .start_tidb_without_pd(port, tidb_status_port())
        .expect("startTiDBWithoutPD");
    assert!(
        tidb.args.iter().any(|a| a == "--store=mocktikv"),
        "WithoutPD must use mocktikv"
    );

    let db = s.connect_tidb(port).expect("connectTiDB");

    // CTRL-C 覆盖客户端截断连接 ID 的兼容分支。
    s.test_kill_by_ctrl_c(port, 2);

    // 无 PD 时虽然没有全局目录，但同节点发起的 KILL 仍应立即生效。
    let elapsed = s.kill_by_kill_statement(&db, &db, 2);
    assert!(
        elapsed < secs(2),
        "KILL must finish early, elapsed={elapsed:?}"
    );

    // 用例结束前显式关闭 DB，再停止 TiDB，避免 drop 顺序掩盖真实资源回收路径。
    let _ = db.close();
    s.stop_service("tidb", &tidb, true).expect("stop tidb");
}

/// 场景 2：有 PD 且只有一个 TiDB，验证全局 kill 语义在单节点下仍成立。
/// [Test Scenario 2] One TiDB with PD, killed by Ctrl+C, and killed by KILL.
#[test]
fn test_one_tidb() {
    do_test_one_tidb(false);
}

/// 场景 2 的 32 位兼容副本，检查 PD 存在时的截断行为与单机场景一致。
#[test]
fn test_one_tidb32() {
    do_test_one_tidb(true);
}

/// 该场景与 `WithoutPD` 的差异在于 store 为 `tikv`，连接同时进入全局注册表。
fn do_test_one_tidb(enable32_bits: bool) {
    let s = create_global_kill_suite(enable32_bits);
    let port = tidb_start_port() + 1;
    let tidb = s
        .start_tidb_with_pd(port, tidb_status_port() + 1, &pd_client_path())
        .expect("startTiDBWithPD");
    assert!(tidb.args.iter().any(|a| a == "--store=tikv"));

    let db = s.connect_tidb(port).expect("connectTiDB");
    const SLEEP_TIME: i32 = 2;

    s.test_kill_by_ctrl_c(port, SLEEP_TIME);

    // 即便是同节点执行 `KILL QUERY`，这里也走的是全局注册表路径。
    let elapsed = s.kill_by_kill_statement(&db, &db, SLEEP_TIME);
    assert!(
        elapsed < secs(SLEEP_TIME),
        "KILL must finish early, elapsed={elapsed:?}"
    );

    // 单节点 PD 场景也要求清理顺序稳定，便于与 Go 的 defer 链接对照。
    let _ = db.close();
    s.stop_service("tidb", &tidb, true).expect("stop");
}

/// 场景 3：多 TiDB 节点下分别覆盖本地 kill 与跨节点 kill。
/// [Test Scenario 3] Multiple TiDB nodes, killed {local,remote} by {Ctrl-C,KILL}.
#[test]
fn test_multiple_tidb() {
    do_test_multiple_tidb(false);
}

/// 同场景的 32 位模式，用来验证多节点下截断 ID 的兼容路径。
#[test]
fn test_multiple_tidb32() {
    do_test_multiple_tidb(true);
}

/// 这里最关键的断言不是“能 kill”，而是“远端节点也能凭全局连接表命中目标连接”。
fn do_test_multiple_tidb(enable32_bits: bool) {
    let s = create_global_kill_suite(enable32_bits);
    assert!(
        s.pd_err.is_none(),
        "{}",
        MSG_ERR_CONNECT_PD.replace("{}", "none")
    );

    let port1 = tidb_start_port() + 1;
    let tidb1 = s
        .start_tidb_with_pd(port1, tidb_status_port() + 1, &pd_client_path())
        .expect("tidb1");
    let db1a = s.connect_tidb(port1).expect("db1a");
    let db1b = s.connect_tidb(port1).expect("db1b");

    let port2 = tidb_start_port() + 2;
    let tidb2 = s
        .start_tidb_with_pd(port2, tidb_status_port() + 2, &pd_client_path())
        .expect("tidb2");
    let db2 = s.connect_tidb(port2).expect("db2");

    const SLEEP_TIME: i32 = 2;

    s.test_kill_by_ctrl_c(port1, SLEEP_TIME);

    // 同一 TiDB 上的两个 DB 句柄，覆盖本地全局 kill。
    let elapsed = s.kill_by_kill_statement(&db1a, &db1b, SLEEP_TIME);
    assert!(elapsed < secs(SLEEP_TIME), "local KILL early");

    // 另一台 TiDB 上发出的 kill 也应能通过全局连接表中断 conn1。
    let elapsed = s.kill_by_kill_statement(&db1a, &db2, SLEEP_TIME);
    assert!(elapsed < secs(SLEEP_TIME), "remote KILL early");

    // 三个 DB 分别来自两个节点，关闭顺序不影响断言，但有助于验证回收逻辑无交叉依赖。
    let _ = db1a.close();
    let _ = db1b.close();
    let _ = db2.close();
    s.stop_service("tidb1", &tidb1, true).unwrap();
    s.stop_service("tidb2", &tidb2, true).unwrap();
}

/// 场景 4：验证 PD 断连后的旧连接失效、拒绝新连接，以及恢复后的重新建连能力。
#[test]
fn test_lost_connection() {
    do_test_lost_connection(false);
}

/// 断连恢复场景的 32 位模式副本，保证 ID 宽度不会改变控制面结论。
#[test]
fn test_lost_connection32() {
    do_test_lost_connection(true);
}

/// 该用例最贴近 Go 中的真实故障流程：
/// 先建立连接并运行长查询，再断 PD，等待超时，确认旧连接报错，最后恢复集群并验证新连接可继续 kill。
fn do_test_lost_connection(enable32_bits: bool) {
    let s = create_global_kill_suite(enable32_bits);
    assert!(s.pd_err.is_none(), "pd must connect");

    let port1 = tidb_start_port() + 1;
    let tidb1 = s
        .start_tidb_with_pd(port1, tidb_status_port() + 1, &pd_client_path())
        .unwrap();
    let db1 = s.connect_tidb(port1).unwrap();

    let port2 = tidb_start_port() + 2;
    let tidb2 = s
        .start_tidb_with_pd(port2, tidb_status_port() + 2, &pd_client_path())
        .unwrap();
    let db2 = s.connect_tidb(port2).unwrap();

    let conn1 = db_conn(&db1).unwrap();
    exec_select1(&conn1.slot).expect("ping");

    let sql_time = lost_connection_to_pd_timeout() + 10;
    let (tx, rx) = mpsc::channel();
    let slot = Arc::clone(&conn1.slot);
    thread::spawn(move || sleep_routine(sql_time, slot, tx));
    thread::sleep(WAIT_TO_STARTUP);

    // 先停 PD，但此时旧连接还不会立刻失效。
    eprintln!("[INFO] shutdown PD to simulate lost connection to PD.");
    s.cluster.stop_pd().unwrap();

    // wait lostConnectionToPDTimeout (+ small detect slack).
    let sleep_time = secs(lost_connection_to_pd_timeout() + 3);
    eprintln!("[INFO] sleep to wait for TiDB PD-lost detect {sleep_time:?}");
    thread::sleep(sleep_time);
    // Apply lost effect (Go TiDB detects asynchronously; harness applies after wait).
    for sref in s
        .cluster
        .inner
        .lock()
        .unwrap()
        .servers
        .values()
        .cloned()
        .collect::<Vec<_>>()
    {
        sref.on_pd_lost();
    }

    let r = rx.recv().expect("sleepRoutine");
    eprintln!(
        "[INFO] sleepRoutine err={:?}",
        r.err.as_ref().map(|e| e.Error())
    );
    // 旧连接应在超时后收到“invalid connection”，而不是被静默吞掉或自然结束。
    let err = r.err.expect("existing connections killed after PD lost");
    assert_eq!(err.Error(), "invalid connection");

    eprintln!("[INFO] check connection after lost connection to PD.");
    // PD 已断且检测窗口已过，新的建连必须失败。
    let err = s.connect_tidb(port1).unwrap_err();
    assert_eq!(err.Error(), "driver: bad connection");

    // 恢复顺序与 Go 保持一致：先恢复 TiKV，再整体拉起集群，然后等待 PD 恢复探测窗口。
    s.cluster.stop_tikv().unwrap();
    s.cluster.start_cluster().unwrap();

    let sleep_time = secs(time_to_check_pd_connection_restored() + 3);
    eprintln!("[INFO] sleep to wait for PD restored detect {sleep_time:?}");
    thread::sleep(sleep_time);

    {
        let db1 = s.connect_tidb(port1).expect("restored db1");
        let db2 = s.connect_tidb(port2).expect("restored db2");

        // 恢复后先测本地 kill，再测远程 kill，证明全局连接表重新建立。
        let elapsed = s.kill_by_kill_statement(&db1, &db1, 2);
        assert!(elapsed < secs(2));
        let elapsed = s.kill_by_kill_statement(&db1, &db2, 2);
        assert!(elapsed < secs(2));

        let _ = db1.close();
        let _ = db2.close();
    }

    conn1.close();
    let _ = db1.close();
    let _ = db2.close();
    s.stop_service("tidb1", &tidb1, true).unwrap();
    s.stop_service("tidb2", &tidb2, true).unwrap();
}

/// 场景 5：验证 server ID 空间会在 32 位耗尽后升级，并在释放名额后允许回到 32 位。
#[test]
fn test_server_id_upgrade_and_downgrade() {
    let s = create_global_kill_suite(true);
    assert!(s.pd_err.is_none());

    const MAX_TIDB_32: usize = 2;
    const MAX_TIDB_64: usize = 2;

    // 预先开足够多的槽位，便于后续按索引停止并重新启动节点。
    let mut tidbs: Vec<Option<Cmd>> = (0..MAX_TIDB_32 * 2).map(|_| None).collect();

    // 前两个 TiDB 占用 32 位 server ID 名额。
    for i in 0..MAX_TIDB_32 {
        tidbs[i] = Some(s.must_start_tidb_with_pd(
            tidb_start_port() + i as i32,
            tidb_status_port() + i as i32,
            &pd_client_path(),
        ));
    }
    for i in 0..MAX_TIDB_32 {
        let conn = s.must_connect_tidb(tidb_start_port() + i as i32);
        conn.must_be32();
        conn.close();
    }

    // 后续节点应自动升级到 64 位 server ID 空间。
    for i in MAX_TIDB_32..(MAX_TIDB_32 + MAX_TIDB_64) {
        tidbs[i] = Some(s.must_start_tidb_with_pd(
            tidb_start_port() + i as i32,
            tidb_status_port() + i as i32,
            &pd_client_path(),
        ));
    }
    for i in MAX_TIDB_32..(MAX_TIDB_32 + MAX_TIDB_64) {
        let conn = s.must_connect_tidb(tidb_start_port() + i as i32);
        conn.must_be64();
        conn.close();
    }

    // 释放一部分节点后，再新启动的节点应重新拿回 32 位名额。
    for i in (MAX_TIDB_32 / 2)..(MAX_TIDB_32 + MAX_TIDB_64) {
        if let Some(cmd) = tidbs[i].take() {
            s.stop_service(&format!("tidb{i}"), &cmd, true).unwrap();
        }
    }

    let db_idx = (MAX_TIDB_32 + MAX_TIDB_64) as i32;
    let tidb = s.must_start_tidb_with_pd(
        tidb_start_port() + db_idx,
        tidb_status_port() + db_idx,
        &pd_client_path(),
    );
    let conn = s.must_connect_tidb(tidb_start_port() + db_idx);
    conn.must_be32();
    conn.close();
    // 新节点验证完成后立即回收，避免影响最后的统一清理。
    s.stop_service(&format!("tidb{db_idx}"), &tidb, true)
        .unwrap();

    for (i, slot) in tidbs.into_iter().enumerate() {
        if let Some(cmd) = slot {
            s.stop_service(&format!("tidb{i}"), &cmd, true).unwrap();
        }
    }
}

/// 场景 6：验证单个 TiDB 上的连接 ID 空间也会经历相同的升级与降级过程。
#[test]
fn test_conn_id_upgrade_and_downgrade() {
    let s = create_global_kill_suite(true);
    assert!(s.pd_err.is_none());

    let tidb = s.must_start_tidb_with_pd(tidb_start_port(), tidb_status_port(), &pd_client_path());

    // Go 测试通过 ldflags 把 32 位连接名额压缩到较小值，这里照搬同一门槛。
    const MAX_CONN_32: usize = (1 << 4) - 1;
    let mut conns32: HashMap<u64, Conn> = HashMap::new();

    // 先占满全部 32 位连接名额。
    for _ in 0..MAX_CONN_32 {
        let conn = s.must_connect_tidb(tidb_start_port());
        assert!(conn.conn_id < (1u64 << 32), "connID {:x}", conn.conn_id);
        conns32.insert(conn.conn_id, conn);
    }
    // 名额耗尽后，新增连接必须升级到 64 位。
    for _ in MAX_CONN_32..(MAX_CONN_32 * 2) {
        let conn = s.must_connect_tidb(tidb_start_port());
        conn.must_be64();
        conn.close();
    }

    // 释放超过一半的 32 位连接后，再新建连接应重新落回 32 位空间。
    let mut count = MAX_CONN_32 / 2 + 1;
    let ids: Vec<u64> = conns32.keys().copied().collect();
    for conn_id in ids {
        if let Some(conn) = conns32.remove(&conn_id) {
            conn.close();
            count -= 1;
            if count == 0 {
                break;
            }
        }
    }
    let conn = s.must_connect_tidb(tidb_start_port());
    conn.must_be32();
    conn.close();

    for (_, conn) in conns32 {
        // 剩余的 32 位连接在测试末尾统一释放，模拟真实连接逐步退出后的降级空间回收。
        conn.close();
    }
    s.stop_service("tidb0", &tidb, true).unwrap();
}

/// 场景 7：区分 `KILL QUERY` 与 `KILL CONNECTION` 对空闲连接的影响。
#[test]
fn test_kill_query_on_idle_connection() {
    let s = create_global_kill_suite(true);
    assert!(s.pd_err.is_none());

    let port1 = tidb_start_port() + 1;
    let tidb1 = s
        .start_tidb_with_pd(port1, tidb_status_port() + 1, &pd_client_path())
        .unwrap();

    let db1 = s.connect_tidb(port1).unwrap();
    let db2 = s.connect_tidb(port1).unwrap();

    let conn1 = db_conn(&db1).unwrap();
    let conn_id1 = conn1.conn_id;
    let conn2 = db_conn(&db2).unwrap();
    // 两条连接都连到同一 TiDB，确保差异完全来自 kill 类型，而非跨节点路由。

    // 空闲连接上的 `KILL QUERY` 不应关闭连接，只应影响当前语句。
    exec_select1(&conn1.slot).expect("select 1");
    s.cluster.kill_query_global(conn_id1).expect("KILL QUERY");
    // verify connection is still alive
    exec_select1(&conn1.slot).expect("select 1 after KILL QUERY");

    // `KILL CONNECTION` 则必须让后续任何请求都看到连接失效。
    s.cluster
        .kill_connection_global(conn_id1)
        .expect("KILL CONNECTION");
    let err = exec_select1(&conn1.slot).unwrap_err();
    assert!(
        !err.Error().is_empty(),
        "connection must be closed after KILL CONNECTION"
    );

    // `conn2` 本身未被 kill，用它证明发起 kill 的会话不会被连带关闭。
    conn2.close();
    let _ = db1.close();
    let _ = db2.close();
    s.stop_service("tidb1", &tidb1, true).unwrap();
}
