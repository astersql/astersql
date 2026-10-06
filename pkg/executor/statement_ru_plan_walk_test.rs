// Copyright 2026 AsterSQL.

use crate::statement_ru_plan_walk::{
    StatementRUFinalOutcome, StatementRUOperatorState, StatementRUOwner, StatementRUWriteSnapshot,
    merge_statement_ru_operator_state, merge_statement_ru_unit_delta,
    new_statement_ru_terminal_calculator, snapshot_statement_ru_writes, statement_ru_failed,
    statement_ru_sort_work, statement_ru_terminal_failure, validate_statement_ru_flat_tree,
};
use crate::statement_ru_reporting::{
    StatementRUEngine, StatementRUFailureReason, StatementRUOperator,
};
use crate::statement_ru_result::{StatementRUCalculationSetup, StatementRUCalculator};
use astersql_planner_core::{FlattenPhysicalPlan, PlanKind, PlanNode};
use astersql_planner_core_base as base;
use astersql_planner_core_operator_physicalop as physicalop;
use astersql_planner_planctx as planctx;
use astersql_resourcegroup::ruv2::model::StmtUnits;
use astersql_util_execdetails::ruv2_metrics::NewRUV2Metrics;
use astersql_util_execdetails::ruv2_metrics::tikvutil;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

struct TypedPlanTestContext(
    AtomicI32,
    base::BuiltinFunctionUsageCounter,
    planctx::variable::SessionVars,
);

impl base::PlanContext for TypedPlanTestContext {
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
        unreachable!()
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        unreachable!()
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
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
fn go_merge_197_wrapped_typed_exec_stmt_plan() {
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let execute = astersql_planner_core::RuntimeExecute::New(Arc::new(
        physicalop::PhysicalTableDual::New(context.clone(), 1),
    ));
    let flat = astersql_planner_core::FlattenTypedPhysicalPlan(&execute)
        .expect("EXECUTE must retain the real target plan");
    assert_eq!(flat.len(), 1);
    assert!(
        flat[0]
            .Origin
            .as_any()
            .is::<physicalop::PhysicalTableDual>()
    );
    let classified = crate::statement_ru_result::classify_statement_ru_plan(&execute);
    assert!(
        classified
            .plan
            .as_any()
            .is::<physicalop::PhysicalTableDual>()
    );
    assert_eq!(
        classified.kind,
        crate::statement_ru_result::StatementRUPlanKind::Other
    );

    let explain = astersql_planner_core::RuntimeExplain::New(
        context.clone(),
        Box::new(physicalop::PhysicalTableDual::New(context, 1)),
        "row".to_owned(),
        true,
    );
    let flat = astersql_planner_core::FlattenTypedPhysicalPlan(&explain)
        .expect("EXPLAIN must retain its target for display");
    assert_eq!(flat.len(), 1);
    assert!(
        flat[0]
            .Origin
            .as_any()
            .is::<physicalop::PhysicalTableDual>()
    );
    let classified = crate::statement_ru_result::classify_statement_ru_plan(&explain);
    assert!(
        classified
            .plan
            .as_any()
            .is::<physicalop::PhysicalTableDual>()
    );

    let plain = astersql_planner_core::RuntimeExplain::New(
        execute.Plan.s_ctx().clone(),
        Box::new(physicalop::PhysicalTableDual::New(
            execute.Plan.s_ctx().clone(),
            1,
        )),
        "row".to_owned(),
        false,
    );
    let classified = crate::statement_ru_result::classify_statement_ru_plan(&plain);
    assert!(
        classified
            .plan
            .as_any()
            .is::<astersql_planner_core::RuntimeExplain>()
    );
    let mut insert = physicalop::Insert::New(execute.Plan.s_ctx().clone());
    assert_eq!(
        crate::statement_ru_result::classify_statement_ru_plan(&insert).sql_type,
        "insert"
    );
    insert.IsReplace = true;
    let classified = crate::statement_ru_result::classify_statement_ru_plan(&insert);
    assert_eq!(
        classified.kind,
        crate::statement_ru_result::StatementRUPlanKind::Write
    );
    assert_eq!(classified.sql_type, "replace");
    let update = physicalop::Update::New(
        execute.Plan.s_ctx().clone(),
        Box::new(physicalop::PhysicalTableDual::New(
            execute.Plan.s_ctx().clone(),
            1,
        )),
    );
    assert_eq!(
        crate::statement_ru_result::classify_statement_ru_plan(&update).sql_type,
        "update"
    );
    let delete = physicalop::Delete::New(
        execute.Plan.s_ctx().clone(),
        Box::new(physicalop::PhysicalTableDual::New(
            execute.Plan.s_ctx().clone(),
            1,
        )),
    );
    assert_eq!(
        crate::statement_ru_result::classify_statement_ru_plan(&delete).sql_type,
        "delete"
    );
    let point = physicalop::PointGetPlan::New(execute.Plan.s_ctx().clone());
    assert_eq!(
        crate::statement_ru_result::classify_statement_ru_plan(&point).kind,
        crate::statement_ru_result::StatementRUPlanKind::PointLookup
    );
}

#[test]
fn go_merge_187_typed_plan_forest() {
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let definition = Arc::new(physicalop::PhysicalCTEDefinition::New(
        context.clone(),
        17,
        Box::new(physicalop::PhysicalTableDual::New(context.clone(), 1)),
        Some(Box::new(physicalop::PhysicalTableDual::New(
            context.clone(),
            2,
        ))),
    ));
    let mut root = physicalop::BasePhysicalPlan::New(context.clone(), "Projection", 0);
    base::PhysicalPlan::set_children(
        &mut root,
        vec![
            Box::new(physicalop::PhysicalCTE::New(
                context.clone(),
                definition.clone(),
            )),
            Box::new(physicalop::PhysicalCTE::New(context.clone(), definition)),
        ],
    );
    let root: Box<dyn base::Plan> = Box::new(root);
    let scalar_plan: Arc<dyn base::PhysicalPlan> =
        Arc::new(physicalop::PhysicalTableDual::New(context.clone(), 3));
    let scalar = astersql_planner_core::ScalarSubqueryEvalCtx::New(
        context,
        0,
        scalar_plan,
        astersql_planner_core::context::BackgroundArc(),
        astersql_infoschema::infoschema::MockInfoSchema(Vec::new()),
    );
    let registered: Vec<Rc<dyn std::any::Any>> = vec![Rc::new(scalar)];
    let forest =
        astersql_planner_core::FlattenTypedPhysicalPlanForest(root.as_ref(), &registered).unwrap();
    assert_eq!(forest.Main.len(), 3);
    assert_eq!(forest.Main[0].ChildrenIdx, vec![1, 2]);
    assert_eq!(forest.CTEs.len(), 1);
    let cte = &forest.CTEs[0];
    assert_eq!(cte.len(), 3);
    assert!(
        cte[0]
            .Origin
            .as_any()
            .is::<physicalop::PhysicalCTEDefinition>()
    );
    assert_eq!(cte[0].ChildrenIdx, vec![1, 2]);
    assert_eq!(cte[0].ChildrenEndIdx, 2);
    assert_eq!(
        cte[1].Label,
        astersql_planner_core::TypedOperatorLabel::SeedPart
    );
    assert_eq!(
        cte[2].Label,
        astersql_planner_core::TypedOperatorLabel::RecursivePart
    );
    assert!(cte[1].IsRoot && cte[2].IsRoot);
    assert!(!cte[1].IsLastChild && cte[2].IsLastChild);
    assert_eq!(forest.ScalarSubQueries.len(), 1);
    let scalar_tree = &forest.ScalarSubQueries[0];
    assert_eq!(scalar_tree[0].ChildrenIdx, vec![1]);
    assert_eq!(scalar_tree[0].ChildrenEndIdx, 1);
    assert!(
        scalar_tree[0]
            .Origin
            .as_any()
            .is::<astersql_planner_core::ScalarSubqueryEvalCtx>()
    );
    assert!(
        scalar_tree[1]
            .Origin
            .as_any()
            .is::<physicalop::PhysicalTableDual>()
    );
}

#[test]
fn go_merge_187_typed_plan_bridge() {
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut scan = physicalop::PhysicalTableScan::New(context.clone());
    scan.PhysicalTableID = 42;
    scan.Desc = true;
    let mut root = physicalop::BasePhysicalPlan::New(context, "Projection", 0);
    base::PhysicalPlan::set_children(&mut root, vec![Box::new(scan)]);
    let root: Box<dyn base::Plan> = Box::new(root);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].ChildrenIdx, vec![1]);
    assert_eq!(tree[0].ChildrenEndIdx, 1);
    assert!(tree[0].IsRoot);
    let scan = tree[1]
        .Origin
        .as_any()
        .downcast_ref::<physicalop::PhysicalTableScan>()
        .unwrap();
    assert_eq!(scan.PhysicalTableID, 42);
    assert!(scan.Desc);

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut reader = physicalop::PhysicalTableReader::New(context.clone());
    reader.StoreType = astersql_kv::StoreType::TiFlash;
    reader.ReadReqType = physicalop::ReadReqType::MPP;
    base::PhysicalPlan::set_children(
        &mut reader,
        vec![Box::new(physicalop::PhysicalTableScan::New(context))],
    );
    let root: Box<dyn base::Plan> = Box::new(reader);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree.len(), 2);
    assert!(tree[0].IsRoot);
    assert!(!tree[1].IsRoot);
    assert_eq!(tree[1].StoreType, astersql_kv::StoreType::TiFlash);
    assert_eq!(tree[1].ReqType, physicalop::ReadReqType::MPP);

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut index_reader = physicalop::PhysicalIndexReader::New(context.clone());
    index_reader.IndexPlan = Some(Box::new(physicalop::PhysicalIndexScan::New(context)));
    let root: Box<dyn base::Plan> = Box::new(index_reader);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree[0].ChildrenIdx, vec![1]);
    assert!(!tree[1].IsRoot);
    assert_eq!(tree[1].StoreType, astersql_kv::StoreType::TiKV);
    assert_eq!(tree[1].ReqType, physicalop::ReadReqType::Cop);

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut lookup = physicalop::PhysicalIndexLookUpReader::New(context.clone());
    lookup.IndexPlan = Some(Box::new(physicalop::PhysicalIndexScan::New(
        context.clone(),
    )));
    lookup.TablePlan = Some(Box::new(physicalop::PhysicalTableScan::New(context)));
    let root: Box<dyn base::Plan> = Box::new(lookup);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree.len(), 3);
    assert_eq!(tree[0].ChildrenIdx, vec![1, 2]);
    assert_eq!(
        tree[1].Label,
        astersql_planner_core::TypedOperatorLabel::BuildSide
    );
    assert_eq!(
        tree[2].Label,
        astersql_planner_core::TypedOperatorLabel::ProbeSide
    );
    assert!(!tree[1].IsINLProbeChild);
    assert!(tree[2].IsINLProbeChild);

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let producer = physicalop::PhysicalSchemaProducer::New(physicalop::BasePhysicalPlan::New(
        context.clone(),
        "HashJoin",
        0,
    ));
    let base_join = physicalop::BasePhysicalJoin::New(producer, base::JoinType::InnerJoin);
    let mut join = physicalop::NewPhysicalHashJoin(base_join, 1, false);
    base::PhysicalPlan::set_children(
        &mut join,
        vec![
            Box::new(physicalop::PhysicalTableScan::New(context.clone())),
            Box::new(physicalop::PhysicalTableScan::New(context)),
        ],
    );
    let root: Box<dyn base::Plan> = Box::new(join);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree[0].ChildrenIdx, vec![1, 2]);
    assert!(tree[0].NeedReverseDriverSide);
    assert_eq!(
        tree[1].Label,
        astersql_planner_core::TypedOperatorLabel::ProbeSide
    );
    assert_eq!(
        tree[2].Label,
        astersql_planner_core::TypedOperatorLabel::BuildSide
    );

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut insert = physicalop::Insert::New(context.clone());
    insert.SelectPlan = Some(Box::new(physicalop::PhysicalTableScan::New(
        context.clone(),
    )));
    insert
        .FKChecks
        .push(Box::new(physicalop::FKCheck::New(context.clone())));
    let mut cascade =
        physicalop::FKCascade::New(context.clone(), physicalop::FKCascadeType::OnDelete);
    cascade
        .CascadePlans
        .push(Box::new(physicalop::PhysicalTableScan::New(context)));
    insert.FKCascades.push(Box::new(cascade));
    let root: Box<dyn base::Plan> = Box::new(insert);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree.len(), 5);
    assert_eq!(tree[0].ChildrenIdx, vec![1, 2, 3]);
    assert_eq!(tree[0].ChildrenEndIdx, 4);
    assert!(tree[1].IsRoot);
    assert!(tree[2].Origin.as_any().is::<physicalop::FKCheck>());
    assert!(!tree[1].IsLastChild);
    assert!(!tree[2].IsLastChild);
    assert!(tree[3].Origin.as_any().is::<physicalop::FKCascade>());
    assert!(tree[3].IsLastChild);
    assert_eq!(tree[3].ChildrenIdx, vec![4]);
    assert!(tree[4].IsRoot);

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let receiver = physicalop::PhysicalShuffleReceiverStub::New(
        context.clone(),
        Some(Box::new(physicalop::PhysicalTableScan::New(context))),
    );
    let root: Box<dyn base::Plan> = Box::new(receiver);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].ChildrenIdx, vec![1]);
    assert!(tree[1].IsRoot);
    assert!(tree[1].IsLastChild);

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut merge = physicalop::PhysicalIndexMergeReader::New(context.clone());
    merge.PartialPlansRaw = vec![
        Box::new(physicalop::PhysicalIndexScan::New(context.clone())),
        Box::new(physicalop::PhysicalIndexScan::New(context.clone())),
    ];
    merge.TablePlan = Some(Box::new(physicalop::PhysicalTableScan::New(context)));
    let root: Box<dyn base::Plan> = Box::new(merge);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree[0].ChildrenIdx, vec![1, 2, 3]);
    for partial in &tree[1..3] {
        assert!(!partial.IsRoot);
        assert_eq!(partial.StoreType, astersql_kv::StoreType::TiKV);
        assert_eq!(partial.ReqType, physicalop::ReadReqType::Cop);
        assert_eq!(
            partial.Label,
            astersql_planner_core::TypedOperatorLabel::BuildSide
        );
    }
    assert_eq!(
        tree[3].Label,
        astersql_planner_core::TypedOperatorLabel::ProbeSide
    );
    assert!(tree[3].IsINLProbeChild);
    assert!(tree[3].IsLastChild);

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let update = physicalop::Update::New(
        context.clone(),
        Box::new(physicalop::PhysicalTableScan::New(context.clone())),
    );
    let root: Box<dyn base::Plan> = Box::new(update);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree[0].ChildrenIdx, vec![1]);
    assert!(tree[1].IsRoot);
    let delete = physicalop::Delete::New(
        context.clone(),
        Box::new(physicalop::PhysicalTableScan::New(context)),
    );
    let root: Box<dyn base::Plan> = Box::new(delete);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(root.as_ref()).unwrap();
    assert_eq!(tree[0].ChildrenIdx, vec![1]);
    assert!(tree[1].IsRoot);
}

