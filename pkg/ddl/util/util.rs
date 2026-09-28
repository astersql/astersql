// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DDL 通用工具：etcd 键路径、删除范围（delete range）任务、作业暂停、
// 会话上下文与索引冲突错误编码等。
//
// 主要内容：
// - etcd 路径常量与内存 `EtcdClient`（含 watch）；
// - GC 删除范围表的加载/完成/更新；
// - DDL 作业暂停、系统库判定、时区与 Raft 引擎探测；
// - 唯一索引冲突时的 `KeyExists` 错误构造。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

/// 待 GC 删除的 key 范围记录表名。
pub const DELETE_RANGES_TABLE: &str = "gc_delete_range";
/// 已完成删除范围记录表名。
pub const DONE_DELETE_RANGES_TABLE: &str = "gc_delete_range_done";
/// DDL Owner 选举在 etcd 上的键。
pub const DDLOwnerKey: &str = "/tidb/ddl/fg/owner";
/// 各节点 schema 版本发布前缀。
pub const DDLAllSchemaVersions: &str = "/tidb/ddl/all_schema_versions";
/// 按 job 维度的 schema 版本前缀。
pub const DDLAllSchemaVersionsByJob: &str = "/tidb/ddl/all_schema_by_job_versions";
/// 全局 schema 版本键。
pub const DDLGlobalSchemaVersion: &str = "/tidb/ddl/global_schema_version";
/// 新 DDL 作业提交通知键。
pub const AddingDDLJobNotifyKey: &str = "/tidb/ddl/add_ddl_job_general";
/// 服务器全局状态键。
pub const ServerGlobalState: &str = "/tidb/server/global_state";
/// 会话在 etcd 上的租约 TTL（秒）。
pub const SessionTTL: i64 = 90;

/// DDL 工具层错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DdlUtilError {
    /// 操作被取消令牌中止。
    Cancelled,
    /// 作业已处于暂停/暂停中。
    PausedJob(i64),
    /// 当前状态不允许暂停作业。
    CannotPauseJob { job_id: i64, reason: String },
    /// 非法十六进制键编码。
    InvalidHex(String),
    /// etcd 操作失败。
    Etcd(String),
    /// 内部 SQL 失败。
    Sql(String),
    /// 唯一索引键冲突（对应 MySQL Duplicate entry）。
    KeyExists { value: String, index: String },
}

/// 面向用户的错误消息格式化。
impl Display for DdlUtilError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("operation cancelled"),
            Self::PausedJob(job_id) => write!(formatter, "DDL job {job_id} is paused"),
            Self::CannotPauseJob { job_id, reason } => {
                write!(formatter, "cannot pause DDL job {job_id}: {reason}")
            }
            Self::InvalidHex(value) => write!(formatter, "invalid hex key: {value}"),
            Self::Etcd(message) => write!(formatter, "etcd operation failed: {message}"),
            Self::Sql(message) => write!(formatter, "internal SQL failed: {message}"),
            Self::KeyExists { value, index } => {
                write!(formatter, "Duplicate entry '{value}' for key '{index}'")
            }
        }
    }
}

/// 标准 Error trait。
impl Error for DdlUtilError {}

/// 协作式取消令牌：置位后检查接口返回 `Cancelled`。
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// 请求取消。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// 已取消则返回错误，否则成功。
    pub fn check(&self) -> Result<(), DdlUtilError> {
        if self.is_cancelled() {
            Err(DdlUtilError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// 内存 etcd 客户端可注入失败的操作种类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EtcdOperation {
    /// 读取。
    Get,
    /// 写入。
    Put,
    /// 删除。
    Delete,
    /// 比较并交换（CAS）。
    CompareAndSwap,
    /// 监听。
    Watch,
}

/// watch 事件类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchEventKind {
    /// 写入/更新。
    Put,
    /// 删除。
    Delete,
}

/// 单次 etcd watch 事件。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchEvent {
    /// 事件种类。
    pub kind: WatchEventKind,
    /// 变更的键。
    pub key: String,
    /// 写入时的新值；删除时为 `None`。
    pub value: Option<String>,
    /// 产生事件时的修订号。
    pub revision: i64,
}

/// 接收 watch 事件的通道包装。
#[derive(Clone)]
pub struct WatchChannel {
    /// 内部接收端。
    receiver: Arc<Mutex<mpsc::Receiver<WatchEvent>>>,
}

