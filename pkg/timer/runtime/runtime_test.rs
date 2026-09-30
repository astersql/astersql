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

// `TimerGroupRuntime` 行为测试。
//
// 覆盖启动停止、Worker 创建、触发优先级、Watch 批处理、全量/增量刷新
// 以及主循环 panic 恢复等路径。

use super::runtime_cache_test::new_test_timer;
use super::runtime_main_test::{
    TestHook, check_not_done, factory_classes, install_hook, new_mock_store, serial_guard,
    wait_until,
};
use crate::api::{
    Context, ManualRequest, NewDefaultTimerClient, NewMemoryTimerStore, NewOptionalVal, OperatorTp,
    PreSchedEventResult, SchedEventIdle, SchedEventInterval, SchedEventTrigger, TimerClient,
    TimerCond, TimerRecord, TimerSpec, TimerStore, WatchTimerEvent, WatchTimerEventCreate,
    WatchTimerEventDelete, WatchTimerEventUpdate, WatchTimerResponse, WithSetSummaryData,
    WithSetWatermark,
};
use crate::cache::{procIdle, procTriggering, procWaitTriggerClose};
use crate::runtime::{NewTimerRuntimeBuilder, retryBusyWorkerInterval};
use crate::worker::TriggerEventRequest;
use chrono::Utc;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 时间戳加上 Duration。
fn add(value: crate::api::Timestamp, duration: Duration) -> crate::api::Timestamp {
    value + chrono::Duration::from_std(duration).unwrap()
}

/// 时间戳减去 Duration。
fn sub(value: crate::api::Timestamp, duration: Duration) -> crate::api::Timestamp {
    value - chrono::Duration::from_std(duration).unwrap()
}

/// 构造带假响应通道的触发请求（测试用）。
fn request(timer: &TimerRecord, event_id: &str, store: &TimerStore) -> TriggerEventRequest {
    let (sender, _receiver) = crossbeam_channel::bounded(1);
    TriggerEventRequest {
        eventID: event_id.to_string(),
        timer: timer.clone(),
        store: store.clone(),
        resp: sender,
    }
}

#[test]
/// 验证 Start/Stop 生命周期与 Running 状态。
fn test_runtime_start_stop() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let ctx = Context::background();
    client
        .CreateTimer(
            &ctx,
            TimerSpec {
                Namespace: "n1".to_string(),
                Key: "k1".to_string(),
                SchedPolicyType: SchedEventInterval.to_string(),
                SchedPolicyExpr: "1m".to_string(),
                HookClass: "hook1".to_string(),
                Watermark: Some(sub(now, Duration::from_secs(120))),
                Enable: true,
                ..TimerSpec::default()
            },
        )
        .unwrap();
    let hook = TestHook::new();
    let factory = install_hook(hook.clone());
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store.clone())
        .RegisterHookFactory("hook1".to_string(), factory)
        .Build();

    runtime.Start();
    assert!(runtime.Running());
    wait_until(Duration::from_secs(5), || hook.sched_calls().len() == 1);
    assert!(runtime.fullRefreshCount() >= 1);
    runtime.Stop();
    assert!(!runtime.Running());
    wait_until(Duration::from_secs(2), || hook.stops() == 1);
    assert_eq!(hook.starts(), 1);
    assert_eq!(factory_classes(), vec!["hook1"]);
    store.Close();
}

