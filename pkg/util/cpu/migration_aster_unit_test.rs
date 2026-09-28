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

// `cpu` 迁移期补充单测：CPU 计数、进程时间单调性、观测器启停与 cgroup failpoint。

use crate::{
    GetCPUCount, GetCPUUsage, NewCPUObserver, TEST_LOCK, cpu_share_from_result, getCPUTime,
    reset_test_state,
};
use std::hint::black_box;
use std::time::{Duration, Instant};

/// `GetCPUCount` 应等于 `available_parallelism`（失败时为 1）。
#[test]
fn cpu_count_matches_the_process_parallelism() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_test_state();
    let expected = std::thread::available_parallelism()
        .map(|count| count.get() as i32)
        .unwrap_or(1);
    assert_eq!(GetCPUCount(), expected);
}

/// 制造短暂 CPU 负载后断言 `getCPUTime` 单调递增，且观测器可干净 Stop。
#[test]
fn process_cpu_time_is_monotonic_and_observer_stops_cleanly() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_test_state();
    let (before_user, before_system) = getCPUTime().expect("read initial process CPU time");
    let deadline = Instant::now() + Duration::from_millis(30);
    let mut value = 1_u64;
    while Instant::now() < deadline {
        value = black_box(value.wrapping_mul(6364136223846793005).wrapping_add(1));
    }
    black_box(value);
    let (after_user, after_system) = getCPUTime().expect("read final process CPU time");
    assert!(after_user >= before_user);
    assert!(after_system >= before_system);
    assert!(after_user + after_system > before_user + before_system);

    let mut observer = NewCPUObserver();
    observer.Start();
    std::thread::sleep(Duration::from_millis(140));
    observer.Stop();
    let (usage, _) = GetCPUUsage();
    assert!(usage.is_finite());
    assert!(usage >= 0.0);
}

/// cgroup 探测 failpoint 触发时标记 unsupported，且不改写既有全局使用率。
#[test]
fn cgroup_probe_failure_matches_go_unsupported_behavior() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_test_state();
    let (usage_before, _) = GetCPUUsage();
    let _scenario = fail::FailScenario::setup();
    fail::cfg("GetCgroupCPUErr", "return").expect("enable Go cgroup failpoint");

    let mut observer = NewCPUObserver();
    observer.Start();

    let (usage, unsupported) = GetCPUUsage();
    assert!(unsupported);
    // Go keeps the process-wide cpuUsage unchanged when the cgroup probe fails.
    assert_eq!(usage, usage_before);
    observer.Stop();
}

/// Go 的 observe 忽略后续 cgroup 读取错误，并用 CPUUsage 零值继续计算。
#[test]
fn sampling_cgroup_error_uses_go_zero_value_cpu_share() {
    let share = cpu_share_from_result(Err(anyhow::anyhow!("transient cgroup error")));
    assert_eq!(share, 0.0);
}
