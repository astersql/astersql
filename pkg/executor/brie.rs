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

// BRIE（Backup / Restore）SQL 执行器与全局任务队列。
//
// 对应 Go 的 BRIE 执行路径：解析 BACKUP/RESTORE/SHOW BACKUP META 等语句，
// 经全局 `brieQueue` 串行调度任务，通过 `brieRuntime` / `tidbGlue` 调用
// 真实备份恢复实现。任务进度、取消与过期清理均在本模块维护。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 清理过期任务的最小间隔（10 分钟）。
pub const clearInterval: Duration = Duration::from_secs(10 * 60);
/// 任务完成后保留时长（30 分钟），超时可从队列清除。
pub const outdatedDuration: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
/// BRIE 路径统一错误类型。
pub struct BrieError(pub String);

impl fmt::Display for BrieError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BrieError {}

/// BRIE 操作结果别名。
pub type BrieResult<T = ()> = Result<T, BrieError>;

/// 当前 Unix 毫秒时间戳。
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// SQL 时间戳包装：毫秒值 + 是否有效。
pub struct sqlTime {
    pub unixMillis: i64,
    pub valid: bool,
}

impl sqlTime {
    fn now() -> Self {
        Self {
            unixMillis: now_millis(),
            valid: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 结果行单元格的简单 datum 表示。
pub enum datum {
    String(String),
    Unsigned(u64),
    Integer(i64),
    Float(f64),
    Time(sqlTime),
    Null,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// BRIE 执行器输出的结果行缓冲。
pub struct resultChunk {
    pub rows: Vec<Vec<datum>>,
}

impl resultChunk {
    /// 清空所有行。
    pub fn reset(&mut self) {
        self.rows.clear();
    }

    /// 追加一行。
    pub fn push_row(&mut self, row: Vec<datum>) {
        self.rows.push(row);
    }
}

#[derive(Clone, Debug, Default)]
/// 可取消任务上下文，支持父子取消传播。
pub struct taskContext {
    canceled: Arc<AtomicBool>,
    parent: Option<Arc<taskContext>>,
}

impl taskContext {
    /// 由父上下文派生子上下文。
    pub fn child(parent: taskContext) -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
            parent: Some(Arc::new(parent)),
        }
    }

    /// 标记本上下文已取消。
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::Release);
    }

