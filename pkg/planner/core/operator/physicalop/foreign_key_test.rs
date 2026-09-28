// Copyright 2026 AsterSQL.

use std::collections::BTreeSet;

use crate::foreign_key::{
    CascadeType, ForeignKeyInfo, ReferentialAction, TableInfo, build_on_insert_fk_triggers,
    build_on_update_fk_triggers,
};

fn foreign_key() -> ForeignKeyInfo {
    ForeignKeyInfo {
        name: "fk_child_parent".to_owned(),
        child_table_id: 11,
        parent_table_id: 22,
        child_columns: vec!["parent_id".to_owned()],
        parent_columns: vec!["id".to_owned()],
        on_delete: ReferentialAction::SetDefault,
        on_update: ReferentialAction::Cascade,
        public: true,
        enabled: true,
    }
}

#[test]
fn child_modification_checks_referenced_columns_exist() {
    let table = TableInfo {
        foreign_keys: vec![foreign_key()],
        ..TableInfo::default()
    };

    let (checks, cascades) = build_on_insert_fk_triggers(&table, false).unwrap();

    assert!(cascades.is_empty());
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].table_id, 22);
    assert_eq!(checks[0].columns, ["id"]);
    assert!(checks[0].check_exist);
    assert_eq!(
        checks[0].operator_info(),
        "foreign_key:fk_child_parent, check_exist"
    );
}

#[test]
fn parent_modification_checks_child_columns_do_not_exist() {
    let table = TableInfo {
        referred_foreign_keys: vec![foreign_key()],
        ..TableInfo::default()
    };
    let updated = BTreeSet::from(["ID".to_owned()]);

    let (checks, cascades) = build_on_update_fk_triggers(&table, &updated).unwrap();

    assert!(checks.is_empty());
    assert_eq!(cascades.len(), 1);
    assert_eq!(cascades[0].cascade_type, CascadeType::OnUpdate);
    assert_eq!(
        cascades[0].operator_info(),
        "foreign_key:fk_child_parent, on_update:CASCADE"
    );
}

#[test]
fn set_default_follows_go_default_restrict_path() {
    let table = TableInfo {
        referred_foreign_keys: vec![foreign_key()],
        ..TableInfo::default()
    };

    let (checks, cascades) = build_on_insert_fk_triggers(&table, true).unwrap();

    assert!(cascades.is_empty());
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].table_id, 11);
    assert_eq!(checks[0].columns, ["parent_id"]);
    assert!(!checks[0].check_exist);
    assert_eq!(
        checks[0].operator_info(),
        "foreign_key:fk_child_parent, check_not_exist"
    );
}

#[test]
fn non_public_cascade_is_forced_to_restrict() {
    let mut fk = foreign_key();
    fk.public = false;
    fk.on_delete = ReferentialAction::Cascade;
    let table = TableInfo {
        referred_foreign_keys: vec![fk],
        ..TableInfo::default()
    };

    let (checks, cascades) = build_on_insert_fk_triggers(&table, true).unwrap();

    assert!(cascades.is_empty());
    assert_eq!(checks.len(), 1);
    assert!(!checks[0].check_exist);
}
