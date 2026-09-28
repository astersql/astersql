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

// cgroup 资源监控（cgmon）：周期性探测 CPU 配额与内存上限，并更新 Prometheus 指标。
//
// 对应 Go `util/cgmon`。cgroup（control group）是 Linux 容器资源限制机制；
// 本模块在探测失败时仍可用主机默认值，并通过可注入 probe 保持与 Go 相同的测试缝。

#![allow(non_snake_case)]

use crate::util::cgroup;
use anyhow::{Result, anyhow};
use prometheus::{Gauge, Opts};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use sysinfo::System;

/// 后台刷新周期（默认 10 秒）。
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// 主机逻辑 CPU 数探测函数类型。
type CPUCountProbe = dyn Fn() -> Result<usize> + Send + Sync;
/// cgroup CPU period/quota 探测函数类型。
type CPUQuotaProbe = dyn Fn() -> Result<(i64, i64)> + Send + Sync;
/// 内存字节数探测函数类型。
type MemoryProbe = dyn Fn() -> Result<u64> + Send + Sync;

/// 后台 worker 的取消通道与线程句柄。
struct Worker {
    cancel: Sender<()>,
    handle: JoinHandle<()>,
}

/// 监控器运行态：是否已有 worker。
#[derive(Default)]
struct MonitorState {
    worker: Option<Worker>,
}

/// 监控器内部共享状态（探针、指标、上次值与 worker）。
struct MonitorInner {
    refresh_interval: Duration,
    cpu_count: Arc<CPUCountProbe>,
    cpu_period_and_quota: Arc<CPUQuotaProbe>,
    total_memory: Arc<MemoryProbe>,
    cgroup_memory_limit: Arc<MemoryProbe>,
    max_procs: Gauge,
    memory_limit: Gauge,
    last_cpu: Mutex<i32>,
    last_memory_limit: Mutex<u64>,
    state: Mutex<MonitorState>,
}

/// CgroupMonitor owns the probes, metrics and worker lifecycle used by cgmon.
/// Probe injection keeps the same test seam as the package variables in Go.
/// 持有探针、指标与 worker 生命周期；注入探针以对齐 Go 包级变量测试缝。
#[derive(Clone)]
pub struct CgroupMonitor {
    inner: Arc<MonitorInner>,
}

impl CgroupMonitor {
    /// 以自定义刷新间隔与四类探针构造监控器，并注册 Prometheus Gauge。
    pub fn with_probes<CPUCount, CPUQuota, TotalMemory, MemoryLimit>(
        refresh_interval: Duration,
        cpu_count: CPUCount,
        cpu_period_and_quota: CPUQuota,
        total_memory: TotalMemory,
        cgroup_memory_limit: MemoryLimit,
    ) -> Self
    where
        CPUCount: Fn() -> Result<usize> + Send + Sync + 'static,
        CPUQuota: Fn() -> Result<(i64, i64)> + Send + Sync + 'static,
        TotalMemory: Fn() -> Result<u64> + Send + Sync + 'static,
        MemoryLimit: Fn() -> Result<u64> + Send + Sync + 'static,
    {
        let max_procs = Gauge::with_opts(
            Opts::new("maxprocs", "The value of GOMAXPROCS.")
                .namespace("tidb")
                .subsystem("server"),
        )
        .expect("cgmon maxprocs metric descriptor must be valid");
        let memory_limit = Gauge::with_opts(
            Opts::new("memory_quota_bytes", "The value of memory quota bytes.")
                .namespace("tidb")
                .subsystem("server"),
        )
        .expect("cgmon memory limit metric descriptor must be valid");
        Self {
            inner: Arc::new(MonitorInner {
                refresh_interval,
                cpu_count: Arc::new(cpu_count),
                cpu_period_and_quota: Arc::new(cpu_period_and_quota),
                total_memory: Arc::new(total_memory),
                cgroup_memory_limit: Arc::new(cgroup_memory_limit),
                max_procs,
                memory_limit,
                last_cpu: Mutex::new(0),
                last_memory_limit: Mutex::new(0),
                state: Mutex::new(MonitorState::default()),
            }),
        }
    }

    /// Starts one background worker and performs the first refresh immediately.
    /// Returns false when this monitor is already running.
    /// 启动一个后台 worker 并立即做首次刷新；已在运行则返回 false。
    pub fn start(&self) -> bool {
        let mut state = lock(&self.inner.state);
        if state.worker.is_some() {
            return false;
        }

        let (cancel, cancelled) = mpsc::channel();
        let inner = Arc::downgrade(&self.inner);
        let refresh_interval = self.inner.refresh_interval;
        // 捕获 panic，避免监控线程拖垮进程。
        let handle = thread::Builder::new()
            .name("cgroup-monitor".to_owned())
            .spawn(move || {
                if catch_unwind(AssertUnwindSafe(|| {
                    refresh_cgroup_loop(inner, cancelled, refresh_interval);
                }))
                .is_err()
                {
                    log::error!("panic recovered in cgroup monitor worker");
                }
            })
            .expect("failed to spawn cgroup monitor worker");
        state.worker = Some(Worker { cancel, handle });
        log::info!("cgroup monitor started");
        true
    }

    /// Cancels and joins the worker. Returns false when it was not running.
    /// 取消并 join worker；未运行则返回 false。
    pub fn stop(&self) -> bool {
        let worker = lock(&self.inner.state).worker.take();
        let Some(worker) = worker else {
            return false;
        };
        let _ = worker.cancel.send(());
        let _ = worker.handle.join();
        log::info!("cgroup monitor stopped");
        true
    }

