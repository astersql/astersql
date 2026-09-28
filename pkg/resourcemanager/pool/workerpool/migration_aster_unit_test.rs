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

// workerpool 核心行为的迁移对照单测。
//
// 覆盖构造/预启动调容、任务结果与动态 Tune、Context 首错与取消传播、
// worker 错误/panic、缩容等待 Close，以及无结果类型与自定义通道。

use crate::workerpool::*;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

/// 测试任务：可携带业务失败或预置 panic 恢复错误。
#[derive(Clone)]
struct TestTask {
    value: i64,
    failure: Option<Error>,
    panic_error: Option<Error>,
}

impl TestTask {
    /// 构造仅含数值的普通任务。
    fn value(value: i64) -> Self {
        Self {
            value,
            failure: std::option::Option::None,
            panic_error: std::option::Option::None,
        }
    }
}

impl TaskMayPanic for TestTask {
    fn RecoverArgs(&self) -> (String, String, Option<Error>) {
        (
            "workerpool-test".to_owned(),
            format!("task {}", self.value),
            self.panic_error.clone(),
        )
    }
}

/// 聚合 worker 处理进度：总和、已处理数与关闭次数。
#[derive(Default)]
struct WorkerState {
    total: AtomicI64,
    handled: Mutex<usize>,
    handled_changed: Condvar,
    closed: AtomicUsize,
}

impl WorkerState {
    /// 阻塞等待已处理任务数达到 expected。
    fn wait_for(&self, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut handled = self.handled.lock().unwrap();
        while *handled < expected {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {expected} tasks"
            );
            let (next, timeout) = self
                .handled_changed
                .wait_timeout(handled, remaining)
                .unwrap();
            handled = next;
            assert!(!timeout.timed_out() || *handled >= expected);
        }
    }
}