impl WatchChannel {
    /// 阻塞接收下一事件。
    pub fn recv(&self) -> Result<WatchEvent, mpsc::RecvError> {
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv()
    }

    /// 非阻塞尝试接收。
    pub fn try_recv(&self) -> Result<WatchEvent, mpsc::TryRecvError> {
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv()
    }
}

/// etcd 键值项及其修改修订号。
#[derive(Clone, Debug)]
pub struct EtcdValue {
    /// 键。
    pub key: String,
    /// 值。
    pub value: String,
    /// 最近一次修改的修订号。
    pub mod_revision: i64,
}

/// 内存 etcd 共享状态。
#[derive(Default)]
struct EtcdState {
    /// 当前键值。
    values: BTreeMap<String, EtcdValue>,
    /// 全局递增修订号。
    revision: i64,
    /// 注入的下一次操作失败队列。
    failures: VecDeque<(EtcdOperation, DdlUtilError)>,
    /// 按精确路径注册的 watcher 发送端。
    watchers: Vec<(String, mpsc::Sender<WatchEvent>)>,
}

/// 进程内模拟的 etcd 客户端，供 DDL 工具与测试使用。
#[derive(Clone, Default)]
pub struct EtcdClient {
    /// 共享状态。
    state: Arc<Mutex<EtcdState>>,
}

impl EtcdClient {
    /// 为指定操作注入一次失败。
    pub fn fail_next(&self, operation: EtcdOperation, error: DdlUtilError) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .push_back((operation, error));
    }

    /// 若队首失败匹配当前操作则弹出并返回错误。
    fn take_failure(state: &mut EtcdState, operation: EtcdOperation) -> Result<(), DdlUtilError> {
        if state
            .failures
            .front()
            .is_some_and(|item| item.0 == operation)
        {
            return Err(state.failures.pop_front().expect("front exists").1);
        }
        Ok(())
    }

    /// 按精确键或前缀读取。
    pub fn get(&self, key: &str, prefix: bool) -> Result<Vec<EtcdValue>, DdlUtilError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::take_failure(&mut state, EtcdOperation::Get)?;
        Ok(state
            .values
            .iter()
            .filter(|(candidate, _)| {
                if prefix {
                    candidate.starts_with(key)
                } else {
                    *candidate == key
                }
            })
            .map(|(_, value)| value.clone())
            .collect())
    }

    /// 写入并通知匹配路径的 watcher，返回新修订号。
    pub fn put(&self, key: &str, value: &str) -> Result<i64, DdlUtilError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::take_failure(&mut state, EtcdOperation::Put)?;
        state.revision += 1;
        let revision = state.revision;
        state.values.insert(
            key.to_owned(),
            EtcdValue {
                key: key.to_owned(),
                value: value.to_owned(),
                mod_revision: revision,
            },
        );
        Self::notify(&mut state, WatchEventKind::Put, key, Some(value), revision);
        Ok(revision)
    }

    /// 仅当当前 `mod_revision` 等于期望值时写入（CAS）。
    pub fn compare_and_put(
        &self,
        key: &str,
        expected_revision: i64,
        value: &str,
    ) -> Result<bool, DdlUtilError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::take_failure(&mut state, EtcdOperation::CompareAndSwap)?;
        let current_revision = state.values.get(key).map_or(0, |value| value.mod_revision);
        if current_revision != expected_revision {
            return Ok(false);
        }
        state.revision += 1;
        let revision = state.revision;
        state.values.insert(
            key.to_owned(),
            EtcdValue {
                key: key.to_owned(),
                value: value.to_owned(),
                mod_revision: revision,
            },
        );
        Self::notify(&mut state, WatchEventKind::Put, key, Some(value), revision);
        Ok(true)
    }

    /// 删除所有匹配前缀的键，并逐个通知 watcher。
    pub fn delete_prefix(&self, prefix: &str) -> Result<usize, DdlUtilError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::take_failure(&mut state, EtcdOperation::Delete)?;
        let keys = state
            .values
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>();
        for key in &keys {
            state.values.remove(key);
            state.revision += 1;
            let revision = state.revision;
            Self::notify(&mut state, WatchEventKind::Delete, key, None, revision);
        }
        Ok(keys.len())
    }

    /// 注册精确路径的 watch 通道。
    pub fn watch(&self, path: &str) -> Result<WatchChannel, DdlUtilError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::take_failure(&mut state, EtcdOperation::Watch)?;
        let (sender, receiver) = mpsc::channel();
        state.watchers.push((path.to_owned(), sender));
        Ok(WatchChannel {
            receiver: Arc::new(Mutex::new(receiver)),
        })
    }

    /// 向路径精确匹配的 watcher 投递事件；发送失败则移除该 watcher。
    fn notify(
        state: &mut EtcdState,
        kind: WatchEventKind,
        key: &str,
        value: Option<&str>,
        revision: i64,
    ) {
        state.watchers.retain(|(path, sender)| {
            if path != key {
                return true;
            }
            sender
                .send(WatchEvent {
                    kind: kind.clone(),
                    key: key.to_owned(),
                    value: value.map(str::to_owned),
                    revision,
                })
                .is_ok()
        });
    }
}