#[test]
/// 验证按 HookClass 懒创建 Worker；未知 class 返回 None。
fn test_ensure_worker() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let store = NewMemoryTimerStore();
    let hook = TestHook::new();
    let factory = install_hook(hook.clone());
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store.clone())
        .RegisterHookFactory("hook1".to_string(), factory)
        .Build();
    runtime.setNowFunc(Arc::new(move || now));
    runtime.initCtx();

    let timer = new_test_timer("worker-1", "1m", sub(now, Duration::from_secs(3600)));
    runtime.cacheUpdate(timer.clone());
    runtime.tryTriggerTimerEvents();
    wait_until(Duration::from_secs(2), || hook.starts() == 1);
    runtime.tryTriggerTimerEvents();
    assert_eq!(factory_classes(), vec!["hook1"]);

    let mut missing = new_test_timer("worker-2", "1m", sub(now, Duration::from_secs(3600)));
    missing.HookClass = "hook2".to_string();
    runtime.cacheUpdate(missing.clone());
    runtime.tryTriggerTimerEvents();
    assert_eq!(runtime.cacheProcStatus(&missing.ID), Some(procIdle));
    runtime.Stop();
    assert_eq!(hook.stops(), 1);
    store.Close();
}

#[test]
/// 验证 tryTriggerTimerEvents：Idle/手动/通道满等路径。
fn test_try_trigger_timer() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let store = NewMemoryTimerStore();
    let hook = TestHook::new();
    hook.set_pre_block(Duration::from_secs(2));
    hook.set_pre_panic();
    let factory = install_hook(hook.clone());
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store.clone())
        .RegisterHookFactory("hook1".to_string(), factory)
        .Build();
    runtime.setNowFunc(Arc::new(move || now));
    runtime.initCtx();

    let t1 = new_test_timer("t1", "1m", sub(now, Duration::from_secs(3600)));
    runtime.cacheUpdate(t1.clone());
    let mut t2 = new_test_timer("t2", "1h", now);
    t2.EventStatus = SchedEventTrigger.to_string();
    t2.EventID = "event2".to_string();
    t2.EventStart = Some(sub(now, Duration::from_secs(3600)));
    t2.Enable = false;
    runtime.cacheUpdate(t2.clone());
    let mut t3 = new_test_timer("t3", "10m", now);
    runtime.cacheUpdate(t3.clone());
    let t4 = new_test_timer("t4", "1m", now);
    runtime.cacheUpdate(t4.clone());
    let t5 = new_test_timer("t5", "5m", sub(now, Duration::from_secs(600)));
    runtime.cacheUpdate(t5.clone());
    let t8 = new_test_timer("t8", "1m", sub(now, Duration::from_secs(7200)));
    runtime.cacheUpdate(t8.clone());
    runtime.cacheSetProcStatus(&t8.ID, procTriggering, "event8");
    let mut t9 = new_test_timer("t9", "1m", sub(now, Duration::from_secs(7200)));
    t9.EventStatus = SchedEventTrigger.to_string();
    t9.EventID = "event9".to_string();
    t9.EventStart = Some(sub(now, Duration::from_secs(7200)));
    runtime.cacheUpdate(t9.clone());
    runtime.cacheSetProcStatus(&t9.ID, procWaitTriggerClose, "event9");

    for index in 0..130 {
        runtime.cacheUpdate(new_test_timer(
            &format!("fill-{index:03}"),
            "1m",
            sub(now, Duration::from_secs(330)),
        ));
    }
    let t6 = new_test_timer("t6", "6m", sub(now, Duration::from_secs(600)));
    let t7 = new_test_timer("t7", "6m", sub(now, Duration::from_secs(600)));
    runtime.cacheUpdate(t6.clone());
    runtime.cacheUpdate(t7.clone());

    runtime.tryTriggerTimerEvents();
    assert_eq!(runtime.cacheProcStatus(&t1.ID), Some(procTriggering));
    assert_eq!(runtime.cacheProcStatus(&t2.ID), Some(procTriggering));
    assert_eq!(runtime.cacheProcStatus(&t3.ID), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus(&t4.ID), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus(&t5.ID), Some(procTriggering));
    assert_eq!(runtime.cacheProcStatus(&t8.ID), Some(procTriggering));
    assert_eq!(runtime.cacheProcStatus(&t9.ID), Some(procWaitTriggerClose));
    let retry_at = add(now, retryBusyWorkerInterval);
    assert_eq!(runtime.cacheProcStatus(&t6.ID), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus(&t7.ID), Some(procIdle));
    assert_eq!(runtime.cacheNextTryTime(&t6.ID), Some(retry_at));
    assert_eq!(runtime.cacheNextTryTime(&t7.ID), Some(retry_at));

    thread::sleep(Duration::from_millis(2200));
    runtime.Stop();
    store.Close();

    let store = NewMemoryTimerStore();
    let manual_hook = TestHook::new();
    let manual_runtime = NewTimerRuntimeBuilder("g1-manual".to_string(), store.clone())
        .RegisterHookFactory("hook1".to_string(), install_hook(manual_hook))
        .Build();
    manual_runtime.setNowFunc(Arc::new(move || now));
    manual_runtime.initCtx();
    t3.Version += 1;
    t3.ManualRequest = ManualRequest {
        ManualRequestID: "req1".to_string(),
        ManualRequestTime: Some(now),
        ManualTimeout: Duration::from_secs(60),
        ManualProcessed: true,
        ManualEventID: "event1".to_string(),
    };
    manual_runtime.cacheUpdate(t3.clone());
    manual_runtime.tryTriggerTimerEvents();
    assert_eq!(manual_runtime.cacheProcStatus(&t3.ID), Some(procIdle));
    t3.Version += 1;
    t3.Enable = false;
    t3.ManualRequest = ManualRequest {
        ManualRequestID: "req2".to_string(),
        ManualRequestTime: Some(now),
        ManualTimeout: Duration::from_secs(60),
        ..ManualRequest::default()
    };
    manual_runtime.cacheUpdate(t3.clone());
    manual_runtime.tryTriggerTimerEvents();
    assert_eq!(manual_runtime.cacheProcStatus(&t3.ID), Some(procIdle));
    t3.Version += 1;
    t3.Enable = true;
    manual_runtime.cacheUpdate(t3.clone());
    manual_runtime.tryTriggerTimerEvents();
    assert_eq!(manual_runtime.cacheProcStatus(&t3.ID), Some(procTriggering));
    manual_runtime.Stop();
    store.Close();
}

