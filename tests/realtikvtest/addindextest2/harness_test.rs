// Copyright 2026 AsterSQL.

use crate::harness::{TestCtx, require};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn metering_regex_rejects_malformed_numeric_fields() {
    let t = TestCtx::new();
    require::Regexp(
        &t,
        r"cluster\{r: 1\d\dB, w: (\d{3}|.*Ki)B\}",
        "cluster{r: 153B, w: 256B}",
    );
    require::Regexp(
        &t,
        r"obj_store\{r: 1.\d+KiB, w: \d.\d+KiB\}",
        "obj_store{r: 1.2KiB, w: 3.4KiB}",
    );

    let cluster = catch_unwind(AssertUnwindSafe(|| {
        require::Regexp(
            &t,
            r"cluster\{r: 1\d\dB, w: (\d{3}|.*Ki)B\}",
            "cluster{r: 1oopsB, w: nopeB}",
        );
    }));
    assert!(
        cluster.is_err(),
        "Go regexp must reject malformed cluster metrics"
    );

    let object_store = catch_unwind(AssertUnwindSafe(|| {
        require::Regexp(
            &t,
            r"obj_store\{r: 1.\d+KiB, w: \d.\d+KiB\}",
            "obj_store{r: 1oopsKiB, w: nopeKiB}",
        );
    }));
    assert!(
        object_store.is_err(),
        "Go regexp must reject malformed object-store metrics"
    );
}
