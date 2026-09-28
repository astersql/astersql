// Copyright 2026 AsterSQL.

use crate::MaxStoreConcurrency;

#[test]
fn max_store_concurrency_is_usable_as_a_store_count() {
    fn accept_store_count(_: usize) {}

    accept_store_count(MaxStoreConcurrency);
    assert_eq!(MaxStoreConcurrency, 128usize);
}
