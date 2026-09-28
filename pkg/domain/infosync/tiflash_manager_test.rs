// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

use super::*;

#[test]
fn new_rule_uses_go_table_record_and_table_end_prefixes() {
    let rule = MakeNewRule(7, 2, vec![]);

    let mut expected_start = b"t".to_vec();
    expected_start.extend_from_slice(&(7_u64 | (1_u64 << 63)).to_be_bytes());
    expected_start.extend_from_slice(b"_r");
    let mut expected_end = b"t".to_vec();
    expected_end.extend_from_slice(&(8_u64 | (1_u64 << 63)).to_be_bytes());

    assert_eq!(rule.StartKey, expected_start);
    assert_eq!(rule.EndKey, expected_end);
}

#[test]
fn reset_sync_status_preserves_existing_acceleration_state() {
    let tiflash = NewMockTiFlash();
    tiflash.HandlePostAccelerateSchedule(42);

    tiflash.ResetSyncStatus(42, true);

    assert_eq!(
        tiflash.GetTableSyncStatus(42),
        Some(mockTiFlashTableInfo {
            Regions: vec![1],
            Accel: true,
        })
    );
}

#[test]
fn mock_group_rule_lookup_matches_go_unfiltered_contract() {
    let tiflash = NewMockTiFlash();
    let mut rule = MakeNewRule(7, 1, vec![]);
    rule.GroupID = "another-group".into();
    tiflash.HandleSetPlacementRule(&rule).unwrap();

    assert_eq!(tiflash.HandleGetGroupRules("tiflash"), vec![rule]);
}

#[test]
fn placement_rule_check_uses_go_semantic_fields_instead_of_rule_id() {
    let tiflash = NewMockTiFlash();
    let stored = MakeNewRule(7, 2, vec!["zone".into()]);
    tiflash.HandleSetPlacementRule(&stored).unwrap();

    let mut expected = stored;
    expected.ID = "a-different-id".into();
    expected.GroupID = "a-different-group".into();
    expected.Index += 1;

    assert!(tiflash.CheckPlacementRule(&expected));
}
