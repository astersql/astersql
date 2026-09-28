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

// 对应 `pkg/executor/test/unstabletest/memory_test.go`。
//
// `TestGlobalMemoryControl` 用真实的 `astersql-util-servermemorylimit::Handle`
// （生产 `ServerMemoryLimitHandle::Run`/`killSessIfNeeded` 实现）驱动一个后台线程，
// 对接真实 `astersql-util-memory::tracker::Tracker`/`MemUsageTop1Tracker`/`SQLKiller`，
// 完整复现 Go 用例里“三个会话 tracker 消费内存 -> 全局内存控制器挑出 Top1 -> kill
// 信号使后续 `Consume` panic 报 `ErrMemoryExceedForInstance`”的真实链路；只是把 Go 里
// `testkit.NewTestKit` 建立的整套会话换成手工构造的会话 `Tracker`/`ProcessInfo`
// （因为 `TestKit` 目前的 `ConcreteSession`/`AnalyzeStatsStore` 还没有把
// `SessionVars.MemTracker` 暴露给测试，见仓库里其它任务对 `astersql-testkit` 的
// 调查结论）。`tidb_server_memory_limit` 阈值故意设成一个真实进程 RSS 必然超过的
// 极小值（而不是 Go 里的 `512 << 20`），因为生产 `killSessIfNeeded` 用
// `ReadMemStats()` 读取的是真实操作系统进程内存，与 `pkg/util/servermemorylimit`
// crate 自带的 `killSessIfNeeded(&mut state, 1, &provider)` 单测（同目录
// `migration_aster_unit_test.rs`）用的思路一致：只要阈值非零，就一定会在第一次
// tick 触发检查，从而让这条真实路径在任意机器上都确定性地跑通。
//
// `TestPBMemoryLeak` 通过当前 TestKit 的真实查询路径执行 `select * from t`：
// TestKit 在返回 `QueryRows` 前排空并关闭底层 `ConcreteRecordSet`，测试再释放
// `QueryRows`，以完整 256 MiB 数据量、会话 tracker 清理和第二次扫描后的进程
// 内存增长上限共同覆盖 Go 用例的结果集资源释放契约。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use astersql_session_sessmgr::memory::{
    MemUsageTop1Tracker, ServerMemoryLimit, ServerMemoryLimitSessMinSize, Tracker,
};
use astersql_session_sessmgr::{InfoSchemaCoordinator, InternalSession, Manager, ProcessInfo};
use astersql_testkit::{DbValue, NewTestKit, TestKit, mockstore::CreateMockStoreAndDomain};
use astersql_util_dbterror_exeerrors::exeerrors::ErrMemoryExceedForInstance;
use astersql_util_intest::EnableInternalCheck;
use astersql_util_memory::memstats::ForceReadMemStats;
use astersql_util_memory::sqlkiller::SQLKiller;
use astersql_util_servermemorylimit::NewServerMemoryLimitHandle;

/// Go tests in this file are not parallel. Serialize the Rust equivalents too,
/// because both exercise process-wide intest and memory-controller state.
static UNSTABLE_MEMORY_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 对应 Go `testkit.MockSessionManager`：只实现全局内存控制器真正用到的
/// `GetProcessInfo`，其余 `Manager`/`InfoSchemaCoordinator` 方法保持最小真实实现。
#[derive(Default)]
struct TestSessionManager {
    processes: Mutex<HashMap<u64, Arc<ProcessInfo>>>,
}

impl TestSessionManager {
    /// 注册一条会话进程信息，供后续 `GetProcessInfo`/`ShowProcessList` 查询。
    fn insert(&self, info: ProcessInfo) {
        self.processes
            .lock()
            .expect("process map lock poisoned")
            .insert(info.ID, Arc::new(info));
    }

    /// Replace a session snapshot while preserving the manager's live-process
    /// lookup semantics.  The Go test updates `ProcessInfo.Time` after the
    /// first cancelled statement so the controller can select the next SQL.
    fn update_time(&self, session_id: u64, time: SystemTime) {
        let mut processes = self.processes.lock().expect("process map lock poisoned");
        let info = processes
            .get(&session_id)
            .unwrap_or_else(|| panic!("session {session_id} is not registered"));
        let mut updated = info.as_ref().Clone();
        updated.Time = time;
        processes.insert(session_id, Arc::new(updated));
    }
}