#[test]
fn go_merge_187_snapshot_writes_copies_current_commit_payload() {
    assert_eq!(
        snapshot_statement_ru_writes(None),
        StatementRUWriteSnapshot::default()
    );
    let details = tikvutil::CommitDetails {
        WriteKeys: 7,
        WriteSize: 4096,
        ..Default::default()
    };
    assert_eq!(
        snapshot_statement_ru_writes(Some(&details)),
        StatementRUWriteSnapshot {
            keys: 7,
            bytes: 4096
        }
    );
    let overflow = tikvutil::CommitDetails {
        WriteKeys: u64::MAX,
        WriteSize: u64::MAX,
        ..Default::default()
    };
    assert_eq!(
        snapshot_statement_ru_writes(Some(&overflow)),
        StatementRUWriteSnapshot {
            keys: -1,
            bytes: -1
        }
    );
}

#[test]
fn go_merge_187_terminal_calculator_adds_response_bytes_once() {
    let metrics = NewRUV2Metrics();
    metrics.AddTiKVCoprocessorResponseBytes(20);
    let setup = StatementRUCalculationSetup {
        frontend_compile_bytes: 5.0,
        full_report: true,
    };
    assert!(new_statement_ru_terminal_calculator(Some(&metrics), setup, false).is_none());
    let calculator = new_statement_ru_terminal_calculator(Some(&metrics), setup, true).unwrap();
    assert_eq!(calculator.units.frontend_compile_bytes, 5.0);
    assert_eq!(calculator.units.net_bytes, 20.0);
    assert_eq!(
        calculator.report.unwrap().units[StatementRUEngine::TiKV as usize]
            [StatementRUOperator::CopTransport as usize]
            .net_bytes,
        20.0
    );

    metrics.SetBypass(true);
    assert_eq!(
        new_statement_ru_terminal_calculator(Some(&metrics), setup, true)
            .unwrap()
            .units
            .net_bytes,
        0.0
    );
    metrics.SetBypass(false);
    metrics.AddTiKVCoprocessorResponseBytes(-30);
    assert!(new_statement_ru_terminal_calculator(Some(&metrics), setup, true).is_none());
}

#[test]
fn analyze_plan_combines_logical_scan_estimates_and_transport_bytes() {
    use crate::statement_ru_plan_walk::{
        calculate_statement_ru_plan, snapshot_statement_ru_runtime_evidence,
    };

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        Default::default(),
        Default::default(),
    ));
    let analyze = astersql_planner_core::RuntimeAnalyze::New(context, Default::default());
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&analyze).unwrap();
    let analyze_id = base::Plan::id(&analyze);
    let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    stats.RecordAnalyzeScanBytes(analyze_id, 1000.0);
    stats.RecordAnalyzeScanBytes(analyze_id, 9.0);
    let metrics = NewRUV2Metrics();
    metrics.AddTiKVCoprocessorResponseBytes(29);
    let evidence = snapshot_statement_ru_runtime_evidence(
        Some(&stats),
        &[analyze_id],
        None,
        None,
        Some(&metrics),
    );
    let mut calculator = new_statement_ru_terminal_calculator(
        Some(&metrics),
        StatementRUCalculationSetup {
            full_report: true,
            ..Default::default()
        },
        true,
    )
    .unwrap();

    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
        StatementRUOperatorState::Complete
    );
    assert_eq!(calculator.units.scan_bytes, 1009.0);
    assert_eq!(calculator.units.net_bytes, 29.0);
}

#[test]
fn go_merge_187_owner_records_first_outcome_and_consumes_terminal_once() {
    let setup = StatementRUCalculationSetup {
        frontend_compile_bytes: 5.0,
        full_report: true,
    };
    let owner = StatementRUOwner::new(setup, false, false, false);
    assert_eq!(owner.final_outcome(), StatementRUFinalOutcome::Unknown);
    assert!(!owner.root_eof());
    owner.record_root_eof();
    assert!(owner.root_eof());
    assert!(owner.record_final_outcome(true));
    assert!(!owner.record_final_outcome(false));
    assert_eq!(owner.final_outcome(), StatementRUFinalOutcome::Success);
    assert_eq!(owner.take_terminal_setup(), Some(setup));
    assert_eq!(owner.take_terminal_setup(), None);

    let failed = StatementRUOwner::new(setup, false, false, false);
    assert!(failed.record_final_outcome(false));
    assert!(!failed.record_final_outcome(true));
    assert_eq!(failed.final_outcome(), StatementRUFinalOutcome::Failure);
    assert_eq!(failed.take_terminal_setup(), None);
}

#[test]
fn go_merge_187_flat_tree_validation_requires_canonical_depth_first_order() {
    let plan = PlanNode::New(
        1,
        PlanKind::Projection,
        vec![
            PlanNode::New(2, PlanKind::Dual, vec![]),
            PlanNode::New(3, PlanKind::Dual, vec![]),
        ],
    );
    let flat = FlattenPhysicalPlan(Some(&plan), false).unwrap();
    assert!(validate_statement_ru_flat_tree(&flat.Main));
    assert!(!validate_statement_ru_flat_tree(&vec![]));

    let mut duplicate = flat.Main.clone();
    duplicate[0].ChildrenIdx[1] = 1;
    assert!(!validate_statement_ru_flat_tree(&duplicate));

    let mut skipped = flat.Main.clone();
    skipped[0].ChildrenIdx[0] = 2;
    assert!(!validate_statement_ru_flat_tree(&skipped));

    let mut unreachable = flat.Main.clone();
    unreachable[0].ChildrenIdx.pop();
    assert!(!validate_statement_ru_flat_tree(&unreachable));
}

#[test]
fn go_merge_187_sort_work_and_atomic_unit_delta_match_go() {
    assert_eq!(statement_ru_sort_work(8, 4), 16.0);
    assert_eq!(statement_ru_sort_work(8, 1), 8.0);
    assert_eq!(statement_ru_sort_work(8, 0), 0.0);
    assert_eq!(statement_ru_sort_work(-1, 4), 0.0);

    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup::default());
    assert!(merge_statement_ru_unit_delta(
        &mut calculator,
        StmtUnits {
            cpu_work: 2.0,
            scan_bytes: 3.0,
            hash_state_rows: 4.0,
            join_output_rows: 5.0,
            ..Default::default()
        }
    ));
    assert_eq!(calculator.units.cpu_work, 2.0);
    assert_eq!(calculator.units.join_output_rows, 5.0);
    let before = calculator.units;
    assert!(!merge_statement_ru_unit_delta(
        &mut calculator,
        StmtUnits {
            cpu_work: 1.0,
            join_output_rows: f64::INFINITY,
            ..Default::default()
        }
    ));
    assert_eq!(calculator.units, before);
    calculator.units.scan_bytes = f64::MAX;
    let before = calculator.units;
    assert!(!merge_statement_ru_unit_delta(
        &mut calculator,
        StmtUnits {
            cpu_work: 1.0,
            scan_bytes: f64::MAX,
            ..Default::default()
        }
    ));
    assert_eq!(calculator.units, before);
}

#[test]
fn go_merge_187_operator_state_merge_matches_go_priority() {
    use StatementRUOperatorState::{Complete, Invalid, Unknown, Unsupported};
    assert_eq!(
        merge_statement_ru_operator_state(Complete, Complete),
        Complete
    );
    assert_eq!(
        merge_statement_ru_operator_state(Unknown, Complete),
        Unsupported
    );
    assert_eq!(
        merge_statement_ru_operator_state(Complete, Unsupported),
        Unsupported
    );
    assert_eq!(
        merge_statement_ru_operator_state(Invalid, Unsupported),
        Invalid
    );
    assert_eq!(
        merge_statement_ru_operator_state(Complete, Invalid),
        Invalid
    );
    assert_eq!(
        statement_ru_failed(Unsupported),
        StatementRUFailureReason::Unsupported
    );
    assert_eq!(
        statement_ru_failed(Invalid),
        StatementRUFailureReason::Invalid
    );
    assert_eq!(
        statement_ru_terminal_failure(false),
        StatementRUFailureReason::NotFinished
    );
    assert_eq!(
        statement_ru_terminal_failure(true),
        StatementRUFailureReason::Invalid
    );
}

#[test]
fn go_merge_187_runtime_evidence_bridge() {
    use crate::statement_ru_plan_walk::{
        StatementRUPointSnapshot, snapshot_statement_ru_runtime_evidence,
    };
    use astersql_util_execdetails::execdetails::{RuntimeStatsColl, kv, util};
    let mut coll = RuntimeStatsColl::default();
    coll.RecordCopStats(
        7,
        kv::TiKV,
        Some(&util::ScanDetail {
            TotalKeys: 6,
            ProcessedKeys: 2,
            ProcessedKeysSize: 8,
            ..Default::default()
        }),
        Default::default(),
        None,
        None,
    );
    coll.GetBasicRuntimeStats(7, true)
        .unwrap()
        .Record(std::time::Duration::ZERO, 0);
    coll.RecordTiFlashExecutionSummaries(
        &[9],
        &[Some(
            astersql_util_execdetails::execdetails::tipb::ExecutorExecutionSummary {
                ExecutorId: "TableScan_9".into(),
                NumProducedRows: Some(0),
                TiflashScanContext: Some(
                    astersql_util_execdetails::execdetails::tipb::TiFlashScanContext {
                        UserReadBytes: Some(0),
                    },
                ),
                ..Default::default()
            },
        )],
    );
    coll.RegisterStatsShared(
        7,
        Box::new(astersql_util_execdetails::execdetails::WriteRuntimeStats { CPUWork: 3.0 }),
    );
    let hash = astersql_util_execdetails::execdetails::HashStateRuntimeStats::default();
    hash.AddRows(4);
    coll.RegisterStatsShared(7, Box::new(hash));
    let mut commit = tikvutil::CommitDetails {
        WriteKeys: 2,
        WriteSize: 58,
        ..Default::default()
    };
    let metrics = NewRUV2Metrics();
    metrics.AddTiKVCoprocessorResponseBytes(19);
    let point = StatementRUPointSnapshot {
        total_keys: 6,
        processed_keys: 2,
        processed_bytes: 8,
        payload_bytes: 3,
        valid: true,
        payload_complete: true,
        scan_detail_complete: true,
    };
    let frozen = snapshot_statement_ru_runtime_evidence(
        Some(&coll),
        &[7, 8, 9],
        Some(point),
        Some(snapshot_statement_ru_writes(Some(&commit))),
        Some(&metrics),
    );
    assert_eq!(
        frozen.writes,
        Some(StatementRUWriteSnapshot { keys: 2, bytes: 58 })
    );
    assert_eq!(frozen.plans[0].write_cpu_work, Some(3.0));
    assert_eq!(frozen.plans[0].hash_state_rows.unwrap().Rows, 4);
    assert!(frozen.plans[1].write_cpu_work.is_none() && frozen.plans[1].hash_state_rows.is_none());
    assert_eq!(frozen.tikv_response_bytes, Some(19));
    assert_eq!(frozen.point, Some(point));
    assert_eq!(frozen.plans[0].scan.as_ref().unwrap().ProcessedKeysSize, 8);
    assert!(frozen.plans[1].scan.is_none());
    assert!(frozen.plans[1].tiflash.is_none());
    assert!(frozen.plans[0].root_rows.Observed());
    assert_eq!(frozen.plans[0].root_rows.Rows, 0);
    assert!(!frozen.plans[1].root_rows.Observed());
    let tiflash = frozen.plans[2].tiflash.unwrap();
    assert_eq!(tiflash.UserReadBytes, 0);
    assert_ne!(
        tiflash.Observed & astersql_util_execdetails::execdetails::TiFlashUnitScan,
        0
    );
    assert_eq!(frozen.point_state(), StatementRUOperatorState::Complete);
    assert_eq!(
        frozen.tikv_response_state(),
        StatementRUOperatorState::Complete
    );
    commit.WriteSize = 999;
    metrics.AddTiKVCoprocessorResponseBytes(99);
    assert_eq!(frozen.writes.unwrap().bytes, 58);
    assert_eq!(frozen.tikv_response_bytes, Some(19));
    let absent = snapshot_statement_ru_runtime_evidence(None, &[7], None, None, None);
    assert!(
        absent.plans.is_empty()
            && absent.point.is_none()
            && absent.writes.is_none()
            && absent.tikv_response_bytes.is_none()
    );
    assert_eq!(absent.point_state(), StatementRUOperatorState::Unsupported);
    assert_eq!(
        absent.tikv_response_state(),
        StatementRUOperatorState::Unsupported
    );
    let zero = snapshot_statement_ru_runtime_evidence(
        Some(&coll),
        &[],
        None,
        Some(StatementRUWriteSnapshot::default()),
        Some(&NewRUV2Metrics()),
    );
    assert_eq!(zero.writes, Some(StatementRUWriteSnapshot::default()));
    assert_eq!(zero.tikv_response_bytes, Some(0));
    assert_eq!(point.state(), StatementRUOperatorState::Complete);
    assert_eq!(
        StatementRUPointSnapshot {
            scan_detail_complete: false,
            ..point
        }
        .state(),
        StatementRUOperatorState::Unsupported
    );
    assert_eq!(
        StatementRUPointSnapshot {
            valid: false,
            ..point
        }
        .state(),
        StatementRUOperatorState::Invalid
    );
}

#[test]
fn go_merge_187_tree_read_path() {
    use crate::statement_ru_plan_walk::calculate_statement_ru_forest;
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut reader = physicalop::PhysicalTableReader::New(context.clone());
    reader.TablePlan = Some(Box::new(physicalop::PhysicalTableScan::New(
        context.clone(),
    )));
    let mut forest = astersql_planner_core::FlattenTypedPhysicalPlanForest(&reader, &[]).unwrap();
    let stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    let mut evidence = crate::statement_ru_plan_walk::snapshot_statement_ru_runtime_evidence(
        Some(&stats),
        &[forest.Main[0].Origin.id(), forest.Main[1].Origin.id()],
        None,
        None,
        None,
    );
    evidence.plans[1].scan = Some(astersql_util_execdetails::execdetails::util::ScanDetail {
        TotalKeys: 10,
        ProcessedKeys: 2,
        ProcessedKeysSize: 8,
        ..Default::default()
    });
    evidence.tikv_response_bytes = Some(13);
    let result = calculate_statement_ru_forest(
        &forest,
        &evidence,
        StatementRUCalculationSetup::default(),
        true,
    )
    .unwrap();
    assert_eq!(result.units.scan_bytes, 40.0);
    assert_eq!(result.units.net_bytes, 13.0);
    assert_eq!(result.units.operator_num, 2.0);
    evidence.plans[1].scan.as_mut().unwrap().ProcessedKeys = -1;
    assert_eq!(
        calculate_statement_ru_forest(
            &forest,
            &evidence,
            StatementRUCalculationSetup::default(),
            true
        )
        .unwrap_err(),
        StatementRUOperatorState::Invalid
    );
    evidence.plans[1].scan = None;
    assert_eq!(
        calculate_statement_ru_forest(
            &forest,
            &evidence,
            StatementRUCalculationSetup::default(),
            true
        )
        .unwrap()
        .units
        .scan_bytes,
        0.0
    );
    forest.Main[1].IsRoot = true;
    assert_eq!(
        calculate_statement_ru_forest(
            &forest,
            &evidence,
            StatementRUCalculationSetup::default(),
            true
        )
        .unwrap_err(),
        StatementRUOperatorState::Unsupported
    );
    forest.Main[1].IsRoot = false;
    forest.Main[0].ChildrenIdx = vec![0];
    assert_eq!(
        calculate_statement_ru_forest(
            &forest,
            &evidence,
            StatementRUCalculationSetup::default(),
            true
        )
        .unwrap_err(),
        StatementRUOperatorState::Invalid
    );
}