#[test]
/// 验证触发优先级按 nextEventTime / tryTriggerTime 排序。
fn test_try_trigger_time_priority() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let store = NewMemoryTimerStore();
    let hook = TestHook::new();
    hook.set_pre_block(Duration::from_secs(1));
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store.clone())
        .RegisterHookFactory("hook1".to_string(), install_hook(hook))
        .Build();
    runtime.setNowFunc(Arc::new(move || now));
    runtime.initCtx();

    let t1 = new_test_timer("t1", "1m", sub(now, Duration::from_secs(3600)));
    runtime.cacheUpdate(t1.clone());
    let t2 = new_test_timer("t2", "1m", sub(now, Duration::from_secs(7200)));
    runtime.cacheUpdate(t2.clone());
    let mut t3 = new_test_timer("t3", "1h", now);
    t3.EventStatus = SchedEventTrigger.to_string();
    t3.EventID = "event2".to_string();
    t3.EventStart = Some(sub(now, Duration::from_secs(60)));
    t3.Enable = false;
    runtime.cacheUpdate(t3.clone());
    let t4 = new_test_timer("t4", "1m", sub(now, Duration::from_secs(600)));
    runtime.cacheUpdate(t4.clone());

    for index in 0..128 {
        runtime.cacheUpdate(new_test_timer(
            &format!("priority-fill-{index:03}"),
            "1m",
            sub(now, Duration::from_secs(6000)),
        ));
    }
    runtime.tryTriggerTimerEvents();
    assert_eq!(runtime.cacheProcStatus(&t3.ID), Some(procTriggering));
    assert_eq!(runtime.cacheProcStatus(&t2.ID), Some(procTriggering));
    assert_eq!(runtime.cacheProcStatus(&t1.ID), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus(&t4.ID), Some(procIdle));
    runtime.Stop();
    store.Close();
}

