// Copyright 2026 AsterSQL.

use super::physical_common_plans::{
    Datum, PartitionType, PhysicalExpr, PhysicalKind, PhysicalPlanNode, PhysicalProperty, SortItem,
    Stats, TaskType,
};
use super::physical_cte::{
    CteDefinition, PhysicalCTE, PhysicalCTEDefinition, PhysicalCte, PhysicalCteSink,
    PhysicalCteStorage, exhaust_physical_cte,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

struct TypedCTEPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TypedCTEPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        unreachable!()
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
fn typed_cte_references_share_real_seed_and_recursive_plans() {
    let context: base::ContextRef = Arc::new(TypedCTEPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ));
    let definition = Arc::new(PhysicalCTEDefinition::New(
        context.clone(),
        42,
        Box::new(crate::PhysicalTableDual::New(context.clone(), 1)),
        Some(Box::new(crate::PhysicalTableDual::New(context.clone(), 2))),
    ));
    let first = PhysicalCTE::New(context.clone(), definition.clone());
    let second = PhysicalCTE::New(context, definition);
    assert!(Arc::ptr_eq(&first.CTE, &second.CTE));
    assert_eq!(first.CTE.IDForStorage, 42);
    assert!(first.CTE.SeedPlan.as_any().is::<crate::PhysicalTableDual>());
    assert!(
        first
            .CTE
            .RecurPlan
            .as_ref()
            .unwrap()
            .as_any()
            .is::<crate::PhysicalTableDual>()
    );
    assert!(base::Plan::as_physical_plan(&first).is_some());
}

fn node(kind: PhysicalKind, children: Vec<PhysicalPlanNode>) -> PhysicalPlanNode {
    PhysicalPlanNode {
        id: 1,
        kind,
        schema: Vec::new(),
        children,
        stats: Stats::default(),
        required_properties: Vec::new(),
    }
}

fn cte(seed_plan: Option<PhysicalPlanNode>) -> PhysicalCte {
    PhysicalCte {
        id_for_storage: 42,
        seed_plan,
        recursive_plan: None,
        distinct: false,
        seed_statistics: Stats::default(),
        recursive_statistics: Stats::default(),
        result_statistics: Stats::default(),
    }
}

#[test]
fn storage_explain_info_matches_go() {
    let storage = PhysicalCteStorage(cte(None));
    assert_eq!(storage.explain_info(), "Non-Recursive CTE Storage");
}

#[test]
fn definition_explain_info_reports_recursion_and_keeps_storage_id_separate() {
    let recursive = node(PhysicalKind::Cte { id: 42 }, Vec::new());
    let mut plan = cte(None);
    plan.recursive_plan = Some(recursive);
    let definition = CteDefinition(plan);
    assert_eq!(definition.explain_info(), "Recursive CTE");
    assert_eq!(definition.explain_id(), "CTE_42");
}

#[test]
fn correlated_columns_include_nested_scalar_arguments() {
    let seed = node(
        PhysicalKind::Selection {
            predicates: vec![PhysicalExpr::Scalar {
                function: "plus".into(),
                args: vec![
                    PhysicalExpr::Constant(Datum::Int(1)),
                    PhysicalExpr::CorrelatedColumn(7),
                ],
            }],
        },
        Vec::new(),
    );
    assert_eq!(cte(Some(seed)).correlated_columns(), [7]);
}

#[test]
fn logical_cte_enumeration_preserves_requested_child_property() {
    let property = PhysicalProperty {
        task_type: TaskType::Mpp,
        sort_items: vec![SortItem {
            column: 9,
            descending: true,
        }],
        expected_count: 12.0,
        partition_type: PartitionType::Hash,
        partition_columns: vec![9],
        can_add_enforcer: false,
        ..PhysicalProperty::default()
    };
    let stats = Stats {
        row_count: 3.0,
        version: 11,
    };

    let (plans, complete) = exhaust_physical_cte(42, &property, stats.clone());

    assert!(complete);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].kind, PhysicalKind::CteStorage { id: 42 });
    assert_eq!(plans[0].stats, stats);
    assert_eq!(plans[0].required_properties, [property]);
}

#[test]
fn sink_pb_uses_fragment_counts_and_clone_drops_task_slices() {
    let sink = PhysicalCteSink {
        id_for_storage: 42,
        compression_mode: "fast".into(),
        self_tasks: vec![1, 2, 3],
        target_tasks: vec![4],
        cte_source_num: 8,
        cte_sink_num: 5,
        child: node(PhysicalKind::Scan { table_id: 7 }, Vec::new()),
    };

    let pb = sink.to_pb().unwrap();
    assert_eq!(pb.source_count, 8);
    assert_eq!(pb.sink_count, 5);

    let cloned = sink.clone();
    assert!(cloned.self_tasks.is_empty());
    assert!(cloned.target_tasks.is_empty());
    assert_eq!(cloned.cte_source_num, 8);
    assert_eq!(cloned.cte_sink_num, 5);
}