#[test]
fn go_merge_187_tree_read_path_point_and_forest() {
    use crate::statement_ru_plan_walk::*;
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let point = physicalop::PointGetPlan::New(context.clone());
    let batch = physicalop::BatchPointGetPlan::New(context.clone());
    let dual = physicalop::PhysicalTableDual::New(context.clone(), 99);
    let forest = astersql_planner_core::TypedFlatPhysicalPlan {
        Main: astersql_planner_core::FlattenTypedPhysicalPlan(&point).unwrap(),
        CTEs: vec![astersql_planner_core::FlattenTypedPhysicalPlan(&dual).unwrap()],
        ScalarSubQueries: vec![astersql_planner_core::FlattenTypedPhysicalPlan(&batch).unwrap()],
    };
    let snapshot = StatementRUPointSnapshot {
        total_keys: 10,
        processed_keys: 2,
        processed_bytes: 8,
        payload_bytes: 7,
        valid: true,
        payload_complete: true,
        scan_detail_complete: true,
    };
    let mut evidence = StatementRURuntimeEvidence::default();
    evidence.points = vec![
        (forest.Main[0].Origin.id(), snapshot),
        (forest.ScalarSubQueries[0][0].Origin.id(), snapshot),
    ];
    evidence.tikv_response_bytes = Some(11);
    let setup = StatementRUCalculationSetup {
        full_report: true,
        frontend_compile_bytes: 4.0,
    };
    let value = calculate_statement_ru_forest(&forest, &evidence, setup, true).unwrap();
    assert_eq!(value.units.scan_bytes, 80.0);
    assert_eq!(value.units.net_bytes, 25.0);
    assert_eq!(value.units.operator_num, 3.0);
    assert_eq!(value.units.frontend_compile_bytes, 4.0);
    let report = value.report.unwrap();
    assert_eq!(
        report.units[StatementRUEngine::TiKV as usize][StatementRUOperator::PointLookup as usize]
            .scan_bytes,
        80.0
    );
    assert_eq!(
        report.units[StatementRUEngine::TiKV as usize][StatementRUOperator::PointLookup as usize]
            .net_bytes,
        14.0
    );
    evidence.points[1].1.scan_detail_complete = false;
    assert_eq!(
        calculate_statement_ru_forest(&forest, &evidence, setup, true).unwrap_err(),
        StatementRUOperatorState::Unsupported
    );
    evidence.points[1].1 = snapshot;
    evidence.points[1].1.processed_bytes = -1;
    assert_eq!(
        calculate_statement_ru_forest(&forest, &evidence, setup, true).unwrap_err(),
        StatementRUOperatorState::Invalid
    );
    evidence.points.clear();
    evidence.point = Some(snapshot);
    assert_eq!(
        calculate_statement_ru_forest(&forest, &evidence, setup, true).unwrap_err(),
        StatementRUOperatorState::Unsupported
    );
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&point).unwrap();
    let mut calculator = StatementRUCalculator::new(setup);
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
        StatementRUOperatorState::Complete
    );
    evidence.point = Some(StatementRUPointSnapshot {
        valid: true,
        ..Default::default()
    });
    let mut calculator = StatementRUCalculator::new(setup);
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
        StatementRUOperatorState::Complete
    );
    assert_eq!(calculator.units.scan_bytes, 0.0);
    assert_eq!(calculator.units.net_bytes, 0.0);
    evidence.point = None;
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
        StatementRUOperatorState::Unsupported
    );
}

#[test]
fn go_merge_187_tree_read_path_readers_and_child_first() {
    use crate::statement_ru_plan_walk::*;
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut index = physicalop::PhysicalIndexReader::New(context.clone());
    index.IndexPlan = Some(Box::new(physicalop::PhysicalIndexScan::New(
        context.clone(),
    )));
    let mut lookup = physicalop::PhysicalIndexLookUpReader::New(context.clone());
    lookup.IndexPlan = Some(Box::new(physicalop::PhysicalIndexScan::New(
        context.clone(),
    )));
    lookup.TablePlan = Some(Box::new(physicalop::PhysicalTableScan::New(
        context.clone(),
    )));
    let mut merge = physicalop::PhysicalIndexMergeReader::New(context.clone());
    merge.PartialPlansRaw = vec![
        Box::new(physicalop::PhysicalIndexScan::New(context.clone())),
        Box::new(physicalop::PhysicalIndexScan::New(context.clone())),
    ];
    merge.TablePlan = Some(Box::new(physicalop::PhysicalTableScan::New(
        context.clone(),
    )));
    for (plan, expected) in [
        (&index as &dyn base::Plan, 40.0),
        (&lookup as &dyn base::Plan, 80.0),
        (&merge as &dyn base::Plan, 120.0),
    ] {
        let mut tree = astersql_planner_core::FlattenTypedPhysicalPlan(plan).unwrap();
        let stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
        let mut evidence =
            snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
        for plan in evidence.plans.iter_mut().skip(1) {
            plan.scan = Some(astersql_util_execdetails::execdetails::util::ScanDetail {
                TotalKeys: 10,
                ProcessedKeys: 2,
                ProcessedKeysSize: 8,
                ..Default::default()
            });
            plan.cop_rows.Rows = 3;
        }
        let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup::default());
        let mut results = vec![StatementRUExplainOperatorResult::default(); tree.len()];
        let root_owned = StmtUnits {
            net_bytes: 13.0,
            ..Default::default()
        };
        let result = calculate_statement_ru_plan_with_operators(
            &tree,
            0,
            &evidence,
            &mut calculator,
            root_owned,
            Some(&mut results),
        );
        assert_eq!(result.state, StatementRUOperatorState::Complete);
        assert_eq!(calculator.units.scan_bytes, expected);
        assert_eq!(calculator.units.operator_num, tree.len() as f64);
        let weights = crate::statement_ru_result::current_statement_ru_weights();
        assert_eq!(
            results[0].cum_ru,
            astersql_resourcegroup::ruv2::model::calculate(
                calculator.units.add(root_owned),
                weights
            )
            .unwrap()
            .total_ru
        );
        // The children must finish before their parent owns scan charges.
        assert!(results[0].cum_ru > results[0].self_ru);
        for child in results.iter().skip(1) {
            assert_eq!(child.self_ru, child.cum_ru);
        }
        let mut explain = StatementRUExplainResult {
            main: results,
            ctes: vec![vec![Default::default()]],
            scalar_subqueries: vec![vec![Default::default()]],
        };
        assert_eq!(
            statement_ru_explain_tree(Some(&mut explain), StatementRUForestKind::Main, 99)
                .unwrap()
                .len(),
            tree.len()
        );
        assert!(
            statement_ru_explain_tree(Some(&mut explain), StatementRUForestKind::CTE, 1).is_none()
        );
        assert!(
            statement_ru_explain_tree(Some(&mut explain), StatementRUForestKind::ScalarSubQuery, 0)
                .is_some()
        );
        // An invalid second child wins over an unsupported first child.
        if tree.len() > 2 {
            tree[1].IsRoot = true;
            evidence.plans[2].cop_rows.Invalid = true;
            assert_eq!(
                calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
                StatementRUOperatorState::Invalid
            );
            evidence.plans[2].cop_rows.Invalid = false;
            assert_eq!(
                calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
                StatementRUOperatorState::Unsupported
            );
        }
    }
}

#[test]
fn go_merge_187_row_and_write_operators() {
    use crate::statement_ru_plan_walk::*;
    use base::Plan;
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let mut projection = physicalop::PhysicalProjection::New(context.clone());
    projection.Exprs = vec![
        Box::new(astersql_expression::Column::default()),
        Box::new(astersql_expression::Column::default()),
    ];
    let mut selection = physicalop::PhysicalSelection::New(context.clone());
    selection.Conditions = (0..3)
        .map(|_| Box::new(astersql_expression::Column::default()) as astersql_expression::ExprBox)
        .collect();
    let mut window = physicalop::PhysicalWindow::New(context.clone());
    window.WindowFuncDescs = vec![Default::default(); 2];
    use astersql_planner_core_operator_logicalop::{FrameBound, WindowFrame};
    window.Frame = Some(WindowFrame {
        Type: Default::default(),
        Start: Some(FrameBound {
            CalcFuncs: vec![Box::new(astersql_expression::Column::default())],
            ..Default::default()
        }),
        End: Some(FrameBound {
            CalcFuncs: vec![Box::new(astersql_expression::Column::default())],
            ..Default::default()
        }),
    });
    let plans: Vec<(Box<dyn base::PhysicalPlan>, StatementRUOperator, f64)> = vec![
        (Box::new(projection), StatementRUOperator::Projection, 16.0),
        (Box::new(selection), StatementRUOperator::Selection, 24.0),
        (
            Box::new(physicalop::PhysicalLimit::New(context.clone(), 0, 1)),
            StatementRUOperator::Limit,
            8.0,
        ),
        (
            Box::new(physicalop::PhysicalMaxOneRow::New(context.clone())),
            StatementRUOperator::Limit,
            8.0,
        ),
        (
            Box::new(physicalop::PhysicalUnionScan::New(context.clone())),
            StatementRUOperator::UnionScan,
            8.0,
        ),
        (
            Box::new(physicalop::PhysicalSort::New(context.clone())),
            StatementRUOperator::Sort,
            24.0,
        ),
        (
            Box::new(physicalop::PhysicalTopN::New(context.clone(), 1, 3)),
            StatementRUOperator::TopN,
            16.0,
        ),
        (Box::new(window), StatementRUOperator::Window, 32.0),
    ];
    for (mut plan, label, work) in plans {
        base::PhysicalPlan::set_children(
            plan.as_mut(),
            vec![Box::new(physicalop::PhysicalTableDual::New(
                context.clone(),
                999,
            ))],
        );
        let mut tree = astersql_planner_core::FlattenTypedPhysicalPlan(plan.as_ref()).unwrap();
        let stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
        let mut evidence =
            snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
        evidence.plans[0].root_rows.Rows = 1;
        evidence.plans[1].root_rows.Rows = 8;
        let make_calculator = || {
            StatementRUCalculator::new(StatementRUCalculationSetup {
                full_report: true,
                ..Default::default()
            })
        };
        let mut calculator = make_calculator();
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
            StatementRUOperatorState::Complete,
            "{label:?}"
        );
        assert_eq!(calculator.units.cpu_work, work, "{label:?}");
        assert_eq!(calculator.units.scan_bytes, 0.0);
        let report = calculator.report.unwrap();
        assert_eq!(
            report.units[StatementRUEngine::TiDB as usize][label as usize].cpu_work,
            work
        );
        assert!(report.seen[StatementRUEngine::TiDB as usize][label as usize]);
        evidence.plans[1].root_rows.Rows = 0;
        let mut calculator = make_calculator();
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
            StatementRUOperatorState::Complete
        );
        assert_eq!(calculator.units.cpu_work, 0.0);
        evidence.plans[1].root_rows.Rows = -1;
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut make_calculator()).state,
            StatementRUOperatorState::Invalid
        );
        evidence.plans[1].root_rows.Rows = 8;
        tree[0].ChildrenIdx.clear();
        tree[0].ChildrenEndIdx = 0;
        assert_ne!(
            calculate_statement_ru_plan(&tree[..1], 0, &evidence, &mut make_calculator()).state,
            StatementRUOperatorState::Complete
        );
    }
    let plans: Vec<Box<dyn base::Plan>> = vec![
        Box::new(physicalop::Insert::New(context.clone())),
        Box::new(physicalop::Update::New(
            context.clone(),
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 0)),
        )),
        Box::new(physicalop::Delete::New(
            context.clone(),
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 0)),
        )),
    ];
    for plan in plans {
        let tree = astersql_planner_core::FlattenTypedPhysicalPlan(plan.as_ref()).unwrap();
        let stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        let mut evidence =
            snapshot_statement_ru_runtime_evidence(Some(&stats), &[plan.id()], None, None, None);
        for (work, state) in [
            (None, StatementRUOperatorState::Unsupported),
            (Some(0.0), StatementRUOperatorState::Complete),
            (Some(6.0), StatementRUOperatorState::Complete),
            (Some(-1.0), StatementRUOperatorState::Invalid),
            (Some(f64::INFINITY), StatementRUOperatorState::Invalid),
            (Some(f64::NAN), StatementRUOperatorState::Invalid),
        ] {
            evidence.plans[0].write_cpu_work = work;
            let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup {
                full_report: true,
                ..Default::default()
            });
            assert_eq!(
                calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
                state
            );
            if state == StatementRUOperatorState::Complete {
                assert_eq!(calculator.units.cpu_work, work.unwrap());
                assert!(calculator.report.unwrap().seen[0][StatementRUOperator::Write as usize]);
            }
        }
    }
    let analyze = astersql_planner_core::RuntimeAnalyze::New(context, Default::default());
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&analyze).unwrap();
    let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    stats.RecordAnalyzeScanBytes(analyze.id(), 12.0);
    stats.RecordAnalyzeScanBytes(analyze.id(), 28.0);
    let mut evidence =
        snapshot_statement_ru_runtime_evidence(Some(&stats), &[analyze.id()], None, None, None);
    stats.RecordAnalyzeScanBytes(analyze.id(), 100.0);
    for (bytes, state) in [
        (Some(40.0), StatementRUOperatorState::Complete),
        (None, StatementRUOperatorState::Complete),
        (Some(-1.0), StatementRUOperatorState::Invalid),
        (Some(f64::INFINITY), StatementRUOperatorState::Invalid),
    ] {
        evidence.plans[0].analyze_scan_bytes = bytes;
        let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup {
            full_report: true,
            ..Default::default()
        });
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
            state
        );
        if state == StatementRUOperatorState::Complete {
            assert_eq!(calculator.units.scan_bytes, bytes.unwrap_or(0.0));
            assert!(
                calculator.report.as_ref().unwrap().seen[0][StatementRUOperator::Analyze as usize]
            );
            assert_eq!(
                calculator.report.unwrap().units[1][StatementRUOperator::Analyze as usize]
                    .scan_bytes,
                bytes.unwrap_or(0.0)
            );
        }
    }
}

#[test]
fn mem_table_and_lock_preserve_child_ru_work() {
    use crate::statement_ru_plan_walk::*;
    use base::PhysicalPlan as _;

    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        Default::default(),
        Default::default(),
    ));
    for rows in [0, 3] {
        let mem_table = physicalop::PhysicalMemTable::New(context.clone());
        let mut projection = physicalop::PhysicalProjection::New(context.clone());
        projection.Exprs = vec![Box::new(astersql_expression::Column::default())];
        projection.set_children(vec![Box::new(mem_table)]);
        let mut lock =
            physicalop::LegacyPhysicalLock::New(context.clone(), "for update".to_owned(), 0);
        lock.set_children(vec![Box::new(projection)]);

        let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&lock).unwrap();
        let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        for operator in &tree {
            stats
                .GetBasicRuntimeStats(operator.Origin.id(), true)
                .unwrap()
                .Record(std::time::Duration::ZERO, rows);
        }
        let ids: Vec<_> = tree.iter().map(|operator| operator.Origin.id()).collect();
        let evidence = snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
        let mut calculator = StatementRUCalculator::new(Default::default());
        let result = calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator);
        assert_eq!(result.state, StatementRUOperatorState::Complete);
        assert_eq!(result.output_rows, rows as i64);
        assert_eq!(calculator.units.cpu_work, rows as f64);
        assert_eq!(calculator.units.operator_num, 3.0);
    }
}

