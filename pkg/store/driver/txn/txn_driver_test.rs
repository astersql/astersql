// Copyright 2026 AsterSQL.

use crate::*;

/// Go assigns the nil value into idxNameCache, so replacing a cached entry with
/// nil must make subsequent lookups behave as a cache miss.
#[test]
fn cache_table_info_none_clears_stale_entry() {
    let mut txn = tikvTxn::from_entries([], 1, false);
    let table = TableInfo {
        ID: 42,
        ..TableInfo::default()
    };

    txn.CacheTableInfo(42, Some(table));
    assert!(txn.GetTableInfo(42).is_some());

    txn.CacheTableInfo(42, None);
    assert!(txn.GetTableInfo(42).is_none());
}