/// 作业涉及的 schema（数据库）信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvolvingSchemaInfo {
    /// 数据库名。
    pub database: String,
}

/// DDL 作业状态机状态。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JobState {
    #[default]
    /// 未启动。
    None,
    /// 运行中。
    Running,
    /// 排队中。
    Queueing,
    /// 正在进入暂停。
    Pausing,
    /// 已暂停。
    Paused,
    /// 正在取消。
    Cancelling,
    /// 已完成。
    Done,
}

/// Schema 对象在线 DDL 中间态。
/// DeleteOnly/WriteOnly 为渐进式变更中的可见性阶段。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SchemaState {
    #[default]
    /// 无状态。
    None,
    /// 仅允许删除相关写。
    DeleteOnly,
    /// 允许写但读路径可能仍受限。
    WriteOnly,
    /// 对用户完全可见可用。
    Public,
}

/// 触发管理命令（如暂停）的操作者。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AdminCommandOperator {
    #[default]
    /// 未知。
    Unknown,
    /// 用户发起。
    User,
    /// 系统内部发起。
    System,
}

/// 精简的 DDL 作业视图（暂停/系统库判定用）。
#[derive(Clone, Debug, Default)]
pub struct Job {
    /// 作业 ID。
    pub id: i64,
    /// 作业状态。
    pub state: JobState,
    /// 当前 schema 对象状态。
    pub schema_state: SchemaState,
    /// 最近管理操作的操作者。
    pub admin_operator: AdminCommandOperator,
    /// Running 时是否允许暂停（对应可回滚作业）。
    pub pausable: bool,
    /// 涉及的数据库列表。
    pub involving_schemas: Vec<InvolvingSchemaInfo>,
}

/// 作业是否涉及系统库（mysql/information_schema 等）。
pub fn HasSysDB(job: &Job) -> bool {
    job.involving_schemas.iter().any(|info| {
        matches!(
            info.database.to_ascii_lowercase().as_str(),
            "mysql" | "information_schema" | "performance_schema" | "metrics_schema" | "sys"
        )
    })
}

/// 是否可暂停：未启动/排队，或 Running 且 `pausable`。
/// Matches Go `Job.IsPausable`: not-started jobs, or running jobs marked rollbackable via `pausable`.
fn job_is_pausable(job: &Job) -> bool {
    matches!(job.state, JobState::None | JobState::Queueing)
        || (job.state == JobState::Running && job.pausable)
}

/// 将可暂停作业切入 `Pausing`，并记录操作者。
pub fn PauseRunningJob(job: &mut Job, by_who: AdminCommandOperator) -> Result<(), DdlUtilError> {
    // 已在暂停流程中则直接报错，避免重复暂停。
    if matches!(job.state, JobState::Pausing | JobState::Paused) {
        return Err(DdlUtilError::PausedJob(job.id));
    }
    if !job_is_pausable(job) {
        return Err(DdlUtilError::CannotPauseJob {
            job_id: job.id,
            reason: format!(
                "state [{:?}] or schema state [{:?}]",
                job.state, job.schema_state
            ),
        });
    }
    job.state = JobState::Pausing;
    job.admin_operator = by_who;
    Ok(())
}

/// GC 删除范围任务：描述待从存储删除的 key 半开区间。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DelRangeTask {
    /// 区间起始键（含）。
    pub start_key: Vec<u8>,
    /// 区间结束键（不含）。
    pub end_key: Vec<u8>,
    /// 关联 DDL 作业 ID。
    pub job_id: i64,
    /// 作业内元素（如表/索引）ID。
    pub element_id: i64,
}

