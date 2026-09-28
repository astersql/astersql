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

// Timer metrics 与 Go 指标约定对齐的单元测试。
//
// 验证 Counter 名称、help、label（scope/type）以及 Hook worker
// 使用的 `hook.<class>` scope 格式与 Go 侧一致。

use crate::{InitTimerMetrics, TimerEventCounter, TimerHookWorkerCounter, TimerScopeCounter};
use prometheus::core::Collector;
use std::sync::Mutex;

/// 串行化指标初始化测试，避免并发改写全局 OnceLock 状态。
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 校验初始化后的指标描述符、标签与计数值与 Go 一致。
#[test]
fn init_matches_go_metric_descriptor_and_labels() {
    let _guard = TEST_LOCK.lock().unwrap();
    InitTimerMetrics();

    let counter = TimerScopeCounter("runtime.group-1", "full_refresh_timers");
    counter.inc_by(2.0);

    // 收集 CounterVec 的 metric family，核对名称、help、标签与取值。
    let family = TimerEventCounter
        .get()
        .expect("timer metric storage should be initialized")
        .read()
        .unwrap()
        .as_ref()
        .expect("timer counter vector should be initialized")
        .collect();
    assert_eq!(family.len(), 1);
    assert_eq!(family[0].name(), "tidb_server_timer_event_count");
    assert_eq!(family[0].help(), "Counter of timer event.");
    let metric = &family[0].get_metric()[0];
    let labels: Vec<_> = metric
        .get_label()
        .iter()
        .map(|pair| (pair.name(), pair.value()))
        .collect();
    assert_eq!(
        labels,
        vec![
            ("scope", "runtime.group-1"),
            ("type", "full_refresh_timers")
        ]
    );
    assert_eq!(metric.get_counter().value(), 2.0);
}

/// 校验 Hook worker 计数器使用 `hook.<class>` scope 且共享同一 CounterVec。
#[test]
fn hook_worker_counter_uses_go_scope_format_and_shares_the_vector() {
    let _guard = TEST_LOCK.lock().unwrap();
    InitTimerMetrics();

    TimerHookWorkerCounter("class-a", "trigger").inc();
    assert_eq!(
        TimerScopeCounter("hook.class-a", "trigger").get(),
        1.0,
        "hook counters must use the hook.<class> scope"
    );
}
