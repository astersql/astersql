// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// CanonicalFindBestTaskRouter 规范物理计划路由单元测试。
// 验证逻辑树枚举、挂接与代价计算，以及 DataSource 转 TableReader/Scan。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan as _, Plan as _};
use logicalop::LogicalPlan as _;

use crate::{
    AggMppRunMode, BasePhysicalJoin, BasePhysicalPlan, CanonicalFindBestTaskRouter,
    ExhaustPhysicalPlans4LogicalAggregation, PhysicalExchangeSender, PhysicalHashAgg,
    PhysicalIndexJoin, PhysicalIndexReader, PhysicalLimit, PhysicalProjection,
    PhysicalSchemaProducer, PhysicalSelection, PhysicalSort, PhysicalTableDual,
    PhysicalTableReader, PhysicalTableScan,
};

#[test]
/// Go waits until attach2Task to build the lookup Reader subtree.
fn decorrelated_semi_index_join_reader_ids_follow_go_attach_sequence() {
    let context = context();
    let producer =
        PhysicalSchemaProducer::New(BasePhysicalPlan::New(context.clone(), "IndexJoin", 0));
    let mut join =
        PhysicalIndexJoin::New(BasePhysicalJoin::New(producer, base::JoinType::SemiJoin)).Init(
            context.clone(),
            property::StatsInfo::default(),
            0,
            Vec::new(),
        );
    join.set_id(30);
    join.FromDecorrelatedApply = true;
    join.BasePhysicalJoin.InnerChildIdx = 1;

    let outer_scan = PhysicalTableScan::New(context.clone());
    let mut outer_sender = PhysicalExchangeSender::New(context.clone());
    outer_sender.set_children(vec![Box::new(outer_scan)]);
    let mut outer_reader = PhysicalTableReader::New(context.clone());
    outer_reader.SetChildren(vec![Box::new(outer_sender)]);

    let inner_scan = PhysicalTableScan::New(context.clone());
    let mut inner_selection = PhysicalSelection::New(context.clone());
    inner_selection.set_children(vec![Box::new(inner_scan)]);
    let mut inner_reader = PhysicalTableReader::New(context);
    inner_reader.SetChildren(vec![Box::new(inner_selection)]);
    join.set_children(vec![Box::new(outer_reader), Box::new(inner_reader)]);

    assert!(
        crate::AlignDecorrelatedSemiIndexJoinReaderPlanIDs(&mut join)
            .expect("align selected Reader subtree")
    );
    let outer_reader = join.children()[0];
    let outer_sender = outer_reader.children()[0];
    let outer_scan = outer_sender.children()[0];
    let inner_reader = join.children()[1];
    let inner_selection = inner_reader.children()[0];
    let inner_scan = inner_selection.children()[0];
    assert_eq!(
        [outer_scan.id(), outer_sender.id(), outer_reader.id()],
        [52, 53, 54]
    );
    assert_eq!(
        [inner_scan.id(), inner_selection.id(), inner_reader.id()],
        [55, 56, 57]
    );
}

#[test]
fn q2_index_join_is_not_left_inside_an_index_reader() {
    let context = context();
    let producer =
        PhysicalSchemaProducer::New(BasePhysicalPlan::New(context.clone(), "IndexJoin", 0));
    let join = PhysicalIndexJoin::New(BasePhysicalJoin::New(producer, base::JoinType::InnerJoin))
        .Init(
            context.clone(),
            property::StatsInfo::default(),
            0,
            Vec::new(),
        );
    let mut reader = PhysicalIndexReader::New(context);
    reader.SetChildren(vec![Box::new(join)]);

    let normalized = crate::strip_reader_around_canonical_index_join(&reader)
        .expect("normalize IndexJoin reader boundary");

    assert!(normalized.as_any().is::<PhysicalIndexJoin>());
}

#[test]
fn mpp_reader_flattening_stops_at_root_index_join_boundary() {
    let context = context();
    let producer =
        PhysicalSchemaProducer::New(BasePhysicalPlan::New(context.clone(), "IndexJoin", 0));
    let mut join =
        PhysicalIndexJoin::New(BasePhysicalJoin::New(producer, base::JoinType::InnerJoin)).Init(
            context.clone(),
            property::StatsInfo::default(),
            0,
            Vec::new(),
        );
    let mut reader = PhysicalTableReader::New(context.clone());
    reader.ReadReqType = crate::ReadReqType::MPP;
    reader.SetChildren(vec![Box::new(PhysicalTableScan::New(context.clone()))]);
    join.set_children(vec![Box::new(reader)]);

    let cloned = crate::FlattenNestedMPPReaders(Box::new(join), &context)
        .expect("flatten nested MPP readers");

    assert!(cloned.children()[0].as_any().is::<PhysicalTableReader>());
}