impl DelRangeTask {
    /// 返回 `(start_key, end_key)` 副本。
    pub fn Range(&self) -> (Vec<u8>, Vec<u8>) {
        (self.start_key.clone(), self.end_key.clone())
    }
}

/// 删除范围表中的一条记录（含时间戳）。
#[derive(Clone, Debug)]
struct DeleteRangeRecord {
    /// 任务内容。
    task: DelRangeTask,
    /// 记录时间戳；小于 safe_point 才可加载执行。
    ts: u64,
}

/// 会话变量快照（系统变量、时区、chunk 大小）。
#[derive(Clone, Debug)]
pub struct SessionVars {
    /// 已加载的系统变量。
    pub system_vars: HashMap<String, String>,
    /// 时区名，如 UTC。
    pub location_name: String,
    /// 时区固定偏移（秒）；名称不可加载时使用。
    pub location_offset_seconds: i32,
    /// 时区名是否可被加载。
    pub location_loadable: bool,
    /// 结果集 chunk 最大行数。
    pub max_chunk_size: usize,
}

/// 默认 UTC、chunk 1024。
impl Default for SessionVars {
    fn default() -> Self {
        Self {
            system_vars: HashMap::new(),
            location_name: "UTC".to_owned(),
            location_offset_seconds: 0,
            location_loadable: true,
            max_chunk_size: 1024,
        }
    }
}

/// 内存会话后端状态。
#[derive(Default)]
struct SessionState {
    /// 活动删除范围。
    delete_ranges: Vec<DeleteRangeRecord>,
    /// 已完成删除范围。
    done_delete_ranges: Vec<DeleteRangeRecord>,
    /// 全局变量存储。
    global_variables: HashMap<String, String>,
    /// 当前会话变量。
    vars: SessionVars,
    /// Raft 存储引擎标识列表。
    raft_engines: Vec<String>,
    /// 注入的下一次 SQL/会话失败。
    failures: VecDeque<DdlUtilError>,
}

/// DDL 工具使用的内存会话上下文。
#[derive(Clone, Default)]
pub struct SessionContext {
    /// 共享状态。
    state: Arc<Mutex<SessionState>>,
}

impl SessionContext {
    /// 向活动或已完成表插入一条删除范围。
    pub fn add_delete_range(&self, task: DelRangeTask, ts: u64, done: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = DeleteRangeRecord { task, ts };
        if done {
            state.done_delete_ranges.push(record);
        } else {
            state.delete_ranges.push(record);
        }
    }

    /// 设置全局变量。
    pub fn set_global_variable(&self, name: impl Into<String>, value: impl Into<String>) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .global_variables
            .insert(name.into(), value.into());
    }

    /// 配置会话时区名与偏移。
    pub fn set_time_zone(&self, name: impl Into<String>, offset_seconds: i32, loadable: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.vars.location_name = name.into();
        state.vars.location_offset_seconds = offset_seconds;
        state.vars.location_loadable = loadable;
    }

    /// 设置可见的 Raft 引擎列表（探测 raft-kv2 用）。
    pub fn set_raft_engines(&self, engines: Vec<String>) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .raft_engines = engines;
    }

    /// 注入下一次会话操作失败。
    pub fn fail_next(&self, error: DdlUtilError) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failures
            .push_back(error);
    }

    /// 克隆当前会话变量。
    pub fn session_vars(&self) -> SessionVars {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .vars
            .clone()
    }
}

