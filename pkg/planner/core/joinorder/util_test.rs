// Copyright 2026 AsterSQL.

use super::util::{Expr, outer_join_side_filters_touch_multiple_leaves};

#[test]
fn outer_join_filters_across_distinct_leaves_disable_reordering() {
    let filters = [
        Expr::Column {
            unique_id: 1,
            leaf_id: 10,
        },
        Expr::Column {
            unique_id: 2,
            leaf_id: 20,
        },
    ];

    assert!(outer_join_side_filters_touch_multiple_leaves(&filters));
}

#[test]
fn outer_join_filters_on_one_leaf_allow_reordering() {
    let filters = [
        Expr::Column {
            unique_id: 1,
            leaf_id: 10,
        },
        Expr::Column {
            unique_id: 2,
            leaf_id: 10,
        },
    ];

    assert!(!outer_join_side_filters_touch_multiple_leaves(&filters));
}
