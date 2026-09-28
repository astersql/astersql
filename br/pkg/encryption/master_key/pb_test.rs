// Copyright 2026 AsterSQL.

use crate::pb::{MasterKey, MasterKeyBackend};

#[test]
fn master_key_default_represents_unset_go_oneof() {
    assert!(matches!(
        MasterKey::default().Backend,
        MasterKeyBackend::Unset
    ));
}
