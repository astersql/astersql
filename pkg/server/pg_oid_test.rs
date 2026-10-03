// Copyright 2026 AsterSQL.
use crate::pg_oid::*;
#[test]
fn pg_introspection_oid_ranges_and_local_indexes() {
    assert_ne!(namespace_oid(42).unwrap(), table_oid(42).unwrap());
    assert_ne!(table_oid(42).unwrap(), index_oid(42, 1).unwrap());
    assert_ne!(index_oid(42, 1).unwrap(), index_oid(43, 1).unwrap());
    for id in [0, i64::MAX, i64::MIN] {
        assert!(namespace_oid(id).is_err());
        assert!(table_oid(id).is_err());
        assert!(index_oid(id, 1).is_err());
    }
    assert!(index_oid(1, i64::MAX).is_err());
    assert_ne!(index_oid(42, -1).unwrap(), index_oid(42, 1).unwrap());
    assert_ne!(index_oid(42, -1).unwrap(), index_oid(42, -2).unwrap());
    let mut seen = std::collections::HashSet::new();
    for (_, oid) in SYSTEM_RELATIONS {
        assert!(seen.insert(*oid));
    }
    for id in -50..=50 {
        if id == 0 {
            continue;
        }
        assert!(seen.insert(namespace_oid(id).unwrap()));
        assert!(seen.insert(table_oid(id).unwrap()));
        for index in 1..=10 {
            assert!(seen.insert(index_oid(id, index).unwrap()));
        }
    }
}
