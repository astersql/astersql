// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 会话管理器中的进程信息（ProcessInfo）快照与 SHOW PROCESSLIST / KILL 相关接口。
//
// `ProcessInfo` 描述一条连接当前正在执行的语句及其资源占用；
// `Manager` trait 供 `SHOW PROCESSLIST`、`INFORMATION_SCHEMA.PROCESSLIST` 与 `KILL` 调用。

#![allow(non_snake_case, non_upper_case_globals)]

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::{
    auth, cursor, disk, execdetails, mdldef, memory, mysql, ppcpuusage, resourcegroup, stmtctx,
    txninfo,
};

/// OOM alarm variables captured with a process snapshot.
/// OOM（内存耗尽）告警相关的会话变量快照，随进程信息一并导出。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OOMAlarmVariablesInfo {
    /// ANALYZE 统计信息版本号。
    pub SessionAnalyzeVersion: isize,
    /// 是否启用超限时的限流动作。
    pub SessionEnabledRateLimitAction: bool,
    /// 单条查询的内存配额（字节）。
    pub SessionMemQuotaQuery: i64,
}

/// Type-erases the associated types of the migrated resource-group checker
/// while retaining the concrete checker and Go's shared-interface semantics.
/// 类型擦除后的 runaway（失控查询）检查器，保留资源组检查语义。
pub trait ErasedRunawayChecker: Send + Sync {}

impl<T> ErasedRunawayChecker for T where T: resourcegroup::RunawayChecker + 'static {}

/// 从执行计划提取统计信息的回调：键为指标名，值为计数。
pub type StatsInfoFn = fn(&dyn Any) -> HashMap<String, u64>;

/// A value in SHOW PROCESSLIST or INFORMATION_SCHEMA.PROCESSLIST.
/// PROCESSLIST 行中的单元格取值（空、无符号、有符号、浮点或文本）。
#[derive(Clone, Debug, PartialEq)]
pub enum ProcessListValue {
    /// SQL NULL。
    Null,
    /// 无符号整数。
    Unsigned(u64),
    /// 有符号整数。
    Signed(i64),
    /// 浮点数。
    Float(f64),
    /// 文本。
    Text(String),
}

impl fmt::Display for ProcessListValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("<nil>"),
            Self::Unsigned(value) => value.fmt(formatter),
            Self::Signed(value) => value.fmt(formatter),
            Self::Float(value) => value.fmt(formatter),
            Self::Text(value) => formatter.write_str(value),
        }
    }
}

/// ProcessInfo is the snapshot used by SHOW PROCESSLIST.
///
/// Go pointer/interface fields and slice headers are represented by `Arc`, so
/// `Clone` below remains a shallow copy instead of silently deep-copying them.
/// SHOW PROCESSLIST 使用的连接进程快照；指针类字段用 `Arc` 做浅拷贝。
pub struct ProcessInfo {
    /// 当前语句开始时间。
    pub Time: SystemTime,
    /// 昂贵查询日志上次记录时间。
    pub ExpensiveLogTime: SystemTime,
    /// 昂贵事务日志上次记录时间。
    pub ExpensiveTxnLogTime: SystemTime,
    /// 当前事务创建时间。
    pub CurTxnCreateTime: SystemTime,
    /// 当前执行计划（类型擦除）。
    pub Plan: Option<Arc<dyn Any + Send + Sync>>,
    /// 游标跟踪器。
    pub CursorTracker: Option<Arc<cursor::CursorTracker>>,
    /// 语句上下文（StatementContext）。
    pub StmtCtx: Option<Arc<stmtctx::StatementContext>>,
    /// SQL CPU 用量采集。
    pub SQLCPUUsage: Option<Arc<ppcpuusage::SQLCPUUsages>>,
    /// StmtCtx 的引用计数，读写快照时用于并发安全。
    pub RefCountOfStmtCtx: Option<Arc<stmtctx::ReferenceCount>>,
    /// 内存跟踪器。
    pub MemTracker: Option<Arc<memory::Tracker>>,
    /// 磁盘溢出跟踪器。
    pub DiskTracker: Option<Arc<disk::Tracker>>,
    /// 失控查询检查器。
    pub RunawayChecker: Option<Arc<dyn ErasedRunawayChecker>>,
    /// 统计信息回调。
    pub StatsInfo: Option<StatsInfoFn>,
    /// 运行时统计收集器。
    pub RuntimeStatsColl: Option<Arc<execdetails::RuntimeStatsColl>>,
    /// 登录用户名。
    pub User: String,
    /// SQL digest（归一化指纹）。
    pub Digest: String,
    /// 客户端主机。
    pub Host: String,
    /// 当前数据库名。
    pub DB: String,
    /// 正在执行的 SQL 文本。
    pub Info: String,
    /// 客户端端口。
    pub Port: String,
    /// 资源组名称。
    pub ResourceGroupName: String,
    /// 会话别名。
    pub SessionAlias: String,
    /// 脱敏后的 SQL。
    pub RedactSQL: String,
    /// 简要二进制执行计划。
    pub BriefBinaryPlan: String,
    /// 涉及的索引名列表。
    pub IndexNames: Arc<Vec<String>>,
    /// 涉及的表 ID 列表。
    pub TableIDs: Arc<Vec<i64>>,
    /// OOM 告警变量快照。
    pub OOMAlarmVariablesInfo: OOMAlarmVariablesInfo,
    /// 连接 ID。
    pub ID: u64,
    /// 当前事务开始时间戳（StartTS，含物理时间）。
    pub CurTxnStartTS: u64,
    /// 最大执行时间（毫秒，0 表示不限）。
    pub MaxExecutionTime: u64,
    /// MySQL server status 位图。
    pub State: u16,
    /// MySQL 命令字节（如 COM_QUERY）。
    pub Command: u8,
}

