// Copyright 2026 AsterSQL.

use crate::Memo;
use logicalop::{JoinType, LogicalJoin, LogicalLimit, LogicalPlan};

fn limit() -> logicalop::LogicalPlanRef {
    Box::new(LogicalLimit::default().Init(crate::main_test::context(), 0))
}

fn join() -> logicalop::LogicalPlanRef {
    Box::new(
        LogicalJoin {
            JoinType: JoinType::InnerJoin,
            ..LogicalJoin::default()
        }
        .Init(crate::main_test::context(), 0),
    )
}

#[test]
#[should_panic(expected = "GetChildStatsAndSchema should not be called on join GE")]
fn single_child_accessor_rejects_join_by_operator_type() {
    let mut memo = Memo::NewMemo(&[]);
    let child = memo.NewGroup();
    let expression = memo.NewGroupExpression(join(), vec![child]);

    expression.borrow().GetChildStatsAndSchema();
}

#[test]
#[should_panic(expected = "GetJoinChildStatsAndSchema should not be called on non-join GE")]
fn join_child_accessor_rejects_non_join_by_operator_type() {
    let mut memo = Memo::NewMemo(&[]);
    let left = memo.NewGroup();
    let right = memo.NewGroup();
    let expression = memo.NewGroupExpression(limit(), vec![left, right]);

    expression.borrow().GetJoinChildStatsAndSchema();
}
