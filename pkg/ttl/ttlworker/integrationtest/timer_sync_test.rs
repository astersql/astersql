// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `TtlTimersSyncer` 定时器同步与手动触发的集成测试。
//
// 覆盖按启用表创建 timer、默认间隔回落、增量更新、truncate/drop 后的
// 延迟删除，以及 manual_trigger 在启用/缺失/禁用场景下的行为。

use astersql_ttl_ttlworker::session::PhysicalTable;
use astersql_ttl_ttlworker::timer_sync::{TimerRecord, TtlTimersSyncer, timer_key};

/// 构造带 schema/表名与 TTL 开关的物理表。
fn table(
    table_id: i64,
    physical_id: i64,
    schema: &str,
    name: &str,
    ttl_enabled: bool,
) -> PhysicalTable {
    PhysicalTable {
        partition_name: None,
        table_id,
        physical_id,
        schema: schema.to_owned(),
        table: name.to_owned(),
        key_columns: Vec::new(),
        ttl_column: "t".to_owned(),
        ttl_enabled,
        definition_version: 1,
        expire_after_seconds: 3600,
    }
}

/// 从同步器缓存取出指定 physical_id 的 timer，缺失则 panic。
fn expect_timer(sync: &TtlTimersSyncer, table_id: i64, physical_id: i64) -> &TimerRecord {
    sync.cached_timer(&timer_key(table_id, physical_id))
        .unwrap_or_else(|| panic!("expected a cached timer for physical_id {physical_id}"))
}

// 对应 Go TestTTLTimerSync 的首次同步：每个已定义 TTL 的表/分区各生成一个 timer，
// TTL_ENABLE=OFF 也保留 disabled timer，tags 与 schedule interval 与表信息一致。
#[test]
fn test_sync_timers_creates_one_timer_per_ttl_table() {
    let mut sync = TtlTimersSyncer::new();
    let (zero_time, zero_ver) = sync.last_sync_info();
    assert_eq!(zero_time, 0);
    assert_eq!(zero_ver, 0);

    // 无 TTL 定义的 t0 不会被 Go infoschema TTLAttribute 枚举，因此不传入。
    let t1 = table(2, 2, "test", "t1", true);
    let t2 = table(4, 4, "test", "t2", false);
    let tp1_p0 = table(3, 30, "test", "tp1", true);
    let tp1_p1 = table(3, 31, "test", "tp1", true);
    let tables = [t1.clone(), t2, tp1_p0.clone(), tp1_p1.clone()];

    sync.sync_timers(&tables, 7, 1_000, |t| {
        if t.physical_id == 2 {
            Some("1h".to_owned())
        } else {
            Some("3h".to_owned())
        }
    });

    let (last_time, last_ver) = sync.last_sync_info();
    assert_eq!(last_time, 1_000);
    assert_eq!(last_ver, 7);

    let disabled = expect_timer(&sync, 4, 4);
    assert!(!disabled.enabled);
    assert_eq!(disabled.key, "/tidb/ttl/physical_table/4/4");

    let timer1 = expect_timer(&sync, 2, 2);
    assert!(timer1.enabled);
    assert_eq!(timer1.schedule_interval, "1h");
    assert_eq!(timer1.data.table_id, 2);
    assert_eq!(timer1.data.physical_id, 2);
    assert_eq!(
        timer1.tags,
        vec!["db=test".to_owned(), "table=t1".to_owned()]
    );

    let timer_p0 = expect_timer(&sync, 3, 30);
    let timer_p1 = expect_timer(&sync, 3, 31);
    assert_eq!(timer_p0.schedule_interval, "3h");
    assert_eq!(timer_p1.schedule_interval, "3h");
    assert_ne!(timer_p0.id, timer_p1.id);
}

// 对应 Go 用例：没有显式 interval（回落到旧默认值）时应使用 OLD_DEFAULT_TTL_JOB_INTERVAL。
#[test]
fn test_sync_timers_falls_back_to_old_default_interval() {
    let mut sync = TtlTimersSyncer::new();
    let t1 = table(1, 1, "test", "t1", true);
    sync.sync_timers(&[t1], 1, 0, |_| None);
    let timer = expect_timer(&sync, 1, 1);
    assert_eq!(
        timer.schedule_interval,
        astersql_ttl_ttlworker::timer_sync::OLD_DEFAULT_TTL_JOB_INTERVAL
    );
}

// 对应 Go 中 "update table" 场景：schedule interval 变化会更新已缓存的 timer，但保留同一个 timer id
// （因为 physical id 未变），并且没有变化的 timer 保持完全不变（`should_sync_timer` 返回 false）。
#[test]
fn test_sync_timers_updates_changed_timer_but_keeps_unrelated_ones_untouched() {
    let mut sync = TtlTimersSyncer::new();
    let t1 = table(1, 1, "test", "t1", true);
    let t2 = table(2, 2, "test", "t2", true);
    sync.sync_timers(&[t1.clone(), t2.clone()], 1, 0, |_| Some("1h".to_owned()));
    let timer1_before = expect_timer(&sync, 1, 1).clone();
    let timer2_before = expect_timer(&sync, 2, 2).clone();

    // Only t1's schedule interval changes.
    sync.sync_timers(&[t1, t2], 2, 10, |t| {
        if t.physical_id == 1 {
            Some("30m".to_owned())
        } else {
            Some("1h".to_owned())
        }
    });

    let timer1_after = expect_timer(&sync, 1, 1);
    assert_eq!(timer1_after.id, timer1_before.id);
    assert_eq!(timer1_after.schedule_interval, "30m");
    assert_ne!(
        timer1_after.schedule_interval,
        timer1_before.schedule_interval
    );

    let timer2_after = expect_timer(&sync, 2, 2);
    assert_eq!(
        *timer2_after, timer2_before,
        "unrelated timer must not be touched by an unrelated update"
    );
}