/// 弹出队首注入失败（模拟 SQL 语句边界）。
fn take_session_failure(state: &mut SessionState) -> Result<(), DdlUtilError> {
    match state.failures.pop_front() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// 加载时间戳小于 safe_point 的活动删除范围。
pub fn LoadDeleteRanges(
    session: &SessionContext,
    safe_point: u64,
) -> Result<Vec<DelRangeTask>, DdlUtilError> {
    load_delete_ranges_from_table(session, false, safe_point)
}

/// 加载时间戳小于 safe_point 的已完成删除范围。
pub fn LoadDoneDeleteRanges(
    session: &SessionContext,
    safe_point: u64,
) -> Result<Vec<DelRangeTask>, DdlUtilError> {
    load_delete_ranges_from_table(session, true, safe_point)
}

/// 按 done 标志从对应内存表过滤并返回任务。
fn load_delete_ranges_from_table(
    session: &SessionContext,
    done: bool,
    safe_point: u64,
) -> Result<Vec<DelRangeTask>, DdlUtilError> {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?;
    let records = if done {
        &state.done_delete_ranges
    } else {
        &state.delete_ranges
    };
    Ok(records
        .iter()
        .filter(|record| record.ts < safe_point)
        .map(|record| record.task.clone())
        .collect())
}

/// 完成一条删除范围：可选记入 done 表，再从活动表删除。
pub fn CompleteDeleteRange(
    session: &SessionContext,
    range: DelRangeTask,
    need_to_record_done: bool,
) -> Result<(), DdlUtilError> {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?; // BEGIN
    // 对应 Go：可选 INSERT IGNORE ... SELECT 到 done 表，再 DELETE 活动行并 COMMIT。
    if need_to_record_done {
        take_session_failure(&mut state)?; // INSERT IGNORE ... SELECT
        if let Some(record) = state
            .delete_ranges
            .iter()
            .find(|record| {
                record.task.job_id == range.job_id && record.task.element_id == range.element_id
            })
            .cloned()
            && !state.done_delete_ranges.iter().any(|done| {
                done.task.job_id == range.job_id && done.task.element_id == range.element_id
            })
        {
            state.done_delete_ranges.push(record);
        }
    }
    take_session_failure(&mut state)?; // DELETE active range
    state.delete_ranges.retain(|record| {
        record.task.job_id != range.job_id || record.task.element_id != range.element_id
    });
    take_session_failure(&mut state)?; // COMMIT
    Ok(())
}

/// 按 job_id + element_id 从活动删除范围表移除。
pub fn RemoveFromGCDeleteRange(
    session: &SessionContext,
    job_id: i64,
    element_id: i64,
) -> Result<(), DdlUtilError> {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?;
    state
        .delete_ranges
        .retain(|record| record.task.job_id != job_id || record.task.element_id != element_id);
    Ok(())
}

/// 按 job_id 移除该作业下全部活动删除范围。
pub fn RemoveMultiFromGCDeleteRange(
    session: &SessionContext,
    job_id: i64,
) -> Result<(), DdlUtilError> {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?;
    state
        .delete_ranges
        .retain(|record| record.task.job_id != job_id);
    Ok(())
}

/// 从已完成删除范围表移除对应记录。
pub fn DeleteDoneRecord(
    session: &SessionContext,
    range: &DelRangeTask,
) -> Result<(), DdlUtilError> {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?;
    state.done_delete_ranges.retain(|record| {
        record.task.job_id != range.job_id || record.task.element_id != range.element_id
    });
    Ok(())
}

/// 在活动表中把匹配的 start_key 更新为 new_start_key（分段删除进度）。
pub fn UpdateDeleteRange(
    session: &SessionContext,
    range: &DelRangeTask,
    new_start_key: &[u8],
    old_start_key: &[u8],
) -> Result<(), DdlUtilError> {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?;
    if let Some(record) = state.delete_ranges.iter_mut().find(|record| {
        record.task.job_id == range.job_id
            && record.task.element_id == range.element_id
            && record.task.start_key == old_start_key
    }) {
        record.task.start_key = new_start_key.to_vec();
    }
    Ok(())
}

/// 将指定全局变量拷贝进会话 system_vars。
pub fn LoadGlobalVars(
    session: &SessionContext,
    variable_names: &[&str],
) -> Result<(), DdlUtilError> {
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?;
    for name in variable_names {
        if let Some(value) = state.global_variables.get(*name).cloned() {
            state.vars.system_vars.insert((*name).to_owned(), value);
        }
    }
    Ok(())
}

/// 返回可加载时区名，或空名 + 固定偏移秒数。
pub fn GetTimeZone(session: &SessionContext) -> (String, i32) {
    let vars = session.session_vars();
    if !vars.location_name.is_empty() && vars.location_loadable {
        (vars.location_name, 0)
    } else {
        (String::new(), vars.location_offset_seconds)
    }
}

/// 模拟器 GC 开关（1 启用 / 0 禁用）。
static EMULATOR_GC_ENABLE: AtomicI32 = AtomicI32::new(1);

/// 启用模拟器 GC。
pub fn EmulatorGCEnable() {
    EMULATOR_GC_ENABLE.store(1, Ordering::Release);
}

/// 禁用模拟器 GC。
pub fn EmulatorGCDisable() {
    EMULATOR_GC_ENABLE.store(0, Ordering::Release);
}

/// 模拟器 GC 是否启用。
pub fn IsEmulatorGCEnable() -> bool {
    EMULATOR_GC_ENABLE.load(Ordering::Acquire) == 1
}

/// TopSQL 内部请求使用的资源组标签字节。
pub const INTERNAL_RESOURCE_GROUP_TAG: &[u8] = &[0];

/// 精简 RPC 请求，仅含资源组标签字段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RpcRequest {
    /// 资源组标签。
    pub resource_group_tag: Vec<u8>,
}

