// Copyright 2026 AsterSQL.

#![allow(non_snake_case)]

use crate::{NewSimplePlanColumnIDAllocator, PlanColumnIDAllocator};

/// Go atomic.Int64.Add wraps on signed overflow; keep the allocator result and
/// stored last ID consistent at the boundary.
#[test]
fn plan_column_id_allocator_wraps_like_go_atomic_int64() {
    let allocator = NewSimplePlanColumnIDAllocator(i64::MAX);

    assert_eq!(allocator.AllocPlanColumnID(), i64::MIN);
    assert_eq!(allocator.GetLastPlanColumnID(), i64::MIN);
}
