// Copyright 2024 PingCAP, Inc. Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

use std::collections::HashSet;

use logicalop_dependency::{LogicalCTE, LogicalPlan, LogicalPlanRef};

/// Fill `IsOuterMostCTE` for every CTE reachable from the logical plan.
///
/// A CTE referenced from the main query may receive predicates from that
/// query.  A CTE referenced from another CTE must not be treated as an outer
/// consumer.  The shared class is marked false as soon as any nested
/// reference is found, matching Go's visited-order semantics.
pub fn RecheckCTE(plan: &mut LogicalPlanRef) {
    let mut visited = HashSet::new();
    recheck_logical_ctes(plan.as_mut(), true, &mut visited);
}

/// Compatibility alias for runtime builder code migrated before the Go API
/// name was restored.
pub fn RecheckLogicalCTE(plan: &mut LogicalPlanRef) {
    RecheckCTE(plan);
}

fn recheck_logical_ctes(
    plan: &mut dyn LogicalPlan,
    is_root_tree: bool,
    visited: &mut HashSet<i32>,
) {
    if let Some(cte) = plan.as_any_mut().downcast_mut::<LogicalCTE>() {
        let (class, storage_id) = {
            let class = cte.Cte.borrow();
            (cte.Cte.clone(), class.IDForStorage)
        };
        if !is_root_tree {
            class.borrow_mut().IsOuterMostCTE = false;
        }
        if !visited.insert(storage_id) {
            return;
        }
        class.borrow_mut().IsOuterMostCTE = is_root_tree;

        let (mut seed, mut recursive) = {
            let mut class = class.borrow_mut();
            (
                class.SeedPartLogicalPlan.take(),
                class.RecursivePartLogicalPlan.take(),
            )
        };
        if let Some(seed_plan) = seed.as_mut() {
            recheck_logical_ctes(seed_plan.as_mut(), false, visited);
        }
        if let Some(recursive_plan) = recursive.as_mut() {
            recheck_logical_ctes(recursive_plan.as_mut(), false, visited);
        }
        let mut class = class.borrow_mut();
        class.SeedPartLogicalPlan = seed;
        class.RecursivePartLogicalPlan = recursive;
        return;
    }
    for child in plan.Children_mut() {
        recheck_logical_ctes(child.as_mut(), is_root_tree, visited);
    }
}
