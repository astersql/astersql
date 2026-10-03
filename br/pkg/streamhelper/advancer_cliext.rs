// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Advancer 对元数据客户端的扩展：任务事件流与 V3 全局检查点读写。
//! 与 Go `advancer_cliext.go` 对齐；`Begin` 将现有任务投递为 `EventAdd`。

use crate::client::MetaDataClient;
use crate::models::{GlobalCheckpointOf, PrefixOfPause, PrefixOfTask, encodeUint64};
use crate::stubs::{
    KeyRange, MetadataRequestContext, MetadataRequestError, StreamBackupTaskInfo, WatchContext,
    WatchEvent, WatchEventType,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

const METADATA_REQUEST_TIMEOUTS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(15),
];

pub(crate) fn runMetadataRequestWithRetry<T: Send + 'static>(
    ctx: &WatchContext,
    timeouts: &[Duration],
    on_timeout: Option<std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    request: impl Fn(MetadataRequestContext) -> Result<T, MetadataRequestError> + Send + Sync + 'static,
) -> Result<T, String> {
    let request = std::sync::Arc::new(request);
    let mut last_error = "context deadline exceeded".to_string();
    for (attempt, timeout) in timeouts.iter().enumerate() {
        if ctx.is_canceled() {
            return Err("watch canceled".into());
        }
        let request_ctx = MetadataRequestContext::new(ctx.clone(), *timeout);
        let worker_ctx = request_ctx.clone();
        let request = request.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let _ = tx.send(request(worker_ctx));
        });
        let result = loop {
            match rx.recv_timeout(request_ctx.remaining().min(Duration::from_millis(10))) {
                Ok(value) => break value,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(MetadataRequestError::Other(
                        "metadata request worker disconnected".into(),
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Err(error) = request_ctx.check() {
                        request_ctx.cancel();
                        if matches!(error, MetadataRequestError::DeadlineExceeded) {
                            if let Some(reset) = &on_timeout {
                                if let Err(error) = reset() {
                                    break Err(MetadataRequestError::Other(error));
                                }
                            }
                        }
                        break Err(error);
                    }
                }
            }
        };
        request_ctx.cancel();
        worker
            .join()
            .map_err(|_| "metadata request worker panicked".to_string())?;
        match result {
            Ok(value) => return Ok(value),
            Err(error) => {
                let retryable = matches!(
                    error,
                    MetadataRequestError::DeadlineExceeded | MetadataRequestError::Unavailable(_)
                );
                last_error = error.to_string();
                if ctx.is_canceled() {
                    return Err("watch canceled".into());
                }
                if !retryable || attempt + 1 == timeouts.len() {
                    return Err(last_error);
                }
            }
        }
    }
    Err(last_error)
}

static METADATA_WATCH_PROGRESS_INTERVAL_MILLIS: AtomicU64 = AtomicU64::new(30_000);
static METADATA_WATCH_IDLE_TIMEOUT_MILLIS: AtomicU64 = AtomicU64::new(90_000);

fn metadataWatchProgressInterval() -> Duration {
    Duration::from_millis(METADATA_WATCH_PROGRESS_INTERVAL_MILLIS.load(Ordering::Acquire))
}

fn metadataWatchIdleTimeout() -> Duration {
    Duration::from_millis(METADATA_WATCH_IDLE_TIMEOUT_MILLIS.load(Ordering::Acquire))
}

pub(crate) fn setMetadataWatchProgressForTest(
    interval: Duration,
    timeout: Duration,
) -> (Duration, Duration) {
    let old_interval =
        METADATA_WATCH_PROGRESS_INTERVAL_MILLIS.swap(interval.as_millis() as u64, Ordering::AcqRel);
    let old_timeout =
        METADATA_WATCH_IDLE_TIMEOUT_MILLIS.swap(timeout.as_millis() as u64, Ordering::AcqRel);
    (
        Duration::from_millis(old_interval),
        Duration::from_millis(old_timeout),
    )
}

static LAST_CHECKPOINT_METRIC: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

