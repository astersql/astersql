// Copyright 2026 AsterSQL.

use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode};
use crate::physical_index_merge_join::PhysicalIndexMergeJoin;

fn node(kind: PhysicalKind) -> PhysicalPlanNode {
    PhysicalPlanNode {
        id: 1,
        kind,
        schema: Vec::new(),
        children: Vec::new(),
        stats: Default::default(),
        required_properties: Vec::new(),
    }
}

fn join() -> PhysicalIndexMergeJoin {
    PhysicalIndexMergeJoin {
        outer: node(PhysicalKind::Scan { table_id: 1 }),
        inner: node(PhysicalKind::Scan { table_id: 2 }),
        key_offset_order: Vec::new(),
        compare_functions: Vec::new(),
        outer_compare_functions: Vec::new(),
        need_outer_sort: false,
        descending: false,
        concurrency: 1,
    }
}

#[test]
fn memory_usage_counts_reserved_compare_function_slots_like_go() {
    let baseline = join();
    let mut reserved = join();
    reserved.compare_functions.reserve_exact(3);
    reserved.outer_compare_functions.reserve_exact(2);

    assert_eq!(
        reserved.memory_usage() - baseline.memory_usage(),
        ((reserved.compare_functions.capacity() + reserved.outer_compare_functions.capacity())
            * std::mem::size_of::<String>()) as i64,
    );
}
