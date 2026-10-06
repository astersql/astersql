// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `adapter` 中执行计划（plan）分类辅助函数的单元测试。
//
// 覆盖：
// - `IsFastPlan`：识别可走快速路径的计划（如 PointGet、Projection→TableDual）；
// - `isNoResultPlan`：识别无需返回结果行的计划（如 calculate_no_delay 的 Projection、
//   空 schema 的 Query）。

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, FormatSQL,
    IsFastPlan, PlanInfo, PlanKind, RecordSet, ResultField, SchemaColumn, StatementKind,
    StatementNode, detachedRecordSet, isNoResultPlan, joinRecordSetErrors,
    statementMaximumExecutionTime,
};
use astersql_errors as errors;
use astersql_sessionctx_vardef::QueryLogMaxLen;
use astersql_util_chunk as chunk;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct DetachedTestExecutor {
    closes: Arc<Mutex<usize>>,
    seen_trace_ids: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl ExecExecutor for DetachedTestExecutor {
    fn Open(&mut self) -> AdapterResult {
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        *self.closes.lock().unwrap() += 1;
        Ok(())
    }
    fn Next(&mut self, _output: &mut chunk::Chunk) -> AdapterResult {
        Ok(())
    }
    fn NextWithContext(
        &mut self,
        context: &ExecutionContext,
        _output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.seen_trace_ids
            .lock()
            .unwrap()
            .push(context.trace_id.clone());
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig::default()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        chunk::Chunk::default()
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &[]
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        Vec::new()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _duration: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

#[test]
fn detached_record_set_owns_executor_and_closes_once_without_session() {
    let closes = Arc::new(Mutex::new(0));
    let seen_trace_ids = Arc::new(Mutex::new(Vec::new()));
    let mut result = detachedRecordSet::new(
        vec![ResultField {
            column_name: "a".into(),
            ..ResultField::default()
        }],
        Box::new(DetachedTestExecutor {
            closes: closes.clone(),
            seen_trace_ids: seen_trace_ids.clone(),
        }),
        "select a".into(),
        Some(ExecutionContext {
            trace_id: vec![1, 2, 3],
            ..ExecutionContext::default()
        }),
    );
    assert_eq!(result.Fields()[0].column_name, "a");
    result.Next(&mut chunk::Chunk::default()).unwrap();
    assert_eq!(*seen_trace_ids.lock().unwrap(), vec![vec![1, 2, 3]]);
    result.Close().unwrap();
    result.Close().unwrap();
    assert_eq!(*closes.lock().unwrap(), 1);
    assert!(result.Next(&mut chunk::Chunk::default()).is_err());
}

#[test]
fn record_set_close_joins_all_original_errors_in_order() {
    let first = errors::New("first");
    let second = errors::New("second");
    assert!(joinRecordSetErrors(&[]).is_none());
    let joined = joinRecordSetErrors(&[first.clone(), second.clone()]).unwrap();
    let members = errors::Errors(&joined);
    assert_eq!(members.len(), 2);
    assert!(members[0].ptr_eq(&first));
    assert!(members[1].ptr_eq(&second));
}

struct RestoreQueryLogMaxLen(i32);

impl Drop for RestoreQueryLogMaxLen {
    fn drop(&mut self) {
        QueryLogMaxLen.Store(self.0);
    }
}

#[test]
fn format_sql_respects_go_query_log_byte_limit() {
    let _restore = RestoreQueryLogMaxLen(QueryLogMaxLen.Load());
    QueryLogMaxLen.Store(0);
    assert_eq!(FormatSQL("a\n\tb").to_string(), "a  b");
    let lazy = FormatSQL("aaaaaaaaaaaaaaaaaaaa");
    QueryLogMaxLen.Store(5);
    assert_eq!(
        FormatSQL("aaaaaaaaaaaaaaaaaaaa").to_string(),
        "aaaaa(len:20)"
    );
    assert_eq!(lazy.to_string(), "aaaaa(len:20)");
}

/// 构造指定种类的最小 PlanInfo，供分类断言使用。
fn plan(kind: PlanKind) -> PlanInfo {
    PlanInfo {
        id: 42,
        kind,
        schema: vec![SchemaColumn::default()],
        calculate_no_delay: false,
        projection_child: None,
        encoded: String::new(),
        binary: String::new(),
        hints: String::new(),
    }
}

/// 核对快速计划与无结果计划的判定与执行路径分类一致。
#[test]
fn adapter_fast_and_no_result_plan_classification_matches_execution_paths() {
    assert!(IsFastPlan(&plan(PlanKind::PointGet)));
    assert!(!IsFastPlan(&plan(PlanKind::Query)));
    let mut projection = plan(PlanKind::Projection);
    projection.projection_child = Some(Box::new(plan(PlanKind::TableDual)));
    assert!(IsFastPlan(&projection));
    // calculate_no_delay 的投影计划视为无结果输出
    projection.calculate_no_delay = true;
    assert!(isNoResultPlan(&projection));
    let mut empty = plan(PlanKind::Query);
    empty.schema.clear();
    assert!(isNoResultPlan(&empty));
}

#[test]
fn transactional_dml_uses_dml_timeout_while_select_keeps_select_timeout() {
    let statement = |kind| StatementNode {
        kind,
        original_text: String::new(),
        text: String::new(),
        secure_text: String::new(),
        prepared_text: None,
    };
    assert_eq!(
        statementMaximumExecutionTime(
            &plan(PlanKind::Insert),
            &statement(StatementKind::Insert),
            30_000,
            60_000,
        ),
        60_000
    );
    assert_eq!(
        statementMaximumExecutionTime(
            &plan(PlanKind::Query),
            &statement(StatementKind::Select),
            30_000,
            60_000,
        ),
        30_000
    );
    assert_eq!(
        statementMaximumExecutionTime(
            &plan(PlanKind::DDL),
            &statement(StatementKind::DDL),
            30_000,
            60_000,
        ),
        0
    );
}
