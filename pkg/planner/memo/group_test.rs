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

// Group 的单元测试：插入/删除/指纹去重与 BuildKeyInfo 传播。
//
// 对应 Go `group_test.go`。因缺少可编译的 BuildLogicalPlanForTest，
// 改为手工搭建 GroupExpr 树，覆盖与 Go 等价的去重与键信息规则。

// 本文件对应 pkg/planner/memo/group_test.go。Go 版本里 TestGroupFingerPrint /
// TestBuildKeyInfo 都是先 `parser.ParseOneStmt` + `coretestsdk.MockContext()` +
// `infoschema.MockInfoSchema` 跑通完整的 Parse -> Resolve ->
// `plannercore.BuildLogicalPlanForTest` 流程，再对生成的真实 LogicalPlan 树调用
// `Convert2Group`/`group.BuildKeyInfo()`。
//
// AsterSQL 目前还没有一个可编译的 `plannercore::BuildLogicalPlanForTest`（仓库里
// 唯一引用它的几个文件——`pkg/planner/cascades/old/optimize_test.rs` 等——本身
// 测试框架依赖"，调用的 `map!`/`defer_reset_transformation_rules`
// 等宏和函数在本仓库里都不存在，这些文件本身编译不过；不是本任务可以依赖的现成
// 基础设施）。因此这里改为直接用生产的 `Group`/`GroupExpr`/`Convert2Group`/
// `BuildKeyInfo` API，手工搭出与 Go 测试等价的 Group/GroupExpr 树——覆盖的是
// `pkg/planner/memo/group.rs` 本身的指纹去重、`BuildKeyInfo` 单子节点 PK/UK
// 继承、`inherits_max_one_row` 的 Limit/Join/未覆盖算子分支——而不经过尚不存在的
// SQL 全链路。每个测试都在文档注释里点出对应的 Go 场景与生产代码位置。
//
// 已知需要注意的实现细节（生产代码，不在本任务 writes 清单内，未修改）：
// `LogicalLimit`/`LogicalProjection` 都没有覆盖 `impl LogicalPlan for
// LogicalXxx` 的 `HashCode`，落回 `BaseLogicalPlan::HashCode`（纯 `id` 编码，
// 见 base_logical_plan.rs:251）；只有 `LogicalSelection` 真正覆盖了内容相关的
// `HashCode`（logical_selection.rs:59-75、540-541，且用排序后的条件哈希列表，
// 天然对条件顺序不敏感）。所以本文件里任何需要在同一个 Group 内插入多个
// Limit/Projection 的场景，都必须先用 `TestPlanContext` 的 `Init()` 给它们分配
// 互不相同的 `PlanID`，否则会被 `Group::Insert` 的指纹去重误判成同一个表达式。

use crate::*;
use astersql_expression::{Column, NewOne, NewSchema, NewZero, Schema};
use astersql_planner_cascades_pattern as pattern;
use astersql_planner_core_operator_logicalop::{
    JoinType, LogicalJoin, LogicalLimit, LogicalProjection, LogicalSelection, TiKVSingleGather,
};
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};

/// Minimal `base::PlanContext` that only allocates plan IDs, starting from 1 so
/// that an `Init()`-ed node's id never collides with a never-`Init()`-ed node's
/// default `id == 0`. Every other `PlanContext` method is unreachable from
/// `LogicalXxx::Init`, which calls nothing else on the context.
/// 最小 PlanContext：自 1 起分配 PlanID，避免与未 Init 节点的默认 id=0 碰撞。
struct TestPlanContext {
    /// 下一个待分配的计划节点 ID。
    next_id: AtomicI32,
}

impl Default for TestPlanContext {
    fn default() -> Self {
        TestPlanContext {
            next_id: AtomicI32::new(1),
        }
    }
}

impl astersql_planner_core_base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        unimplemented!("not exercised by group tests")
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        unimplemented!("not exercised by group tests")
    }
    fn GetRangerCtx(&self) -> &astersql_planner_core_base::RangerContext<'_> {
        unimplemented!("not exercised by group tests")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        unimplemented!("not exercised by group tests")
    }
    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        unimplemented!("not exercised by group tests")
    }
    fn BuiltinFunctionUsageInc(&self, _scalar_func_sig_name: &str) {}
}

/// 构造共享的测试用 PlanContext。
fn test_ctx() -> astersql_planner_core_base::ContextRef {
    std::sync::Arc::new(TestPlanContext::default())
}

