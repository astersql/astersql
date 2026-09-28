// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `Detach` 递归拆离执行器树的单元测试。
//
// 覆盖：成功拆离时重置副本状态且不修改原树；子节点不可拆离时整体失败（原子性）。
use crate::detach::{
    Detach, DetachableBuildPbContext, DetachableDistSqlContext, DetachableEvalContext,
    DetachableExecutor, DetachableExprContext, DetachableFilter, DetachableRangeContext,
    IndexLookUpExecutorContext, OptionalEvalProperties, ProjectionExec, ProjectionExecutorContext,
    SelectionExec, TableReaderExecutor, TableReaderExecutorContext,
};

/// 测试用可拆离执行器：可配置是否允许拆离，以及子树结构。
#[derive(Clone, Debug, Eq, PartialEq)]
struct TestExecutor {
    /// 为 false 时 `detach_shallow` 返回 `None`。
    detachable: bool,
    /// 业务状态；拆离副本应复位为 0。
    state: usize,
    children: Vec<TestExecutor>,
}

impl DetachableExecutor for TestExecutor {
    fn detach_shallow(&self) -> Option<Self> {
        self.detachable.then(|| Self {
            detachable: true,
            state: 0,
            children: Vec::new(),
        })
    }

    fn all_children(&self) -> Vec<Self> {
        self.children.clone()
    }

    fn set_all_children(&mut self, children: Vec<Self>) {
        self.children = children;
    }
}

/// 递归拆离成功：副本状态复位，原树不变。
#[test]
fn detach_recursively_resets_executor_state_without_mutating_original() {
    let original = TestExecutor {
        detachable: true,
        state: 8,
        children: vec![TestExecutor {
            detachable: true,
            state: 5,
            children: Vec::new(),
        }],
    };
    let (detached, ok) = Detach(&original);
    assert!(ok);
    let detached = detached.unwrap();
    assert_eq!(detached.state, 0);
    assert_eq!(detached.children[0].state, 0);
    assert_eq!(original.state, 8);
    assert_eq!(original.children[0].state, 5);
}

/// 子节点不可拆离时整体失败，且不留下半拆离副作用。
#[test]
fn detach_is_atomic_when_a_child_cannot_detach() {
    let original = TestExecutor {
        detachable: true,
        state: 3,
        children: vec![TestExecutor {
            detachable: false,
            state: 9,
            children: Vec::new(),
        }],
    };
    assert_eq!(Detach(&original), (None, false));
    assert_eq!(original.children[0].state, 9);
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExprContext(Option<u8>);

impl DetachableExprContext for ExprContext {
    fn into_static(&self) -> Option<Self> {
        self.0.map(|value| Self(Some(value + 10)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DistSqlContext(u8);

impl DetachableDistSqlContext for DistSqlContext {
    fn detach(&self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RangeContext(u8);

impl DetachableRangeContext<ExprContext> for RangeContext {
    fn detach(&self, expression_context: &ExprContext) -> Self {
        Self(self.0 + expression_context.0.unwrap())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildPbContext(u8);

impl DetachableBuildPbContext<ExprContext> for BuildPbContext {
    fn detach(&self, expression_context: &ExprContext) -> Self {
        Self(self.0 + expression_context.0.unwrap())
    }
}

fn reader_context(
    expression_context: ExprContext,
) -> TableReaderExecutorContext<ExprContext, DistSqlContext, RangeContext, BuildPbContext> {
    TableReaderExecutorContext {
        expression_context,
        distsql_context: DistSqlContext(2),
        range_context: RangeContext(3),
        build_pb_context: BuildPbContext(4),
    }
}

#[test]
fn reader_context_detaches_every_session_bound_component() {
    let original = reader_context(ExprContext(Some(5)));
    let detached = original.Detach();

    assert_eq!(detached.expression_context, ExprContext(Some(15)));
    assert_eq!(detached.distsql_context, DistSqlContext(3));
    assert_eq!(detached.range_context, RangeContext(18));
    assert_eq!(detached.build_pb_context, BuildPbContext(19));
    assert_eq!(original.expression_context, ExprContext(Some(5)));
    assert_eq!(original.distsql_context, DistSqlContext(2));
    assert_eq!(original.range_context, RangeContext(3));
    assert_eq!(original.build_pb_context, BuildPbContext(4));
}

#[test]
fn reader_context_preserves_non_session_context_without_detaching_dependencies() {
    let original = reader_context(ExprContext(None));
    let detached = original.Detach();
    assert_eq!(detached.expression_context, ExprContext(None));
    assert_eq!(detached.distsql_context, DistSqlContext(2));
    assert_eq!(detached.range_context, RangeContext(3));
    assert_eq!(detached.build_pb_context, BuildPbContext(4));
}

#[test]
fn index_lookup_context_preserves_go_return_value() {
    let original = IndexLookUpExecutorContext {
        table_reader_context: reader_context(ExprContext(Some(5))),
    };
    let detached = original.Detach();

    assert_eq!(
        detached.table_reader_context.expression_context,
        ExprContext(Some(5))
    );
    assert_eq!(
        detached.table_reader_context.distsql_context,
        DistSqlContext(2)
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EvalContext(Option<u8>);

impl DetachableEvalContext for EvalContext {
    fn into_static(&self) -> Option<Self> {
        self.0.map(|value| Self(Some(value + 10)))
    }
}

#[test]
fn projection_and_selection_contexts_staticize_only_session_evaluation_contexts() {
    let session = ProjectionExecutorContext {
        evaluation_context: EvalContext(Some(4)),
    };
    assert_eq!(session.Detach().evaluation_context, EvalContext(Some(14)));

    let non_session = ProjectionExecutorContext {
        evaluation_context: EvalContext(None),
    };
    assert_eq!(non_session.Detach().evaluation_context, EvalContext(None));
}

#[derive(Clone)]
struct OptionalProperties(bool);

impl OptionalEvalProperties for OptionalProperties {
    fn is_empty(&self) -> bool {
        self.0
    }
}

#[derive(Clone)]
struct Filter(bool);

impl DetachableFilter for Filter {
    fn required_optional_properties_are_empty(&self) -> bool {
        self.0
    }
}

#[test]
fn executor_shells_detach_context_and_reject_required_optional_properties() {
    let table_reader = TableReaderExecutor { context: 3_u8 };
    let (detached, ok) = table_reader.DetachWith(|context| context + 10);
    assert!(ok);
    assert_eq!(detached.unwrap().context, 13);

    let projection = ProjectionExec {
        context: 4_u8,
        required_optional_properties: OptionalProperties(true),
    };
    let (detached, ok) = projection.DetachWith(|context| context + 10);
    assert!(ok);
    assert_eq!(detached.unwrap().context, 14);

    let projection = ProjectionExec {
        context: 4_u8,
        required_optional_properties: OptionalProperties(false),
    };
    assert!(!projection.DetachWith(|context| context + 10).1);

    let selection = SelectionExec {
        context: 5_u8,
        filters: vec![Filter(true), Filter(true)],
    };
    let (detached, ok) = selection.DetachWith(|context| context + 10);
    assert!(ok);
    assert_eq!(detached.unwrap().context, 15);

    let selection = SelectionExec {
        context: 5_u8,
        filters: vec![Filter(true), Filter(false)],
    };
    assert!(!selection.DetachWith(|context| context + 10).1);
}