/// 为请求填充资源组标签的回调。
pub type ResourceGroupTagger = Arc<dyn Fn(&mut RpcRequest) + Send + Sync>;

/// 返回写入内部标签的 TopSQL tagger。
pub fn GetInternalResourceGroupTaggerForTopSQL() -> ResourceGroupTagger {
    Arc::new(|request| request.resource_group_tag = INTERNAL_RESOURCE_GROUP_TAG.to_vec())
}

/// 判断标签是否为内部 TopSQL 资源组标签。
pub fn IsInternalResourceGroupTaggerForTopSQL(tag: &[u8]) -> bool {
    tag == INTERNAL_RESOURCE_GROUP_TAG
}

/// 重试前休眠；零间隔则立即返回。
fn retry_pause(duration: Duration) {
    if !duration.is_zero() {
        thread::sleep(duration);
    }
}

/// 带重试地删除 etcd 前缀下全部键。
pub fn DeleteKeysWithPrefixFromEtcd(
    prefix: &str,
    etcd_client: &EtcdClient,
    retry_count: usize,
    retry_interval: Duration,
) -> Result<(), DdlUtilError> {
    if retry_count == 0 {
        return Ok(());
    }
    let mut last_error = DdlUtilError::Etcd("delete was not attempted".to_owned());
    for _ in 0..retry_count {
        match etcd_client.delete_prefix(prefix) {
            Ok(_) => return Ok(()),
            Err(error) => {
                last_error = error;
                retry_pause(retry_interval);
            }
        }
    }
    Err(last_error)
}

/// 带取消与重试的单调写入：先读修订号再 CAS，避免覆盖并发更新。
pub fn PutKVToEtcdMono(
    cancellation: &CancellationToken,
    etcd_client: &EtcdClient,
    retry_count: usize,
    key: &str,
    value: &str,
    retry_interval: Duration,
) -> Result<(), DdlUtilError> {
    if retry_count == 0 {
        return Ok(());
    }
    let mut last_error = DdlUtilError::Etcd("put was not attempted".to_owned());
    for _ in 0..retry_count {
        cancellation.check()?;
        // 先取当前修订号，再 compare_and_put；CAS 失败则重试。
        let previous_revision = match etcd_client.get(key, false) {
            Ok(values) => values.first().map_or(0, |value| value.mod_revision),
            Err(error) => {
                last_error = error;
                retry_pause(retry_interval);
                continue;
            }
        };
        match etcd_client.compare_and_put(key, previous_revision, value) {
            Ok(true) => return Ok(()),
            Ok(false) => {
                last_error = DdlUtilError::Etcd(
                    "performing compare-and-swap during PutKVToEtcd failed".to_owned(),
                )
            }
            Err(error) => last_error = error,
        }
        retry_pause(retry_interval);
    }
    Err(last_error)
}

/// 带取消与重试的普通 Put（不比较修订号）。
pub fn PutKVToEtcd(
    cancellation: &CancellationToken,
    etcd_client: &EtcdClient,
    retry_count: usize,
    key: &str,
    value: &str,
    retry_interval: Duration,
) -> Result<(), DdlUtilError> {
    if retry_count == 0 {
        return Ok(());
    }
    let mut last_error = DdlUtilError::Etcd("put was not attempted".to_owned());
    for _ in 0..retry_count {
        cancellation.check()?;
        match etcd_client.put(key, value) {
            Ok(_) => return Ok(()),
            Err(error) => {
                last_error = error;
                retry_pause(retry_interval);
            }
        }
    }
    Err(last_error)
}

/// 将二进制键格式化为 SQL 风格字面量（空为 `''`，否则 `0x` 十六进制）。
pub fn WrapKey2String(key: &[u8]) -> String {
    if key.is_empty() {
        "''".to_owned()
    } else {
        format!("0x{}", encode_hex(key))
    }
}