    /// 自身或祖先是否已取消。
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_canceled())
    }

    /// 取消对应的错误对象。
    pub fn error(&self) -> BrieError {
        BrieError("context canceled".into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 进度快照中的命令名与总量。
pub struct brieTaskProgressState {
    pub cmd: String,
    pub total: i64,
}

/// 任务进度：原子当前值 + 互斥保护的命令/总量状态。
pub struct brieTaskProgress {
    current: AtomicI64,
    state: Mutex<brieTaskProgressState>,
}

impl brieTaskProgress {
    /// 构造处于 Wait 状态的进度对象。
    fn waiting() -> Self {
        Self {
            current: AtomicI64::new(0),
            state: Mutex::new(brieTaskProgressState {
                cmd: "Wait".into(),
                total: 1,
            }),
        }
    }

    /// 当前进度 +1。
    pub fn Inc(&self) {
        self.current.fetch_add(1, Ordering::AcqRel);
    }

    /// 当前进度增加 cnt。
    pub fn IncBy(&self, cnt: i64) {
        self.current.fetch_add(cnt, Ordering::AcqRel);
    }

    /// 读取当前进度值。
    pub fn GetCurrent(&self) -> i64 {
        self.current.load(Ordering::Acquire)
    }

    /// 关闭进度：未达总量则标记 Canceled，并将 current 置为 total。
    /// 关闭会话。
    pub fn Close(&self) {
        let mut state = self.state.lock().expect("BRIE progress mutex poisoned");
        if self.current.load(Ordering::Acquire) < state.total {
            state.cmd.push_str(" Canceled");
        }
        self.current.store(state.total, Ordering::Release);
    }

    /// 开始新阶段：重置命令名、总量与当前值。
    fn start(&self, cmd: &str, total: i64) {
        let mut state = self.state.lock().expect("BRIE progress mutex poisoned");
        state.cmd = cmd.into();
        state.total = total;
        self.current.store(0, Ordering::Release);
    }

    /// 返回 (cmd, total, current) 快照。
    fn snapshot(&self) -> (String, i64, i64) {
        let state = self.state.lock().expect("BRIE progress mutex poisoned");
        (
            state.cmd.clone(),
            state.total,
            self.current.load(Ordering::Acquire),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// BRIE 语句种类。
pub enum brieKind {
    Backup,
    Restore,
    ShowBackupMeta,
    ShowQuery,
    CancelJob,
}

impl fmt::Display for brieKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Backup => "BACKUP",
            Self::Restore => "RESTORE",
            Self::ShowBackupMeta => "SHOW BACKUP META",
            Self::ShowQuery => "SHOW BRIE QUERY",
            Self::CancelJob => "CANCEL BRIE JOB",
        })
    }
}

#[derive(Clone, Debug)]
/// 队列中登记的任务元信息（查询文本、时间戳、存储、TSO 等）。
pub struct brieTaskInfo {
    pub id: u64,
    pub query: String,
    pub queueTime: sqlTime,
    pub execTime: sqlTime,
    pub finishTime: sqlTime,
    pub kind: brieKind,
    pub storage: String,
    pub connID: u64,
    pub backupTS: u64,
    pub restoreTS: u64,
    pub archiveSize: u64,
    pub message: String,
}

impl brieTaskInfo {
    /// 按种类构造空任务信息。
    fn new(kind: brieKind) -> Self {
        Self {
            id: 0,
            query: String::new(),
            queueTime: sqlTime::default(),
            execTime: sqlTime::default(),
            finishTime: sqlTime::default(),
            kind,
            storage: String::new(),
            connID: 0,
            backupTS: 0,
            restoreTS: 0,
            archiveSize: 0,
            message: String::new(),
        }
    }
}

/// 队列项：任务信息、进度与可取消上下文。
pub struct brieQueueItem {
    pub info: Arc<Mutex<brieTaskInfo>>,
    pub progress: Arc<brieTaskProgress>,
    pub context: taskContext,
}

/// 全局 BRIE 任务队列：串行 worker、等待队列与过期清理。
pub struct brieQueue {
    nextID: AtomicU64,
    tasks: Mutex<BTreeMap<u64, Arc<brieQueueItem>>>,
    lastClearMillis: AtomicI64,
    workerBusy: Mutex<bool>,
    workerReady: Condvar,
    waitingTaskIDs: Mutex<VecDeque<u64>>,
}

impl brieQueue {
    /// 构造空队列。
    fn new() -> Self {
        Self {
            nextID: AtomicU64::new(0),
            tasks: Mutex::new(BTreeMap::new()),
            lastClearMillis: AtomicI64::new(0),
            workerBusy: Mutex::new(false),
            workerReady: Condvar::new(),
            waitingTaskIDs: Mutex::new(VecDeque::new()),
        }
    }

    /// 注册任务并加入等待队列，返回子上下文与任务 ID。
    pub fn registerTask(
        &self,
        context: taskContext,
        info: Arc<Mutex<brieTaskInfo>>,
    ) -> (taskContext, u64) {
        let task_context = taskContext::child(context);
        let item = Arc::new(brieQueueItem {
            info: info.clone(),
            progress: Arc::new(brieTaskProgress::waiting()),
            context: task_context.clone(),
        });
        let task_id = self.nextID.fetch_add(1, Ordering::AcqRel) + 1;
        self.tasks
            .lock()
            .expect("BRIE task map mutex poisoned")
            .insert(task_id, item);
        self.waitingTaskIDs
            .lock()
            .expect("BRIE waiting queue mutex poisoned")
            .push_back(task_id);
        info.lock().expect("BRIE task info mutex poisoned").id = task_id;
        (task_context, task_id)
    }

    /// 按 ID 查询任务信息快照。
    pub fn queryTask(&self, taskID: u64) -> Option<brieTaskInfo> {
        self.tasks
            .lock()
            .expect("BRIE task map mutex poisoned")
            .get(&taskID)
            .map(|item| {
                item.info
                    .lock()
                    .expect("BRIE task info mutex poisoned")
                    .clone()
            })
    }

    /// 阻塞直到成为队首且 worker 空闲，获取进度句柄开始执行。
    pub fn acquireTask(
        &self,
        taskContext: &taskContext,
        taskID: u64,
    ) -> BrieResult<Arc<brieTaskProgress>> {
        // 仅当本任务在等待队列队首且 worker 空闲时获得执行权
        let mut busy = self.workerBusy.lock().expect("BRIE worker mutex poisoned");
        loop {
            if taskContext.is_canceled() {
                return Err(taskContext.error());
            }
            let is_front = self
                .waitingTaskIDs
                .lock()
                .expect("BRIE waiting queue mutex poisoned")
                .front()
                .is_some_and(|front| *front == taskID);
            if !*busy && is_front {
                *busy = true;
                self.waitingTaskIDs
                    .lock()
                    .expect("BRIE waiting queue mutex poisoned")
                    .pop_front();
                let item = self
                    .tasks
                    .lock()
                    .expect("BRIE task map mutex poisoned")
                    .get(&taskID)
                    .cloned();
                if let Some(item) = item {
                    return Ok(item.progress.clone());
                }
                *busy = false;
                self.workerReady.notify_one();
                return Err(BrieError(format!(
                    "backup/restore task {taskID} is canceled"
                )));
            }
            let (next, _) = self
                .workerReady
                .wait_timeout(busy, Duration::from_millis(20))
                .expect("BRIE worker mutex poisoned while waiting");
            busy = next;
        }
    }

    /// 释放 worker，唤醒下一个等待者。
    pub fn releaseTask(&self) {
        let mut busy = self.workerBusy.lock().expect("BRIE worker mutex poisoned");
        *busy = false;
        self.workerReady.notify_one();
    }

    /// 取消指定任务；不存在返回 false。
    pub fn cancelTask(&self, taskID: u64) -> bool {
        let item = self
            .tasks
            .lock()
            .expect("BRIE task map mutex poisoned")
            .get(&taskID)
            .cloned();
        let Some(item) = item else {
            return false;
        };
        item.context.cancel();
        item.progress.Close();
        self.waitingTaskIDs
            .lock()
            .expect("BRIE waiting queue mutex poisoned")
            .retain(|waiting_id| *waiting_id != taskID);
        self.workerReady.notify_all();
        true
    }

    /// 按间隔清理已完成且超过 outdatedDuration 的任务。
    pub fn clearTask(&self) {
        let now = now_millis();
        let last = self.lastClearMillis.load(Ordering::Acquire);
        if last != 0 && now - last < clearInterval.as_millis() as i64 {
            return;
        }
        self.lastClearMillis.store(now, Ordering::Release);
        self.tasks
            .lock()
            .expect("BRIE task map mutex poisoned")
            .retain(|_, item| {
                let finish = item
                    .info
                    .lock()
                    .expect("BRIE task info mutex poisoned")
                    .finishTime
                    .clone();
                !finish.valid || now - finish.unixMillis <= outdatedDuration.as_millis() as i64
            });
    }

    /// 返回当前全部队列项快照。
    fn items(&self) -> Vec<Arc<brieQueueItem>> {
        self.tasks
            .lock()
            .expect("BRIE task map mutex poisoned")
            .values()
            .cloned()
            .collect()
    }
}

static globalBRIEQueue: LazyLock<RwLock<Arc<brieQueue>>> =
    LazyLock::new(|| RwLock::new(Arc::new(brieQueue::new())));

/// 取得当前全局队列 Arc。
fn current_queue() -> Arc<brieQueue> {
    globalBRIEQueue
        .read()
        .expect("global BRIE queue lock poisoned")
        .clone()
}

/// 测试用：重置全局队列为空。
pub fn ResetGlobalBRIEQueueForTest() {
    *globalBRIEQueue
        .write()
        .expect("global BRIE queue lock poisoned") = Arc::new(brieQueue::new());
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TLS 证书配置。
pub struct tlsConfig {
    pub ca: String,
    pub cert: String,
    pub key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 全局集群配置：PD 地址、TLS、存储类型。
pub struct globalConfig {
    pub pdAddresses: Vec<String>,
    pub tls: tlsConfig,
    pub storeType: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 规范化后的存储 URL 与 scheme。
pub struct normalizedStorage {
    pub url: String,
    pub scheme: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 备份加密方法与密钥。
pub struct cipherInfo {
    pub method: String,
    pub key: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 备份/恢复共用配置（限速、并发、校验和、过滤等）。
pub struct commonConfig {
    pub pdAddresses: Vec<String>,
    pub tls: tlsConfig,
    pub storage: String,
    pub rateLimit: u64,
    pub concurrency: u32,
    pub checksum: bool,
    pub sendCredentials: bool,
    pub checksumConcurrency: usize,
    pub cipher: cipherInfo,
    pub filterStrings: Vec<String>,
    pub caseInsensitiveFilter: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 备份专用配置：增量 TSO、压缩、是否忽略统计等。
pub struct backupConfig {
    pub common: commonConfig,
    pub lastBackupTS: u64,
    pub timeAgoNanos: u64,
    pub backupTS: u64,
    pub compression: String,
    pub compressionLevel: i32,
    pub ignoreStats: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 恢复专用配置：在线恢复、TiFlash、系统表、加载统计等。
pub struct restoreConfig {
    pub common: commonConfig,
    pub online: bool,
    pub waitTiFlashReady: bool,
    pub withSystemTable: bool,
    pub loadStats: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// BRIE 语句选项类型枚举。
pub enum brieOptionType {
    RateLimit,
    Concurrency,
    Checksum,
    SendCredentials,
    ChecksumConcurrency,
    EncryptionKeyFile,
    EncryptionMethod,
    LastBackupTS,
    LastBackupTSO,
    BackupTimeAgo,
    BackupTSO,
    BackupTS,
    Compression,
    CompressionLevel,
    IgnoreStats,
    Online,
    WaitTiFlashReady,
    WithSystemTable,
    LoadStats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条语句选项及其数值/字符串载荷。
pub struct brieOption {
    pub optionType: brieOptionType,
    pub uintValue: u64,
    pub stringValue: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表级过滤：schema.table。
pub struct tableFilter {
    pub schema: String,
    pub table: String,
}

#[derive(Clone, Debug)]
/// 解析后的 BRIE 语句：种类、存储、任务 ID、选项与过滤范围。
pub struct brieStmt {
    pub kind: brieKind,
    pub storage: String,
    pub jobID: i64,
    pub options: Vec<brieOption>,
    pub tables: Vec<tableFilter>,
    pub schemas: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// SHOW BACKUP META 配置。
pub struct showConfig {
    pub storage: String,
    pub cipher: cipherInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// backupmeta 中单表统计。
pub struct backupMetaTable {
    pub databaseName: String,
    pub tableName: String,
    pub kvCount: u64,
    pub kvSize: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// backupmeta 整体：版本范围与表列表。
pub struct backupMetadata {
    pub startVersion: u64,
    pub endVersion: u64,
    pub tables: Vec<backupMetaTable>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 库信息摘要。
pub struct databaseInfo {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表信息摘要。
pub struct tableInfo {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Placement Policy 及 SHOW CREATE 文本。
pub struct placementPolicyInfo {
    pub id: i64,
    pub name: String,
    pub showCreateSQL: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 建表选项名值对。
pub struct createTableOption {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ALTER TABLE MODE 参数。
pub struct alterTableModeArgs {
    pub schemaID: i64,
    pub tableID: i64,
    pub tableMode: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// REFRESH META 参数。
pub struct refreshMetaArgs {
    pub schemaID: i64,
    pub tableID: i64,
    pub involvedDatabase: String,
    pub involvedTable: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Glue 客户端类型（当前仅 SQL）。
pub enum brieClient {
    Sql,
}

/// BRIE 运行时边界：配置、存储规范化、执行备份/恢复与会话 DDL。
pub trait brieRuntime: Send + Sync {
    fn global_config(&self) -> globalConfig;
    fn sem_v1_enabled(&self) -> bool;
    fn normalize_storage_url(
        &self,
        raw: &str,
        common: &mut commonConfig,
    ) -> BrieResult<normalizedStorage>;
    fn restore_brie_query(&self, statement: &brieStmt) -> BrieResult<String>;
    fn read_cipher_key_file(&self, path: &str) -> BrieResult<Vec<u8>>;
    fn parse_timestamp(&self, value: &str, timezone: &str) -> BrieResult<u64>;
    fn connection_id(&self) -> u64;
    fn timezone(&self) -> String;
    fn check_killed(&self) -> BrieResult;
    fn append_job_not_found_warning(&self, task_id: u64);
    fn read_backup_metadata(
        &self,
        context: &taskContext,
        config: &showConfig,
    ) -> BrieResult<backupMetadata>;
    fn format_tso_time(&self, tso: u64, timezone: &str) -> BrieResult<sqlTime>;
    fn run_backup(
        &self,
        context: &taskContext,
        glue: &mut tidbGlue,
        config: &backupConfig,
    ) -> BrieResult;
    fn run_restore(
        &self,
        context: &taskContext,
        glue: &mut tidbGlue,
        config: &restoreConfig,
    ) -> BrieResult;
    fn domain_handle(&self, session_id: u64) -> BrieResult<u64>;
    fn storage_handle(&self, session_id: u64) -> BrieResult<u64>;
    fn create_session(&self, parent_session_id: u64) -> BrieResult<u64>;
    fn close_session(&self, session_id: u64);
    fn tidb_info(&self) -> String;
    fn execute_restricted_brie_sql(&self, session_id: u64, sql: &str) -> BrieResult;
    fn execute_internal_brie_sql(
        &self,
        session_id: u64,
        sql: &str,
        arguments: &[datum],
    ) -> BrieResult;
    fn create_database(&self, session_id: u64, schema: &databaseInfo) -> BrieResult;
    fn create_table(
        &self,
        session_id: u64,
        database_name: &str,
        table: &tableInfo,
        options: &[createTableOption],
    ) -> BrieResult;
    fn create_tables(
        &self,
        session_id: u64,
        tables: &BTreeMap<String, Vec<tableInfo>>,
        options: &[createTableOption],
    ) -> BrieResult;
    fn query_string(&self, session_id: u64) -> String;
    fn set_query_string(&self, session_id: u64, query: &str);
    fn create_placement_policy_ignore_existing(
        &self,
        session_id: u64,
        policy: &placementPolicyInfo,
    ) -> BrieResult;
    fn global_table_value(&self, session_id: u64, name: &str) -> BrieResult<String>;
    fn global_system_variable(&self, session_id: u64, name: &str) -> BrieResult<String>;
    fn alter_table_mode(&self, session_id: u64, args: &alterTableModeArgs) -> BrieResult;
    fn refresh_meta(&self, session_id: u64, args: &refreshMetaArgs) -> BrieResult;
    fn log_one_shot_session_closed(&self);
}

/// 由语句构建 BRIE 执行器的构建器。
pub struct executorBuilder {
    pub runtime: Arc<dyn brieRuntime>,
    pub sessionID: u64,
    pub error: Option<BrieError>,
}

impl executorBuilder {
    /// 按会话时区解析时间戳字符串为 TSO。
    pub fn parseTSString(&self, ts: &str) -> BrieResult<u64> {
        self.runtime.parse_timestamp(ts, &self.runtime.timezone())
    }

    /// 构建执行器；失败时把错误记入 builder.error。
    pub fn buildBRIE(&mut self, statement: &mut brieStmt) -> Option<brieExecutor> {
        match self.try_build_brie(statement) {
            Ok(executor) => Some(executor),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    /// 按语句种类构建 Show/Cancel 或 Backup/Restore 主执行器。
    fn try_build_brie(&self, statement: &mut brieStmt) -> BrieResult<brieExecutor> {
        // Show/Cancel：一次性或轻量执行器；Backup/Restore 继续下方配置
        match statement.kind {
            brieKind::ShowBackupMeta => {
                return Ok(execOnce(brieExecutor::ShowMeta(showMetaExec {
                    showConfig: buildShowMetadataConfigFrom(statement),
                    runtime: self.runtime.clone(),
                })));
            }
            brieKind::ShowQuery => {
                return Ok(execOnce(brieExecutor::ShowQuery(showQueryExec {
                    targetID: statement.jobID as u64,
                })));
            }
            brieKind::CancelJob => {
                return Ok(brieExecutor::Cancel(cancelJobExec {
                    targetID: statement.jobID as u64,
                    runtime: self.runtime.clone(),
                }));
            }
            brieKind::Backup | brieKind::Restore => {}
        }

        // 规范化存储 URL，校验 SEM 与 tikv store，合并选项与过滤
        let global = self.runtime.global_config();
        let mut common = commonConfig {
            pdAddresses: global.pdAddresses,
            tls: global.tls,
            ..Default::default()
        };
        let normalized_storage = self
            .runtime
            .normalize_storage_url(&statement.storage, &mut common)
            .map_err(|error| BrieError(format!("invalid destination URL: {error}")))?;
        let scheme = normalized_storage.scheme.as_str();
        if self.runtime.sem_v1_enabled() && matches!(scheme, "hdfs" | "local" | "file" | "") {
            return Err(BrieError(format!(
                "{scheme} storage is not supported with SEM"
            )));
        }
        if global.storeType != "tikv" {
            return Err(BrieError(format!(
                "{} requires tikv store, not {}",
                statement.kind, global.storeType
            )));
        }
        let storage = normalized_storage.url;
        common.storage = storage.clone();
        for option in &statement.options {
            match option.optionType {
                brieOptionType::RateLimit => common.rateLimit = option.uintValue,
                brieOptionType::Concurrency => common.concurrency = option.uintValue as u32,
                brieOptionType::Checksum => common.checksum = option.uintValue != 0,
                brieOptionType::SendCredentials => common.sendCredentials = option.uintValue != 0,
                brieOptionType::ChecksumConcurrency => {
                    common.checksumConcurrency = option.uintValue as usize
                }
                brieOptionType::EncryptionKeyFile => {
                    common.cipher.key = self.runtime.read_cipher_key_file(&option.stringValue)?;
                }
                brieOptionType::EncryptionMethod => {
                    if !matches!(
                        option.stringValue.as_str(),
                        "aes128-ctr" | "aes192-ctr" | "aes256-ctr" | "plaintext"
                    ) {
                        return Err(BrieError(format!(
                            "unsupported encryption method: {}",
                            option.stringValue
                        )));
                    }
                    common.cipher.method = option.stringValue.clone();
                }
                _ => {}
            }
        }
        // 表过滤 > schema 过滤 > 默认 *.*
        if !statement.tables.is_empty() {
            common.filterStrings = statement
                .tables
                .iter()
                .map(|table| format!("`{}`.`{}`", table.schema, table.table))
                .collect();
        } else if !statement.schemas.is_empty() {
            common.filterStrings = statement
                .schemas
                .iter()
                .map(|schema| format!("`{schema}`.*"))
                .collect();
        } else {
            common.filterStrings = vec!["*.*".into()];
        }
        common.caseInsensitiveFilter = true;
        statement.storage = storage.clone();
        let mut info = brieTaskInfo::new(statement.kind);
        info.storage = storage;
        info.query = restoreQuery(self.runtime.as_ref(), statement);
        let info = Arc::new(Mutex::new(info));

        let mut executor = BRIEExec {
            backupCfg: None,
            restoreCfg: None,
            showConfig: None,
            info: Some(info),
            runtime: self.runtime.clone(),
            sessionID: self.sessionID,
        };
        match statement.kind {
            // 填充备份专用选项（增量 TSO、压缩等）
            brieKind::Backup => {
                let mut config = backupConfig {
                    common,
                    ..Default::default()
                };
                for option in &statement.options {
                    match option.optionType {
                        brieOptionType::LastBackupTS => {
                            config.lastBackupTS = self.parseTSString(&option.stringValue)?
                        }
                        brieOptionType::LastBackupTSO => config.lastBackupTS = option.uintValue,
                        brieOptionType::BackupTimeAgo => config.timeAgoNanos = option.uintValue,
                        brieOptionType::BackupTSO => config.backupTS = option.uintValue,
                        brieOptionType::BackupTS => {
                            config.backupTS = self.parseTSString(&option.stringValue)?
                        }
                        brieOptionType::Compression => {
                            if !matches!(option.stringValue.as_str(), "zstd" | "snappy" | "lz4") {
                                return Err(BrieError(format!(
                                    "unsupported compression type: {}",
                                    option.stringValue
                                )));
                            }
                            config.compression = option.stringValue.clone();
                        }
                        brieOptionType::CompressionLevel => {
                            config.compressionLevel = option.uintValue as i32
                        }
                        brieOptionType::IgnoreStats => config.ignoreStats = option.uintValue != 0,
                        _ => {}
                    }
                }
                executor.backupCfg = Some(config);
            }
            // 填充恢复专用选项
            brieKind::Restore => {
                let mut config = restoreConfig {
                    common,
                    ..Default::default()
                };
                for option in &statement.options {
                    match option.optionType {
                        brieOptionType::Online => config.online = option.uintValue != 0,
                        brieOptionType::WaitTiFlashReady => {
                            config.waitTiFlashReady = option.uintValue != 0
                        }
                        brieOptionType::WithSystemTable => {
                            config.withSystemTable = option.uintValue != 0
                        }
                        brieOptionType::LoadStats => config.loadStats = option.uintValue != 0,
                        _ => {}
                    }
                }
                executor.restoreCfg = Some(config);
            }
            _ => {
                return Err(BrieError(format!(
                    "unsupported BRIE statement kind: {}",
                    statement.kind
                )));
            }
        }
        Ok(brieExecutor::Main(executor))
    }
}

/// BRIE 执行器变体：一次性、Show、Cancel 与主路径。
pub enum brieExecutor {
    OneShot(Box<oneshotExecutor>),
    ShowQuery(showQueryExec),
    Cancel(cancelJobExec),
    ShowMeta(showMetaExec),
    Main(BRIEExec),
}

impl brieExecutor {
    /// 分派到具体执行器的 Next。
    fn execute(&mut self, context: &taskContext, chunk: &mut resultChunk) -> BrieResult {
        match self {
            Self::OneShot(executor) => executor.Next(context, chunk),
            Self::ShowQuery(executor) => executor.Next(context, chunk),
            Self::Cancel(executor) => executor.Next(context, chunk),
            Self::ShowMeta(executor) => executor.Next(context, chunk),
            Self::Main(executor) => executor.Next(context, chunk),
        }
    }
}

/// 只产出一次结果的包装执行器。
pub struct oneshotExecutor {
    pub executor: Box<brieExecutor>,
    pub finished: bool,
}

impl oneshotExecutor {
    /// 首次执行内层执行器，之后返回空结果。
    pub fn Next(&mut self, context: &taskContext, chunk: &mut resultChunk) -> BrieResult {
        if self.finished {
            chunk.reset();
            return Ok(());
        }
        self.executor.execute(context, chunk)?;
        self.finished = true;
        Ok(())
    }
}

/// 将执行器包装为 oneshot。
pub fn execOnce(executor: brieExecutor) -> brieExecutor {
    brieExecutor::OneShot(Box::new(oneshotExecutor {
        executor: Box::new(executor),
        finished: false,
    }))
}

/// SHOW BRIE QUERY：按任务 ID 返回原始 SQL。
pub struct showQueryExec {
    pub targetID: u64,
}

impl showQueryExec {
    /// 查询任务原始 SQL 并写入结果行。
    pub fn Next(&mut self, _context: &taskContext, chunk: &mut resultChunk) -> BrieResult {
        chunk.reset();
        if let Some(task) = current_queue().queryTask(self.targetID) {
            chunk.push_row(vec![datum::String(task.query)]);
        }
        Ok(())
    }
}

/// CANCEL BRIE JOB：取消队列中的任务。
pub struct cancelJobExec {
    pub targetID: u64,
    pub runtime: Arc<dyn brieRuntime>,
}

impl cancelJobExec {
    /// 取消任务；不存在时追加 warning。
    pub fn Next(&mut self, _context: &taskContext, chunk: &mut resultChunk) -> BrieResult {
        chunk.reset();
        if !current_queue().cancelTask(self.targetID) {
            self.runtime.append_job_not_found_warning(self.targetID);
        }
        Ok(())
    }
}

/// SHOW BACKUP META：读取 backupmeta 并输出表级统计。
pub struct showMetaExec {
    pub showConfig: showConfig,
    pub runtime: Arc<dyn brieRuntime>,
}

/// 从语句构造 SHOW BACKUP META 配置（默认 plaintext）。
pub fn buildShowMetadataConfigFrom(statement: &brieStmt) -> showConfig {
    assert_eq!(statement.kind, brieKind::ShowBackupMeta);
    showConfig {
        storage: statement.storage.clone(),
        cipher: cipherInfo {
            method: "plaintext".into(),
            key: Vec::new(),
        },
    }
}

impl showMetaExec {
    /// 读取 backupmeta 并按表输出元数据行。
    pub fn Next(&mut self, context: &taskContext, chunk: &mut resultChunk) -> BrieResult {
        // 读取 backupmeta，按表输出库名/表名/KV 统计与起止 TSO 时间
        let metadata = self
            .runtime
            .read_backup_metadata(context, &self.showConfig)
            .map_err(|error| {
                BrieError(format!("failed to read metadata from backupmeta: {error}"))
            })?;
        let timezone = self.runtime.timezone();
        for table in metadata.tables {
            let start = if metadata.startVersion > 0 {
                datum::Time(
                    self.runtime
                        .format_tso_time(metadata.startVersion, &timezone)?,
                )
            } else {
                datum::Null
            };
            chunk.push_row(vec![
                datum::String(table.databaseName),
                datum::String(table.tableName),
                datum::Integer(table.kvCount as i64),
                datum::Integer(table.kvSize as i64),
                start,
                datum::Time(
                    self.runtime
                        .format_tso_time(metadata.endVersion, &timezone)?,
                ),
            ]);
        }
        Ok(())
    }
}

/// BACKUP/RESTORE 主执行器：登记队列、运行 glue、写出结果行。
pub struct BRIEExec {
    pub backupCfg: Option<backupConfig>,
    pub restoreCfg: Option<restoreConfig>,
    pub showConfig: Option<showConfig>,
    pub info: Option<Arc<Mutex<brieTaskInfo>>>,
    pub runtime: Arc<dyn brieRuntime>,
    pub sessionID: u64,
}

/// RAII：作用域结束时取消任务（异常/提前返回保护）。
struct cancelTaskGuard {
    queue: Arc<brieQueue>,
    task_id: u64,
}

impl Drop for cancelTaskGuard {
    fn drop(&mut self) {
        self.queue.cancelTask(self.task_id);
    }
}

/// RAII：作用域结束时释放 worker。
struct releaseTaskGuard(Arc<brieQueue>);

impl Drop for releaseTaskGuard {
    fn drop(&mut self) {
        self.0.releaseTask();
    }
}

impl BRIEExec {
    /// 登记任务、等待调度、执行 backup/restore，并输出结果行。
    pub fn Next(&mut self, context: &taskContext, chunk: &mut resultChunk) -> BrieResult {
        chunk.reset();
        let Some(info) = self.info.clone() else {
            return Ok(());
        };
        let queue = current_queue();
        // 清理过期任务后登记，并启动 kill 监视线程
        queue.clearTask();
        {
            let mut info = info.lock().expect("BRIE task info mutex poisoned");
            info.connID = self.runtime.connection_id();
            info.queueTime = sqlTime::now();
        }
        let (task_context, task_id) = queue.registerTask(context.clone(), info.clone());
        let _cancel_guard = cancelTaskGuard {
            queue: queue.clone(),
            task_id,
        };
        let monitor_context = task_context.clone();
        let monitor_queue = queue.clone();
        let monitor_runtime = self.runtime.clone();
        // 周期性检查会话是否被 kill，若是则取消任务
        thread::spawn(move || {
            while !monitor_context.is_canceled() {
                thread::sleep(Duration::from_secs(3));
                if monitor_runtime.check_killed().is_err() {
                    monitor_queue.cancelTask(task_id);
                    break;
                }
            }
        });

        // 获得执行权后构造 tidbGlue 并调用运行时
        let progress = queue.acquireTask(&task_context, task_id)?;
        let _release_guard = releaseTaskGuard(queue);
        info.lock().expect("BRIE task info mutex poisoned").execTime = sqlTime::now();
        let mut glue = tidbGlue {
            runtime: self.runtime.clone(),
            sessionID: self.sessionID,
            progress,
            info: info.clone(),
        };
        let kind = info.lock().expect("BRIE task info mutex poisoned").kind;
        let result = match kind {
            brieKind::Backup => handleBRIEError(
                self.runtime.run_backup(
                    &task_context,
                    &mut glue,
                    self.backupCfg
                        .as_ref()
                        .expect("backup executor requires backup config"),
                ),
                brieErrorClass::Backup,
            ),
            brieKind::Restore => handleBRIEError(
                self.runtime.run_restore(
                    &task_context,
                    &mut glue,
                    self.restoreCfg
                        .as_ref()
                        .expect("restore executor requires restore config"),
                ),
                brieErrorClass::Restore,
            ),
            _ => Err(BrieError(format!(
                "unsupported BRIE statement kind: {kind}"
            ))),
        };
        {
            let mut info = info.lock().expect("BRIE task info mutex poisoned");
            info.finishTime = sqlTime::now();
            if let Err(error) = &result {
                info.message = error.to_string();
            } else {
                info.message.clear();
            }
        }
        result?;
        let snapshot = info.lock().expect("BRIE task info mutex poisoned").clone();
        match kind {
            brieKind::Backup => chunk.push_row(vec![
                datum::String(snapshot.storage),
                datum::Unsigned(snapshot.archiveSize),
                datum::Unsigned(snapshot.backupTS),
                datum::Time(snapshot.queueTime),
                datum::Time(snapshot.execTime),
            ]),
            brieKind::Restore => chunk.push_row(vec![
                datum::String(snapshot.storage),
                datum::Unsigned(snapshot.archiveSize),
                datum::Unsigned(snapshot.backupTS),
                datum::Unsigned(snapshot.restoreTS),
                datum::Time(snapshot.queueTime),
                datum::Time(snapshot.execTime),
            ]),
            _ => unreachable!(),
        }
        self.info = None;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 错误归类：备份或恢复。
pub enum brieErrorClass {
    Backup,
    Restore,
}

/// 为底层错误添加 BRIE backup/restore 前缀。
pub fn handleBRIEError(result: BrieResult, class: brieErrorClass) -> BrieResult {
    result.map_err(|error| {
        let operation = match class {
            brieErrorClass::Backup => "backup",
            brieErrorClass::Restore => "restore",
        };
        BrieError(format!("BRIE {operation} failed: {error}"))
    })
}

/// SHOW BRIE 列表：遍历队列输出进度行。
pub struct ShowExec {
    pub result: resultChunk,
}

impl ShowExec {
    /// 按种类收集任务行到 result，并顺带清理过期任务。
    pub fn fetchShowBRIE(&mut self, kind: brieKind) -> BrieResult {
        let queue = current_queue();
        for item in queue.items() {
            let info = item
                .info
                .lock()
                .expect("BRIE task info mutex poisoned")
                .clone();
            if info.kind != kind {
                continue;
            }
            let (command, total, current) = item.progress.snapshot();
            self.result.push_row(vec![
                datum::Unsigned(info.id),
                datum::String(info.storage),
                datum::String(command),
                datum::Float(100.0 * current as f64 / total as f64),
                datum::Time(info.queueTime),
                datum::Time(info.execTime),
                datum::Time(info.finishTime),
                datum::Unsigned(info.connID),
                if info.message.is_empty() {
                    datum::Null
                } else {
                    datum::String(info.message)
                },
            ]);
        }
        queue.clearTask();
        Ok(())
    }
}

/// BR 库与 TiDB 会话/进度的粘合层（Glue）。
pub struct tidbGlue {
    pub runtime: Arc<dyn brieRuntime>,
    pub sessionID: u64,
    pub progress: Arc<brieTaskProgress>,
    pub info: Arc<Mutex<brieTaskInfo>>,
}

impl tidbGlue {
    /// 取得 domain 句柄。
    pub fn GetDomain(&self, _storage: u64) -> BrieResult<u64> {
        self.runtime.domain_handle(self.sessionID)
    }

    /// 创建一次性会话。
    pub fn CreateSession(&self, _storage: u64) -> BrieResult<tidbGlueSession> {
        Ok(tidbGlueSession {
            runtime: self.runtime.clone(),
            sessionID: self.runtime.create_session(self.sessionID)?,
        })
    }

    /// 打开存储句柄。
    pub fn Open(&self, _path: &str) -> BrieResult<u64> {
        self.runtime.storage_handle(self.sessionID)
    }

    /// 是否拥有存储所有权（固定 false）。
    pub fn OwnsStorage(&self) -> bool {
        false
    }

    /// 开始进度阶段并返回共享进度对象。
    pub fn StartProgress(
        &self,
        _context: &taskContext,
        cmdName: &str,
        total: i64,
        _redirectLog: bool,
    ) -> Arc<brieTaskProgress> {
        self.progress.start(cmdName, total);
        self.progress.clone()
    }

    /// 记录 BackupTS/RestoreTS/Size 等到任务信息。
    pub fn Record(&self, name: &str, value: u64) {
        let mut info = self.info.lock().expect("BRIE task info mutex poisoned");
        match name {
            "BackupTS" => info.backupTS = value,
            "RestoreTS" => info.restoreTS = value,
            "Size" => info.archiveSize = value,
            _ => {}
        }
    }

    /// 返回 TiDB 版本信息文本。
    pub fn GetVersion(&self) -> String {
        format!("TiDB\n{}", self.runtime.tidb_info())
    }

    /// 创建一次性会话执行回调后关闭。
    pub fn UseOneShotSession(
        &self,
        _storage: u64,
        _closeDomain: bool,
        callback: &mut dyn FnMut(&mut tidbGlueSession) -> BrieResult,
    ) -> BrieResult {
        let mut session = self.CreateSession(0)?;
        let result = callback(&mut session);
        session.Close();
        self.runtime.log_one_shot_session_closed();
        result
    }

    /// 返回客户端类型。
    pub fn GetClient(&self) -> brieClient {
        brieClient::Sql
    }
}

/// Glue 会话：受限 SQL、建库建表与元数据刷新。
pub struct tidbGlueSession {
    pub runtime: Arc<dyn brieRuntime>,
    pub sessionID: u64,
}

impl tidbGlueSession {
    /// 执行受限 SQL。
    pub fn Execute(&self, _context: &taskContext, sql: &str) -> BrieResult {
        self.runtime
            .execute_restricted_brie_sql(self.sessionID, sql)
    }

    /// 执行内部 SQL（可带参数）。
    pub fn ExecuteInternal(
        &self,
        _context: &taskContext,
        sql: &str,
        arguments: &[datum],
    ) -> BrieResult {
        self.runtime
            .execute_internal_brie_sql(self.sessionID, sql, arguments)
    }

    /// 建库，已存在则报错。
    pub fn CreateDatabaseOnExistError(
        &self,
        _context: &taskContext,
        schema: &databaseInfo,
    ) -> BrieResult {
        self.runtime.create_database(self.sessionID, schema)
    }

    /// 建表。
    pub fn CreateTable(
        &self,
        _context: &taskContext,
        databaseName: &str,
        table: &tableInfo,
        options: &[createTableOption],
    ) -> BrieResult {
        self.runtime
            .create_table(self.sessionID, databaseName, table, options)
    }

    /// 批量建表。
    pub fn CreateTables(
        &self,
        _context: &taskContext,
        tables: &BTreeMap<String, Vec<tableInfo>>,
        options: &[createTableOption],
    ) -> BrieResult {
        self.runtime.create_tables(self.sessionID, tables, options)
    }

    /// 创建 Placement Policy；临时切换 query string。
    pub fn CreatePlacementPolicy(
        &self,
        _context: &taskContext,
        policy: &placementPolicyInfo,
    ) -> BrieResult {
        let original = self.runtime.query_string(self.sessionID);
        self.runtime
            .set_query_string(self.sessionID, &policy.showCreateSQL);
        let result = self
            .runtime
            .create_placement_policy_ignore_existing(self.sessionID, policy);
        self.runtime.set_query_string(self.sessionID, &original);
        result
    }

    pub fn Close(&self) {
        self.runtime.close_session(self.sessionID);
    }

    /// 读全局表变量。
    pub fn GetGlobalVariable(&self, name: &str) -> BrieResult<String> {
        self.runtime.global_table_value(self.sessionID, name)
    }

    /// 读全局系统变量。
    pub fn GetGlobalSysVar(&self, name: &str) -> BrieResult<String> {
        self.runtime.global_system_variable(self.sessionID, name)
    }

    /// 返回会话 ID。
    pub fn GetSessionCtx(&self) -> u64 {
        self.sessionID
    }

    /// 修改表模式；临时设置展示用 query string。
    pub fn AlterTableMode(
        &self,
        _context: &taskContext,
        schemaID: i64,
        tableID: i64,
        tableMode: &str,
    ) -> BrieResult {
        let original = self.runtime.query_string(self.sessionID);
        self.runtime.set_query_string(
            self.sessionID,
            &format!("ALTER TABLE MODE SCHEMA_ID={schemaID} TABLE_ID={tableID} TO {tableMode}"),
        );
        let result = self.runtime.alter_table_mode(
            self.sessionID,
            &alterTableModeArgs {
                schemaID,
                tableID,
                tableMode: tableMode.into(),
            },
        );
        self.runtime.set_query_string(self.sessionID, &original);
        result
    }

    /// 刷新元数据；临时设置展示用 query string。
    pub fn RefreshMeta(&self, _context: &taskContext, args: &refreshMetaArgs) -> BrieResult {
        let original = self.runtime.query_string(self.sessionID);
        self.runtime.set_query_string(
            self.sessionID,
            &format!(
                "REFRESH META SCHEMA_ID={} TABLE_ID={} INVOLVED_DB={} INVOLVED_TABLE={}",
                args.schemaID, args.tableID, args.involvedDatabase, args.involvedTable
            ),
        );
        let result = self.runtime.refresh_meta(self.sessionID, args);
        self.runtime.set_query_string(self.sessionID, &original);
        result
    }
}

/// 还原语句的可展示 SQL 文本；失败时返回 N/A。
pub fn restoreQuery(runtime: &dyn brieRuntime, statement: &brieStmt) -> String {
    runtime
        .restore_brie_query(statement)
        .unwrap_or_else(|_| "N/A".into())
}
