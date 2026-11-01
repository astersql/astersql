// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::{
    backpressure::{DEFAULT_DELAY, TaskBackpressureController},
    server_maintainer::Cancellation,
    time_proxy::{Clock, current_clock},
};
use crossbeam_channel::bounded;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};
pub type Task = Box<dyn FnOnce() -> Result<(), String> + Send + 'static>;
pub(crate) struct TaskRequest {
    pub name: String,
    pub task: Task,
}
#[derive(Default)]
pub(crate) struct TaskQueue {
    pub tasks: VecDeque<TaskRequest>,
}
impl TaskQueue {
    pub fn push(&mut self, t: TaskRequest) {
        self.tasks.push_back(t)
    }
    pub fn pop(&mut self) -> Option<TaskRequest> {
        self.tasks.pop_front()
    }
    pub fn len(&self) -> usize {
        self.tasks.len()
    }
    pub fn clear(&mut self) -> usize {
        let n = self.len();
        self.tasks = VecDeque::new();
        n
    }
}
#[derive(Default)]
pub struct Metrics {
    pub submitted: AtomicI64,
    pub finished: AtomicI64,
    pub failed: AtomicI64,
    pub timeout: AtomicI64,
    pub rejected: AtomicI64,
    pub backpressure: AtomicI64,
    pub running: AtomicI64,
    pub waiting: AtomicI64,
    pub timed_out_running: AtomicI64,
    pub backpressure_blocked: AtomicI64,
}
#[derive(Default)]
struct State {
    lifecycle: u8,
    queue: TaskQueue,
    workers: usize,
    max: usize,
    pending: usize,
}
pub struct TaskExecutor {
    state: Mutex<State>,
    changed: Condvar,
    timeout: Mutex<Duration>,
    backpressure: RwLock<Option<Arc<dyn TaskBackpressureController>>>,
    close: Cancellation,
    clock: Clock,
    pub metrics: Metrics,
}
impl TaskExecutor {
    pub fn new(max: isize, timeout: Duration) -> Arc<Self> {
        Self::with_clock(max, timeout, current_clock())
    }
    pub fn with_clock(max: isize, timeout: Duration, clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                max: max.max(1) as usize,
                ..State::default()
            }),
            changed: Condvar::new(),
            timeout: Mutex::new(timeout),
            backpressure: RwLock::new(None),
            close: Cancellation::default(),
            clock,
            metrics: Metrics::default(),
        })
    }
    pub fn run(self: &Arc<Self>) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.lifecycle != 0 {
            return false;
        }
        s.lifecycle = 1;
        let n = s.max;
        self.start_workers(&mut s, n);
        true
    }
    fn start_workers(self: &Arc<Self>, s: &mut State, n: usize) {
        for _ in 0..n {
            s.workers += 1;
            let e = self.clone();
            std::thread::spawn(move || e.worker_loop());
        }
    }
    pub fn set_max_concurrency(self: &Arc<Self>, max: isize) {
        if max <= 0 {
            return;
        }
        let mut s = self.state.lock().unwrap();
        if s.lifecycle == 2 {
            return;
        }
        let old = s.max;
        s.max = max as usize;
        if s.max > old {
            let n = s.max - old;
            self.start_workers(&mut s, n)
        } else {
            self.changed.notify_all()
        }
    }
    pub fn set_timeout(&self, timeout: Duration) {
        let s = self.state.lock().unwrap();
        if s.lifecycle != 2 {
            *self.timeout.lock().unwrap() = timeout
        }
    }
    pub fn set_backpressure_controller(&self, c: Option<Arc<dyn TaskBackpressureController>>) {
        *self.backpressure.write().unwrap() = c
    }
    pub fn submit(&self, name: impl Into<String>, task: Task) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.lifecycle == 2 {
            self.metrics.rejected.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        self.metrics.submitted.fetch_add(1, Ordering::SeqCst);
        self.metrics.waiting.fetch_add(1, Ordering::SeqCst);
        s.pending += 1;
        s.queue.push(TaskRequest {
            name: name.into(),
            task,
        });
        self.changed.notify_one();
        true
    }
    pub fn close(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.lifecycle == 2 {
            return false;
        }
        s.lifecycle = 2;
        self.close.cancel();
        let n = s.queue.clear();
        s.pending -= n;
        self.metrics.waiting.fetch_sub(n as i64, Ordering::SeqCst);
        self.changed.notify_all();
        while s.pending > 0 || s.workers > 0 {
            s = self.changed.wait(s).unwrap()
        }
        true
    }
    fn exit_worker(&self, s: &mut State) -> bool {
        if (s.lifecycle == 2 && s.queue.len() == 0) || s.workers > s.max {
            s.workers -= 1;
            self.changed.notify_all();
            true
        } else {
            false
        }
    }
    fn next_task(&self) -> Option<TaskRequest> {
        let mut in_backpressure = false;
        loop {
            let mut s = self.state.lock().unwrap();
            while s.lifecycle != 2 && s.queue.len() == 0 {
                if self.exit_worker(&mut s) {
                    return None;
                }
                s = self.changed.wait(s).unwrap();
            }
            if self.exit_worker(&mut s) {
                return None;
            }
            drop(s);
            // Clone under the controller lock, then call external code outside all locks.
            let controller = self.backpressure.read().unwrap().clone();
            if let Some(c) = controller {
                let (blocked, delay) = c.should_backpressure();
                if blocked {
                    if !in_backpressure {
                        self.metrics.backpressure.fetch_add(1, Ordering::SeqCst);
                        in_backpressure = true
                    }
                    self.metrics
                        .backpressure_blocked
                        .fetch_add(1, Ordering::SeqCst);
                    let done = self.close.done();
                    let after = self.clock.after(if delay.is_zero() {
                        DEFAULT_DELAY
                    } else {
                        delay
                    });
                    crossbeam_channel::select! {
                        recv(done) -> _ => {},
                        recv(after) -> _ => {},
                    }
                    self.metrics
                        .backpressure_blocked
                        .fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
            }
            in_backpressure = false;
            let mut s = self.state.lock().unwrap();
            if self.exit_worker(&mut s) {
                return None;
            }
            if let Some(t) = s.queue.pop() {
                self.metrics.waiting.fetch_sub(1, Ordering::SeqCst);
                return Some(t);
            }
        }
    }
    fn worker_loop(self: Arc<Self>) {
        while let Some(req) = self.next_task() {
            self.run_task(req)
        }
    }
    pub(crate) fn safe_execute(req: TaskRequest) -> Result<(), String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(req.task)) {
            Ok(result) => result,
            Err(panic) => {
                let message = if let Some(m) = panic.downcast_ref::<&str>() {
                    (*m).to_owned()
                } else if let Some(m) = panic.downcast_ref::<String>() {
                    m.clone()
                } else {
                    format!("{panic:?}")
                };
                let err = format!("task panicked, task={}, panic={message}", req.name);
                log::error!(
                    "{err}; stack={}",
                    std::backtrace::Backtrace::force_capture()
                );
                Err(err)
            }
        }
    }
    fn finish_task(&self, result: Result<(), String>) {
        let mut s = self.state.lock().unwrap();
        self.metrics.finished.fetch_add(1, Ordering::SeqCst);
        if result.is_err() {
            self.metrics.failed.fetch_add(1, Ordering::SeqCst);
        }
        s.pending -= 1;
        self.changed.notify_all();
    }
    fn run_task(self: &Arc<Self>, req: TaskRequest) {
        self.metrics.running.fetch_add(1, Ordering::SeqCst);
        let timeout = *self.timeout.lock().unwrap();
        if timeout.is_zero() {
            let result = Self::safe_execute(req);
            self.metrics.running.fetch_sub(1, Ordering::SeqCst);
            self.finish_task(result);
            return;
        }
        let task_name = req.name.clone();
        let task_start = self.clock.now();
        let timer = self.clock.timer(timeout);
        let (tx, done) = bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(Self::safe_execute(req));
        });
        crossbeam_channel::select! {
            recv(done) -> result => {
                self.metrics.running.fetch_sub(1, Ordering::SeqCst);
                self.finish_task(result.unwrap());
            },
            recv(timer.c) -> _ => {
                self.metrics.timeout.fetch_add(1, Ordering::SeqCst);
                self.metrics.running.fetch_sub(1, Ordering::SeqCst);
                self.metrics.timed_out_running.fetch_add(1, Ordering::SeqCst);
                log::warn!(
                    "task timed out, continue in background; task={task_name}; exec_start={task_start}; timeout={timeout:?}; running_count={}; waiting_count={}; timed_out_running_count={}",
                    self.metrics.running.load(Ordering::SeqCst),
                    self.metrics.waiting.load(Ordering::SeqCst),
                    self.metrics.timed_out_running.load(Ordering::SeqCst),
                );
                let e = self.clone();
                std::thread::spawn(move || {
                    let result = done.recv().unwrap();
                    e.metrics.timed_out_running.fetch_sub(1, Ordering::SeqCst);
                    e.finish_task(result);
                });
            }
        }
    }
}
