// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{PhysicalPlan, Plan};

use crate::{PhysicalSelection, PhysicalTableScan};

static IGNORE_INLIST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct PushDownClient;

impl kv::Client for PushDownClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("selection protobuf test must not send a KV request")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

struct TestPlanContext {
    plan_id: AtomicI32,
    session: planctx::variable::SessionVars,
    expression: Arc<exprstatic::ExprContext>,
    build_pb: base::BuildPBContext,
    builtin_usage: base::BuiltinFunctionUsageCounter,
}

impl TestPlanContext {
    fn new() -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let build_expression: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            session: planctx::variable::SessionVars::default(),
            expression,
            build_pb: base::BuildPBContext {
                ExprCtx: build_expression,
                Client: Some(Arc::new(PushDownClient)),
                TiFlashFastScan: false,
                TiFlashFineGrainedShuffleBatchSize: 0,
                GroupConcatMaxLen: 0,
                InExplainStmt: false,
                WarnHandler: None,
                ExtraWarnghandler: None,
            },
            builtin_usage: base::BuiltinFunctionUsageCounter::default(),
        }
    }
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("selection protobuf test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        &self.build_pb
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.builtin_usage.Inc(name)
    }
}

#[test]
fn normalized_explain_honors_ignore_inlist_plan_digest_like_go() {
    let _guard = IGNORE_INLIST_LOCK.lock().expect("lock ignore-in-list flag");
    let context = Arc::new(TestPlanContext::new());
    let result_type = *expression::types::NewFieldType(expression::mysql::TypeLonglong);
    let condition = expression::NewFunctionBase(
        context.expression.as_ref(),
        expression::ast::In,
        result_type.clone(),
        vec![
            Box::new(expression::Column::new(result_type, 1, 1, 0)),
            Box::new(expression::NewInt64Const(1)),
            Box::new(expression::NewInt64Const(2)),
        ],
    )
    .expect("build IN condition");
    let mut selection = PhysicalSelection::New(context);
    selection.Conditions.push(condition);
    let previous = vardef::IgnoreInlistPlanDigest.Load();

    vardef::IgnoreInlistPlanDigest.Store(true);
    let ignored = selection.ExplainNormalizedInfo();
    vardef::IgnoreInlistPlanDigest.Store(false);
    let normalized = selection.ExplainNormalizedInfo();
    vardef::IgnoreInlistPlanDigest.Store(previous);

    assert_ne!(ignored, normalized);
    assert_eq!(
        ignored,
        String::from_utf8_lossy(&expression::SortedExplainExpressionListIgnoreInlist(
            &selection.Conditions,
        ))
    );
}

#[test]
fn tiflash_protobuf_carries_explain_id_like_go() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let mut selection = PhysicalSelection::New(context.clone());
    let expected_id = selection.explain_id(&[]).to_string();
    let mut scan = PhysicalTableScan::New(context);
    scan.Table = Some(model::TableInfo {
        ID: 42,
        ..Default::default()
    });
    selection.set_children(vec![Box::new(scan)]);
    let mut build_pb = base::BuildPBContext {
        ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
        Client: Some(Arc::new(PushDownClient)),
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };

    let executor = selection
        .ToPB(&mut build_pb, kv::StoreType::TiFlash)
        .expect("encode TiFlash selection");

    assert_eq!(executor.get_executor_id(), expected_id);
}

#[test]
fn empty_tikv_selection_does_not_require_a_client() {
    let context: base::ContextRef = Arc::new(TestPlanContext::new());
    let selection = PhysicalSelection::New(context);
    let mut build_pb = base::BuildPBContext {
        ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
        Client: None,
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };

    let executor = selection
        .ToPB(&mut build_pb, kv::StoreType::TiKV)
        .expect("an expression-free TiKV selection does not consult the client");

    assert_eq!(executor.get_tp(), tipb::ExecType::TypeSelection);
    assert!(executor.get_selection().get_conditions().is_empty());
    assert!(executor.get_executor_id().is_empty());
}

