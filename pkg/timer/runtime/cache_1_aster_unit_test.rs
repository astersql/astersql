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

// TimersCache / worker / runtime 与 Go 行为对齐的单元测试。
//
// 覆盖缓存更新排序、处理状态复位、手动请求立即触发、重试时间下限、
// Location 偏移比较，以及 worker 构建事件更新与 runtime 处理 worker 响应。

use astersql_timer_runtime::api::{
    Context, Hook, ManualRequest, NewDefaultTimerClient, PreSchedEventResult, SchedEventIdle,
    SchedEventInterval, TimerClient, TimerRecord, TimerShedEvent, TimerSpec,
};
use astersql_timer_runtime::cache::{
    NowFn, TimersCache, locationChanged, procIdle, procTriggering, procWaitTriggerClose,
};
use astersql_timer_runtime::runtime::{NewTimerRuntimeBuilder, TriggerEventResponse};
use astersql_timer_runtime::worker::{TriggerEventRequest, buildEventUpdate, triggerEvent};
use chrono::{FixedOffset, TimeZone, Utc};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 测试固定时钟：2026-07-15 08:00:00 UTC。
fn fixed_now() -> chrono::DateTime<FixedOffset> {
    Utc.with_ymd_and_hms(2026, 7, 15, 8, 0, 0)
        .unwrap()
        .fixed_offset()
}

/// 构造启用的 INTERVAL 定时器夹具；watermark 相对 fixed_now 偏移分钟数。
fn timer(id: &str, expr: &str, watermark_minutes: i64) -> TimerRecord {
    TimerRecord {
        TimerSpec: TimerSpec {
            Namespace: "n1".to_string(),
            Key: format!("key-{id}"),
            SchedPolicyType: SchedEventInterval.to_string(),
            SchedPolicyExpr: expr.to_string(),
            HookClass: "hook1".to_string(),
            Watermark: Some(fixed_now() + chrono::Duration::minutes(watermark_minutes)),
            Enable: true,
            ..TimerSpec::default()
        },
        ID: id.to_string(),
        EventStatus: SchedEventIdle.to_string(),
        Version: 1,
        ..TimerRecord::default()
    }
}

/// 校验更新排序、WaitTriggerClose 复位、tryTrigger 过滤与全量刷新删多余项。
#[test]
fn cache_update_sort_status_and_full_refresh_match_go() {
    let now: NowFn = Arc::new(fixed_now);
    let mut cache = TimersCache::with_now(now);
    let mut t1 = timer("t1", "10m", 0);
    let t2 = timer("t2", "5m", 0);

    // 同版本重复更新应返回 false；更早触发的 t2 应排在 t1 前。
    assert!(cache.updateTimer(&t1));
    assert!(!cache.updateTimer(&t1.clone()));
    assert!(cache.updateTimer(&t2));
    assert_eq!(cache.sortedTimerIDs(), vec!["t2", "t1"]);

    // EventID 不变时保持 WaitTriggerClose；清空 EventID 后应回到 Idle。
    t1.EventID = "event-1".to_string();
    cache.setTimerProcStatus("t1", procWaitTriggerClose, "event-1".to_string());
    assert!(cache.waitCloseTimerIDs().contains("t1"));
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    assert_eq!(cache.item("t1").unwrap().procStatus, procWaitTriggerClose);

    t1.EventID.clear();
    t1.Version += 1;
    assert!(cache.updateTimer(&t1));
    assert_eq!(cache.item("t1").unwrap().procStatus, procIdle);
    assert!(!cache.waitCloseTimerIDs().contains("t1"));

    // Triggering 的 t1 不应出现在 tryTrigger 列表；全量刷新删除未出现的 t2。
    cache.setTimerProcStatus("t1", procTriggering, "event-2".to_string());
    assert_eq!(cache.tryTriggerTimerIDs(), vec!["t2"]);
    cache.fullUpdateTimers(vec![t1.clone()]);
    assert!(!cache.hasTimer("t2"));
    assert_eq!(cache.item("t1").unwrap().procStatus, procTriggering);
}

/// 校验手动请求立即触发、Idle 重试时间下限，以及 Location 偏移比较。
#[test]
fn cache_manual_request_retry_floor_and_location_match_go() {
    let mut cache = TimersCache::with_now(Arc::new(fixed_now));
    let mut record = timer("manual", "1m", 0);
    record.ManualRequest = ManualRequest {
        ManualRequestID: "request-1".to_string(),
        ManualRequestTime: Some(fixed_now()),
        ManualTimeout: Duration::from_secs(60),
        ..ManualRequest::default()
    };
    // 手动请求中 nextEventTime 应为 now。
    assert!(cache.updateTimer(&record));
    assert_eq!(
        cache.item("manual").unwrap().nextEventTime,
        Some(fixed_now())
    );

    // Idle 下早于 nextEventTime 的下调应被忽略。
    cache.updateNextTryTriggerTime("manual", fixed_now() - chrono::Duration::seconds(1));
    assert_eq!(
        cache.item("manual").unwrap().nextTryTriggerTime,
        fixed_now()
    );
    cache.updateNextTryTriggerTime("manual", fixed_now() + chrono::Duration::seconds(2));
    assert_eq!(
        cache.item("manual").unwrap().nextTryTriggerTime,
        fixed_now() + chrono::Duration::seconds(2)
    );

    let east = FixedOffset::east_opt(2 * 60 * 60).unwrap();
    let same = FixedOffset::east_opt(2 * 60 * 60).unwrap();
    let west = FixedOffset::west_opt(7 * 60 * 60).unwrap();
    assert!(!locationChanged(
        &Some(astersql_timer_runtime::api::TimerLocation::Fixed(east)),
        &Some(astersql_timer_runtime::api::TimerLocation::Fixed(same))
    ));
    assert!(locationChanged(
        &Some(astersql_timer_runtime::api::TimerLocation::Fixed(east)),
        &Some(astersql_timer_runtime::api::TimerLocation::Fixed(west))
    ));
}

