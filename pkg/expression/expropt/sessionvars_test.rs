// Copyright 2026 AsterSQL.

use std::sync::Arc;

use crate::*;

#[test]
fn location_assertion_uses_go_compatible_exact_names() {
    let vars = Arc::new(variable::SessionVars::new());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::sessionvars::assert_session_vars_location_matches("UTC", vars.as_ref());
    }));

    assert!(
        result.is_err(),
        "Go treats UTC and +00:00 as distinct location names"
    );
}
