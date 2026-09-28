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

// spool 迁移对照单测：选项、构造注册、非阻塞容量、并发提交与调谐。
//
// 覆盖与 Go 一致的非法参数、重名注册、过载、panic 恢复、逐步 Tune
// 以及 release_and_wait 对阻塞提交者的唤醒顺序。

use astersql_resourcemanager_spool::poolmanager::{Meta, MetaEvent, SignalChannel, TaskChannel};
use astersql_resourcemanager_spool::util::UNKNOWN;
use astersql_resourcemanager_spool::*;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// 生成带标签的唯一池名，避免并行测试注册冲突。
fn unique_name(label: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "spool-migration-{label}-{}",
        NEXT.fetch_add(1, Ordering::SeqCst)
    )
}

/// 在超时前自旋等待条件成立（用于异步 worker 收敛）。
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn task_meta_blocking_select_wakes_on_exit_without_polling() {
    let tasks = TaskChannel::new();
    let exit = SignalChannel::bounded(1);
    let meta = Meta::NewMeta(1, exit.clone(), tasks, 1);
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        ready_tx.send(()).unwrap();
        meta.recv_event()
    });
    ready_rx.recv().unwrap();
    exit.try_send().unwrap();
    assert!(matches!(worker.join().unwrap(), MetaEvent::Exit));
}

/// 选项按序应用，默认 blocking=true；后写覆盖先写。
#[test]
fn options_apply_in_order_and_default_to_blocking() {
    assert!(default_option().Blocking);

    let options = [
        with_blocking(false),
        with_blocking(true),
        with_blocking(false),
    ];
    assert!(!load_options(&options).Blocking);
}

#[test]
fn go_option_api_names_remain_available() {
    let defaults = DefaultOption();
    assert!(defaults.Blocking);
    let option: Option = WithBlocking(false);
    assert!(!load_options(&[option]).Blocking);
}

#[test]
fn go_pool_api_names_remain_available() {
    let options = [WithBlocking(false)];
    let pool = NewPool(unique_name("go-api"), 1, UNKNOWN, &options).unwrap();
    assert_eq!(pool.Cap(), 1);
    assert_eq!(pool.Running(), 0);
    assert_eq!(pool.GetOriginConcurrency(), 1);
    pool.Run(|| {}).unwrap();
    wait_until(|| pool.Running() == 0);
    pool.Tune(2);
    assert_eq!(pool.Cap(), 2);
    pool.ReleaseAndWait();
}

#[test]
fn admission_subtraction_wraps_like_go_int32() {
    let options = [WithBlocking(false)];
    let pool = NewPool(unique_name("wrap"), i32::MAX, UNKNOWN, &options).unwrap();
    let tasks = TaskChannel::new();
    pool.RunWithConcurrency(tasks.clone(), 1_u32 << 31).unwrap();
    assert_eq!(pool.Run(|| {}).unwrap_err(), Error::Overload);
    tasks.close();
    pool.ReleaseAndWait();
}

/// 容量为 0 非法；成功构造后登记到 poolmanager，并保留 origin 与 tune 时间戳语义。
#[test]
fn constructor_registers_the_pool_and_preserves_base_pool_state() {
    assert_eq!(
        Pool::new(unique_name("invalid"), 0, UNKNOWN, &[])
            .unwrap_err()
            .to_string(),
        "the pool params are invalid"
    );

    let name = unique_name("registered");
    let pool = Pool::new(name.clone(), 2, UNKNOWN, &[]).unwrap();
    assert_eq!(pool.name(), name);
    assert_eq!(pool.cap(), 2);
    assert_eq!(pool.get_origin_concurrency(), 2);

    let duplicate = Pool::new(name, 1, UNKNOWN, &[]).unwrap_err();
    assert_eq!(duplicate.to_string(), "pool is already exist");

    // tune(0) 应被忽略且不刷新时间戳；有效扩容才更新 last_tuner_ts。
    let before = pool.last_tuner_ts();
    pool.tune(0);
    assert_eq!(pool.last_tuner_ts(), before);
    thread::sleep(Duration::from_millis(1));
    pool.tune(3);
    assert_eq!(pool.cap(), 3);
    assert!(pool.last_tuner_ts() > before);
    pool.release_and_wait();
}