/// 记录 PreSched / Sched 回调顺序的测试 Hook。
#[derive(Default)]
struct RecordingHook {
    events: Arc<Mutex<Vec<String>>>,
}

impl Hook for RecordingHook {
    fn Start(&mut self) {}
    fn Stop(&mut self) {}

    fn OnPreSchedEvent(
        &mut self,
        _ctx: &Context,
        event: &dyn TimerShedEvent,
    ) -> astersql_timer_runtime::api::TimerResult<PreSchedEventResult> {
        self.events
            .lock()
            .unwrap()
            .push(format!("pre:{}", event.EventID()));
        Ok(PreSchedEventResult {
            EventData: b"event-data".to_vec(),
            ..PreSchedEventResult::default()
        })
    }

    fn OnSchedEvent(
        &mut self,
        _ctx: &Context,
        event: &dyn TimerShedEvent,
    ) -> astersql_timer_runtime::api::TimerResult<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("sched:{}", event.EventID()));
        Ok(())
    }
}

/// 校验 buildEventUpdate 字段与 triggerEvent 调用 Hook 的顺序/落库结果。
#[test]
fn worker_builds_event_update_and_triggers_hook_like_go() {
    let store = astersql_timer_runtime::api::NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let ctx = Context::background();
    let created = client
        .CreateTimer(&ctx, timer("", "1m", -2).TimerSpec)
        .unwrap();
    let (response_tx, _response_rx) = crossbeam_channel::bounded(1);
    let request = TriggerEventRequest {
        eventID: "event-565".to_string(),
        timer: created.clone(),
        store: store.clone(),
        resp: response_tx,
    };

    let update = buildEventUpdate(
        &request,
        PreSchedEventResult {
            EventData: b"payload".to_vec(),
            ..PreSchedEventResult::default()
        },
        &fixed_now,
    );
    assert_eq!(update.EventID.Get(), Some(&"event-565".to_string()));
    assert_eq!(update.CheckVersion.Get(), Some(&created.Version));

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut hook = RecordingHook {
        events: Arc::clone(&events),
    };
    let response = triggerEvent(&ctx, Some(&mut hook), &request, &fixed_now);
    assert!(response.success);
    assert_eq!(
        events.lock().unwrap().as_slice(),
        ["pre:event-565", "sched:event-565"]
    );
    let refreshed = store.GetByID(&ctx, &created.ID).unwrap();
    assert_eq!(refreshed.EventID, "event-565");
    assert_eq!(refreshed.EventData, b"event-data");
}

/// 校验 runtime 处理 done/retry 响应后的状态、下次尝试时间与等待时长。
#[test]
fn runtime_handles_worker_response_and_trigger_duration_like_go() {
    let store = astersql_timer_runtime::api::NewMemoryTimerStore();
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store).Build();
    runtime.setNowFunc(Arc::new(fixed_now));
    runtime.cacheUpdate(timer("t1", "1m", -2));
    runtime.cacheSetProcStatus("t1", procTriggering, "event-1");

    let mut triggered = timer("t1", "1m", -2);
    triggered.Version = 2;
    triggered.EventStatus = astersql_timer_runtime::api::SchedEventTrigger.to_string();
    triggered.EventID = "event-1".to_string();
    triggered.EventStart = Some(fixed_now());
    // done → WaitTriggerClose；retry → 回到 Idle 并推迟下次尝试。
    runtime.handleWorkerResponse(TriggerEventResponse::done("t1", "event-1", Some(triggered)));
    assert_eq!(runtime.cacheProcStatus("t1"), Some(procWaitTriggerClose));

    runtime.handleWorkerResponse(TriggerEventResponse::retry(
        "t1",
        "event-1",
        Some(Duration::from_secs(12)),
    ));
    assert_eq!(runtime.cacheProcStatus("t1"), Some(procIdle));
    assert_eq!(
        runtime.cacheNextTryTime("t1"),
        Some(fixed_now() + chrono::Duration::seconds(12))
    );
    assert_eq!(
        runtime.getNextTryTriggerDuration(fixed_now()),
        Duration::from_secs(12)
    );
}
