// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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
// DXF 计量 Meter：管理各任务 Recorder、周期性 scrape/flush，并对失败载荷重试。
//
// Classic 内核模式下全局 Meter 为空操作；NextGen 下通过计量 SDK Writer
// 将增量写入对象存储等后端。

// limitations under the License.

use crate::data::{Data, MeterItem};
use crate::recorder::Recorder;
use anyhow::Result;
use log::{info, warn};
use proto::task::TaskBase;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 单次写入超时；不可过长，以免超过 pod 优雅退出宽限期。
/// The timeout cannot be too long because the pod grace period is fixed.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// 计量数据类别标识，写入对象路径时使用。
pub const CATEGORY: &str = "dxf";
/// 失败载荷最大重试次数，超过后丢弃。
pub const MAX_RETRY_COUNT: u32 = 10;
/// 重试循环的等待间隔。
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Flush 周期（毫秒），原子存储以便测试时可调；默认 60s。
/// Go exposes FlushInterval for tests. An atomic millisecond value avoids a
/// mutable global while retaining runtime configurability.
pub static FlushIntervalMillis: AtomicU64 = AtomicU64::new(60_000);

/// 取消上下文的共享状态：标志、互斥锁与条件变量。
#[derive(Default)]
struct ContextState {
    /// 是否已取消。
    cancelled: AtomicBool,
    /// 与 `wake` 配合实现可中断等待。
    mutex: Mutex<()>,
    /// 取消时唤醒等待中的线程。
    wake: Condvar,
}

/// 同步可取消上下文（对齐 Go `context.Context` 在本包中的用法）。
/// Clone 共享取消状态；`with_timeout` 子上下文额外带截止时间。
/// A small synchronous cancellation context mirroring the Go methods used by
/// this package. Clones share cancellation, while timeout children add a
/// deadline visible to the writer.
#[derive(Clone, Default)]
pub struct Context {
    /// 共享取消状态。
    state: Arc<ContextState>,
    /// 可选截止时间（超时子上下文）。
    deadline: Option<Instant>,
}

impl Context {
    /// 永不超时的根上下文。
    pub fn background() -> Self {
        Self::default()
    }

    /// 派生带超时的子上下文（共享取消状态）。
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            state: self.state.clone(),
            deadline: Some(Instant::now() + timeout),
        }
    }

    /// 返回截止时间（若有）。
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// 标记取消并唤醒所有等待者。
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
        self.state.wake.notify_all();
    }

    /// 是否已取消或已过截止时间。
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// 等待取消或指定时长；取消/截止优先时返回 true（对齐 Go `select` on `ctx.Done()`）。
    /// Waits for cancellation or for the requested duration. Returns true when
    /// cancellation/deadline wins, matching a Go `select` on `ctx.Done()`.
    pub fn wait(&self, duration: Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }

        let started = Instant::now();
        let requested_deadline = started + duration;
        let deadline = self
            .deadline
            .map_or(requested_deadline, |value| value.min(requested_deadline));
        let mut guard = self
            .state
            .mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        loop {
            if self.is_cancelled() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return self.is_cancelled();
            }
            let (next_guard, result) = self
                .state
                .wake
                .wait_timeout(guard, deadline.saturating_duration_since(now))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard = next_guard;
            if result.timed_out() {
                return self.is_cancelled();
            }
        }
    }
}

/// 一次写入计量后端的完整载荷。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeteringData {
    /// Writer 实例 UUID（对象名组成部分）。
    pub self_id: String,
    /// Flush 对齐的时间戳（秒）。
    pub timestamp: i64,
    /// 类别，通常为 `dxf`。
    pub category: String,
    /// 各任务的增量计量条目。
    pub items: Vec<MeterItem>,
}

