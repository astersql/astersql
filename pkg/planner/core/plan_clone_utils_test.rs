// Copyright 2026 AsterSQL.

use crate::{AccessPath, PlanKind, PlanNode, cloneLogicalSubtree, freshAccessPath};

#[test]
fn fresh_access_path_preserves_identity_and_clears_analysis_state() {
    let source = AccessPath {
        index_name: "idx_customer".to_owned(),
        ranges: vec!["[42,42]".to_owned()],
        count_after_access: 1,
        count_after_index: 1,
        partial_alternative_index_paths: vec![AccessPath {
            index_name: "idx_alternative".to_owned(),
            ..AccessPath::default()
        }],
    };

    let fresh = freshAccessPath(&source);

    assert_eq!(fresh.index_name, source.index_name);
    assert!(fresh.ranges.is_empty());
    assert_eq!(fresh.count_after_access, 0);
    assert_eq!(fresh.count_after_index, 0);
    assert!(fresh.partial_alternative_index_paths.is_empty());
}

#[test]
fn logical_subtree_clone_assigns_fresh_unique_ids() {
    let source = PlanNode::New(
        7,
        PlanKind::Projection,
        vec![PlanNode::New(
            11,
            PlanKind::Selection {
                conditions: vec!["a = 1".to_owned()],
            },
            vec![PlanNode::New(
                19,
                PlanKind::DataSource {
                    table: "t".to_owned(),
                    alias: None,
                    partition_id: None,
                },
                Vec::new(),
            )],
        )],
    );

    let (cloned, ok) = cloneLogicalSubtree(&source);
    let cloned = cloned.expect("supported subtree must clone");

    assert!(ok);
    let original_ids = [
        source.id,
        source.children[0].id,
        source.children[0].children[0].id,
    ];
    let cloned_ids = [
        cloned.id,
        cloned.children[0].id,
        cloned.children[0].children[0].id,
    ];
    assert!(cloned_ids.iter().all(|id| !original_ids.contains(id)));
    assert_ne!(cloned_ids[0], cloned_ids[1]);
    assert_ne!(cloned_ids[0], cloned_ids[2]);
    assert_ne!(cloned_ids[1], cloned_ids[2]);
}

#[test]
fn unsupported_child_aborts_the_entire_clone() {
    let source = PlanNode::New(
        1,
        PlanKind::Projection,
        vec![PlanNode::New(2, PlanKind::Apply, Vec::new())],
    );

    assert_eq!(cloneLogicalSubtree(&source), (None, false));
}
