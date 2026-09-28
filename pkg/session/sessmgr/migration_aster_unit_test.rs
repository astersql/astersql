// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// sessmgr 迁移基线单元测试。
//
// 校验进程列表行格式、浅拷贝语义、information_schema 行字段，
// 以及带 normal-close 原因的 Kill 路径。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use astersql_session_sessmgr::memory::{NewTracker, Tracker};
use astersql_session_sessmgr::mysql::{
    ComSleep, ServerMoreResultsExists, ServerStatusAutocommit, ServerStatusInTrans,
};
use astersql_session_sessmgr::ppcpuusage::{CPUUsages, SQLCPUUsages};
use astersql_session_sessmgr::stmtctx::{NewStmtCtx, ReferenceCount};
use astersql_session_sessmgr::{
    InfoSchemaCoordinator, InternalSession, KillWithNormalCloseMsg, Manager, NormalCloseKiller,
    NormalCloseMsgKillStmt, OOMAlarmVariablesInfo, ProcessInfo, ProcessListValue, serverStatus2Str,
};
use chrono::TimeZone;
use chrono_tz::UTC;

fn stats_info(_: &dyn std::any::Any) -> HashMap<String, u64> {
    HashMap::new()
}

#[test]
/// Go `int` follows the target pointer width.
fn oom_analyze_version_uses_go_int_width() {
    let version: isize = 2;
    let info = OOMAlarmVariablesInfo {
        SessionAnalyzeVersion: version,
        ..OOMAlarmVariablesInfo::default()
    };
    assert_eq!(info.SessionAnalyzeVersion, version);
}

#[test]
/// SHOW PROCESSLIST 行格式应与 Go 行为一致。
fn show_process_rows_match_go_behavior() {
    let info = ProcessInfo {
        Time: SystemTime::now(),
        ID: 1,
        User: "test".to_owned(),
        Host: "www".to_owned(),
        DB: "db".to_owned(),
        Command: ComSleep,
        State: ServerStatusInTrans | ServerStatusAutocommit,
        Info: "test".to_owned(),
        ..ProcessInfo::default()
    };

    let row = info.ToRowForShow(false);
    assert_eq!(row.len(), 8);
    assert_eq!(row[0], ProcessListValue::Unsigned(1));
    assert_eq!(row[1], ProcessListValue::Text("test".to_owned()));
    assert_eq!(row[2], ProcessListValue::Text("www".to_owned()));
    assert_eq!(row[3], ProcessListValue::Text("db".to_owned()));
    assert_eq!(row[4], ProcessListValue::Text("Sleep".to_owned()));
    assert!(matches!(row[5], ProcessListValue::Unsigned(0..=1)));
    assert_eq!(
        row[6],
        ProcessListValue::Text("in transaction; autocommit".to_owned())
    );
    assert_eq!(row[7], ProcessListValue::Text("test".to_owned()));
}

#[test]
/// 全文本、NULL DB、IPv6 Host:Port 展示与 Go 一致。
fn show_process_handles_full_text_nulls_and_ipv6_like_go() {
    let long_info = "界".repeat(101);
    let info = ProcessInfo {
        Host: "2001:db8::1".to_owned(),
        Port: "4000".to_owned(),
        Info: long_info.clone(),
        ..ProcessInfo::default()
    };

    let short = info.ToRowForShow(false);
    let full = info.ToRowForShow(true);
    assert_eq!(
        short[2],
        ProcessListValue::Text("[2001:db8::1]:4000".to_owned())
    );
    assert_eq!(short[3], ProcessListValue::Null);
    assert_eq!(short[7], ProcessListValue::Text("界".repeat(100)));
    assert_eq!(full[7], ProcessListValue::Text(long_info));

    let empty = ProcessInfo::default().ToRowForShow(true);
    assert_eq!(empty[3], ProcessListValue::Null);
    assert_eq!(empty[7], ProcessListValue::Null);
}

#[test]
/// Go `time.Since` saturates durations older than roughly 292 years.
fn show_process_zero_time_uses_go_duration_saturation() {
    let row = ProcessInfo::default().ToRowForShow(true);
    assert_eq!(
        row[5],
        ProcessListValue::Unsigned((i64::MAX as u64) / 1_000_000_000)
    );
}