#[test]
fn nested_mpp_reader_pass_through_is_folded_into_its_fragment() {
    let context = context();
    let mut sender = PhysicalExchangeSender::New(context.clone());
    sender.ExchangeType = tipb::ExchangeType::PassThrough;
    sender.set_children(vec![Box::new(PhysicalTableScan::New(context.clone()))]);
    let mut reader = PhysicalTableReader::New(context.clone());
    reader.ReadReqType = crate::ReadReqType::MPP;
    reader.SetChildren(vec![Box::new(sender)]);

    let flattened = crate::FlattenNestedMPPReaders(Box::new(reader), &context)
        .expect("flatten nested MPP reader");

    assert!(flattened.as_any().is::<PhysicalTableScan>());
}

#[test]
fn mpp_pipeline_flattening_keeps_the_root_reader_boundary() {
    let context = context();
    let mut nested_sender = PhysicalExchangeSender::New(context.clone());
    nested_sender.ExchangeType = tipb::ExchangeType::PassThrough;
    nested_sender.set_children(vec![Box::new(PhysicalTableScan::New(context.clone()))]);
    let mut nested_reader = PhysicalTableReader::New(context.clone());
    nested_reader.ReadReqType = crate::ReadReqType::MPP;
    nested_reader.SetChildren(vec![Box::new(nested_sender)]);

    let mut aggregate = PhysicalHashAgg {
        BasePhysicalAgg: crate::BasePhysicalAgg::New(PhysicalSchemaProducer::New(
            BasePhysicalPlan::New(context.clone(), "HashAgg", 0),
        )),
        TiflashPreAggMode: String::new(),
    };
    aggregate.set_children(vec![Box::new(nested_reader)]);
    let mut outer_sender = PhysicalExchangeSender::New(context.clone());
    outer_sender.ExchangeType = tipb::ExchangeType::PassThrough;
    outer_sender.set_children(vec![Box::new(aggregate)]);
    let mut outer_reader = PhysicalTableReader::New(context.clone());
    outer_reader.ReadReqType = crate::ReadReqType::MPP;
    outer_reader.SetChildren(vec![Box::new(outer_sender)]);

    let flattened = crate::FlattenNestedMPPReadersBelowRoot(Box::new(outer_reader), &context)
        .expect("flatten MPP pipeline below its root reader");

    assert!(flattened.as_any().is::<PhysicalTableReader>());
    assert!(
        flattened.children()[0]
            .as_any()
            .is::<PhysicalExchangeSender>()
    );
    assert!(
        flattened.children()[0].children()[0]
            .as_any()
            .is::<PhysicalHashAgg>()
    );
    assert!(
        flattened.children()[0].children()[0].children()[0]
            .as_any()
            .is::<PhysicalTableScan>()
    );
}

#[test]
fn index_join_rejects_keys_that_do_not_belong_to_selected_children() {
    let mut outer_key = expression::Column::default();
    outer_key.UniqueID = 12;
    let mut inner_key = expression::Column::default();
    inner_key.UniqueID = 13;
    let mut unrelated = expression::Column::default();
    unrelated.UniqueID = 55;

    assert!(!crate::index_join_keys_match_children(
        &[outer_key],
        &[inner_key.Clone()],
        &expression::NewSchema(vec![unrelated]),
        &expression::NewSchema(vec![inner_key]),
    ));
}