/// 计量写入器抽象（对齐 PingCAP metering SDK）；核心状态机依赖其行为而非具体实现。
/// Boundary implemented by the real PingCAP metering SDK adapter. The SDK has
/// no published Rust crate, so the core state machine depends on its behavior,
/// not on a fabricated replacement implementation.
pub trait MeteringWriter: Send + Sync + 'static {
    /// 写入一条计量载荷。
    fn write(&self, context: &Context, data: MeteringData) -> Result<()>;
    /// 关闭写入器并释放资源。
    fn close(&self) -> Result<()>;
}

/// 计量后端存储配置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeteringConfig {
    /// 存储类型（如 s3、azure）。
    pub storage_type: String,
    /// Bucket / 容器名（可含前缀）。
    pub bucket: String,
    /// 是否允许覆盖同名对象（重试需为 true）。
    pub overwrite_existing: bool,
}

impl MeteringConfig {
    /// 构造默认不覆盖的配置。
    pub fn new(storage_type: impl Into<String>, bucket: impl Into<String>) -> Self {
        Self {
            storage_type: storage_type.into(),
            bucket: bucket.into(),
            overwrite_existing: false,
        }
    }
}

/// 根据配置创建 `MeteringWriter` 的工厂。
pub trait WriterFactory {
    /// 创建写入器实例。
    fn create(&self, config: &MeteringConfig) -> Result<Arc<dyn MeteringWriter>>;
}

/// 进程级全局 Meter 实例槽。
static METERING_INSTANCE: OnceLock<RwLock<Option<Arc<Meter>>>> = OnceLock::new();

/// 惰性初始化并返回全局 Meter 槽。
fn metering_instance() -> &'static RwLock<Option<Arc<Meter>>> {
    METERING_INSTANCE.get_or_init(|| RwLock::new(None))
}

/// 为任务注册/获取共享 Recorder；Classic 或未安装 Meter 时返回空 Recorder。
/// RegisterRecorder returns the shared recorder for a task.
pub fn RegisterRecorder(task: &TaskBase) -> Arc<Recorder> {
    let meter = metering_instance()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if kerneltype::IsClassic() || meter.is_none() {
        return Arc::new(Recorder::default());
    }
    meter
        .unwrap()
        .get_or_register_recorder(Recorder::new(task.ID, &task.Keyspace, task.Type))
}

/// 标记 recorder 待注销，待最后一次 flush 完成后真正移除。
/// UnregisterRecorder marks a recorder for one final flush.
pub fn UnregisterRecorder(task_id: i64) {
    let meter = metering_instance()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if kerneltype::IsClassic() {
        return;
    }
    if let Some(meter) = meter {
        meter.unregister_recorder(task_id);
    }
}

/// 委托已安装的 Meter 写入；Classic 或无 Meter 时为空操作。
/// WriteMeterData delegates to the installed meter. Classic mode and an empty
/// global meter remain no-ops as in Go.
pub fn WriteMeterData(
    context: &Context,
    timestamp: i64,
    uuid: impl Into<String>,
    items: Vec<MeterItem>,
) -> Result<()> {
    let meter = metering_instance()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if kerneltype::IsClassic() || meter.is_none() {
        return Ok(());
    }
    meter
        .unwrap()
        .write_meter_data(context, timestamp, uuid.into(), items)
}

/// 原子替换或清空进程级 Meter。
/// SetMetering atomically replaces or clears the process-wide meter.
pub fn SetMetering(meter: Option<Arc<Meter>>) {
    *metering_instance()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = meter;
}

/// 包装 Recorder，并记录是否已请求注销。
#[derive(Clone)]
struct WrappedRecorder {
    /// 共享的用量累计器。
    recorder: Arc<Recorder>,
    /// 为 true 表示等待最终 flush 后移除。
    unregistered: bool,
}

/// 写入失败后待按原时间戳重试的载荷。
#[derive(Clone)]
struct WriteFailData {
    /// 原始 flush 时间戳。
    timestamp: i64,
    /// 已重试次数。
    retry_count: u32,
    /// 失败时的完整增量条目。
    items: Vec<MeterItem>,
}

