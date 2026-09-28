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

// ExprIter 的单元测试：Pattern 匹配、组合枚举与按 EngineType 过滤。
//
// 对应 Go `expr_iterator_test.go`。因 mock PlanContext 限制，用最小
// `TestPlanContext` 分配互异 PlanID，避免 Limit/Projection 指纹碰撞。

// 本文件对应 pkg/planner/memo/expr_iterator_test.go。Go 版本用
// `coretestsdk.MockContext()` 起一个真实 mock session/domain 只是为了满足
// `logicalop.XXX{}.Init(ctx, 0)` 的签名，测试本身完全不依赖 SQL/统计信息。
// `pkg/planner/util/coretestsdk` 现在的 Rust mock（见 mock.rs）只是一个不含
// PlanContext 能力的数据结构体，无法喂给需要真实 `base::ContextRef` 的
// `Init`。这里改用一个只实现 `alloc_plan_id` 的最小 `TestPlanContext`
// （其它 `base::PlanContext` 方法本测试路径不会触达，`unimplemented!()` 即可），
// 满足 `LogicalLimit`/`LogicalProjection`/`LogicalSelection` 各自 `Init` 的签名。
// 这一步是必需的，不是简化：`LogicalSelection::HashCode` 按 `Conditions` 编码内容
// （`logical_selection.rs:59-75`，且被接到了 `impl LogicalPlan for
// LogicalSelection` 的 `HashCode`，见同文件 540-541 行），但
// `LogicalLimit`/`LogicalProjection` 各自的同名方法只是"孤立"的内在方法，并没有
// 覆盖 `impl LogicalPlan for LogicalLimit/LogicalProjection` 的 `HashCode`——
// trait 方法用的是 `BaseLogicalPlan` 的默认实现（纯 `id` 编码，
// `base_logical_plan.rs:251`）。也就是说，若不 `Init()`，同一个 Group 里两个不同
// `Count`/`Exprs` 的 Limit/Projection 会因为都是 `id == 0` 而被
// `Group::Insert` 的指纹去重判成"同一个表达式"，插入会静默失败——这是生产代码里
// Limit/Projection 尚未接上内容相关 `HashCode` 的既有差距（不在本任务 writes
// 清单内，未修改源码），测试这里通过给每个节点分配互不相同的 `PlanID` 来规避，
// 同时仍然按 Go 原文塞入不同的 `Count`/`NewOne()`/`NewZero()`/`NewNull()` 保留
// 语义上的内容差异。

use crate::*;
use astersql_expression::{NewNull, NewOne, NewSchema, NewZero};
use astersql_planner_cascades_pattern as pattern;
use astersql_planner_core_operator_logicalop::{
    Expression, LogicalJoin, LogicalLimit, LogicalPlanRef, LogicalProjection, LogicalSelection,
    TiKVSingleGather,
};
use std::sync::atomic::{AtomicI32, Ordering};

/// Minimal `base::PlanContext` that only allocates plan IDs; every other method
/// is unreachable from `LogicalXxx::Init`, which calls nothing else on the context.
/// 最小 PlanContext：仅分配 PlanID；Init 路径不会触达其它方法。
#[derive(Default)]
struct TestPlanContext {
    /// 下一个待分配的计划节点 ID。
    next_id: AtomicI32,
}

impl astersql_planner_core_base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        unimplemented!("not exercised by expr_iterator tests")
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        unimplemented!("not exercised by expr_iterator tests")
    }
    fn GetRangerCtx(&self) -> &astersql_planner_core_base::RangerContext<'_> {
        unimplemented!("not exercised by expr_iterator tests")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        unimplemented!("not exercised by expr_iterator tests")
    }
    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        unimplemented!("not exercised by expr_iterator tests")
    }
    fn BuiltinFunctionUsageInc(&self, _scalar_func_sig_name: &str) {}
}

/// 构造共享的测试用 PlanContext。
fn test_ctx() -> astersql_planner_core_base::ContextRef {
    std::sync::Arc::new(TestPlanContext::default())
}

/// 空列 Schema，满足 NewGroupWithSchema 入参。
fn schema() -> astersql_expression::Schema {
    NewSchema(Vec::new())
}

