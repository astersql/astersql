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

// `TimersCache` 单元测试。
//
// 验证定时器插入/更新、按触发时间排序、全量替换以及 Location 变更检测。

use crate::api::{
    ManualRequest, SchedEventCron, SchedEventIdle, SchedEventInterval, SchedEventTrigger,
    TimerLocation, TimerRecord, TimerSpec, Timestamp,
};
use crate::cache::{
    TimersCache, locationChanged, newTimersCache, procIdle, procTriggering, procWaitTriggerClose,
};
use chrono::{FixedOffset, TimeZone, Utc};
use std::sync::Arc;
use std::time::Duration;

/// 构造带间隔策略的测试用定时器记录。
pub fn new_test_timer(id: &str, policy_expr: &str, watermark: Timestamp) -> TimerRecord {
    TimerRecord {
        ID: id.to_string(),
        TimerSpec: TimerSpec {
            Namespace: "n1".to_string(),
            Key: format!("key-{id}"),
            SchedPolicyType: SchedEventInterval.to_string(),
            SchedPolicyExpr: policy_expr.to_string(),
            HookClass: "hook1".to_string(),
            Watermark: Some(watermark),
            Enable: true,
            ..TimerSpec::default()
        },
        Location: Some(TimerLocation::Fixed(*watermark.offset())),
        EventStatus: SchedEventIdle.to_string(),
        Version: 1,
        ..TimerRecord::default()
    }
}

/// 返回极远未来时间，用于断言“不会触发”的场景。
fn far_future() -> Timestamp {
    Utc.with_ymd_and_hms(2999, 1, 1, 0, 0, 0)
        .single()
        .unwrap()
        .fixed_offset()
}

/// Go `time.Time{}` 对应的 UTC 零值。
fn go_zero_time() -> Timestamp {
    Utc.with_ymd_and_hms(1, 1, 1, 0, 0, 0)
        .single()
        .unwrap()
        .fixed_offset()
}

/// 时间戳加上 std Duration。
fn add(value: Timestamp, duration: Duration) -> Timestamp {
    value + chrono::Duration::from_std(duration).unwrap()
}

/// 时间戳减去 std Duration。
fn sub(value: Timestamp, duration: Duration) -> Timestamp {
    value - chrono::Duration::from_std(duration).unwrap()
}

/// 校验缓存按 tryTriggerTime 排序的遍历结果与期望一致。
fn check_sorted_cache(cache: &TimersCache, sorted: &[(&TimerRecord, Timestamp)]) {
    let mut index = 0;
    cache.iterTryTriggerTimers(|timer, try_trigger_time, next_event_time| {
        let (expected_timer, expected_try_time) = sorted[index];
        assert_eq!(timer, expected_timer);
        let item = cache.item(&timer.ID).expect("timer must be cached");
        assert_eq!(&item.timer, expected_timer);

        if timer.ManualRequest.IsManualRequesting() {
            assert_eq!(Some(try_trigger_time), next_event_time);
        } else {
            match timer.NextEventTime() {
                Ok((next, ok)) if !timer.Enable => {
                    assert!(next.is_none());
                    assert!(!ok);
                }
                Ok((next, ok)) => {
                    assert!(ok);
                    assert_eq!(next, next_event_time);
                }
                Err(_) => assert!(next_event_time.is_none()),
            }
        }
        assert_eq!(try_trigger_time, expected_try_time);
        index += 1;
        true
    });
    assert_eq!(index, sorted.len());
}

