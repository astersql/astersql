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

// DXF Handle 层：任务提交/等待/取消/暂停恢复、重试、云存储 URI、计量上报等对外 API。
//
// 通过全局安装的 `Runtime` trait 对象对接存储与集群元数据；测试可替换为确定性实现。
// Region 是 TiKV 的数据分片单位；默认 split size/keys 影响导入等任务的切分粒度。

use crate::{proto, schstatus, storage};
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 与 storage 层共用的错误类型别名。
pub type Error = storage::Error;
/// Handle 层统一 Result。
pub type Result<T> = std::result::Result<T, Error>;

/// 轮询任务是否完成/暂停的间隔。
pub const CHECK_TASK_FINISH_INTERVAL: Duration = Duration::from_millis(300);
/// 任务变更通知 channel 容量（满则丢弃，仅作唤醒信号）。
pub const TASK_CHANGED_CH_CAPACITY: usize = 1;
/// DXF 采样日志分类名。
pub const DXF_LOG_CATEGORY: &str = "dxf";
/// 采样错误日志的节流周期。
pub const SAMPLE_LOG_TICK: Duration = Duration::from_secs(60);
/// 采样开始前允许原样输出的前 N 条日志。
pub const SAMPLE_LOG_FIRST: usize = 10;
/// Next-gen 内核下任务的固定目标 scope。
pub const NEXT_GEN_TARGET_SCOPE: &str = "dxf_service";
/// Classic 内核默认 Region 切分大小（96MiB）。
pub const DEF_REGION_SPLIT_SIZE: i64 = 96 * 1024 * 1024;
/// Classic 内核默认 Region 切分键数。
pub const DEF_REGION_SPLIT_KEYS: i64 = 960_000;

#[derive(Clone, Default)]
/// 可取消的轻量上下文：用 Condvar 实现超时等待与取消通知。
pub struct Context {
    cancelled: Arc<(Mutex<bool>, Condvar)>,
    cancellation_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl Context {
    /// 与调度任务及对象存储共用取消标志。
    pub fn from_cancellation_flag(flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            cancelled: Arc::default(),
            cancellation_flag: flag,
        }
    }
    /// 构造未取消的后台上下文。
    pub fn background() -> Self {
        Self::default()
    }

    /// 标记取消并唤醒所有等待者。
    pub fn cancel(&self) {
        self.cancellation_flag
            .store(true, std::sync::atomic::Ordering::Release);
        let (lock, wake) = &*self.cancelled;
        *lock.lock().expect("context lock poisoned") = true;
        wake.notify_all();
    }