/// 构造并 Init 一个 LogicalSelection。
fn selection(
    ctx: &astersql_planner_core_base::ContextRef,
    conditions: Vec<Expression>,
) -> LogicalPlanRef {
    Box::new(
        LogicalSelection {
            Conditions: conditions,
            ..LogicalSelection::default()
        }
        .Init(ctx.clone(), 0),
    )
}
/// 构造并 Init 一个 LogicalLimit。
fn limit(ctx: &astersql_planner_core_base::ContextRef, count: u64) -> LogicalPlanRef {
    Box::new(
        LogicalLimit {
            Count: count,
            ..LogicalLimit::default()
        }
        .Init(ctx.clone(), 0),
    )
}
/// 构造并 Init 一个 LogicalProjection。
fn projection(
    ctx: &astersql_planner_core_base::ContextRef,
    exprs: Vec<Expression>,
) -> LogicalPlanRef {
    Box::new(
        LogicalProjection {
            Exprs: exprs,
            ..LogicalProjection::default()
        }
        .Init(ctx.clone(), 0),
    )
}
/// 构造并 Init 一个 LogicalJoin。
fn join(ctx: &astersql_planner_core_base::ContextRef) -> LogicalPlanRef {
    Box::new(LogicalJoin::default().Init(ctx.clone(), 0))
}
/// 构造并 Init 一个 TiKVSingleGather（单点 Gather）。
fn tikv_single_gather(ctx: &astersql_planner_core_base::ContextRef) -> LogicalPlanRef {
    Box::new(TiKVSingleGather::default().Init(ctx.clone(), 0))
}

/// 用 Join 表达式挂上给定子 Group，建成 Join Group。
fn join_group(ctx: &astersql_planner_core_base::ContextRef, children: Vec<GroupRef>) -> GroupRef {
    let expr = NewGroupExpr(join(ctx));
    expr.borrow_mut().SetChildren(children);
    NewGroupWithSchema(expr, &schema())
}

// 对应 Go TestNewExprIterFromGroupElem：join(projection, selection) 模式能从 join
// group element 直接构造出两棵匹配的子迭代器，各自绑定到 g0/g1 里第一个满足对应
// operand 的表达式。
#[test]
fn new_expr_iter_from_group_elem_binds_children_to_first_matching_operand() {
    let ctx = test_ctx();
    let g0 = NewGroupWithSchema(NewGroupExpr(selection(&ctx, vec![])), &schema());
    assert!(Group::Insert(&g0, NewGroupExpr(limit(&ctx, 1))));
    assert!(Group::Insert(&g0, NewGroupExpr(projection(&ctx, vec![]))));
    assert!(Group::Insert(&g0, NewGroupExpr(limit(&ctx, 2))));

    let g1 = NewGroupWithSchema(
        NewGroupExpr(selection(&ctx, vec![Box::new(NewOne())])),
        &schema(),
    );
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 3))));
    assert!(Group::Insert(&g1, NewGroupExpr(projection(&ctx, vec![]))));
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 4))));

    let g2 = join_group(&ctx, vec![g0.clone(), g1.clone()]);

    let pat = pattern::BuildPattern(
        pattern::OperandJoin,
        pattern::EngineAll,
        vec![
            pattern::BuildPattern(pattern::OperandProjection, pattern::EngineAll, Vec::new()),
            pattern::BuildPattern(pattern::OperandSelection, pattern::EngineAll, Vec::new()),
        ],
    );
    let iter = NewExprIterFromGroupElem(&g2, 0, &pat).expect("root join expression should match");

    // Go's constructor binds only Element for the root expression; the root
    // iterator must not enumerate sibling expressions in the same Group.
    assert!(iter.Group.is_none());
    assert_eq!(iter.Element, Some(0));
    assert!(iter.Matched());
    assert_eq!(iter.Pattern.Operand, pattern::OperandJoin);
    assert_eq!(iter.Children.len(), 2);

    assert!(std::rc::Rc::ptr_eq(
        iter.Children[0].Group.as_ref().unwrap(),
        &g0
    ));
    assert_eq!(
        iter.Children[0].Element,
        g0.borrow().GetFirstElem(pattern::OperandProjection)
    );
    assert!(iter.Children[0].Matched());
    assert_eq!(iter.Children[0].Pattern.Operand, pattern::OperandProjection);
    assert!(iter.Children[0].Children.is_empty());

    assert!(std::rc::Rc::ptr_eq(
        iter.Children[1].Group.as_ref().unwrap(),
        &g1
    ));
    assert_eq!(
        iter.Children[1].Element,
        g1.borrow().GetFirstElem(pattern::OperandSelection)
    );
    assert!(iter.Children[1].Matched());
    assert_eq!(iter.Children[1].Pattern.Operand, pattern::OperandSelection);
    assert!(iter.Children[1].Children.is_empty());
}