#[test]
/// Root/MPP 边界统一识别三种 IndexJoin 动态类型。
fn canonical_router_recognizes_all_index_join_families() {
    use std::any::{Any, TypeId};

    use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode};
    use crate::physical_index_hash_join::LegacyPhysicalIndexHashJoin;
    use crate::physical_index_merge_join::LegacyPhysicalIndexMergeJoin;

    let node = |kind| PhysicalPlanNode {
        id: 1,
        kind,
        schema: Vec::new(),
        children: Vec::new(),
        stats: Default::default(),
        required_properties: Vec::new(),
    };
    let hash = LegacyPhysicalIndexHashJoin {
        outer: node(PhysicalKind::IndexHashJoin),
        inner: node(PhysicalKind::LocalIndexLookup),
        keep_outer_order: false,
        concurrency: 1,
        cached_cost: None,
    };
    let merge = LegacyPhysicalIndexMergeJoin {
        outer: node(PhysicalKind::IndexMergeJoin),
        inner: node(PhysicalKind::LocalIndexLookup),
        key_offset_order: Vec::new(),
        compare_functions: Vec::new(),
        outer_compare_functions: Vec::new(),
        need_outer_sort: false,
        descending: false,
        concurrency: 1,
    };

    assert!(crate::is_canonical_index_join_type(TypeId::of::<
        crate::PhysicalIndexJoin,
    >()));
    assert!(crate::is_canonical_index_join_type(hash.type_id()));
    assert!(crate::is_canonical_index_join_type(merge.type_id()));
    let make_base = || {
        PhysicalIndexJoin::New(BasePhysicalJoin::New(
            PhysicalSchemaProducer::New(BasePhysicalPlan::New(context(), "IndexJoin", 0)),
            base::JoinType::InnerJoin,
        ))
    };
    let typed_hash = crate::PhysicalIndexHashJoin::New(make_base());
    let typed_merge = crate::PhysicalIndexMergeJoin::New(make_base());
    assert!(crate::is_canonical_index_join_type(typed_hash.type_id()));
    assert!(crate::is_canonical_index_join_type(typed_merge.type_id()));
    assert!(crate::index_join_base(&typed_hash).is_some());
    assert!(crate::index_join_base(&typed_merge).is_some());
}

/// 路由测试用的最小 PlanContext：提供表达式与 Ranger 上下文，避免 DataSource 统计推导 panic。
struct TestPlanContext {
    plan_id: AtomicI32,
    session_vars: planctx::variable::SessionVars,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
    // 中文补充：DataSource 统计推导始终取 ExprCtx/RangerCtx，故此处必须返回可用实例。
    // `logicalop::DataSource::DeriveStats` unconditionally fetches an
    // `ExprContext` and a `RangerContext` (mirroring Go's
    // `deriveStats4DataSource`/`deriveTablePathStats`, which always read
    // `ds.SCtx().GetExprCtx()`/`GetRangerCtx()` up front even when there are
    // zero conditions to normalize or no correlated-column access path), so
    // unlike the other mocks in this file these two must return a real,
    // non-panicking context rather than `panic!`.
    expr_ctx: exprstatic::ExprContext,
    ranger_ctx: planctx::rangerctx::RangerContext<'static>,
}

/// 实现计划上下文接口；未覆盖的路径以 panic 明示测试不依赖。
impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session_vars
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        &self.expr_ctx
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        &self.ranger_ctx
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("router test does not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("router test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

/// 构造带默认会话变量与空表达式上下文的测试上下文。
fn context() -> base::ContextRef {
    context_with_mpp(false)
}

/// 构造可按需启用 MPP 的最小计划上下文。
fn context_with_mpp(allow_mpp: bool) -> base::ContextRef {
    let mut session_vars = planctx::variable::SessionVars::default();
    session_vars.AllowMPPExecution = allow_mpp;
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        session_vars,
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
        expr_ctx: exprstatic::NewExprContext(Vec::new()),
        ranger_ctx: planctx::rangerctx::RangerContext {
            TypeCtx: expression::types::DefaultStmtNoWarningContext.clone(),
            ErrCtx: expression::errctx::StrictNoWarningContext.clone(),
            ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
            RangeFallbackHandler: None,
            PlanCacheTracker: None,
            OptimizerFixControl: Default::default(),
            UseCache: false,
            RegardNULLAsPoint: true,
            OptPrefixIndexSingleScan: false,
        },
    })
}

#[test]
fn physical_init_reuses_the_constructor_plan_id() {
    let context = context();
    let projection_id = PhysicalProjection::New(context.clone())
        .Init(
            context.clone(),
            property::StatsInfo::default(),
            7,
            Vec::new(),
        )
        .id();
    let next_id = BasePhysicalPlan::New(context, "next", 0).id();

    assert_eq!(next_id, projection_id + 1);
}