#[test]
/// 覆盖 updateTimer：插入、版本冲突、禁用与手动请求对触发时间的影响。
fn test_cache_update() {
    let now = Utc::now().fixed_offset();
    let mut cache = newTimersCache();
    cache.setNowFunc(Arc::new(move || now));

    let mut t1 = new_test_timer("t1", "10m", now);
    assert!(cache.updateTimer(&t1));
    assert_eq!(cache.item(&t1.ID).unwrap().timer, t1);
    check_sorted_cache(&cache, &[(&t1, add(now, Duration::from_secs(600)))]);
    assert_eq!(cache.sortedTimerIDs().len(), 1);

    assert!(!cache.updateTimer(&t1.Clone()));
    check_sorted_cache(&cache, &[(&t1, add(now, Duration::from_secs(600)))]);

    t1.SchedPolicyType = SchedEventCron.to_string();
    t1.SchedPolicyExpr = "* 1 * * *".to_string();
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    let cron_next = t1.NextEventTime().unwrap().0.unwrap();
    check_sorted_cache(&cache, &[(&t1, cron_next)]);

    t1.Location = Some(TimerLocation::Fixed(
        FixedOffset::east_opt(2 * 3600).unwrap(),
    ));
    assert!(cache.updateTimer(&t1));
    let zoned_cron_next = t1.NextEventTime().unwrap().0.unwrap();
    check_sorted_cache(&cache, &[(&t1, zoned_cron_next)]);

    t1.Location = Some(TimerLocation::Fixed(*now.offset()));
    t1.SchedPolicyType = SchedEventInterval.to_string();
    t1.SchedPolicyExpr = "invalid".to_string();
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    check_sorted_cache(&cache, &[(&t1, far_future())]);

    cache.updateNextTryTriggerTime(&t1.ID, add(now, Duration::from_secs(7)));
    check_sorted_cache(&cache, &[(&t1, far_future())]);

    t1.SchedPolicyExpr = "1m".to_string();
    t1.Enable = false;
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    check_sorted_cache(&cache, &[(&t1, far_future())]);

    t1.Enable = true;
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    check_sorted_cache(&cache, &[(&t1, add(now, Duration::from_secs(60)))]);
    cache.updateNextTryTriggerTime(&t1.ID, add(now, Duration::from_secs(59)));
    check_sorted_cache(&cache, &[(&t1, add(now, Duration::from_secs(60)))]);
    cache.updateNextTryTriggerTime(&t1.ID, add(now, Duration::from_secs(61)));
    check_sorted_cache(&cache, &[(&t1, add(now, Duration::from_secs(61)))]);

    t1.Version += 1;
    cache.setTimerProcStatus(&t1.ID, procTriggering, "event1".to_string());
    assert!(cache.updateTimer(&t1));
    check_sorted_cache(&cache, &[]);
    let item = cache.item(&t1.ID).unwrap();
    assert_eq!(item.procStatus, procTriggering);
    assert_eq!(item.triggerEventID, "event1");

    t1.EventStatus = SchedEventTrigger.to_string();
    t1.EventStart = Some(sub(now, Duration::from_secs(10)));
    t1.EventID = "event1".to_string();
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    cache.setTimerProcStatus(&t1.ID, procIdle, "event1".to_string());
    check_sorted_cache(&cache, &[(&t1, sub(now, Duration::from_secs(10)))]);
    assert!(cache.waitCloseTimerIDs().is_empty());

    cache.setTimerProcStatus(&t1.ID, procWaitTriggerClose, "event1".to_string());
    check_sorted_cache(&cache, &[]);
    assert!(cache.waitCloseTimerIDs().contains(&t1.ID));
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    assert_eq!(cache.item(&t1.ID).unwrap().procStatus, procWaitTriggerClose);
    assert!(cache.waitCloseTimerIDs().contains(&t1.ID));

    t1.EventStatus = SchedEventIdle.to_string();
    t1.EventID.clear();
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    assert_eq!(cache.item(&t1.ID).unwrap().procStatus, procIdle);
    assert_eq!(cache.item(&t1.ID).unwrap().triggerEventID, "");
    assert!(cache.waitCloseTimerIDs().is_empty());

    t1.Version += 1;
    t1.ManualRequest = ManualRequest {
        ManualRequestID: "req1".to_string(),
        ManualRequestTime: Some(now),
        ManualTimeout: Duration::from_secs(60),
        ManualProcessed: true,
        ..ManualRequest::default()
    };
    assert!(cache.updateTimer(&t1));
    check_sorted_cache(&cache, &[(&t1, add(now, Duration::from_secs(60)))]);

    t1.Version += 1;
    t1.ManualRequest = ManualRequest {
        ManualRequestID: "req2".to_string(),
        ManualRequestTime: Some(now),
        ManualTimeout: Duration::from_secs(60),
        ..ManualRequest::default()
    };
    assert!(cache.updateTimer(&t1));
    assert_eq!(cache.item(&t1.ID).unwrap().procStatus, procIdle);
    assert!(cache.waitCloseTimerIDs().is_empty());
    check_sorted_cache(&cache, &[(&t1, now)]);
}

