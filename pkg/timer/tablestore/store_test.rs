// Copyright 2026 AsterSQL.

use super::store::index_merge_is_disabled;

#[test]
fn index_merge_state_matches_tidb_boolean_values() {
    for disabled in ["OFF", "off", "0", "FALSE", "false"] {
        assert!(index_merge_is_disabled(disabled), "{disabled}");
    }
    for enabled in ["ON", "on", "1", "TRUE", "true"] {
        assert!(!index_merge_is_disabled(enabled), "{enabled}");
    }
}
