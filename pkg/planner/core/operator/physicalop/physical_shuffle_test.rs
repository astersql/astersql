// Copyright 2026 AsterSQL.

use crate::physical_common_plans::{
    PhysicalExpr, PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats,
};
use crate::physical_shuffle::{
    PartitionSplitterType, PhysicalShuffle, PhysicalShuffleReceiverStub,
};

fn plan(id: i64, schema: Vec<i64>) -> PhysicalPlanNode {
    PhysicalPlanNode {
        id,
        kind: PhysicalKind::Other(format!("source-{id}")),
        schema,
        children: Vec::new(),
        stats: Stats::default(),
        required_properties: Vec::new(),
    }
}

#[test]
fn explain_info_lists_data_source_explain_ids_like_go() {
    let shuffle = PhysicalShuffle {
        concurrency: 4,
        tails: Vec::new(),
        data_sources: vec![plan(7, vec![1]), plan(11, vec![2])],
        splitter_type: PartitionSplitterType::Hash,
        by_item_arrays: Vec::new(),
        schema: Vec::new(),
    };

    assert_eq!(
        shuffle.explain_info(),
        "execution info: concurrency:4, data sources:[7 11]"
    );
}

#[test]
fn resolve_indices_uses_each_corresponding_data_source_schema() {
    let mut shuffle = PhysicalShuffle {
        concurrency: 2,
        tails: Vec::new(),
        data_sources: vec![plan(1, vec![10]), plan(2, vec![20])],
        splitter_type: PartitionSplitterType::Hash,
        by_item_arrays: vec![
            vec![PhysicalExpr::Column(10)],
            vec![PhysicalExpr::Column(20)],
        ],
        schema: vec![10, 20],
    };

    assert_eq!(shuffle.resolve_indices(), Ok(()));
    shuffle.by_item_arrays[1][0] = PhysicalExpr::Column(10);
    assert_eq!(
        shuffle.resolve_indices(),
        Err("column 10 is absent from child schema".to_owned())
    );
}

#[test]
fn into_plan_preserves_every_worker_tail_after_data_sources() {
    let shuffle = PhysicalShuffle {
        concurrency: 2,
        tails: vec![plan(5, vec![10]), plan(6, vec![20])],
        data_sources: vec![plan(1, vec![10]), plan(2, vec![20])],
        splitter_type: PartitionSplitterType::Range,
        by_item_arrays: Vec::new(),
        schema: vec![10, 20],
    };

    let node = shuffle
        .into_plan(Stats::default(), PhysicalProperty::default())
        .unwrap();
    assert_eq!(
        node.children
            .iter()
            .map(|child| child.id)
            .collect::<Vec<_>>(),
        vec![1, 2, 5, 6]
    );
}

#[test]
fn receiver_memory_includes_its_optional_data_source_like_go() {
    let source = plan(9, Vec::with_capacity(4));
    let source_memory = source.memory_usage();
    let receiver = PhysicalShuffleReceiverStub {
        receiver_index: 0,
        schema: Vec::new(),
        data_source: Some(Box::new(source)),
    };

    assert_eq!(
        receiver.memory_usage(),
        std::mem::size_of::<PhysicalShuffleReceiverStub>() as i64 + source_memory
    );
}
