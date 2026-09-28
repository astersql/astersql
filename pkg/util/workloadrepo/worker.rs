// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库 Worker 核心类型与生命周期管理。
//
// 对应 Go `pkg/util/workloadrepo/worker.go`。定义仓库表描述、后端抽象
// （`RepositoryBackend`：执行 SQL、查列定义、etcd KV 等）、全局 Worker 单例，
// 以及启用/停用仓库、默认采集表清单与带重试的查询封装。

use chrono::{DateTime, Local};
use std::sync::{Arc, Mutex, OnceLock};

use crate::*;

/// 快照表：按全局 SNAP_ID 批量落库。
pub const snapshotTable: i32 = 0;
/// 采样表：周期性采集瞬时状态。
pub const samplingTable: i32 = 1;
/// 元数据表（如 HIST_SNAPSHOTS），使用预置 CREATE 语句。
pub const metadataTable: i32 = 2;

/// 一张待采集/落库的源表与目标历史表映射。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct repositoryTable {
    /// 源 schema，如 INFORMATION_SCHEMA。
    pub schema: String,
    /// 源表名。
    pub table: String,
    /// 表类型：snapshot / sampling / metadata。
    pub tableType: i32,
    /// 目标历史表名（通常 `HIST_` 前缀）。
    pub destTable: String,
    /// 可选 WHERE 过滤。
    pub whereClause: String,
    /// 预置 CREATE 语句（元数据表用）。
    pub createStmt: String,
    /// 缓存的 INSERT…SELECT 语句。
    pub insertStmt: String,
}

/// 源表列定义，用于拼建表与插入语句。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnDefinition {
    pub name: String,
    pub type_description: String,
    pub comment: String,
}

/// 查询参数/结果单元格取值。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    UInt(u64),
    Int(i64),
    String(String),
    Time(DateTime<Local>),
}
/// 一行查询结果。
pub type Row = Vec<Value>;

/// 仓库所需的会话/存储后端能力抽象。
pub trait RepositoryBackend: Send + Sync {
    /// 执行 SQL 并返回结果行。
    fn execute(&self, sql: &str, args: &[Value]) -> Result<Vec<Row>, String>;
    /// 读取源表列定义。
    fn source_columns(&self, schema: &str, table: &str) -> Result<Vec<ColumnDefinition>, String>;
    /// 目标表是否已存在。
    fn table_exists(&self, table: &str) -> bool;
    /// 列出表的分区名。
    fn partitions(&self, table: &str) -> Result<Vec<String>, String>;
    /// 当前 TiDB 实例 ID。
    fn instance_id(&self) -> Result<String, String>;
    /// 是否为 DDL owner（负责建表）。
    fn is_owner(&self) -> bool;
    /// etcd 是否可用（SNAP_ID 协调依赖）。
    fn etcd_available(&self) -> bool;
    /// etcd 创建键（已存在返回 false）。
    fn kv_create(&self, key: &str, value: &str) -> Result<bool, String>;
    /// etcd 读取键。
    fn kv_get(&self, key: &str) -> Result<Option<String>, String>;
    /// etcd CAS 更新。
    fn kv_cas(&self, key: &str, old: &str, new: &str) -> Result<bool, String>;
}

/// Worker 可变运行状态（启用标志、间隔、实例 ID 等）。
#[derive(Clone, Debug)]
pub(crate) struct WorkerState {
    pub(crate) enabled: bool,
    pub(crate) started: bool,
    pub(crate) instanceID: String,
    pub(crate) samplingInterval: i32,
    pub(crate) snapshotInterval: i32,
    pub(crate) retentionDays: i32,
}

/// 工作负载仓库 Worker：持有后端与待采集表清单。
pub struct worker {
    pub backend: Arc<dyn RepositoryBackend>,
    pub workloadTables: Mutex<Vec<repositoryTable>>,
    pub(crate) state: Mutex<WorkerState>,
}

