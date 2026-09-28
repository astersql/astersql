// Copyright 2026 AsterSQL.

use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode, Stats};
use crate::physical_indexlookup::{PhysicalLocalIndexLookup, detach_root_table_scan};

fn node(
    id: i64,
    kind: PhysicalKind,
    schema: Vec<i64>,
    children: Vec<PhysicalPlanNode>,
) -> PhysicalPlanNode {
    PhysicalPlanNode {
        id,
        kind,
        schema,
        children,
        stats: Stats::default(),
        required_properties: Vec::new(),
    }
}

fn scan(id: i64, schema: Vec<i64>) -> PhysicalPlanNode {
    node(id, PhysicalKind::Scan { table_id: 1 }, schema, Vec::new())
}

#[test]
fn explain_reports_go_index_handle_offsets() {
    let lookup = PhysicalLocalIndexLookup {
        index_plan: scan(1, vec![10, -1]),
        table_plan: scan(2, vec![10, 20]),
        keep_order: true,
        schema: vec![10, 20],
        index_handle_offsets: vec![1],
    };

    assert_eq!(lookup.explain_info(), "index handle offsets:[1]");
}

#[test]
fn build_resets_only_the_cloned_table_side_like_go() {
    let lookup = PhysicalLocalIndexLookup {
        index_plan: scan(7, vec![10, -1]),
        table_plan: scan(8, vec![10, 20]),
        keep_order: false,
        schema: vec![10, 20],
        index_handle_offsets: vec![1],
    };
    let mut next_id = 100;

    let built = lookup
        .build_push_down_plan(Vec::new(), None, &mut next_id)
        .expect("lookup plan should build");

    assert_eq!(
        built.children[0].id, 7,
        "the original index plan is not cloned/reset in Go"
    );
    assert_eq!(
        built.children[1].id, 100,
        "the cloned table plan receives a fresh ID"
    );
}

#[test]
fn detach_requires_the_table_scan_to_be_a_leaf() {
    let mut invalid = node(
        1,
        PhysicalKind::Scan { table_id: 1 },
        vec![1],
        vec![scan(2, vec![1])],
    );

    assert!(detach_root_table_scan(&mut invalid).is_err());
}

#[test]
fn handle_offset_matches_go_for_partition_and_common_handles() {
    assert_eq!(
        PhysicalLocalIndexLookup::index_handle_offsets_for_schema(&[11, -1, -2], false)
            .expect("integer handle should be found"),
        vec![1]
    );
    assert_eq!(
        PhysicalLocalIndexLookup::index_handle_offsets_for_schema(&[-3, -2], true)
            .expect("common handles are read from the index value"),
        Vec::<u32>::new()
    );
    assert!(PhysicalLocalIndexLookup::index_handle_offsets_for_schema(&[-3, -2], false).is_err());
}

#[test]
fn protobuf_matches_go_and_rejects_non_tikv_stores() {
    let lookup = PhysicalLocalIndexLookup {
        index_plan: scan(1, vec![10, -1]),
        table_plan: scan(2, vec![10]),
        keep_order: false,
        schema: vec![10],
        index_handle_offsets: vec![1],
    };

    let protobuf = lookup
        .to_pb(kv::StoreType::TiKV)
        .expect("TiKV is supported");
    assert_eq!(protobuf.get_tp(), tipb::ExecType::TypeIndexLookUp);
    assert_eq!(protobuf.get_index_lookup().get_index_handle_offsets(), &[1]);
    assert!(lookup.to_pb(kv::StoreType::TiFlash).is_err());
}
