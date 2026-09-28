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

// Timer 相关 Prometheus 指标定义。
//
// 提供全局 `TimerEventCounter` 及按 scope/type 取值的包装函数，
// 对应 Go 侧 `tidb_server_timer_event_count` 计数器，用于观测定时器运行时事件。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{OnceLock, RwLock};

// TimerEventCounter is the counter for timer events
/// 全局定时器事件计数器存储；OnceLock 保证只初始化一次，RwLock 保护内部 Option。
pub static TimerEventCounter: OnceLock<RwLock<Option<prometheus::CounterVec>>> = OnceLock::new();

// InitTimerMetrics 对应 Go 初始化函数：创建带 scope/type label 的 timer event counter。
/// 初始化定时器事件 CounterVec，命名空间为 tidb/server，标签为 scope 与 type。
pub fn InitTimerMetrics() {
    let counter = metricscommon::NewCounterVec(
        prometheus::Opts::new("timer_event_count", "Counter of timer event.")
            .namespace("tidb")
            .subsystem("server"),
        &["scope".to_string(), "type".to_string()],
    );

    // 写入全局 OnceLock；若锁被毒化则恢复内层值继续使用。
    *TimerEventCounter
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(counter);
}

// TimerHookWorkerCounter creates a counter for a hook's event
/// 为指定 Hook 类创建事件计数器；scope 格式为 `hook.<hookClass>`。
pub fn TimerHookWorkerCounter(hookClass: &str, event: &str) -> prometheus::Counter {
    TimerScopeCounter(&format!("hook.{}", hookClass), event)
}

// TimerScopeCounter 对应 Go 的 WithLabelValues 包装：按 scope 和 event 选择 counter。
/// 按 scope 与 event（对应 type 标签）从全局 CounterVec 取出具体 Counter。
pub fn TimerScopeCounter(scope: &str, event: &str) -> prometheus::Counter {
    TimerEventCounter
        .get()
        .expect("TimerEventCounter should be initialized before use")
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .expect("TimerEventCounter should be initialized before use")
        .with_label_values(&[scope, event])
}
