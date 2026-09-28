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
// Next 输入计量、子执行器生命周期、错误包装以及 RUV2 指标映射等公共约束。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::executor::{
    BaseExecutorV2, BasicRuntimeStats, Chunk, Error, ExecContext, Executor, FieldType, RUV2Metrics,
    Result, Schema, SessionVars, addRUV2ExecutorMetricCached, calcCellCount, needNextIOAcc,
    ruv2ExecutorMetricByType,
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

    fn ruv2NextCache(&mut self) -> Option<&mut crate::executor::Ruv2NextCacheState> {
        self.base.ruv2NextCache()
    }

    fn reusableNextIOAcc(&mut self) -> Option<Arc<Mutex<crate::executor::NextIOAcc>>> {
        self.base.reusableNextIOAcc()
    }
}

#[test]
/// 验证单元格数及 Next 输入累加器的启用边界与 Go 实现一致。
fn next_io_accounting_matches_go_boundaries() {
    assert_eq!(calcCellCount(3, 0), 0);
    assert_eq!(calcCellCount(3, 4), 12);
    assert_eq!(calcCellCount(usize::MAX, 2), -2);

    let mut acc = crate::executor::NextIOAcc::default();
    acc.addInput(3, 0);
    assert_eq!((acc.in_rows, acc.in_cells), (3, 0));
    acc.in_rows = i64::MAX;
    acc.addInput(1, 1);
    assert_eq!((acc.in_rows, acc.in_cells), (i64::MIN, 1));

    let vars = SessionVars::default();
    let mut executor = BaseExecutorV2::NewBaseExecutorV2(&vars, None, 0, vec![]);
    let first = executor.reusableNextIOAcc().unwrap();
    first.lock().unwrap().addInput(4, 2);
    let second = executor.reusableNextIOAcc().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let second_state = second.lock().unwrap();
    assert_eq!((second_state.in_rows, second_state.in_cells), (0, 0));

    assert!(needNextIOAcc(true, false, 1));
    assert!(!needNextIOAcc(false, false, 1));
    assert!(needNextIOAcc(false, true, 1));
    assert!(!needNextIOAcc(true, true, 0));
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

#[test]
/// 验证 Go 执行器类型到 RUV2 元数据的映射，以及按单元格累计的缓存路径。
fn ruv2_mapping_and_cached_accounting_match_go() {
    let cases = [
        ("*executor.BatchPointGetExec", 1, "BatchPointGetExec", true),
        ("*executor.PointGetExecutor", 1, "PointGetExecutor", true),
        ("*executor.LimitExec", 1, "LimitExec", true),
        ("*aggregate.HashAggExec", 2, "HashAggExec", false),
        ("*aggregate.StreamAggExec", 3, "StreamAggExec", false),
        ("*executor.ExpandExec", 2, "ExpandExec", false),
        (
            "*executor.IndexLookUpExecutor",
            2,
            "IndexLookUpExecutor",
            false,
        ),
        (
            "*executor.IndexReaderExecutor",
            2,
            "IndexReaderExecutor",
            false,
        ),
        (
            "*executor.MemTableReaderExec",
            2,
            "MemTableReaderExec",
            false,
        ),
        ("*executor.ProjectionExec", 2, "ProjectionExec", true),
        ("*executor.SelectionExec", 2, "SelectionExec", false),
        ("*executor.SelectLockExec", 2, "SelectLockExec", true),
        ("*executor.TableDualExec", 2, "TableDualExec", false),
        (
            "*executor.TableReaderExecutor",
            2,
            "TableReaderExecutor",
            false,
        ),
        ("*executor.UnionScanExec", 2, "UnionScanExec", false),
        ("*join.HashJoinV1Exec", 2, "HashJoinV1Exec", false),
        ("*join.HashJoinV2Exec", 2, "HashJoinV2Exec", false),
        ("*join.IndexLookUpJoin", 2, "IndexLookUpJoin", true),
        (
            "*join.IndexLookUpMergeJoin",
            2,
            "IndexLookUpMergeJoin",
            true,
        ),
        (
            "*join.IndexNestedLoopHashJoin",
            2,
            "IndexNestedLoopHashJoin",
            true,
        ),
        ("*join.MergeJoinExec", 2, "MergeJoinExec", false),
        ("*sortexec.TopNExec", 2, "TopNExec", true),
        ("*sortexec.SortExec", 3, "SortExec", true),
        ("*windows.WindowExec", 2, "WindowExec", false),
        ("*windows.PipelinedWindowExec", 2, "WindowExec", false),
        ("*windows.OrderedWindowExec", 2, "WindowExec", false),
    ];
    for (typ, level, label, use_cells) in cases {
        let info = ruv2ExecutorMetricByType(typ).unwrap();
        assert_eq!(
            (info.level, info.label, info.use_cells),
            (level, label, use_cells)
        );
    }
    for stale in [
        "*executor.HashJoinExec",
        "*executor.IndexLookUpJoin",
        "*executor.SortExec",
        "*executor.WindowExec",
    ] {
        assert!(ruv2ExecutorMetricByType(stale).is_none(), "{stale}");
    }

    let metrics = RUV2Metrics::default();
    let info = ruv2ExecutorMetricByType("*executor.PointGetExecutor").unwrap();
    addRUV2ExecutorMetricCached(Some(&metrics), &info, 2, 3, 20, 30);
    assert_eq!(metrics.value(info.level, info.label), 50);
    addRUV2ExecutorMetricCached(Some(&metrics), &info, 0, 0, i64::MAX, i64::MAX);
    assert_eq!(metrics.value(info.level, info.label), 48);
}