/// 空列 Schema。
fn schema() -> Schema {
    NewSchema(Vec::new())
}

/// 无初始表达式的叶子 Group。
fn leaf_group() -> GroupRef {
    NewGroupWithSchema(None, &schema())
}

// 对应 Go TestNewGroup：新建的 Group 持有初始表达式、登记一个指纹，且没有任何轮次
// 被标记为已探索。
#[test]
fn new_group_holds_initial_expression_and_registers_its_fingerprint() {
    let expr = NewGroupExpr(Box::new(LogicalLimit::default()));
    let g = NewGroupWithSchema(expr.clone(), &schema());

    assert_eq!(g.borrow().Equivalents.len(), 1);
    assert!(Rc::ptr_eq(&g.borrow().Equivalents[0], &expr));
    assert_eq!(g.borrow().Fingerprints.len(), 1);
    assert!(!g.borrow().Explored(0));
}

// 对应 Go TestGroupInsert：向 Group 重新插入同一个（因此指纹相同的）GroupExpr 会
// 被去重拒绝；插入一个指纹确实不同的表达式则会成功。Go 版本通过手改私有字段
// `selfFingerprint` 伪造"指纹不同"，这个字段在 Rust 里对 group_test 模块不可见
// （只对 group_expr.rs 自身可见），这里改用一个真正内容不同的 Limit 达到同样的
// "指纹不同则必然插入成功"效果，不改变被测行为。
#[test]
fn group_insert_dedupes_by_fingerprint_but_accepts_expression_with_different_fingerprint() {
    let ctx = test_ctx();
    let expr = NewGroupExpr(Box::new(LogicalLimit::default().Init(ctx.clone(), 0)));
    let g = NewGroupWithSchema(expr.clone(), &schema());

    assert!(!Group::Insert(&g, expr.clone()));

    let other = NewGroupExpr(Box::new(
        LogicalLimit {
            Count: 1,
            ..LogicalLimit::default()
        }
        .Init(ctx.clone(), 0),
    ));
    assert!(Group::Insert(&g, other));
    assert_eq!(g.borrow().Equivalents.len(), 2);
}

// 对应 Go TestGroupDelete：删除已存在的表达式后等价链表为空；再次删除同一个（已经
// 不存在的）表达式不会报错，也不会改变状态。
#[test]
fn group_delete_removes_expression_and_is_idempotent() {
    let expr = NewGroupExpr(Box::new(LogicalLimit::default()));
    let g = NewGroupWithSchema(expr.clone(), &schema());
    assert_eq!(g.borrow().Equivalents.len(), 1);

    Group::Delete(&g, &expr);
    assert_eq!(g.borrow().Equivalents.len(), 0);

    Group::Delete(&g, &expr);
    assert_eq!(g.borrow().Equivalents.len(), 0);
}

// Go Group.Delete 按指纹找到并移除容器里的表达式，但只会清空调用参数 e 的
// Group 字段；若传入的是一个同指纹的等价对象，被移除的原对象仍保留原归属。
#[test]
fn group_delete_with_equivalent_expression_preserves_removed_expression_owner() {
    let stored = NewGroupExpr(Box::new(LogicalLimit::default()));
    let equivalent = NewGroupExpr(Box::new(LogicalLimit::default()));
    let g = NewGroupWithSchema(stored.clone(), &schema());

    Group::Delete(&g, &equivalent);

    assert!(g.borrow().Equivalents.is_empty());
    assert!(stored.borrow().Group.upgrade().is_some());
    assert!(equivalent.borrow().Group.upgrade().is_none());
}

// 对应 Go TestGroupDeleteAll：先插入 Selection/Limit/Projection 三个不同的等价
// 表达式，确认 GetFirstElem/Exists 都能工作，再 DeleteAll 后全部清空。
#[test]
fn group_delete_all_clears_equivalents_first_elem_and_existence() {
    let ctx = test_ctx();
    let expr = NewGroupExpr(Box::new(
        LogicalSelection {
            Conditions: vec![Box::new(NewOne())],
            ..LogicalSelection::default()
        }
        .Init(ctx.clone(), 0),
    ));
    let g = NewGroupWithSchema(expr.clone(), &schema());
    assert!(Group::Insert(
        &g,
        NewGroupExpr(Box::new(LogicalLimit::default().Init(ctx.clone(), 0)))
    ));
    assert!(Group::Insert(
        &g,
        NewGroupExpr(Box::new(LogicalProjection::default().Init(ctx.clone(), 0)))
    ));
    assert_eq!(g.borrow().Equivalents.len(), 3);
    assert!(
        g.borrow()
            .GetFirstElem(pattern::OperandProjection)
            .is_some()
    );
    assert!(g.borrow().Exists(&expr));

    Group::DeleteAll(&g);
    assert_eq!(g.borrow().Equivalents.len(), 0);
    assert!(
        g.borrow()
            .GetFirstElem(pattern::OperandProjection)
            .is_none()
    );
    assert!(!g.borrow().Exists(&expr));
    // Go DeleteAll replaces the list/maps only; existing GroupExpr.Group remains intact.
    assert!(expr.borrow().Group.upgrade().is_some());
}