#[test]
/// 验证 handleWorkerResponse 对成功、失败与元数据变更的缓存更新。
fn test_handle_hook_worker_response() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let store = NewMemoryTimerStore();
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store.clone()).Build();
    runtime.setNowFunc(Arc::new(move || now));
    runtime.initCtx();
    let t1 = new_test_timer("t1", "1m", sub(now, Duration::from_secs(3600)));

    runtime.cacheUpdate(t1.clone());
    runtime.cacheSetProcStatus(&t1.ID, procTriggering, "event1");
    let mut triggered = t1.clone();
    triggered.EventID = "event1".to_string();
    triggered.EventStatus = SchedEventTrigger.to_string();
    triggered.EventStart = Some(now);
    triggered.EventData = b"data1".to_vec();
    triggered.Version += 1;
    runtime.handleWorkerResponse(
        request(&t1, "event1", &store)
            .DoneResponse()
            .WithNewTimerRecord(Some(triggered.clone())),
    );
    assert_eq!(runtime.cacheProcStatus(&t1.ID), Some(procWaitTriggerClose));

    runtime.cacheUpdate(t1.clone());
    runtime.handleWorkerResponse(
        request(&t1, "event1", &store)
            .RetryDefaultResponse()
            .WithNewTimerRecord(None)
            .WithRetryImmediately(),
    );
    assert_eq!(runtime.cacheProcStatus(&t1.ID), None);

    runtime.cacheUpdate(t1.clone());
    let mut changed = t1.clone();
    changed.Version += 1;
    changed.Watermark = Some(add(now, Duration::from_secs(1)));
    runtime.handleWorkerResponse(
        request(&t1, "event1", &store)
            .RetryDefaultResponse()
            .WithNewTimerRecord(Some(changed.clone()))
            .WithRetryImmediately(),
    );
    assert_eq!(runtime.cacheProcStatus(&t1.ID), Some(procIdle));
    assert!(!runtime.cacheUpdate(changed));

    let mut reset = t1.clone();
    reset.Version += 2;
    runtime.cacheUpdate(reset);
    runtime.handleWorkerResponse(
        request(&t1, "event1", &store)
            .RetryDefaultResponse()
            .WithRetryAfter(Duration::from_secs(12)),
    );
    assert_eq!(runtime.cacheProcStatus(&t1.ID), Some(procIdle));
    assert_eq!(
        runtime.cacheNextTryTime(&t1.ID),
        Some(add(now, Duration::from_secs(12)))
    );
    runtime.Stop();
    store.Close();
}

#[test]
/// 验证 getNextTryTriggerDuration 的上下界裁剪。
fn test_next_try_trigger_duration() {
    let _serial = serial_guard();
    let base = Utc::now().fixed_offset();
    let now = Arc::new(Mutex::new(base));
    let now_fn = Arc::clone(&now);
    let store = NewMemoryTimerStore();
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store.clone()).Build();
    runtime.setNowFunc(Arc::new(move || *now_fn.lock().unwrap()));
    runtime.initCtx();
    let t1 = new_test_timer("t1", "0.1m", base);
    runtime.cacheUpdate(t1.clone());
    runtime.cacheSetProcStatus(&t1.ID, procTriggering, "event1");
    runtime.cacheUpdate(new_test_timer("t2", "1.5m", base));
    runtime.cacheUpdate(new_test_timer("t3", "2m", base));
    assert_eq!(
        runtime.getNextTryTriggerDuration(base),
        Duration::from_secs(60)
    );
    *now.lock().unwrap() = add(base, Duration::from_secs(70));
    let current = *now.lock().unwrap();
    assert_eq!(
        runtime.getNextTryTriggerDuration(current),
        Duration::from_secs(20)
    );
    *now.lock().unwrap() = add(base, Duration::from_millis(89_500));
    let last_try = sub(*now.lock().unwrap(), Duration::from_secs(1));
    assert_eq!(
        runtime.getNextTryTriggerDuration(last_try),
        Duration::from_millis(500)
    );
    let current = *now.lock().unwrap();
    assert_eq!(
        runtime.getNextTryTriggerDuration(current),
        Duration::from_secs(1)
    );
    assert_eq!(
        runtime.getNextTryTriggerDuration(add(current, Duration::from_millis(100))),
        Duration::from_secs(1)
    );
    *now.lock().unwrap() = add(base, Duration::from_secs(3600));
    assert_eq!(runtime.getNextTryTriggerDuration(base), Duration::ZERO);
    runtime.Stop();
    store.Close();
}

