// Copyright 2026 AsterSQL.
// `EXPLAIN ANALYZE` 执行器单元测试。
//
// 验证 `ExplainExec` 在分析模式下：
// - 先 Open/Next/Close 被分析子执行器，再分页输出渲染后的计划行；
// - `generateExplainInfo` 在 Next 与 Close 同时失败时保留两者错误信息。
//
// `EXPLAIN ANALYZE`：在真正执行 SQL 的同时收集运行时统计并展示执行计划。

use std::sync::{Arc, Mutex};

use crate::explain::{
    ExplainAnalyzeExecutor, ExplainContext, ExplainExec, ExplainPlan, ExplainRuntime,
    MemoryDebugModeHandler,
};
use crate::foreign_key::WithForeignKeyTrigger;
use astersql_errors as errors;
use astersql_types::datum::FieldType;
use astersql_util_chunk as chunk;
use astersql_util_execdetails::execdetails::RURuntimeStats;

#[derive(Default)]
/// 记录被分析子执行器 Open/Next/Close 调用次数。
struct AnalyzeState {
    opens: usize,
    nexts: usize,
    closes: usize,
}

/// 可配置失败点的伪子执行器，实现 `ExplainAnalyzeExecutor`。
struct AnalyzeExecutor {
    state: Arc<Mutex<AnalyzeState>>,
    emitted: bool,
    panic_next: bool,
    fail_next: bool,
    fail_close: bool,
}

impl ExplainAnalyzeExecutor for AnalyzeExecutor {
    fn Open(&mut self, _ctx: ExplainContext) -> Result<(), errors::SharedError> {
        self.state.lock().unwrap().opens += 1;
        Ok(())
    }

    fn Next(
        &mut self,
        _ctx: ExplainContext,
        output: &mut chunk::Chunk,
    ) -> Result<(), errors::SharedError> {
        self.state.lock().unwrap().nexts += 1;
        if self.panic_next {
            panic!("next panic");
        }
        if self.fail_next {
            return Err(errors::New("next error"));
        }
        if !self.emitted {
            output.AppendString(0, "drained");
            self.emitted = true;
        }
        Ok(())
    }

    fn Close(&mut self) -> Result<(), errors::SharedError> {
        self.state.lock().unwrap().closes += 1;
        if self.fail_close {
            Err(errors::New("close error"))
        } else {
            Ok(())
        }
    }

    fn NewCacheChunk(&mut self) -> Box<chunk::Chunk> {
        chunk::NewChunkWithCapacity(vec![FieldType::default()], 2)
    }

    fn SchemaLen(&mut self) -> usize {
        1
    }

    fn ForeignKeyTrigger(&mut self) -> Option<&mut dyn WithForeignKeyTrigger> {
        None
    }
}

/// 记录计划渲染次数。
struct PlanState {
    renders: usize,
}

/// 固定返回若干行的伪执行计划，用于分页断言。
struct AnalyzePlan {
    state: Arc<Mutex<PlanState>>,
    rows: Vec<Vec<String>>,
}

impl ExplainPlan for AnalyzePlan {
    fn Analyze(&self) -> bool {
        true
    }

    fn TargetPlanID(&self) -> Option<i32> {
        None
    }

    fn RenderResult(&mut self) -> Result<(), errors::SharedError> {
        self.state.lock().unwrap().renders += 1;
        Ok(())
    }

    fn Rows(&self) -> Vec<Vec<String>> {
        self.rows.clone()
    }
}

/// 测试用运行时：限制 chunk 大小为 2，便于验证分页。
struct Runtime;

impl ExplainRuntime for Runtime {
    fn MaxChunkSize(&self) -> usize {
        2
    }

    fn MemoryDebugHandler(&self, _ctx: ExplainContext) -> Option<MemoryDebugModeHandler> {
        None
    }

    fn BuildRURuntimeStats(&mut self, _ctx: &ExplainContext) -> Option<RURuntimeStats> {
        None
    }

