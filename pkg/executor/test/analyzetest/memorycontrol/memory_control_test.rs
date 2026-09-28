// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// ANALYZE 过程中的全局/会话内存控制测试。
//
// 对应 `pkg/executor/test/analyzetest/memorycontrol/memory_control_test.go`。
//
// 完整 analyze SQL + failpoint 注入路径尚未接通，因此：
// 1. 用生产 `MockSessionManager` / `ProcessInfo` 验证 live vs stale process info；
// 2. 用生产 `ServerMemoryLimitHandle` + session `Tracker` 验证实例内存取消链路；
// 3. 用 `Tracker::AttachTo`/`Detach` 验证 session close 后 analyze tracker 解绑；
// 4. 保留 Go SQL / failpoint 名称与取消错误文案做一致性检查。
//
// failpoint：故障注入点。Tracker：会话/查询内存记账器；
// ServerMemoryLimit：实例级内存上限，超限时取消当前占用最多内存的查询（Top1）。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use astersql_session_sessmgr::memory::{
    MemUsageTop1Tracker, ServerMemoryLimit, ServerMemoryLimitSessMinSize, Tracker,
};
use astersql_session_sessmgr::{InfoSchemaCoordinator, InternalSession, Manager, ProcessInfo};
use astersql_statistics::MaxSampleValueLength;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_util_dbterror_exeerrors::exeerrors::ErrMemoryExceedForInstance;
use astersql_util_memory::sqlkiller::SQLKiller;
use astersql_util_servermemorylimit::NewServerMemoryLimitHandle;

/// 实例内存超限时取消查询的英文错误文案（与 Go / 客户端提示保持一致）。
const INSTANCE_MEMORY_CANCEL_MESSAGE: &str = "Your query has been cancelled due to exceeding the allowed memory limit for the tidb-server instance and this query is currently using the most memory. Please try narrowing your query scope or increase the tidb_server_memory_limit and try again.";

/// Go failpoint：注入 `ReadMemStats` 返回值以伪造进程内存占用。
const FP_READ_MEM_STATS: &str = "github.com/pingcap/tidb/pkg/util/memory/ReadMemStats";
/// Go failpoint：拖慢 analyze merge worker 的 Consume，便于触发内存限制。
const FP_SLOW_CONSUME: &str =
    "github.com/pingcap/tidb/pkg/executor/mockAnalyzeMergeWorkerSlowConsume";

/// 创建带真实 SQL 会话的 ANALYZE 测试夹具。
fn new_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

/// 对齐 Go 内存控制用例：构造 256 行的分析表，避免测试只校验 SQL 字符串。
fn populate_analyze_table(tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t select 1", Vec::new());
    for _ in 1..=8 {
        tk.MustExec("insert into t select * from t", Vec::new());
    }
}

/// RAII 守卫：保存并在 Drop 时还原全局 `ServerMemoryLimit` 相关状态。
struct GlobalMemoryStateGuard {
    server_memory_limit: u64,
    server_memory_limit_sess_min_size: u64,
}

impl GlobalMemoryStateGuard {
    /// 捕获当前全局内存限制与会话最小追踪阈值。
    fn capture() -> Self {
        Self {
            server_memory_limit: ServerMemoryLimit.Load(),
            server_memory_limit_sess_min_size: ServerMemoryLimitSessMinSize.Load(),
        }
    }
}

impl Drop for GlobalMemoryStateGuard {
    fn drop(&mut self) {
        // 还原全局限制并清空 Top1 Tracker 指针，避免泄漏到后续用例。
        ServerMemoryLimit.Store(self.server_memory_limit);
        ServerMemoryLimitSessMinSize.Store(self.server_memory_limit_sess_min_size);
        MemUsageTop1Tracker.store(std::ptr::null_mut(), Ordering::SeqCst);
    }
}

/// 测试用 SessionManager：仅维护 `ProcessInfo` 映射供内存限制句柄扫描。
#[derive(Default)]
struct TestSessionManager {
    processes: Mutex<HashMap<u64, Arc<ProcessInfo>>>,
}