#[test]
/// Root 请求仍枚举 MppTiDB 聚合，使 TiFlash partial agg 可在 TiDB 完成 final agg。
fn grouped_root_aggregation_enumerates_mpp_tidb_candidate() {
    let context = context_with_mpp(true);
    let column = expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let mut source = logicalop::DataSource::default().Init(context.clone(), 0);
    source.TableInfo.TiFlashReplica = Some(expression::model::TiFlashReplicaInfo {
        Count: 1,
        Available: true,
        ..Default::default()
    });
    source.SetSchema(expression::NewSchema(vec![column.Clone()]));
    source.SetStats(property::StatsInfo {
        RowCount: 100.0,
        ..Default::default()
    });
    let source_has_tiflash = source.HasTiFlash();
    source
        .base_mut()
        .PreparePossibleProperties(&[source_has_tiflash]);

    let count = aggregation::NewAggFuncDesc(
        context.GetExprCtx(),
        parser_ast::AggFuncCount,
        vec![Box::new(column.Clone())],
        false,
    )
    .expect("build COUNT descriptor");
    let output = expression::Column::new(count.RetTp.clone().expect("COUNT return type"), 2, 2, 0);
    let mut logical = logicalop::LogicalAggregation {
        AggFuncs: vec![count],
        GroupByItems: vec![Box::new(column)],
        ..Default::default()
    }
    .Init(context, 0);
    logical.SetSchema(expression::NewSchema(vec![output]));
    logical.SetStats(property::StatsInfo {
        RowCount: 10.0,
        ..Default::default()
    });
    logical.SetChildren(vec![Box::new(source)]);

    let candidates =
        ExhaustPhysicalPlans4LogicalAggregation(&logical, &property::PhysicalProperty::default());
    assert!(candidates.iter().any(|candidate| {
        candidate
            .as_any()
            .downcast_ref::<PhysicalHashAgg>()
            .is_some_and(|aggregate| aggregate.BasePhysicalAgg.MppRunMode == AggMppRunMode::MppTiDB)
    }));

    // Ordinary scalar aggregates finish in TiDB; an enclosing MPP projection
    // must not turn that root boundary back into an all-MPP aggregate.
    logical.GroupByItems.clear();
    let root =
        ExhaustPhysicalPlans4LogicalAggregation(&logical, &property::PhysicalProperty::default());
    assert!(root.iter().any(|candidate| {
        candidate
            .as_any()
            .downcast_ref::<PhysicalHashAgg>()
            .is_some_and(|aggregate| aggregate.BasePhysicalAgg.MppRunMode == AggMppRunMode::MppTiDB)
    }));
    let mut required = property::PhysicalProperty::default();
    required.TaskTp = property::MppTaskType;
    let mpp = ExhaustPhysicalPlans4LogicalAggregation(&logical, &required);
    assert!(mpp.iter().all(|candidate| {
        candidate
            .as_any()
            .downcast_ref::<PhysicalHashAgg>()
            .is_none_or(|aggregate| {
                aggregate.BasePhysicalAgg.MppRunMode == AggMppRunMode::MppScalar
            })
    }));
}

#[test]
/// A single hash key cannot satisfy a grouped MPP property that requires both keys.
fn grouped_mpp_partition_requires_every_composite_key() {
    let partition_column = |unique_id| property::MPPPartitionColumn {
        Col: expression::Column::new(
            *expression::types::NewFieldType(expression::mysql::TypeLonglong),
            unique_id,
            unique_id,
            0,
        ),
        CollateID: 0,
    };
    let suppkey = partition_column(1);
    let partkey = partition_column(2);

    assert!(!crate::mpp_agg_partition_is_satisfied(
        property::HashType,
        &[suppkey.Clone()],
        &[suppkey, partkey],
    ));
}

