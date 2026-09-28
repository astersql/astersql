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

// Windows 平台进程 CPU 占用百分比采样。
//
// 通过 `GetProcessTimes` 读取 Kernel/User 时间，与上次采样的墙钟时间差相除得到百分比。

#![allow(dead_code, non_snake_case)]

#[cfg(windows)]
use std::sync::atomic::{AtomicI64, Ordering};
#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

const WINDOWS_TO_UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

// 对应 Go 的 `//go:build windows` 构建约束。
/// 上次采样的墙钟时间（Unix 纳秒）。
#[cfg(windows)]
static LAST_INSPECT_UNIX_NANO: AtomicI64 = AtomicI64::new(0);
/// 上次采样的进程 CPU 累计时间（纳秒）。
#[cfg(windows)]
static LAST_CPU_USAGE_TIME: AtomicI64 = AtomicI64::new(0);

// GetCPUPercentage calculates CPU usage and returns percentage in float64(e.g. 2.5 means 2.5%).
// GetCPUPercentage 读取 Windows 进程 Kernel/User 时间，与上次采样差值相除得到百分比。
/// 返回自上次调用以来的进程 CPU 占用百分比；`GetProcessTimes` 失败时返回 0。
#[cfg(windows)]
pub fn GetCPUPercentage() -> f64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return 0.0;
    }
    // Go 的 Filetime.Nanoseconds 先换算到 Unix epoch，再以 int64 环绕算术乘 100。
    let filetime_nanos = |value: FILETIME| {
        let ticks = ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
        filetime_ticks_to_unix_nanos(ticks)
    };
    let usageTime = filetime_nanos(user).wrapping_add(filetime_nanos(kernel));
    let nowTime = now_unix_nano();
    let lastUsage = LAST_CPU_USAGE_TIME.load(Ordering::SeqCst);
    let lastInspect = LAST_INSPECT_UNIX_NANO.load(Ordering::SeqCst);
    let perc =
        usageTime.wrapping_sub(lastUsage) as f64 / nowTime.wrapping_sub(lastInspect) as f64 * 100.0;
    LAST_INSPECT_UNIX_NANO.store(nowTime, Ordering::SeqCst);
    LAST_CPU_USAGE_TIME.store(usageTime, Ordering::SeqCst);
    perc
}

/// 对齐 Go `syscall.Filetime.Nanoseconds` 的 epoch 换算和 `int64` 环绕语义。
pub(crate) fn filetime_ticks_to_unix_nanos(ticks: u64) -> i64 {
    (ticks as i64)
        .wrapping_sub(WINDOWS_TO_UNIX_EPOCH_TICKS as i64)
        .wrapping_mul(100)
}

/// 当前 Unix 纳秒时间戳。
#[cfg(windows)]
fn now_unix_nano() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}