/// Meter 可变状态：recorder 表、上次 flush 快照、待重试载荷。
#[derive(Default)]
struct MeterState {
    /// task_id -> 包装后的 recorder。
    recorders: HashMap<i64, WrappedRecorder>,
    /// 各任务上次成功 scrape 后的快照，用于算增量。
    last_flushed_data: HashMap<i64, Data>,
    /// timestamp -> 待重试失败载荷。
    pending_retry_data: HashMap<i64, WriteFailData>,
}

/// 拥有 recorder 快照、失败载荷与 SDK Writer 的计量核心。
/// Meter owns recorder snapshots, failed payloads, and the SDK writer.
pub struct Meter {
    /// 可变状态互斥保护。
    state: Mutex<MeterState>,
    /// 本 Meter 实例的 UUID（对象名用）。
    uuid: String,
    /// 计量写入器。
    writer: Arc<dyn MeteringWriter>,
}

impl Meter {
    /// 当 storage_type 与 bucket 均非空时构造 Writer；否则返回 None（禁用计量）。
    /// Constructs the SDK writer when both required storage fields are present.
    pub fn new(config: &MeteringConfig, factory: &dyn WriterFactory) -> Result<Option<Arc<Self>>> {
        if config.storage_type.is_empty() || config.bucket.is_empty() {
            return Ok(None);
        }
        let mut writer_config = config.clone();
        // 重试使用相同时间戳因而对象名相同，必须允许覆盖。
        // A retry uses the same timestamp and therefore the same object name.
        writer_config.overwrite_existing = true;
        let writer = factory.create(&writer_config)?;
        Ok(Some(Self::with_writer(writer)))
    }

