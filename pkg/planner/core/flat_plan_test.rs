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

use crate::{
    ExplainFlatPlanInRowFormat, FlattenPhysicalPlan, JoinType, OperatorLabel, PlanKind, PlanNode,
};

fn node(id: i32, kind: PlanKind, children: Vec<PlanNode>) -> PlanNode {
    PlanNode::New(id, kind, children)
}

#[test]
fn get_select_plan_skips_dml_prefix_and_foreign_key_suffix() {
    let select = node(
        2,
        PlanKind::Projection,
        vec![node(3, PlanKind::TableScan { table: "t".into() }, vec![])],
    );
    let insert = node(
        1,
        PlanKind::Insert,
        vec![select, node(4, PlanKind::FKCheck, vec![])],
    );
    let flat = FlattenPhysicalPlan(Some(&insert), false).unwrap();

    let (select, offset) = flat.GetSelectPlan();
    assert_eq!(offset, 1);
    assert_eq!(select.len(), 2);
    assert!(matches!(select[0].Origin.kind, PlanKind::Projection));
    assert!(matches!(select[1].Origin.kind, PlanKind::TableScan { .. }));
}

#[test]
fn operator_labels_match_go_string_contract_without_double_wrapping() {
    assert_eq!(OperatorLabel::BuildSide.to_string(), "(Build)");
    assert_eq!(OperatorLabel::ProbeSide.to_string(), "(Probe)");

    let join = node(
        1,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![],
        },
        vec![
            node(2, PlanKind::Dual, vec![]),
            node(3, PlanKind::Dual, vec![]),
        ],
    );
    let flat = FlattenPhysicalPlan(Some(&join), false).unwrap();
    let rows = ExplainFlatPlanInRowFormat(&flat, "row", false);
    assert!(rows[1][0].ends_with("(Probe)"));
    assert!(!rows[1][0].contains("((Probe))"));
}

#[test]
fn right_outer_merge_join_builds_on_left_like_go() {
    let join = node(
        1,
        PlanKind::MergeJoin {
            join_type: JoinType::RightOuterJoin,
            keys: vec![],
        },
        vec![
            node(2, PlanKind::Dual, vec![]),
            node(3, PlanKind::Dual, vec![]),
        ],
    );
    let flat = FlattenPhysicalPlan(Some(&join), false).unwrap();

    assert_eq!(flat.Main[1].Label, OperatorLabel::BuildSide);
    assert_eq!(flat.Main[2].Label, OperatorLabel::ProbeSide);
    assert!(!flat.Main[0].NeedReverseDriverSide);
}

#[test]
fn physical_children_inherit_root_status_like_go() {
    let root = node(
        1,
        PlanKind::Projection,
        vec![node(2, PlanKind::Dual, vec![])],
    );
    let flat = FlattenPhysicalPlan(Some(&root), false).unwrap();

    assert!(flat.Main[0].IsRoot);
    assert!(flat.Main[1].IsRoot);
}

use base::Plan;
use base_dependency as base;
use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};
struct TypedPlanTestContext(
    AtomicI32,
    base::BuiltinFunctionUsageCounter,
    variable_dependency::session::SessionVars,
);

impl base::PlanContext for TypedPlanTestContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        &self.2
    }
    fn GetExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        unreachable!()
    }
    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        unreachable!()
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        unreachable!()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        unreachable!()
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

#[test]
fn go_merge_187_analyze_and_commit_flat_leaves() {
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        Default::default(),
        Default::default(),
    ));
    let analyze = crate::RuntimeAnalyze::New(context.clone(), Default::default());
    let tree = crate::FlattenTypedPhysicalPlan(&analyze).expect("ANALYZE must reach the RU walker");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].Origin.id(), analyze.id());
    assert!(tree[0].IsRoot);
    assert!(tree[0].ChildrenIdx.is_empty());
    let commit = crate::RuntimeSimple::New(
        context,
        crate::ast::NodeRef::new(Box::new(crate::ast::CommitStmt::default())),
    );
    let tree = crate::FlattenTypedPhysicalPlan(&commit).expect("COMMIT must reach the RU walker");
    assert_eq!(tree.len(), 1);
    assert!(tree[0].IsRoot);
    assert!(tree[0].ChildrenIdx.is_empty());
}
