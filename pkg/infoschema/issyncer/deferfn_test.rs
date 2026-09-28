// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `deferFn` 延迟回调的单元测试。
//
// 对应 Go `deferfn_test.go`：真实 sleep 300ms 后调用 `check`，用 `Arc<AtomicBool>`
// 标记回调是否执行（Rust 闭包需 `'static`，不能像 Go 那样按引用捕获栈上 bool）。

// Ported from pkg/infoschema/issyncer/deferfn_test.go. Unlike the earlier
// mechanical draft, this really sleeps 300ms (like Go's `time.Sleep`) and
// really checks the deferred callbacks, using `Arc<AtomicBool>` flags since
// Rust closures captured by `add` must be `'static` (unlike Go, which can
// close over stack-local `bool`s by reference).

use crate::DeferFn;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 验证短延迟回调在 sleep 后触发，长延迟（10 分钟）仍保留。
#[test]
fn test_defer_fn() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<DeferFn>();

    let mut df = DeferFn::default();
    let a = Arc::new(AtomicBool::new(false));
    let b = Arc::new(AtomicBool::new(false));
    let c = Arc::new(AtomicBool::new(false));
    let d = Arc::new(AtomicBool::new(false));

    let (a1, b1, c1, d1) = (a.clone(), b.clone(), c.clone(), d.clone());
    // a/b/d：50/100/150ms 后应触发；c：10 分钟后，300ms sleep 后仍不应触发。
    df.add(
        move || a1.store(true, Ordering::SeqCst),
        Instant::now() + Duration::from_millis(50),
    );
    df.add(
        move || b1.store(true, Ordering::SeqCst),
        Instant::now() + Duration::from_millis(100),
    );
    df.add(
        move || c1.store(true, Ordering::SeqCst),
        Instant::now() + Duration::from_secs(10 * 60),
    );
    df.add(
        move || d1.store(true, Ordering::SeqCst),
        Instant::now() + Duration::from_millis(150),
    );

    std::thread::sleep(Duration::from_millis(300));
    df.check();

    assert!(a.load(Ordering::SeqCst));
    assert!(b.load(Ordering::SeqCst));
    assert!(!c.load(Ordering::SeqCst));
    assert!(d.load(Ordering::SeqCst));
    // 仅长延迟 c 仍挂起。
    assert_eq!(df.len(), 1);

    // Go's deferFn protects add/check with a mutex; concurrent registration
    // must not lose callbacks or race with the queue.
    let concurrent = Arc::new(DeferFn::default());
    let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let concurrent = concurrent.clone();
        let fired = fired.clone();
        workers.push(std::thread::spawn(move || {
            concurrent.add(
                move || {
                    fired.fetch_add(1, Ordering::SeqCst);
                },
                Instant::now(),
            );
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(concurrent.len(), 8);
    std::thread::sleep(Duration::from_millis(1));
    concurrent.check();
    assert_eq!(fired.load(Ordering::SeqCst), 8);
}

/// Go's `check` keeps the deferFn mutex held while callbacks run, so a
/// concurrent `add` cannot mutate the queue until the callback returns.
#[test]
fn check_holds_the_lock_while_running_callbacks() {
    let df = Arc::new(DeferFn::default());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    df.add(
        move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        },
        Instant::now() - Duration::from_millis(1),
    );

    let checker = {
        let df = df.clone();
        std::thread::spawn(move || df.check())
    };
    entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

    let (added_tx, added_rx) = mpsc::channel();
    let adder = {
        let df = df.clone();
        std::thread::spawn(move || {
            df.add(|| {}, Instant::now() + Duration::from_secs(60));
            added_tx.send(()).unwrap();
        })
    };
    assert!(added_rx.recv_timeout(Duration::from_millis(50)).is_err());

    release_tx.send(()).unwrap();
    checker.join().unwrap();
    added_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    adder.join().unwrap();
    assert_eq!(df.len(), 1);
}