impl InfoSchemaCoordinator for TestSessionManager {
    fn StoreInternalSession(&self, _: InternalSession) {}
    fn DeleteInternalSession(&self, _: &InternalSession) {}
    fn ContainsInternalSession(&self, _: &InternalSession) -> bool {
        false
    }
    fn InternalSessionCount(&self) -> isize {
        0
    }
    fn CheckOldRunningTxn(
        &self,
        _: &mut HashMap<i64, Arc<astersql_session_sessmgr::mdldef::JobMDL>>,
    ) {
    }
    fn KillNonFlashbackClusterConn(&self) {}
}

impl Manager for TestSessionManager {
    fn ShowProcessList(&self) -> HashMap<u64, Arc<ProcessInfo>> {
        self.processes
            .lock()
            .expect("process map lock poisoned")
            .clone()
    }
    fn ShowTxnList(&self) -> Vec<Arc<astersql_session_sessmgr::txninfo::TxnInfo>> {
        Vec::new()
    }
    fn GetProcessInfo(&self, id: u64) -> Option<Arc<ProcessInfo>> {
        self.processes
            .lock()
            .expect("process map lock poisoned")
            .get(&id)
            .cloned()
    }
    fn Kill(&self, _connection_id: u64, _query: bool, _max_execution_time: bool, _runaway: bool) {}
    fn KillAllConnections(&self) {}
    fn UpdateTLSConfig(&self, _cfg: Option<Arc<rustls::ServerConfig>>) {}
    fn ServerID(&self) -> u64 {
        0
    }
    fn GetInternalSessionStartTSList(&self) -> Vec<u64> {
        Vec::new()
    }
    fn GetConAttrs(
        &self,
        _user: &astersql_session_sessmgr::auth::UserIdentity,
    ) -> HashMap<u64, HashMap<String, String>> {
        HashMap::new()
    }
    fn GetStatusVars(&self) -> HashMap<u64, HashMap<String, String>> {
        HashMap::new()
    }
}

/// 对应 Go 里 `tkN.Session().GetSessionVars().MemTracker`：构造一个 session-root
/// tracker，挂上真实 `SQLKiller`。Go 额外调用
/// `tracker.FallbackOldAndSetNewAction(&memory.PanicOnExceed{})` 注册硬限额超限时的
/// panic 动作，但这里两个 tracker 都用 `bytesLimit = -1`（无限额），从不会触发硬限额
/// 检查；真正让 `Consume` panic 的是 `Tracker::Consume` 里独立于 action 链的
/// “正向消费后若 session root 的 `Killer.HandleSignal()` 返回 Err 就 panic”分支
/// （见 `pkg/util/memory/tracker.rs`），因此不需要接线 action。
fn new_session_tracker(session_id: u64) -> Arc<Tracker> {
    let mut tracker = Tracker::new(0, -1);
    tracker.IsRootTrackerOfSess = true;
    tracker.SessionID.Store(session_id);
    tracker.Killer = Some(Box::new(SQLKiller::new()));
    tracker
        .Killer
        .as_ref()
        .expect("killer just assigned")
        .ConnID
        .store(session_id, Ordering::SeqCst);
    Arc::new(tracker)
}

/// 把会话根 tracker 挂到 `ProcessInfo`，供 `Manager::GetProcessInfo` 暴露给全局内存控制器。
fn new_process_info(session_id: u64, tracker: &Arc<Tracker>) -> ProcessInfo {
    ProcessInfo {
        ID: session_id,
        Time: SystemTime::now(),
        MemTracker: Some(Arc::clone(tracker)),
        ..ProcessInfo::default()
    }
}

#[derive(Debug)]
struct ScanSummary {
    row_count: usize,
    payload_bytes: usize,
    heap_inuse_after_close: u64,
}