/// 构造与 Go `time.Time` 零值等价的时间（公元 0001-01-01 UTC）。
fn go_zero_time() -> SystemTime {
    // Go's time.Time zero value is 0001-01-01 UTC.
    UNIX_EPOCH
        .checked_sub(Duration::from_secs(62_135_596_800))
        .unwrap_or(UNIX_EPOCH)
}

impl Default for ProcessInfo {
    fn default() -> Self {
        Self {
            Time: go_zero_time(),
            ExpensiveLogTime: go_zero_time(),
            ExpensiveTxnLogTime: go_zero_time(),
            CurTxnCreateTime: go_zero_time(),
            Plan: None,
            CursorTracker: None,
            StmtCtx: None,
            SQLCPUUsage: None,
            RefCountOfStmtCtx: None,
            MemTracker: None,
            DiskTracker: None,
            RunawayChecker: None,
            StatsInfo: None,
            RuntimeStatsColl: None,
            User: String::new(),
            Digest: String::new(),
            Host: String::new(),
            DB: String::new(),
            Info: String::new(),
            Port: String::new(),
            ResourceGroupName: String::new(),
            SessionAlias: String::new(),
            RedactSQL: String::new(),
            BriefBinaryPlan: String::new(),
            IndexNames: Arc::new(Vec::new()),
            TableIDs: Arc::new(Vec::new()),
            OOMAlarmVariablesInfo: OOMAlarmVariablesInfo::default(),
            ID: 0,
            CurTxnStartTS: 0,
            MaxExecutionTime: 0,
            State: 0,
            Command: 0,
        }
    }
}

/// RAII 守卫：离开作用域时 Decrease StmtCtx 引用计数。
struct ReferenceGuard<'a>(&'a stmtctx::ReferenceCount);

impl Drop for ReferenceGuard<'_> {
    fn drop(&mut self) {
        self.0.Decrease();
    }
}

impl ProcessInfo {
    /// Clone returns the same shallow snapshot copy as Go's `cp := *pi`.
    /// 浅拷贝快照，与 Go 对结构体解引用赋值行为一致。
    pub fn Clone(&self) -> ProcessInfo {
        self.clone_shallow()
    }

    /// Returns row data for SHOW [FULL] PROCESSLIST.
    /// 生成 SHOW [FULL] PROCESSLIST 的一行；`full` 为假时 Info 截断到 100 字符。
    pub fn ToRowForShow(&self, full: bool) -> Vec<ProcessListValue> {
        // Info / DB 为空时按 MySQL 习惯输出 NULL；非 FULL 时截断 SQL 文本。
        let info = if self.Info.is_empty() {
            ProcessListValue::Null
        } else if full {
            ProcessListValue::Text(self.Info.clone())
        } else {
            ProcessListValue::Text(self.Info.chars().take(100).collect())
        };
        let db = if self.DB.is_empty() {
            ProcessListValue::Null
        } else {
            ProcessListValue::Text(self.DB.clone())
        };
        // 有端口时拼成 host:port（IPv6 加方括号）。
        let host = if self.Port.is_empty() {
            self.Host.clone()
        } else {
            join_host_port(&self.Host, &self.Port)
        };

        vec![
            ProcessListValue::Unsigned(self.ID),
            ProcessListValue::Text(self.User.clone()),
            ProcessListValue::Text(host),
            db,
            ProcessListValue::Text(command_name(self.Command).to_owned()),
            ProcessListValue::Unsigned(elapsed_seconds(self.Time, SystemTime::now())),
            ProcessListValue::Text(serverStatus2Str(self.State)),
            info,
        ]
    }

