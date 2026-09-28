// Copyright 2017 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Aster 迁移补充单测：注入假 `now`，验证系统时间回退能触发错误回调。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use astersql_util_systimemon::StartMonitor;

/// 第一次返回基准时间，后续返回倒退 2 秒；期望 1 秒内触发 `systimeErrHandler`。
#[test]
fn detects_a_backward_system_time_jump() {
    let err_triggered = Arc::new(AtomicBool::new(false));
    let now_calls = Arc::new(AtomicUsize::new(0));
    let base = SystemTime::now();

    let thread_err_triggered = Arc::clone(&err_triggered);
    let thread_now_calls = Arc::clone(&now_calls);
    // 监控循环会阻塞，放在独立线程中运行。
    std::thread::spawn(move || {
        StartMonitor(
            move || {
                // 第 0 次采样用基准时间，之后人为回退 2 秒以模拟时钟跳变。
                if thread_now_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    base
                } else {
                    base - Duration::from_secs(2)
                }
            },
            move || thread_err_triggered.store(true, Ordering::SeqCst),
        );
    });

    // 轮询等待回调，超时则失败（监控周期约 100ms）。
    let deadline = Instant::now() + Duration::from_secs(1);
    while !err_triggered.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        err_triggered.load(Ordering::SeqCst),
        "the monitor did not report the two-second backward jump within one second"
    );
}
