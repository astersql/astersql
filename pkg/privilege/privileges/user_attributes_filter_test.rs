// Copyright 2026 AsterSQL.

use crate::cache_test::{
    RowDataSource, dynamic_priv_row, role_edge_row, tables_priv_row, user_row,
};
use crate::*;

fn manager(source: RowDataSource) -> UserPrivileges {
    let handle = Handle::New();
    handle.UpdateAll(&source).unwrap();
    NewUserPrivileges(handle)
}

#[test]
#[serial_test::serial]
fn user_attributes_update_roles_and_system_user_rules() {
    let mut source = RowDataSource::default();
    source.user = vec![
        user_row("%", "viewer", &[]),
        user_row("%", "updater", &[]),
        user_row("%", "creator", &["create_user"]),
        user_row("%", "system", &[]),
        user_row("%", "ordinary", &[]),
    ];
    source.tables_priv = vec![tables_priv_row(
        "%", "mysql", "updater", "user", "Update", "",
    )];
    source.dynamic_priv = vec![dynamic_priv_row("%", "system", "SYSTEM_USER", false)];
    source.role_edges = vec![
        role_edge_row("%", "updater", "%", "viewer"),
        role_edge_row("%", "system", "%", "creator"),
        role_edge_row("%", "system", "%", "ordinary"),
    ];
    let pm = manager(source.clone());
    assert!(!NewUserAttrFilter(&[], "viewer", "localhost", Some(&pm)).Visible("ordinary", "%"));
    let roles = [RoleIdentity::new("updater", "%")];
    assert!(NewUserAttrFilter(&roles, "viewer", "localhost", Some(&pm)).Visible("system", "%"));
    assert!(NewUserAttrFilter(&[], "updater", "localhost", Some(&pm)).Visible("system", "%"));
    let creator = NewUserAttrFilter(&[], "creator", "localhost", Some(&pm));
    assert!(!creator.Visible("system", "%"));
    assert!(creator.Visible("ordinary", "%"));
    assert!(creator.Visible("creator", "%"));
    source
        .dynamic_priv
        .push(dynamic_priv_row("%", "creator", "SYSTEM_USER", false));
    let pm = manager(source);
    assert!(NewUserAttrFilter(&[], "creator", "localhost", Some(&pm)).Visible("system", "%"));
}

#[test]
#[serial_test::serial]
fn user_attributes_host_matching_and_fallbacks() {
    let source = RowDataSource {
        user: vec![
            user_row("%", "viewer", &[]),
            user_row("192.168.%", "viewer", &[]),
            user_row("%", "other", &[]),
        ],
        ..Default::default()
    };
    let pm = manager(source);
    let filter = NewUserAttrFilter(&[], "viewer", "localhost", Some(&pm));
    assert!(filter.Visible("viewer", "%"));
    assert!(!filter.Visible("viewer", "192.168.%"));
    assert!(!filter.Visible("missing", "%"));
    assert!(NewUserAttrFilter(&[], "viewer", "localhost", None).Visible("other", "%"));
    assert!(NewUserAttrFilter(&[], "", "", Some(&pm)).Visible("other", "%"));
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            set_skip_with_grant(self.0);
        }
    }
    let _reset = Reset(SkipWithGrant());
    set_skip_with_grant(true);
    assert!(NewUserAttrFilter(&[], "viewer", "localhost", Some(&pm)).Visible("other", "%"));
}
