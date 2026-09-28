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

// `cpu` 模块单元测试：验证观测器在容器内能采到正使用率，以及 failpoint 下的 unsupported 行为。

#![allow(dead_code, non_snake_case)]

use crate::{GetCPUUsage, NewCPUObserver, TEST_LOCK, cgroup, reset_test_state};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

// TestCPUValue 对应 Go 的容器内 CPU 采样测试；非容器环境下直接跳过。
/// 容器内启动观测器并制造 CPU 负载，断言使用率落在 (0, 1)。
#[test]
fn TestCPUValue() {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_test_state();
    // If it's not in the container,
    // it will have less files and the test case will fail forever.
    if !cgroup::InContainer() {
        // Go 使用 t.Skip；用早返回表达“非容器环境不验证 CPU 文件数量”的语义。
        return;
    }
    let mut observer = NewCPUObserver();
    let exit = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();

    for _ in 0..10 {
        let exit = exit.clone();
        handles.push(thread::spawn(move || {
            // Go goroutine 在 default 分支里 runtime.Gosched，制造持续 CPU 活动但可响应退出信号。
            while !exit.load(Ordering::SeqCst) {
                thread::yield_now();
            }
        }));
    }

    observer.Start();
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(200));
        let (value, unsupported) = GetCPUUsage();
        assert!(!unsupported);
        assert!(value > 0.0);
        assert!(value < 1.0);
    }
    observer.Stop();

    // 对应 close(exit) 和 wg.Wait，确保所有忙等线程结束，避免泄露到后续测试。
    exit.store(true, Ordering::SeqCst);
    for handle in handles {
        handle.join().expect("Go wg.Wait equivalent");
    }
}

// TestFailpointCPUValue 对应 Go 的 failpoint 分支：cgroup 失败时 observer 标记 unsupported。