// 对应 Go TestGroupExists：初始表达式存在；删除后 Exists 返回 false。
#[test]
fn group_exists_reflects_current_membership() {
    let expr = NewGroupExpr(Box::new(LogicalLimit::default()));
    let g = NewGroupWithSchema(expr.clone(), &schema());
    assert!(g.borrow().Exists(&expr));

    Group::Delete(&g, &expr);
    assert!(!g.borrow().Exists(&expr));
}

// 对应 Go TestGroupFingerPrint 的四个场景，改用手工搭建的 GroupExpr 树（理由见
// 文件头注释）：
// 1. 相同 ExprNode（这里用两个内容/children 都相同、都未 Init 的 Projection 模拟，
//    因为二者退化到同一个 `id == 0` 的 HashCode，具有完全相同的指纹语义）加相同
//    children，插入被去重拒绝；
// 2. 相同 ExprNode 但 children 换成一个新 Group（即使内容相同，子 Group 的稳定
//    ID 不同），指纹因此不同，插入成功；
// 3. 换成不同的 ExprNode（Init 出一个真正不同 PlanID 的 Limit），指纹不同，插入
//    成功；
// 4. 两个条件相同但顺序相反的 Selection——`LogicalSelection::HashCode` 会对条件
//    哈希列表排序（logical_selection.rs:71），顺序无关，指纹相同，第二次插入被
//    去重拒绝。
#[test]
fn group_fingerprint_distinguishes_children_and_node_identity_but_ignores_condition_order() {
    let ctx = test_ctx();
    let leaf = leaf_group();

    let base_expr = NewGroupExpr(Box::new(LogicalProjection::default()));
    base_expr.borrow_mut().SetChildren(vec![leaf.clone()]);
    let group1 = NewGroupWithSchema(base_expr, &schema());

    // 场景 1：相同 ExprNode（同样未 Init、同样没有条件的 Projection）+ 相同 children。
    let same_node_same_children = NewGroupExpr(Box::new(LogicalProjection::default()));
    same_node_same_children
        .borrow_mut()
        .SetChildren(vec![leaf.clone()]);
    assert!(!Group::Insert(&group1, same_node_same_children));
    assert_eq!(group1.borrow().Equivalents.len(), 1);

    // 场景 2：相同 ExprNode，但 children 换成一个新 Group（内容相同，Group 身份不同）。
    let different_child_group = leaf_group();
    let same_node_different_children = NewGroupExpr(Box::new(LogicalProjection::default()));
    same_node_different_children
        .borrow_mut()
        .SetChildren(vec![different_child_group]);
    assert!(Group::Insert(&group1, same_node_different_children));
    assert_eq!(group1.borrow().Equivalents.len(), 2);

    // 场景 3：不同的 ExprNode（真正 Init 出一个不同 PlanID 的 Limit），相同 children。
    let different_node = NewGroupExpr(Box::new(LogicalLimit::default().Init(ctx.clone(), 0)));
    different_node.borrow_mut().SetChildren(vec![leaf.clone()]);
    assert!(Group::Insert(&group1, different_node));
    assert_eq!(group1.borrow().Equivalents.len(), 3);

    // 场景 4：两个条件相同但顺序相反的 Selection，指纹应当相同（排序后的哈希列表）。
    let cond_a = Box::new(NewOne());
    let cond_b = Box::new(NewZero());
    let selection_forward = NewGroupExpr(Box::new(LogicalSelection {
        Conditions: vec![cond_a.clone(), cond_b.clone()],
        ..LogicalSelection::default()
    }));
    selection_forward
        .borrow_mut()
        .SetChildren(vec![leaf.clone()]);
    assert!(Group::Insert(&group1, selection_forward));
    assert_eq!(group1.borrow().Equivalents.len(), 4);

    let selection_reversed = NewGroupExpr(Box::new(LogicalSelection {
        Conditions: vec![cond_b, cond_a],
        ..LogicalSelection::default()
    }));
    selection_reversed.borrow_mut().SetChildren(vec![leaf]);
    assert!(!Group::Insert(&group1, selection_reversed));
    assert_eq!(group1.borrow().Equivalents.len(), 4);
}