// 对应 Go TestExprIterNext：g0 有 3 个 Projection、g1 有 3 个 Selection，
// join(projection, selection) 模式应枚举 3*3=9 种组合，每次 Next() 都保持
// Join/Projection/Selection 的 operand 不变。
#[test]
fn expr_iter_next_enumerates_all_combinations_of_matching_children() {
    let ctx = test_ctx();
    let g0 = NewGroupWithSchema(
        NewGroupExpr(projection(&ctx, vec![Box::new(NewZero())])),
        &schema(),
    );
    assert!(Group::Insert(&g0, NewGroupExpr(limit(&ctx, 1))));
    assert!(Group::Insert(
        &g0,
        NewGroupExpr(projection(&ctx, vec![Box::new(NewOne())]))
    ));
    assert!(Group::Insert(&g0, NewGroupExpr(limit(&ctx, 2))));
    assert!(Group::Insert(
        &g0,
        NewGroupExpr(projection(&ctx, vec![Box::new(NewNull())]))
    ));

    let g1 = NewGroupWithSchema(
        NewGroupExpr(selection(&ctx, vec![Box::new(NewNull())])),
        &schema(),
    );
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 3))));
    assert!(Group::Insert(
        &g1,
        NewGroupExpr(selection(&ctx, vec![Box::new(NewOne())]))
    ));
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 4))));
    assert!(Group::Insert(
        &g1,
        NewGroupExpr(selection(&ctx, vec![Box::new(NewZero())]))
    ));

    let g2 = join_group(&ctx, vec![g0.clone(), g1.clone()]);

    let pat = pattern::BuildPattern(
        pattern::OperandJoin,
        pattern::EngineAll,
        vec![
            pattern::BuildPattern(pattern::OperandProjection, pattern::EngineAll, Vec::new()),
            pattern::BuildPattern(pattern::OperandSelection, pattern::EngineAll, Vec::new()),
        ],
    );
    let mut iter =
        NewExprIterFromGroupElem(&g2, 0, &pat).expect("root join expression should match");

    let mut count = 0;
    while iter.Matched() {
        count += 1;
        assert_eq!(iter.Pattern.Operand, pattern::OperandJoin);
        assert_eq!(iter.Children.len(), 2);

        assert!(std::rc::Rc::ptr_eq(
            iter.Children[0].Group.as_ref().unwrap(),
            &g0
        ));
        assert!(iter.Children[0].Matched());
        assert_eq!(iter.Children[0].Pattern.Operand, pattern::OperandProjection);
        assert!(iter.Children[0].Children.is_empty());

        assert!(std::rc::Rc::ptr_eq(
            iter.Children[1].Group.as_ref().unwrap(),
            &g1
        ));
        assert!(iter.Children[1].Matched());
        assert_eq!(iter.Children[1].Pattern.Operand, pattern::OperandSelection);
        assert!(iter.Children[1].Children.is_empty());

        iter.Next();
    }
    assert_eq!(count, 9);
}

