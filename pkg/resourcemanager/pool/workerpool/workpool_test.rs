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

// workerpool 与 Go workpool_test 对齐的功能单测。
//
// 覆盖任务累加、随机扩缩容、None/有结果通道差异、自定义通道与上下文取消。

#![allow(non_snake_case)]

use crate::workerpool::{
    Channel, Context, Error, NewContext, None as NoResult, TaskMayPanic, Worker, WorkerPool,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

/// 跨任务累加的全局计数，对应 Go 测试中的共享累加器。
static GLOBAL_COUNT: AtomicI64 = AtomicI64::new(0);
/// 待完成任务倒计时，用于 wait_for_tasks 同步。
static TASK_WAIT: LazyLock<(Mutex<usize>, Condvar)> =
    LazyLock::new(|| (Mutex::new(0), Condvar::new()));

/// 增加待完成任务计数。
fn add_pending(count: usize) {
    *TASK_WAIT.0.lock().unwrap() += count;
}

/// 单个任务完成时递减计数并可能唤醒等待者。
fn task_done() {
    let mut pending = TASK_WAIT.0.lock().unwrap();
    assert!(*pending > 0);
    *pending -= 1;
    if *pending == 0 {
        TASK_WAIT.1.notify_all();
    }
}

/// 阻塞直到所有挂起任务完成。
fn wait_for_tasks() {
    let mut pending = TASK_WAIT.0.lock().unwrap();
    while *pending != 0 {
        pending = TASK_WAIT.1.wait(pending).unwrap();
    }
}

/// 携带 i64 载荷的简单任务。
#[derive(Debug)]
struct Int64Task(i64);

impl TaskMayPanic for Int64Task {
    fn RecoverArgs(&self) -> (String, String, Option<Error>) {
        (String::new(), String::new(), None)
    }
}

/// 将任务值累加到 GLOBAL_COUNT 的测试 Worker。
struct MyWorker {
    id: i32,
}

impl Worker<Int64Task, ()> for MyWorker {
    fn HandleTask(&mut self, task: Int64Task, _send: &mut dyn FnMut(())) -> Result<(), Error> {
        GLOBAL_COUNT.fetch_add(task.0, Ordering::SeqCst);
        task_done();
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        let _ = self.id;
        Ok(())
    }
}

/// MyWorker 工厂，对应 Go createMyWorker。
fn createMyWorker() -> MyWorker {
    MyWorker { id: 0 }
}

/// 后台消费结果通道直至关闭，返回收到的条目数。
fn consume_results<R: Send + 'static>(results: Channel<R>) -> thread::JoinHandle<usize> {
    thread::spawn(move || {
        let mut count = 0;
        while results.recv().is_some() {
            count += 1;
        }
        count
    })
}

/// 多轮投递与 Tune 后，累加结果与容量应与 Go 用例一致。
#[test]
fn TestWorkerPool() {
    let mut pool = WorkerPool::<Int64Task, ()>::NewWorkerPool("test", (), 3, createMyWorker);
    pool.Start(NewContext(Context::background()));
    GLOBAL_COUNT.store(0, Ordering::SeqCst);

    let consumer = consume_results(pool.GetResultChan().expect("result channel"));

    add_pending(10);
    for i in 0..10 {
        assert!(pool.AddTask(Int64Task(i)));
    }
    wait_for_tasks();
    assert_eq!(pool.Cap(), 3);
    assert_eq!(GLOBAL_COUNT.load(Ordering::SeqCst), 45);

    pool.Tune(5, false);
    add_pending(10);
    for i in 0..10 {
        assert!(pool.AddTask(Int64Task(i)));
    }
    wait_for_tasks();
    assert_eq!(pool.Cap(), 5);
    assert_eq!(GLOBAL_COUNT.load(Ordering::SeqCst), 90);

    pool.Tune(2, false);
    add_pending(10);
    for i in 0..10 {
        assert!(pool.AddTask(Int64Task(i)));
    }
    wait_for_tasks();
    assert_eq!(pool.Cap(), 2);
    assert_eq!(GLOBAL_COUNT.load(Ordering::SeqCst), 135);

    pool.CloseAndWait();
    assert_eq!(consumer.join().unwrap(), 0);
}

