// Copyright 2026 AsterSQL.

use crate::session::PhysicalTable;
use crate::timer_sync::{TtlTimersSyncer, timer_key};

fn table(ttl_enabled: bool) -> PhysicalTable {
    PhysicalTable {
        partition_name: None,
        table_id: 1,
        physical_id: 1,
        schema: "test".to_owned(),
        table: "t".to_owned(),
        key_columns: Vec::new(),
        ttl_column: "ts".to_owned(),
        ttl_enabled,
        definition_version: 1,
        expire_after_seconds: 3_600,
    }
}

#[test]
fn old_timer_is_deleted_immediately_after_its_table_disappears() {
    let mut syncer = TtlTimersSyncer::new();
    syncer.set_delay_delete_interval(100);
    syncer.sync_timers(&[table(true)], 1, 10, |_| Some("1h".to_owned()));

    syncer.sync_timers(&[], 2, 111, |_| Some("1h".to_owned()));

    assert!(syncer.cached_timer(&timer_key(1, 1)).is_none());
}

#[test]
fn reappearing_disabled_table_is_not_later_deleted_as_stale() {
    let mut syncer = TtlTimersSyncer::new();
    syncer.set_delay_delete_interval(100);
    let disabled = table(false);
    syncer.sync_timers(&[disabled.clone()], 1, 10, |_| Some("1h".to_owned()));
    syncer.sync_timers(&[], 2, 20, |_| Some("1h".to_owned()));
    assert_eq!(
        syncer.cached_timer(&timer_key(1, 1)).unwrap().deleted_at,
        Some(20)
    );

    syncer.sync_timers(&[disabled], 3, 30, |_| Some("1h".to_owned()));
    assert_eq!(
        syncer.cached_timer(&timer_key(1, 1)).unwrap().deleted_at,
        None
    );
    syncer.sync_timers(&[table(false)], 4, 121, |_| Some("1h".to_owned()));
    assert!(syncer.cached_timer(&timer_key(1, 1)).is_some());
}
