// Copyright 2026 AsterSQL.

use crate::{IsTiFlashContained, PlanKind, PlanNode, StoreType};

fn node(kind: PlanKind, store_type: StoreType, children: Vec<PlanNode>) -> PlanNode {
    let mut plan = PlanNode::New(1, kind, children);
    plan.store_type = store_type;
    plan
}

#[test]
fn only_tiflash_table_readers_are_reported() {
    let scan = node(
        PlanKind::TableScan { table: "t".into() },
        StoreType::TiFlash,
        vec![],
    );

    assert_eq!(IsTiFlashContained(&scan), (false, false));
}

#[test]
fn exchange_flag_comes_from_tiflash_table_plan_root() {
    let exchange = node(
        PlanKind::ExchangeSender { task_ids: vec![] },
        StoreType::Root,
        vec![],
    );
    let reader = node(PlanKind::TableReader, StoreType::TiFlash, vec![exchange]);

    assert_eq!(IsTiFlashContained(&reader), (true, true));
}

#[test]
fn nested_exchange_is_not_a_table_plan_root() {
    let exchange = node(
        PlanKind::ExchangeSender { task_ids: vec![] },
        StoreType::TiFlash,
        vec![],
    );
    let projection = node(PlanKind::Projection, StoreType::TiFlash, vec![exchange]);
    let reader = node(PlanKind::TableReader, StoreType::TiFlash, vec![projection]);

    assert_eq!(IsTiFlashContained(&reader), (true, false));
}

#[test]
fn logical_nodes_do_not_expose_physical_descendants() {
    let reader = node(PlanKind::TableReader, StoreType::TiFlash, vec![]);
    let logical = node(
        PlanKind::DataSource {
            table: "t".into(),
            alias: None,
            partition_id: None,
        },
        StoreType::Root,
        vec![reader],
    );

    assert_eq!(IsTiFlashContained(&logical), (false, false));
}