// 对应 Go TestGroupGetFirstElem：按 operand 查询时返回同类表达式里最早插入的那个，
// OperandAny 返回链表首项。5 个表达式两两之间都必须有不同的指纹（否则会被静默
// 去重）；两个 Limit 使用不同 Count，以保持其 Go 语义哈希不同。
#[test]
fn group_get_first_elem_returns_first_inserted_expression_per_operand() {
    let ctx = test_ctx();
    let expr0 = NewGroupExpr(Box::new(LogicalProjection::default().Init(ctx.clone(), 0)));
    let expr1 = NewGroupExpr(Box::new(
        LogicalLimit {
            Count: 1,
            ..LogicalLimit::default()
        }
        .Init(ctx.clone(), 0),
    ));
    let expr2 = NewGroupExpr(Box::new(LogicalProjection::default().Init(ctx.clone(), 0)));
    let expr3 = NewGroupExpr(Box::new(
        LogicalLimit {
            Count: 2,
            ..LogicalLimit::default()
        }
        .Init(ctx.clone(), 0),
    ));
    let expr4 = NewGroupExpr(Box::new(LogicalProjection::default().Init(ctx.clone(), 0)));

    let g = NewGroupWithSchema(expr0.clone(), &schema());
    assert!(Group::Insert(&g, expr1.clone()));
    assert!(Group::Insert(&g, expr2));
    assert!(Group::Insert(&g, expr3));
    assert!(Group::Insert(&g, expr4));

    let first_projection = g.borrow().GetFirstElem(pattern::OperandProjection).unwrap();
    assert!(Rc::ptr_eq(
        &g.borrow().Equivalents[first_projection],
        &expr0
    ));
    let first_limit = g.borrow().GetFirstElem(pattern::OperandLimit).unwrap();
    assert!(Rc::ptr_eq(&g.borrow().Equivalents[first_limit], &expr1));
    let first_any = g.borrow().GetFirstElem(pattern::OperandAny).unwrap();
    assert!(Rc::ptr_eq(&g.borrow().Equivalents[first_any], &expr0));
}

// 对应 Go TestFirstElemAfterDelete：删除同类 operand 的首项后，GetFirstElem 应
// 切换到后续同类表达式；全部删除后返回 None。
#[test]
fn first_elem_after_delete_switches_to_next_expression_with_same_operand() {
    let ctx = test_ctx();
    let old_expr = NewGroupExpr(Box::new(
        LogicalLimit {
            Count: 10,
            ..LogicalLimit::default()
        }
        .Init(ctx.clone(), 0),
    ));
    let g = NewGroupWithSchema(old_expr.clone(), &schema());
    let new_expr = NewGroupExpr(Box::new(
        LogicalLimit {
            Count: 20,
            ..LogicalLimit::default()
        }
        .Init(ctx.clone(), 0),
    ));
    assert!(Group::Insert(&g, new_expr.clone()));

    let first = g.borrow().GetFirstElem(pattern::OperandLimit).unwrap();
    assert!(Rc::ptr_eq(&g.borrow().Equivalents[first], &old_expr));

    Group::Delete(&g, &old_expr);
    let first = g.borrow().GetFirstElem(pattern::OperandLimit).unwrap();
    assert!(Rc::ptr_eq(&g.borrow().Equivalents[first], &new_expr));

    Group::Delete(&g, &new_expr);
    assert!(g.borrow().GetFirstElem(pattern::OperandLimit).is_none());
}

/// 构造带单列主键/唯一键集合的 Schema，供 BuildKeyInfo 继承测试使用。
fn schema_with_pk(column: Column) -> Schema {
    let mut schema = NewSchema(vec![column.clone()]);
    schema.PKOrUK = vec![vec![column]];
    schema
}