#[test]
fn go_merge_187_row_and_write_operators_topn_boundaries() {
    use crate::statement_ru_plan_walk::*;
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    for (offset, count, root, expected, work) in [
        (u64::MAX, 1, true, StatementRUOperatorState::Invalid, 0.0),
        (u64::MAX, 0, true, StatementRUOperatorState::Complete, 0.0),
        (1, 3, false, StatementRUOperatorState::Unsupported, 0.0),
        (
            0,
            3,
            false,
            StatementRUOperatorState::Complete,
            8.0 * 3f64.log2(),
        ),
        (0, 1, true, StatementRUOperatorState::Complete, 8.0),
    ] {
        let mut topn = physicalop::PhysicalTopN::New(context.clone(), offset, count);
        base::PhysicalPlan::set_children(
            &mut topn,
            vec![Box::new(physicalop::PhysicalTableDual::New(
                context.clone(),
                999,
            ))],
        );
        let mut tree = astersql_planner_core::FlattenTypedPhysicalPlan(&topn).unwrap();
        tree[0].IsRoot = root;
        tree[0].StoreType = astersql_kv::StoreType::TiKV;
        tree[0].ReqType = physicalop::ReadReqType::Cop;
        let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        stats
            .GetBasicRuntimeStats(tree[1].Origin.id(), true)
            .unwrap()
            .Record(std::time::Duration::ZERO, 8);
        let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
        let evidence = snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
        let mut calculator = StatementRUCalculator::new(Default::default());
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
            expected
        );
        assert_eq!(calculator.units.cpu_work, work);
    }
    let mut union = physicalop::PhysicalUnionAll::New(context.clone());
    base::PhysicalPlan::set_children(
        &mut union,
        vec![
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 99)),
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 99)),
        ],
    );
    let mut tree = astersql_planner_core::FlattenTypedPhysicalPlan(&union).unwrap();
    let mut calculator = StatementRUCalculator::new(Default::default());
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &Default::default(), &mut calculator).state,
        StatementRUOperatorState::Complete
    );
    assert_eq!(calculator.units.cpu_work, 0.0);
    assert_eq!(calculator.units.operator_num, 3.0);
    tree[0].IsRoot = false;
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &Default::default(), &mut calculator).state,
        StatementRUOperatorState::Unsupported
    );
}

#[test]
fn go_merge_187_row_and_write_operators_forest_snapshot() {
    use crate::statement_ru_plan_walk::*;
    use base::Plan;
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        Default::default(),
        Default::default(),
    ));
    let insert = physicalop::Insert::New(context.clone());
    let commit = astersql_planner_core::RuntimeSimple::New(
        context.clone(),
        astersql_parser_ast::NodeRef::new(Box::new(astersql_parser_ast::CommitStmt::default())),
    );
    let other = astersql_planner_core::RuntimeSimple::New(
        context,
        astersql_parser_ast::NodeRef::new(Box::new(astersql_parser_ast::RollbackStmt::default())),
    );
    for (plan, sql_type, write_statement) in [
        (&insert as &dyn base::Plan, "insert", 1.0),
        (&commit as &dyn base::Plan, "commit", 0.0),
    ] {
        let forest = astersql_planner_core::FlattenTypedPhysicalPlanForest(plan, &[]).unwrap();
        let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        stats.RegisterStatsShared(
            plan.id(),
            Box::new(astersql_util_execdetails::execdetails::WriteRuntimeStats { CPUWork: 6.0 }),
        );
        let evidence = snapshot_statement_ru_runtime_evidence(
            Some(&stats),
            &[plan.id()],
            None,
            Some(StatementRUWriteSnapshot { keys: 2, bytes: 58 }),
            None,
        );
        stats.RegisterStatsShared(
            plan.id(),
            Box::new(astersql_util_execdetails::execdetails::WriteRuntimeStats { CPUWork: 100.0 }),
        );
        let result = calculate_statement_ru_forest(
            &forest,
            &evidence,
            StatementRUCalculationSetup {
                full_report: true,
                ..Default::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(result.sql_type, sql_type);
        assert_eq!(result.units.write_keys, 2.0);
        assert_eq!(result.units.write_bytes, 58.0);
        assert_eq!(result.units.write_statement, write_statement);
        assert_eq!(
            result.units.cpu_work,
            if write_statement == 1.0 { 6.0 } else { 0.0 }
        );
        assert_eq!(
            result.report.unwrap().units[StatementRUEngine::TiKV as usize]
                [StatementRUOperator::KVWrite as usize]
                .write_bytes,
            58.0
        );
        let mut invalid = evidence.clone();
        invalid.writes.as_mut().unwrap().bytes = -1;
        assert!(
            calculate_statement_ru_forest(&forest, &invalid, Default::default(), true).is_err()
        );
    }
    let forest = astersql_planner_core::FlattenTypedPhysicalPlanForest(&other, &[]).unwrap();
    assert!(matches!(
        calculate_statement_ru_forest(&forest, &Default::default(), Default::default(), true),
        Err(StatementRUOperatorState::Unsupported)
    ));
}

#[test]
fn go_merge_187_join_aggregation() {
    use crate::statement_ru_plan_walk::*;
    let context: base::ContextRef = Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ));
    let producer = physicalop::PhysicalSchemaProducer::New(physicalop::NewBasePhysicalPlan(
        context.clone(),
        "HashJoin",
        0,
    ));
    let mut join = physicalop::NewPhysicalHashJoin(
        physicalop::BasePhysicalJoin::New(producer, base::JoinType::InnerJoin),
        1,
        false,
    );
    join.BasePhysicalJoin
        .LeftConditions
        .push(Box::new(astersql_expression::Column::default()));
    base::PhysicalPlan::set_children(
        &mut join,
        vec![
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 99)),
            Box::new(physicalop::PhysicalTableDual::New(context, 99)),
        ],
    );
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&join).unwrap();
    let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
    record_statement_ru_root_rows(&mut stats, ids[0], 3);
    record_statement_ru_root_rows(&mut stats, ids[1], 8);
    record_statement_ru_root_rows(&mut stats, ids[2], 5);
    let hash_stats = astersql_util_execdetails::execdetails::HashStateRuntimeStats::default();
    hash_stats.AddRows(7);
    stats.RegisterStatsShared(ids[0], Box::new(hash_stats));
    let evidence = snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup::default());
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
        StatementRUOperatorState::Complete
    );
    assert_eq!(calculator.units.cpu_work, 13.0);
    assert_eq!(calculator.units.hash_state_rows, 7.0);
    assert_eq!(calculator.units.join_output_rows, 3.0);
}

fn ru_join_context() -> base::ContextRef {
    Arc::new(TypedPlanTestContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
        planctx::variable::SessionVars::default(),
    ))
}

fn record_statement_ru_root_rows(
    stats: &mut astersql_util_execdetails::execdetails::RuntimeStatsColl,
    plan_id: i32,
    rows: i32,
) {
    stats
        .GetBasicRuntimeStats(plan_id, true)
        .unwrap()
        .Record(std::time::Duration::ZERO, rows);
}

fn completed_statement_ru_hash_state_rows(
    rows: u64,
) -> astersql_util_execdetails::execdetails::HashStateRowsSnapshot {
    let stats = astersql_util_execdetails::execdetails::HashStateRuntimeStats::default();
    stats.AddRows(rows);
    stats.HashStateRowsSnapshot()
}

fn ru_join_base(context: base::ContextRef, name: &str) -> physicalop::BasePhysicalJoin {
    let mut join = physicalop::BasePhysicalJoin::New(
        physicalop::PhysicalSchemaProducer::New(physicalop::NewBasePhysicalPlan(context, name, 0)),
        base::JoinType::InnerJoin,
    );
    join.LeftConditions = vec![Box::new(astersql_expression::Column::default())];
    join.RightConditions = vec![Box::new(astersql_expression::Column::default()); 2];
    join.OtherConditions = vec![Box::new(astersql_expression::Column::default()); 3];
    join.OuterJoinKeys = vec![astersql_expression::Column::default(); 4];
    join
}
fn ru_index_join(context: base::ContextRef) -> physicalop::PhysicalIndexJoin {
    let mut join = physicalop::PhysicalIndexJoin::New(ru_join_base(context, "IndexJoin"));
    join.OuterHashKeys = vec![astersql_expression::Column::default(); 5];
    let mut filters = physicalop::ColWithCmpFuncManager::New(None, 0);
    filters.OpType = vec!["gt".into(), "lt".into()];
    join.CompareFilters = Some(filters);
    join
}
fn ru_comparator() -> physicalop::JoinCompareFunc {
    Arc::new(|_, _, _, _| 0)
}

#[test]
fn statement_ru_join_counts_missing_rows_as_zero_after_teardown() {
    use crate::statement_ru_plan_walk::*;
    let context = ru_join_context();
    let mut join = physicalop::PhysicalMergeJoin {
        BasePhysicalJoin: ru_join_base(context.clone(), "MergeJoin"),
        Desc: false,
        CompareFuncs: vec![ru_comparator()],
    };
    base::PhysicalPlan::set_children(
        &mut join,
        vec![
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 1)),
            Box::new(physicalop::PhysicalTableDual::New(context, 1)),
        ],
    );
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&join).unwrap();
    let mut calculator = StatementRUCalculator::new(Default::default());

    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &Default::default(), &mut calculator).state,
        StatementRUOperatorState::Complete
    );
    assert_eq!(calculator.units.cpu_work, 0.0);
    assert_eq!(calculator.units.hash_state_rows, 0.0);
    assert_eq!(calculator.units.join_output_rows, 0.0);
    assert_eq!(calculator.units.operator_num, 3.0);
}

#[test]
fn go_merge_187_join_aggregation_variants_and_contracts() {
    use crate::statement_ru_plan_walk::*;
    use StatementRUOperatorState::*;
    let context = ru_join_context();
    let mut hash =
        physicalop::NewPhysicalHashJoin(ru_join_base(context.clone(), "HashJoin"), 8, false);
    let expr_ctx = astersql_expression_exprstatic::NewExprContext(Vec::new());
    let equal = astersql_expression::NewFunctionBase(
        &expr_ctx,
        astersql_expression::ast::EQ,
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        vec![
            Box::new(astersql_expression::NewInt64Const(1)),
            Box::new(astersql_expression::NewInt64Const(1)),
        ],
    )
    .unwrap();
    let equal = equal.as_scalar_function().unwrap();
    hash.EqualConditions = vec![equal.clone_scalar(), equal.clone_scalar()];
    hash.NAEqualConditions = vec![equal.clone_scalar()];
    let merge = physicalop::PhysicalMergeJoin {
        BasePhysicalJoin: ru_join_base(context.clone(), "MergeJoin"),
        Desc: false,
        CompareFuncs: vec![ru_comparator(); 4],
    };
    let index = ru_index_join(context.clone());
    let index_hash = physicalop::PhysicalIndexHashJoin::New(ru_index_join(context.clone()));
    let mut index_merge = physicalop::PhysicalIndexMergeJoin::New(ru_index_join(context.clone()));
    index_merge.CompareFuncs = vec![ru_comparator(); 4];
    index_merge.OuterCompareFuncs = vec![ru_comparator(); 5];
    let plans: Vec<(Box<dyn base::PhysicalPlan>, f64, bool)> = vec![
        (Box::new(hash), 9.0, true),
        (Box::new(merge), 10.0, false),
        (Box::new(index), 12.0, false),
        (Box::new(index_hash), 13.0, false),
        (Box::new(index_merge), 17.0, false),
    ];
    for (mut plan, slots, hash) in plans {
        plan.set_children(vec![
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 100)),
            Box::new(physicalop::PhysicalTableDual::New(context.clone(), 200)),
        ]);
        let mut tree = astersql_planner_core::FlattenTypedPhysicalPlan(plan.as_ref()).unwrap();
        let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
        record_statement_ru_root_rows(&mut stats, ids[0], 3);
        record_statement_ru_root_rows(&mut stats, ids[1], 8);
        record_statement_ru_root_rows(&mut stats, ids[2], 5);
        let mut evidence =
            snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
        evidence.plans[0].hash_state_rows = Some(completed_statement_ru_hash_state_rows(7));
        let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup {
            full_report: true,
            ..Default::default()
        });
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calculator).state,
            Complete,
            "{}",
            plan.tp(&[])
        );
        assert_eq!(calculator.units.cpu_work, 13.0 * slots);
        assert_eq!(calculator.units.join_output_rows, 3.0);
        assert_eq!(
            calculator.units.hash_state_rows,
            if hash { 7.0 } else { 0.0 }
        );
        if physicalop::index_join_base(plan.as_ref()).is_some() {
            assert_eq!(
                tree[1].Label,
                astersql_planner_core::TypedOperatorLabel::BuildSide
            );
            assert_eq!(
                tree[2].Label,
                astersql_planner_core::TypedOperatorLabel::ProbeSide
            );
        }
        let children = [
            StatementRUOperatorResult {
                state: Complete,
                output_rows: 8,
            },
            StatementRUOperatorResult {
                state: Complete,
                output_rows: 5,
            },
        ];
        let fresh = || StatementRUCalculator::new(StatementRUCalculationSetup::default());
        let mut calc = fresh();
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &children, 3, None, false, &mut calc),
            Complete
        );
        assert_eq!(calc.units.hash_state_rows, 0.0);
        let mut bad = evidence.plans[0].clone();
        bad.hash_state_rows.as_mut().unwrap().Rows = -1;
        let mut calc = fresh();
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &children, 3, Some(&bad), false, &mut calc,),
            if hash { Invalid } else { Complete }
        );
        if hash {
            assert_eq!(calc.units.cpu_work, 0.0);
        }
        let mut calc = fresh();
        let huge = [StatementRUOperatorResult {
            state: Complete,
            output_rows: i64::MAX,
        }; 2];
        // Integer-backed work cannot overflow f64, but adding to an infinite aggregate must fail atomically.
        calc.units.cpu_work = f64::INFINITY;
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &huge, 3, None, false, &mut calc),
            Invalid
        );
        tree[0].IsRoot = false;
        tree[0].StoreType = astersql_kv::StoreType::TiKV;
        tree[0].ReqType = physicalop::ReadReqType::Cop;
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &children, 3, None, false, &mut fresh(),),
            Unsupported
        );
        tree[0].StoreType = astersql_kv::StoreType::TiFlash;
        tree[0].ReqType = physicalop::ReadReqType::MPP;
        let mut mpp = evidence.plans[0].clone();
        mpp.tiflash = Some(
            astersql_util_execdetails::execdetails::TiFlashExecutionUnits {
                HashDistinctEntries: 11,
                HashBuildRows: 17,
                ..Default::default()
            },
        );
        let mut calc = fresh();
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &children, 3, Some(&mpp), true, &mut calc,),
            if hash { Complete } else { Unsupported }
        );
        assert_eq!(calc.units.hash_state_rows, if hash { 28.0 } else { 0.0 });
        mpp.tiflash.as_mut().unwrap().Invalid = true;
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &children, 3, Some(&mpp), true, &mut fresh(),),
            if hash { Invalid } else { Unsupported }
        );
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &children[..1], 3, None, true, &mut fresh(),),
            if hash { Invalid } else { Unsupported }
        );
        drop(tree);
        if let Some(join) = plan
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalHashJoin>()
        {
            join.BasePhysicalJoin.JoinType = base::JoinType::FullOuterJoin;
        } else if let Some(join) = plan
            .as_any_mut()
            .downcast_mut::<physicalop::PhysicalMergeJoin>()
        {
            join.BasePhysicalJoin.JoinType = base::JoinType::FullOuterJoin;
        } else {
            physicalop::index_join_base_mut(plan.as_mut())
                .unwrap()
                .BasePhysicalJoin
                .JoinType = base::JoinType::FullOuterJoin;
        }
        let tree = astersql_planner_core::FlattenTypedPhysicalPlan(plan.as_ref()).unwrap();
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut fresh()).state,
            Unsupported
        );
    }
    for join_type in [base::JoinType::InnerJoin, base::JoinType::FullOuterJoin] {
        let mut base = ru_join_base(context.clone(), "HashJoin");
        base.JoinType = join_type;
        let mut join = physicalop::NewPhysicalHashJoin(base, 1, false);
        base::PhysicalPlan::set_children(
            &mut join,
            vec![
                Box::new(physicalop::PhysicalTableDual::New(context.clone(), 1)),
                Box::new(physicalop::PhysicalTableDual::New(context.clone(), 1)),
            ],
        );
        let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&join).unwrap();
        let ids: Vec<_> = tree.iter().map(|operator| operator.Origin.id()).collect();
        let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        record_statement_ru_root_rows(&mut stats, ids[0], 1);
        record_statement_ru_root_rows(&mut stats, ids[1], 1);
        record_statement_ru_root_rows(&mut stats, ids[2], 1);
        let hash_stats = astersql_util_execdetails::execdetails::HashStateRuntimeStats::default();
        stats.RegisterStatsShared(ids[0], Box::new(hash_stats));
        let evidence = snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
        let mut calc = StatementRUCalculator::new(Default::default());
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calc).state,
            if join_type == base::JoinType::FullOuterJoin {
                Unsupported
            } else {
                Complete
            }
        );
    }
    // The production canonical IndexHashJoin retains the complete IndexJoin base.
    for name in ["IndexHashJoin", "IndexMergeJoin"] {
        let mut join = ru_index_join(context.clone());
        join.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetTP(name);
        base::PhysicalPlan::set_children(
            &mut join,
            vec![
                Box::new(physicalop::PhysicalTableDual::New(context.clone(), 1)),
                Box::new(physicalop::PhysicalTableDual::New(context.clone(), 1)),
            ],
        );
        let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&join).unwrap();
        let children = [
            StatementRUOperatorResult {
                state: Complete,
                output_rows: 8,
            },
            StatementRUOperatorResult {
                state: Complete,
                output_rows: 5,
            },
        ];
        let mut calc = StatementRUCalculator::new(Default::default());
        assert_eq!(
            collect_statement_ru_join_units(&tree[0], &children, 3, None, false, &mut calc),
            if name == "IndexHashJoin" {
                Complete
            } else {
                Unsupported
            }
        );
        assert_eq!(
            calc.units.cpu_work,
            if name == "IndexHashJoin" { 169.0 } else { 0.0 }
        );
    }
}