    /// 将非 FULL 的 PROCESSLIST 行格式化为调试字符串。
    pub fn String(&self) -> String {
        let rows = self.ToRowForShow(false);
        format!(
            "{{id:{}, user:{}, host:{}, db:{}, command:{}, time:{}, state:{}, info:{}}}",
            rows[0], rows[1], rows[2], rows[3], rows[4], rows[5], rows[6], rows[7]
        )
    }

    /// 将事务 StartTS 解码为可读时间与原始 TS；StartTS 为 0 表示无事务。
    fn txnStartTs(&self, tz: Tz) -> String {
        if self.CurTxnStartTS == 0 {
            return String::new();
        }
        // TiDB StartTS 高 46 位为物理毫秒时间（右移 18 位得到）。
        let physical_millis = (self.CurTxnStartTS >> 18) as i64;
        let Some(physical_time) = DateTime::<Utc>::from_timestamp_millis(physical_millis) else {
            return String::new();
        };
        format!(
            "{}({})",
            physical_time
                .with_timezone(&tz)
                .format("%m-%d %H:%M:%S%.3f"),
            self.CurTxnStartTS
        )
    }

    /// Returns row data for INFORMATION_SCHEMA.PROCESSLIST.
    /// 生成 INFORMATION_SCHEMA.PROCESSLIST 扩展行（含内存、磁盘、CPU、资源组等）。
    pub fn ToRow(&self, tz: Tz) -> Vec<ProcessListValue> {
        let mut bytes_consumed = 0_i64;
        let mut disk_consumed = 0_i64;
        let mem_arbitration = ProcessListValue::Null;
        let mem_wait_arbitrate_start_time = ProcessListValue::Null;
        let mem_wait_arbitrate_bytes = ProcessListValue::Null;
        let mut affected_rows = ProcessListValue::Null;

        // 通过引用计数短暂持有 StmtCtx，避免与语句并发结束时产生数据竞争。
        if let Some(ref_count) = self.RefCountOfStmtCtx.as_deref()
            && ref_count.TryIncrease()
        {
            let _guard = ReferenceGuard(ref_count);
            if let Some(stmt_ctx) = self.StmtCtx.as_deref() {
                if let Some(mem_tracker) = self.MemTracker.as_deref() {
                    bytes_consumed = mem_tracker.BytesConsumed();
                }
                // The completed task-344 crate exposes the non-arbitrator build.
                // In that build Go's MemArbitration/WaitArbitrate branches both
                // return their zero values, represented by the three Nulls above.
                // 非仲裁器构建下内存仲裁相关列恒为 NULL。
                let _ = (&stmt_ctx.MemTracker, tz);
                if let Some(disk_tracker) = self.DiskTracker.as_deref() {
                    disk_consumed = disk_tracker.BytesConsumed();
                }
                affected_rows = ProcessListValue::Unsigned(stmt_ctx.AffectedRows());
            }
        }

        let cpu_usages = self
            .SQLCPUUsage
            .as_deref()
            .map(ppcpuusage::SQLCPUUsages::GetCPUUsages)
            .unwrap_or_default();
        // 在 SHOW FULL 基础列后追加 digest、内存、磁盘、事务 TS、资源组与 CPU。
        let mut row = self.ToRowForShow(true);
        row.extend([
            ProcessListValue::Text(self.Digest.clone()),
            ProcessListValue::Signed(bytes_consumed),
            mem_arbitration,
            mem_wait_arbitrate_start_time,
            mem_wait_arbitrate_bytes,
            ProcessListValue::Signed(disk_consumed),
            ProcessListValue::Text(self.txnStartTs(tz)),
            ProcessListValue::Text(self.ResourceGroupName.clone()),
            ProcessListValue::Text(self.SessionAlias.clone()),
            affected_rows,
            ProcessListValue::Signed(cpu_usages.TidbCPUTime.as_nanos() as i64),
            ProcessListValue::Signed(cpu_usages.TikvCPUTime.as_nanos() as i64),
        ]);
        row
    }

