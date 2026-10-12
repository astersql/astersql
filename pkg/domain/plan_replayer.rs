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

// Plan Replayer（执行计划重放）任务收集、dump 与过期文件 GC。
//
// Plan Replayer 用于捕获 SQL 的执行计划及相关上下文（schema、stats、bindings 等），
// 打包成可下载的 dump 文件，便于离线复现与排查优化问题。
// 本模块覆盖：
// - dump 文件名时间戳解析与按租约清理过期文件；
// - capture 任务 key 收集/去重；
// - dump 任务投递、运行中占用与完成状态管理。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Plan Replayer 任务唯一键：SQL digest + plan digest（摘要哈希）。
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlanReplayerTaskKey {
    /// SQL 文本摘要。
    pub sql_digest: String,
    /// 执行计划摘要。
    pub plan_digest: String,
}

/// 一次 dump 所需的全部上下文：表统计、会话变量、语句、计划与 capture 标志。
#[derive(Clone, Debug, Default)]
pub struct PlanReplayerDumpTask {
    /// 任务键。
    pub key: PlanReplayerTaskKey,
    /// 表 ID 到统计信息载荷的映射（简化为字符串）。
    pub table_stats: Vec<(i64, String)>,
    /// 事务开始时间戳（Timestamp，TiKV MVCC 版本号）。
    pub start_ts: u64,
    /// 会话级 SQL Binding（强制走指定计划的绑定规则）。
    pub session_bindings: Vec<String>,
    /// 已编码的执行计划；非空时可直接写出，跳过 explain。
    pub encoded_plan: String,
    /// 会话变量快照。
    pub session_variables: Vec<(String, String)>,
    /// 待 dump 的 SQL 语句列表。
    pub statements: Vec<String>,
    /// 是否执行 `EXPLAIN ANALYZE`（带真实执行统计）。
    pub analyze: bool,
    /// 历史统计快照时间戳；0 表示用最新统计。
    pub historical_stats_ts: u64,
    /// 优化器 debug trace 载荷。
    pub debug_trace: Vec<String>,
    /// dump 文件名（亦用作 status 表 token）。
    pub file_name: String,
    /// 外部存储预签名下载 URL。
    pub presigned_url: String,
    /// 是否来自 capture（自动捕获）路径。
    pub is_capture: bool,
    /// 是否持续 capture；成功后标记 finished，避免重复 dump。
    pub is_continuous_capture: bool,
}

/// 写入 `mysql.plan_replayer_status` 的状态记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanReplayerStatusRecord {
    /// SQL 文本摘要。
    pub sql_digest: String,
    /// 执行计划摘要。
    pub plan_digest: String,
    /// 原始 SQL 文本。
    pub origin_sql: String,
    /// 对应 dump 文件 token。
    pub token: String,
    /// 失败原因；空表示成功。
    pub failed_reason: String,
}

/// 从 dump 文件名解析创建时间。
///
/// 约定文件名形如 `..._<nanos>.zip`：取最后一个 `_` 与 `.` 之间的纳秒时间戳。
pub fn parse_dump_time(file_name: &str) -> Result<SystemTime, String> {
    // 只取 basename，避免路径干扰解析。
    let base = Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("failed to parse the file: {file_name}"))?;
    let start = base
        .rfind('_')
        .ok_or_else(|| format!("failed to parse the file: {file_name}"))?
        + 1;
    let end = base
        .rfind('.')
        .filter(|end| *end > start)
        .ok_or_else(|| format!("failed to parse the file: {file_name}"))?;
    let nanos = base[start..end]
        .parse::<i64>()
        .map_err(|_| format!("failed to parse the file: {file_name}"))?;
    if nanos >= 0 {
        UNIX_EPOCH
            .checked_add(Duration::from_nanos(nanos as u64))
            .ok_or_else(|| format!("failed to parse the file: {file_name}"))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_nanos(nanos.unsigned_abs()))
            .ok_or_else(|| format!("failed to parse the file: {file_name}"))
    }
}

/// dump 文件存储抽象：列举、删除文件，以及删除 status 表记录。
pub trait DumpFileStore {
    /// 列举指定子目录下的文件。
    fn list(&self, path: &str) -> Result<Vec<String>, String>;
    /// 删除单个 dump 文件。
    fn delete(&self, path: &str) -> Result<(), String>;
    /// 按 token 清理 `plan_replayer_status` 记录。
    fn delete_status(&self, token: &str) -> Result<(), String>;
}

/// 周期性清理 plan replayer / trace dump 过期文件的检查器。
pub struct DumpFileGcChecker {
    /// 需要扫描的目录列表。
    paths: Vec<String>,
    /// 串行化 GC，避免并发清理。
    lock: Mutex<()>,
}