fn port_e2f259adf7_real_scan(enabled: bool, row_count: i64) -> PhysicalTableScan {
    let mut ctx = TestPlanContext::new();
    ctx.session.EnableLateMaterialization = enabled;
    let mut scan = PhysicalTableScan::New(Arc::new(ctx));
    scan.StoreType = kv::StoreType::TiFlash;
    scan.TblColHists = Some(Arc::new(*statistics::NewHistColl(9, row_count, 0, 3, 0)));
    scan.PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(property::StatsInfo {
            RowCount: row_count as f64,
            ..Default::default()
        });
    let ty = *expression::types::NewFieldType(expression::mysql::TypeLonglong);
    let columns = (0..3)
        .map(|i| expression::Column::new(ty.clone(), 7 + i, 101 + i, i as isize))
        .collect::<Vec<_>>();
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(columns));
    scan.Columns = (0..3)
        .map(|i| model::ColumnInfo {
            ID: 7 + i,
            Name: parser_ast::NewCIStr(["a", "b", "c"][i as usize]),
            FieldType: ty.clone(),
            ..Default::default()
        })
        .collect();
    scan
}

fn port_e2f259adf7_function(
    scan: &PhysicalTableScan,
    name: &str,
    args: Vec<expression::ExprBox>,
) -> expression::ExprBox {
    expression::NewFunctionBase(
        scan.s_ctx().GetExprCtx(),
        name,
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        args,
    )
    .unwrap()
}
fn port_e2f259adf7_column(index: i64) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        7 + index,
        101 + index,
        index as isize,
    ))
}
fn port_e2f259adf7_filters(scan: &PhysicalTableScan) -> Vec<expression::ExprBox> {
    let plus = port_e2f259adf7_function(
        scan,
        "plus",
        vec![port_e2f259adf7_column(0), port_e2f259adf7_column(1)],
    );
    vec![
        port_e2f259adf7_function(
            scan,
            "gt",
            vec![plus, Box::new(expression::NewInt64Const(100))],
        ),
        port_e2f259adf7_function(
            scan,
            "in",
            vec![
                port_e2f259adf7_column(2),
                Box::new(expression::NewInt64Const(1)),
                Box::new(expression::NewInt64Const(2)),
                Box::new(expression::NewInt64Const(3)),
            ],
        ),
    ]
}

#[test]
fn port_e2f259adf7_real_forced_compound_filter_bypasses_small_table_and_keeps_residual() {
    use crate::tiflash_predicate_push_down::handle_physical_tiflash_late_materialization;
    let mut scan = port_e2f259adf7_real_scan(true, 1);
    scan.FilterCondition = port_e2f259adf7_filters(&scan);
    scan.ForcedLateMaterializationFilterColumnIDs = vec![101, 102];
    handle_physical_tiflash_late_materialization(&mut scan);
    assert_eq!(scan.LateMaterializationFilterCondition.len(), 1);
    assert_eq!(
        scan.LateMaterializationFilterCondition[0].HashCode(),
        scan.FilterCondition[0].HashCode()
    );
    assert_eq!(
        scan.FilterCondition.len(),
        2,
        "original predicates remain owned for residual selection"
    );
    assert!(
        scan.LateMaterializationSelectivity > 0.0 && scan.LateMaterializationSelectivity <= 1.0
    );
    assert_eq!(
        scan.stats_info().RowCount,
        scan.LateMaterializationSelectivity
    );
}

#[test]
fn port_e2f259adf7_real_forced_filter_respects_enable_order_and_fulltext_gates() {
    use crate::tiflash_predicate_push_down::handle_physical_tiflash_late_materialization;
    for mode in 0..4 {
        let mut scan = port_e2f259adf7_real_scan(mode != 0, 20_000);
        scan.FilterCondition = port_e2f259adf7_filters(&scan);
        scan.ForcedLateMaterializationFilterColumnIDs =
            if mode == 3 { vec![999] } else { vec![101, 102] };
        scan.KeepOrder = mode == 1;
        if mode == 2 {
            let mut index = crate::ColumnarIndexExtra::default();
            index.QueryInfo.mut_fts_query_info();
            scan.UsedColumnarIndexes.push(index);
        }
        handle_physical_tiflash_late_materialization(&mut scan);
        assert!(
            scan.LateMaterializationFilterCondition.is_empty(),
            "mode={mode}"
        );
        assert_eq!(scan.stats_info().RowCount, 20_000.0);
    }
}

