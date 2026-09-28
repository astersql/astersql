// Copyright 2026 AsterSQL.

use crate::region::{RegionInfo, beforeEnd};
use crate::stubs::metapb;

#[test]
fn contains_interior_matches_go_region_getter_semantics() {
    let bounded = RegionInfo {
        Region: Some(metapb::Region {
            StartKey: b"a".to_vec(),
            EndKey: b"z".to_vec(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!bounded.ContainsInterior(b"a"));
    assert!(bounded.ContainsInterior(b"m"));
    assert!(!bounded.ContainsInterior(b"z"));

    // Go's generated protobuf getters return empty keys for a nil *Region.
    // Consequently a non-empty key is interior to the unbounded empty range.
    let missing = RegionInfo::default();
    assert!(!missing.ContainsInterior(b""));
    assert!(missing.ContainsInterior(b"a"));
}

#[test]
fn before_end_matches_go_empty_end_sentinel() {
    assert!(beforeEnd(b"a", b"b"));
    assert!(!beforeEnd(b"b", b"b"));
    assert!(beforeEnd(b"z", b""));
}

#[test]
fn to_zap_fields_is_nil_safe() {
    assert_eq!(RegionInfo::ToZapFields(None), "");
    assert_eq!(RegionInfo::ToZapFields(Some(&RegionInfo::default())), "");
    let present = RegionInfo {
        Region: Some(metapb::Region::default()),
        ..Default::default()
    };
    assert_eq!(RegionInfo::ToZapFields(Some(&present)), "region");
}