/// 探测会话侧首个引擎是否为 `raft-kv2`。
pub fn IsRaftKv2(
    cancellation: &CancellationToken,
    session: &SessionContext,
) -> Result<bool, DdlUtilError> {
    cancellation.check()?;
    let mut state = session
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take_session_failure(&mut state)?;
    Ok(state
        .raft_engines
        .first()
        .is_some_and(|engine| engine == "raft-kv2"))
}

/// 目录存在且至少有一个条目时返回 true。
pub fn FolderNotEmpty(path: &str) -> bool {
    fs::read_dir(path)
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some()
}

/// 索引中的列描述（偏移、前缀长度、是否 binary）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexColumnInfo {
    /// 列在表中的偏移。
    pub offset: usize,
    /// 前缀索引长度；`None` 表示整列。
    pub prefix_length: Option<usize>,
    /// 是否按二进制展示冲突值。
    pub binary: bool,
}

/// 索引元信息（名称与列）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引名。
    pub name: String,
    /// 索引列。
    pub columns: Vec<IndexColumnInfo>,
}

/// 精简表元信息（含可选表锁）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    /// 表 ID。
    pub id: i64,
    /// 所属库 ID。
    pub db_id: i64,
    /// 表名。
    pub name: String,
    /// 表锁信息。
    pub lock: Option<TableLockInfo>,
}

/// 表锁：持有会话列表与锁类型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableLockInfo {
    /// 持锁会话。
    pub sessions: Vec<SessionInfo>,
    /// 锁类型名。
    pub lock_type: String,
}

/// 会话标识：server_id + session_id。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SessionInfo {
    /// TiDB 节点 ID。
    pub server_id: String,
    /// 会话 ID。
    pub session_id: u64,
}

/// 某张表上的一种锁类型描述。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableLockTpInfo {
    /// schema（库）ID。
    pub schema_id: i64,
    /// 表 ID。
    pub table_id: i64,
    /// 锁类型。
    pub lock_type: String,
}

/// 当前 keyspace 名；非空时 GenKeyExistsErr 会剥离键前缀。
static KEYSPACE_NAME: Mutex<String> = Mutex::new(String::new());

/// 设置全局 keyspace 名。
pub fn SetKeyspaceName(name: impl Into<String>) {
    *KEYSPACE_NAME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = name.into();
}

/// 根据索引键/值构造唯一约束冲突错误消息。
pub fn GenKeyExistsErr(
    key: &[u8],
    value: &[u8],
    index: &IndexInfo,
    table: &TableInfo,
) -> DdlUtilError {
    // keyspace 场景下跳过 4 字节前缀，再按列拆分解码冲突展示值。
    let key = if key.len() > 4
        && key[0] == b'x'
        && !KEYSPACE_NAME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    {
        &key[4..]
    } else {
        key
    };
    let index_name = format!("{}.{}", table.name, index.name);
    let source = if key.len() > 19 { &key[19..] } else { value };
    let encoded_values = if source.contains(&0) {
        source.split(|byte| *byte == 0).collect::<Vec<_>>()
    } else if source.contains(&b'|') {
        source.split(|byte| *byte == b'|').collect::<Vec<_>>()
    } else {
        vec![source]
    };
    let values = index
        .columns
        .iter()
        .enumerate()
        .map(|(position, column)| {
            let raw = encoded_values.get(position).copied().unwrap_or_default();
            let raw = column
                .prefix_length
                .map_or(raw, |length| &raw[..raw.len().min(length)]);
            if column.binary || std::str::from_utf8(raw).is_err() {
                format!("0x{}", encode_hex(raw))
            } else {
                String::from_utf8_lossy(raw).into_owned()
            }
        })
        .collect::<Vec<_>>();
    DdlUtilError::KeyExists {
        value: if values.is_empty() {
            encode_hex(key)
        } else {
            values.join("-")
        },
        index: index_name,
    }
}

/// 将字节编码为小写十六进制字符串。
fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

/// 将偶长度十六进制字符串解码为字节；奇数长度报错。
pub fn DecodeHexKey(value: &str) -> Result<Vec<u8>, DdlUtilError> {
    if !value.len().is_multiple_of(2) {
        return Err(DdlUtilError::InvalidHex(value.to_owned()));
    }
    (0..value.len())
        .step_by(2)
        .map(|position| {
            u8::from_str_radix(&value[position..position + 2], 16)
                .map_err(|_| DdlUtilError::InvalidHex(value.to_owned()))
        })
        .collect()
}