/// 轮询直到池 Running 为 0。
fn wait_until_idle(pool: &WorkerPool<TestTask, i64>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while pool.Running() != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for pool to become idle"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

/// 将任务值累加到共享状态并回传结果。
struct TestWorker {
    state: Arc<WorkerState>,
}

impl Worker<TestTask, i64> for TestWorker {
    fn HandleTask(&mut self, task: TestTask, send: &mut dyn FnMut(i64)) -> Result<(), Error> {
        if let Some(err) = task.failure {
            return Err(err);
        }
        if task.panic_error.is_some() {
            panic!("planned task panic");
        }
        self.state.total.fetch_add(task.value, Ordering::SeqCst);
        send(task.value);
        let mut handled = self.state.handled.lock().unwrap();
        *handled += 1;
        self.state.handled_changed.notify_all();
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        self.state.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// 用共享状态工厂创建测试池。
fn pool_with_state(workers: i32, state: Arc<WorkerState>) -> WorkerPool<TestTask, i64> {
    WorkerPool::NewWorkerPool("test", (), workers, move || {
        Box::new(TestWorker {
            state: Arc::clone(&state),
        })
    })
}

/// 非正容量钳制为 1，预启动 Tune 应更新 Cap 与 LastTunerTs。
#[test]
fn constructor_and_pre_start_tune_match_go_boundaries() {
    let state = Arc::new(WorkerState::default());
    let mut pool = pool_with_state(0, state);
    assert_eq!(pool.Name(), "test");
    assert_eq!(pool.Cap(), 1);
    assert_eq!(pool.GetOriginConcurrency(), 1);
    assert_eq!(pool.LastTunerTs(), UNIX_EPOCH);

    pool.Tune(5, true);
    assert_eq!(pool.Cap(), 5);
    assert!(pool.LastTunerTs() > UNIX_EPOCH);
    pool.Start(NewContext(Context::background()));
    pool.CloseAndWait();
}

/// 多轮投递任务并动态扩缩容，结果与关闭次数应与 Go 一致。
#[test]
fn tasks_results_and_dynamic_tuning_match_go() {
    let state = Arc::new(WorkerState::default());
    let mut pool = pool_with_state(3, Arc::clone(&state));
    pool.Start(NewContext(Context::background()));
    let results = pool
        .GetResultChan()
        .expect("ordinary result type has a channel");
    let consumer = thread::spawn(move || {
        let mut sum = 0;
        while let Some(value) = results.recv() {
            sum += value;
        }
        sum
    });

    for round in 0..3 {
        for value in 0..10 {
            assert!(pool.AddTask(TestTask::value(value)));
        }
        state.wait_for((round + 1) * 10);
        match round {
            0 => pool.Tune(5, false),
            1 => pool.Tune(2, true),
            _ => {}
        }
    }

    wait_until_idle(&pool);
    assert_eq!(pool.Cap(), 2);
    assert_eq!(pool.Running(), 0);
    assert_eq!(state.total.load(Ordering::SeqCst), 135);
    pool.CloseAndWait();
    assert_eq!(consumer.join().unwrap(), 135);
    assert_eq!(state.closed.load(Ordering::SeqCst), 5);
}

/// 子上下文保留首错、取消不影响父；父取消应传播到后代。
#[test]
fn context_keeps_first_error_and_propagates_cancellation() {
    let parent = Context::background();
    let child = NewContext(parent.clone());
    child.OnError(Error::new("first"));
    child.OnError(Error::new("second"));
    assert_eq!(child.OperatorErr().unwrap().to_string(), "first");
    assert!(child.IsCancelled());
    assert!(
        !parent.IsCancelled(),
        "child cancellation must not cancel its parent"
    );

    let descendant = NewContext(parent.clone());
    parent.Cancel();
    assert!(
        descendant.IsCancelled(),
        "parent cancellation must reach children"
    );
}

/// HandleTask 返回错误或 panic 时，应取消流水线并写入 OperatorErr。
#[test]
fn worker_error_and_panic_cancel_the_pipeline() {
    for task in [
        TestTask {
            value: 1,
            failure: Some(Error::new("handle failed")),
            panic_error: std::option::Option::None,
        },
        TestTask {
            value: 2,
            failure: std::option::Option::None,
            panic_error: Some(Error::new("recovered panic")),
        },
    ] {
        let state = Arc::new(WorkerState::default());
        let mut pool = pool_with_state(1, state);
        let context = NewContext(Context::background());
        pool.Start(context.clone());
        assert!(pool.AddTask(task.clone()));
        pool.Release();
        assert_eq!(context.OperatorErr(), task.failure.or(task.panic_error));
        assert_eq!(pool.Running(), 0);
    }
}

/// Tune(wait=true) 应等待被移除 worker 的 Close 完成（类似 WaitGroup）。
#[test]
fn tune_wait_includes_worker_close_like_go_waitgroup() {
    struct SlowClosingWorker {
        closed: Arc<AtomicUsize>,
    }

    impl Worker<TestTask, None> for SlowClosingWorker {
        fn HandleTask(
            &mut self,
            _task: TestTask,
            _send: &mut dyn FnMut(None),
        ) -> Result<(), Error> {
            Ok(())
        }

        fn Close(&mut self) -> Result<(), Error> {
            thread::sleep(Duration::from_millis(50));
            self.closed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let closed = Arc::new(AtomicUsize::new(0));
    let factory_state = Arc::clone(&closed);
    let mut pool = WorkerPool::NewWorkerPool("slow-close", (), 2, move || SlowClosingWorker {
        closed: Arc::clone(&factory_state),
    });
    pool.Start(NewContext(Context::background()));
    pool.Tune(1, true);
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    pool.CloseAndWait();
    assert_eq!(closed.load(Ordering::SeqCst), 2);
}

/// None 结果无通道；自定义任务/结果通道应能完整投递。
#[test]
fn none_result_and_custom_channels_match_go() {
    struct NoResultWorker;
    impl Worker<TestTask, None> for NoResultWorker {
        fn HandleTask(
            &mut self,
            _task: TestTask,
            _send: &mut dyn FnMut(None),
        ) -> Result<(), Error> {
            Ok(())
        }
        fn Close(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    let mut none_pool = WorkerPool::NewWorkerPool("none", (), 1, || Box::new(NoResultWorker));
    none_pool.Start(NewContext(Context::background()));
    assert!(none_pool.GetResultChan().is_none());
    none_pool.CloseAndWait();

    let state = Arc::new(WorkerState::default());
    let mut pool = pool_with_state(3, Arc::clone(&state));
    let tasks = Channel::bounded(0);
    let results = Channel::bounded(0);
    pool.SetTaskReceiver(tasks.clone());
    pool.SetResultSender(results.clone());
    pool.Start(NewContext(Context::background()));
    let consumer = thread::spawn(move || {
        let mut count = 0;
        while results.recv().is_some() {
            count += 1;
        }
        count
    });
    for value in 0..5 {
        assert!(tasks.send(TestTask::value(value)));
    }
    tasks.close();
    pool.Release();
    assert_eq!(consumer.join().unwrap(), 5);
    assert_eq!(state.total.load(Ordering::SeqCst), 10);
}

/// 取消应打断阻塞在结果发送上的 worker，使 Running 归零。
#[test]
fn cancellation_unblocks_a_worker_waiting_to_send_result() {
    let state = Arc::new(WorkerState::default());
    let mut pool = pool_with_state(1, state);
    let context = NewContext(Context::background());
    pool.Start(context.clone());
    assert!(pool.AddTask(TestTask::value(1)));
    context.Cancel();
    pool.Release();
    assert_eq!(pool.Running(), 0);
}
