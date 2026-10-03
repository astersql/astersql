// Copyright 2026 AsterSQL.
use super::index_join_probe::apply_index_floor;
#[test]
fn index_floor_preserves_filter_ratio_and_unique_limit() {
    assert_eq!(apply_index_floor(2.0, 0.5, 1000.0, false), (1000.0, 250.0));
    assert_eq!(apply_index_floor(0.0, 0.0, 1000.0, false), (1000.0, 1000.0));
    assert_eq!(apply_index_floor(1.0, 1.0, 1000.0, true), (1.0, 1.0));
    assert_eq!(
        apply_index_floor(2000.0, 500.0, 1000.0, false),
        (2000.0, 500.0)
    );
}

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
struct TestPlanContext {
    plan_id: AtomicI32,
    session_vars: planctx::variable::SessionVars,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
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
    context_with_fix(None)
}

/// 构造可按需启用 MPP 的最小计划上下文。
fn context_with_fix(value: Option<&str>) -> base::ContextRef {
    let mut session_vars = planctx::variable::SessionVars::default();
    if let Some(value) = value {
        session_vars
            .SetSystemVar("tidb_opt_fix_control", value)
            .unwrap();
    }
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
fn usable_keys_floor_guards_and_fix_control_recording() {
    use super::index_join_probe::{ProbePathResult, access_rows_floor};
    let mut ctx = context();
    let stats = property::StatsInfo {
        RowCount: 2000.0,
        ..Default::default()
    };
    let mut result = ProbePathResult {
        scan: crate::PhysicalIndexScan::New(ctx.clone()),
        used_cols: 1,
        eq_ndv: 2.0,
        last_col_is_range: false,
        last_col_manager: None,
        key_offsets: vec![0, -1],
    };
    ctx.GetSessionVars().ResetRelevantOptVarsAndFixes(true);
    assert_eq!(access_rows_floor(ctx.as_ref(), None, Some(&result), 2), 0.0);
    assert_eq!(access_rows_floor(ctx.as_ref(), Some(&stats), None, 2), 0.0);
    assert_eq!(
        access_rows_floor(ctx.as_ref(), Some(&stats), Some(&result), 2),
        1000.0
    );
    assert!(
        ctx.GetSessionVars()
            .RelevantOptVarsAndFixes()
            .1
            .contains(&fixcontrol::Fix44855)
    );
    result.eq_ndv = 0.0;
    assert_eq!(
        access_rows_floor(ctx.as_ref(), Some(&stats), Some(&result), 2),
        0.0
    );
    result.eq_ndv = 2.0;
    result.last_col_is_range = true;
    assert_eq!(
        access_rows_floor(ctx.as_ref(), Some(&stats), Some(&result), 2),
        0.0
    );
    result.last_col_is_range = false;
    result.last_col_manager = Some(crate::ColWithCmpFuncManager::New(None, -1));
    assert_eq!(
        access_rows_floor(ctx.as_ref(), Some(&stats), Some(&result), 2),
        0.0
    );
    result.last_col_manager = None;
    result.key_offsets = vec![1, 1, 0];
    result.used_cols = 2;
    assert_eq!(
        access_rows_floor(ctx.as_ref(), Some(&stats), Some(&result), 2),
        1000.0
    );
    result.used_cols = 3;
    assert_eq!(
        access_rows_floor(ctx.as_ref(), Some(&stats), Some(&result), 2),
        0.0
    );
    // Recreate with OFF; the original scan retains its own real context.
    ctx = context_off();
    result.used_cols = 2;
    assert_eq!(
        access_rows_floor(ctx.as_ref(), Some(&stats), Some(&result), 2),
        0.0
    );
}
fn context_off() -> base::ContextRef {
    context_with_fix(Some("44855:OFF"))
}

#[test]
fn index_upper_bound_uses_initialized_column_and_matching_index_ndv() {
    use super::index_join_probe::ndv_lower_bound;
    let mut histograms = statistics::NewHistColl(42, 2000, 0, 2, 1);
    let field_type = expression::types::NewFieldType(expression::mysql::TypeLonglong);
    for (id, ndv) in [(7, 2), (8, 1000)] {
        histograms.Columns.insert(
            id,
            Box::new(statistics::Column {
                CMSketch: None,
                TopN: None,
                FMSketch: None,
                Info: None,
                Histogram: statistics::NewHistogram(id, ndv, 0, 1, &field_type, 0, 0),
                StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
                PhysicalID: 42,
                StatsVer: 2,
                IsHandle: false,
            }),
        );
    }
    histograms.Idx2ColUniqueIDs.insert(10, vec![8, 7]);
    histograms.Indices.insert(
        10,
        Box::new(statistics::Index {
            CMSketch: None,
            TopN: None,
            FMSketch: None,
            Info: None,
            Histogram: statistics::NewHistogram(10, 1500, 0, 1, &field_type, 0, 0),
            StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
            PhysicalID: 42,
            StatsVer: 2,
        }),
    );
    assert_eq!(ndv_lower_bound(vec![7], None), -1.0);
    assert_eq!(ndv_lower_bound(vec![], Some(&histograms)), -1.0);
    assert_eq!(ndv_lower_bound(vec![7], Some(&histograms)), 2.0);
    assert_eq!(ndv_lower_bound(vec![7, 8], Some(&histograms)), 1500.0);
    histograms.Indices.get_mut(&10).unwrap().StatsLoadedStatus = Default::default();
    assert_eq!(ndv_lower_bound(vec![7, 8], Some(&histograms)), 1000.0);
    histograms.Columns.get_mut(&8).unwrap().StatsLoadedStatus = Default::default();
    assert_eq!(ndv_lower_bound(vec![7, 8], Some(&histograms)), 2.0);
    assert_eq!(ndv_lower_bound(vec![8], Some(&histograms)), -1.0);
}

#[test]
fn dynamic_tail_range_keeps_typed_collators_and_comparison_manager() {
    use logicalop::LogicalPlan as _;
    let ctx = context();
    let int_type = *expression::types::NewFieldType(expression::mysql::TypeLonglong);
    let columns = (0..3)
        .map(|offset| {
            expression::Column::new(int_type.clone(), offset + 1, offset + 11, offset as isize)
        })
        .collect::<Vec<_>>();
    let outer = expression::Column::new(int_type.clone(), 4, 44, 0);
    let primary = model::IndexInfo {
        ID: 1,
        Name: parser_ast::NewCIStr("PRIMARY"),
        Primary: true,
        Unique: true,
        Columns: (0..2)
            .map(|offset| model::IndexColumn {
                Name: parser_ast::NewCIStr(&format!("c{offset}")),
                Offset: offset,
                Length: -1,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut source = logicalop::DataSource::default().Init(ctx.clone(), 0);
    source.Columns = (0..3)
        .map(|offset| model::ColumnInfo {
            ID: offset + 1,
            Name: parser_ast::NewCIStr(&format!("c{offset}")),
            FieldType: int_type.clone(),
            ..Default::default()
        })
        .collect();
    source.TableInfo = model::TableInfo {
        ID: 42,
        Name: parser_ast::NewCIStr("inner_table"),
        IsCommonHandle: true,
        Columns: source.Columns.clone(),
        Indices: vec![primary],
        ..Default::default()
    };
    source.PhysicalTableID = 42;
    source.SetSchema(expression::NewSchema(
        columns.iter().map(expression::Column::Clone).collect(),
    ));
    source.TableStats = property::StatsInfo {
        RowCount: 2000.0,
        StatsVersion: 2,
        ColNDVs: [(11, 2.0), (12, 1000.0), (13, 1000.0)]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    source.PossibleAccessPaths = vec![planner_util::AccessPath {
        IsCommonHandlePath: true,
        StoreType: kv::StoreType::TiKV,
        ..Default::default()
    }];
    let producer = crate::PhysicalSchemaProducer::New(crate::BasePhysicalPlan::New(
        ctx.clone(),
        "IndexJoin",
        0,
    ));
    let mut join = crate::PhysicalIndexJoin::New(crate::BasePhysicalJoin::New(
        producer,
        base::JoinType::InnerJoin,
    ));
    join.BasePhysicalJoin.InnerJoinKeys = vec![columns[0].Clone(), columns[2].Clone()];
    join.BasePhysicalJoin.OuterJoinKeys = vec![outer.Clone(), outer.Clone()];
    join.BasePhysicalJoin.OtherConditions = vec![
        expression::NewFunction(
            ctx.GetExprCtx(),
            parser_ast::GT,
            *expression::types::NewFieldType(expression::mysql::TypeTiny),
            vec![Box::new(columns[1].Clone()), Box::new(outer)],
        )
        .unwrap(),
    ];
    let probe = super::index_join_probe::best_probe(&source, &join, 1.0)
        .unwrap()
        .expect("common handle prefix candidate");
    assert_eq!(probe.result.used_cols, 2);
    assert!(probe.result.last_col_is_range);
    assert!(probe.result.last_col_manager.is_some());
    assert_eq!(probe.result.key_offsets, vec![0, -1]);
    for range in &probe.result.scan.Ranges.0 {
        assert_eq!(range.Collators.len(), range.LowVal.len());
        assert!(range.IsPoint(ctx.GetRangerCtx()));
    }
    assert_eq!(
        super::index_join_probe::access_rows_floor(
            ctx.as_ref(),
            Some(&source.TableStats),
            Some(&probe.result),
            2
        ),
        0.0
    );
}