#[test]
/// Clone 对指针/切片字段应为浅拷贝（Arc 共享）。
fn clone_is_shallow_for_go_pointer_and_slice_fields() {
    let stmt_ctx = Arc::from(NewStmtCtx());
    let refs = Arc::new(ReferenceCount::default());
    let mem: Arc<Tracker> = Arc::from(NewTracker(-1, -1));
    let indexes = Arc::new(vec!["idx_a".to_owned()]);
    let tables = Arc::new(vec![42]);
    let info = ProcessInfo {
        ID: 233,
        User: "PingCAP".to_owned(),
        Host: "127.0.0.1".to_owned(),
        DB: "Database".to_owned(),
        Info: "select * from table where a > 1".to_owned(),
        CurTxnStartTS: 23_333,
        StatsInfo: Some(stats_info),
        StmtCtx: Some(Arc::clone(&stmt_ctx)),
        RefCountOfStmtCtx: Some(Arc::clone(&refs)),
        MemTracker: Some(Arc::clone(&mem)),
        SessionAlias: "alias123".to_owned(),
        IndexNames: Arc::clone(&indexes),
        TableIDs: Arc::clone(&tables),
        ..ProcessInfo::default()
    };

    let cloned = info.Clone();
    assert_eq!(cloned.ID, info.ID);
    assert_eq!(cloned.User, info.User);
    assert_eq!(cloned.Host, info.Host);
    assert_eq!(cloned.DB, info.DB);
    assert_eq!(cloned.Info, info.Info);
    assert_eq!(cloned.CurTxnStartTS, info.CurTxnStartTS);
    assert_eq!(
        cloned.StatsInfo.map(|function| function as usize),
        info.StatsInfo.map(|function| function as usize)
    );
    assert_eq!(cloned.SessionAlias, info.SessionAlias);
    assert!(Arc::ptr_eq(cloned.StmtCtx.as_ref().unwrap(), &stmt_ctx));
    assert!(Arc::ptr_eq(
        cloned.RefCountOfStmtCtx.as_ref().unwrap(),
        &refs
    ));
    assert!(Arc::ptr_eq(cloned.MemTracker.as_ref().unwrap(), &mem));
    assert!(Arc::ptr_eq(&cloned.IndexNames, &indexes));
    assert!(Arc::ptr_eq(&cloned.TableIDs, &tables));
}

#[test]
/// information_schema 进程行应包含内存/磁盘/事务/CPU 字段。
fn information_schema_row_includes_trackers_txn_and_cpu() {
    let mut stmt_ctx = *NewStmtCtx();
    stmt_ctx.AddAffectedRows(9);
    stmt_ctx.MemTracker = Some(NewTracker(-1, -1));

    let mem: Arc<Tracker> = Arc::from(NewTracker(-1, -1));
    mem.Consume(123);
    let disk: Arc<Tracker> = Arc::from(NewTracker(-1, -1));
    disk.Consume(456);
    let cpu = Arc::new(SQLCPUUsages::default());
    cpu.SetCPUUsages(CPUUsages {
        TidbCPUTime: Duration::from_nanos(7),
        TikvCPUTime: Duration::from_nanos(11),
    });

    let physical = UTC
        .with_ymd_and_hms(2026, 7, 14, 12, 34, 56)
        .single()
        .unwrap()
        .timestamp_millis() as u64;
    let info = ProcessInfo {
        StmtCtx: Some(Arc::new(stmt_ctx)),
        RefCountOfStmtCtx: Some(Arc::new(ReferenceCount::default())),
        MemTracker: Some(mem),
        DiskTracker: Some(disk),
        SQLCPUUsage: Some(cpu),
        CurTxnStartTS: (physical << 18) | 17,
        Digest: "digest".to_owned(),
        ResourceGroupName: "rg".to_owned(),
        SessionAlias: "alias".to_owned(),
        ..ProcessInfo::default()
    };

    let row = info.ToRow(UTC);
    assert_eq!(row.len(), 20);
    assert_eq!(row[8], ProcessListValue::Text("digest".to_owned()));
    assert_eq!(row[9], ProcessListValue::Signed(123));
    assert_eq!(row[13], ProcessListValue::Signed(456));
    assert_eq!(
        row[14],
        ProcessListValue::Text(format!("07-14 12:34:56.000({})", info.CurTxnStartTS))
    );
    assert_eq!(row[17], ProcessListValue::Unsigned(9));
    assert_eq!(row[18], ProcessListValue::Signed(7));
    assert_eq!(row[19], ProcessListValue::Signed(11));
}

