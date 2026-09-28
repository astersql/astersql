// Copyright 2026 AsterSQL.

use std::collections::HashSet;

use expression::{Column, ExprBox};
use property::SortItem;

use crate::{
    find_max_prefix_len, is_sort_prop_compatible_with_join_keys, move_equal_to_other_conditions,
    reorder_by_offsets,
};

fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        id,
        id,
        0,
    )
}

#[test]
fn max_prefix_uses_the_longest_candidate_like_go() {
    let keys = vec![column(1), column(2), column(3)];
    let candidates = vec![vec![column(1)], vec![column(1), column(2)], vec![column(2)]];
    assert_eq!(find_max_prefix_len(&candidates, &keys), 2);
}

#[test]
fn offset_reordering_keeps_unselected_values_in_original_order() {
    assert_eq!(
        reorder_by_offsets(&[10, 20, 30, 40], &[2, 0]),
        [30, 10, 20, 40]
    );
    assert_eq!(
        reorder_by_offsets(&[true, false, false], &[1]),
        [false, true, false]
    );
}

#[test]
fn unused_equal_conditions_are_appended_after_existing_other_conditions() {
    let other: Vec<ExprBox> = vec![Box::new(column(9))];
    let equal: Vec<ExprBox> = vec![
        Box::new(column(1)),
        Box::new(column(2)),
        Box::new(column(3)),
    ];
    let moved = move_equal_to_other_conditions(&other, &equal, &[1]);
    let ids = moved
        .iter()
        .map(|expression| expression.as_column().unwrap().UniqueID)
        .collect::<Vec<_>>();
    assert_eq!(ids, [9, 1, 3]);
}

#[test]
fn sort_property_may_skip_only_leading_constant_join_keys() {
    let keys = vec![column(1), column(2), column(3)];
    let sort_on_two = vec![SortItem {
        Col: column(2),
        Desc: false,
    }];
    assert!(is_sort_prop_compatible_with_join_keys(
        &sort_on_two,
        &keys,
        &HashSet::from([1]),
    ));
    let sort_on_three = vec![SortItem {
        Col: column(3),
        Desc: false,
    }];
    assert!(!is_sort_prop_compatible_with_join_keys(
        &sort_on_three,
        &keys,
        &HashSet::from([1]),
    ));
    assert!(is_sort_prop_compatible_with_join_keys(
        &sort_on_three,
        &keys,
        &HashSet::from([1, 2]),
    ));
}