impl DumpFileGcChecker {
    /// 注册待 GC 的路径。
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            paths,
            lock: Mutex::new(()),
        }
    }

    /// Run the Domain GC pass with the current sysvar retention for ordinary
    /// files and the fixed seven-day capture retention used by Go.
    pub fn gc_with_current_retention<S: DumpFileStore>(
        &self,
        store: &S,
        now: SystemTime,
    ) -> Result<Vec<String>, String> {
        self.gc(
            store,
            now,
            astersql_sessionctx_vardef::GetPlanReplayerFileRetentionTime(),
            Duration::from_secs(7 * 24 * 60 * 60),
        )
    }

    /// 按默认租约与 capture 租约删除过期文件。
    ///
    /// capture 类文件通常保留更久；删除 replayer 文件时同步清理 status。
    pub fn gc<S: DumpFileStore>(
        &self,
        store: &S,
        now: SystemTime,
        default_lease: Duration,
        capture_lease: Duration,
    ) -> Result<Vec<String>, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "dump file GC lock poisoned".to_string())?;
        // 计算两类过期截止时间：普通 dump 与 capture dump。
        let default_cutoff = now.checked_sub(default_lease).unwrap_or(UNIX_EPOCH);
        let capture_cutoff = now.checked_sub(capture_lease).unwrap_or(UNIX_EPOCH);
        let mut deleted = Vec::new();
        for path in &self.paths {
            let Ok(files) = store.list(path) else {
                // Go 仅记录 WalkDir 错误，并继续清理其它注册目录。
                continue;
            };
            for file in files {
                let Ok(created) = parse_dump_time(&file) else {
                    continue;
                };
                let base = Path::new(&file)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                let replayer = base.contains("replayer");
                // capture 使用更长租约，避免刚捕获的文件被过早删除。
                let cutoff = if replayer && base.contains("capture") {
                    capture_cutoff
                } else {
                    default_cutoff
                };
                if created <= cutoff {
                    if store.delete(&file).is_err() {
                        // 单个文件删除失败不能中断同一目录中其它文件的 GC。
                        continue;
                    }
                    if replayer {
                        // Go 的状态表清理是 best-effort；dump 文件已删除即视为 GC 成功。
                        let _ = store.delete_status(base);
                    }
                    deleted.push(file);
                }
            }
        }
        Ok(deleted)
    }
}

/// 内存中待处理的 capture 任务集合（已过滤掉 handled）。
#[derive(Default)]
pub struct PlanReplayerTaskCollector {
    /// 读写锁保护的任务 key 集合。
    tasks: RwLock<BTreeSet<PlanReplayerTaskKey>>,
}

/// SQL boundary used by plan-replayer task collection.
///
/// Implementations must execute both lookups with the supplied internal-request
/// context so slow logging and statement-summary filtering see the query as
/// internal, matching Go's `ExecuteInternal` path.
pub trait PlanReplayerTaskSource {
    /// Return every registered capture task.
    fn registered_tasks(
        &self,
        context: &astersql_kv::Context,
        sql: &str,
    ) -> Result<Vec<PlanReplayerTaskKey>, String>;

    /// Return whether one registered task still needs handling.
    fn is_unhandled(
        &self,
        context: &astersql_kv::Context,
        key: &PlanReplayerTaskKey,
    ) -> Result<bool, String>;
}

/// SQL used by Go's `collectAllPlanReplayerTask` implementation.
pub const COLLECT_PLAN_REPLAYER_TASK_SQL: &str =
    "select sql_digest, plan_digest from mysql.plan_replayer_task";

impl PlanReplayerTaskCollector {
    /// Collect registered, unhandled tasks through an internal SQL context.
    ///
    /// The current set is replaced only after every lookup succeeds, preserving
    /// Go's error path where `setupTasks` is not reached on query failure.
    pub fn collect_plan_replayer_tasks<S: PlanReplayerTaskSource>(
        &self,
        source: &S,
    ) -> Result<(), String> {
        let context = astersql_kv::WithInternalSourceType(
            astersql_kv::Context::todo(),
            astersql_kv::InternalTxnStatsForegroundPriority,
        );
        let registered = source.registered_tasks(&context, COLLECT_PLAN_REPLAYER_TASK_SQL)?;
        let mut tasks = Vec::new();
        for key in registered {
            if source.is_unhandled(&context, &key)? {
                tasks.push(key);
            }
        }
        self.collect(tasks, &BTreeSet::new());
        Ok(())
    }

    /// 用已注册但尚未 handled 的任务覆盖当前集合。
    pub fn collect(
        &self,
        registered: Vec<PlanReplayerTaskKey>,
        handled: &BTreeSet<PlanReplayerTaskKey>,
    ) {
        *self
            .tasks
            .write()
            .expect("plan replayer collector poisoned") = registered
            .into_iter()
            .filter(|key| !handled.contains(key))
            .collect();
    }
    /// 返回当前待处理任务副本。
    pub fn tasks(&self) -> Vec<PlanReplayerTaskKey> {
        self.tasks
            .read()
            .expect("plan replayer collector poisoned")
            .iter()
            .cloned()
            .collect()
    }
    /// 投递成功后移除对应 key。
    pub fn remove(&self, key: &PlanReplayerTaskKey) {
        self.tasks
            .write()
            .expect("plan replayer collector poisoned")
            .remove(key);
    }
}