    /// 字段级浅拷贝，`Arc` 字段只增加引用计数。
    fn clone_shallow(&self) -> ProcessInfo {
        ProcessInfo {
            Time: self.Time,
            ExpensiveLogTime: self.ExpensiveLogTime,
            ExpensiveTxnLogTime: self.ExpensiveTxnLogTime,
            CurTxnCreateTime: self.CurTxnCreateTime,
            Plan: self.Plan.clone(),
            CursorTracker: self.CursorTracker.clone(),
            StmtCtx: self.StmtCtx.clone(),
            SQLCPUUsage: self.SQLCPUUsage.clone(),
            RefCountOfStmtCtx: self.RefCountOfStmtCtx.clone(),
            MemTracker: self.MemTracker.clone(),
            DiskTracker: self.DiskTracker.clone(),
            RunawayChecker: self.RunawayChecker.clone(),
            StatsInfo: self.StatsInfo,
            RuntimeStatsColl: self.RuntimeStatsColl.clone(),
            User: self.User.clone(),
            Digest: self.Digest.clone(),
            Host: self.Host.clone(),
            DB: self.DB.clone(),
            Info: self.Info.clone(),
            Port: self.Port.clone(),
            ResourceGroupName: self.ResourceGroupName.clone(),
            SessionAlias: self.SessionAlias.clone(),
            RedactSQL: self.RedactSQL.clone(),
            BriefBinaryPlan: self.BriefBinaryPlan.clone(),
            IndexNames: self.IndexNames.clone(),
            TableIDs: self.TableIDs.clone(),
            OOMAlarmVariablesInfo: self.OOMAlarmVariablesInfo.clone(),
            ID: self.ID,
            CurTxnStartTS: self.CurTxnStartTS,
            MaxExecutionTime: self.MaxExecutionTime,
            State: self.State,
            Command: self.Command,
        }
    }
}

/// 计算从 start 到 now 的整秒差；若 now 早于 start 则按负数再截断为 u64（与 Go 一致）。
fn elapsed_seconds(start: SystemTime, now: SystemTime) -> u64 {
    let signed_nanos = match now.duration_since(start) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    let go_duration_nanos = signed_nanos.clamp(i64::MIN as i128, i64::MAX as i128);
    (go_duration_nanos / 1_000_000_000) as u64
}

/// 拼接 host:port；含冒号的主机视为 IPv6，加方括号。
fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// 将 MySQL 命令字节映射为可读名称。
fn command_name(command: u8) -> &'static str {
    mysql::Command2Str
        .iter()
        .find_map(|&(value, name)| (value == command).then_some(name))
        .unwrap_or("")
}

/// 按位升序排列的 server status 标志及其展示文案（与 Go 一致）。
const ASC_SERVER_STATUS: &[(u16, &str)] = &[
    (mysql::ServerStatusInTrans, "in transaction"),
    (mysql::ServerStatusAutocommit, "autocommit"),
    (mysql::ServerMoreResultsExists, "more results exists"),
    (mysql::ServerStatusNoGoodIndexUsed, "no good index used"),
    (mysql::ServerStatusNoIndexUsed, "no index used"),
    (mysql::ServerStatusCursorExists, "cursor exists"),
    (mysql::ServerStatusLastRowSend, "last row send"),
    (mysql::ServerStatusDBDropped, "db dropped"),
    (
        mysql::ServerStatusNoBackslashEscaped,
        "no backslash escaped",
    ),
    (mysql::ServerStatusMetadataChanged, "metadata changed"),
    (mysql::ServerStatusWasSlow, "was slow"),
    (mysql::ServerPSOutParams, "ps out params"),
];

