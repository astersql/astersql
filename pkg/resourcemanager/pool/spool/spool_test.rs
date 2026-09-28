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

// spool 线程池行为单测：释放、调容、过载与 TaskManager 协作。
//
// 对应 Go `spool_test.go`：验证并发提交下 release、扩缩容、非阻塞 Overload，
// 以及 `run_with_concurrency` 在槽位不足时的截断与后续拒绝。

#![allow(non_snake_case)]

use astersql_resourcemanager_spool::poolmanager::TaskChannel;
use astersql_resourcemanager_spool::util::UNKNOWN;
use astersql_resourcemanager_spool::{Error, Pool, with_blocking};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// 生成带标签的唯一池名，避免并行测试注册冲突。
fn unique_name(label: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("spool-test-{label}-{}", NEXT.fetch_add(1, Ordering::SeqCst))
}

/// 在超时前轮询等待条件成立，用于同步异步线程状态。
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

/// 进程内只执行一次公共测试环境初始化。
fn setup_for_common_test() {
    static SETUP: Once = Once::new();
    SETUP.call_once(testsetup::SetupForCommonTest);
}

/// 故意消耗栈的演示任务，用于 Overload 场景下的额外提交。
fn demoFunc() {
    f(2);
}

/// 递归分配栈帧以增加栈压力。
fn f(n: i32) {
    if n == 0 {
        return;
    }
    let use_stack = [0_u8; 100];
    let _ = use_stack[3];
    f(n - 1);
}

/// 多线程持续提交时调用 release_and_wait，确认能安全收尾。
#[test]
fn TestReleaseWhenRunningPool() {
    setup_for_common_test();
    let pool = Pool::new(unique_name("release-running"), 1, UNKNOWN, &[]).unwrap();
    let mut submitters = Vec::new();

    for range in [0..30, 100..130] {
        let pool = pool.clone();
        submitters.push(thread::spawn(move || {
            for _ in range {
                let _ = pool.run(|| thread::sleep(Duration::from_micros(100)));
            }
        }));
    }

    thread::sleep(Duration::from_micros(100));
    pool.release_and_wait();
    for submitter in submitters {
        submitter.join().unwrap();
    }
}

