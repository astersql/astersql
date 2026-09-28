// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// GroupExpr 的单元测试：构造与指纹编码。
//
// 对应 Go `group_expr_test.go`。指纹用 Group::ID 而非 Go 的指针地址，
// 编码结构仍为「子数量 + 子 ID + 算子 HashCode」。

// 本文件对应 pkg/planner/memo/group_expr_test.go 的 TestNewGroupExpr /
// TestGroupExprFingerprint。
//
// 指纹编码方式和 Go 略有差异：Go 用 `reflect.ValueOf(childGroup).Pointer()`
// 把子 Group 的裸指针地址编码进指纹；Rust 版改用 `Group::ID()`（一个稳定的
// `u64` 自增序号，见 `group.rs` 的 `NEXT_GROUP_ID`），语义上同样是「子 Group 的
// 稳定身份标识」，但取值来源不同，没法照抄 Go 的具体字节。这里按当前生产实现
// 校验同样的编码结构：2 字节子节点数量 + 每个子节点 8 字节 ID + `LogicalPlan`
// 自身的 `HashCode()`。

use crate::*;
use astersql_expression::NewSchema;
use astersql_planner_core_operator_logicalop::LogicalLimit;

// 对应 Go TestNewGroupExpr：NewGroupExpr 原样保存传入的算子节点，新表达式没有
// 子节点，也没有任何轮次被标记为已探索。
#[test]
fn new_group_expr_holds_the_plan_node_with_no_children_and_no_explored_marks() {
    let expr = NewGroupExpr(Box::new(LogicalLimit::default()));
    let expr_ref = expr.borrow();
    assert!(expr_ref.ExprNode.as_any().is::<LogicalLimit>());
    assert!(expr_ref.Children.is_empty());
    assert!(!expr_ref.Explored(0));
}

// 对应 Go TestGroupExprFingerprint：指纹 = 子节点数量(2 字节，大端) + 每个子
// Group 的稳定标识(8 字节，大端) + 算子自身的 HashCode。
#[test]
fn group_expr_fingerprint_encodes_child_count_child_group_id_and_plan_hash() {
    let plan = LogicalLimit {
        Count: 3,
        ..LogicalLimit::default()
    };
    let plan_hash = plan.HashCode();
    let expr = NewGroupExpr(Box::new(plan));
    let child_group = NewGroupWithSchema(None, &NewSchema(Vec::new()));
    expr.borrow_mut().SetChildren(vec![child_group.clone()]);

    let mut expected = 1u16.to_be_bytes().to_vec();
    expected.extend_from_slice(&child_group.borrow().ID().to_be_bytes());
    expected.extend_from_slice(&plan_hash);

    assert_eq!(expr.borrow_mut().FingerPrint(), expected);
}