/// 随机反复扩缩容，以及启动前/取消后 Tune 边界。
#[test]
fn TestTunePoolSize() {
    {
        let mut pool = WorkerPool::<Int64Task, ()>::NewWorkerPool("test", (), 3, createMyWorker);
        pool.Start(NewContext(Context::background()));
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        eprintln!("seed: {seed}");
        let mut random = StdRng::seed_from_u64(seed);
        for _ in 0..100 {
            let wait = random.gen_bool(0.5);
            let larger = pool.Cap() + random.gen_range(0..10) + 2;
            pool.Tune(larger, wait);
            assert_eq!(pool.Cap(), larger);
            let smaller = pool.Cap() / 2;
            pool.Tune(smaller, wait);
            assert_eq!(pool.Cap(), smaller);
        }
        pool.CloseAndWait();
    }

    {
        let mut pool = WorkerPool::<Int64Task, ()>::NewWorkerPool("test", (), 10, createMyWorker);
        pool.Tune(5, true);
        pool.Start(NewContext(Context::background()));
        pool.CloseAndWait();
        assert_eq!(pool.Cap(), 5);
    }

    {
        let mut pool = WorkerPool::<Int64Task, ()>::NewWorkerPool("test", (), 10, createMyWorker);
        let worker_context = NewContext(Context::background());
        pool.Start(worker_context.clone());
        worker_context.Cancel();
        pool.Tune(5, true);
        pool.Release();
    }
}

/// 发送默认结果的哑 Worker，用于结果通道有无的断言。
struct DummyWorker;

impl<R: Default + Send> Worker<Int64Task, R> for DummyWorker {
    fn HandleTask(&mut self, _task: Int64Task, send: &mut dyn FnMut(R)) -> Result<(), Error> {
        send(R::default());
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// None 结果无通道；i64/() 结果应有通道。
#[test]
fn TestWorkerPoolNoneResult() {
    let mut pool = WorkerPool::<Int64Task, NoResult>::NewWorkerPool("test", (), 3, || DummyWorker);
    pool.Start(NewContext(Context::background()));
    assert!(pool.GetResultChan().is_none());
    pool.CloseAndWait();

    let mut pool2 = WorkerPool::<Int64Task, i64>::NewWorkerPool("test", (), 3, || DummyWorker);
    pool2.Start(NewContext(Context::background()));
    assert!(pool2.GetResultChan().is_some());
    pool2.CloseAndWait();

    let mut pool3 = WorkerPool::<Int64Task, ()>::NewWorkerPool("test", (), 3, || DummyWorker);
    pool3.Start(NewContext(Context::background()));
    assert!(pool3.GetResultChan().is_some());
    pool3.CloseAndWait();
}

/// 使用外部任务/结果通道时，关闭后应收到全部结果。
#[test]
fn TestWorkerPoolCustomChan() {
    let mut pool = WorkerPool::<Int64Task, i64>::NewWorkerPool("test", (), 3, || DummyWorker);

    let task_channel = Channel::bounded(0);
    pool.SetTaskReceiver(task_channel.clone());
    let result_channel = Channel::bounded(0);
    pool.SetResultSender(result_channel.clone());
    let consumer = consume_results(result_channel);

    pool.Start(NewContext(Context::background()));
    for i in 0..5 {
        assert!(task_channel.send(Int64Task(i)));
    }
    task_channel.close();
    pool.Release();
    assert_eq!(consumer.join().unwrap(), 5);
}

/// 取消 operator 上下文后 Release，Running 应变为 0。
#[test]
fn TestWorkerPoolCancelContext() {
    let worker_context = NewContext(Context::background());
    let mut pool = WorkerPool::<Int64Task, i64>::NewWorkerPool("test", (), 3, || DummyWorker);
    pool.Start(worker_context.clone());
    assert!(pool.AddTask(Int64Task(1)));

    worker_context.Cancel();
    pool.Release();
    assert_eq!(pool.Running(), 0);
}