/// 对外导出的 Worker 类型别名。
pub type WorkloadRepoWorker = worker;

/// 默认采集表：HIST_SNAPSHOTS 元数据 + 若干 INFORMATION_SCHEMA 快照/采样表。
pub fn defaultWorkloadTables() -> Vec<repositoryTable> {
    let mut tables = vec![repositoryTable {
        tableType: metadataTable,
        destTable: histSnapshotsTable.into(),
        createStmt: format!(
            "CREATE TABLE IF NOT EXISTS `{workloadSchema}`.`{histSnapshotsTable}` (\
             `SNAP_ID` INT UNSIGNED NOT NULL COMMENT 'Global unique identifier of the snapshot', \
             `BEGIN_TIME` DATETIME NOT NULL COMMENT 'Datetime that TiDB begins taking this snapshot.', \
             `END_TIME` DATETIME NULL COMMENT 'Datetime that TiDB finish taking this snapshot.', \
             `DB_VER` JSON NULL COMMENT 'Versions of TiDB, TiKV, PD at the moment', \
             `WR_VER` INT UNSIGNED NULL COMMENT 'Version to identify the compatibility of workload schema between releases.', \
             `SOURCE` VARCHAR(20) NULL COMMENT 'The program that initializes the snaphost. ', \
             `ERROR` TEXT DEFAULT NULL COMMENT 'extra messages are written if anything happens to block that snapshots.')"
        ),
        ..Default::default()
    }];
    // 快照类：索引/语句统计与客户端错误汇总。
    for table in [
        "TIDB_INDEX_USAGE",
        "TIDB_STATEMENTS_STATS",
        "CLIENT_ERRORS_SUMMARY_BY_HOST",
        "CLIENT_ERRORS_SUMMARY_BY_USER",
        "CLIENT_ERRORS_SUMMARY_GLOBAL",
    ] {
        tables.push(repositoryTable {
            schema: "INFORMATION_SCHEMA".into(),
            table: table.into(),
            tableType: snapshotTable,
            ..Default::default()
        });
    }
    // 采样类：进程列表、锁等待、事务、内存与死锁等瞬时视图。
    for table in [
        "PROCESSLIST",
        "DATA_LOCK_WAITS",
        "TIDB_TRX",
        "MEMORY_USAGE",
        "DEADLOCKS",
    ] {
        tables.push(repositoryTable {
            schema: "INFORMATION_SCHEMA".into(),
            table: table.into(),
            tableType: samplingTable,
            ..Default::default()
        });
    }
    tables
}

/// 用给定后端与表清单构造 Worker（默认间隔来自常量）。
pub fn initializeWorker(
    backend: Arc<dyn RepositoryBackend>,
    workloadTables: Vec<repositoryTable>,
) -> Arc<worker> {
    Arc::new(worker {
        backend,
        workloadTables: Mutex::new(workloadTables),
        state: Mutex::new(WorkerState {
            enabled: false,
            started: false,
            instanceID: String::new(),
            samplingInterval: defSamplingInterval,
            snapshotInterval: defSnapshotInterval,
            retentionDays: defRententionDays,
        }),
    })
}

/// 进程内全局 Worker 槽位。
static WORKER: OnceLock<Mutex<Option<Arc<worker>>>> = OnceLock::new();
fn globalWorker() -> &'static Mutex<Option<Arc<worker>>> {
    WORKER.get_or_init(|| Mutex::new(None))
}

/// 通过全局 Worker 触发一次 takeSnapshot；未启用则报错。
pub fn takeSnapshot() -> Result<u64, String> {
    let worker = globalWorker()
        .lock()
        .unwrap()
        .clone()
        .ok_or(errWorkloadNotStarted)?;
    if !worker.state.lock().unwrap().enabled {
        return Err(errWorkloadNotStarted.into());
    }
    worker.takeSnapshot()
}