/// 验证 tune 扩缩容、阻塞任务释放后 running 回落，以及 TaskChannel 多并发消费。
#[test]
fn TestPoolTuneScaleUpAndDown() {
    setup_for_common_test();
    let options = [with_blocking(true)];
    let pool = Pool::new(unique_name("tune"), 2, UNKNOWN, &options).unwrap();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));

    // 构造需显式信号才能结束的阻塞任务。
    let blocking_task = || {
        let release_rx = Arc::clone(&release_rx);
        move || {
            release_rx.lock().unwrap().recv().unwrap();
        }
    };

    for _ in 0..2 {
        pool.run(blocking_task()).unwrap();
    }
    assert_eq!(pool.running(), 2);

    pool.tune(3);
    pool.run(blocking_task()).unwrap();
    assert_eq!(pool.running(), 3);

    let mut submitters = Vec::new();
    for _ in 0..5 {
        let pool = pool.clone();
        let release_rx = Arc::clone(&release_rx);
        submitters.push(thread::spawn(move || {
            pool.run(move || {
                release_rx.lock().unwrap().recv().unwrap();
            })
        }));
    }
    pool.tune(8);
    for submitter in submitters {
        submitter.join().unwrap().unwrap();
    }
    wait_until(|| pool.running() == 8);

    pool.tune(2);
    for _ in 0..6 {
        release_tx.send(()).unwrap();
    }
    wait_until(|| pool.running() == 2);
    for _ in 0..2 {
        release_tx.send(()).unwrap();
    }
    wait_until(|| pool.running() == 0);

    let completed = Arc::new(AtomicUsize::new(0));
    let tasks = TaskChannel::new();
    pool.run_with_concurrency(tasks.clone(), 2).unwrap();
    assert_eq!(pool.running(), 2);
    for _ in 0..10 {
        let completed = Arc::clone(&completed);
        tasks
            .send(Box::new(move || {
                completed.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
    }
    wait_until(|| completed.load(Ordering::SeqCst) == 10);
    assert_eq!(pool.running(), 2);
    tasks.close();
    wait_until(|| pool.running() == 0);
    pool.release_and_wait();
}

/// 非阻塞模式下容量占满后再次 run 应返回 Overload。
#[test]
fn TestRunOverload() {
    setup_for_common_test();
    let stop = Arc::new(AtomicBool::new(false));
    let options = [with_blocking(false)];
    let pool = Pool::new(unique_name("overload"), 10, UNKNOWN, &options).unwrap();

    for _ in 0..10 {
        let stop = Arc::clone(&stop);
        pool.run(move || {
            while !stop.load(Ordering::SeqCst) {
                thread::yield_now();
            }
        })
        .unwrap();
    }
    assert_eq!(pool.run(demoFunc).unwrap_err(), Error::Overload);

    stop.store(true, Ordering::SeqCst);
    pool.release_and_wait();
}

/// 请求并发远大于容量时截断到容量，后续提交应 Overload。
#[test]
fn TestRunWithNotEnough() {
    setup_for_common_test();
    let stop = AtomicBool::new(false);
    let tasks = TaskChannel::new();
    let options = [with_blocking(false)];
    let pool = Pool::new(unique_name("not-enough"), 10, UNKNOWN, &options).unwrap();

    pool.run_with_concurrency(tasks.clone(), 110).unwrap();
    assert_eq!(pool.running(), 10);
    assert_eq!(
        pool.run_with_concurrency(tasks.clone(), 1).unwrap_err(),
        Error::Overload
    );
    assert_eq!(pool.run(|| {}).unwrap_err(), Error::Overload);
    stop.store(true, Ordering::SeqCst);
    tasks.close();
    wait_until(|| pool.running() == 0);
    pool.release_and_wait();
}

/// 容量为 1 时请求并发 2 只启动 1 个 worker，仍能消费完通道任务。
#[test]
fn TestRunWithNotEnough2() {
    setup_for_common_test();
    let tasks = TaskChannel::new();
    let completed = Arc::new(AtomicUsize::new(0));
    let options = [with_blocking(false)];
    let pool = Pool::new(unique_name("not-enough-two"), 1, UNKNOWN, &options).unwrap();

    pool.run_with_concurrency(tasks.clone(), 2).unwrap();
    assert_eq!(pool.running(), 1);
    assert_eq!(
        pool.run_with_concurrency(tasks.clone(), 1).unwrap_err(),
        Error::Overload
    );
    assert_eq!(pool.run(|| {}).unwrap_err(), Error::Overload);
    for _ in 0..100 {
        let completed = Arc::clone(&completed);
        tasks
            .send(Box::new(move || {
                completed.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
    }
    tasks.close();
    wait_until(|| pool.running() == 0);
    assert_eq!(completed.load(Ordering::SeqCst), 100);
    pool.release_and_wait();
}

/// 验证 TaskManager 配合 tune 扩缩容时 running 随 Overclock/Downclock 变化。
#[test]
fn TestWithTaskManager() {
    setup_for_common_test();
    let tasks = TaskChannel::new();
    let options = [with_blocking(false)];
    let pool = Pool::new(unique_name("task-manager"), 1, UNKNOWN, &options).unwrap();

    pool.run_with_concurrency(tasks.clone(), 2).unwrap();
    let worker_ready = Arc::new(AtomicBool::new(false));
    let ready = Arc::clone(&worker_ready);
    tasks
        .send(Box::new(move || ready.store(true, Ordering::SeqCst)))
        .unwrap();
    wait_until(|| worker_ready.load(Ordering::SeqCst));
    assert_eq!(pool.running(), 1);

    pool.tune(2);
    wait_until(|| pool.running() == 2);
    pool.tune(3);
    wait_until(|| pool.running() == 3);

    pool.tune(2);
    wait_until(|| pool.running() == 2);
    pool.tune(1);
    wait_until(|| pool.running() == 1);

    tasks.close();
    wait_until(|| pool.running() == 0);
    pool.release_and_wait();
}