    /// 刷新 CPU 配额：取主机核数与 cgroup quota 的较小者，更新 maxprocs 指标。
    pub fn refresh_cgroup_cpu(&self) -> Result<()> {
        let host_cpu = (self.inner.cpu_count)()?.max(1);
        let mut quota = i32::try_from(host_cpu)
            .map_err(|_| anyhow!("host logical CPU count {host_cpu} exceeds i32"))?;
        let cgroup_result = (self.inner.cpu_period_and_quota)();

        // cgroup 有效时用 quota/period 比率向上取整，且不超过主机核数。
        if let Ok((cpu_period, cpu_quota)) = &cgroup_result
            && *cpu_period > 0
            && *cpu_quota > 0
        {
            let ratio = *cpu_quota as f64 / *cpu_period as f64;
            if ratio < f64::from(quota) {
                quota = ratio.ceil() as i32;
            }
        }

        let mut last_cpu = lock(&self.inner.last_cpu);
        if quota != *last_cpu {
            log::info!("set the maxprocs: quota={quota}");
            self.inner.max_procs.set(f64::from(quota));
            *last_cpu = quota;
        }
        cgroup_result.map(|_| ())
    }

    /// 刷新内存上限：取主机总内存与 cgroup limit 的较小者，更新指标。
    pub fn refresh_cgroup_memory(&self) -> Result<()> {
        let mut memory_limit = (self.inner.total_memory)()?;
        let cgroup_result = (self.inner.cgroup_memory_limit)();
        if let Ok(cgroup_limit) = &cgroup_result
            && *cgroup_limit < memory_limit
        {
            memory_limit = *cgroup_limit;
        }

        let mut last_memory_limit = lock(&self.inner.last_memory_limit);
        if memory_limit != *last_memory_limit {
            log::info!("set the memory limit: memLimit={memory_limit}");
            self.inner.memory_limit.set(memory_limit as f64);
            *last_memory_limit = memory_limit;
        }
        cgroup_result.map(|_| ())
    }

    /// 最近一次生效的 CPU 配额（逻辑核数）。
    pub fn last_cpu(&self) -> i32 {
        *lock(&self.inner.last_cpu)
    }

    /// 最近一次生效的内存上限（字节）。
    pub fn last_memory_limit(&self) -> u64 {
        *lock(&self.inner.last_memory_limit)
    }

    /// 当前 maxprocs Gauge 读数。
    pub fn max_procs_metric(&self) -> f64 {
        self.inner.max_procs.get()
    }

    /// 当前 memory_quota_bytes Gauge 读数。
    pub fn memory_limit_metric(&self) -> f64 {
        self.inner.memory_limit.get()
    }
}

/// 获取互斥锁；遇 poison 则恢复以继续监控。
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 后台循环：首次刷新后按间隔等待，收到取消或升级失败则退出。
fn refresh_cgroup_loop(
    inner: Weak<MonitorInner>,
    cancelled: mpsc::Receiver<()>,
    refresh_interval: Duration,
) {
    if !refresh_once(&inner, true) {
        return;
    }
    loop {
        match cancelled.recv_timeout(refresh_interval) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) if !refresh_once(&inner, false) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// 执行一次 CPU/内存刷新；`inner` 已释放则返回 false。首次失败用 warn，之后用 debug。
fn refresh_once(inner: &Weak<MonitorInner>, initial: bool) -> bool {
    let Some(inner) = inner.upgrade() else {
        return false;
    };
    let monitor = CgroupMonitor { inner };
    match monitor.refresh_cgroup_cpu() {
        Err(err) if initial => log::warn!("failed to get cgroup cpu quota: {err:#}"),
        Err(err) => log::debug!("failed to get cgroup cpu quota: {err:#}"),
        Ok(()) => {}
    }
    match monitor.refresh_cgroup_memory() {
        Err(err) if initial => log::warn!("failed to get cgroup memory limit: {err:#}"),
        Err(err) => log::debug!("failed to get cgroup memory limit: {err:#}"),
        Ok(()) => {}
    }
    true
}

/// 通过 sysinfo 读取主机总内存。
fn system_memory_total() -> Result<u64> {
    let mut system = System::new();
    system.refresh_memory();
    Ok(system.total_memory())
}

/// 进程级默认监控器：真实 cgroup/主机探针，10s 刷新。
static GLOBAL_MONITOR: LazyLock<CgroupMonitor> = LazyLock::new(|| {
    CgroupMonitor::with_probes(
        REFRESH_INTERVAL,
        || {
            Ok(thread::available_parallelism()
                .map_err(anyhow::Error::from)?
                .get())
        },
        cgroup::GetCPUPeriodAndQuota,
        system_memory_total,
        cgroup::GetMemoryLimit,
    )
});

/// StartCgroupMonitor uses to start the cgroup monitoring.
/// WARN: as in Go, callers should serialize start and stop operations.
/// 启动全局 cgroup 监控（仅 Linux）；调用方须串行化 start/stop。
pub fn StartCgroupMonitor() {
    if cfg!(target_os = "linux") {
        GLOBAL_MONITOR.start();
    }
}

/// StopCgroupMonitor uses to stop the cgroup monitoring.
/// WARN: as in Go, callers should serialize start and stop operations.
/// 停止全局 cgroup 监控（仅 Linux）；调用方须串行化 start/stop。
pub fn StopCgroupMonitor() {
    if cfg!(target_os = "linux") {
        GLOBAL_MONITOR.stop();
    }
}
