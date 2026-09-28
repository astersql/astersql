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

// Finalizer 行为单测。
//
// 验证多次 `run` 计数、`stop` 后不再回调，以及 stop 标志与回调内断言的配合。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::finalizer::newFinalizer;

/// 先连续 run 到上限，再 stop，确认之后 run 返回 false 且计数不变。
#[test]
fn test_finalizer() {
    let max_count = 8;
    let count = Arc::new(AtomicI32::new(0));
    let stopped = Arc::new(AtomicBool::new(false));
    let callback_count = Arc::clone(&count);
    let callback_stopped = Arc::clone(&stopped);
    let finalizer = newFinalizer(Box::new(move || {
        let n = callback_count.fetch_add(1, Ordering::SeqCst) + 1;
        // 若已 stop 仍进入回调则失败（对齐 Go：停止后不应再触发）。
        assert!(
            n <= max_count || !callback_stopped.load(Ordering::SeqCst),
            "cannot execute finalizer callback after finalizer has been stopped"
        );
    }));

    // Rust exposes the runtime collection boundary explicitly through `run`.
    // 显式在“回收边界”调用 run，模拟 Go finalizer 的多次触发。
    for expected in 1..=max_count {
        assert!(finalizer.run());
        assert_eq!(expected, count.load(Ordering::SeqCst));
    }

    finalizer.stop();
    stopped.store(true, Ordering::SeqCst);
    assert_eq!(max_count, count.load(Ordering::SeqCst));
    // stop 后连续 run 应均失败，且计数冻结。
    assert!(!finalizer.run());
    assert!(!finalizer.run());
    assert_eq!(max_count, count.load(Ordering::SeqCst));
}

/// Go 的 finalizer 由运行时 GC 自动驱动；Rust 适配也必须在生产环境自动回调，
/// 不能只依赖测试显式调用 `run`。
#[test]
fn finalizer_runs_without_manual_notification() {
    let calls = Arc::new(AtomicI32::new(0));
    let callback_calls = Arc::clone(&calls);
    let finalizer = newFinalizer(Box::new(move || {
        callback_calls.fetch_add(1, Ordering::SeqCst);
    }));

    let deadline = Instant::now() + Duration::from_secs(2);
    while calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }

    finalizer.stop();
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "runtime finalizer driver never invoked the callback"
    );
}