/// 非阻塞模式下满容量返回 Overload；任务 panic 后可恢复并继续接受任务；关闭后返回 Closed。
#[test]
fn nonblocking_run_enforces_capacity_and_recovers_panics() {
    let options = [with_blocking(false)];
    let pool = Pool::new(unique_name("run"), 1, UNKNOWN, &options).unwrap();
    let (release, blocked) = mpsc::channel();
    pool.run(move || blocked.recv().unwrap()).unwrap();
    assert_eq!(pool.running(), 1);
    assert_eq!(pool.run(|| {}).unwrap_err(), Error::Overload);

    release.send(()).unwrap();
    wait_until(|| pool.running() == 0);
    pool.run(|| panic!("planned task panic")).unwrap();
    wait_until(|| pool.running() == 0);
    pool.release_and_wait();
    assert_eq!(pool.run(|| {}).unwrap_err(), Error::Closed);
}

/// run_with_concurrency 按当前空闲槽启动 worker；满载再提交 Overload；close 后排空任务。
#[test]
fn run_with_concurrency_uses_available_slots_and_drains_until_close() {
    let options = [with_blocking(false)];
    let pool = Pool::new(unique_name("concurrency"), 3, UNKNOWN, &options).unwrap();
    let tasks = TaskChannel::new();
    let completed = Arc::new(AtomicUsize::new(0));
    for _ in 0..20 {
        let completed = Arc::clone(&completed);
        tasks
            .send(Box::new(move || {
                completed.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
    }

    pool.run_with_concurrency(tasks.clone(), 100).unwrap();
    assert_eq!(pool.running(), 3);
    assert_eq!(
        pool.run_with_concurrency(tasks.clone(), 1).unwrap_err(),
        Error::Overload
    );
    tasks.close();

    wait_until(|| completed.load(Ordering::SeqCst) == 20);
    wait_until(|| pool.running() == 0);
    pool.release_and_wait();
}

/// Tune 对已注册的 concurrency worker 逐步扩/缩，每次一步到位到目标 running。
#[test]
fn tuning_scales_registered_task_workers_one_step_at_a_time() {
    let options = [with_blocking(false)];
    let pool = Pool::new(unique_name("tune"), 1, UNKNOWN, &options).unwrap();
    let tasks = TaskChannel::new();
    pool.run_with_concurrency(tasks.clone(), 3).unwrap();
    assert_eq!(pool.running(), 1);
    let ready = Arc::new(AtomicUsize::new(0));
    let worker_ready = Arc::clone(&ready);
    tasks
        .send(Box::new(move || {
            worker_ready.store(1, Ordering::SeqCst);
        }))
        .unwrap();
    wait_until(|| ready.load(Ordering::SeqCst) == 1);

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

/// release_and_wait：先让阻塞中的提交者以 Overload 失败，再等在跑任务结束后才返回。
#[test]
fn release_stops_blocked_submitters_then_waits_for_running_tasks() {
    let pool = Pool::new(unique_name("release"), 1, UNKNOWN, &[]).unwrap();
    let (finish_first, first) = mpsc::channel();
    pool.run(move || first.recv().unwrap()).unwrap();

    let blocked_pool = pool.clone();
    let blocked = thread::spawn(move || blocked_pool.run(|| {}));
    wait_until(|| pool.waiting() == 1);

    let release_pool = pool.clone();
    let released = Arc::new(AtomicUsize::new(0));
    let release_observer = Arc::clone(&released);
    let release = thread::spawn(move || {
        release_pool.release_and_wait();
        release_observer.store(1, Ordering::SeqCst);
    });

    assert_eq!(blocked.join().unwrap().unwrap_err(), Error::Overload);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    finish_first.send(()).unwrap();
    release.join().unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);
}