#[test]
/// Trigger 状态即使缺失 EventStart，也应像 Go 的 time.Time 零值一样参与排序。
fn test_trigger_without_event_start_uses_go_zero_time() {
    let now = Utc::now().fixed_offset();
    let mut cache = newTimersCache();
    let mut timer = new_test_timer("trigger-with-zero-start", "10m", now);
    timer.EventStatus = SchedEventTrigger.to_string();
    timer.EventStart = None;

    assert!(cache.updateTimer(&timer));
    check_sorted_cache(&cache, &[(&timer, go_zero_time())]);
}

#[test]
/// 空 ID 仍是已缓存记录；同版本重复更新必须像 Go 的非空 timer 指针一样被拒绝。
fn test_empty_id_same_version_update_is_ignored() {
    let now = Utc::now().fixed_offset();
    let mut cache = newTimersCache();
    let timer = new_test_timer("", "10m", now);

    assert!(cache.updateTimer(&timer));
    assert!(!cache.updateTimer(&timer.Clone()));
    assert_eq!(cache.sortedTimerIDs(), vec![String::new()]);
}

#[test]
/// 覆盖多定时器场景下缓存排序与下次触发时间。
fn test_cache_sort() {
    let now = Utc::now().fixed_offset();
    let mut cache = newTimersCache();
    cache.setNowFunc(Arc::new(move || now));
    let minute = |minutes: u64| add(now, Duration::from_secs(minutes * 60));

    let mut t1 = new_test_timer("t1", "10m", now);
    let mut t2 = new_test_timer("t2", "20m", now);
    let mut t3 = new_test_timer("t3", "5m", now);
    let mut t4 = new_test_timer("t4", "3m", now);
    check_sorted_cache(&cache, &[]);
    assert!(cache.updateTimer(&t1));
    check_sorted_cache(&cache, &[(&t1, minute(10))]);
    assert!(cache.updateTimer(&t2));
    check_sorted_cache(&cache, &[(&t1, minute(10)), (&t2, minute(20))]);
    assert!(cache.updateTimer(&t3));
    check_sorted_cache(
        &cache,
        &[(&t3, minute(5)), (&t1, minute(10)), (&t2, minute(20))],
    );
    assert!(cache.updateTimer(&t4));
    check_sorted_cache(
        &cache,
        &[
            (&t4, minute(3)),
            (&t3, minute(5)),
            (&t1, minute(10)),
            (&t2, minute(20)),
        ],
    );

    t3.SchedPolicyExpr = "1m".to_string();
    t3.Version += 1;
    assert!(cache.updateTimer(&t3));
    check_sorted_cache(
        &cache,
        &[
            (&t3, minute(1)),
            (&t4, minute(3)),
            (&t1, minute(10)),
            (&t2, minute(20)),
        ],
    );
    t2.SchedPolicyExpr = "2m".to_string();
    t2.Version += 1;
    assert!(cache.updateTimer(&t2));
    check_sorted_cache(
        &cache,
        &[
            (&t3, minute(1)),
            (&t2, minute(2)),
            (&t4, minute(3)),
            (&t1, minute(10)),
        ],
    );
    t4.SchedPolicyExpr = "15m".to_string();
    t4.Version += 1;
    assert!(cache.updateTimer(&t4));
    check_sorted_cache(
        &cache,
        &[
            (&t3, minute(1)),
            (&t2, minute(2)),
            (&t1, minute(10)),
            (&t4, minute(15)),
        ],
    );
    t3.SchedPolicyExpr = "12m".to_string();
    t3.Version += 1;
    assert!(cache.updateTimer(&t3));
    check_sorted_cache(
        &cache,
        &[
            (&t2, minute(2)),
            (&t1, minute(10)),
            (&t3, minute(12)),
            (&t4, minute(15)),
        ],
    );
    t2.SchedPolicyExpr = "1m".to_string();
    t2.Version += 1;
    assert!(cache.updateTimer(&t2));
    check_sorted_cache(
        &cache,
        &[
            (&t2, minute(1)),
            (&t1, minute(10)),
            (&t3, minute(12)),
            (&t4, minute(15)),
        ],
    );
    t1.SchedPolicyExpr = "11m".to_string();
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    check_sorted_cache(
        &cache,
        &[
            (&t2, minute(1)),
            (&t1, minute(11)),
            (&t3, minute(12)),
            (&t4, minute(15)),
        ],
    );
    t4.SchedPolicyExpr = "16m".to_string();
    t4.Version += 1;
    assert!(cache.updateTimer(&t4));
    check_sorted_cache(
        &cache,
        &[
            (&t2, minute(1)),
            (&t1, minute(11)),
            (&t3, minute(12)),
            (&t4, minute(16)),
        ],
    );

    cache.updateNextTryTriggerTime(&t2.ID, minute(20));
    check_sorted_cache(
        &cache,
        &[
            (&t1, minute(11)),
            (&t3, minute(12)),
            (&t4, minute(16)),
            (&t2, minute(20)),
        ],
    );
    cache.updateNextTryTriggerTime(&t2.ID, minute(14));
    check_sorted_cache(
        &cache,
        &[
            (&t1, minute(11)),
            (&t3, minute(12)),
            (&t2, minute(14)),
            (&t4, minute(16)),
        ],
    );
    cache.updateNextTryTriggerTime(&t3.ID, minute(15));
    check_sorted_cache(
        &cache,
        &[
            (&t1, minute(11)),
            (&t2, minute(14)),
            (&t3, minute(15)),
            (&t4, minute(16)),
        ],
    );
    t3.Version += 1;
    assert!(cache.updateTimer(&t3));
    check_sorted_cache(
        &cache,
        &[
            (&t1, minute(11)),
            (&t3, minute(12)),
            (&t2, minute(14)),
            (&t4, minute(16)),
        ],
    );
}