    /// 使用给定 Writer 构造 Meter（测试与工厂路径共用）。
    pub fn with_writer(writer: Arc<dyn MeteringWriter>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(MeterState::default()),
            uuid: uuid::Uuid::new_v4().to_string().replace('-', "_"),
            writer,
        })
    }

    /// 获取状态锁；poison 时仍取出内部数据。
    fn lock_state(&self) -> std::sync::MutexGuard<'_, MeterState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 按 task_id 注册或返回已有 Recorder；若曾注销则清除注销标记。
    pub(crate) fn get_or_register_recorder(&self, recorder: Recorder) -> Arc<Recorder> {
        let task_id = recorder.task_id_for_meter();
        let mut state = self.lock_state();
        if let Some(old) = state.recorders.get_mut(&task_id) {
            old.unregistered = false;
            return old.recorder.clone();
        }
        let recorder = Arc::new(recorder);
        state.recorders.insert(
            task_id,
            WrappedRecorder {
                recorder: recorder.clone(),
                unregistered: false,
            },
        );
        recorder
    }

    /// 标记 recorder 待注销（不立即删除）。
    pub(crate) fn unregister_recorder(&self, task_id: i64) {
        if let Some(recorder) = self.lock_state().recorders.get_mut(&task_id) {
            recorder.unregistered = true;
        }
    }

    /// 抓取所有 recorder 的当前累计快照。
    fn scrape_current_data(&self) -> HashMap<i64, Data> {
        let recorders: Vec<(i64, Arc<Recorder>)> = self
            .lock_state()
            .recorders
            .iter()
            .map(|(task_id, wrapped)| (*task_id, wrapped.recorder.clone()))
            .collect();
        recorders
            .into_iter()
            .map(|(task_id, recorder)| (task_id, recorder.curr_data()))
            .collect()
    }

    /// 相对 last_flushed_data 计算各任务正增量条目。
    fn calculate_data_items(&self, current_data: &HashMap<i64, Data>) -> Vec<MeterItem> {
        let state = self.lock_state();
        current_data
            .iter()
            .filter_map(|(task_id, current)| {
                let previous = state
                    .last_flushed_data
                    .get(task_id)
                    .cloned()
                    .unwrap_or_default();
                current.cal_meter_data_item(&previous)
            })
            .collect()
    }

    /// Flush 后更新快照，并移除已注销且数据已对齐的 recorder。
    fn after_flush(&self, current_data: HashMap<i64, Data>) {
        let mut state = self.lock_state();
        state.last_flushed_data = current_data;
        let remove_ids: Vec<i64> = state
            .recorders
            .iter()
            .filter_map(|(task_id, wrapped)| {
                if !wrapped.unregistered {
                    return None;
                }
                let flushed = state.last_flushed_data.get(task_id)?;
                flushed
                    .equals(&wrapped.recorder.curr_data())
                    .then_some(*task_id)
            })
            .collect();

        for task_id in remove_ids {
            if let Some(removed) = state.recorders.remove(&task_id) {
                state.last_flushed_data.remove(&task_id);
                info!(
                    "recorder unregistered and finished final flush: {}",
                    removed.recorder.curr_data()
                );
            }
        }
    }

    /// 记录失败载荷，供后续按原时间戳重试。
    pub(crate) fn add_failed_data(&self, timestamp: i64, items: Vec<MeterItem>) {
        self.lock_state().pending_retry_data.insert(
            timestamp,
            WriteFailData {
                timestamp,
                retry_count: 0,
                items,
            },
        );
    }

    /// 遍历待重试载荷再次写入；达到上限则丢弃并打日志。
    pub(crate) fn retry_write(&self, context: &Context) {
        let pending: Vec<WriteFailData> = self
            .lock_state()
            .pending_retry_data
            .values()
            .cloned()
            .collect();
        if pending.is_empty() {
            return;
        }

        let mut first_error = None;
        for failed in pending {
            match self.write_meter_data(
                context,
                failed.timestamp,
                self.uuid.clone(),
                failed.items.clone(),
            ) {
                Ok(()) => {
                    info!(
                        "succeeded writing metering data after {} retries at {}",
                        failed.retry_count, failed.timestamp
                    );
                    self.lock_state()
                        .pending_retry_data
                        .remove(&failed.timestamp);
                }
                Err(error) => {
                    if context.is_cancelled() {
                        break;
                    }
                    if first_error.is_none() {
                        first_error = Some(error.to_string());
                    }
                    let mut state = self.lock_state();
                    if let Some(current) = state.pending_retry_data.get_mut(&failed.timestamp) {
                        current.retry_count += 1;
                        if current.retry_count >= MAX_RETRY_COUNT {
                            warn!(
                                "dropping metering data at {} after {} retries",
                                current.timestamp, current.retry_count
                            );
                            state.pending_retry_data.remove(&failed.timestamp);
                        }
                    }
                }
            }
        }
        if let Some(error) = first_error {
            warn!("failed to retry writing some metering data: {error}");
        }
    }

    /// 按 RETRY_INTERVAL 周期调用 `retry_write`，直到取消。
    fn retry_loop(&self, context: &Context) {
        while !context.wait(RETRY_INTERVAL) {
            self.retry_write(context);
        }
    }

    /// 按 FlushInterval 对齐墙钟整点周期 flush；取消后再做一次尽力最终 flush。
    fn flush_loop(self: &Arc<Self>, context: &Context) {
        let interval_ms = FlushIntervalMillis.load(Ordering::Relaxed).max(1);
        let now_ms = unix_millis();
        let mut next_ms = now_ms - (now_ms % interval_ms) + interval_ms;
        while !context.is_cancelled() {
            let wait_ms = next_ms.saturating_sub(unix_millis());
            if context.wait(Duration::from_millis(wait_ms)) {
                break;
            }
            self.flush(context, (next_ms / 1_000) as i64);
            next_ms = next_ms.saturating_add(interval_ms);
        }

        // Best effort final flush after cancellation, exactly as the Go loop.
        self.flush(context, (next_ms / 1_000) as i64);
        if let Err(error) = self.writer.close() {
            warn!("metering writer close failed: {error}");
        }
    }

    /// 启动 flush 与 retry 两个循环，并等待二者退出。
    /// StartFlushLoop runs flush and retry loops and waits for both to exit.
    pub fn StartFlushLoop(self: Arc<Self>, context: Context) {
        let retry_meter = self.clone();
        let retry_context = context.clone();
        let retry = std::thread::spawn(move || retry_meter.retry_loop(&retry_context));
        self.flush_loop(&context);
        if retry.join().is_err() {
            warn!("metering retry loop panicked");
        }
    }

    /// 执行一次 scrape→算增量→写入；失败则保留载荷重试，但仍推进快照。
    pub(crate) fn flush(&self, context: &Context, timestamp: i64) {
        let started = Instant::now();
        let current_data = self.scrape_current_data();
        let items = self.calculate_data_items(&current_data);
        if items.is_empty() {
            info!(
                "no metering data to flush at {timestamp}; recorders={}, duration={:?}",
                current_data.len(),
                started.elapsed()
            );
            self.after_flush(current_data);
            return;
        }

        if let Err(error) =
            self.write_meter_data(context, timestamp, self.uuid.clone(), items.clone())
        {
            warn!(
                "failed to write metering data at {timestamp}: {error}; duration={:?}",
                started.elapsed()
            );
            // 常规下一次 flush 从当前快照起算增量；失败的精确载荷仅按原时间戳重试。
            // The next regular flush starts from the current snapshot. The exact
            // failed payload is retried only with its original timestamp.
            self.add_failed_data(timestamp, items);
        } else {
            info!(
                "succeeded writing metering data at {timestamp}; duration={:?}",
                started.elapsed()
            );
        }
        self.after_flush(current_data);
    }

    /// 带 WRITE_TIMEOUT 调用 Writer；失败时递增计量失败指标。
    pub fn write_meter_data(
        &self,
        context: &Context,
        timestamp: i64,
        uuid: String,
        items: Vec<MeterItem>,
    ) -> Result<()> {
        let flush_context = context.with_timeout(WRITE_TIMEOUT);
        let result = self.writer.write(
            &flush_context,
            MeteringData {
                self_id: uuid,
                timestamp,
                category: CATEGORY.to_owned(),
                items,
            },
        );
        if result.is_err() {
            dxfmetric::InitDistTaskMetrics()
                .ExecuteEventCounter
                .with_label_values(&["-", dxfmetric::EventMeterWriteFailed])
                .inc();
        }
        result
    }

    /// 关闭底层 Writer。
    pub fn Close(&self) -> Result<()> {
        let result = self.writer.close();
        if let Err(error) = &result {
            warn!("failed to close metering writer: {error}");
        }
        result
    }

    /// 测试辅助：是否仍持有该 task 的 recorder。
    #[cfg(test)]
    pub(crate) fn contains_recorder(&self, task_id: i64) -> bool {
        self.lock_state().recorders.contains_key(&task_id)
    }

    /// 测试辅助：recorder 是否已标记注销。
    #[cfg(test)]
    pub(crate) fn is_unregistered(&self, task_id: i64) -> bool {
        self.lock_state()
            .recorders
            .get(&task_id)
            .is_some_and(|recorder| recorder.unregistered)
    }

    /// 测试辅助：待重试载荷数量。
    #[cfg(test)]
    pub(crate) fn pending_retry_len(&self) -> usize {
        self.lock_state().pending_retry_data.len()
    }

    /// 测试辅助：读取某任务上次 flush 快照。
    #[cfg(test)]
    pub(crate) fn last_flushed_data(&self, task_id: i64) -> Option<Data> {
        self.lock_state().last_flushed_data.get(&task_id).cloned()
    }
}

/// 当前 Unix 毫秒时间戳。
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