/// 返回上传成功后记录的任务检查点指标值。
pub fn lastCheckpointMetric(taskName: &str) -> Option<u64> {
    LAST_CHECKPOINT_METRIC
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .get(taskName)
        .copied()
}

/// 日志备份任务变更事件类型（增删/错误/暂停/恢复）。
/// 数值与 Go `EventType` iota 保持一致，便于序列化对照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    /// 新增或发现任务。
    EventAdd = 0,
    /// 任务被删除。
    EventDel = 1,
    /// 监听/解析出错。
    EventErr = 2,
    /// 任务进入暂停。
    EventPause = 3,
    /// 任务从暂停恢复。
    EventResume = 4,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            EventType::EventAdd => "Add",
            EventType::EventDel => "Del",
            EventType::EventErr => "Err",
            EventType::EventPause => "Pause",
            EventType::EventResume => "Resume",
        };
        write!(f, "{s}")
    }
}

/// 单条任务事件：类型、名称、可选任务信息/范围或错误。
/// `Begin` 路径通常只填 Type/Name/Info；监听路径可附带 Ranges。
#[derive(Clone, Debug)]
pub struct TaskEvent {
    /// 事件类别。
    pub Type: EventType,
    /// 任务名；`EventErr` 时可为空。
    pub Name: String,
    /// 任务元信息（Add 时必有）。
    pub Info: Option<StreamBackupTaskInfo>,
    /// 任务 key 范围；本端口 Begin 暂不填充。
    pub Ranges: Vec<KeyRange>,
    /// 错误详情；仅 `EventErr` 使用。
    pub Err: Option<String>,
}

impl std::fmt::Display for TaskEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(err) = &self.Err {
            write!(f, "{}({}, err = {})", self.Type, self.Name, err)
        } else {
            write!(f, "{}({})", self.Type, self.Name)
        }
    }
}

/// 构造仅携带错误信息的 `EventErr`，供监听失败路径上报。
pub fn errorEvent(err: String) -> TaskEvent {
    TaskEvent {
        Type: EventType::EventErr,
        Name: String::new(),
        Info: None,
        Ranges: Vec::new(),
        Err: Some(err),
    }
}

