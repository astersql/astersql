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

// 分步（stepped）TestKit：后台线程执行步骤并在断点处同步。
//
// 测试主线程可 `WaitBreakpoint` / `Continue` 控制执行节奏，用于验证并发、
// 事务交错或 DDL 可见性等需要精确时序的场景。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use crate::db_driver::{Database, DbValue};
use crate::result::Result;
use crate::testkit::TestKit;
use crate::{TestError, TestResult};

/// 断点到达/放行计数与停止标志。
#[derive(Default)]
struct BreakpointState {
    /// 各断点名称已到达次数。
    reached: HashMap<String, usize>,
    /// 各断点名称已放行次数。
    released: HashMap<String, usize>,
    /// 为 true 时所有等待方应退出（Drop/停止）。
    stopped: bool,
}

/// 跨线程断点控制器：checkpoint 等待 resume，wait 等待到达次数。
#[derive(Clone, Default)]
pub struct StepController {
    state: Arc<(Mutex<BreakpointState>, Condvar)>,
}

impl StepController {
    /// 工作线程到达命名断点：增加 reached，阻塞直到对应 released 或停止。
    pub fn checkpoint(&self, name: &str) -> TestResult {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().expect("step controller poisoned");
        *state.reached.entry(name.to_owned()).or_default() += 1;
        let occurrence = state.reached[name];
        wake.notify_all();
        // 等待主线程对该 occurrence 放行，或整体 stop。
        while state.released.get(name).copied().unwrap_or_default() < occurrence && !state.stopped {
            state = wake.wait(state).expect("step controller poisoned");
        }
        if state.stopped {
            Err(TestError::new("stepped execution stopped"))
        } else {
            Ok(())
        }
    }

    /// 主线程等待某断点至少到达 `occurrence` 次。
    pub fn wait(&self, name: &str, occurrence: usize) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().expect("step controller poisoned");
        while state.reached.get(name).copied().unwrap_or_default() < occurrence && !state.stopped {
            state = wake.wait(state).expect("step controller poisoned");
        }
    }

    /// 放行某断点一次（与某次 checkpoint 配对）。
    pub fn resume(&self, name: &str) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().expect("step controller poisoned");
        let reached = state.reached.get(name).copied().unwrap_or_default();
        let released = state.released.get(name).copied().unwrap_or_default();
        if state.stopped || released >= reached {
            drop(state);
            panic!("stepped testkit is not stopped at breakpoint {name}");
        }
        *state.released.entry(name.to_owned()).or_default() += 1;
        wake.notify_all();
    }

    /// 标记停止并唤醒所有等待者。
    pub fn stop(&self) {
        let (lock, wake) = &*self.state;
        lock.lock().expect("step controller poisoned").stopped = true;
        wake.notify_all();
    }
}

/// 单步闭包：持有可变 TestKit 与控制器。
type Step = Box<dyn FnOnce(&mut TestKit, &StepController) -> TestResult<Option<Result>> + Send>;

/// 分步测试工具：排队步骤后在后台线程顺序执行。
pub struct SteppedTestKit {
    database: Arc<dyn Database>,
    controller: StepController,
    steps: VecDeque<Step>,
    worker: Option<JoinHandle<TestResult>>,
    last_result: Arc<Mutex<Option<Result>>>,
}

impl SteppedTestKit {
    /// 基于给定 Database 构造未启动的分步 TestKit。
    pub fn new(database: Arc<dyn Database>) -> Self {
        Self {
            database,
            controller: StepController::default(),
            steps: VecDeque::new(),
            worker: None,
            last_result: Arc::new(Mutex::new(None)),
        }
    }

    /// 追加一步：执行 SQL（Exec），忽略结果集。
    pub fn SteppedMustExec(&mut self, sql: &str, args: Vec<DbValue>) -> &mut Self {
        let sql = sql.to_owned();
        self.steps.push_back(Box::new(move |testkit, _| {
            testkit.Exec(&sql, args).map(|_| None)
        }));
        self
    }

    /// 追加一步：执行查询（Query）。
    pub fn SteppedMustQuery(&mut self, sql: &str, args: Vec<DbValue>) -> &mut Self {
        let sql = sql.to_owned();
        self.steps.push_back(Box::new(move |testkit, _| {
            testkit
                .Query(&sql, args)
                .map(|rows| Some(Result::new(rows.string_rows())))
        }));
        self
    }

    /// 追加一步：在工作线程上进入命名断点。
    pub fn Checkpoint(&mut self, name: &str) -> &mut Self {
        let name = name.to_owned();
        self.steps.push_back(Box::new(move |_, controller| {
            controller.checkpoint(&name).map(|_| None)
        }));
        self
    }

    /// 追加自定义步骤闭包。
    pub fn Custom<F>(&mut self, step: F) -> &mut Self
    where
        F: FnOnce(&mut TestKit, &StepController) -> TestResult + Send + 'static,
    {
        self.steps.push_back(Box::new(move |testkit, controller| {
            step(testkit, controller).map(|_| None)
        }));
        self
    }

    /// 启动后台工作线程顺序执行已排队步骤（仅可调用一次）。
    pub fn Start(&mut self) {
        assert!(self.worker.is_none(), "stepped testkit already started");
        let database = self.database.clone();
        let controller = self.controller.clone();
        let mut steps = std::mem::take(&mut self.steps);
        let last_result = Arc::clone(&self.last_result);
        last_result
            .lock()
            .expect("stepped result lock poisoned")
            .take();
        self.worker = Some(thread::spawn(move || {
            let mut testkit = TestKit::new(database);
            while let Some(step) = steps.pop_front() {
                if let Some(result) = step(&mut testkit, &controller)? {
                    *last_result.lock().expect("stepped result lock poisoned") = Some(result);
                }
            }
            Ok(())
        }));
    }

    /// 阻塞直到命名断点到达指定次数。
    pub fn WaitBreakpoint(&self, name: &str, occurrence: usize) {
        self.controller.wait(name, occurrence);
    }
    /// 放行命名断点一次。
    pub fn Continue(&self, name: &str) {
        self.controller.resume(name);
    }

    /// 等待工作线程结束并返回其结果。
    pub fn Wait(&mut self) -> TestResult {
        self.worker
            .take()
            .ok_or_else(|| TestError::new("stepped testkit not started"))?
            .join()
            .map_err(|_| TestError::new("stepped testkit worker panicked"))?
    }

    /// Return the last query result produced by a stepped query.
    pub fn GetResult(&self) -> Option<Result> {
        self.last_result
            .lock()
            .expect("stepped result lock poisoned")
            .clone()
    }

    /// Return the last query result, failing if the last query has not run.
    pub fn GetQueryResult(&self) -> Result {
        self.GetResult()
            .expect("stepped testkit has no completed query result")
    }
}

impl Drop for SteppedTestKit {
    fn drop(&mut self) {
        // 析构时停止控制器，避免工作线程永久阻塞在断点上。
        self.controller.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// 保留对 `Result` 类型的引用，避免未使用告警（与 Go 侧结果类型对齐）。
#[allow(dead_code)]
fn _keep_result_type(_: Result) {}