// 对应 Go TestBuildKeyInfo 的核心传播规则（对应 group.rs 的 `BuildKeyInfo` 自由
// 函数与 `inherits_max_one_row`，见文件头注释）：
// - case 1：单孩子时父 Group 直接继承孩子 schema 的 PKOrUK（group.rs:261-263）；
// - case 2/4：Limit/Selection 等在 `inherits_max_one_row` 名单里的算子，MaxOneRow
//   直接取第一个孩子（group.rs:283-293）；
// - Join 只有两个孩子都是 MaxOneRow 才继承（294 行），单孩子或某一侧非
//   MaxOneRow 都不继承；
// - 不在名单里的算子（如 TiKVSingleGather）即使孩子是 MaxOneRow 也不会继承。
#[test]
fn build_key_info_inherits_pk_and_max_one_row_per_operand_rules() {
    let ctx = test_ctx();
    let mut column = Column::default();
    column.UniqueID = 1;

    // case 1：单孩子继承 PKOrUK。`NewGroupWithSchema` 只会把传入 schema 的
    // `Columns` 拷进 `Prop.Schema`（见 group.rs 的 `NewGroupWithSchema`），
    // `PKOrUK`/`NullableUK` 不会跟着抄一份，所以这里手动把 PKOrUK 补回去，模拟
    // "孩子已经推导出主键" 之后的状态。
    let child_with_pk = NewGroupWithSchema(None, &schema_with_pk(column.clone()));
    child_with_pk
        .borrow_mut()
        .Prop
        .Schema
        .as_mut()
        .unwrap()
        .PKOrUK = vec![vec![column.clone()]];
    let selection_expr = NewGroupExpr(Box::new(LogicalSelection::default().Init(ctx.clone(), 0)));
    selection_expr
        .borrow_mut()
        .SetChildren(vec![child_with_pk.clone()]);
    let selection_group = NewGroupWithSchema(selection_expr, &schema_with_pk(column.clone()));
    BuildKeyInfo(&selection_group);
    assert_eq!(
        selection_group
            .borrow()
            .Prop
            .Schema
            .as_ref()
            .unwrap()
            .PKOrUK
            .len(),
        1
    );

    // case 2：Selection 的 MaxOneRow 直接继承唯一孩子。
    child_with_pk.borrow_mut().Prop.MaxOneRow = true;
    let selection_expr2 = NewGroupExpr(Box::new(LogicalSelection::default().Init(ctx.clone(), 0)));
    selection_expr2
        .borrow_mut()
        .SetChildren(vec![child_with_pk.clone()]);
    let selection_group2 = NewGroupWithSchema(selection_expr2, &schema());
    BuildKeyInfo(&selection_group2);
    assert!(selection_group2.borrow().Prop.MaxOneRow);

    // case 3：Join 只有两个孩子都 MaxOneRow 才继承；这里只有一个孩子是。
    let other_child = leaf_group();
    let join_expr = NewGroupExpr(Box::new(LogicalJoin::default().Init(ctx.clone(), 0)));
    join_expr
        .borrow_mut()
        .SetChildren(vec![child_with_pk.clone(), other_child]);
    let join_group_ = NewGroupWithSchema(join_expr, &schema());
    BuildKeyInfo(&join_group_);
    assert!(!join_group_.borrow().Prop.MaxOneRow);

    // Semi joins return only rows from the left input, so Go's
    // HasMaxOneRow uses the left child's guarantee and ignores the right.
    let semi_expr = NewGroupExpr(Box::new(
        LogicalJoin {
            JoinType: JoinType::SemiJoin,
            ..LogicalJoin::default()
        }
        .Init(ctx.clone(), 0),
    ));
    semi_expr
        .borrow_mut()
        .SetChildren(vec![child_with_pk.clone(), leaf_group()]);
    let semi_group = NewGroupWithSchema(semi_expr, &schema());
    BuildKeyInfo(&semi_group);
    assert!(semi_group.borrow().Prop.MaxOneRow);

    // case 4：不在继承名单里的算子（TiKVSingleGather）即使孩子是 MaxOneRow 也不继承。
    let gather_expr = NewGroupExpr(Box::new(TiKVSingleGather::default().Init(ctx.clone(), 0)));
    gather_expr
        .borrow_mut()
        .SetChildren(vec![child_with_pk.clone()]);
    let gather_group = NewGroupWithSchema(gather_expr, &schema());
    BuildKeyInfo(&gather_group);
    assert!(!gather_group.borrow().Prop.MaxOneRow);
}

// Go 的 int 位图对超出机器字宽的动态左移截断为 0，因此越界轮次是 no-op。
#[test]
fn explore_mark_out_of_range_round_is_a_no_op() {
    let mut mark = ExploreMark::default();

    mark.SetExplored(64);
    assert!(!mark.Explored(64));
    mark.SetUnexplored(64);
    assert_eq!(mark, ExploreMark::default());
}