/// 解析 etcd 中 8 字节大端全局检查点；长度不符则报错。
pub fn parseGlobalCheckpointValue(value: &[u8]) -> Result<u64, String> {
    if value.len() != 8 {
        return Err(format!(
            "the global checkpoint isn't 64bits (it is {} bytes, value = {:?})",
            value.len(),
            value
        ));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(value);
    Ok(u64::from_be_bytes(buf))
}

/// 推进器侧元数据扩展：基于 `MetaDataClient` 读写任务与全局检查点。
#[derive(Clone)]
pub struct AdvancerExt {
    pub meta: MetaDataClient,
}

impl AdvancerExt {
    fn getFullTasksAsEvent(&self) -> Result<(Vec<TaskEvent>, i64), String> {
        let (tasks, revision) = self.meta.GetAllTasksWithRevision()?;
        let mut events = Vec::with_capacity(tasks.len());
        for t in tasks {
            let ranges = t.Ranges()?;
            events.push(TaskEvent {
                Type: EventType::EventAdd,
                Name: t.Info.Name.clone(),
                Info: Some(t.Info.clone()),
                Ranges: ranges,
                Err: None,
            });
        }
        Ok((events, revision))
    }

    /// 仅投递当前快照，供仍使用同步 `StreamMeta` 适配层的调用方过渡。
    pub fn BeginSnapshot(&self, ch: &mut Vec<TaskEvent>) -> Result<(), String> {
        let (events, _) = self.getFullTasksAsEvent()?;
        ch.extend(events);
        Ok(())
    }

    /// 投递当前任务快照，并从同一 revision 的下一版本持续监听任务与暂停事件。
    pub fn Begin(&self, ctx: WatchContext, ch: mpsc::Sender<TaskEvent>) -> Result<(), String> {
        let (initial, revision) = self.getFullTasksAsEvent()?;
        for event in initial {
            ch.send(event).map_err(|err| err.to_string())?;
        }
        let task_watch = self.meta.KV.WatchPrefix(&PrefixOfTask(), revision + 1)?;
        let pause_watch = self.meta.KV.WatchPrefix(&PrefixOfPause(), revision + 1)?;
        let this = self.clone();
        std::thread::spawn(move || {
            let mut last_progress = Instant::now();
            let mut last_progress_request = Instant::now();
            loop {
                if ctx.is_canceled() {
                    let _ = ch.send(errorEvent("watch canceled".into()));
                    return;
                }
                let mut progressed = false;
                for receiver in [&task_watch, &pause_watch] {
                    loop {
                        match receiver.try_recv() {
                            Ok(event) => {
                                progressed = true;
                                if event.Type == WatchEventType::Progress {
                                    continue;
                                }
                                let task_event = this.toTaskEvent(event).unwrap_or_else(errorEvent);
                                if ch.send(task_event).is_err() {
                                    return;
                                }
                            }
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                let _ = ch.send(errorEvent("watch channel closed".into()));
                                return;
                            }
                        }
                    }
                }
                if progressed {
                    last_progress = Instant::now();
                } else if last_progress.elapsed() >= metadataWatchIdleTimeout() {
                    let _ = ch.send(errorEvent(format!(
                        "watching task metadata timed out after {:?} without etcd progress",
                        metadataWatchIdleTimeout()
                    )));
                    return;
                }
                if last_progress_request.elapsed() >= metadataWatchProgressInterval() {
                    if let Err(err) = this.meta.KV.RequestWatchProgress() {
                        let _ = ch.send(errorEvent(err));
                        return;
                    }
                    last_progress_request = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(())
    }

    fn toTaskEvent(&self, event: WatchEvent) -> Result<TaskEvent, String> {
        let task_prefix = PrefixOfTask();
        let pause_prefix = PrefixOfPause();
        let (prefix, task_event) = if event.Key.starts_with(task_prefix.as_bytes()) {
            (&task_prefix, true)
        } else if event.Key.starts_with(pause_prefix.as_bytes()) {
            (&pause_prefix, false)
        } else {
            return Err(format!(
                "the path isn't a task/pause path ({})",
                String::from_utf8_lossy(&event.Key)
            ));
        };
        let name = String::from_utf8_lossy(&event.Key[prefix.len()..]).into_owned();
        let Type = match (event.Type, task_event) {
            (WatchEventType::Put, true) => EventType::EventAdd,
            (WatchEventType::Delete, true) => EventType::EventDel,
            (WatchEventType::Put, false) => EventType::EventPause,
            (WatchEventType::Delete, false) => EventType::EventResume,
            (WatchEventType::Progress, _) => {
                return Err("progress notification is not a task event".into());
            }
        };
        let (Info, Ranges) = if task_event && event.Type == WatchEventType::Put {
            let info: StreamBackupTaskInfo =
                serde_json::from_slice(&event.Value).map_err(|err| err.to_string())?;
            let ranges = self.meta.TaskByInfo(info.clone()).Ranges()?;
            (Some(info), ranges)
        } else {
            (None, Vec::new())
        };
        Ok(TaskEvent {
            Type,
            Name: name,
            Info,
            Ranges,
            Err: None,
        })
    }

    /// 读取任务的 V3 全局检查点；键缺失时返回 0。
    pub fn GetGlobalCheckpointForTask(&self, taskName: &str) -> Result<u64, String> {
        let key = GlobalCheckpointOf(taskName);
        let value = self.meta.KV.Get(&key)?;
        if value.is_empty() {
            return Ok(0);
        }
        parseGlobalCheckpointValue(&value)
    }

    /// 上传 V3 全局检查点；仅当新值不小于旧值时才写入（单调递增）。
    pub fn UploadV3GlobalCheckpointForTask(
        &self,
        taskName: &str,
        checkpoint: u64,
    ) -> Result<(), String> {
        let key = GlobalCheckpointOf(taskName);
        let old = self.GetGlobalCheckpointForTask(taskName)?;
        // 防止回退：落后于已持久化的全局检查点则跳过。
        if checkpoint < old {
            return Ok(());
        }
        let kv = self.meta.KV.clone();
        let encoded = encodeUint64(checkpoint);
        runMetadataRequestWithRetry(
            &WatchContext::new(),
            &METADATA_REQUEST_TIMEOUTS,
            None,
            move |ctx| kv.PutWithRequestContext(&ctx, &key, &encoded),
        )?;
        LAST_CHECKPOINT_METRIC
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .insert(taskName.to_string(), checkpoint);
        Ok(())
    }

    /// 清除任务的 V3 全局检查点键。
    pub fn ClearV3GlobalCheckpointForTask(&self, taskName: &str) -> Result<(), String> {
        self.meta.KV.Delete(&GlobalCheckpointOf(taskName))
    }
}

impl MetaDataClient {
    fn getGlobalCheckpointWithRevision(
        &self,
        ctx: &WatchContext,
        taskName: &str,
    ) -> Result<(u64, i64), String> {
        let kv = self.KV.clone();
        let key = GlobalCheckpointOf(taskName);
        let value =
            runMetadataRequestWithRetry(ctx, &METADATA_REQUEST_TIMEOUTS, None, move |ctx| {
                kv.GetWithRequestContext(&ctx, &key)
            })?;
        let Some(bytes) = value.Value else {
            return Ok((0, value.Revision));
        };
        Ok((parseGlobalCheckpointValue(&bytes)?, value.Revision))
    }

    /// 等待任务全局检查点严格推进；从读取 revision+1 watch，避免读/监听竞态。
    pub fn WaitGlobalCheckpointAdvance(
        &self,
        ctx: WatchContext,
        taskName: &str,
        current: u64,
    ) -> Result<(), String> {
        let key = GlobalCheckpointOf(taskName);
        loop {
            let (checkpoint, revision) = self.getGlobalCheckpointWithRevision(&ctx, taskName)?;
            if checkpoint > current {
                return Ok(());
            }
            let kv = self.KV.clone();
            let watch_key = key.clone();
            let reset_kv = kv.clone();
            let watch = runMetadataRequestWithRetry(
                &ctx,
                &METADATA_REQUEST_TIMEOUTS,
                Some(std::sync::Arc::new(move || reset_kv.ResetWatcher())),
                move |request_ctx| {
                    let result =
                        kv.WatchPrefixWithRequestContext(&request_ctx, &watch_key, revision + 1);
                    if matches!(result, Err(MetadataRequestError::DeadlineExceeded))
                        && !request_ctx.was_cancelled()
                    {
                        kv.ResetWatcher().map_err(MetadataRequestError::Other)?;
                    }
                    result
                },
            )
            .map_err(|error| {
                if error == "context deadline exceeded" && !ctx.is_canceled() {
                    "PiTR checkpoint watch restart required".into()
                } else {
                    error
                }
            })?;
            let mut last_progress = Instant::now();
            let mut last_progress_request = Instant::now();
            loop {
                if ctx.is_canceled() {
                    return Err("watch canceled".into());
                }
                match watch.recv_timeout(Duration::from_millis(10)) {
                    Ok(event) => {
                        last_progress = Instant::now();
                        if event.Type == WatchEventType::Progress {
                            continue;
                        }
                        if event.Type != WatchEventType::Put || event.Key != key.as_bytes() {
                            continue;
                        }
                        if parseGlobalCheckpointValue(&event.Value)? > current {
                            return Ok(());
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if last_progress.elapsed() >= metadataWatchIdleTimeout() {
                            return Err(format!(
                                "watching global checkpoint timed out after {:?} without etcd progress",
                                metadataWatchIdleTimeout()
                            ));
                        }
                        if last_progress_request.elapsed() >= metadataWatchProgressInterval() {
                            self.KV.RequestWatchProgress()?;
                            last_progress_request = Instant::now();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }
    }
}
