// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
// 物理计划工具函数单元测试。
//
// 验证 `FlattenListPushDownPlan` / `FlattenTreePushDownPlan` 的展开顺序，
// 与 Go 侧 `physical_utils_test.go` 语义对齐。

// 本文件对应 pkg/planner/core/operator/physicalop/physical_utils_test.go。两个 Go
// 测试都只依赖 `base.PhysicalPlan.SetChildren` 拼接手工构造的物理计划树，再调用
// `FlattenListPushDownPlan`/`FlattenTreePushDownPlan` 断言展开顺序。
// 不解析 SQL；这两个函数在本 crate 里都是真实生产实现（见 physical_utils.rs），因此
// 这里是逐条语义对齐的忠实迁移。
//
// 唯一的结构性差异：Go 用 `PhysicalLocalIndexLookUp`（`physical_indexlookup.go`）
// 作为二元“回表”节点占位符；该文件在本仓库里整体处于尚未接通的翻译存档状态（未加入
// `base::PhysicalPlan` 体系，见 physical_indexlookup.rs 顶部说明），因此这里改用同样
// 已注册为 `base::PhysicalPlan`、同样是二元子节点占位符的 `PhysicalIndexLookUpReader`
// 代替，测试只依赖“二元节点 + 通用 children() 遍历”这一点，与 Go 用例验证的
// `FlattenTreePushDownPlan` 行为完全等价。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::PhysicalPlan;

use crate::{
    CalcChildExpectedCnt, FlattenListPushDownPlan, FlattenTreePushDownPlan, GetTblStats,
    PhysicalIndexLookUpReader, PhysicalIndexScan, PhysicalLimit, PhysicalProjection,
    PhysicalTableReader, PhysicalTableScan,
};

/// 最小 PlanContext：分配计划 ID，并提供优化器变量记录所需的 SessionVars。
struct TestPlanContext(
    AtomicI32,
    base::BuiltinFunctionUsageCounter,
    planctx::variable::SessionVars,
    exprstatic::ExprContext,
);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.2
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        &self.3
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("physical_utils flatten test does not build ranges")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

/// 构造测试用 ContextRef。
fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
        exprstatic::NewExprContext(Vec::new()),
    ))
}

#[test]
fn get_tbl_stats_uses_leaf_scan_histograms_like_go() {
    let ctx = context();
    let table_hist: property::HistCollRef = Arc::new("table histogram");
    let derived_hist: property::HistCollRef = Arc::new("derived projection histogram");
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.TblColHists = Some(table_hist.clone());
    let mut projection = PhysicalProjection::New(ctx);
    projection.set_stats(property::StatsInfo {
        HistColl: Some(derived_hist),
        ..property::StatsInfo::default()
    });
    projection.set_children(vec![Box::new(scan)]);

    let actual = GetTblStats(Some(&projection)).expect("table scan histogram");
    assert!(Arc::ptr_eq(&actual, &table_hist));

    let index_hist: property::HistCollRef = Arc::new("index histogram");
    let mut index_scan = PhysicalIndexScan::New(context());
    index_scan.TblColHists = Some(index_hist.clone());
    let actual = GetTblStats(Some(&index_scan)).expect("index scan histogram");
    assert!(Arc::ptr_eq(&actual, &index_hist));
}

#[test]
fn calc_child_expected_cnt_records_ordering_selectivity_variable() {
    let ctx = context();
    ctx.GetSessionVars().ResetRelevantOptVarsAndFixes(true);
    let property = property::NewPhysicalProperty(
        property::RootTaskType,
        &[expression::Column::default()],
        false,
        1.0,
        false,
    );

    let _ = CalcChildExpectedCnt(ctx.as_ref(), &property, 100.0, 10.0);

    assert_eq!(
        ctx.GetSessionVars().RelevantOptVarsAndFixes().0,
        vec![vardef::TiDBOptOrderingIdxSelRatio.to_owned()]
    );
}

/// 提取一个 `Box<dyn PhysicalPlan>` 的数据指针，忽略 vtable，用来做等价于 Go
/// `require.Same` 的对象身份比较（Rust 没有指针相等的 trait-object 内建断言）。
fn identity(plan: &dyn PhysicalPlan) -> *const () {
    plan as *const dyn PhysicalPlan as *const ()
}

/// connectOneChildPlans 对应 Go 测试辅助：把多个物理计划按单链父子关系串起来，便于验证 flatten 顺序。
fn connect_one_child_plans(mut plans: Vec<Box<dyn PhysicalPlan>>) -> Box<dyn PhysicalPlan> {
    for index in (1..plans.len()).rev() {
        let child = plans.remove(index);
        plans[index - 1].set_children(vec![child]);
    }
    plans.remove(0)
}

// test_flatten_list_push_down_plan 对应 Go 的 TestFlattenListPushDownPlan：单链
// flatten 断言返回顺序从叶子到根。
#[test]
/// 单链 Limit→Projection→TableReader→TableScan，断言叶子在前。
fn test_flatten_list_push_down_plan() {
    let ctx = context();
    let plans: Vec<Box<dyn PhysicalPlan>> = vec![
        Box::new(PhysicalLimit::New(ctx.clone(), 0, 0)),
        Box::new(PhysicalProjection::New(ctx.clone())),
        Box::new(PhysicalTableReader::New(ctx.clone())),
        Box::new(PhysicalTableScan::New(ctx)),
    ];
    let identities: Vec<*const ()> = plans.iter().map(|plan| identity(plan.as_ref())).collect();
    let root = connect_one_child_plans(plans);

    let flatten = FlattenListPushDownPlan(root.as_ref());
    assert_eq!(identities.len(), flatten.len());
    assert_eq!(identity(flatten[3]), identities[0]);
    assert_eq!(identity(flatten[2]), identities[1]);
    assert_eq!(identity(flatten[1]), identities[2]);
    assert_eq!(identity(flatten[0]), identities[3]);
}