/// Converts the server-status bit field in the same ascending order as Go.
/// 将 server status 位图转为分号分隔的状态文案。
pub fn serverStatus2Str(state: u16) -> String {
    ASC_SERVER_STATUS
        .iter()
        .filter_map(|&(flag, name)| (state & flag != 0).then_some(name))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 内部会话句柄的类型擦除别名。
pub type InternalSession = Arc<dyn Any + Send + Sync>;

/// InfoSchema operations required from a session manager.
/// 会话管理器向 InfoSchema 暴露的内部会话登记与 DDL 相关协调接口。
pub trait InfoSchemaCoordinator: Send + Sync {
    /// 登记内部会话。
    fn StoreInternalSession(&self, se: InternalSession);
    /// 删除内部会话登记。
    fn DeleteInternalSession(&self, se: &InternalSession);
    /// 是否已登记该内部会话。
    fn ContainsInternalSession(&self, se: &InternalSession) -> bool;
    /// 当前内部会话数量。
    fn InternalSessionCount(&self) -> isize;
    /// 检查是否存在阻碍 DDL 的旧事务（MDL：元数据锁）。
    fn CheckOldRunningTxn(&self, jobs: &mut HashMap<i64, Arc<mdldef::JobMDL>>);
    /// 终止非 flashback cluster 相关连接。
    fn KillNonFlashbackClusterConn(&self);
}

/// 本地 kill 语句的正常关闭原因文案。
pub const NormalCloseMsgKillStmt: &str = "kill stmt";
/// 来自远端的 kill 语句正常关闭原因文案。
pub const NormalCloseMsgKillStmtFromRemote: &str = "kill stmt from remote";

/// 支持携带正常关闭原因文案的 kill 扩展接口。
pub trait NormalCloseKiller {
    /// 带正常关闭消息地终止连接或当前查询。
    fn KillWithNormalCloseMsg(
        &self,
        connectionID: u64,
        query: bool,
        maxExecutionTime: bool,
        runaway: bool,
        normalCloseMsg: &str,
    );
}

/// One `performance_schema.accounts` connection-lifecycle summary.
/// Empty user/host identities are represented as `None`, matching MySQL's
/// anonymous/background-row normalization without inventing internal threads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PerformanceSchemaAccountSummary {
    pub user: Option<String>,
    pub host: Option<String>,
    pub current_connections: u64,
    pub total_connections: u64,
}

/// Session-manager interface used by SHOW PROCESSLIST and KILL.
/// SHOW PROCESSLIST / KILL 使用的会话管理器主接口。
pub trait Manager: InfoSchemaCoordinator {
    /// 列出全部连接的进程快照。
    fn ShowProcessList(&self) -> HashMap<u64, Arc<ProcessInfo>>;
    /// 列出事务信息。
    fn ShowTxnList(&self) -> Vec<Arc<txninfo::TxnInfo>>;
    /// 按连接 ID 取进程信息。
    fn GetProcessInfo(&self, id: u64) -> Option<Arc<ProcessInfo>>;
    /// 终止指定连接；`query` 为真时只杀当前查询。
    fn Kill(&self, connectionID: u64, query: bool, maxExecutionTime: bool, runaway: bool);
    /// 终止全部连接。
    fn KillAllConnections(&self);
    /// 热更新 TLS 配置。
    fn UpdateTLSConfig(&self, cfg: Option<Arc<rustls::ServerConfig>>);
    /// 本实例 server ID。
    fn ServerID(&self) -> u64;
    /// 内部会话的开始时间戳列表。
    fn GetInternalSessionStartTSList(&self) -> Vec<u64>;
    /// 指定用户的连接属性。
    fn GetConAttrs(&self, user: &auth::UserIdentity) -> HashMap<u64, HashMap<String, String>>;
    /// 各连接的状态变量。
    fn GetStatusVars(&self) -> HashMap<u64, HashMap<String, String>>;

    /// Return external connection summaries for the required dynamic
    /// `performance_schema` account tables. Implementations that do not own a
    /// network listener expose no rows.
    fn GetPerformanceSchemaAccountSummaries(&self) -> Vec<PerformanceSchemaAccountSummary> {
        Vec::new()
    }

    /// 若实现支持 NormalCloseKiller 则返回扩展接口。
    fn as_normal_close_killer(&self) -> Option<&dyn NormalCloseKiller> {
        None
    }
}

/// Kills through the extended interface only when a non-empty close reason is
/// supplied; otherwise it follows the ordinary Manager.Kill path.
/// 关闭原因非空且实现支持扩展接口时走带文案的 kill，否则走普通 Kill。
pub fn KillWithNormalCloseMsg(
    sm: &dyn Manager,
    connectionID: u64,
    query: bool,
    maxExecutionTime: bool,
    runaway: bool,
    normalCloseMsg: &str,
) {
    if !normalCloseMsg.is_empty()
        && let Some(killer) = sm.as_normal_close_killer()
    {
        killer.KillWithNormalCloseMsg(
            connectionID,
            query,
            maxExecutionTime,
            runaway,
            normalCloseMsg,
        );
        return;
    }
    sm.Kill(connectionID, query, maxExecutionTime, runaway);
}
