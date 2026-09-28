// Copyright 2026 AsterSQL.

use crate::{PlanKind, PlanNode, StoreType, isPlanCacheable};

#[test]
fn tiflash_table_reader_is_not_plan_cacheable() {
    let mut plan = PlanNode::New(1, PlanKind::TableReader, Vec::new());
    plan.store_type = StoreType::TiFlash;

    assert_eq!(
        isPlanCacheable(&plan, 0, i64::MAX),
        (false, "TiFlash plan is un-cacheable".to_owned())
    );
}