#[test]
/// 验证全量刷新与 List 失败时不破坏缓存。
fn test_full_refresh_timers() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let (core, store) = new_mock_store();
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store)
        .SetCond(Arc::new(TimerCond {
            Namespace: NewOptionalVal("n1".to_string()),
            ..TimerCond::default()
        }))
        .Build();
    runtime.initCtx();
    let mut timers = Vec::new();
    for index in 0..7 {
        let mut timer = new_test_timer(&format!("t{index}"), "1m", now);
        let mut status = procIdle;
        if index == 2 || index == 4 {
            timer.EventStatus = SchedEventTrigger.to_string();
            timer.EventStart = Some(now);
            timer.EventID = format!("event{}", index + 1);
            status = procWaitTriggerClose;
        } else if index == 6 {
            status = procTriggering;
        }
        runtime.cacheUpdate(timer.clone());
        runtime.cacheSetProcStatus(&timer.ID, status, &timer.EventID);
        timers.push(timer);
    }
    core.push_list_error("mockErr");
    runtime.fullRefreshTimers();
    assert_eq!(runtime.fullRefreshCount(), 1);
    assert!(
        timers
            .iter()
            .all(|timer| runtime.cacheProcStatus(&timer.ID).is_some())
    );

    let mut t0_new = timers[0].clone();
    t0_new.Version += 1;
    let mut t2_new = timers[2].clone();
    t2_new.Version += 1;
    let mut t4_new = timers[4].clone();
    t4_new.EventStatus = SchedEventIdle.to_string();
    t4_new.EventID.clear();
    t4_new.Version += 1;
    let mut t6_new = timers[6].clone();
    t6_new.Version += 1;
    core.push_list(vec![
        t0_new.clone(),
        timers[1].clone(),
        t2_new.clone(),
        t4_new.clone(),
        t6_new.clone(),
    ]);
    runtime.fullRefreshTimers();
    assert_eq!(runtime.fullRefreshCount(), 2);
    assert_eq!(runtime.cacheProcStatus("t0"), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus("t2"), Some(procWaitTriggerClose));
    assert_eq!(runtime.cacheProcStatus("t4"), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus("t6"), Some(procTriggering));
    assert_eq!(runtime.cacheProcStatus("t3"), None);
    assert_eq!(runtime.cacheProcStatus("t5"), None);
    assert!(!runtime.cacheUpdate(t0_new));
    runtime.Stop();
}

