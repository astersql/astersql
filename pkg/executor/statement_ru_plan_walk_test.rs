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
