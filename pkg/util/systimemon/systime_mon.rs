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

// 系统时间回退监控：周期性采样，发现时钟倒退时调用错误处理回调。
//
// TiDB/AsterSQL 依赖单调前进的墙钟时间做超时、TTL 等判断；NTP 校时或手动改时可能导致
// 时间回退，本模块用于检测该异常。对应 Go `pkg/util/systimemon`。

use std::thread;
use std::time::{Duration, Instant, SystemTime};

// StartMonitor calls systimeErrHandler if system time jump backward.
/// 每 100ms 采样一次 `now()`；若新时间早于上次采样，则调用 `systimeErrHandler`。
///
/// `now` 可注入假时钟便于测试；本函数永不返回（无限循环）。
#[allow(non_snake_case)]
pub fn StartMonitor<Now, SystimeErrHandler>(mut now: Now, mut systimeErrHandler: SystimeErrHandler)
where
    Now: FnMut() -> SystemTime,
    SystimeErrHandler: FnMut(),
{
    log::info!("start system time monitor");

    let interval = Duration::from_millis(100);
    let mut next_tick = Instant::now() + interval;
    loop {
        let last = now();
        // Go 的 Ticker 使用固定节拍；采样变慢时消费已到达的 tick，而不是重启间隔。
        thread::sleep(next_tick.saturating_duration_since(Instant::now()));
        let tick_observed_at = Instant::now();
        while next_tick <= tick_observed_at {
            next_tick += interval;
        }
        // 墙钟回退：新采样严格早于上次记录的时间点。
        if now() < last {
            log::error!("system time jump backward; last={last:?}");
            systimeErrHandler();
        }
    }
}
