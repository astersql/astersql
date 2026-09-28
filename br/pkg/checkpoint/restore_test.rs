// Copyright 2026 AsterSQL.

use serde_json::json;
use uuid::Uuid;

use crate::restore::CheckpointMetadataForSnapshotRestore;

#[test]
fn snapshot_restore_uuid_uses_go_text_json_encoding() {
    let restore_uuid = Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8").unwrap();
    let metadata = CheckpointMetadataForSnapshotRestore {
        RestoreUUID: restore_uuid,
        ..Default::default()
    };

    let encoded = serde_json::to_value(&metadata).unwrap();
    assert_eq!(
        encoded["restore-uuid"],
        json!("67e55044-10b1-426f-9247-bb680e5fe0c8")
    );

    let decoded: CheckpointMetadataForSnapshotRestore = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.RestoreUUID, restore_uuid);
}