// 对应 Go "truncate table" 场景：truncate 后物理表 ID 变化，旧 timer 应被标记为 disabled
// （保留在缓存中等待延迟删除），新物理 ID 对应一个全新的 enabled timer。
#[test]
fn test_sync_timers_truncate_disables_old_timer_and_creates_new_one() {
    let mut sync = TtlTimersSyncer::new();
    let old_table = table(5, 50, "test", "t", true);
    sync.sync_timers(&[old_table], 1, 0, |_| Some("1h".to_owned()));
    let old_timer = expect_timer(&sync, 5, 50).clone();
    assert!(old_timer.enabled);

    // Truncation assigns a brand-new physical id while keeping the logical table id.
    let truncated_table = table(5, 51, "test", "t", true);
    sync.sync_timers(&[truncated_table], 2, 5, |_| Some("1h".to_owned()));

    let old_timer_after = expect_timer(&sync, 5, 50);
    assert!(!old_timer_after.enabled);
    assert_eq!(old_timer_after.deleted_at, Some(5));

    let new_timer = expect_timer(&sync, 5, 51);
    assert!(new_timer.enabled);
    assert_ne!(new_timer.id, old_timer.id);
}

// 对应 Go "drop table" 与 "clear deleted tables"：表从 InfoSchema 消失后 timer 先转为 disabled，
// 经过 delay_delete_seconds 之后才会被彻底清理出缓存。
#[test]
fn test_sync_timers_delay_deletes_removed_tables() {
    let mut sync = TtlTimersSyncer::new();
    sync.set_delay_delete_interval(100);
    let t1 = table(1, 1, "test", "t1", true);
    sync.sync_timers(&[t1.clone()], 1, 0, |_| Some("1h".to_owned()));
    assert!(expect_timer(&sync, 1, 1).enabled);

    // Table dropped: sync with an empty table list.
    sync.sync_timers(&[], 2, 10, |_| Some("1h".to_owned()));
    let disabled = expect_timer(&sync, 1, 1);
    assert!(!disabled.enabled);
    assert_eq!(disabled.deleted_at, Some(10));

    // Still within the delay window: entry survives.
    sync.sync_timers(&[], 3, 50, |_| Some("1h".to_owned()));
    assert!(sync.cached_timer(&timer_key(1, 1)).is_some());

    // Past the delay window: entry is purged from the cache.
    sync.sync_timers(&[], 4, 200, |_| Some("1h".to_owned()));
    assert!(sync.cached_timer(&timer_key(1, 1)).is_none());
}

// 对应 Go TestTTLTimerSync 结尾的 Reset：清空 last_sync_time/last_sync_ver 和 timer 缓存。
#[test]
fn test_reset_clears_sync_state_and_cache() {
    let mut sync = TtlTimersSyncer::new();
    let t1 = table(1, 1, "test", "t1", true);
    sync.sync_timers(&[t1], 5, 100, |_| Some("1h".to_owned()));
    assert!(sync.cached_timer(&timer_key(1, 1)).is_some());

    sync.reset();
    let (time, ver) = sync.last_sync_info();
    assert_eq!(time, 0);
    assert_eq!(ver, 0);
    assert!(sync.cached_timer(&timer_key(1, 1)).is_none());
}

// 对应 Go TestTTLManualTriggerOneTimer 的成功路径：已同步且启用的 timer 可以被手动触发。
#[test]
fn test_manual_trigger_succeeds_for_enabled_timer() {
    let mut sync = TtlTimersSyncer::new();
    let t1 = table(1, 1, "test", "t1", true);
    sync.sync_timers(&[t1.clone()], 1, 0, |_| Some("1h".to_owned()));

    let (timer_id, request_id) = sync
        .manual_trigger(&t1, "req-1")
        .expect("trigger should succeed");
    assert!(!timer_id.is_empty());
    assert_eq!(request_id, "req-1");
}

// 对应 Go 中 "timer not exist" 分支：从未同步过的表没有对应 timer，手动触发应报错。
#[test]
fn test_manual_trigger_fails_when_timer_was_never_synced() {
    let sync = TtlTimersSyncer::new();
    let never_synced = table(9, 9, "test", "unknown", true);
    let err = sync
        .manual_trigger(&never_synced, "req-1")
        .expect_err("triggering an unknown timer must fail");
    assert_eq!(err, "timer not found");
}

// 对应 Go 中 "manual trigger is not allowed when timer is disabled" 分支：表被删除/禁用后
// timer 转为 disabled，此时手动触发应报错而不是静默使用旧记录。
#[test]
fn test_manual_trigger_fails_once_timer_is_disabled() {
    let mut sync = TtlTimersSyncer::new();
    sync.set_delay_delete_interval(1_000_000);
    let t1 = table(1, 1, "test", "t1", true);
    sync.sync_timers(&[t1.clone()], 1, 0, |_| Some("1h".to_owned()));
    // Drop the table: the cached timer becomes disabled but stays cached during the delay window.
    sync.sync_timers(&[], 2, 10, |_| Some("1h".to_owned()));

    let err = sync
        .manual_trigger(&t1, "req-1")
        .expect_err("triggering a disabled timer must fail");
    assert_eq!(err, "manual trigger is not allowed when timer is disabled");
}