#[test]
fn go_merge_187_join_aggregation_hash_and_stream_sites() {
    use crate::statement_ru_plan_walk::*;
    use StatementRUOperatorState::*;
    let context = ru_join_context();
    for hash in [true, false] {
        let mut agg = physicalop::BasePhysicalAgg::New(physicalop::PhysicalSchemaProducer::New(
            physicalop::NewBasePhysicalPlan(
                context.clone(),
                if hash { "HashAgg" } else { "StreamAgg" },
                0,
            ),
        ));
        agg.GroupByItems = vec![Box::new(astersql_expression::Column::default()); 2];
        agg.AggFuncs = vec![astersql_expression_aggregation::AggFuncDesc {
            baseFuncDesc: astersql_expression_aggregation::baseFuncDesc {
                Name: "count".into(),
                Args: Vec::new(),
                RetTp: None,
            },
            Mode: Default::default(),
            HasDistinct: false,
            OrderByItems: Vec::new(),
            GroupingID: 0,
        }];
        let mut plan: Box<dyn base::PhysicalPlan> = if hash {
            Box::new(physicalop::PhysicalHashAgg {
                BasePhysicalAgg: agg,
                TiflashPreAggMode: String::new(),
            })
        } else {
            Box::new(physicalop::PhysicalStreamAgg {
                BasePhysicalAgg: agg,
            })
        };
        plan.set_children(vec![Box::new(physicalop::PhysicalTableDual::New(
            context.clone(),
            99,
        ))]);
        let mut tree = astersql_planner_core::FlattenTypedPhysicalPlan(plan.as_ref()).unwrap();
        let mut stats = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
        record_statement_ru_root_rows(&mut stats, ids[0], 3);
        record_statement_ru_root_rows(&mut stats, ids[1], 8);
        let mut evidence =
            snapshot_statement_ru_runtime_evidence(Some(&stats), &ids, None, None, None);
        evidence.plans[0].hash_state_rows = Some(completed_statement_ru_hash_state_rows(7));
        let mut calc = StatementRUCalculator::new(Default::default());
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calc).state,
            Complete
        );
        assert_eq!(calc.units.cpu_work, 24.0);
        assert_eq!(calc.units.hash_state_rows, if hash { 7.0 } else { 0.0 });
        let child = [StatementRUOperatorResult {
            state: Complete,
            output_rows: 8,
        }];
        let fresh = || StatementRUCalculator::new(Default::default());
        for (root, mpp, output, expected) in [
            (true, false, 3, 7.0),
            (false, false, 3, 3.0),
            (false, true, 3, 28.0),
        ] {
            tree[0].IsRoot = root;
            let mut stats = evidence.plans[0].clone();
            stats.tiflash = Some(
                astersql_util_execdetails::execdetails::TiFlashExecutionUnits {
                    HashDistinctEntries: 11,
                    HashBuildRows: 17,
                    ..Default::default()
                },
            );
            let mut calc = fresh();
            assert_eq!(
                collect_statement_ru_aggregation_units(
                    &tree[0],
                    &child,
                    output,
                    Some(&stats),
                    mpp,
                    true,
                    &mut calc
                ),
                Complete
            );
            assert_eq!(
                calc.units.hash_state_rows,
                if hash { expected } else { 0.0 }
            );
            assert_eq!(calc.units.cpu_work, 24.0);
            stats.hash_state_rows.as_mut().unwrap().Rows = -1;
            stats.tiflash.as_mut().unwrap().Invalid = true;
            assert_eq!(
                collect_statement_ru_aggregation_units(
                    &tree[0],
                    &child,
                    output,
                    Some(&stats),
                    mpp,
                    true,
                    &mut fresh()
                ),
                if hash && (root || mpp) {
                    Invalid
                } else {
                    Complete
                }
            );
            let mut calc = fresh();
            assert_eq!(
                collect_statement_ru_aggregation_units(
                    &tree[0], &child, 0, None, mpp, true, &mut calc
                ),
                Complete
            );
            assert_eq!(calc.units.hash_state_rows, 0.0);
        }
        assert_eq!(
            collect_statement_ru_aggregation_units(
                &tree[0],
                &child,
                3,
                None,
                false,
                false,
                &mut fresh()
            ),
            Unsupported
        );
        assert_eq!(
            collect_statement_ru_aggregation_units(
                &tree[0],
                &[],
                3,
                None,
                false,
                true,
                &mut fresh()
            ),
            Invalid
        );
    }
}

#[test]
fn go_merge_187_mpp_cte_site() {
    use crate::statement_ru_plan_walk::*;
    use crate::statement_ru_reporting::*;
    use StatementRUOperatorState::*;
    use base::{PhysicalPlan as _, Plan as _};
    let context = ru_join_context();
    let scan = physicalop::PhysicalTableScan::New(context.clone());
    let mut sort = physicalop::PhysicalSort::New(context.clone());
    sort.set_children(vec![Box::new(scan)]);
    let mut sender = physicalop::PhysicalExchangeSender::New(context.clone());
    sender.set_children(vec![Box::new(sort)]);
    let mut reader = physicalop::PhysicalTableReader::New(context.clone());
    reader.StoreType = astersql_kv::StoreType::TiFlash;
    reader.ReadReqType = physicalop::ReadReqType::MPP;
    reader.TablePlan = Some(Box::new(sender));
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&reader).unwrap();
    let collector = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
    let mut evidence =
        snapshot_statement_ru_runtime_evidence(Some(&collector), &ids, None, None, None);
    for (index, rows, read, send, cross) in [
        (1, 200, 0, 100, 50),
        (2, 200, 0, 0, 0),
        (3, 200, 300, 20, 10),
    ] {
        evidence.plans[index].tiflash = Some(
            astersql_util_execdetails::execdetails::TiFlashExecutionUnits {
                Rows: rows,
                UserReadBytes: read,
                InnerZoneSendBytes: send,
                InterZoneSendBytes: cross,
                ..Default::default()
            },
        );
    }
    let mut calc = StatementRUCalculator::new(Default::default());
    calc.report = Some(StatementRUFullReport::default());
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calc).state,
        Complete
    );
    assert_eq!(
        calc.units,
        StmtUnits {
            cpu_work: 200.0 * 200.0_f64.log2(),
            scan_bytes: 300.0,
            net_bytes: 180.0,
            cross_az_net_bytes: 60.0,
            operator_num: 4.0,
            ..Default::default()
        }
    );
    assert_eq!(
        calc.compute[StatementRUEngine::TiDB as usize].operator_num,
        1.0
    );
    assert_eq!(
        calc.compute[StatementRUEngine::TiKV as usize],
        StatementRUComputeUnits::default()
    );
    assert_eq!(
        calc.compute[StatementRUEngine::TiFlash as usize].scan_bytes,
        300.0
    );
    let report = calc.report.as_ref().unwrap();
    let sum = report
        .units
        .iter()
        .flatten()
        .fold(StmtUnits::default(), |sum, units| sum.add(*units));
    assert_eq!(sum, calc.units);
    let mut explain = vec![StatementRUExplainOperatorResult::default(); tree.len()];
    let mut explain_calc = StatementRUCalculator::new(Default::default());
    assert_eq!(
        calculate_statement_ru_plan_with_operators(
            &tree,
            0,
            &evidence,
            &mut explain_calc,
            StmtUnits::default(),
            Some(&mut explain)
        )
        .state,
        Complete
    );
    let finalized = calc.finalize().unwrap();
    assert!((explain[0].cum_ru - finalized.result.total_ru).abs() < 1e-9);
    assert!(
        (explain.iter().map(|operator| operator.self_ru).sum::<f64>() - finalized.result.total_ru)
            .abs()
            < 1e-9
    );
    assert_eq!(finalized.engine_ru.tikv, 0.0);
    assert_eq!(finalized.engine_ru.tidb, 1.0);
    assert!(
        (finalized.engine_ru.tidb + finalized.engine_ru.tiflash - finalized.result.total_ru).abs()
            < 1e-9
    );
    let empty = StatementRURuntimeEvidence::default();
    let mut partial = StatementRUCalculator::new(Default::default());
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &empty, &mut partial).state,
        Complete
    );
    let engines = statement_ru_engine_result(
        partial.units,
        partial.compute,
        astersql_resourcegroup::ruv2::model::default_weights(),
    );
    assert_eq!(engines.tikv, 0.0);
    assert_eq!(partial.finalize().unwrap().result.total_ru, 31.0);
    evidence.plans[1].tiflash.as_mut().unwrap().Invalid = true;
    assert_eq!(
        calculate_statement_ru_plan(
            &tree,
            0,
            &evidence,
            &mut StatementRUCalculator::new(Default::default())
        )
        .state,
        Invalid
    );
    reader.ReadReqType = physicalop::ReadReqType::BatchCop;
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&reader).unwrap();
    assert_eq!(
        calculate_statement_ru_plan(
            &tree,
            0,
            &empty,
            &mut StatementRUCalculator::new(Default::default())
        )
        .state,
        Unsupported
    );
}

#[test]
fn go_merge_187_mpp_cte_site_shuffle_and_orchestration() {
    use crate::statement_ru_plan_walk::*;
    use StatementRUOperatorState::*;
    use base::{PhysicalPlan as _, Plan as _};
    let context = ru_join_context();
    for splitter in [
        physicalop::physical_shuffle::PartitionSplitterType::Hash,
        physicalop::physical_shuffle::PartitionSplitterType::Range,
    ] {
        let source = physicalop::PhysicalTableDual::New(context.clone(), 99);
        let source_id = source.id();
        let duplicate = source.clone_physical(context.clone()).unwrap();
        let receiver =
            physicalop::PhysicalShuffleReceiverStub::New(context.clone(), Some(Box::new(source)));
        let mut shuffle = physicalop::PhysicalShuffle::New(context.clone(), 8, Vec::new());
        shuffle.DataSources = vec![duplicate];
        shuffle.ByItemArrays = vec![vec![Box::new(astersql_expression::Column::default()); 2]];
        shuffle.SplitterType = splitter;
        shuffle.set_children(vec![Box::new(receiver)]);
        let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&shuffle).unwrap();
        let mut collector = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
        collector
            .GetBasicRuntimeStats(source_id, true)
            .unwrap()
            .Record(std::time::Duration::ZERO, 7);
        let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
        let mut evidence =
            snapshot_statement_ru_runtime_evidence(Some(&collector), &ids, None, None, None);
        let mut calc = StatementRUCalculator::new(Default::default());
        assert_eq!(
            calculate_statement_ru_plan(&tree, 0, &evidence, &mut calc).state,
            Complete
        );
        assert_eq!(calc.units.cpu_work, 21.0);
        assert_eq!(calc.units.operator_num, 3.0);
        evidence
            .plans
            .iter_mut()
            .find(|stats| stats.plan_id == source_id)
            .unwrap()
            .root_rows
            .Rows = -1;
        assert_eq!(
            calculate_statement_ru_plan(
                &tree,
                0,
                &evidence,
                &mut StatementRUCalculator::new(Default::default())
            )
            .state,
            Invalid
        );
    }
    let mut sequence = physicalop::PhysicalSequence::New(context.clone());
    sequence.set_children(vec![
        Box::new(physicalop::PhysicalCTETable::New(context.clone(), 17)),
        Box::new(physicalop::PhysicalTableDual::New(context.clone(), 1)),
    ]);
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&sequence).unwrap();
    let mut calc = StatementRUCalculator::new(Default::default());
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &Default::default(), &mut calc).state,
        Complete
    );
    assert_eq!(calc.units.cpu_work, 0.0);
    assert_eq!(calc.units.operator_num, 3.0);
}