#[test]
fn port_e2f259adf7_real_split_excludes_simple_inverted_predicates_by_physical_id() {
    use crate::tiflash_predicate_push_down::split_forced_late_materialization_conditions;
    let mut scan = port_e2f259adf7_real_scan(true, 1);
    scan.ForcedLateMaterializationFilterColumnIDs = vec![101, 102];
    let simple = port_e2f259adf7_function(
        &scan,
        "gt",
        vec![
            port_e2f259adf7_column(0),
            Box::new(expression::NewInt64Const(3)),
        ],
    );
    let mut conditions = vec![simple];
    conditions.extend(port_e2f259adf7_filters(&scan));
    conditions.push(Box::new(expression::NewInt64Const(1)));
    let (selected, rest) = split_forced_late_materialization_conditions(&conditions, &scan);
    assert_eq!((selected.len(), rest.len()), (2, 2));
    let mut index = crate::ColumnarIndexExtra::default();
    index.QueryInfo.mut_inverted_query_info().set_column_id(7);
    scan.UsedColumnarIndexes.push(index);
    let (selected, rest) = split_forced_late_materialization_conditions(&conditions, &scan);
    assert_eq!((selected.len(), rest.len()), (1, 3));
    assert_eq!(
        selected[0].HashCode(),
        conditions[1].HashCode(),
        "compound a+b is not a simple inverted predicate"
    );
    scan.ForcedLateMaterializationFilterColumnIDs.clear();
    let (selected, rest) = split_forced_late_materialization_conditions(&conditions, &scan);
    assert!(selected.is_empty());
    assert_eq!(rest.len(), 4);
}

#[test]
fn port_e2f259adf7_real_small_table_does_not_force_declared_inverted_index() {
    use crate::tiflash_predicate_push_down::handle_physical_tiflash_late_materialization;
    for matching in [false, true] {
        let mut scan = port_e2f259adf7_real_scan(true, 4);
        scan.Table = Some(model::TableInfo {
            ID: 9,
            Indices: vec![model::IndexInfo {
                ID: 1,
                Name: parser_ast::NewCIStr("idx_a"),
                InvertedInfo: Some(model::InvertedIndexInfo::default()),
                ..Default::default()
            }],
            ..Default::default()
        });
        scan.ForcedLateMaterializationFilterColumnIDs = vec![103];
        scan.FilterCondition = vec![port_e2f259adf7_function(
            &scan,
            "eq",
            vec![
                port_e2f259adf7_column(0),
                Box::new(expression::NewInt64Const(1)),
            ],
        )];
        if matching {
            scan.FilterCondition.push(port_e2f259adf7_function(
                &scan,
                "gt",
                vec![
                    port_e2f259adf7_column(2),
                    Box::new(expression::NewInt64Const(1)),
                ],
            ));
        }
        handle_physical_tiflash_late_materialization(&mut scan);
        assert!(scan.UsedColumnarIndexes.is_empty());
        assert_eq!(
            scan.LateMaterializationFilterCondition.len(),
            usize::from(matching)
        );
        if matching {
            assert_eq!(
                scan.LateMaterializationFilterCondition[0].HashCode(),
                scan.FilterCondition[1].HashCode()
            );
        }
    }
}

#[test]
fn port_e2f259adf7_real_indexed_and_unindexed_force_matrix() {
    use crate::tiflash_predicate_push_down::handle_physical_tiflash_late_materialization;
    for enabled in [false, true] {
        for force_c in [false, true] {
            let mut scan = port_e2f259adf7_real_scan(enabled, 20_000);
            scan.ForcedLateMaterializationFilterColumnIDs =
                if force_c { vec![101, 103] } else { vec![101] };
            scan.FilterCondition = vec![
                port_e2f259adf7_function(
                    &scan,
                    "gt",
                    vec![
                        port_e2f259adf7_column(0),
                        Box::new(expression::NewInt64Const(1)),
                    ],
                ),
                port_e2f259adf7_function(
                    &scan,
                    "gt",
                    vec![
                        port_e2f259adf7_column(2),
                        Box::new(expression::NewInt64Const(1)),
                    ],
                ),
            ];
            let mut index = crate::ColumnarIndexExtra::default();
            index.QueryInfo.mut_inverted_query_info().set_column_id(7);
            scan.UsedColumnarIndexes.push(index);
            handle_physical_tiflash_late_materialization(&mut scan);
            assert_eq!(scan.UsedColumnarIndexes.len(), 1);
            assert_eq!(
                scan.LateMaterializationFilterCondition.len(),
                usize::from(enabled && force_c)
            );
            if enabled && force_c {
                assert_eq!(
                    scan.LateMaterializationFilterCondition[0].HashCode(),
                    scan.FilterCondition[1].HashCode()
                );
            }
        }
    }
}
