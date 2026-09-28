// Copyright 2026 AsterSQL.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::New;

#[test]
fn missing_store_and_region_fail_loudly_like_go() {
    let cluster = New();
    assert!(
        catch_unwind(AssertUnwindSafe(|| cluster.RemoveStore(42))).is_err(),
        "Go RemoveStore dereferences the missing store and panics"
    );

    let cluster = New();
    assert!(
        catch_unwind(AssertUnwindSafe(|| cluster.UpdateRegion(42, |_| {}))).is_err(),
        "Go UpdateRegion passes nil to its mutator; Rust must not silently skip it"
    );
}