#[test]
/// 服务器状态位转字符串顺序与 Go 一致，忽略未知位。
fn server_status_uses_go_order_and_ignores_unknown_bits() {
    assert_eq!(
        serverStatus2Str(
            ServerMoreResultsExists | ServerStatusAutocommit | ServerStatusInTrans | 0x8000
        ),
        "in transaction; autocommit; more results exists"
    );
}

#[derive(Default)]
/// 记录 Kill 调用参数的 Manager mock。
struct MockManager {
    calls: Mutex<Vec<String>>,
    supports_normal_close: bool,
}

impl InfoSchemaCoordinator for MockManager {
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

impl NormalCloseKiller for MockManager {
    fn KillWithNormalCloseMsg(
        &self,
        connectionID: u64,
        query: bool,
        maxExecutionTime: bool,
        runaway: bool,
        normalCloseMsg: &str,
    ) {
        self.calls.lock().unwrap().push(format!(
            "normal:{connectionID}:{query}:{maxExecutionTime}:{runaway}:{normalCloseMsg}"
        ));
    }
}

impl Manager for MockManager {
    fn ShowProcessList(&self) -> HashMap<u64, Arc<ProcessInfo>> {
        HashMap::new()
    }
    fn ShowTxnList(&self) -> Vec<Arc<astersql_session_sessmgr::txninfo::TxnInfo>> {
        Vec::new()
    }
    fn GetProcessInfo(&self, _: u64) -> Option<Arc<ProcessInfo>> {
        None
    }
    fn Kill(&self, connectionID: u64, query: bool, maxExecutionTime: bool, runaway: bool) {
        self.calls.lock().unwrap().push(format!(
            "plain:{connectionID}:{query}:{maxExecutionTime}:{runaway}"
        ));
    }
    fn KillAllConnections(&self) {}
    fn UpdateTLSConfig(&self, _: Option<Arc<rustls::ServerConfig>>) {}
    fn ServerID(&self) -> u64 {
        0
    }
    fn GetInternalSessionStartTSList(&self) -> Vec<u64> {
        Vec::new()
    }
    fn GetConAttrs(
        &self,
        _: &astersql_session_sessmgr::auth::UserIdentity,
    ) -> HashMap<u64, HashMap<String, String>> {
        HashMap::new()
    }
    fn GetStatusVars(&self) -> HashMap<u64, HashMap<String, String>> {
        HashMap::new()
    }
    fn as_normal_close_killer(&self) -> Option<&dyn NormalCloseKiller> {
        self.supports_normal_close
            .then_some(self as &dyn NormalCloseKiller)
    }
}

#[test]
/// 非空 normal-close 消息走扩展 Killer；空串回退普通 Kill。
fn normal_close_reason_uses_extended_killer_only_when_non_empty() {
    let manager = MockManager {
        supports_normal_close: true,
        ..MockManager::default()
    };

    KillWithNormalCloseMsg(&manager, 7, true, false, true, NormalCloseMsgKillStmt);
    KillWithNormalCloseMsg(&manager, 8, false, true, false, "");

    assert_eq!(
        *manager.calls.lock().unwrap(),
        [
            "normal:7:true:false:true:kill stmt",
            "plain:8:false:true:false",
        ]
    );

    let plain_manager = MockManager::default();
    KillWithNormalCloseMsg(&plain_manager, 9, true, true, false, NormalCloseMsgKillStmt);
    assert_eq!(
        *plain_manager.calls.lock().unwrap(),
        ["plain:9:true:true:false"]
    );
}