#[test]
fn go_merge_187_mpp_cte_site_shared_forest() {
    use crate::statement_ru_plan_walk::*;
    use base::{PhysicalPlan as _, Plan as _};
    let context = ru_join_context();
    let seed_source = physicalop::PhysicalCTETable::New(context.clone(), 17);
    let seed_source_id = seed_source.id();
    let mut seed = physicalop::PhysicalProjection::New(context.clone());
    seed.Exprs = vec![Box::new(astersql_expression::Column::default()); 2];
    seed.set_children(vec![Box::new(seed_source)]);
    let definition = Arc::new(physicalop::PhysicalCTEDefinition::New(
        context.clone(),
        17,
        Box::new(seed),
        Some(Box::new(physicalop::PhysicalTableDual::New(
            context.clone(),
            1,
        ))),
    ));
    let mut root = physicalop::PhysicalUnionAll::New(context.clone());
    root.set_children(vec![
        Box::new(physicalop::PhysicalCTE::New(
            context.clone(),
            definition.clone(),
        )),
        Box::new(physicalop::PhysicalCTE::New(context.clone(), definition)),
    ]);
    let scalar_source = physicalop::PhysicalTableDual::New(context.clone(), 99);
    let scalar_source_id = scalar_source.id();
    let mut scalar_child = physicalop::PhysicalProjection::New(context.clone());
    scalar_child.Exprs = vec![Box::new(astersql_expression::Column::default()); 3];
    scalar_child.set_children(vec![Box::new(scalar_source)]);
    let scalar = astersql_planner_core::ScalarSubqueryEvalCtx::New(
        context.clone(),
        0,
        Arc::new(scalar_child),
        astersql_planner_core::context::BackgroundArc(),
        astersql_infoschema::infoschema::MockInfoSchema(Vec::new()),
    );
    let registered: Vec<Rc<dyn std::any::Any>> = vec![Rc::new(scalar)];
    let mut forest =
        astersql_planner_core::FlattenTypedPhysicalPlanForest(&root, &registered).unwrap();
    assert_eq!(forest.CTEs.len(), 1);
    assert_eq!(forest.ScalarSubQueries.len(), 1);
    let mut collector = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    collector
        .GetBasicRuntimeStats(seed_source_id, true)
        .unwrap()
        .Record(std::time::Duration::ZERO, 5);
    collector
        .GetBasicRuntimeStats(scalar_source_id, true)
        .unwrap()
        .Record(std::time::Duration::ZERO, 3);
    let ids: Vec<_> = std::iter::once(&forest.Main)
        .chain(&forest.CTEs)
        .chain(&forest.ScalarSubQueries)
        .flatten()
        .map(|op| op.Origin.id())
        .collect();
    let evidence = snapshot_statement_ru_runtime_evidence(Some(&collector), &ids, None, None, None);
    let finalized =
        calculate_statement_ru_forest(&forest, &evidence, Default::default(), true).unwrap();
    assert_eq!(finalized.units.operator_num, 10.0);
    assert_eq!(finalized.units.cpu_work, 19.0); // Shared producer once; scalar child owns its work.
    assert!(calculate_statement_ru_forest(&forest, &evidence, Default::default(), false).is_err());
    let invalid_definition = &mut forest.CTEs[0];
    invalid_definition[1].Label = astersql_planner_core::TypedOperatorLabel::RecursivePart;
    assert_eq!(
        calculate_statement_ru_plan(
            invalid_definition,
            0,
            &Default::default(),
            &mut StatementRUCalculator::new(Default::default())
        )
        .state,
        StatementRUOperatorState::Unsupported
    );
}

#[test]
fn go_merge_187_mpp_cte_site_exchange_rows_and_rejected_sites() {
    use crate::statement_ru_plan_walk::*;
    use base::PhysicalPlan as _;
    let ctx = ru_join_context();
    let mut sender = physicalop::PhysicalExchangeSender::New(ctx.clone());
    sender.set_children(vec![Box::new(physicalop::PhysicalTableScan::New(
        ctx.clone(),
    ))]);
    let mut receiver = physicalop::PhysicalExchangeReceiver::New(ctx.clone());
    receiver.set_children(vec![Box::new(sender)]);
    let mut projection = physicalop::PhysicalProjection::New(ctx.clone());
    projection.Exprs = vec![Box::new(astersql_expression::Column::default()); 2];
    projection.set_children(vec![Box::new(receiver)]);
    let mut reader = physicalop::PhysicalTableReader::New(ctx.clone());
    reader.StoreType = astersql_kv::StoreType::TiFlash;
    reader.ReadReqType = physicalop::ReadReqType::MPP;
    reader.TablePlan = Some(Box::new(projection));
    let mut tree = astersql_planner_core::FlattenTypedPhysicalPlan(&reader).unwrap();
    let collector = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
    let mut evidence =
        snapshot_statement_ru_runtime_evidence(Some(&collector), &ids, None, None, None);
    for (index, rows, read, send) in [
        (1, 300, 0, 0),
        (2, 300, 0, 1000),
        (3, 20, 0, 30),
        (4, 10, 40, 5),
    ] {
        evidence.plans[index].tiflash = Some(
            astersql_util_execdetails::execdetails::TiFlashExecutionUnits {
                Rows: rows,
                UserReadBytes: read,
                InnerZoneSendBytes: send,
                ..Default::default()
            },
        );
    }
    let mut calc = StatementRUCalculator::new(Default::default());
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calc).state,
        StatementRUOperatorState::Complete
    );
    assert_eq!(calc.units.cpu_work, 600.0); // Receiver rows, not sender/source rows.
    assert_eq!(calc.units.net_bytes, 35.0); // Receiver sends are not charged.
    assert_eq!(calc.units.scan_bytes, 40.0);
    evidence.plans[2].tiflash.as_mut().unwrap().Rows = i64::MAX as u64 + 1;
    assert_eq!(
        calculate_statement_ru_plan(
            &tree,
            0,
            &evidence,
            &mut StatementRUCalculator::new(Default::default())
        )
        .state,
        StatementRUOperatorState::Invalid
    );
    for root in [true, false] {
        for store in [
            astersql_kv::StoreType::TiDB,
            astersql_kv::StoreType::TiKV,
            astersql_kv::StoreType::TiFlash,
        ] {
            for req in [
                physicalop::ReadReqType::Cop,
                physicalop::ReadReqType::BatchCop,
                physicalop::ReadReqType::MPP,
            ] {
                tree[1].IsRoot = root;
                tree[1].StoreType = store;
                tree[1].ReqType = req;
                assert_eq!(
                    statement_ru_operator_runs_at_supported_site(&tree[1]),
                    root || (store == astersql_kv::StoreType::TiKV
                        && req == physicalop::ReadReqType::Cop)
                        || (!root
                            && store == astersql_kv::StoreType::TiFlash
                            && req == physicalop::ReadReqType::MPP)
                );
            }
        }
    }
    // A columnar-search TableScan remains unsupported even at the MPP site.
    let mut scan = physicalop::PhysicalTableScan::New(ctx);
    scan.UsedColumnarIndexes
        .push(physicalop::ColumnarIndexExtra::default());
    reader.TablePlan = Some(Box::new(scan));
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&reader).unwrap();
    assert_eq!(
        calculate_statement_ru_plan(
            &tree,
            0,
            &Default::default(),
            &mut StatementRUCalculator::new(Default::default())
        )
        .state,
        StatementRUOperatorState::Unsupported
    );
}

#[test]
fn go_merge_187_mpp_cte_site_reader_collects_both_join_scans() {
    use crate::statement_ru_plan_walk::*;
    use crate::statement_ru_reporting::*;
    use base::PhysicalPlan as _;
    let ctx = ru_join_context();
    let mut join = physicalop::NewPhysicalHashJoin(ru_join_base(ctx.clone(), "HashJoin"), 1, false);
    join.set_children(vec![
        Box::new(physicalop::PhysicalTableScan::New(ctx.clone())),
        Box::new(physicalop::PhysicalTableScan::New(ctx.clone())),
    ]);
    let mut reader = physicalop::PhysicalTableReader::New(ctx);
    reader.StoreType = astersql_kv::StoreType::TiFlash;
    reader.ReadReqType = physicalop::ReadReqType::MPP;
    reader.TablePlan = Some(Box::new(join));
    let tree = astersql_planner_core::FlattenTypedPhysicalPlan(&reader).unwrap();
    let collector = astersql_util_execdetails::execdetails::NewRuntimeStatsColl(None);
    let ids: Vec<_> = tree.iter().map(|op| op.Origin.id()).collect();
    let mut evidence =
        snapshot_statement_ru_runtime_evidence(Some(&collector), &ids, None, None, None);
    for (index, rows, bytes, send) in [(1, 3, 0, 0), (2, 8, 110, 1), (3, 5, 220, 2)] {
        evidence.plans[index].tiflash = Some(
            astersql_util_execdetails::execdetails::TiFlashExecutionUnits {
                Rows: rows,
                UserReadBytes: bytes,
                InnerZoneSendBytes: send,
                ..Default::default()
            },
        );
    }
    evidence.plans[1]
        .tiflash
        .as_mut()
        .unwrap()
        .HashDistinctEntries = 4;
    evidence.plans[1].tiflash.as_mut().unwrap().HashBuildRows = 6;
    let mut calc = StatementRUCalculator::new(Default::default());
    assert_eq!(
        calculate_statement_ru_plan(&tree, 0, &evidence, &mut calc).state,
        StatementRUOperatorState::Complete
    );
    assert_eq!(calc.units.cpu_work, (8.0 + 5.0) * 6.0); // 1 left + 2 right + 3 other conditions.
    assert_eq!(calc.units.hash_state_rows, 10.0);
    assert_eq!(calc.units.join_output_rows, 3.0);
    assert_eq!(calc.units.scan_bytes, 330.0);
    assert_eq!(calc.units.net_bytes, 3.0);
    assert_eq!(
        calc.compute[StatementRUEngine::TiFlash as usize].scan_bytes,
        330.0
    );
    assert_eq!(
        calc.compute[StatementRUEngine::TiFlash as usize].join_output_rows,
        3.0
    );
    assert_eq!(calc.finalize().unwrap().engine_ru.tikv, 0.0);
}

fn ru_terminal_stmt() -> crate::adapter::ExecStmt {
    use crate::adapter::*;
    let runtime = Arc::new(RUTerminalRuntime::default());
    let typed = physicalop::PhysicalTableDual::New(ru_join_context(), 0);
    let id = base::Plan::id(&typed);
    ExecStmt {
        GoCtx: None,
        InfoSchema: 0,
        Plan: PlanInfo {
            id,
            kind: crate::adapter::PlanKind::Query,
            schema: vec![],
            calculate_no_delay: false,
            projection_child: None,
            encoded: String::new(),
            binary: String::new(),
            hints: String::new(),
        },
        TypedPlan: Some(Arc::new(typed)),
        StmtNode: StatementNode {
            kind: StatementKind::Select,
            original_text: "select 1".into(),
            text: "select 1".into(),
            secure_text: "select 1".into(),
            prepared_text: None,
        },
        Ctx: runtime,
        LowerPriority: false,
        isPreparedStmt: false,
        isSelectForUpdate: false,
        retryCount: 0,
        retryStartTime: None,
        phaseBuildDurations: Default::default(),
        phaseOpenDurations: Default::default(),
        phaseNextDurations: Default::default(),
        phaseLockDurations: Default::default(),
        OutputNames: vec![],
        PsStmt: None,
        Ti: None,
        StatementCtx: StatementContext {
            statement_ru_owner: Some(Arc::new(StatementRUOwner::new(
                StatementRUCalculationSetup {
                    frontend_compile_bytes: 13.0,
                    full_report: false,
                },
                false,
                false,
                false,
            ))),
            ..Default::default()
        },
    }
}

#[test]
fn go_merge_187_terminal_once() {
    let mut stmt = ru_terminal_stmt();
    let owner = stmt.StatementCtx.statement_ru_owner.clone().unwrap();
    owner.record_final_outcome(true);
    owner.record_root_eof();
    stmt.FinishExecuteStmt(0, None, false);
    assert_eq!(
        owner.take_terminal_setup(),
        None,
        "the real executor terminal must consume the RU owner"
    );
    let finalized = stmt.StatementCtx.statement_ru_finalized.clone().unwrap();
    assert_eq!(finalized.units.frontend_compile_bytes, 13.0);
    assert_eq!(finalized.units.operator_num, 1.0);
    stmt.FinishExecuteStmt(0, Some(errors::New("late terminal error")), false);
    assert!(Arc::ptr_eq(
        &finalized,
        stmt.StatementCtx.statement_ru_finalized.as_ref().unwrap()
    ));
}

