// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// GOGC 目标值缓存：对齐 Go `runtime/debug` 的 GC 百分比（GOGC）读写接口。
//
// 由环境变量 `GOGC` 初始化（默认 100）；`SetGOGC`/`GetGOGC` 用原子变量保存当前值。
// 迁移阶段不调用真实 Go runtime，仅保留配置与观测侧所需的数值语义。

use std::env;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicI32, Ordering};

/// 当前 GOGC 缓存；首次访问时从环境变量解析，缺省或非法则为 100。
// gogcValue 对应 Go 包级 int64 变量；使用 AtomicI64 保留 sync/atomic 读写语义。
static gogcValue: LazyLock<AtomicI32> = LazyLock::new(|| {
    // 解析失败或未设置时沿用 Go 默认 100。
    let value = env::var("GOGC")
        .ok()
        .and_then(|raw| raw.parse::<i32>().ok())
        .unwrap_or(100);
    AtomicI32::new(value)
});

/// 包初始化入口：触发 LazyLock 读入环境变量（对齐 Go `init`）。
// init 对应 Go init：读取 GOGC 环境变量并初始化指标。
// Rust 没有同名包初始化钩子，这里保留成普通函数，供后续接线时决定调用时机。
pub fn init() {
    let _ = gogcValue.load(Ordering::SeqCst);
}

/// 设置 GOGC；`val <= 0` 时归一为 100，返回替换前的旧值。
// SetGOGC update GOGC and related metrics.
// SetGOGC 保留 Go 语义：非正值归一化为 100，调用 runtime/debug.SetGCPercent 并更新指标和缓存值。
pub fn SetGOGC(mut val: i32) -> i32 {
    if val <= 0 {
        val = 100;
    }
    gogcValue.swap(val, Ordering::SeqCst)
}

/// 原子读取当前 GOGC 值。
// GetGOGC returns the current value of GOGC.
// GetGOGC 使用原子读取返回最近一次设置的 GOGC 值。
pub fn GetGOGC() -> i32 {
    gogcValue.load(Ordering::SeqCst)
}
