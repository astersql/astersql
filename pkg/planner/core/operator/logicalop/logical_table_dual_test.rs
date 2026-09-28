// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

struct TestPlanContext {
    plan_id: AtomicI32,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("logical table dual tests do not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("logical table dual tests do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("logical table dual tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("logical table dual tests do not perform null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("logical table dual tests do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

#[test]
fn init_and_explain_preserve_go_row_count_contract() {
    let dual = LogicalTableDual {
        RowCount: 2,
        ..LogicalTableDual::default()
    }
    .Init(context(), 7);

    assert_eq!(dual.RowCount, 2, "Go Init does not normalize RowCount");
    assert_eq!(dual.ExplainInfo(), "rowcount:2");
}

#[test]
fn hash_code_encodes_type_query_block_and_row_count_like_go() {
    let dual = LogicalTableDual {
        RowCount: -1,
        ..LogicalTableDual::default()
    }
    .Init(context(), 7);
    let mut expected = Vec::with_capacity(12);
    expected.extend_from_slice(
        &(plancodec::TypeStringToPhysicalID(plancodec::TypeDual) as u32).to_be_bytes(),
    );
    expected.extend_from_slice(&7_u32.to_be_bytes());
    expected.extend_from_slice(&u32::MAX.to_be_bytes());

    assert_eq!(dual.HashCode(), expected);
}

#[test]
fn pruning_all_unused_columns_can_leave_a_zero_column_dual() {
    let mut first = Column::default();
    first.UniqueID = 1;
    let mut second = Column::default();
    second.UniqueID = 2;
    let mut dual = LogicalTableDual::default();
    dual.SetSchema(expression::NewSchema(vec![first, second]));

    dual.PruneColumns(&[]).expect("dual pruning cannot fail");

    assert!(dual.Schema().Columns.is_empty());
}

#[test]
fn key_info_marks_only_exactly_one_row_like_go() {
    for (row_count, expected) in [(0, false), (1, true), (2, false), (-1, false)] {
        let mut dual = LogicalTableDual {
            RowCount: row_count,
            ..LogicalTableDual::default()
        };
        dual.SetMaxOneRow(true);

        dual.BuildKeyInfo();

        assert_eq!(dual.MaxOneRow(), expected, "RowCount={row_count}");
    }
}

#[test]
fn stats_cover_columns_and_respect_the_reload_cache_flag() {
    let mut first = Column::default();
    first.UniqueID = 11;
    let mut second = Column::default();
    second.UniqueID = 12;
    let mut dual = LogicalTableDual {
        RowCount: 1,
        ..LogicalTableDual::default()
    };
    dual.SetSchema(expression::NewSchema(vec![first, second]));

    let (derived, changed) = dual.DeriveStats(false).expect("stats derive succeeds");
    assert!(changed);
    assert_eq!(derived.RowCount, 1.0);
    assert_eq!(derived.ColNDVs.get(&11), Some(&1.0));
    assert_eq!(derived.ColNDVs.get(&12), Some(&1.0));

    dual.RowCount = 0;
    let (cached, changed) = dual.DeriveStats(false).expect("cached stats are reused");
    assert!(!changed);
    assert_eq!(cached.RowCount, 1.0);

    let (reloaded, changed) = dual.DeriveStats(true).expect("stats reload succeeds");
    assert!(changed);
    assert_eq!(reloaded.RowCount, 0.0);
    assert_eq!(reloaded.ColNDVs.get(&11), Some(&0.0));
    assert_eq!(reloaded.ColNDVs.get(&12), Some(&0.0));
}