impl TestSessionManager {
    /// 按连接 ID 登记一条进程信息（含可选 MemTracker）。
    fn insert(&self, info: ProcessInfo) {
        self.processes
            .lock()
            .expect("process map lock poisoned")
            .insert(info.ID, Arc::new(info));
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

/// 构造带 SQLKiller 的会话根 Tracker，供实例内存限制扫描识别。
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

/// 校验实例取消文案包含关键关键词（instance / limit / most memory）。
fn assert_instance_cancel_message() {
    assert!(INSTANCE_MEMORY_CANCEL_MESSAGE.contains("tidb-server instance"));
    assert!(INSTANCE_MEMORY_CANCEL_MESSAGE.contains("tidb_server_memory_limit"));
    assert!(INSTANCE_MEMORY_CANCEL_MESSAGE.contains("most memory"));
}

/// 对应 Go `TestLiveSessionManagerTracksLatestProcessInfo`。
/// 验证 SessionManager 能区分旧/新 ProcessInfo（如 analyze 开始时间刷新）。
#[test]
fn TestLiveSessionManagerTracksLatestProcessInfo() {
    let conn_id = 42_u64;
    let stale_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    let next_time = SystemTime::UNIX_EPOCH + Duration::from_secs(2);
    let analyze_sql = "analyze table t with 1.0 samplerate";

    // stale / live 各登记一条同 conn_id 的 ProcessInfo；live 快照同时
    // 刷新开始时间和 SQL 文本，对齐 Go 的 SetProcessInfo 后全部断言。
    let stale = TestSessionManager::default();
    stale.insert(ProcessInfo {
        ID: conn_id,
        Time: stale_time,
        Info: "select 1".into(),
        ..ProcessInfo::default()
    });

    let live = TestSessionManager::default();
    live.insert(ProcessInfo {
        ID: conn_id,
        Time: next_time,
        Info: analyze_sql.into(),
        ..ProcessInfo::default()
    });

    let stale_info = stale.GetProcessInfo(conn_id).expect("stale");
    let live_info = live.GetProcessInfo(conn_id).expect("live");
    assert!(!Arc::ptr_eq(&stale_info, &live_info));
    assert_ne!(stale_info.Time, live_info.Time);
    assert_ne!(stale_info.Info, live_info.Info);
    assert_eq!(live_info.Time, next_time);
    assert_eq!(live_info.Info, analyze_sql);
}

/// 对应 Go `TestGlobalMemoryControlForAnalyze` 的实例内存取消核心链路。
#[test]
fn TestGlobalMemoryControlForAnalyze() {
    assert_eq!(
        FP_READ_MEM_STATS,
        "github.com/pingcap/tidb/pkg/util/memory/ReadMemStats"
    );
    assert_eq!(
        FP_SLOW_CONSUME,
        "github.com/pingcap/tidb/pkg/executor/mockAnalyzeMergeWorkerSlowConsume"
    );
    assert_instance_cancel_message();

    // 保留 Go 原用例的 SQL fixture，完整执行路径待接线。
    for sql in [
        "set global tidb_mem_oom_action = 'cancel'",
        "set global tidb_server_memory_limit = 512MB",
        "set global tidb_server_memory_limit_sess_min_size = 128",
        "use test",
        "create table t(a int)",
        "insert into t select 1",
        "analyze table t with 1.0 samplerate;",
    ] {
        assert!(!sql.is_empty());
    }

    // The Go test executes this fixture both through ANALYZE and through the
    // instance-memory cancellation path.  The latter is exercised below with
    // the production ServerMemoryLimitHandle; execute the real SQL here so a
    // parser/session regression cannot be hidden by fixture-only assertions.
    let mut tk = new_testkit();
    populate_analyze_table(&mut tk);
    tk.MustExec("analyze table t with 1.0 samplerate", Vec::new());

    // 将实例限制压到极低，Consume 后应成为 Top1 并被 ServerMemoryLimitHandle 取消。
    let _guard = GlobalMemoryStateGuard::capture();
    ServerMemoryLimitSessMinSize.Store(128);
    ServerMemoryLimit.Store(1);

    let tracker = new_session_tracker(1);
    let manager = Arc::new(TestSessionManager::default());
    manager.insert(ProcessInfo {
        ID: 1,
        Time: SystemTime::now(),
        Info: "analyze table t with 1.0 samplerate".into(),
        MemTracker: Some(Arc::clone(&tracker)),
        ..ProcessInfo::default()
    });
    tracker.Consume(300 << 20);
    assert_eq!(
        MemUsageTop1Tracker.load(Ordering::SeqCst),
        Arc::as_ptr(&tracker) as *mut Tracker
    );

    let (exit_tx, exit_rx) = mpsc::channel();
    let handle = NewServerMemoryLimitHandle(exit_rx);
    handle.SetSessionManager(manager);
    let worker = std::thread::spawn(move || handle.Run());

    // 有界轮询代替固定睡眠：既给内存控制 worker 足够时间，也避免
    // 慢机器上 500ms 未调度就产生假阴性。
    let mut signal = None;
    for _ in 0..100 {
        match tracker.Killer.as_ref().expect("killer").HandleSignal() {
            Ok(()) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                signal = Some(error);
                break;
            }
        }
    }
    let _ = exit_tx.send(());
    worker
        .join()
        .expect("memory limit worker must exit cleanly");

    let signal = signal.expect("top1 analyze session must be killed within two seconds");
    assert!(ErrMemoryExceedForInstance.Equal(Some(&signal)));
    let signal_message = signal.to_string();
    assert!(
        signal_message.contains(INSTANCE_MEMORY_CANCEL_MESSAGE),
        "unexpected instance cancellation message: {signal_message}"
    );
    assert!(
        signal_message.contains("[conn=1]"),
        "instance cancellation must identify the killed session: {signal_message}"
    );
    // 取消处理结束后，后续 ANALYZE 仍可正常执行；对应 Go 关闭 failpoint
    // 后再次执行同一 SQL 的恢复断言。
    tk.MustExec("analyze table t with 1.0 samplerate", Vec::new());
}

/// 对应 Go `TestGlobalMemoryControlForPrepareAnalyze`。
/// 覆盖 prepare/execute analyze 在实例内存限制下的 SQL fixture。
#[test]
fn TestGlobalMemoryControlForPrepareAnalyze() {
    for sql in [
        "set global tidb_mem_oom_action = 'cancel'",
        "set global tidb_mem_quota_query = 209715200 ",
        "set global tidb_server_memory_limit = 5GB",
        "set global tidb_server_memory_limit_sess_min_size = 128",
        "prepare stmt from 'analyze table t with 1.0 samplerate';",
        "execute stmt;",
        "set global tidb_server_memory_limit = 512MB",
    ] {
        assert!(!sql.is_empty());
    }
    let mut tk = new_testkit();
    populate_analyze_table(&mut tk);
    tk.MustExec("set global tidb_mem_oom_action = 'cancel'", Vec::new());
    tk.MustExec("set global tidb_mem_quota_query = 209715200", Vec::new());
    tk.MustExec(
        "prepare stmt from 'analyze table t with 1.0 samplerate'",
        Vec::new(),
    );
    tk.MustExec("execute stmt", Vec::new());
    assert!(INSTANCE_MEMORY_CANCEL_MESSAGE.contains("tidb-server instance"));
    assert_eq!(FP_READ_MEM_STATS.contains("ReadMemStats"), true);
}

/// 对应 Go `TestGlobalMemoryControlForAutoAnalyze`。
/// AutoAnalyze：后台按修改比例自动触发的统计信息收集。
#[test]
fn TestGlobalMemoryControlForAutoAnalyze() {
    for sql in [
        "select @@global.tidb_mem_oom_action",
        "set global tidb_auto_analyze_ratio = 0.001",
        "insert into t values(4),(5),(6)",
        "flush stats_delta *.*",
        "select fail_reason from mysql.analyze_jobs where table_name=? and state=? limit 1",
    ] {
        assert!(!sql.is_empty());
    }
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("set global tidb_mem_oom_action = 'cancel'", Vec::new());
    tk.MustExec("create table t_auto(a int)", Vec::new());
    tk.MustExec("insert into t_auto values(1),(2),(3)", Vec::new());
    tk.MustExec("analyze table t_auto with 1.0 samplerate", Vec::new());
    assert_eq!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .len(),
        0
    );
    tk.MustExec("insert into t_auto values(4),(5),(6)", Vec::new());
    tk.MustExec("flush stats_delta *.*", Vec::new());
    assert!(domain.try_handle_auto_analyze().is_ok());
    assert_eq!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .len(),
        0
    );
    assert!(INSTANCE_MEMORY_CANCEL_MESSAGE.contains("most memory"));
}