/// Execute a full table scan through the canonical TestKit query path and
/// consume every returned row before the result is dropped.  The Go helper
/// uses `RecordSet.Next` until an empty chunk and then closes the record set;
/// Rust's TestKit drains and closes the concrete record set at the same
/// boundary before returning `QueryRows`.
fn read_all_rows_and_close(tk: &TestKit, sql: &str) -> ScanSummary {
    let rows = tk.Query(sql, Vec::new()).expect("table scan must succeed");
    let row_count = rows.rows.len();
    let payload_bytes = rows
        .rows
        .iter()
        .flatten()
        .map(|cell| match cell {
            DbValue::String(value) => value.len(),
            DbValue::Bytes(value) => value.len(),
            DbValue::Null => 0,
            DbValue::Bool(_) => std::mem::size_of::<bool>(),
            DbValue::I64(_) => std::mem::size_of::<i64>(),
            DbValue::U64(_) => std::mem::size_of::<u64>(),
            DbValue::F64(_) => std::mem::size_of::<f64>(),
        })
        .sum();
    drop(rows);
    ScanSummary {
        row_count,
        payload_bytes,
        heap_inuse_after_close: ForceReadMemStats().heap_inuse,
    }
}

/// RAII 收尾：测试结束后把本用例改动的全局状态复原，避免影响同进程里的其它测试。
struct GlobalMemoryStateGuard {
    server_memory_limit: u64,
    server_memory_limit_sess_min_size: u64,
}

impl GlobalMemoryStateGuard {
    /// 记录当前全局内存限制相关原子量，供 Drop 时复原。
    fn capture() -> Self {
        Self {
            server_memory_limit: ServerMemoryLimit.Load(),
            server_memory_limit_sess_min_size: ServerMemoryLimitSessMinSize.Load(),
        }
    }
}

impl Drop for GlobalMemoryStateGuard {
    fn drop(&mut self) {
        ServerMemoryLimit.Store(self.server_memory_limit);
        ServerMemoryLimitSessMinSize.Store(self.server_memory_limit_sess_min_size);
        MemUsageTop1Tracker.store(std::ptr::null_mut(), Ordering::SeqCst);
    }
}

/// Rust's `intest.InTest` variant is selected at compile time. The dynamic
/// `EnableInternalCheck` switch is its per-test equivalent for enabling the
/// internal assertions that Go's `enableIntestAssertForTest` turns on.
struct IntestAssertGuard {
    enable_internal_check: bool,
}

impl IntestAssertGuard {
    fn enable() -> Self {
        Self {
            enable_internal_check: EnableInternalCheck.swap(true, Ordering::SeqCst),
        }
    }
}

impl Drop for IntestAssertGuard {
    fn drop(&mut self) {
        EnableInternalCheck.store(self.enable_internal_check, Ordering::SeqCst);
    }
}