/// dump worker 的运行中/已完成任务状态，用于去重与并发占用。
#[derive(Default)]
pub struct PlanReplayerDumpTaskStatus {
    /// 正在被 worker 处理的 key。
    running: RwLock<BTreeSet<PlanReplayerTaskKey>>,
    /// 持续 capture 已成功完成的 key。
    finished: RwLock<BTreeSet<PlanReplayerTaskKey>>,
}

impl PlanReplayerDumpTaskStatus {
    /// 运行中任务数（测试用）。
    pub fn running_len(&self) -> usize {
        self.running
            .read()
            .expect("plan replayer status poisoned")
            .len()
    }
    /// 已完成任务数（测试用）。
    pub fn finished_len(&self) -> usize {
        self.finished
            .read()
            .expect("plan replayer status poisoned")
            .len()
    }
    /// 清空已完成缓存（GC 删除文件后或测试用）。
    pub fn clean_finished(&self) {
        self.finished
            .write()
            .expect("plan replayer status poisoned")
            .clear();
    }
    /// 尝试抢占运行中 key；已存在则返回 false。
    pub fn occupy(&self, key: &PlanReplayerTaskKey) -> bool {
        self.running
            .write()
            .expect("plan replayer status poisoned")
            .insert(key.clone())
    }
    /// 释放运行中 key。
    pub fn release(&self, key: &PlanReplayerTaskKey) {
        self.running
            .write()
            .expect("plan replayer status poisoned")
            .remove(key);
    }
    /// 是否已在 finished 集合中。
    pub fn was_finished(&self, key: &PlanReplayerTaskKey) -> bool {
        self.finished
            .read()
            .expect("plan replayer status poisoned")
            .contains(key)
    }
    /// 标记任务完成。
    pub fn set_finished(&self, key: PlanReplayerTaskKey) {
        self.finished
            .write()
            .expect("plan replayer status poisoned")
            .insert(key);
    }
}

/// Plan Replayer 对外句柄：收集端 + 有界 channel 投递端。
pub struct PlanReplayerHandle {
    /// 共享的任务收集器。
    collector: Arc<PlanReplayerTaskCollector>,
    /// 有界同步 channel；`None` 表示已关闭。
    sender: Mutex<Option<mpsc::SyncSender<PlanReplayerDumpTask>>>,
}

impl PlanReplayerHandle {
    /// 创建句柄与接收端；容量至少为 1。
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<PlanReplayerDumpTask>) {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        (
            Self {
                collector: Arc::new(PlanReplayerTaskCollector::default()),
                sender: Mutex::new(Some(sender)),
            },
            receiver,
        )
    }
    /// 取得收集器共享引用。
    pub fn collector(&self) -> Arc<PlanReplayerTaskCollector> {
        self.collector.clone()
    }
    /// 非阻塞投递 dump 任务；channel 满则丢弃并返回 false。
    ///
    /// 非持续 capture 投递成功后立即从收集器移除，失败可在下一轮重新收集。
    pub fn send_task(&self, task: PlanReplayerDumpTask) -> bool {
        let key = task.key.clone();
        let sent = self
            .sender
            .lock()
            .expect("plan replayer sender poisoned")
            .as_ref()
            .is_some_and(|sender| sender.try_send(task.clone()).is_ok());
        if sent && !task.is_continuous_capture {
            self.collector.remove(&key);
        }
        sent
    }
    /// 关闭投递端，worker 侧随即结束。
    pub fn close(&self) {
        self.sender
            .lock()
            .expect("plan replayer sender poisoned")
            .take();
    }
}

/// 实际写出 dump 内容并返回 status 记录的后端抽象。
pub trait PlanReplayerDumper {
    /// 执行一次 dump；成功返回待写入的 status 记录。
    fn dump(
        &self,
        task: &mut PlanReplayerDumpTask,
    ) -> Result<Vec<PlanReplayerStatusRecord>, String>;
}

/// 处理单个 dump 任务：跳过已完成、抢占运行锁、调用 dumper、释放并标记完成。
///
/// 对齐 Go worker 的去重与并发保护语义。
pub fn handle_dump_task(
    dumper: &dyn PlanReplayerDumper,
    status: &PlanReplayerDumpTaskStatus,
    task: &mut PlanReplayerDumpTask,
) -> bool {
    // 只有持续 capture 任务需要 finished 去重；普通任务失败后允许再次处理，
    // 与 Go worker 仅在 IsContinuesCapture 分支检查 finishedTask 一致。
    if (task.is_continuous_capture && status.was_finished(&task.key)) || !status.occupy(&task.key) {
        return false;
    }
    let result = dumper.dump(task);
    status.release(&task.key);
    if result.is_ok() && task.is_continuous_capture {
        status.set_finished(task.key.clone());
    }
    result.is_ok()
}
