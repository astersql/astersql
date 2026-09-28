// Copyright 2026 AsterSQL.

use super::foreign_key::{CascadeType, FkCascade, FkCheck, ReferentialAction};
use super::physical_common_plans::{Delete, Insert, Update};
use super::plan_clone_generated::CloneForPlanCache;

#[test]
fn dml_with_foreign_key_metadata_is_not_cacheable_like_go() {
    let check = FkCheck::default();

    let insert = Insert {
        fk_checks: vec![check.clone()],
        ..Insert::default()
    };
    assert!(insert.clone_for_plan_cache().is_none());

    let update = Update {
        fk_checks: vec![check.clone()],
        ..Update::default()
    };
    assert!(update.clone_for_plan_cache().is_none());

    let delete = Delete {
        fk_checks: vec![check],
        ..Delete::default()
    };
    assert!(delete.clone_for_plan_cache().is_none());
}

#[test]
fn dml_with_foreign_key_cascade_is_not_cacheable_like_go() {
    let cascade = FkCascade {
        name: "fk".to_owned(),
        child_table_id: 1,
        child_columns: vec!["child".to_owned()],
        parent_columns: vec!["parent".to_owned()],
        cascade_type: CascadeType::OnDelete,
        action: ReferentialAction::Cascade,
    };

    assert!(
        Insert {
            fk_cascades: vec![cascade.clone()],
            ..Insert::default()
        }
        .clone_for_plan_cache()
        .is_none()
    );
    assert!(
        Update {
            fk_cascades: vec![cascade.clone()],
            ..Update::default()
        }
        .clone_for_plan_cache()
        .is_none()
    );
    assert!(
        Delete {
            fk_cascades: vec![cascade],
            ..Delete::default()
        }
        .clone_for_plan_cache()
        .is_none()
    );
}