// 对应 Go TestExprIterReset：selection 分支的三个 GroupExpr 都再挂一层包含
// Selection/Limit 的公共 Group（g2），模式为 join(projection, selection(limit))；
// 枚举数是 projection 的 3 种 * (selection 的 3 种 * g2 内 2 个 limit) = 3*3*2=18。
#[test]
fn expr_iter_reset_enumerates_nested_selection_limit_combinations() {
    let ctx = test_ctx();
    let g0 = NewGroupWithSchema(
        NewGroupExpr(projection(&ctx, vec![Box::new(NewZero())])),
        &schema(),
    );
    assert!(Group::Insert(&g0, NewGroupExpr(limit(&ctx, 1))));
    assert!(Group::Insert(
        &g0,
        NewGroupExpr(projection(&ctx, vec![Box::new(NewOne())]))
    ));
    assert!(Group::Insert(&g0, NewGroupExpr(limit(&ctx, 2))));
    assert!(Group::Insert(
        &g0,
        NewGroupExpr(projection(&ctx, vec![Box::new(NewNull())]))
    ));

    let sel1 = NewGroupExpr(selection(&ctx, vec![Box::new(NewNull())]));
    let g1 = NewGroupWithSchema(sel1.clone(), &schema());
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 3))));
    let sel2 = NewGroupExpr(selection(&ctx, vec![Box::new(NewOne())]));
    assert!(Group::Insert(&g1, sel2.clone()));
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 4))));
    let sel3 = NewGroupExpr(selection(&ctx, vec![Box::new(NewZero())]));
    assert!(Group::Insert(&g1, sel3.clone()));

    let g2 = NewGroupWithSchema(
        NewGroupExpr(selection(&ctx, vec![Box::new(NewNull())])),
        &schema(),
    );
    assert!(Group::Insert(&g2, NewGroupExpr(limit(&ctx, 3))));
    assert!(Group::Insert(
        &g2,
        NewGroupExpr(selection(&ctx, vec![Box::new(NewOne())]))
    ));
    assert!(Group::Insert(&g2, NewGroupExpr(limit(&ctx, 4))));
    assert!(Group::Insert(
        &g2,
        NewGroupExpr(selection(&ctx, vec![Box::new(NewZero())]))
    ));

    // Match Go's setup order: link the three Selection expressions only after
    // both mixed-operand groups have been populated.
    sel1.borrow_mut().SetChildren(vec![g2.clone()]);
    sel2.borrow_mut().SetChildren(vec![g2.clone()]);
    sel3.borrow_mut().SetChildren(vec![g2.clone()]);

    let g3 = join_group(&ctx, vec![g0.clone(), g1.clone()]);

    let lhs = pattern::BuildPattern(pattern::OperandProjection, pattern::EngineAll, Vec::new());
    let rhs = pattern::BuildPattern(
        pattern::OperandSelection,
        pattern::EngineAll,
        vec![pattern::BuildPattern(
            pattern::OperandLimit,
            pattern::EngineAll,
            Vec::new(),
        )],
    );
    let pat = pattern::BuildPattern(pattern::OperandJoin, pattern::EngineAll, vec![lhs, rhs]);

    let mut iter =
        NewExprIterFromGroupElem(&g3, 0, &pat).expect("root join expression should match");

    let mut count = 0;
    while iter.Matched() {
        count += 1;
        assert_eq!(iter.Pattern.Operand, pattern::OperandJoin);
        assert_eq!(iter.Children.len(), 2);

        assert!(std::rc::Rc::ptr_eq(
            iter.Children[0].Group.as_ref().unwrap(),
            &g0
        ));
        assert!(iter.Children[0].Matched());
        assert_eq!(iter.Children[0].Pattern.Operand, pattern::OperandProjection);
        assert!(iter.Children[0].Children.is_empty());

        assert!(std::rc::Rc::ptr_eq(
            iter.Children[1].Group.as_ref().unwrap(),
            &g1
        ));
        assert!(iter.Children[1].Matched());
        assert_eq!(iter.Children[1].Pattern.Operand, pattern::OperandSelection);
        assert_eq!(iter.Children[1].Children.len(), 1);

        assert!(std::rc::Rc::ptr_eq(
            iter.Children[1].Children[0].Group.as_ref().unwrap(),
            &g2
        ));
        assert!(iter.Children[1].Children[0].Matched());
        assert_eq!(
            iter.Children[1].Children[0].Pattern.Operand,
            pattern::OperandLimit
        );
        assert!(iter.Children[1].Children[0].Children.is_empty());

        iter.Next();
    }
    assert_eq!(count, 18);
}

fn count_matched_iter(group: &GroupRef, pat: &pattern::Pattern) -> i32 {
    let mut count = 0;
    for element in 0..group.borrow().Equivalents.len() {
        let Some(mut iter) = NewExprIterFromGroupElem(group, element, pat) else {
            continue;
        };
        while iter.Matched() {
            count += 1;
            iter.Next();
        }
    }
    count
}

