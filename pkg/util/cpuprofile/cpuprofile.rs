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

// 并行 CPU profiler：单进程全局采样，多消费者非阻塞分发 pprof 数据。
//
// 对应 Go `runtime/pprof` 风格：同时只允许一个采样器；注册消费者后按固定间隔
// 采集并 `try_send` 到各 channel；无消费者时不启动采样。对外提供全局
// `StartCPUProfiler` / `Register` 等入口。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use pprof::protos::Message;

/// The Go package exposes this duration for tests. Rust stores milliseconds in
/// an atomic so reads and updates remain data-race free.
/// 默认 profiling 间隔（毫秒，原子存储便于测试动态调整）。
pub static DefProfileDuration: AtomicU64 = AtomicU64::new(1_000);

/// 累计启动采样次数（有消费者时每次开启一轮 +1）。
static CPU_PROFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// CPU profile 相关错误，携带可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct CpuProfileError {
    message: String,
}

impl CpuProfileError {
    /// 由任意可转为 String 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<pprof::Error> for CpuProfileError {
    fn from(error: pprof::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<prost::DecodeError> for CpuProfileError {
    fn from(error: prost::DecodeError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<prost::EncodeError> for CpuProfileError {
    fn from(error: prost::EncodeError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<std::io::Error> for CpuProfileError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// 设置全局默认 profiling 间隔（至少 1ms）。
pub fn set_profile_duration(duration: Duration) {
    let millis = duration.as_millis().max(1).min(u64::MAX as u128) as u64;
    DefProfileDuration.store(millis, Ordering::SeqCst);
}

/// 读取当前默认 profiling 间隔。
pub fn profile_duration() -> Duration {
    Duration::from_millis(DefProfileDuration.load(Ordering::SeqCst).max(1))
}

/// 返回已启动的采样轮次计数。
pub fn cpu_profile_count() -> u64 {
    CPU_PROFILE_COUNTER.load(Ordering::Relaxed)
}

/// A consumer receives each completed interval. Sending is deliberately
/// nonblocking, matching the Go channel `select { case c <- data: default: }`.
/// 消费者发送端类型：非阻塞投递完整区间的 profile 数据。
pub type ProfileConsumer = crossbeam_channel::Sender<Arc<ProfileData>>;

/// 一轮采样结果：成功时带 protobuf 字节，失败时带错误。
#[derive(Clone, Debug, Default)]
pub struct ProfileData {
    pub Data: Vec<u8>,
    pub Error: Option<CpuProfileError>,
}

impl ProfileData {
    /// 构造成功结果。
    pub fn success(data: Vec<u8>) -> Self {
        Self {
            Data: data,
            Error: None,
        }
    }

    /// 构造失败结果（空 Data）。
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            Data: Vec::new(),
            Error: Some(CpuProfileError::new(message)),
        }
    }
}

/// profiler 共享核心：消费者列表、唤醒通道、停止标志与上次数据大小。
struct ProfilerCore {
    consumers: Mutex<Vec<ProfileConsumer>>,
    notify_register: crossbeam_channel::Sender<()>,
    notify_recv: crossbeam_channel::Receiver<()>,
    stopped: AtomicBool,
    last_data_size: AtomicUsize,
}

impl ProfilerCore {
    /// 当前已注册消费者数量。
    fn consumers_count(&self) -> usize {
        self.consumers
            .lock()
            .expect("profiler consumer mutex poisoned")
            .len()
    }

    /// 非阻塞地向所有消费者投递同一份数据。
    fn send(&self, data: Arc<ProfileData>) {
        let consumers = self
            .consumers
            .lock()
            .expect("profiler consumer mutex poisoned");
        for consumer in consumers.iter() {
            let _ = consumer.try_send(data.clone());
        }
    }
}

/// Only one running instance is supported because pprof-rs, like Go's
/// runtime/pprof, installs a process-global CPU sampler.
/// 并行 CPU profiler：后台循环按间隔采样并分发给消费者。
pub struct parallelCPUProfiler {
    core: Arc<ProfilerCore>,
    profileData: Option<Arc<ProfileData>>,
    wg: Option<JoinHandle<()>>,
    started: bool,
}

/// 构造未启动的并行 profiler。
pub fn newParallelCPUProfiler() -> parallelCPUProfiler {
    let (notify_register, notify_recv) = crossbeam_channel::bounded(1);
    parallelCPUProfiler {
        core: Arc::new(ProfilerCore {
            consumers: Mutex::new(Vec::new()),
            notify_register,
            notify_recv,
            stopped: AtomicBool::new(false),
            last_data_size: AtomicUsize::new(0),
        }),
        profileData: None,
        wg: None,
        started: false,
    }
}

/// “已启动”错误工厂，与 Go 文案一致。
pub fn errProfilerAlreadyStarted() -> CpuProfileError {
    CpuProfileError::new("parallelCPUProfiler is already started")
}

impl parallelCPUProfiler {
    /// 启动后台 profiling 循环；重复启动返回错误。
    pub fn start(&mut self) -> Result<(), CpuProfileError> {
        if self.started {
            return Err(errProfilerAlreadyStarted());
        }
        self.started = true;
        self.core.stopped.store(false, Ordering::SeqCst);
        // 清空可能残留的唤醒通知，避免立刻空转。
        while self.core.notify_recv.try_recv().is_ok() {}
        let core = self.core.clone();
        self.wg = Some(std::thread::spawn(move || {
            if catch_unwind(AssertUnwindSafe(|| profiling_loop(core))).is_err() {
                log::error!("parallel cpu profiler panicked");
            }
        }));
        log::info!("parallel cpu profiler started");
        Ok(())
    }

    /// 停止后台循环并 join worker。
    pub fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.started = false;
        self.core.stopped.store(true, Ordering::SeqCst);
        let _ = self.core.notify_register.try_send(());
        if let Some(handle) = self.wg.take() {
            if handle.join().is_err() {
                log::error!("parallel cpu profiler worker join failed");
            }
        }
        log::info!("parallel cpu profiler stopped");
    }

    /// 注册消费者；同一 channel 不重复添加，并唤醒循环。
    pub fn register(&mut self, consumer: ProfileConsumer) {
        let mut consumers = self
            .core
            .consumers
            .lock()
            .expect("profiler consumer mutex poisoned");
        if !consumers
            .iter()
            .any(|registered| registered.same_channel(&consumer))
        {
            consumers.push(consumer);
        }
        drop(consumers);
        let _ = self.core.notify_register.try_send(());
    }

    /// 按 channel 身份移除消费者。
    pub fn unregister(&mut self, consumer: ProfileConsumer) {
        self.core
            .consumers
            .lock()
            .expect("profiler consumer mutex poisoned")
            .retain(|registered| !registered.same_channel(&consumer));
    }

    /// 已注册消费者数量。
    pub fn consumersCount(&self) -> usize {
        self.core.consumers_count()
    }

    /// 测试辅助：注入待发送的 profile 数据。
    pub fn set_profile_data(&mut self, data: ProfileData) {
        self.profileData = Some(Arc::new(data));
    }

    /// 是否已有待发送数据。
    pub fn has_profile_data(&self) -> bool {
        self.profileData.is_some()
    }

    /// 取出并分发已注入的 profile 数据。
    pub fn sendToConsumers(&mut self) {
        if let Some(data) = self.profileData.take() {
            self.core.send(data);
        }
    }
}

/// 后台主循环：按间隔结束上一轮采样、向消费者发送，并在有消费者时开启新一轮。
fn profiling_loop(core: Arc<ProfilerCore>) {
    let mut active: Option<pprof::ProfilerGuard<'static>> = None;

    loop {
        let _ = core.notify_recv.recv_timeout(profile_duration());
        if core.stopped.load(Ordering::SeqCst) {
            break;
        }

        if let Some(guard) = active.take() {
            let data = finish_profile(&guard, core.last_data_size.load(Ordering::Relaxed));
            drop(guard);
            core.last_data_size
                .store(data.Data.len(), Ordering::Relaxed);
            core.send(Arc::new(data));
        }

        if core.consumers_count() == 0 {
            continue;
        }

        CPU_PROFILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        match pprof::ProfilerGuardBuilder::default()
            .frequency(100)
            .blocklist(&["libc", "libgcc", "pthread", "vdso"])
            .build()
        {
            Ok(guard) => active = Some(guard),
            Err(error) => core.send(Arc::new(ProfileData::failure(error.to_string()))),
        }
    }

    // Dropping the guard stops a currently running interval. Go does the same
    // in the profiling-loop defer without publishing a partial final profile.
    drop(active);
}

/// 将当前 guard 的报告编码为 pprof protobuf；按上次大小预分配缓冲。
fn finish_profile(guard: &pprof::ProfilerGuard<'_>, last_data_size: usize) -> ProfileData {
    let result = guard.report().build().and_then(|report| report.pprof());
    match result {
        Ok(profile) => {
            let capacity = (last_data_size / 4096 + 1) * 4096;
            let mut data = Vec::with_capacity(capacity);
            match profile.encode(&mut data) {
                Ok(()) => {
                    if data.is_empty() {
                        ProfileData::failure("pprof report encoded to empty buffer")
                    } else {
                        ProfileData::success(data)
                    }
                }
                Err(error) => ProfileData::failure(error.to_string()),
            }
        }
        Err(error) => ProfileData::failure(error.to_string()),
    }
}

/// 进程内唯一全局 profiler 实例。
fn global_profiler() -> &'static Mutex<parallelCPUProfiler> {
    static GLOBAL: OnceLock<Mutex<parallelCPUProfiler>> = OnceLock::new();
    GLOBAL.get_or_init(|| Mutex::new(newParallelCPUProfiler()))
}

/// 启动全局 CPU profiler。
pub fn StartCPUProfiler() -> Result<(), CpuProfileError> {
    global_profiler()
        .lock()
        .expect("global profiler mutex poisoned")
        .start()
}

/// 停止全局 CPU profiler。
pub fn StopCPUProfiler() {
    global_profiler()
        .lock()
        .expect("global profiler mutex poisoned")
        .stop();
}

/// 向全局 profiler 注册消费者（`None` 为 no-op，对齐 Go）。
pub fn Register(consumer: Option<ProfileConsumer>) {
    if let Some(consumer) = consumer {
        global_profiler()
            .lock()
            .expect("global profiler mutex poisoned")
            .register(consumer);
    }
}

/// 从全局 profiler 注销消费者（`None` 为 no-op）。
pub fn Unregister(consumer: Option<ProfileConsumer>) {
    if let Some(consumer) = consumer {
        global_profiler()
            .lock()
            .expect("global profiler mutex poisoned")
            .unregister(consumer);
    }
}

/// 全局已注册消费者数量。
pub fn global_consumers_count() -> usize {
    global_profiler()
        .lock()
        .expect("global profiler mutex poisoned")
        .consumersCount()
}

/// reset_global_profiler_for_test mirrors Go tests that replace `globalCPUProfiler`
/// with `newParallelCPUProfiler()` between cases so leftover consumers cannot leak.
/// 测试用：停止并清空全局 profiler 状态，避免用例间泄漏。
pub fn reset_global_profiler_for_test() {
    let mut profiler = global_profiler()
        .lock()
        .expect("global profiler mutex poisoned");
    profiler.stop();
    profiler
        .core
        .consumers
        .lock()
        .expect("profiler consumer mutex poisoned")
        .clear();
    profiler.profileData = None;
}