/// 包初始化占位（与 Go init 对应，当前无操作）。
pub fn init() {}
/// 注册全局 Worker；若此前已启用则立即 start 新 Worker。
pub fn SetupRepository(newWorker: Arc<worker>) -> Result<(), String> {
    let enabled = globalWorker()
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|worker| worker.state.lock().unwrap().enabled);
    *globalWorker().lock().unwrap() = Some(Arc::clone(&newWorker));
    if enabled {
        newWorker.start()?;
    }
    Ok(())
}
/// 取出并停止全局 Worker。
pub fn StopRepository() {
    if let Some(worker) = globalWorker().lock().unwrap().take() {
        worker.stop();
    }
}

/// 执行一次查询（无重试）。
pub fn runQuery(
    backend: &dyn RepositoryBackend,
    sql: &str,
    args: &[Value],
) -> Result<Vec<Row>, String> {
    backend.execute(sql, args)
}
/// 最多重试 5 次执行查询，全部失败时拼接错误返回。
pub fn execRetry(
    backend: &dyn RepositoryBackend,
    sql: &str,
    args: &[Value],
) -> Result<Vec<Row>, String> {
    let mut errors = Vec::new();
    for _ in 0..5 {
        match runQuery(backend, sql, args) {
            Ok(rows) => return Ok(rows),
            Err(error) => errors.push(error),
        }
    }
    Err(errors.join("\n"))
}

impl worker {
    /// 返回可用后端会话（当前实现直接克隆 Arc）。
    pub fn getSessionWithRetry(&self) -> Arc<dyn RepositoryBackend> {
        Arc::clone(&self.backend)
    }
    /// 惰性读取并缓存实例 ID。
    pub fn readInstanceID(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.instanceID.is_empty() {
            state.instanceID = self.backend.instance_id()?;
        }
        Ok(())
    }
    /// 为空的 destTable 填充默认 `HIST_<源表>` 名称。
    pub fn fillInTableNames(&self) {
        for table in self.workloadTables.lock().unwrap().iter_mut() {
            if !table.table.is_empty() && table.destTable.is_empty() {
                table.destTable = format!("HIST_{}", table.table);
            }
        }
    }
    /// Owner 建表、校验表就绪、读取实例 ID，并将 started 置 true。
    pub fn startRepository(&self, now: DateTime<Local>) -> Result<(), String> {
        self.fillInTableNames();
        if self.backend.is_owner() {
            self.createAllTables(now)?;
        }
        if !self.checkTablesExists(now) {
            return Err("repository tables are not ready".into());
        }
        self.readInstanceID()?;
        self.state.lock().unwrap().started = true;
        Ok(())
    }
    /// 启用仓库：要求 etcd 可用，并完成 startRepository。
    pub fn start(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.enabled = true;
        if state.started {
            return Ok(());
        }
        if !self.backend.etcd_available() {
            return Err(errUnsupportedEtcdRequired.into());
        }
        drop(state);
        self.startRepository(Local::now())
    }
    /// 停用并清除 started 标志。
    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.enabled = false;
        state.started = false;
    }
    /// 按 dest 配置启停：`"table"` 启动，其它值停止。
    pub fn setRepositoryDest(&self, dst: &str) -> Result<(), String> {
        match validateDest(dst)?.as_str() {
            "table" => self.start(),
            _ => {
                self.stop();
                Ok(())
            }
        }
    }
    /// 是否已启用。
    pub fn enabled(&self) -> bool {
        self.state.lock().unwrap().enabled
    }
    /// 仓库基础设施是否已启动完成。
    pub fn started(&self) -> bool {
        self.state.lock().unwrap().started
    }
    /// 当前缓存的实例 ID。
    pub fn instanceID(&self) -> String {
        self.state.lock().unwrap().instanceID.clone()
    }
    /// 返回 (采样间隔, 快照间隔, 保留天数)。
    pub fn intervals(&self) -> (i32, i32, i32) {
        let state = self.state.lock().unwrap();
        (
            state.samplingInterval,
            state.snapshotInterval,
            state.retentionDays,
        )
    }
}
