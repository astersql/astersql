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

// 进程 CPU 使用率观测器。
//
// 后台线程周期性读取本进程用户态/系统态 CPU 时间与 cgroup（控制组）配额，
// 用指数移动平均（EMA）平滑后写入全局原子变量与 Prometheus 指标。
// 内核过旧或 cgroup 不可用时标记 `unsupported`，调用方可据此降级。

#![allow(dead_code, non_snake_case)]

use crate::{cgroup, mathutil, metrics};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 最近一次 EMA 平滑后的 CPU 使用率（以 `f64` 位模式存入原子量）。
static CPU_USAGE: AtomicU64 = AtomicU64::new(0.0_f64.to_bits());

// If your kernel is lower than linux 4.7, you cannot get the cpu usage in the container.
// unsupported 标记当前内核/cgroup 环境是否无法提供容器 CPU 信息。
static UNSUPPORTED: AtomicBool = AtomicBool::new(false);

// GetCPUUsage returns the cpu usage of the current process.
// GetCPUUsage 返回最近一次观测到的 CPU 使用率，以及是否不支持容器内 CPU 观测。
/// 读取全局 CPU 使用率与 unsupported 标志。
pub fn GetCPUUsage() -> (f64, bool) {
    (
        f64::from_bits(CPU_USAGE.load(Ordering::SeqCst)),
        UNSUPPORTED.load(Ordering::SeqCst),
    )
}

/// 测试用：将全局使用率与 unsupported 标志复位。
#[cfg(test)]
pub(crate) fn reset_test_state() {
    CPU_USAGE.store(0.0_f64.to_bits(), Ordering::SeqCst);
    UNSUPPORTED.store(false, Ordering::SeqCst);
}

/// 观测器内部状态：上次采样的用户/系统时间、时间戳与 EMA。
struct ObserverState {
    utime: i64,
    stime: i64,
    now: i64,
    cpu: mathutil::ExponentialMovingAverage,
}

/// 周期性采样进程 CPU 并更新全局使用率的观测器。
pub struct Observer {
    state: Arc<Mutex<ObserverState>>,
    exit: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

// NewCPUObserver returns a cpu observer.
/// 构造尚未启动的 CPU 观测器（EMA 参数与 Go 版一致）。
pub fn NewCPUObserver() -> Observer {
    Observer {
        state: Arc::new(Mutex::new(ObserverState {
            utime: 0,
            stime: 0,
            now: now_unix_nano(),
            cpu: *mathutil::NewExponentialMovingAverage(0.95, 10),
        })),
        exit: None,
        worker: None,
    }
}

impl Observer {
    // Start starts the cpu observer.
    /// 探测 cgroup 后启动后台采样线程；失败则置 unsupported。
    pub fn Start(&mut self) {
        fail::fail_point!("GetCgroupCPUErr", |_| {
            UNSUPPORTED.store(true, Ordering::SeqCst);
            return;
        });
        if let Err(err) = cgroup::GetCgroupCPU() {
            UNSUPPORTED.store(true, Ordering::SeqCst);
            log::error!("GetCgroupCPU: {err}");
            return;
        }

        let (exit, receiver) = mpsc::channel();
        self.exit = Some(exit);
        let state = Arc::clone(&self.state);
        // 约每 100ms 采样一次；关闭 exit 通道后线程退出。
        self.worker = Some(std::thread::spawn(move || {
            while matches!(
                receiver.recv_timeout(Duration::from_millis(100)),
                Err(RecvTimeoutError::Timeout)
            ) {
                let mut state = state.lock().expect("cpu observer mutex poisoned");
                let current = observe(&mut state);
                state.cpu.Add(current);
                let usage = state.cpu.Get();
                CPU_USAGE.store(usage.to_bits(), Ordering::SeqCst);
                unsafe {
                    let gauge = std::ptr::addr_of!(metrics::EMACPUUsageGauge);
                    if let Some(gauge) = (&*gauge).as_ref() {
                        gauge.set(usage);
                    }
                }
            }
        }));
    }

    // Stop stops the cpu observer.
    /// 关闭退出通道并等待后台线程结束。
    pub fn Stop(&mut self) {
        self.exit.take();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

/// 根据两次采样的用户/系统 CPU 时间差与墙钟时间差，再除以 cgroup CPU share 得到瞬时使用率。
fn observe(state: &mut ObserverState) -> f64 {
    let (user, sys) = match getCPUTime() {
        Ok(times) => times,
        Err(err) => {
            log::error!("getCPUTime: {err}");
            (0, 0)
        }
    };
    let cpu_share = cpu_share_from_result(cgroup::GetCgroupCPU());
    let now = now_unix_nano();
    let duration = (now - state.now) as f64;
    // 将毫秒累计时间放大到纳秒量级，与墙钟纳秒对齐后再求速率。
    let utime = user * 1_000_000;
    let stime = sys * 1_000_000;
    let user_rate = (utime - state.utime) as f64 / duration;
    let system_rate = (stime - state.stime) as f64 / duration;
    state.now = now;
    state.utime = utime;
    state.stime = stime;
    (system_rate + user_rate) / cpu_share
}

/// Go 忽略采样阶段的 cgroup 错误，并继续使用 `CPUUsage{}` 的零值 share。
pub(crate) fn cpu_share_from_result(result: anyhow::Result<cgroup::CPUUsage>) -> f64 {
    result.unwrap_or_default().CPUShares()
}

/// Enables the Go-compatible cgroup probe failure for cross-crate integration tests.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn setup_cgroup_cpu_error_failpoint_for_test() -> impl Drop {
    let scenario = fail::FailScenario::setup();
    fail::cfg("GetCgroupCPUErr", "return").expect("enable Go cgroup failpoint");
    scenario
}

// getCPUTime returns the cumulative user/system time (in ms) since the process start.
/// Unix：通过 `getrusage(RUSAGE_SELF)` 读取本进程累计用户/系统 CPU 时间（毫秒）。
#[cfg(unix)]
pub fn getCPUTime() -> io::Result<(i64, i64)> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let usage = unsafe { usage.assume_init() };
    let millis = |time: libc::timeval| time.tv_sec as i64 * 1_000 + time.tv_usec as i64 / 1_000;
    Ok((millis(usage.ru_utime) as i64, millis(usage.ru_stime) as i64))
}

/// 非 Unix 平台桩：返回 Unsupported。
#[cfg(not(unix))]
pub fn getCPUTime() -> io::Result<(i64, i64)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process user/system CPU time is unsupported on this platform",
    ))
}

// GetCPUCount returns the number of logical CPUs usable by the current process.
/// 返回当前进程可用的逻辑 CPU 数；可通过 failpoint `mockNumCpu` 注入。
pub fn GetCPUCount() -> i32 {
    fail::fail_point!("mockNumCpu", |value| {
        value
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(1)
    });
    std::thread::available_parallelism()
        .map(|count| count.get() as i32)
        .unwrap_or(1)
}

// now_unix_nano 对应 time.Now().UnixNano()，供采样时间计算使用。
fn now_unix_nano() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}