/// Own the controller's exit channel and join handle so an assertion failure
/// cannot leave a background controller racing later tests and global cleanup.
struct ServerMemoryLimitWorker {
    exit_tx: Option<mpsc::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ServerMemoryLimitWorker {
    fn start(manager: Arc<TestSessionManager>) -> Self {
        let (exit_tx, exit_rx) = mpsc::channel();
        let handle = NewServerMemoryLimitHandle(exit_rx);
        handle.SetSessionManager(manager);
        Self {
            exit_tx: Some(exit_tx),
            worker: Some(std::thread::spawn(move || handle.Run())),
        }
    }

    fn shutdown(&mut self) -> std::thread::Result<()> {
        if let Some(exit_tx) = self.exit_tx.take() {
            let _ = exit_tx.send(());
        }
        self.worker
            .take()
            .map_or(Ok(()), std::thread::JoinHandle::join)
    }

    fn join(mut self) {
        self.shutdown()
            .expect("global memory limit worker thread must not panic");
    }
}

impl Drop for ServerMemoryLimitWorker {
    fn drop(&mut self) {
        // Preserve the original assertion panic while still guaranteeing that
        // the controller receives its exit signal and cannot outlive the test.
        let _ = self.shutdown();
    }
}

/// 对应 Go `TestGlobalMemoryControl` 中 Top1 tracker 被真正 kill 的核心链路：
///   1. 三个会话按 100/200/300 MB 顺序 `Consume`，每次都会更新真实
///      `MemUsageTop1Tracker`（`Tracker::Consume` 内部的 CAS 比较逻辑），全部在
///      启动后台控制器*之前*完成，避免控制器在中间状态下误杀非 Top1 会话；
///   2. 后台 `ServerMemoryLimitHandle::Run()` 每 100ms 检查一次真实进程 RSS
///      （`astersql-util-memory::memstats::ReadMemStats`），一旦超过
///      `tidb_server_memory_limit` 就给 Top1 tracker 的 `SQLKiller`
///      发 `ServerMemoryExceeded` 信号；
///   3. tracker3（Top1）的 `Killer.HandleSignal()` 应报
///      `ErrMemoryExceedForInstance`，tracker1/tracker2 应保持无信号；
///   4. 之后任何正向 `tracker3.Consume(_)` 都会在真实 `Tracker::Consume` 内部
///      直接 panic（Go `util.WithRecovery` 捕获的同一条路径）。
#[test]
fn global_memory_control_kills_top1_session_tracker() {
    let _serial = UNSTABLE_MEMORY_TEST_LOCK
        .lock()
        .expect("unstable memory test lock poisoned");
    let _intest_asserts = IntestAssertGuard::enable();
    let _guard = GlobalMemoryStateGuard::capture();

    // 128 字节的 sess-min-size 与 Go 用例的 `tidb_server_memory_limit_sess_min_size = 128` 对齐。
    ServerMemoryLimitSessMinSize.Store(128);
    // 阈值设成任意真实进程 RSS 都会超过的极小非零值：`killSessIfNeeded` 只要
    // `bt != 0` 且 `heap_inuse > bt` 就会挑选 Top1 会话发 kill 信号，不依赖具体
    // 数值，因此不需要像 Go 那样先靠真实分配把 RSS 推过一个大阈值。
    ServerMemoryLimit.Store(1);

    let tracker1 = new_session_tracker(1);
    let tracker2 = new_session_tracker(2);
    let tracker3 = new_session_tracker(3);

    let manager = Arc::new(TestSessionManager::default());
    manager.insert(new_process_info(1, &tracker1));
    manager.insert(new_process_info(2, &tracker2));
    manager.insert(new_process_info(3, &tracker3));

    // 三次 Consume 必须先于后台控制器启动完成，确保 MemUsageTop1Tracker 落定在
    // tracker3 之后才可能被后台线程读到，避免控制器在中间状态下选中较小的 tracker。
    tracker1.Consume(100 << 20); // 100 MB
    tracker2.Consume(200 << 20); // 200 MB
    tracker3.Consume(300 << 20); // 300 MB -> Top1
    assert_eq!(
        MemUsageTop1Tracker.load(Ordering::SeqCst),
        Arc::as_ptr(&tracker3) as *mut Tracker,
        "tracker3 must be the top1 session tracker before the controller starts"
    );

    let worker = ServerMemoryLimitWorker::start(manager.clone());

    // 后台控制器每 100ms tick 一次；等待足够久确保至少命中一次超限检查。
    std::thread::sleep(Duration::from_millis(500));

    assert!(
        tracker1
            .Killer
            .as_ref()
            .expect("tracker1 killer")
            .HandleSignal()
            .is_ok(),
        "tracker1 must not receive a kill signal"
    );
    assert!(
        tracker2
            .Killer
            .as_ref()
            .expect("tracker2 killer")
            .HandleSignal()
            .is_ok(),
        "tracker2 must not receive a kill signal"
    );
    match tracker3
        .Killer
        .as_ref()
        .expect("tracker3 killer")
        .HandleSignal()
    {
        Ok(()) => panic!("tracker3 (top1 consumer) must be killed by the global memory controller"),
        Err(error) => assert!(
            ErrMemoryExceedForInstance.Equal(Some(&error)),
            "expected ErrMemoryExceedForInstance, got {error}"
        ),
    }
    assert_eq!(
        MemUsageTop1Tracker.load(Ordering::SeqCst),
        Arc::as_ptr(&tracker3) as *mut Tracker,
        "MemUsageTop1Tracker must still point at the killed top1 session tracker"
    );

    // 对应 Go `util.WithRecovery(func() { tracker3.Consume(1) }, ...)`：一旦
    // kill 信号已经落在会话根 tracker 上，真实 `Tracker::Consume` 会在正向消费
    // 后自动调用 `Killer.HandleSignal()` 并 panic。
    let panic_payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tracker3.Consume(1);
    }))
    .expect_err("tracker3.Consume() must panic once a kill signal is pending");
    let message = panic_payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        message.contains("tidb-server instance"),
        "panic message should mention the instance memory limit, got {message:?}"
    );

    // Match the remainder of Go `TestGlobalMemoryControl`: while tracker3's
    // cancelled SQL is finishing, tracker2 may grow without panicking.  Once
    // tracker3 releases its memory and its ProcessInfo timestamp advances,
    // the controller is allowed to clear the first kill state and choose the
    // next top consumer.
    tracker2.Consume(300 << 20); // 500 MB total; the first kill is still tracker3.
    assert_eq!(tracker2.BytesConsumed(), 500 << 20);
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        tracker2
            .Killer
            .as_ref()
            .expect("tracker2 killer")
            .HandleSignal()
            .is_ok(),
        "tracker2 must not be killed before tracker3 finishes"
    );

    tracker3.Consume(-(300 << 20));
    manager.update_time(3, SystemTime::now());
    std::thread::sleep(Duration::from_millis(150));

    // The controller must observe the changed statement timestamp, clear the
    // old kill state, and let tracker2 become the new Top1 tracker.
    assert_eq!(
        MemUsageTop1Tracker.load(Ordering::SeqCst),
        Arc::as_ptr(&tracker2) as *mut Tracker,
        "tracker2 must become the top1 tracker after tracker3 is released"
    );
    std::thread::sleep(Duration::from_millis(500));
    match tracker2
        .Killer
        .as_ref()
        .expect("tracker2 killer")
        .HandleSignal()
    {
        Ok(()) => panic!("tracker2 must be killed after becoming the next top1 consumer"),
        Err(error) => assert!(
            ErrMemoryExceedForInstance.Equal(Some(&error)),
            "expected ErrMemoryExceedForInstance for tracker2, got {error}"
        ),
    }

    let panic_payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tracker2.Consume(1);
    }))
    .expect_err("tracker2.Consume() must panic once its kill signal is pending");
    let message = panic_payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        message.contains("tidb-server instance"),
        "panic message should mention the instance memory limit, got {message:?}"
    );

    worker.join();
}

