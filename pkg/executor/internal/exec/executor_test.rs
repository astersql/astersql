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

// 执行器通用包装层的回归测试。
//
// 通过可配置的测试执行器记录生命周期事件并注入延迟、错误或 panic，覆盖
// 子执行器生命周期与错误包装等公共约束。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::executor::{
    BaseExecutorV2, BasicRuntimeStats, Chunk, Error, ExecContext, Executor, FieldType, Result,
    Schema, SessionVars,
};

/// 可观察并可注入异常行为的最小执行器，用于隔离验证外层包装逻辑。
struct TestExecutor {
    base: BaseExecutorV2,
    events: Arc<Mutex<Vec<&'static str>>>,
    open_delay: Duration,
    next_rows: usize,
    next_cols: usize,
    open_error: Option<Error>,
    close_error: Option<Error>,
    panic_on_open: bool,
}

impl TestExecutor {
    fn new(vars: &SessionVars, events: Arc<Mutex<Vec<&'static str>>>) -> Self {
        Self {
            base: BaseExecutorV2::NewBaseExecutorV2(vars, None, 0, vec![]),
            events,
            open_delay: Duration::ZERO,
            next_rows: 0,
            next_cols: 0,
            open_error: None,
            close_error: None,
            panic_on_open: false,
        }
    }

    fn record(&self, event: &'static str) {
        self.events.lock().unwrap().push(event);
    }
}

impl Executor for TestExecutor {
    fn executorType(&self) -> &'static str {
        "*executor.PointGetExecutor"
    }

    fn Open(&mut self, _ctx: &ExecContext) -> Result<()> {
        self.record("open");
        if self.panic_on_open {
            panic!("test panic");
        }
        thread::sleep(self.open_delay);
        self.open_error.clone().map_or(Ok(()), Err)
    }

    fn Next(&mut self, _ctx: &ExecContext, req: &mut Chunk) -> Result<()> {
        self.record("next");
        req.SetNumRows(self.next_rows);
        if self.next_cols != req.NumCols() {
            return Err(Error::Other(
                "test executor received wrong column count".into(),
            ));
        }
        Ok(())
    }

    fn Close(&mut self) -> Result<()> {
        self.record("close");
        self.close_error.clone().map_or(Ok(()), Err)
    }

    fn Schema(&self) -> Schema {
        self.base.Schema()
    }

    fn RetFieldTypes(&self) -> Vec<FieldType> {
        self.base.RetFieldTypes()
    }

    fn InitCap(&self) -> usize {
        self.base.InitCap()
    }

    fn MaxChunkSize(&self) -> usize {
        self.base.MaxChunkSize()
    }

    fn AllChildren(&self) -> &[Box<dyn Executor>] {
        self.base.AllChildren()
    }

    fn SetAllChildren(&mut self, children: Vec<Box<dyn Executor>>) {
        self.base.SetAllChildren(children)
    }

    fn RuntimeStats(&self) -> Option<Arc<Mutex<BasicRuntimeStats>>> {
        self.base.RuntimeStats()
    }

    fn HandleSQLKillerSignal(&self) -> Result<()> {
        self.base.HandleSQLKillerSignal()
    }

    fn RegisterSQLAndPlanInExecForTopProfiling(&self) {
        self.base.RegisterSQLAndPlanInExecForTopProfiling()
    }
}

#[test]
/// 验证父执行器依次包装子节点的 Open/Close，并记录耗时与限制输出块大小。
fn base_executor_lifecycle_wraps_children_and_records_open_duration() {
    let vars = SessionVars::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut child = TestExecutor::new(&vars, events.clone());
    child.open_delay = Duration::from_millis(2);
    let mut parent = BaseExecutorV2::NewBaseExecutorV2(
        &vars,
        Some(Schema {
            fields: vec![FieldType { type_code: 1 }],
        }),
        7,
        vec![Box::new(child)],
    );

    crate::executor::Open(&ExecContext::default(), &mut parent).unwrap();
    assert_eq!(events.lock().unwrap().as_slice(), &["open"]);
    let stats = parent.RuntimeStats().unwrap();
    assert!(stats.lock().unwrap().open >= Duration::from_millis(1));

    let mut chunk = parent.NewChunk();
    assert_eq!(chunk.NumCols(), 1);
    assert_eq!(chunk.capacity, vars.init_chunk_size);
    assert_eq!(chunk.max_size, vars.max_chunk_size);
    chunk.SetNumRows(vars.max_chunk_size + 1);
    assert_eq!(chunk.NumRows(), vars.max_chunk_size);

    crate::executor::Close(&mut parent).unwrap();
    assert_eq!(events.lock().unwrap().as_slice(), &["open", "close"]);
}

#[test]
/// SQL kill 应在调用执行器前短路，而执行器 panic 应转换为统一错误。
fn wrappers_propagate_killer_and_panic_errors() {
    let mut vars = SessionVars::default();
    vars.killed
        .store(true, std::sync::atomic::Ordering::Release);
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut executor = TestExecutor::new(&vars, events.clone());
    executor.next_cols = 0;
    let mut chunk = executor.NewChunk();
    assert_eq!(
        crate::executor::Next(&ExecContext::default(), &mut executor, &mut chunk),
        Err(Error::Killed)
    );
    assert!(events.lock().unwrap().is_empty());

    let vars = SessionVars::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut executor = TestExecutor::new(&vars, events);
    executor.panic_on_open = true;
    assert_eq!(
        crate::executor::Open(&ExecContext::default(), &mut executor),
        Err(Error::Panic)
    );
}