#[test]
/// A normal TableReader -> ExchangeSender MPP boundary is not a nested reader.
fn projection_accepts_normal_mpp_reader_boundary() {
    let context = context_with_mpp(true);
    let column = expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeNewDecimal),
        1,
        1,
        0,
    );
    let mut source = logicalop::DataSource::default().Init(context.clone(), 0);
    source.TableInfo.ID = 42;
    source.TableInfo.Name = model::ast::NewCIStr("customer");
    source.TableInfo.TiFlashReplica = Some(expression::model::TiFlashReplicaInfo {
        Count: 1,
        Available: true,
        ..Default::default()
    });
    source.PhysicalTableID = 42;
    source.TableStats.RowCount = 10_000.0;
    source.PossibleAccessPaths = vec![planner_util::AccessPath {
        IsIntHandlePath: true,
        CountAfterAccess: 10_000.0,
        ..planner_util::AccessPath::default()
    }];
    source.SetSchema(expression::NewSchema(vec![column.Clone()]));
    source.DeriveStats(false).expect("derive source statistics");

    let schema = source.Schema().Clone();
    let mut logical = logicalop::LogicalProjection {
        Exprs: vec![Box::new(column)],
        ..Default::default()
    }
    .Init(context, 0);
    logical.SetSchema(schema);
    logical.SetStats(property::StatsInfo {
        RowCount: 10_000.0,
        ..Default::default()
    });
    logical.SetChildren(vec![Box::new(source)]);

    let mut required = property::PhysicalProperty::default();
    required.TaskTp = property::MppTaskType;
    required.ExpectedCnt = f64::MAX;
    let task = CanonicalFindBestTaskRouter(&mut logical, &required)
        .expect("projection must retain a normal MPP reader child");
    assert!(!task.invalid());
    let reader = task
        .plan()
        .as_any()
        .downcast_ref::<PhysicalTableReader>()
        .expect("normal MPP boundary must keep its root TableReader");
    let sender = reader
        .children()
        .first()
        .copied()
        .expect("TableReader must retain its MPP fragment")
        .as_any()
        .downcast_ref::<PhysicalExchangeSender>()
        .expect("MPP reader must retain its exchange boundary");
    let projection = sender
        .children()
        .first()
        .copied()
        .expect("exchange sender must retain its MPP fragment")
        .as_any()
        .downcast_ref::<PhysicalProjection>()
        .expect("projection must be pushed below the exchange boundary");
    assert!(
        projection
            .children()
            .first()
            .is_some_and(|child| child.as_any().is::<PhysicalTableScan>())
    );
}

#[test]
/// Dual + Limit 经规范路由得到 PhysicalLimit→PhysicalTableDual，并可计算有限代价。
fn canonical_router_exhausts_attaches_and_costs_a_logical_tree() {
    let context = context();
    // 最小逻辑树：一行 Dual 上挂 Limit。
    let mut dual = logicalop::LogicalTableDual {
        RowCount: 1,
        ..Default::default()
    }
    .Init(context.clone(), 0);
    dual.DeriveStats(false).expect("derive dual statistics");

    let mut limit = logicalop::LogicalLimit {
        Count: 1,
        ..Default::default()
    }
    .Init(context.clone(), 0);
    limit.SetChildren(vec![Box::new(dual)]);

    // 规范路由：枚举物理候选、挂接子任务并选代价最优。
    let task = CanonicalFindBestTaskRouter(&mut limit, &property::PhysicalProperty::default())
        .expect("canonical router must select a physical task");
    assert!(!task.invalid());
    let limit = task
        .plan()
        .as_any()
        .downcast_ref::<PhysicalLimit>()
        .expect("logical limit becomes PhysicalLimit");
    let children = limit.children();
    assert_eq!(children.len(), 1);
    assert!(children[0].as_any().is::<PhysicalTableDual>());

    let mut physical = task
        .plan()
        .clone_physical(context)
        .expect("clone selected physical tree");
    let cost = physical
        .get_plan_cost_ver1(
            property::RootTaskType,
            &costusage::new_default_plan_cost_option(),
        )
        .expect("cost selected physical tree");
    assert!(cost.is_finite());
}

#[test]
/// DataSource 走整数 handle 访问路径，路由为 PhysicalTableReader 并保留表元数据。
fn canonical_router_converts_a_table_data_source_to_reader_and_scan() {
    let context = context();
    // 配置表元数据与整数 handle 访问路径。
    let mut source = logicalop::DataSource::default().Init(context, 0);
    source.TableInfo.ID = 42;
    source.TableInfo.Name = model::ast::NewCIStr("orders");
    source.PhysicalTableID = 42;
    source.TableStats.RowCount = 8.0;
    source.PossibleAccessPaths = vec![planner_util::AccessPath {
        IsIntHandlePath: true,
        ..planner_util::AccessPath::default()
    }];
    source
        .DeriveStats(false)
        .expect("derive data source statistics");

    let task = CanonicalFindBestTaskRouter(&mut source, &property::PhysicalProperty::default())
        .expect("table access path must become a reader task");
    let reader = task
        .plan()
        .as_any()
        .downcast_ref::<PhysicalTableReader>()
        .expect("table gather becomes PhysicalTableReader");
    let scan = reader
        .GetTableScan()
        .expect("table reader retains its PhysicalTableScan");
    assert_eq!(scan.PhysicalTableID, 42);
    assert_eq!(
        scan.Table.as_ref().map(|table| table.Name.O.as_str()),
        Some("orders")
    );
    assert_eq!(
        source.TableInfo.ID, 42,
        "routing restores logical ownership"
    );
}