// test_flatten_tree_push_down_plan 对应 Go 的 TestFlattenTreePushDownPlan：保留
// Go 原注释里那棵无效但专用于验证展开顺序的计划树 fixture。
//
//          Limit1
//         /
//        IndexLookUp1
//      /          \
//     Limit2      IndexLookUp2
//    /           /           \
//   Projection  IndexScan2   TableScan
//  /
//  IndexScan1
// Should have order: IndexScan1, Projection, Limit2, IndexScan2, TableScan, IndexLookUp2, IndexLookUp1, Limit1
#[test]
/// 树形 fixture：断言后序展开与 unnatural 边下标。
fn test_flatten_tree_push_down_plan() {
    let ctx = context();
    let mut limit1: Box<dyn PhysicalPlan> = Box::new(PhysicalLimit::New(ctx.clone(), 0, 0));
    let mut index_lookup1: Box<dyn PhysicalPlan> =
        Box::new(PhysicalIndexLookUpReader::New(ctx.clone()));
    let mut limit2: Box<dyn PhysicalPlan> = Box::new(PhysicalLimit::New(ctx.clone(), 0, 0));
    let mut projection: Box<dyn PhysicalPlan> = Box::new(PhysicalProjection::New(ctx.clone()));
    let index_scan1: Box<dyn PhysicalPlan> = Box::new(PhysicalIndexScan::New(ctx.clone()));
    let mut index_lookup2: Box<dyn PhysicalPlan> =
        Box::new(PhysicalIndexLookUpReader::New(ctx.clone()));
    let index_scan2: Box<dyn PhysicalPlan> = Box::new(PhysicalIndexScan::New(ctx.clone()));
    let table_scan: Box<dyn PhysicalPlan> = Box::new(PhysicalTableScan::New(ctx));

    let limit1_id = identity(limit1.as_ref());
    let index_lookup1_id = identity(index_lookup1.as_ref());
    let limit2_id = identity(limit2.as_ref());
    let projection_id = identity(projection.as_ref());
    let index_scan1_id = identity(index_scan1.as_ref());
    let index_lookup2_id = identity(index_lookup2.as_ref());
    let index_scan2_id = identity(index_scan2.as_ref());
    let table_scan_id = identity(table_scan.as_ref());

    // Limit2 -> Projection -> IndexScan1 单链。
    projection.set_children(vec![index_scan1]);
    limit2.set_children(vec![projection]);

    index_lookup2.set_children(vec![index_scan2, table_scan]);
    // IndexLookUp2 为二元节点：IndexScan2 与 TableScan。
    index_lookup1.set_children(vec![limit2, index_lookup2]);
    limit1.set_children(vec![index_lookup1]);

    let (flatten, unnatural) = FlattenTreePushDownPlan(limit1.as_ref());
    // unnatural: Limit2→IndexLookUp1、IndexScan2→IndexLookUp2（非紧邻）。
    assert_eq!(flatten.len(), 8);
    assert_eq!(identity(flatten[0]), index_scan1_id);
    assert_eq!(identity(flatten[1]), projection_id);
    assert_eq!(identity(flatten[2]), limit2_id);
    assert_eq!(identity(flatten[3]), index_scan2_id);
    assert_eq!(identity(flatten[4]), table_scan_id);
    assert_eq!(identity(flatten[5]), index_lookup2_id);
    assert_eq!(identity(flatten[6]), index_lookup1_id);
    assert_eq!(identity(flatten[7]), limit1_id);

    assert_eq!(unnatural.len(), 2);
    assert_eq!(unnatural.get(&2), Some(&6));
    assert_eq!(unnatural.get(&3), Some(&5));
}

#[test]
fn count_extrema_partial_schema_keeps_count_and_collated_value() {
    let ctx = context();
    for name in [parser_ast::AggFuncMaxCount, parser_ast::AggFuncMinCount] {
        let mut ft = *expression::types::NewFieldType(expression::mysql::TypeString);
        ft.SetCharset("utf8mb4".into());
        ft.SetCollate("utf8mb4_general_ci".into());
        let function = aggregation::NewAggFuncDesc(
            ctx.GetExprCtx(),
            name,
            vec![Box::new(expression::Column::new(ft, 1, 1, 0))],
            false,
        )
        .unwrap();
        let mut agg = crate::BasePhysicalAgg::New(crate::PhysicalSchemaProducer::New(
            crate::BasePhysicalPlan::New(ctx.clone(), "HashAgg", 0),
        ));
        agg.PhysicalSchemaProducer
            .SetSchema(expression::NewSchema(vec![expression::Column::new(
                function.RetTp.clone().unwrap(),
                2,
                2,
                0,
            )]));
        agg.AggFuncs = vec![function];
        let (partial, final_info) = crate::BuildFinalModeAggregation(&agg, false).unwrap();
        assert_eq!(partial.Schema.Columns.len(), 2);
        assert_eq!(
            partial.Schema.Columns[0]
                .RetType
                .as_ref()
                .unwrap()
                .GetType(),
            expression::mysql::TypeLonglong
        );
        assert_eq!(
            partial.Schema.Columns[1]
                .RetType
                .as_ref()
                .unwrap()
                .GetType(),
            expression::mysql::TypeString
        );
        assert_eq!(
            partial.Schema.Columns[1]
                .RetType
                .as_ref()
                .unwrap()
                .GetCollate(),
            "utf8mb4_general_ci"
        );
        assert_eq!(final_info.AggFuncs[0].Args.len(), 2);
    }
}