#[test]
/// 验证 Watch 批处理：创建/更新刷新、删除移除。
fn test_batch_handler_watch_responses() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let (core, store) = new_mock_store();
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store)
        .SetCond(Arc::new(TimerCond {
            Namespace: NewOptionalVal("n1".to_string()),
            ..TimerCond::default()
        }))
        .Build();
    runtime.initCtx();
    let mut timers = Vec::new();
    for index in 0..7 {
        let mut timer = new_test_timer(&format!("t{index}"), "1m", now);
        let status = if index == 2 {
            procWaitTriggerClose
        } else if index == 6 {
            procTriggering
        } else {
            procIdle
        };
        if index == 2 {
            timer.EventStatus = SchedEventTrigger.to_string();
            timer.EventStart = Some(now);
            timer.EventID = "event3".to_string();
        }
        runtime.cacheUpdate(timer.clone());
        runtime.cacheSetProcStatus(&timer.ID, status, &timer.EventID);
        timers.push(timer);
    }
    let t10 = new_test_timer("t10", "1m", now);
    let mut t2_new = timers[2].clone();
    t2_new.EventStatus = SchedEventIdle.to_string();
    t2_new.EventID.clear();
    t2_new.Version += 1;
    let mut t6_new = timers[6].clone();
    t6_new.Version += 1;
    core.push_list(vec![t2_new.clone(), t6_new.clone(), t10.clone()]);
    assert!(runtime.batchHandleWatchResponses(vec![
        WatchTimerResponse {
            Events: vec![
                WatchTimerEvent {
                    Tp: WatchTimerEventDelete,
                    TimerID: "t0".to_string()
                },
                WatchTimerEvent {
                    Tp: WatchTimerEventCreate,
                    TimerID: "t10".to_string()
                },
            ]
        },
        WatchTimerResponse {
            Events: vec![
                WatchTimerEvent {
                    Tp: WatchTimerEventUpdate,
                    TimerID: "t2".to_string()
                },
                WatchTimerEvent {
                    Tp: WatchTimerEventDelete,
                    TimerID: "t5".to_string()
                },
            ]
        },
    ]));
    assert_eq!(runtime.partialRefreshCount(), 1);
    assert_eq!(
        core.observed_ids().last().unwrap(),
        &vec!["t10".to_string(), "t2".to_string()]
    );
    assert_eq!(runtime.cacheProcStatus("t0"), None);
    assert_eq!(runtime.cacheProcStatus("t5"), None);
    assert_eq!(runtime.cacheProcStatus("t10"), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus("t2"), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus("t6"), Some(procTriggering));
    runtime.Stop();
}

#[test]
/// 验证 WaitTriggerClose 定时器通过增量刷新收尾。
fn test_close_waiting_close_timers() {
    let _serial = serial_guard();
    let now = Utc::now().fixed_offset();
    let (core, store) = new_mock_store();
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store)
        .SetCond(Arc::new(TimerCond {
            Namespace: NewOptionalVal("n1".to_string()),
            ..TimerCond::default()
        }))
        .Build();
    runtime.initCtx();
    assert!(!runtime.tryCloseTriggeringTimers());
    let mut timers = Vec::new();
    for index in 0..5 {
        let mut timer = new_test_timer(&format!("t{index}"), "1m", now);
        timer.EventStatus = SchedEventTrigger.to_string();
        timer.EventStart = Some(now);
        timer.EventID = format!("event{index}");
        runtime.cacheUpdate(timer.clone());
        runtime.cacheSetProcStatus(&timer.ID, procWaitTriggerClose, &timer.EventID);
        timers.push(timer);
    }
    core.push_list(timers.clone());
    assert!(!runtime.tryCloseTriggeringTimers());
    assert_eq!(core.observed_ids().last().unwrap().len(), 5);
    let ids: HashSet<String> = (0..5).map(|index| format!("t{index}")).collect();
    let cond = runtime.buildTimerIDsCond(&ids);
    let and = cond
        .as_any()
        .downcast_ref::<crate::api::Operator>()
        .unwrap();
    assert_eq!(and.Op, OperatorTp::OperatorAnd);
    assert!(!and.Not);
    assert_eq!(and.Children.len(), 2);
    let or = and.Children[1]
        .as_any()
        .downcast_ref::<crate::api::Operator>()
        .unwrap();
    assert_eq!(or.Op, OperatorTp::OperatorOr);
    assert_eq!(or.Children.len(), 5);

    let mut t1_new = timers[1].clone();
    t1_new.EventStatus = SchedEventIdle.to_string();
    t1_new.EventID.clear();
    t1_new.Version += 1;
    let mut t4_new = timers[4].clone();
    t4_new.EventID = "event_next".to_string();
    t4_new.Version += 1;
    core.push_list(vec![timers[0].clone(), t1_new, timers[2].clone(), t4_new]);
    assert!(runtime.tryCloseTriggeringTimers());
    assert_eq!(runtime.cacheProcStatus("t0"), Some(procWaitTriggerClose));
    assert_eq!(runtime.cacheProcStatus("t1"), Some(procIdle));
    assert_eq!(runtime.cacheProcStatus("t2"), Some(procWaitTriggerClose));
    assert_eq!(runtime.cacheProcStatus("t3"), None);
    assert_eq!(runtime.cacheProcStatus("t4"), Some(procIdle));
    runtime.Stop();
}