#[test]
/// Go reserves the selected root-candidate sequence without renumbering its child subtree.
fn selected_sort_projection_hash_agg_ids_follow_go_candidate_sequence() {
    let context = context();
    let group_column = expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let mut source = logicalop::DataSource::default().Init(context.clone(), 0);
    source.TableInfo.ID = 42;
    source.TableInfo.Name = model::ast::NewCIStr("orders");
    source.PhysicalTableID = 42;
    source.TableStats.RowCount = 100.0;
    source.PossibleAccessPaths = vec![planner_util::AccessPath {
        IsIntHandlePath: true,
        ..planner_util::AccessPath::default()
    }];
    source.SetSchema(expression::NewSchema(vec![group_column.Clone()]));
    source
        .DeriveStats(false)
        .expect("derive data source statistics");

    let count = aggregation::NewAggFuncDesc(
        context.GetExprCtx(),
        parser_ast::AggFuncCount,
        vec![Box::new(group_column.Clone())],
        false,
    )
    .expect("build COUNT descriptor");
    let count_column =
        expression::Column::new(count.RetTp.clone().expect("COUNT return type"), 2, 2, 0);
    let aggregate_schema = expression::NewSchema(vec![group_column.Clone(), count_column.Clone()]);
    let mut aggregate = logicalop::LogicalAggregation {
        AggFuncs: vec![count],
        GroupByItems: vec![Box::new(group_column.Clone())],
        ..Default::default()
    }
    .Init(context.clone(), 0);
    aggregate.SetSchema(aggregate_schema.Clone());
    aggregate.SetStats(property::StatsInfo {
        RowCount: 10.0,
        ..Default::default()
    });
    aggregate.SetChildren(vec![Box::new(source)]);

    let mut projection = logicalop::LogicalProjection {
        Exprs: vec![Box::new(group_column.Clone()), Box::new(count_column)],
        ..Default::default()
    }
    .Init(context.clone(), 0);
    projection.SetSchema(aggregate_schema.Clone());
    projection.SetStats(property::StatsInfo {
        RowCount: 10.0,
        ..Default::default()
    });
    projection.SetChildren(vec![Box::new(aggregate)]);

    let mut sort = logicalop::LogicalSort {
        ByItems: vec![planner_util::ByItems {
            Expr: Box::new(group_column),
            Desc: false,
        }],
        ..Default::default()
    }
    .Init(context, 0);
    sort.SetSchema(aggregate_schema);
    sort.SetStats(property::StatsInfo {
        RowCount: 10.0,
        ..Default::default()
    });
    sort.SetChildren(vec![Box::new(projection)]);

    let mut required = property::PhysicalProperty::default();
    required.ExpectedCnt = f64::MAX;
    let mut task = CanonicalFindBestTaskRouter(&mut sort, &required)
        .expect("canonical router must select the root candidate chain");
    let child_id = task.plan().children()[0].children()[0].children()[0].id();
    crate::AlignSelectedRootCandidatePlanIDs(task.plan_mut())
        .expect("align selected root candidate IDs");

    let root = task
        .plan()
        .as_any()
        .downcast_ref::<PhysicalSort>()
        .expect("root is PhysicalSort");
    let projection = root.children()[0]
        .as_any()
        .downcast_ref::<PhysicalProjection>()
        .expect("Sort child is PhysicalProjection");
    let aggregate = projection.children()[0]
        .as_any()
        .downcast_ref::<PhysicalHashAgg>()
        .expect("Projection child is PhysicalHashAgg");

    assert_eq!(projection.id() - root.id(), 2);
    assert_eq!(aggregate.id() - root.id(), 6);
    assert_eq!(aggregate.children()[0].id(), child_id);
}