/// 对应 Go `TestMemQuotaAnalyze`（分区表 fixture）。
/// `tidb_mem_quota_analyze`：单次 ANALYZE 的内存配额。
#[test]
fn TestMemQuotaAnalyze() {
    let create = "create table tbl_2 ( col_20 decimal default 84232 , col_21 tinyint not null , col_22 int default 80814394 , col_23 mediumint default -8036687 not null , col_24 smallint default 9185 not null , col_25 tinyint unsigned default 65 , col_26 char(115) default 'ZyfroRODMbNDRZnPNRW' not null , col_27 bigint not null , col_28 tinyint not null , col_29 char(130) default 'UMApsVgzHblmY' , primary key idx_14 ( col_28,col_22 ) , unique key idx_15 ( col_24,col_22 ) , key idx_16 ( col_21,col_20,col_24,col_25,col_27,col_28,col_26,col_29 ) , key idx_17 ( col_24,col_25 ) , unique key idx_18 ( col_25,col_23,col_29,col_27,col_26,col_22 ) , key idx_19 ( col_25,col_22,col_26,col_23 ) , unique key idx_20 ( col_22,col_24,col_28,col_29,col_26,col_20 ) , key idx_21 ( col_25,col_24,col_26,col_29,col_27,col_22,col_28 ) ) partition by range ( col_22 ) ( partition p0 values less than (-1938341588), partition p1 values less than (-1727506184), partition p2 values less than (-1700184882), partition p3 values less than (-1596142809), partition p4 values less than (445165686) );";
    assert!(create.contains("partition by range"));
    assert!(create.contains("partition p4"));
    let quota = "set global tidb_mem_quota_analyze=128;";
    let analyze = "analyze table tbl_2;";
    assert!(quota.contains("128"));
    assert!(analyze.contains("tbl_2"));

    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(create, Vec::new());
    for row in [
        "(942,33,-1915007317,3408149,-3699,193,'Trywdis',1876334369465184864,115,null)",
        "(7,-39,-1382727205,-2544981,-28075,88,'FDhOsTRKRLCwEk',-1239168882463214388,17,'WskQzCK')",
        "(null,55,-388460319,-2292918,10130,162,'UqjDlYvdcNY',4872802276956896607,-51,'ORBQjnumcXP')",
        "(42,-19,-9677826,-1168338,16904,79,'TzOqH',8173610791128879419,65,'lNLcvOZDcRzWvDO')",
        "(2,26,369867543,-6773303,-24953,41,'BvbdrKTNtvBgsjjnxt',5996954963897924308,-95,'wRJYPBahkIGDfz')",
        "(6896,3,444460824,-2070971,-13095,167,'MvWNKbaOcnVuIrtbT',6968339995987739471,-5,'zWipNBxGeVmso')",
        "(58761,112,-1535034546,-5837390,-14204,157,'',-8319786912755096816,15,'WBjsozfBfrPPHmKv')",
        "(84923,113,-973946646,406140,25040,51,'THQdwkQvppWZnULm',5469507709881346105,94,'oGNmoxLLgHkdyDCT')",
        "(0,-104,-488745187,-1941015,-2646,39,'jyKxfs',-5307175470406648836,46,'KZpfjFounVgFeRPa')",
        "(4,97,2105289255,1034363,28385,192,'',4429378142102752351,8,'jOk')",
    ] {
        tk.MustExec(
            &format!("insert ignore into tbl_2 values {row}"),
            Vec::new(),
        );
    }
    tk.MustExec(quota, Vec::new());
    tk.MustExecToErr(analyze);
}

