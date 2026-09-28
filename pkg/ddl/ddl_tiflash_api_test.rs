// Copyright 2026 AsterSQL.

use crate::ddl_tiflash_api::{PollTiFlashContext, TiFlashReplicaStatus, poll_replica_status};
use std::collections::BTreeMap;

#[test]
fn partition_backoff_is_keyed_by_physical_id_like_go() {
    let mut context = PollTiFlashContext::new(8, 1, 8, 2);
    let mut statuses = vec![
        TiFlashReplicaStatus {
            table_id: 10,
            physical_id: 11,
            replica_count: 1,
            available: false,
            progress: 0.0,
        },
        TiFlashReplicaStatus {
            table_id: 10,
            physical_id: 12,
            replica_count: 1,
            available: false,
            progress: 0.0,
        },
    ];

    let completed =
        poll_replica_status(&mut context, &mut statuses, &BTreeMap::from([(12, 1.0)])).unwrap();

    assert_eq!(completed, 1);
    assert!(!statuses[0].available);
    assert!(statuses[1].available);
    assert!(context.get(11).is_some());
    assert!(context.get(12).is_none());
}