    /// 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_flag
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// 等待指定时长；若期间被取消则返回 context canceled。
    pub(crate) fn wait(&self, duration: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + duration;
        let (lock, wake) = &*self.cancelled;
        let mut cancelled = lock.lock().expect("context lock poisoned");
        loop {
            if *cancelled || self.is_cancelled() {
                return Err(Error::new("context canceled"));
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(now);
            (cancelled, _) = wake
                .wait_timeout(cancelled, remaining.min(Duration::from_millis(10)))
                .expect("context lock poisoned");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 结构化日志字段（键值对）。
pub struct LogField {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 采样日志配置：分类、节流周期、首发条数与附加字段。
pub struct SampleLogger {
    pub category: &'static str,
    pub tick: Duration,
    pub first: usize,
    pub fields: Vec<LogField>,
}

/// 使用 DXF 默认分类与采样参数构造 SampleLogger。
pub fn NewSampleErrVerboseLogger(fields: Vec<LogField>) -> SampleLogger {
    SampleLogger {
        category: DXF_LOG_CATEGORY,
        tick: SAMPLE_LOG_TICK,
        first: SAMPLE_LOG_FIRST,
        fields,
    }
}

#[derive(Default)]
/// 对象存储访问计数统计。
pub struct AccessStats {
    requests: Mutex<u64>,
}

impl AccessStats {
    /// 已记录的请求次数。
    pub fn requests(&self) -> u64 {
        *self.requests.lock().expect("access stats lock poisoned")
    }

    /// 递增请求计数。
    pub fn record_request(&self) {
        let mut requests = self.requests.lock().expect("access stats lock poisoned");
        *requests += 1;
    }
}

/// 对象存储抽象，至少暴露 URI。
pub trait ObjectStorage: Send + Sync {
    fn uri(&self) -> &str;
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 计量（metering）字段值：整数或文本。
pub enum MeterValue {
    Integer(i64),
    Text(String),
}

/// 单条计量记录：有序字段映射。
pub type MeterItem = BTreeMap<String, MeterValue>;

/// Runtime 封装进程侧集成（与 Go 实现对齐）：
/// 任务存储、集群元数据、对象存储与计量。应用安装一份实现；
/// 测试可安装确定性实现而不改动下方任务控制逻辑。
/// Runtime contains the process integrations used by the Go implementation:
/// task storage, cluster metadata, object storage and metering. Applications
/// install one implementation; tests can install a deterministic implementation
/// without changing the task-control logic below.
pub trait Runtime: Send + Sync {
    fn get_cpu_count_of_node(&self, ctx: &Context) -> Result<i32>;
    fn get_task_by_key_with_history(&self, ctx: &Context, key: &str)
    -> Result<Option<proto::Task>>;
    #[allow(clippy::too_many_arguments)]
    fn create_task(
        &self,
        ctx: &Context,
        key: &str,
        task_type: proto::TaskType,
        keyspace: &str,
        required_slots: i32,
        target_scope: &str,
        max_node_count: i32,
        extra_params: proto::ExtraParams,
        meta: Vec<u8>,
    ) -> Result<i64>;
    fn get_task_by_id(&self, ctx: &Context, id: i64) -> Result<proto::Task>;
    fn get_task_by_id_with_history(&self, ctx: &Context, id: i64) -> Result<proto::Task>;
    fn get_task_base_by_id_with_history(&self, ctx: &Context, id: i64) -> Result<proto::TaskBase>;
    fn get_task_by_key(&self, ctx: &Context, key: &str) -> Result<Option<proto::Task>>;
    fn cancel_task(&self, ctx: &Context, id: i64) -> Result<()>;
    fn pause_task(&self, ctx: &Context, key: &str) -> Result<bool>;
    fn resume_task(&self, ctx: &Context, key: &str) -> Result<bool>;

    fn get_task_bases_in_states(
        &self,
        ctx: &Context,
        states: &[proto::TaskState],
    ) -> Result<Vec<proto::TaskBase>>;
    fn get_all_nodes(&self, ctx: &Context) -> Result<Vec<proto::ManagedNode>>;
    fn get_busy_nodes(&self, ctx: &Context) -> Result<Vec<schstatus::Node>>;
    fn owner_exec_id(&self, ctx: &Context) -> Result<String>;
    fn get_active_task_summary(&self, ctx: &Context) -> Result<storage::ActiveTaskSummary>;
    fn list_history_tasks(
        &self,
        ctx: &Context,
        page_size: i32,
        page_token: i64,
        keyspace: &str,
    ) -> Result<storage::HistoryTaskPage>;
    fn local_cpu_count(&self) -> i32;

    fn update_pause_scale_in_flag(&self, ctx: &Context, flag: &schstatus::TTLFlag) -> Result<()>;
    fn get_pause_scale_in_flag(&self, ctx: &Context) -> Result<Option<schstatus::TTLFlag>>;
    fn get_schedule_tune_factors(
        &self,
        ctx: &Context,
        keyspace: &str,
    ) -> Result<Option<schstatus::TTLTuneFactors>>;

    fn is_next_gen(&self) -> bool;
    fn service_scope(&self) -> String;
    fn cloud_storage_uri(&self) -> String;
    fn sem_enabled(&self) -> bool;
    fn cluster_id(&self, ctx: &Context) -> Option<u64>;
    fn new_object_store(
        &self,
        ctx: &Context,
        uri: &str,
        recording: Option<Arc<AccessStats>>,
    ) -> Result<Arc<dyn ObjectStorage>>;
    fn write_meter_data(
        &self,
        ctx: &Context,
        timestamp: i64,
        key: &str,
        item: &MeterItem,
    ) -> Result<()>;
}

/// 全局 Runtime 安装槽（OnceLock + RwLock）。
fn runtime_cell() -> &'static RwLock<Option<Arc<dyn Runtime>>> {
    static RUNTIME: OnceLock<RwLock<Option<Arc<dyn Runtime>>>> = OnceLock::new();
    RUNTIME.get_or_init(|| RwLock::new(None))
}

/// 安装全局 Runtime，返回被替换的旧值。
pub fn InstallRuntime(value: Arc<dyn Runtime>) -> Option<Arc<dyn Runtime>> {
    runtime_cell()
        .write()
        .expect("runtime lock poisoned")
        .replace(value)
}

/// 清除全局 Runtime，返回旧值。
pub fn ClearRuntime() -> Option<Arc<dyn Runtime>> {
    runtime_cell()
        .write()
        .expect("runtime lock poisoned")
        .take()
}

/// 读取已安装 Runtime；未安装则报错。
pub(crate) fn runtime() -> Result<Arc<dyn Runtime>> {
    runtime_cell()
        .read()
        .expect("runtime lock poisoned")
        .clone()
        .ok_or_else(|| Error::new("DXF handle runtime is not installed"))
}

/// 任务变更同步 channel（容量 1，用作非阻塞唤醒）。
fn task_changed_channel() -> &'static (SyncSender<()>, Mutex<Receiver<()>>) {
    static CHANNEL: OnceLock<(SyncSender<()>, Mutex<Receiver<()>>)> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(TASK_CHANGED_CH_CAPACITY);
        (sender, Mutex::new(receiver))
    })
}

/// 通知任务状态可能已变；channel 满或断开时静默忽略。
pub fn NotifyTaskChange() {
    match task_changed_channel().0.try_send(()) {
        Ok(()) | Err(TrySendError::Full(())) => {}
        Err(TrySendError::Disconnected(())) => {}
    }
}

/// 非阻塞尝试收取一次任务变更信号。
pub fn TryRecvTaskChange() -> bool {
    match task_changed_channel()
        .1
        .lock()
        .expect("task changed receiver lock poisoned")
        .try_recv()
    {
        Ok(()) => true,
        Err(TryRecvError::Empty | TryRecvError::Disconnected) => false,
    }
}

/// 查询节点 CPU 核数（经 Runtime）。
pub fn GetCPUCountOfNode(ctx: &Context) -> Result<i32> {
    runtime()?.get_cpu_count_of_node(ctx)
}

#[allow(clippy::too_many_arguments)]
/// 使用默认 ExtraParams 提交任务。
pub fn SubmitTask(
    ctx: &Context,
    task_key: &str,
    task_type: proto::TaskType,
    keyspace: &str,
    required_slots: i32,
    target_scope: &str,
    max_node_count: i32,
    task_meta: Vec<u8>,
) -> Result<proto::Task> {
    SubmitTaskWithExtraParams(
        ctx,
        task_key,
        task_type,
        keyspace,
        required_slots,
        target_scope,
        max_node_count,
        proto::ExtraParams::default(),
        task_meta,
    )
}

#[allow(clippy::too_many_arguments)]
/// 带 ExtraParams 提交任务：若 key（含历史）已存在则返回 ErrTaskAlreadyExists。
pub fn SubmitTaskWithExtraParams(
    ctx: &Context,
    task_key: &str,
    task_type: proto::TaskType,
    keyspace: &str,
    required_slots: i32,
    target_scope: &str,
    max_node_count: i32,
    extra_params: proto::ExtraParams,
    task_meta: Vec<u8>,
) -> Result<proto::Task> {
    let runtime = runtime()?;
    match runtime.get_task_by_key_with_history(ctx, task_key) {
        Ok(Some(_)) => return Err(storage::ErrTaskAlreadyExists.into()),
        Ok(None) => {}
        Err(error) if error == storage::ErrTaskNotFound => {}
        Err(error) => return Err(error),
    }
    let task_id = runtime.create_task(
        ctx,
        task_key,
        task_type,
        keyspace,
        required_slots,
        target_scope,
        max_node_count,
        extra_params,
        task_meta,
    )?;
    let task = runtime.get_task_by_id(ctx, task_id)?;
    NotifyTaskChange();
    Ok(task)
}

/// 等待任务完成或暂停，忽略具体任务内容。
pub fn WaitTaskDoneOrPaused(ctx: &Context, id: i64) -> Result<()> {
    WaitTaskDoneOrPausedWithResult(ctx, id).map(|_| ())
}

/// 等待任务完成/暂停并返回最终任务；Reverted/Failed 转为错误。
pub fn WaitTaskDoneOrPausedWithResult(ctx: &Context, id: i64) -> Result<proto::Task> {
    WaitTask(ctx, id, |task| {
        task.IsDone() || task.State == proto::TaskStatePaused
    })?;
    let task = runtime()?.get_task_by_id_with_history(ctx, id)?;
    match task.State {
        proto::TaskStateSucceed | proto::TaskStatePaused => Ok(task),
        proto::TaskStateReverted => {
            if let Some(error) = &task.Error {
                return Err(Error::new(error.clone()));
            }
            Err(Error::new(format!(
                "task stopped with state {}",
                task.State
            )))
        }
        proto::TaskStateFailed => Err(Error::new(format!(
            "task stopped with state {}, err {}",
            task.State,
            task.Error.as_deref().unwrap_or("<nil>")
        ))),
        _ => Ok(task),
    }
}

/// 按 key 查找任务后等待其完成（含历史表）。
pub fn WaitTaskDoneByKey(ctx: &Context, task_key: &str) -> Result<()> {
    let task = runtime()?
        .get_task_by_key_with_history(ctx, task_key)?
        .ok_or_else(|| Error::from(storage::ErrTaskNotFound))?;
    WaitTask(ctx, task.ID, proto::TaskBase::IsDone).map(|_| ())
}

/// 按间隔轮询任务基座，直到 `matches` 为真；瞬时管理器失败时继续轮询（对齐 Go）。
pub fn WaitTask<F>(ctx: &Context, id: i64, mut matches: F) -> Result<proto::TaskBase>
where
    F: FnMut(&proto::TaskBase) -> bool,
{
    let runtime = runtime()?;
    loop {
        ctx.wait(CHECK_TASK_FINISH_INTERVAL)?;
        // Go deliberately keeps polling after transient manager failures.
        let Ok(task) = runtime.get_task_base_by_id_with_history(ctx, id) else {
            continue;
        };
        if matches(&task) {
            return Ok(task);
        }
    }
}

/// 按 key 取消任务；不存在则静默成功。
pub fn CancelTask(ctx: &Context, task_key: &str) -> Result<()> {
    let runtime = runtime()?;
    let task = match runtime.get_task_by_key(ctx, task_key) {
        Ok(Some(task)) => task,
        Ok(None) => return Ok(()),
        Err(error) if error == storage::ErrTaskNotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    runtime.cancel_task(ctx, task.ID)
}

/// 按 key 暂停任务。
pub fn PauseTask(ctx: &Context, task_key: &str) -> Result<()> {
    let _found = runtime()?.pause_task(ctx, task_key)?;
    Ok(())
}

/// 按 key 恢复任务。
pub fn ResumeTask(ctx: &Context, task_key: &str) -> Result<()> {
    let _found = runtime()?.resume_task(ctx, task_key)?;
    Ok(())
}

/// 重试退避策略：根据第几次重试返回等待时长。
pub trait Backoffer {
    fn backoff(&self, retry: i32) -> Duration;
}

impl<F> Backoffer for F
where
    F: Fn(i32) -> Duration,
{
    fn backoff(&self, retry: i32) -> Duration {
        self(retry)
    }
}

/// 在 max_retry 次内执行 operation；可重试错误则退避后重试，不可重试则立即返回。
pub fn RunWithRetry<B, F>(
    ctx: &Context,
    max_retry: i32,
    backoffer: &B,
    mut on_retry: impl FnMut(i32, i32, &Error),
    mut operation: F,
) -> Result<()>
where
    B: Backoffer,
    F: FnMut(&Context) -> (bool, Result<()>),
{
    let mut last_error = None;
    for retry in 0..max_retry {
        let (retryable, result) = operation(ctx);
        match result {
            Ok(()) => return Ok(()),
            Err(error) if !retryable => return Err(error),
            Err(error) => {
                on_retry(retry, max_retry, &error);
                last_error = Some(error);
                ctx.wait(backoffer.backoff(retry))?;
            }
        }
    }
    // Go returns its nil lastErr when maxRetry is zero.
    match last_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// 返回默认 Region 切分 (size, keys)；next-gen 使用更大常量。
pub fn GetDefaultRegionSplitConfig() -> Result<(i64, i64)> {
    if runtime()?.is_next_gen() {
        return Ok((1024 * 1024 * 1024, 102_400_000));
    }
    Ok((DEF_REGION_SPLIT_SIZE, DEF_REGION_SPLIT_KEYS))
}

/// 返回任务目标 scope：next-gen 固定为 dxf_service，否则取 service_scope。
pub fn GetTargetScope() -> Result<String> {
    let runtime = runtime()?;
    if runtime.is_next_gen() {
        Ok(NEXT_GEN_TARGET_SCOPE.to_owned())
    } else {
        Ok(runtime.service_scope())
    }
}

/// 在云存储 URI 路径下追加 `/dxf`（及可选 cluster_id），保留 query/fragment。
pub fn GetCloudStorageURI(ctx: &Context) -> Result<String> {
    let runtime = runtime()?;
    let uri = runtime.cloud_storage_uri();
    if uri.is_empty() {
        return Ok(uri);
    }
    Ok(resolve_cloud_storage_uri(
        &uri,
        runtime.sem_enabled(),
        || runtime.cluster_id(ctx),
    ))
}

/// Resolve the same URI for a concrete worker store without installing a global
/// task runtime. The cluster lookup remains lazy and is skipped without a prefix.
pub fn resolve_cloud_storage_uri(
    uri: &str,
    sem_enabled: bool,
    cluster_id: impl FnOnce() -> Option<u64>,
) -> String {
    if uri.is_empty() {
        return String::new();
    }
    // 拆分 query/fragment，再在 path 末尾插入 /dxf[/cluster_id]。
    let suffix_at = uri
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '?' | '#').then_some(index))
        .unwrap_or(uri.len());
    let (base, suffix) = uri.split_at(suffix_at);
    let path_start = base
        .find("://")
        .and_then(|scheme| base[scheme + 3..].find('/').map(|path| scheme + 3 + path))
        .unwrap_or(base.len());
    let (authority, path) = base.split_at(path_start);
    let has_prefix = !path.trim_matches('/').is_empty();
    let cluster = if !sem_enabled && has_prefix {
        cluster_id().map(|id| id.to_string())
    } else {
        None
    };
    let mut joined = path.trim_end_matches('/').to_owned();
    joined.push_str("/dxf");
    if let Some(cluster) = cluster {
        joined.push('/');
        joined.push_str(&cluster);
    }
    // objstore.Prefix.ToPath returns a directory prefix with a trailing slash;
    // preserve that Go-visible shape even when the input URI has no query or
    // fragment suffix.
    if !joined.ends_with('/') {
        joined.push('/');
    }
    format!("{authority}{joined}{suffix}")
}

/// 更新“暂停缩容”TTL 标志。
pub fn UpdatePauseScaleInFlag(ctx: &Context, flag: &schstatus::TTLFlag) -> Result<()> {
    runtime()?.update_pause_scale_in_flag(ctx, flag)
}

/// 读取调度调优因子；缺失或 TTL 过期则返回默认值。
pub fn GetScheduleTuneFactors(ctx: &Context, keyspace: &str) -> Result<schstatus::TuneFactors> {
    let Some(factors) = runtime()?.get_schedule_tune_factors(ctx, keyspace)? else {
        return Ok(schstatus::GetDefaultTuneFactors());
    };
    if factors.TTLInfo.ExpireTime < SystemTime::now() {
        Ok(schstatus::GetDefaultTuneFactors())
    } else {
        Ok(factors.TuneFactors)
    }
}

/// 创建带访问计数的对象存储。
pub fn NewObjStoreWithRecording(
    ctx: &Context,
    uri: &str,
) -> Result<(Arc<AccessStats>, Arc<dyn ObjectStorage>)> {
    let recording = Arc::new(AccessStats::default());
    let store = runtime()?.new_object_store(ctx, uri, Some(recording.clone()))?;
    Ok((recording, store))
}

/// 创建不带访问计数的对象存储。
pub fn NewObjStore(ctx: &Context, uri: &str) -> Result<Arc<dyn ObjectStorage>> {
    runtime()?.new_object_store(ctx, uri, None)
}

/// 组装行数/KV 字节等计量字段并写入 Meter；时间戳按分钟对齐。
pub fn SendRowAndSizeMeterData(
    ctx: &Context,
    task: &proto::Task,
    rows: i64,
    data_kv_size: i64,
    index_kv_size: i64,
) -> Result<MeterItem> {
    let updated = task
        .StateUpdateTime
        .duration_since(UNIX_EPOCH)
        .map_err(|error| Error::new(error.to_string()))?;
    let timestamp = (updated.as_secs() / 60 * 60) as i64;
    let duration_seconds = match task.StateUpdateTime.duration_since(task.CreateTime) {
        Ok(duration) => duration.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    };
    let mut item = MeterItem::new();
    item.insert("task_id".to_owned(), MeterValue::Integer(task.ID));
    item.insert(
        "keyspace".to_owned(),
        MeterValue::Text(task.Keyspace.clone()),
    );
    item.insert(
        "task_type".to_owned(),
        MeterValue::Text(task.Type.to_owned()),
    );
    item.insert("row_count".to_owned(), MeterValue::Integer(rows));
    if data_kv_size > 0 {
        item.insert(
            "data_kv_bytes".to_owned(),
            MeterValue::Integer(data_kv_size),
        );
    }
    item.insert(
        "index_kv_bytes".to_owned(),
        MeterValue::Integer(index_kv_size),
    );
    item.insert(
        "required_slots".to_owned(),
        MeterValue::Integer(i64::from(task.RequiredSlots)),
    );
    item.insert(
        "max_node_count".to_owned(),
        MeterValue::Integer(i64::from(task.MaxNodeCount)),
    );
    item.insert(
        "duration_seconds".to_owned(),
        MeterValue::Integer(duration_seconds),
    );
    runtime()?.write_meter_data(ctx, timestamp, &format!("{}_{}", task.Type, task.ID), &item)?;
    Ok(item)
}
