// Copyright 2026 AsterSQL.

use crate::{hash_columns_equivalent_on_inner_join, task_hash_cols_satisfy_output};

#[test]
fn outer_task_hash_keys_survive_an_unpartitioned_parent_request() {
    let task_hash_cols = vec![property::MPPPartitionColumn {
        Col: expression::Column::default(),
        CollateID: 0,
    }];

    assert!(task_hash_cols_satisfy_output(&task_hash_cols, &[]));
    assert!(!task_hash_cols_satisfy_output(&[], &[]));
}

#[test]
fn inner_join_equal_keys_satisfy_mpp_partition_without_another_exchange() {
    let left = expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        1,
        11,
        0,
    );
    let right = expression::Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        2,
        22,
        0,
    );
    let supplied = [property::MPPPartitionColumn {
        Col: right.clone(),
        CollateID: 0,
    }];
    let required = [property::MPPPartitionColumn {
        Col: left.clone(),
        CollateID: 0,
    }];

    assert!(hash_columns_equivalent_on_inner_join(
        base::JoinType::InnerJoin,
        &[left.clone()],
        &[right.clone()],
        &supplied,
        &required,
    ));
    assert!(!hash_columns_equivalent_on_inner_join(
        base::JoinType::LeftOuterJoin,
        &[left.clone()],
        &[right.clone()],
        &supplied,
        &required,
    ));
    let wrong_collation = [property::MPPPartitionColumn {
        Col: left.clone(),
        CollateID: 1,
    }];
    assert!(!hash_columns_equivalent_on_inner_join(
        base::JoinType::InnerJoin,
        &[left],
        &[right],
        &supplied,
        &wrong_collation,
    ));
}
