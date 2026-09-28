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

// POSIX（Linux/macOS/FreeBSD 等）平台进程 CPU 占用百分比采样。
//
// 通过 `getrusage(RUSAGE_SELF)` 读取用户态+系统态累计时间，与上次采样的墙钟时间差相除得到百分比。

#![allow(dead_code, non_snake_case)]

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// 对应 Go 的 `//go:build linux || darwin || freebsd || unix` 构建约束。
/// 上次采样的墙钟时间（Unix 纳秒）。
#[cfg(any(unix, target_os = "linux", target_os = "macos", target_os = "freebsd"))]
static LAST_INSPECT_UNIX_NANO: AtomicI64 = AtomicI64::new(0);
/// 上次采样的进程 CPU 累计时间（纳秒）。
#[cfg(any(unix, target_os = "linux", target_os = "macos", target_os = "freebsd"))]
static LAST_CPU_USAGE_TIME: AtomicI64 = AtomicI64::new(0);

// GetCPUPercentage calculates CPU usage and returns percentage in float64(e.g. 2.5 means 2.5%).
// http://man7.org/linux/man-pages/man2/getrusage.2.html
// GetCPUPercentage 读取 RUSAGE_SELF 的用户态和系统态耗时，与上次采样差值相除得到百分比。
/// 返回自上次调用以来的进程 CPU 占用百分比。
#[cfg(any(unix, target_os = "linux", target_os = "macos", target_os = "freebsd"))]
pub fn GetCPUPercentage() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // Go 忽略 syscall.Getrusage 的错误；也不改变这个容错策略。
    let _ = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let usageTime = timeval_nano(ru.ru_utime) + timeval_nano(ru.ru_stime);
    let nowTime = now_unix_nano();
    let lastUsage = LAST_CPU_USAGE_TIME.load(Ordering::SeqCst);
    let lastInspect = LAST_INSPECT_UNIX_NANO.load(Ordering::SeqCst);
    let perc = cpu_percentage_from_samples(lastInspect, lastUsage, nowTime, usageTime);
    LAST_INSPECT_UNIX_NANO.store(nowTime, Ordering::SeqCst);
    LAST_CPU_USAGE_TIME.store(usageTime, Ordering::SeqCst);
    perc
}

/// 由两次采样的墙钟与 CPU 时间计算百分比：Δcpu / Δwall * 100。
pub(crate) fn cpu_percentage_from_samples(
    last_inspect_nanos: i64,
    last_usage_nanos: i64,
    now_nanos: i64,
    usage_nanos: i64,
) -> f64 {
    (usage_nanos - last_usage_nanos) as f64 / (now_nanos - last_inspect_nanos) as f64 * 100.0
}

/// 将 `timeval` 转为纳秒。
#[allow(clippy::unnecessary_cast)]
fn timeval_nano(tv: libc::timeval) -> i64 {
    // libc uses c_long here; explicit widening is required on 32-bit Unix.
    tv.tv_sec as i64 * 1_000_000_000 + tv.tv_usec as i64 * 1_000
}

/// 当前 Unix 纳秒时间戳。
fn now_unix_nano() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}
