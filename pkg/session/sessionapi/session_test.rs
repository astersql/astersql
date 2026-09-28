// Copyright 2026 AsterSQL.

use crate::{Session, extension};
use std::sync::Arc;

#[test]
fn set_extensions_preserves_nullable_go_pointer_contract() {
    fn assert_signature<S: Session>() {
        let _: fn(&mut S, Option<Arc<extension::SessionExtensions>>) = S::SetExtensions;
    }
}