use crate::adapter::{
    AdapterResult, ChunkConfig, Digest, ExecExecutor, FieldName, Key, PessimisticErrorAction,
    PlanInfo, Priority, RebuiltPlan, StatementContext, StatementNode, StatementSummary,
    TelemetryInfo, pessimisticTxn,
};
use astersql_errors as errors;
use astersql_util_chunk as chunk;
use std::time::{Duration, SystemTime};
#[derive(Default)]
struct RUTerminalRuntime {
    state: crate::statement_ru_result::StatementRUInstallState,
    panic_evidence: bool,
    ru_version: Option<u8>,
    stream_mode: u8,
    reentrant_stmt: std::cell::RefCell<Option<crate::adapter::ExecStmt>>,
    published: std::cell::RefCell<Vec<(String, f64, f64, f64)>>,
    topsql: std::cell::RefCell<Vec<f64>>,
    calibration: std::cell::RefCell<Vec<astersql_resourcegroup::ruv2::model::StmtUnits>>,
}
#[allow(unused_variables)]
impl crate::adapter::AdapterRuntime for RUTerminalRuntime {
    fn BuildExecutor(
        &self,
        _plan: &PlanInfo,
        _telemetry: Option<&TelemetryInfo>,
    ) -> AdapterResult<Box<dyn ExecExecutor>> {
        Ok(Box::new(RUTerminalExecutor::new(self.stream_mode)))
    }
    fn BuildPointGetExecutor(
        &self,
        _plan: &PlanInfo,
        _start_ts: u64,
        _prepared_key: Option<&str>,
    ) -> AdapterResult<Box<dyn ExecExecutor>> {
        Ok(Box::new(RUTerminalExecutor::new(self.stream_mode)))
    }
    fn RebuildPlan(
        &self,
        _statement: &StatementNode,
        _previous_summary: &PlanInfo,
        _previous_names: &[FieldName],
    ) -> AdapterResult<RebuiltPlan> {
        unreachable!("unused terminal test runtime method")
    }
    fn NewChunk(&self, _config: &ChunkConfig) -> chunk::Chunk {
        Default::default()
    }
    fn StatementReadTS(&self) -> AdapterResult<u64> {
        Ok(0)
    }
    fn TransactionStartTS(&self) -> u64 {
        Default::default()
    }
    fn SnapshotTS(&self) -> u64 {
        Default::default()
    }
    fn LowResolutionTSO(&self) -> bool {
        Default::default()
    }
    fn IsPessimistic(&self) -> bool {
        Default::default()
    }
    fn SupportsSelectForUpdate(&self) -> bool {
        Default::default()
    }
    fn SupportsPreparedExecution(&self) -> bool {
        Default::default()
    }
    fn ForeignKeyChecks(&self) -> bool {
        Default::default()
    }
    fn StmtCommit(&self) -> AdapterResult {
        Ok(())
    }
    fn ForeignKeySavepointName(&self) -> String {
        Default::default()
    }
    fn ReleaseForeignKeySavepoint(&self, _savepoint: &str) {}
    fn SetInHandleForeignKeyTrigger(&self, _active: bool) {}
    fn ForeignKeyCheckInSharedLock(&self) -> bool {
        Default::default()
    }
    fn KillSignal(&self) -> AdapterResult {
        if self.stream_mode == 5 {
            Err(errors::New("killed"))
        } else {
            Ok(())
        }
    }
    fn CurrentDatabase(&self) -> String {
        Default::default()
    }
    fn InitialChunkSize(&self) -> usize {
        Default::default()
    }
    fn MaximumChunkSize(&self) -> usize {
        Default::default()
    }
    fn Command(&self) -> u8 {
        Default::default()
    }
    fn MaximumExecutionTime(&self) -> u64 {
        Default::default()
    }
    fn SetProcessInfo(&self, _sql: &str, _started: SystemTime, _command: u8, _maximum_time: u64) {}
    fn CancelMaximumExecutionTime(&self) {}
    fn SetPriority(&self, _priority: Priority) {}
    fn SetLastFoundRows(&self, _rows: u64) {}
    fn AddFoundRows(&self, _rows: u64) {}
    fn ResetStatementForRetry(&self) {}
    fn InheritExecuteStatement(&self, _statement: &mut StatementNode) -> AdapterResult {
        Ok(())
    }
    fn PreparedStatementSQL(&self) -> String {
        Default::default()
    }
    fn IsReadOnly(&self, _statement: &StatementNode) -> bool {
        Default::default()
    }
    fn PrepareFKCascadeContext(&self) {}
    fn HandleFKTriggerError(&self) -> AdapterResult {
        Ok(())
    }
    fn DetachTrackers(&self) {}
    fn ResetCTEStorage(&self) -> AdapterResult {
        Ok(())
    }
    fn OnPessimisticStmtStart(&self) -> AdapterResult {
        Ok(())
    }
    fn OnPessimisticStmtEnd(&self, _success: bool) -> AdapterResult {
        Ok(())
    }
    fn PessimisticTransaction(&self) -> AdapterResult<Box<dyn pessimisticTxn>> {
        unreachable!("unused terminal test runtime method")
    }
    fn ResetUnchangedKeysForLock(&self) {}
    fn CollectUnchangedKeysForXLock(&self, _keys: Vec<Key>) -> Vec<Key> {
        Default::default()
    }
    fn CollectUnchangedKeysForSLock(&self, _keys: Vec<Key>) -> Vec<Key> {
        Default::default()
    }
    fn LockKeys(&self, _keys: &[Key], _shared: bool) -> AdapterResult {
        Ok(())
    }
    fn OnPessimisticLockError(
        &self,
        _error: &errors::SharedError,
    ) -> AdapterResult<PessimisticErrorAction> {
        unreachable!("unused terminal test runtime method")
    }
    fn OnPessimisticStmtRetry(&self) -> AdapterResult {
        Ok(())
    }
    fn RollbackStatementForRetry(&self) -> AdapterResult {
        Ok(())
    }
    fn MaximumPessimisticRetries(&self) -> usize {
        Default::default()
    }
    fn StatementContext(&self) -> StatementContext {
        Default::default()
    }
    fn SetStatementContext(&self, _context: &StatementContext) {}
    fn Digest(&self, _text: &str) -> Digest {
        Default::default()
    }
    fn Audit(&self, _sql: &str) {}
    fn ObservePhase(&self, _phase: &str, _internal: bool, _duration: Duration) {}
    fn RecordDMLMetric(&self, _statement_type: &str, _value: i64) {}
    fn RUVersion(&self) -> u8 {
        self.ru_version.unwrap_or(2)
    }
    fn RUV2ReporterAvailable(&self) -> bool {
        true
    }
    fn ResourceGroupName(&self) -> String {
        "ru_test".into()
    }
    fn ReportRUV2Consumption(&self, group: &str, tikv: f64, tidb: f64, tiflash: f64) {
        self.published
            .borrow_mut()
            .push((group.into(), tikv, tidb, tiflash));
    }
    fn StatementRUCalibration(
        &self,
        _: crate::statement_ru_result::StatementRUCalibrationState,
        units: astersql_resourcegroup::ruv2::model::StmtUnits,
    ) {
        self.calibration.borrow_mut().push(units);
    }
    fn RecordLastQuery(&self, _error: Option<&str>) {}
    fn PlanReplayerCapture(&self, _statement: &StatementNode, _start_ts: u64, _continuous: bool) {}
    fn SlowQuery(&self, _transaction_ts: u64, _sql: &str, _success: bool, _has_more_results: bool) {
    }
    fn Summary(&self, _summary: &StatementSummary) {}
    fn UpdatePreviousStatement(&self, _sql: &str, _digest: &str) {}
    fn RecordNetworkTraffic(&self, _sent: u64, _received: u64, _mpp: u64) {}
    fn RecordPlanCache(&self, _hit: bool, _reason: Option<&str>) {}
    fn TopSQLStart(&self, _sql_digest: &[u8], _plan_digest: &[u8]) {}
    fn TopSQLFinish(&self, total_ru_v2: f64) {
        self.topsql.borrow_mut().push(total_ru_v2);
    }
    fn RestrictedSQL(&self) -> bool {
        Default::default()
    }
    fn RedactLog(&self) -> bool {
        Default::default()
    }
    fn StatementRUInstallState(
        &self,
        _: &crate::adapter::StatementNode,
    ) -> Option<crate::statement_ru_result::StatementRUInstallState> {
        if let Some(mut stmt) = self.reentrant_stmt.borrow_mut().take() {
            assert!(
                stmt.finishStatementRU(None).is_none(),
                "reentry must see a consumed owner"
            );
        }
        Some(self.state.clone())
    }
    fn StatementRURuntimeEvidence(
        &self,
        _: &[i32],
    ) -> crate::statement_ru_plan_walk::StatementRURuntimeEvidence {
        assert!(!self.panic_evidence, "terminal evidence panic");
        Default::default()
    }
}