    fn RegisterRURuntimeStats(&mut self, _target_plan_id: i32, _stats: RURuntimeStats) {}
}

/// 构造带分析子执行器与固定计划行的 `ExplainExec`。
fn explain_executor(
    analyze_state: Arc<Mutex<AnalyzeState>>,
    plan_state: Arc<Mutex<PlanState>>,
    panic_next: bool,
    fail_next: bool,
    fail_close: bool,
) -> ExplainExec {
    ExplainExec {
        runtime: Box::new(Runtime),
        explain: Box::new(AnalyzePlan {
            state: plan_state,
            rows: vec![
                vec!["r1".to_owned()],
                vec!["r2".to_owned()],
                vec!["r3".to_owned()],
            ],
        }),
        analyzeExec: Some(Box::new(AnalyzeExecutor {
            state: analyze_state,
            emitted: false,
            panic_next,
            fail_next,
            fail_close,
        })),
        executed: false,
        rows: None,
        cursor: 0,
    }
}

#[test]
/// 验证：分析子执行器先被排空，再按 MaxChunkSize 分页输出渲染行。
fn explain_analyze_opens_drains_closes_then_pages_rendered_rows() {
    let analyze_state = Arc::new(Mutex::new(AnalyzeState::default()));
    let plan_state = Arc::new(Mutex::new(PlanState { renders: 0 }));
    let mut executor = explain_executor(
        Arc::clone(&analyze_state),
        Arc::clone(&plan_state),
        false,
        false,
        false,
    );
    executor.Open(ExplainContext::default()).unwrap();
    let mut output = chunk::NewChunkWithCapacity(vec![FieldType::default()], 2);

    executor
        .Next(ExplainContext::default(), output.as_mut())
        .unwrap();
    assert_eq!(output.NumRows(), 2);
    executor
        .Next(ExplainContext::default(), output.as_mut())
        .unwrap();
    assert_eq!(output.NumRows(), 1);
    executor
        .Next(ExplainContext::default(), output.as_mut())
        .unwrap();
    assert_eq!(output.NumRows(), 0);

    let state = analyze_state.lock().unwrap();
    assert_eq!((state.opens, state.nexts, state.closes), (1, 2, 1));
    assert_eq!(plan_state.lock().unwrap().renders, 1);
}

#[test]
/// 验证：Next 与 Close 均失败时错误串拼接两者，且不触发计划渲染。
fn explain_analyze_preserves_next_error_before_close_error() {
    let analyze_state = Arc::new(Mutex::new(AnalyzeState::default()));
    let plan_state = Arc::new(Mutex::new(PlanState { renders: 0 }));
    let mut executor = explain_executor(
        Arc::clone(&analyze_state),
        Arc::clone(&plan_state),
        false,
        true,
        true,
    );
    let error = executor
        .generateExplainInfo(ExplainContext::default())
        .unwrap_err();
    assert_eq!(error.to_string(), "next error, close error");
    assert_eq!(analyze_state.lock().unwrap().closes, 1);
    assert_eq!(plan_state.lock().unwrap().renders, 0);
}

#[test]
/// 与 Go 的 recover/defer 契约一致：Next panic 仍须 Close，且 Close 错误随后返回。
fn explain_analyze_closes_after_next_panic() {
    let analyze_state = Arc::new(Mutex::new(AnalyzeState::default()));
    let plan_state = Arc::new(Mutex::new(PlanState { renders: 0 }));
    let mut executor = explain_executor(
        Arc::clone(&analyze_state),
        Arc::clone(&plan_state),
        true,
        false,
        true,
    );

    let error = executor
        .generateExplainInfo(ExplainContext::default())
        .unwrap_err();

    assert_eq!(error.to_string(), "next panic, close error");
    assert_eq!(analyze_state.lock().unwrap().closes, 1);
    assert_eq!(plan_state.lock().unwrap().renders, 0);
}
