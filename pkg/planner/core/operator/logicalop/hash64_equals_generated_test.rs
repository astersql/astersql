// Copyright 2026 AsterSQL.

use crate::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        id + 100,
        0,
    )
}

fn hash(value: &impl Fn(&mut dyn Hasher)) -> u64 {
    let mut hasher = DefaultHasher::new();
    value(&mut hasher);
    hasher.finish()
}

#[test]
fn aggregation_possible_properties_participate_in_hash_and_equals() {
    let left = LogicalAggregation::default();
    let mut right = LogicalAggregation::default();
    right.PossibleProperties = vec![vec![column(1)]];

    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&|hasher| left.Hash64(hasher)),
        hash(&|hasher| right.Hash64(hasher))
    );
}

#[test]
fn projection_flags_participate_in_hash_and_equals() {
    let left = LogicalProjection::default();
    for right in [
        LogicalProjection {
            CalculateNoDelay: true,
            ..Default::default()
        },
        LogicalProjection {
            Proj4Expand: true,
            ..Default::default()
        },
    ] {
        assert!(!left.Equals(&right));
        assert_ne!(
            hash(&|hasher| left.Hash64(hasher)),
            hash(&|hasher| right.Hash64(hasher))
        );
    }
}

#[test]
fn top_n_prefer_limit_to_cop_participates_in_hash_and_equals() {
    let left = LogicalTopN::default();
    let right = LogicalTopN {
        PreferLimitToCop: true,
        ..Default::default()
    };

    assert!(!left.Equals(&right));
    assert_ne!(
        hash(&|hasher| left.Hash64(hasher)),
        hash(&|hasher| right.Hash64(hasher))
    );
}

#[test]
fn show_payload_is_excluded_like_go_generated_contract() {
    let left = LogicalShow::default();
    let mut right = LogicalShow::default();
    right.ShowContents.DBName = "test".into();

    assert!(left.Equals(&right));
    assert_eq!(
        hash(&|hasher| left.Hash64(hasher)),
        hash(&|hasher| right.Hash64(hasher))
    );
}

#[test]
fn show_ddl_job_number_is_excluded_like_go_generated_contract() {
    let left = LogicalShowDDLJobs::default();
    let right = LogicalShowDDLJobs {
        JobNumber: 1,
        ..Default::default()
    };

    assert!(left.Equals(&right));
    assert_eq!(
        hash(&|hasher| left.Hash64(hasher)),
        hash(&|hasher| right.Hash64(hasher))
    );
}