// 对应 Go TestExprIterWithEngineType：Group4 是 Join{Group3, Group3}，Group3 下有
// 两个 TiKVSingleGather，一个套 TiFlash 引擎的 Group1，一个套 TiKV 引擎的
// Group2；用不同 EngineTypeSet 的 Pattern 验证按引擎过滤的匹配计数。
#[test]
fn expr_iter_with_engine_type_filters_matches_by_engine_type_set() {
    let ctx = test_ctx();
    let g1 = NewGroupWithSchema(
        NewGroupExpr(selection(&ctx, vec![Box::new(NewOne())])),
        &schema(),
    );
    g1.borrow_mut().SetEngineType(pattern::EngineTiFlash);
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 1))));
    assert!(Group::Insert(
        &g1,
        NewGroupExpr(projection(&ctx, vec![Box::new(NewOne())]))
    ));
    assert!(Group::Insert(&g1, NewGroupExpr(limit(&ctx, 2))));

    let g2 = NewGroupWithSchema(
        NewGroupExpr(selection(&ctx, vec![Box::new(NewOne())])),
        &schema(),
    );
    g2.borrow_mut().SetEngineType(pattern::EngineTiKV);
    assert!(Group::Insert(&g2, NewGroupExpr(limit(&ctx, 2))));
    assert!(Group::Insert(
        &g2,
        NewGroupExpr(projection(&ctx, vec![Box::new(NewOne())]))
    ));
    assert!(Group::Insert(&g2, NewGroupExpr(limit(&ctx, 3))));

    let flash_gather = NewGroupExpr(tikv_single_gather(&ctx));
    flash_gather.borrow_mut().SetChildren(vec![g1.clone()]);
    let g3 = NewGroupWithSchema(flash_gather, &schema());
    g3.borrow_mut().SetEngineType(pattern::EngineTiDB);

    let tikv_gather = NewGroupExpr(tikv_single_gather(&ctx));
    tikv_gather.borrow_mut().SetChildren(vec![g2.clone()]);
    assert!(Group::Insert(&g3, tikv_gather));

    let join_expr = NewGroupExpr(join(&ctx));
    join_expr
        .borrow_mut()
        .SetChildren(vec![g3.clone(), g3.clone()]);
    let g4 = NewGroupWithSchema(join_expr, &schema());
    g4.borrow_mut().SetEngineType(pattern::EngineTiDB);

    // Group4: Join input:[Group3, Group3]
    // Group3: TiKVSingleGather input:[Group2 EngineTiKV], TiKVSingleGather input:[Group1 EngineTiFlash]

    let gather_limit = |engine| {
        pattern::BuildPattern(
            pattern::OperandTiKVSingleGather,
            pattern::EngineTiDBOnly,
            vec![pattern::BuildPattern(
                pattern::OperandLimit,
                engine,
                Vec::new(),
            )],
        )
    };
    assert_eq!(
        count_matched_iter(&g3, &gather_limit(pattern::EngineTiKVOnly)),
        2
    );
    assert_eq!(
        count_matched_iter(&g3, &gather_limit(pattern::EngineTiFlashOnly)),
        2
    );
    assert_eq!(
        count_matched_iter(&g3, &gather_limit(pattern::EngineTiKVOrTiFlash)),
        4
    );
    let p3 = pattern::BuildPattern(
        pattern::OperandTiKVSingleGather,
        pattern::EngineTiDBOnly,
        vec![pattern::BuildPattern(
            pattern::OperandSelection,
            pattern::EngineTiFlashOnly,
            Vec::new(),
        )],
    );
    assert_eq!(count_matched_iter(&g3, &p3), 1);
    let p4 = pattern::BuildPattern(
        pattern::OperandTiKVSingleGather,
        pattern::EngineTiDBOnly,
        vec![pattern::BuildPattern(
            pattern::OperandProjection,
            pattern::EngineTiKVOnly,
            Vec::new(),
        )],
    );
    assert_eq!(count_matched_iter(&g3, &p4), 1);

    let join_gather_limit = |left, right| {
        pattern::BuildPattern(
            pattern::OperandJoin,
            pattern::EngineTiDBOnly,
            vec![gather_limit(left), gather_limit(right)],
        )
    };
    assert_eq!(
        count_matched_iter(
            &g4,
            &join_gather_limit(pattern::EngineTiKVOnly, pattern::EngineTiKVOnly)
        ),
        4
    );
    assert_eq!(
        count_matched_iter(
            &g4,
            &join_gather_limit(pattern::EngineTiFlashOnly, pattern::EngineTiKVOnly)
        ),
        4
    );
    assert_eq!(
        count_matched_iter(
            &g4,
            &join_gather_limit(pattern::EngineTiKVOrTiFlash, pattern::EngineTiKVOrTiFlash)
        ),
        16
    );

    // 不是 EngineType 测试用例，而是验证叶子模式没有 AnyOperand 时的匹配。
    let p8 = pattern::BuildPattern(
        pattern::OperandJoin,
        pattern::EngineTiDBOnly,
        vec![
            pattern::BuildPattern(
                pattern::OperandTiKVSingleGather,
                pattern::EngineTiDBOnly,
                Vec::new(),
            ),
            pattern::BuildPattern(
                pattern::OperandTiKVSingleGather,
                pattern::EngineTiDBOnly,
                Vec::new(),
            ),
        ],
    );
    assert_eq!(count_matched_iter(&g4, &p8), 4);
}
