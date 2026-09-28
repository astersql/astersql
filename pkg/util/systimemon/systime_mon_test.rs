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

// `StartMonitor` 行为单测：注入回退时钟，确认错误回调在时限内触发。
//
// 对应 Go `TestSystimeMonitor`。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use super::StartMonitor;

// TestSystimeMonitor 对应 Go 的同名测试：第一次 now() 返回当前时间，
// 第二次返回倒退 2 秒的时间，从而触发 StartMonitor 的错误回调。
/// 模拟 2 秒时钟回退，断言 1 秒内 `systimeErrHandler` 被调用。
#[test]
fn test_systime_monitor() {
    let err_triggered = Arc::new(AtomicBool::new(false));
    let now_triggered = Arc::new(AtomicBool::new(false));
    let base = SystemTime::now();

    let thread_err_triggered = Arc::clone(&err_triggered);
    let thread_now_triggered = Arc::clone(&now_triggered);
    // 监控为无限循环，必须在后台线程启动。
    std::thread::spawn(move || {
        StartMonitor(
            move || {
                // 首次返回基准时间，之后固定回退 2 秒。
                if !thread_now_triggered.swap(true, Ordering::SeqCst) {
                    return base;
                }

                base - Duration::from_secs(2)
            },
            move || {
                thread_err_triggered.store(true, Ordering::SeqCst);
            },
        );
    });

    // 等待回调或超时；监控采样间隔为 100ms。
    let deadline = Instant::now() + Duration::from_secs(1);
    while !err_triggered.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        err_triggered.load(Ordering::SeqCst),
        "the monitor did not report the two-second backward jump within one second"
    );
}

/// Go 的 `time.Ticker` 在 `now()` 阻塞期间仍继续计时；已经到达的 tick 应可立即消费。
#[test]
fn slow_sample_does_not_restart_the_tick_interval() {
    let (elapsed_tx, elapsed_rx) = mpsc::channel();
    let started = Instant::now();
    let base = SystemTime::now();

    std::thread::spawn(move || {
        let mut first = true;
        StartMonitor(
            move || {
                if first {
                    first = false;
                    std::thread::sleep(Duration::from_millis(250));
                    base
                } else {
                    base - Duration::from_secs(2)
                }
            },
            move || {
                let _ = elapsed_tx.send(started.elapsed());
            },
        );
    });

    let elapsed = elapsed_rx
        .recv_timeout(Duration::from_millis(325))
        .expect("an already elapsed ticker event should be consumed without another 100ms delay");
    assert!(elapsed < Duration::from_millis(325), "elapsed: {elapsed:?}");
}