#[test]
/// 覆盖 fullUpdateTimers：全量替换缓存内容。
fn test_full_update_cache() {
    let now = Utc::now().fixed_offset();
    let mut cache = newTimersCache();
    cache.setNowFunc(Arc::new(move || now));
    let mut t1 = new_test_timer("t1", "10m", now);
    let t2 = new_test_timer("t2", "20m", now);
    let mut t3 = new_test_timer("t3", "30m", now);
    let t4 = new_test_timer("t4", "40m", now);
    for timer in [&t1, &t2, &t3, &t4] {
        assert!(cache.updateTimer(timer));
    }
    t1.SchedPolicyExpr = "15m".to_string();
    t1.Version += 1;
    t3.SchedPolicyExpr = "1m".to_string();
    t3.Version += 1;
    let t5 = new_test_timer("t5", "25m", now);
    cache.fullUpdateTimers(vec![t1.clone(), t3.clone(), t5.clone()]);
    check_sorted_cache(
        &cache,
        &[
            (&t3, add(now, Duration::from_secs(60))),
            (&t1, add(now, Duration::from_secs(900))),
            (&t5, add(now, Duration::from_secs(1500))),
        ],
    );
    assert_eq!(cache.sortedTimerIDs().len(), 3);
    assert!(!cache.hasTimer(&t2.ID));
    assert!(!cache.hasTimer(&t4.ID));
}

#[test]
/// 覆盖 locationChanged：时区/位置变更检测。
fn test_location_changed() {
    let ny1 = Some(TimerLocation::Named("America/New_York".parse().unwrap()));
    let la = Some(TimerLocation::Named("America/Los_Angeles".parse().unwrap()));
    let ny2 = Some(TimerLocation::Named("America/New_York".parse().unwrap()));
    let fixed_name1 = Some(TimerLocation::Fixed(FixedOffset::east_opt(7200).unwrap()));
    let fixed_name2 = Some(TimerLocation::Fixed(FixedOffset::east_opt(7200).unwrap()));
    let fixed_other = Some(TimerLocation::Fixed(FixedOffset::east_opt(3600).unwrap()));
    for (index, (a, b, expected)) in [
        (None, None, false),
        (ny1.clone(), None, true),
        (None, ny1.clone(), true),
        (ny1.clone(), la, true),
        (ny1, ny2, false),
        (fixed_name1.clone(), fixed_name2, false),
        (fixed_name1, fixed_other, true),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(locationChanged(&a, &b), expected, "case {index}");
    }
}