struct RUTerminalExecutor {
    mode: u8,
    calls: usize,
    schema: Vec<crate::adapter::SchemaColumn>,
}
impl RUTerminalExecutor {
    fn new(mode: u8) -> Self {
        Self {
            mode,
            calls: 0,
            schema: if mode == 6 {
                vec![]
            } else {
                vec![crate::adapter::SchemaColumn {
                    field_type: Default::default(),
                }]
            },
        }
    }
}
impl ExecExecutor for RUTerminalExecutor {
    fn Open(&mut self) -> AdapterResult {
        assert!(self.mode != 7, "open panic");
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        if self.mode == 4 {
            Err(errors::New("close failed"))
        } else {
            Ok(())
        }
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        output.Reset();
        self.calls += 1;
        match self.mode {
            2 => return Err(errors::New("read failed")),
            3 => panic!("read panic"),
            1 if self.calls == 1 => output.SetNumVirtualRows(1),
            _ => {}
        }
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        Default::default()
    }
    fn NewChunk(&self) -> chunk::Chunk {
        Default::default()
    }
    fn Schema(&self) -> &[crate::adapter::SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn crate::adapter::CascadeBatch>> {
        vec![]
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

#[test]
fn go_merge_187_terminal_once_suppression_and_first_outcome() {
    for case in 0..13 {
        let mut stmt = ru_terminal_stmt();
        let original_plan = stmt.TypedPlan.clone();
        let mut state = crate::statement_ru_result::StatementRUInstallState::default();
        let mut terminal_error = None;
        let mut panic_evidence = false;
        // First outcome, independent EOF and install/live classifications are all gates.
        if case != 0 {
            stmt.RecordStatementRUFinalOutcome(case != 1);
        }
        if case != 2 {
            stmt.recordStatementRURootEOF();
        }
        match case {
            3 => terminal_error = Some(errors::New("terminal failure")),
            4 => state.cursor_exists = true,
            5 => state.restricted_sql = true,
            6 => state.statement_context_present = false,
            7 => stmt.TypedPlan = None,
            8 => stmt.TypedPlan = Some(Arc::new(physicalop::PointGetPlan::New(ru_join_context()))),
            9 => panic_evidence = true,
            10 | 11 => {
                stmt.StatementCtx.statement_ru_owner = Some(Arc::new(StatementRUOwner::new(
                    Default::default(),
                    case == 11,
                    false,
                    case == 10,
                )));
                stmt.RecordStatementRUFinalOutcome(true);
                stmt.recordStatementRURootEOF();
            }
            12 => stmt.RecordStatementRUFinalOutcome(false), // A recorded success still wins.
            _ => {}
        }
        stmt.Ctx = Arc::new(RUTerminalRuntime {
            state,
            panic_evidence,
            stream_mode: 0,
            ..Default::default()
        });
        let result = stmt.finishStatementRU(terminal_error.as_ref());
        assert_eq!(result.is_some(), case == 12, "case {case}");
        stmt.Ctx = Arc::new(RUTerminalRuntime::default());
        stmt.TypedPlan = original_plan;
        stmt.RecordStatementRUFinalOutcome(true);
        stmt.recordStatementRURootEOF();
        assert!(
            stmt.finishStatementRU(None).is_none(),
            "case {case} must stay consumed"
        );
    }
    let mut stmt = ru_terminal_stmt();
    stmt.StatementCtx.statement_ru_owner = None;
    stmt.RecordStatementRUFinalOutcome(true);
    stmt.recordStatementRURootEOF();
    stmt.abortStatementRU();
    assert!(stmt.finishStatementRU(None).is_none());
}

#[test]
fn go_merge_187_terminal_once_stream_boundaries() {
    for mode in 0..=5 {
        let mut stmt = ru_terminal_stmt();
        stmt.Ctx = Arc::new(RUTerminalRuntime {
            stream_mode: mode,
            ..Default::default()
        });
        let owner = stmt.StatementCtx.statement_ru_owner.clone().unwrap();
        let mut rows = stmt.Exec().unwrap().unwrap();
        stmt.RecordStatementRUFinalOutcome(true);
        let mut output = rows.NewChunk();
        let read = rows.Next(&mut output);
        if mode >= 2 && mode != 4 {
            assert!(read.is_err());
        } else {
            assert!(read.is_ok());
        }
        assert_eq!(owner.root_eof(), mode == 0 || mode == 4);
        let close = rows.Close();
        assert_eq!(close.is_err(), mode == 4);
        assert!(owner.take_terminal_setup().is_none());
        assert!(stmt.finishStatementRU(None).is_none());
    }
    // Clones of ExecStmt share the owner. A full stream produces a value only once.
    let mut stmt = ru_terminal_stmt();
    stmt.Ctx = Arc::new(RUTerminalRuntime {
        stream_mode: 1,
        ..Default::default()
    });
    let owner = stmt.StatementCtx.statement_ru_owner.clone().unwrap();
    let mut rows = stmt.Exec().unwrap().unwrap();
    let mut output = rows.NewChunk();
    rows.Next(&mut output).unwrap();
    assert!(!owner.root_eof());
    rows.Next(&mut output).unwrap();
    assert!(owner.root_eof());
    stmt.RecordStatementRUFinalOutcome(true);
    let value = stmt.finishStatementRU(None).unwrap();
    assert_eq!(value.units.frontend_compile_bytes, 13.0);
    assert_eq!(value.units.operator_num, 1.0);
    rows.Close().unwrap();
    assert!(stmt.finishStatementRU(None).is_none());
}

#[test]
fn go_merge_187_terminal_once_no_delay_and_commit() {
    let mut stmt = ru_terminal_stmt();
    let owner = stmt.StatementCtx.statement_ru_owner.clone().unwrap();
    let mut executor = RUTerminalExecutor::new(6);
    stmt.handleNoDelayExecutor(&mut executor).unwrap();
    assert!(
        !owner.root_eof(),
        "a generic no-delay wrapper does not prove completion"
    );
    stmt.TypedPlan = Some(Arc::new(physicalop::Insert::New(ru_join_context())));
    stmt.handleNoDelayExecutor(&mut executor).unwrap();
    assert!(owner.root_eof());

    let mut stmt = ru_terminal_stmt();
    stmt.TypedPlan = Some(Arc::new(astersql_planner_core::RuntimeSimple::New(
        ru_join_context(),
        astersql_parser_ast::NodeRef::new(Box::new(astersql_parser_ast::CommitStmt::default())),
    )));
    stmt.StatementCtx.statement_ru_evidence = Some(Arc::new(
        crate::statement_ru_plan_walk::StatementRURuntimeEvidence {
            writes: Some(StatementRUWriteSnapshot { keys: 2, bytes: 58 }),
            tikv_response_bytes: Some(100),
            ..Default::default()
        },
    ));
    stmt.RecordStatementRUFinalOutcome(true);
    stmt.handleNoDelayExecutor(&mut executor).unwrap();
    let finalized = stmt.finishStatementRU(None).unwrap();
    assert_eq!(finalized.sql_type, "commit");
    assert_eq!(finalized.units.write_keys, 2.0);
    assert_eq!(finalized.units.write_bytes, 58.0);
    assert_eq!(
        finalized.units.net_bytes, 0.0,
        "COMMIT has no cop transport charge"
    );
    assert!(stmt.finishStatementRU(None).is_none());
}

#[test]
fn go_merge_187_terminal_once_reentry_invalid_and_ttl() {
    let mut stmt = ru_terminal_stmt();
    let runtime = Arc::new(RUTerminalRuntime::default());
    stmt.Ctx = runtime.clone();
    stmt.RecordStatementRUFinalOutcome(true);
    stmt.recordStatementRURootEOF();
    runtime.reentrant_stmt.replace(Some(stmt.clone()));
    assert!(stmt.finishStatementRU(None).is_some());
    assert!(runtime.reentrant_stmt.borrow().is_none());
    assert!(stmt.finishStatementRU(None).is_none());

    let mut invalid = ru_terminal_stmt();
    invalid.RecordStatementRUFinalOutcome(true);
    invalid.recordStatementRURootEOF();
    invalid.StatementCtx.statement_ru_evidence = Some(Arc::new(
        crate::statement_ru_plan_walk::StatementRURuntimeEvidence {
            tikv_response_bytes: Some(-1),
            ..Default::default()
        },
    ));
    assert!(invalid.finishStatementRU(None).is_none());
    invalid.StatementCtx.statement_ru_evidence = None;
    assert!(invalid.finishStatementRU(None).is_none());

    let mut ttl = ru_terminal_stmt();
    ttl.StatementCtx.statement_ru_owner = Some(Arc::new(StatementRUOwner::new(
        Default::default(),
        true,
        true,
        false,
    )));
    ttl.RecordStatementRUFinalOutcome(true);
    ttl.recordStatementRURootEOF();
    // Install-time TTL attribution survives restoration of the live restricted state.
    assert!(ttl.finishStatementRU(None).is_some());

    let mut point = ru_terminal_stmt();
    point.TypedPlan = Some(Arc::new(physicalop::PointGetPlan::New(ru_join_context())));
    point.RecordStatementRUFinalOutcome(true);
    point.recordStatementRURootEOF();
    point.StatementCtx.statement_ru_evidence = Some(Arc::new(
        crate::statement_ru_plan_walk::StatementRURuntimeEvidence {
            point: Some(crate::statement_ru_plan_walk::StatementRUPointSnapshot {
                valid: true,
                scan_detail_complete: true,
                payload_complete: true,
                payload_bytes: 40,
                total_keys: 2,
                processed_keys: 2,
                processed_bytes: 58,
                ..Default::default()
            }),
            ..Default::default()
        },
    ));
    let finalized = point.finishStatementRU(None).unwrap();
    assert_eq!(finalized.units.scan_bytes, 58.0);
    assert_eq!(finalized.units.net_bytes, 40.0);
    assert_eq!(finalized.units.operator_num, 1.0);
    assert!(point.finishStatementRU(None).is_none());
}

#[test]
fn go_merge_187_terminal_once_buffered_and_execution_failures() {
    let mut stmt = ru_terminal_stmt();
    let owner = stmt.StatementCtx.statement_ru_owner.clone().unwrap();
    let mut executor = RUTerminalExecutor::new(1);
    let mut buffered = stmt.runPessimisticSelectForUpdate(&mut executor).unwrap();
    assert!(
        owner.root_eof(),
        "execution EOF precedes draining buffered rows"
    );
    stmt.RecordStatementRUFinalOutcome(true);
    buffered.Close().unwrap();
    assert!(owner.take_terminal_setup().is_none());

    let mut stmt = ru_terminal_stmt();
    let owner = stmt.StatementCtx.statement_ru_owner.clone().unwrap();
    stmt.TypedPlan = Some(Arc::new(astersql_planner_core::RuntimeAnalyze::New(
        ru_join_context(),
        astersql_planner_core::Analyze::default(),
    )));
    stmt.handleNoDelayExecutor(&mut RUTerminalExecutor::new(6))
        .unwrap();
    assert!(owner.root_eof());

    for fast in [false, true] {
        let mut stmt = ru_terminal_stmt();
        // Builds successfully, then Open panics. Both real execution entrypoints abort.
        stmt.Ctx = Arc::new(RUTerminalRuntime {
            stream_mode: 7,
            ..Default::default()
        });
        let owner = stmt.StatementCtx.statement_ru_owner.clone().unwrap();
        if fast {
            assert!(stmt.PointGet().is_err());
        } else {
            assert!(stmt.Exec().is_err());
        }
        assert_eq!(owner.final_outcome(), StatementRUFinalOutcome::Failure);
        assert!(owner.take_terminal_setup().is_none());
        assert!(stmt.finishStatementRU(None).is_none());
    }
}

#[test]
fn go_merge_195_197_publish_snapshot_real_terminal_once() {
    use astersql_metrics::ru_v2;
    let (total, units, statements) = {
        let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe {
            if (&*std::ptr::addr_of!(ru_v2::RUV2Total)).is_none() {
                ru_v2::InitRUV2Metrics();
            }
        }
        unsafe {
            (
                (&*std::ptr::addr_of!(ru_v2::RUV2Total)).clone().unwrap(),
                (&*std::ptr::addr_of!(ru_v2::RUV2Unit)).clone().unwrap(),
                (&*std::ptr::addr_of!(ru_v2::RUV2Statements))
                    .clone()
                    .unwrap(),
            )
        }
    };
    let before_total = total.get();
    let frontend = units.with_label_values(&["tidb", "sql_frontend", "frontend_compile_bytes"]);
    let before_frontend = frontend.get();
    let success = statements.with_label_values(&["success", "incomplete"]);
    let before_success = success.get();
    let runtime = Arc::new(RUTerminalRuntime::default());
    let mut stmt = ru_terminal_stmt();
    stmt.Ctx = runtime.clone();
    stmt.StatementCtx.statement_ru_owner = Some(Arc::new(StatementRUOwner::new(
        StatementRUCalculationSetup {
            frontend_compile_bytes: 13.0,
            full_report: true,
        },
        false,
        false,
        false,
    )));
    stmt.RecordStatementRUFinalOutcome(true);
    stmt.recordStatementRURootEOF();
    stmt.FinishExecuteStmt(0, None, false);
    let snapshot = stmt.StatementCtx.statement_ru_finalized.clone().unwrap();
    assert_eq!(
        *runtime.published.borrow(),
        [(
            "ru_test".into(),
            snapshot.engine_ru.tikv,
            snapshot.engine_ru.tidb,
            snapshot.engine_ru.tiflash
        )]
    );
    assert_eq!(*runtime.topsql.borrow(), [snapshot.result.total_ru]);
    assert_eq!(*runtime.calibration.borrow(), [snapshot.units]);
    stmt.FinishExecuteStmt(0, None, false);
    assert_eq!(runtime.published.borrow().len(), 1);
    assert_eq!(runtime.calibration.borrow().len(), 1);
    assert_eq!(runtime.topsql.borrow().len(), 1);
    assert_eq!(stmt.StatementCtx.total_ru, snapshot.result.total_ru);
    assert_eq!(total.get() - before_total, snapshot.result.total_ru);
    assert_eq!(frontend.get() - before_frontend, 13.0);
    assert_eq!(success.get() - before_success, 1.0);
    let failure = statements.with_label_values(&["failed", "statement_error"]);
    let before_failure = failure.get();
    let mut aborted = ru_terminal_stmt();
    aborted.StatementCtx.statement_ru_owner = Some(Arc::new(StatementRUOwner::new(
        StatementRUCalculationSetup {
            frontend_compile_bytes: 13.0,
            full_report: true,
        },
        false,
        false,
        false,
    )));
    aborted.RecordStatementRUFinalOutcome(false);
    aborted.abortStatementRU();
    aborted.FinishExecuteStmt(0, None, false);
    assert_eq!(failure.get() - before_failure, 1.0);
    assert_eq!(total.get() - before_total, snapshot.result.total_ru);
    let ttl_counter = {
        let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe { (&*std::ptr::addr_of!(ru_v2::RUV2TTLTotal)).clone().unwrap() }
    };
    let before_ttl = ttl_counter.get();
    let mut ttl = ru_terminal_stmt();
    ttl.StatementCtx.statement_ru_owner = Some(Arc::new(StatementRUOwner::new(
        StatementRUCalculationSetup {
            frontend_compile_bytes: 13.0,
            full_report: false,
        },
        true,
        true,
        false,
    )));
    ttl.RecordStatementRUFinalOutcome(true);
    ttl.recordStatementRURootEOF();
    ttl.FinishExecuteStmt(0, None, false);
    let ttl_result = ttl
        .StatementCtx
        .statement_ru_finalized
        .as_ref()
        .unwrap()
        .result
        .total_ru;
    ttl.FinishExecuteStmt(0, None, false);
    assert_eq!(ttl_counter.get() - before_ttl, ttl_result);
    assert_eq!(
        total.get() - before_total,
        snapshot.result.total_ru + ttl_result
    );
}

#[test]
fn go_merge_20_187_195_197_production_ru_legacy_consumer_never_publishes() {
    use crate::adapter::ExecutionContext;
    use astersql_util_execdetails::ruv2_metrics::kvrpcpb;
    for version in [1, 2] {
        for finished in [false, true] {
            let runtime = Arc::new(RUTerminalRuntime {
                ru_version: Some(version),
                ..Default::default()
            });
            let mut stmt = ru_terminal_stmt();
            stmt.Ctx = runtime.clone();
            let metrics = Arc::new(NewRUV2Metrics());
            let details = Arc::new(tikvutil::RUDetails::default());
            details.AddTiKVRUV2(999.0);
            let mut response = kvrpcpb::Ruv2::new();
            response.set_coprocessor_response_bytes(17);
            details.AddRUV2(&response);
            stmt.StatementCtx.ru_metrics = Some(metrics.clone());
            stmt.GoCtx = Some(ExecutionContext {
                ru_details: Some(details),
                ..Default::default()
            });
            stmt.RecordStatementRUFinalOutcome(finished);
            if finished {
                stmt.recordStatementRURootEOF();
            }
            stmt.FinishExecuteStmt(0, None, false);
            assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 17);
            let expected = usize::from(finished);
            assert_eq!(
                runtime.published.borrow().len(),
                expected,
                "RUVersion {version}: legacy RU must not be published alongside terminal RU"
            );
            if finished {
                let snapshot = stmt.StatementCtx.statement_ru_finalized.clone().unwrap();
                assert_eq!(runtime.published.borrow()[0].2, snapshot.engine_ru.tidb);
                assert_eq!(runtime.published.borrow()[0].1, snapshot.engine_ru.tikv);
                assert_eq!(stmt.StatementCtx.total_ru, snapshot.result.total_ru);
            } else {
                assert_eq!(stmt.StatementCtx.total_ru, 0.0);
                assert!(stmt.StatementCtx.statement_ru_finalized.is_none());
            }
            stmt.FinishExecuteStmt(0, None, false);
            assert_eq!(runtime.published.borrow().len(), expected);
            assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 17);
        }
    }
}

#[test]
fn go_merge_20_187_195_197_production_ru_canonical_sql() {
    use std::sync::Mutex;
    #[derive(Default)]
    struct Reporter(Mutex<Vec<(f64, f64, f64)>>);
    impl astersql_domain::ruv2_reporter::RUV2ConsumptionReporter for Reporter {
        fn report_ruv2_consumption(&self, _: &str, tikv: f64, tidb: f64, tiflash: f64) {
            self.0.lock().unwrap().push((tikv, tidb, tiflash));
        }
    }
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    domain.set_ru_version(2);
    session
        .execute("create table ru_production (id int primary key, v int)")
        .unwrap();
    session
        .execute("insert into ru_production values (1, 10), (2, 20)")
        .unwrap();
    let reporter = Arc::new(Reporter::default());
    domain.bind_ruv2_consumption_reporter(Some(reporter.clone()));
    for sql in [
        "select v from ru_production where id = 1",
        "select v from ru_production where id >= 1 limit 1",
    ] {
        reporter.0.lock().unwrap().clear();
        let prepared = session
            .PreparePlannedKVSelect(sql, domain.info_schema())
            .unwrap();
        let result = session
            .ExecutePreparedPlannedKVSelectThroughAdapter(prepared, &[])
            .unwrap();
        assert_eq!(result.Rows.len(), 1);
        let published = reporter.0.lock().unwrap().clone();
        assert_eq!(
            published.len(),
            1,
            "{sql}: complete canonical execution publishes once"
        );
        assert!(published[0].1 > 0.0);
        assert_eq!(published[0].2, 0.0);
    }
    domain.bind_ruv2_consumption_reporter(None);
    domain.close();
}

#[test]
fn go_merge_20_187_195_197_production_ru_unbridged_sql_has_no_estimated_publication() {
    use astersql_session::testutil::TestRecordSet;
    use std::sync::Mutex;
    #[derive(Default)]
    struct Reporter(Mutex<Vec<(f64, f64, f64)>>);
    impl astersql_domain::ruv2_reporter::RUV2ConsumptionReporter for Reporter {
        fn report_ruv2_consumption(&self, _: &str, tikv: f64, tidb: f64, tiflash: f64) {
            self.0.lock().unwrap().push((tikv, tidb, tiflash));
        }
    }
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    domain.set_ru_version(2);
    session
        .execute("create table ru_unbridged (id int primary key)")
        .unwrap();
    let reporter = Arc::new(Reporter::default());
    domain.bind_ruv2_consumption_reporter(Some(reporter.clone()));
    // These canonical dispatch branches currently bypass ExecStmt. Keep the gap
    // observable: no legacy/estimated RU may hide the missing production bridge.
    for sql in [
        "select 1",
        "insert into ru_unbridged values (1)",
        "analyze table ru_unbridged",
    ] {
        for mut result in session.execute(sql).unwrap() {
            while result.Next().unwrap().is_some() {}
            result.Close().unwrap();
        }
        assert!(
            reporter.0.lock().unwrap().is_empty(),
            "{sql}: unbridged SQL must not publish a fabricated RU"
        );
    }
    domain.bind_ruv2_consumption_reporter(None);
    domain.close();
}

#[test]
fn statement_ru_concurrent_terminal_has_one_consumer() {
    let owner = Arc::new(StatementRUOwner::new(
        Default::default(),
        false,
        false,
        false,
    ));
    assert!(owner.record_final_outcome(true));
    let start = std::sync::Barrier::new(32);
    let consumed = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..32 {
            let owner = &owner;
            let start = &start;
            let consumed = &consumed;
            scope.spawn(move || {
                start.wait();
                if owner.take_terminal_setup().is_some() {
                    consumed.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    assert_eq!(consumed.load(Ordering::Relaxed), 1);
    assert_eq!(owner.final_outcome(), StatementRUFinalOutcome::Success);
    assert!(owner.take_terminal_setup().is_none());
}

#[test]
fn statement_ru_concurrent_outcomes_keep_the_first_record() {
    let owner = StatementRUOwner::new(Default::default(), false, false, false);
    let start = std::sync::Barrier::new(32);
    let recorded = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for index in 0..32 {
            let owner = &owner;
            let start = &start;
            let recorded = &recorded;
            scope.spawn(move || {
                start.wait();
                if owner.record_final_outcome(index % 2 == 0) {
                    recorded.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    assert_eq!(recorded.load(Ordering::Relaxed), 1);
    let outcome = owner.final_outcome();
    assert_ne!(outcome, StatementRUFinalOutcome::Unknown);
    assert!(!owner.record_final_outcome(outcome != StatementRUFinalOutcome::Success));
    assert_eq!(owner.final_outcome(), outcome);
    assert_eq!(
        owner.take_terminal_setup().is_some(),
        outcome == StatementRUFinalOutcome::Success
    );
    assert!(owner.take_terminal_setup().is_none());
}

#[test]
fn statement_ru_reader_terminal_freezes_scan_and_transport_once() {
    let total = {
        let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        unsafe {
            if (&*std::ptr::addr_of!(astersql_metrics::ru_v2::RUV2Total)).is_none() {
                astersql_metrics::ru_v2::InitRUV2Metrics();
            }
            (&*std::ptr::addr_of!(astersql_metrics::ru_v2::RUV2Total))
                .clone()
                .unwrap()
        }
    };
    let before_total = total.get();

    use crate::statement_ru_plan_walk::snapshot_statement_ru_runtime_evidence;
    use astersql_util_execdetails::execdetails as exec;

    let context = ru_join_context();
    let scan = physicalop::PhysicalTableScan::New(context.clone());
    let scan_id = base::Plan::id(&scan);
    let mut reader = physicalop::PhysicalTableReader::New(context);
    reader.TablePlan = Some(Box::new(scan));
    let reader_id = base::Plan::id(&reader);
    let mut collector = exec::NewRuntimeStatsColl(None);
    collector.RecordCopStats(
        scan_id,
        exec::kv::TiKV,
        Some(&exec::util::ScanDetail {
            TotalKeys: 1,
            ProcessedKeys: 1,
            ProcessedKeysSize: 10,
            ..Default::default()
        }),
        Default::default(),
        None,
        None,
    );
    let metrics = NewRUV2Metrics();
    metrics.AddTiKVCoprocessorResponseBytes(20);
    let frozen = snapshot_statement_ru_runtime_evidence(
        Some(&collector),
        &[reader_id, scan_id],
        None,
        None,
        Some(&metrics),
    );
    // Changing the producer after the snapshot must not change terminal units.
    collector.RecordCopStats(
        scan_id,
        exec::kv::TiKV,
        Some(&exec::util::ScanDetail {
            ProcessedKeysSize: 99,
            ..Default::default()
        }),
        Default::default(),
        None,
        None,
    );
    metrics.AddTiKVCoprocessorResponseBytes(99);
    let runtime = Arc::new(RUTerminalRuntime::default());
    let mut stmt = ru_terminal_stmt();
    stmt.Ctx = runtime.clone();
    stmt.TypedPlan = Some(Arc::new(reader));
    stmt.StatementCtx.statement_ru_evidence = Some(Arc::new(frozen));
    stmt.RecordStatementRUFinalOutcome(true);
    stmt.recordStatementRURootEOF();
    stmt.FinishExecuteStmt(0, None, false);
    let snapshot = stmt.StatementCtx.statement_ru_finalized.clone().unwrap();
    assert_eq!(snapshot.units.scan_bytes, 10.0);
    assert_eq!(snapshot.units.net_bytes, 20.0);
    assert_eq!(snapshot.units.frontend_compile_bytes, 13.0);
    assert_eq!(snapshot.units.operator_num, 2.0);
    assert_eq!(runtime.published.borrow().len(), 1);
    assert_eq!(runtime.published.borrow()[0].1, snapshot.engine_ru.tikv);
    assert_eq!(snapshot.engine_ru.tikv, 31.0);
    assert_eq!(snapshot.engine_ru.tidb, 14.0);
    assert_eq!(snapshot.result.total_ru, 45.0);
    assert_eq!(total.get() - before_total, 45.0);
    stmt.FinishExecuteStmt(0, None, false);
    assert_eq!(runtime.published.borrow().len(), 1);
    assert_eq!(total.get() - before_total, 45.0);
    assert!(Arc::ptr_eq(
        &snapshot,
        stmt.StatementCtx.statement_ru_finalized.as_ref().unwrap()
    ));
}