#[test]
/// 验证 WatchSupported 为假时得到 never 通道。
fn test_create_watch_timer_chan() {
    let _serial = serial_guard();
    let (core, store) = new_mock_store();
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store).Build();
    let (sender, receiver) = crossbeam_channel::bounded(1);
    sender
        .send(WatchTimerResponse {
            Events: vec![WatchTimerEvent {
                Tp: WatchTimerEventCreate,
                TimerID: "AAA".to_string(),
            }],
        })
        .unwrap();
    core.push_watch_supported(true);
    core.push_watch(receiver);
    let got = runtime.createWatchTimerChan(&Context::background());
    let response = got.try_recv().unwrap();
    assert_eq!(response.Events.len(), 1);
    assert_eq!(response.Events[0].TimerID, "AAA");
    core.push_watch_supported(false);
    let idle = runtime.createWatchTimerChan(&Context::background());
    assert!(matches!(
        idle.try_recv(),
        Err(crossbeam_channel::TryRecvError::Empty)
    ));
}

#[test]
/// 验证 Watch 断开后按间隔重新订阅。
fn test_watch_timer_retry() {
    let _serial = serial_guard();
    let (core, store) = new_mock_store();
    let (closed_sender, closed) = crossbeam_channel::bounded(0);
    drop(closed_sender);
    let (_normal_sender, normal) = crossbeam_channel::bounded(0);
    core.push_watch_supported(true);
    core.push_watch_supported(true);
    core.push_watch(closed);
    core.push_watch(normal);
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store).Build();
    let started = Instant::now();
    runtime.Start();
    wait_until(Duration::from_secs(7), || core.watch_calls() >= 2);
    assert!(started.elapsed() >= Duration::from_secs(5));
    runtime.Stop();
}

#[test]
fn go_merge_43_timer_watch_update_triggers_manual_request_promptly() {
    let _serial = serial_guard();
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let ctx = Context::background();
    let timer = client
        .CreateTimer(
            &ctx,
            TimerSpec {
                Key: "watch-manual".into(),
                SchedPolicyType: SchedEventInterval.into(),
                SchedPolicyExpr: "24h".into(),
                HookClass: "watch-hook".into(),
                Watermark: Some(Utc::now().fixed_offset()),
                Enable: true,
                ..TimerSpec::default()
            },
        )
        .unwrap();
    let hook = TestHook::new();
    let runtime = NewTimerRuntimeBuilder("watch-group".into(), store.clone())
        .RegisterHookFactory("watch-hook".into(), install_hook(hook.clone()))
        .Build();
    runtime.Start();
    wait_until(Duration::from_secs(2), || runtime.fullRefreshCount() > 0);
    client.ManualTriggerEvent(&ctx, &timer.ID).unwrap();
    wait_until(Duration::from_secs(5), || !hook.sched_calls().is_empty());
    runtime.Stop();
    store.Close();
}

