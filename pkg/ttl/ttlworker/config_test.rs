// Copyright 2026 AsterSQL.

use std::time::Duration;

use crate::config::{
    CHECK_TRIGGERED_JOB_INTERVAL, IntervalOverrides, JOB_MANAGER_LOOP_TICKER_INTERVAL,
    JOB_MANAGER_SYNC_TIMER_INTERVAL, RESIZE_WORKERS_INTERVAL, SPLIT_SCAN_COUNT,
    TASK_MANAGER_CHECK_TASK_INTERVAL, TASK_MANAGER_LOOP_TICKER_INTERVAL, TTL_GC_INTERVAL,
    TTL_TASK_HEARTBEAT_TICKER_INTERVAL, UPDATE_INFO_SCHEMA_CACHE_INTERVAL,
    UPDATE_TTL_TABLE_STATUS_CACHE_INTERVAL, scan_split_count,
};

#[test]
fn interval_defaults_and_overrides_match_go_getters() {
    let defaults = IntervalOverrides::default();
    assert_eq!(defaults.check_job(), JOB_MANAGER_LOOP_TICKER_INTERVAL);
    assert_eq!(defaults.heartbeat(), JOB_MANAGER_LOOP_TICKER_INTERVAL);
    assert_eq!(defaults.sync_timer(), JOB_MANAGER_SYNC_TIMER_INTERVAL);
    assert_eq!(
        defaults.update_info_schema(),
        UPDATE_INFO_SCHEMA_CACHE_INTERVAL
    );
    assert_eq!(
        defaults.update_table_status(),
        UPDATE_TTL_TABLE_STATUS_CACHE_INTERVAL
    );
    assert_eq!(defaults.resize_workers(), RESIZE_WORKERS_INTERVAL);
    assert_eq!(defaults.check_task(), TASK_MANAGER_CHECK_TASK_INTERVAL);
    assert_eq!(defaults.task_loop(), TASK_MANAGER_LOOP_TICKER_INTERVAL);
    assert_eq!(
        defaults.task_heartbeat(),
        TTL_TASK_HEARTBEAT_TICKER_INTERVAL
    );
    assert_eq!(defaults.check_triggered_job(), CHECK_TRIGGERED_JOB_INTERVAL);
    assert_eq!(defaults.gc(), TTL_GC_INTERVAL);

    let override_value = Duration::from_nanos(7);
    let overrides = IntervalOverrides {
        check_job: Some(override_value),
        heartbeat: Some(override_value),
        sync_timer: Some(override_value),
        update_info_schema: Some(override_value),
        update_table_status: Some(override_value),
        resize_workers: Some(override_value),
        check_task: Some(override_value),
        task_loop: Some(override_value),
        task_heartbeat: Some(override_value),
        check_triggered_job: Some(override_value),
        gc: Some(override_value),
    };
    assert_eq!(overrides.check_job(), override_value);
    assert_eq!(overrides.heartbeat(), override_value);
    assert_eq!(overrides.sync_timer(), override_value);
    assert_eq!(overrides.update_info_schema(), override_value);
    assert_eq!(overrides.update_table_status(), override_value);
    assert_eq!(overrides.resize_workers(), override_value);
    assert_eq!(overrides.check_task(), override_value);
    assert_eq!(overrides.task_loop(), override_value);
    assert_eq!(overrides.task_heartbeat(), override_value);
    assert_eq!(overrides.check_triggered_job(), override_value);
    assert_eq!(overrides.gc(), override_value);
}

#[test]
fn scan_split_count_matches_go_store_count_boundaries() {
    assert_eq!(scan_split_count(false, 0), SPLIT_SCAN_COUNT);
    assert_eq!(scan_split_count(false, 128), SPLIT_SCAN_COUNT);
    assert_eq!(scan_split_count(true, 0), SPLIT_SCAN_COUNT);
    assert_eq!(scan_split_count(true, SPLIT_SCAN_COUNT), SPLIT_SCAN_COUNT);
    assert_eq!(
        scan_split_count(true, SPLIT_SCAN_COUNT + 1),
        SPLIT_SCAN_COUNT + 1
    );
    assert_eq!(scan_split_count(true, 128), 128);
}