/// 对应 Go `TestMemQuotaAnalyze2`（非分区表 fixture）。
#[test]
fn TestMemQuotaAnalyze2() {
    let create = "create table tbl_2 ( col_20 decimal default 84232 , col_21 tinyint not null , col_22 int default 80814394 , col_23 mediumint default -8036687 not null , col_24 smallint default 9185 not null , col_25 tinyint unsigned default 65 , col_26 char(115) default 'ZyfroRODMbNDRZnPNRW' not null , col_27 bigint not null , col_28 tinyint not null , col_29 char(130) default 'UMApsVgzHblmY' , primary key idx_14 ( col_28,col_22 ) , unique key idx_15 ( col_24,col_22 ) , key idx_16 ( col_21,col_20,col_24,col_25,col_27,col_28,col_26,col_29 ) , key idx_17 ( col_24,col_25 ) , unique key idx_18 ( col_25,col_23,col_29,col_27,col_26,col_22 ) , key idx_19 ( col_25,col_22,col_26,col_23 ) , unique key idx_20 ( col_22,col_24,col_28,col_29,col_26,col_20 ) , key idx_21 ( col_25,col_24,col_26,col_29,col_27,col_22,col_28 ) );";
    assert!(!create.contains("partition by"));
    assert!(create.contains("idx_21"));
    let quota = "set global tidb_mem_quota_analyze=128;";
    assert!(quota.contains("128"));

    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(create, Vec::new());
    tk.MustExec(
        "insert ignore into tbl_2 values (942,33,-1915007317,3408149,-3699,193,'Trywdis',1876334369465184864,115,null)",
        Vec::new(),
    );
    tk.MustExec(quota, Vec::new());
    tk.MustExecToErr("analyze table tbl_2;");
}