/// 对应 Go `TestPBMemoryLeak`：准备 256MB 的定长行，完整扫描两次，并确认
/// 每次扫描都返回全部数据、语句 tracker 没有留下子 tracker，且第二次扫描
/// 后的进程内存增长小于 Go 用例相同的 `total_size / 5` 阈值。Rust 没有 Go
/// tracing GC 的 `TotalAlloc`；因此用实际扫描到的 payload 字节数证明首轮确实
/// 物化完整负载，再用生产 `ForceReadMemStats` 的 RSS 快照覆盖重复扫描增长。
#[test]
fn pb_memory_leak_scans_release_result_sets() {
    let _serial = UNSTABLE_MEMORY_TEST_LOCK
        .lock()
        .expect("unstable memory test lock poisoned");
    let _intest_asserts = IntestAssertGuard::enable();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("create database test_mem", Vec::new());
    tk.MustExec("use test_mem", Vec::new());

    let total_size = 256usize << 20;
    let block_size = 8usize << 10;
    let num_rows = total_size / block_size;
    tk.MustExec(
        &format!("create table t (c varchar({block_size}))"),
        Vec::new(),
    );
    // Keep the Go fixture's exact 256MiB/8KiB/32,768-row data shape while
    // grouping inserts for the in-process Rust mock KV store.  The grouping
    // changes only setup overhead, not the table contents being scanned.
    let batch_size = 128;
    let values = std::iter::repeat_n(format!("(space({block_size}))"), batch_size)
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!("insert into t values {values}");
    for _ in 0..num_rows / batch_size {
        tk.MustExec(&insert_sql, Vec::new());
    }

    let first_scan = read_all_rows_and_close(&tk, "select * from t");
    assert_eq!(first_scan.row_count, num_rows);
    assert_eq!(
        first_scan.payload_bytes, total_size,
        "the first scan must materialize the full 256 MiB fixture"
    );
    assert_eq!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .len(),
        0,
        "first scan must release its statement tracker"
    );

    let second_scan = read_all_rows_and_close(&tk, "select * from t");
    assert_eq!(second_scan.row_count, num_rows);
    assert_eq!(
        second_scan.payload_bytes, total_size,
        "the second scan must materialize the same full fixture"
    );
    assert_eq!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .len(),
        0,
        "second scan must not retain result-set trackers"
    );
    let heap_growth = second_scan
        .heap_inuse_after_close
        .saturating_sub(first_scan.heap_inuse_after_close);
    assert!(
        heap_growth < (total_size / 5) as u64,
        "heap growth after the second scan must stay below the Go delta: growth={heap_growth}, delta={}",
        total_size / 5
    );
}
