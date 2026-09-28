// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use base::{JoinType, PhysicalPlan, Plan};

use crate::ColWithCmpFuncManager;
use crate::{BasePhysicalJoin, BasePhysicalPlan, PhysicalIndexJoin, PhysicalSchemaProducer};

struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        std::process::abort()
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        std::process::abort()
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

fn column(id: i64, index: isize) -> expression::Column {
    expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        id,
        id,
        index,
    )
}

#[test]
fn index_hash_join_adds_remaining_cross_child_equality_keys_like_go() {
    let orders_key = column(11, 0);
    let supplier_key = column(12, 1);
    let lineitem_order_key = column(21, 0);
    let lineitem_supplier_key = column(22, 1);
    let outer_schema = expression::NewSchema(vec![orders_key.Clone(), supplier_key.Clone()]);
    let inner_schema = expression::NewSchema(vec![
        lineitem_order_key.Clone(),
        lineitem_supplier_key.Clone(),
    ]);
    let mut outer_keys = vec![orders_key.Clone()];
    let mut inner_keys = vec![lineitem_order_key.Clone()];

    assert!(super::append_outer_hash_key_pair(
        &outer_schema,
        &inner_schema,
        &lineitem_supplier_key,
        &supplier_key,
        &mut outer_keys,
        &mut inner_keys,
    ));
    assert_eq!(
        outer_keys
            .iter()
            .map(|key| key.UniqueID)
            .collect::<Vec<_>>(),
        vec![11, 12]
    );
    assert_eq!(
        inner_keys
            .iter()
            .map(|key| key.UniqueID)
            .collect::<Vec<_>>(),
        vec![21, 22]
    );
    assert!(!super::append_outer_hash_key_pair(
        &outer_schema,
        &inner_schema,
        &supplier_key,
        &lineitem_supplier_key,
        &mut outer_keys,
        &mut inner_keys,
    ));
}

fn scan(ctx: base::ContextRef, columns: Vec<expression::Column>) -> crate::PhysicalTableScan {
    let mut scan = crate::PhysicalTableScan::New(ctx);
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(columns));
    scan
}

#[test]
fn compare_filter_memory_usage_counts_owned_operator_and_constants() {
    let mut manager = ColWithCmpFuncManager::New(None, -1);
    let empty = manager.MemoryUsage();

    manager
        .OpType
        .push("greater-than-with-owned-storage".to_owned());
    manager
        .TmpConstant
        .push(expression::Constant::null(mysql::r#type::TypeLonglong));

    assert!(
        manager.MemoryUsage() > empty,
        "Go parity requires owned operator strings and temporary constants to be counted"
    );
}

#[test]
fn resolve_indices_covers_conditions_compare_schema_and_duplicate_outputs() {
    let ctx = context();
    let left_first = column(11, 0);
    let left_duplicate = column(11, 1);
    let right = column(22, 0);
    let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(ctx.clone(), "IndexJoin", 0));
    let mut join = PhysicalIndexJoin::New(BasePhysicalJoin::New(producer, JoinType::InnerJoin));
    join.set_children(vec![
        Box::new(scan(
            ctx.clone(),
            vec![left_first.Clone(), left_duplicate.Clone()],
        )),
        Box::new(scan(ctx, vec![right.Clone()])),
    ]);
    join.BasePhysicalJoin
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![
            column(11, -1),
            column(11, -1),
            column(22, -1),
        ]));
    join.BasePhysicalJoin.LeftConditions = vec![Box::new(column(11, -1))];
    join.BasePhysicalJoin.RightConditions = vec![Box::new(column(22, -1))];
    join.BasePhysicalJoin.OtherConditions = vec![Box::new(column(22, -1))];
    let mut compare = ColWithCmpFuncManager::New(Some(column(11, -1)), -1);
    compare.AffectedColSchema = expression::NewSchema(vec![column(11, -1)]);
    join.CompareFilters = Some(compare);

    join.ResolveIndices().unwrap();

    assert_eq!(
        join.BasePhysicalJoin.LeftConditions[0]
            .as_column()
            .unwrap()
            .Index,
        0
    );
    assert_eq!(
        join.BasePhysicalJoin.RightConditions[0]
            .as_column()
            .unwrap()
            .Index,
        0
    );
    assert_eq!(
        join.BasePhysicalJoin.OtherConditions[0]
            .as_column()
            .unwrap()
            .Index,
        2
    );
    assert_eq!(
        join.CompareFilters
            .as_ref()
            .unwrap()
            .AffectedColSchema
            .Columns[0]
            .Index,
        0
    );
    assert_eq!(
        join.BasePhysicalJoin
            .PhysicalSchemaProducer
            .Schema()
            .Columns
            .iter()
            .map(|column| column.Index)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
fn resolve_indices_accepts_reordered_outputs_without_reusing_duplicate_children() {
    let ctx = context();
    let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(ctx.clone(), "IndexJoin", 0));
    let mut join = PhysicalIndexJoin::New(BasePhysicalJoin::New(producer, JoinType::InnerJoin));
    join.set_children(vec![
        Box::new(scan(ctx.clone(), vec![column(11, 0), column(11, 1)])),
        Box::new(scan(ctx, vec![column(22, 0), column(33, 1)])),
    ]);
    join.BasePhysicalJoin
        .PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![
            column(22, -1),
            column(11, -1),
            column(33, -1),
            column(11, -1),
        ]));

    join.ResolveIndices().unwrap();

    assert_eq!(
        join.BasePhysicalJoin
            .PhysicalSchemaProducer
            .Schema()
            .Columns
            .iter()
            .map(|column| column.Index)
            .collect::<Vec<_>>(),
        [2, 0, 3, 1]
    );
}