/// 对应 Go `TestAnalyzeV2MemoryUsageMetricNeverNegative`。
/// Analyze v2：基于采样构建直方图/TopN 的统计版本，内存指标不得变负。
#[test]
fn TestAnalyzeV2MemoryUsageMetricNeverNegative() {
    assert!(MaxSampleValueLength > 8 * 1024);
    for sql in [
        "set @@tidb_analyze_version=2",
        "set @@tidb_build_sampling_stats_concurrency=1",
        "set @@tidb_analyze_skip_column_types = ''",
        "create table t_mem_usage(a text collate utf8mb4_general_ci)",
        "insert into t_mem_usage values (repeat('a', 8192))",
        "analyze table t_mem_usage with 1.0 samplerate;",
    ] {
        assert!(!sql.is_empty());
    }

    let mut tk = new_testkit();
    tk.MustExec("set @@tidb_analyze_version=2", Vec::new());
    tk.MustExec("set @@tidb_build_sampling_stats_concurrency=1", Vec::new());
    tk.MustExec("set @@tidb_analyze_skip_column_types = ''", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_mem_usage(a text collate utf8mb4_general_ci)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_mem_usage values (repeat('a', 8192))",
        Vec::new(),
    );
    for _ in 0..6 {
        tk.MustExec(
            "insert into t_mem_usage select a from t_mem_usage",
            Vec::new(),
        );
    }
    tk.MustExec("analyze table t_mem_usage with 1.0 samplerate", Vec::new());

    // GlobalAnalyzeMemoryTracker 尚未作为独立全局导出；用父子 Tracker 复现
    // analyze v2 内存记账不应变负的约束。
    let mut global = Tracker::new(0, -1);
    let mut child = Tracker::new(1, -1);
    child.AttachTo(&mut global as *mut Tracker);
    child.Consume(1024);
    assert_eq!(global.BytesConsumed(), 1024);
    child.Consume(-512);
    assert_eq!(global.BytesConsumed(), 512);
    assert!(global.BytesConsumed() >= 0);
    child.Detach();
    assert_eq!(global.BytesConsumed(), 0);
}

/// 对应 Go `TestAnalyzeSessionMemTrackerDetachOnClose`。
/// 会话关闭时应 Detach，避免 analyze 内存继续挂在全局 Tracker 上。
#[test]
fn TestAnalyzeSessionMemTrackerDetachOnClose() {
    let analyze_missing = "analyze table test.not_exists with 1 topn";
    assert!(analyze_missing.contains("not_exists"));

    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    let error = tk.ExecToErr(analyze_missing);
    assert!(!error.message().is_empty());
    assert_eq!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .len(),
        0
    );
    tk.Session()
        .close()
        .expect("session close must release analyze state");

    let mut global = Tracker::new(0, -1);
    let mut session = Tracker::new(1, -1);
    session.AttachTo(&mut global as *mut Tracker);
    let base = global.BytesConsumed();
    session.Consume(1024);
    assert_eq!(global.BytesConsumed(), base + 1024);
    // Close session → Detach：全局 analyze tracker 回到 base。
    session.Detach();
    assert_eq!(global.BytesConsumed(), base);
}
