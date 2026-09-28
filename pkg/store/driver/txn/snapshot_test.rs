// Copyright 2026 AsterSQL.

use crate::{Priority, getTiKVPriority};

/// Go's kv.Priority constants are iota-based: Normal=0, Low=1, High=2.
#[test]
fn snapshot_priority_mapping_matches_go_constants() {
    assert_eq!(getTiKVPriority(0), Priority::Normal);
    assert_eq!(getTiKVPriority(1), Priority::Low);
    assert_eq!(getTiKVPriority(2), Priority::High);
    assert_eq!(getTiKVPriority(-1), Priority::Normal);
    assert_eq!(getTiKVPriority(99), Priority::Normal);
}