#[test]
/// 端到端：从到期到 Hook 回调再到状态收尾。
fn test_timer_full_process() {
    let _serial = serial_guard();
    let base = Utc::now().fixed_offset();
    let now = Arc::new(Mutex::new(base));
    let now_fn = Arc::clone(&now);
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let hook = TestHook::new();
    hook.push_pre_result(Ok(PreSchedEventResult {
        EventData: b"eventdata1".to_vec(),
        ..PreSchedEventResult::default()
    }));
    hook.push_sched_result(Ok(()));
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store.clone())
        .RegisterHookFactory("h1".to_string(), install_hook(hook.clone()))
        .Build();
    runtime.setNowFunc(Arc::new(move || *now_fn.lock().unwrap()));
    let ctx = Context::todo();
    let timer = client
        .CreateTimer(
            &ctx,
            TimerSpec {
                Key: "key1".to_string(),
                Data: b"timer1data".to_vec(),
                SchedPolicyType: SchedEventInterval.to_string(),
                SchedPolicyExpr: "1m".to_string(),
                HookClass: "h1".to_string(),
                Watermark: Some(sub(base, Duration::from_secs(120))),
                Enable: true,
                ..TimerSpec::default()
            },
        )
        .unwrap();
    runtime.Start();
    wait_until(Duration::from_secs(5), || hook.sched_calls().len() == 1);
    let triggered = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(triggered.EventStatus, SchedEventTrigger);
    assert_eq!(triggered.EventData, b"eventdata1");
    assert_eq!(triggered.EventStart, Some(base));
    check_not_done(Duration::from_millis(200), || hook.sched_calls().len() > 1);

    client
        .CloseTimerEvent(
            &ctx,
            &timer.ID,
            &triggered.EventID,
            vec![
                WithSetWatermark(add(base, Duration::from_secs(120))),
                WithSetSummaryData(b"summary1".to_vec()),
            ],
        )
        .unwrap();
    let closed = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(closed.EventStatus, SchedEventIdle);
    assert!(closed.EventID.is_empty());
    assert!(closed.EventStart.is_none());
    assert!(closed.EventData.is_empty());
    assert_eq!(closed.SummaryData, b"summary1");

    hook.push_pre_result(Ok(PreSchedEventResult {
        EventData: b"eventdata2".to_vec(),
        ..PreSchedEventResult::default()
    }));
    hook.push_sched_result(Ok(()));
    *now.lock().unwrap() = add(base, Duration::from_secs(181));
    let ids = HashSet::from([timer.ID.clone()]);
    assert!(runtime.partialRefreshTimers(&ids));
    runtime.tryTriggerTimerEvents();
    wait_until(Duration::from_secs(5), || hook.sched_calls().len() == 2);
    let triggered_again = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(triggered_again.EventStatus, SchedEventTrigger);
    assert_eq!(triggered_again.EventData, b"eventdata2");
    runtime.Stop();
    store.Close();
}

#[test]
/// 验证主循环 panic 后可由恢复外壳重试继续。
fn test_timer_runtime_loop_panic_recover() {
    let _serial = serial_guard();
    let (core, store) = new_mock_store();
    core.push_watch_supported(false);
    core.push_watch_supported(false);
    core.push_watch_supported(false);
    core.push_list_panic("store panic 1");
    core.push_list_panic("store panic 2");
    core.push_list(Vec::new());
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store).Build();
    runtime.Start();
    wait_until(Duration::from_secs(25), || core.list_calls() >= 3);
    assert!(runtime.Running());
    runtime.Stop();

    let (core, store) = new_mock_store();
    core.push_watch_supported(false);
    core.push_list_panic("store panic always");
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store).Build();
    runtime.Start();
    wait_until(Duration::from_secs(2), || core.list_calls() >= 1);
    let before_stop = Instant::now();
    runtime.Stop();
    assert!(before_stop.elapsed() < Duration::from_secs(1));

    let (core, store) = new_mock_store();
    core.push_watch_supported(false);
    core.push_list_panic("store panic before long retry");
    let runtime = NewTimerRuntimeBuilder("g1".to_string(), store).Build();
    runtime.Start();
    wait_until(Duration::from_secs(2), || core.list_calls() >= 1);
    thread::sleep(Duration::from_millis(20));
    let before_stop = Instant::now();
    runtime.Stop();
    assert!(before_stop.elapsed() < Duration::from_secs(1));
}
