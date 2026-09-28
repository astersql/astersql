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

//! `pkg/timer/store_intergartion_test.go` 对应的 Rust 可执行测试。
//!
//! Go 版本同时检验内存存储和 SQL 存储；SQL 测试夹具依赖 Cargo 单元测试进程外的
//! TiDB/etcd 服务，因此这里通过真实内存实现验证可确定复现的存储契约。SQL 编码、
//! 会话边界等行为由 `pkg/timer/tablestore/sql_test.rs` 及表存储实现测试覆盖。

use astersql_timer_api::{
    And, Context, ErrEventIDNotMatch, ErrTimerNotExist, ErrVersionNotMatch, EventExtra,
    ManualRequest, NewMemTimerWatchEventNotifier, NewMemoryTimerStore, NewOptionalVal, Not,
    SchedEventIdle, SchedEventInterval, SchedEventTrigger, TimerCond, TimerRecord, TimerSpec,
    TimerStore, TimerUpdate, TimerWatchEventNotifier, WatchTimerChan, WatchTimerEventCreate,
    WatchTimerEventDelete, WatchTimerEventUpdate,
};
use std::sync::Arc;
use std::time::Duration;

/// 构造仅包含测试所需字段的计时器记录模板。
fn record(namespace: &str, key: &str) -> TimerRecord {
    TimerRecord {
        TimerSpec: TimerSpec {
            Namespace: namespace.to_string(),
            Key: key.to_string(),
            Data: b"data1".to_vec(),
            SchedPolicyType: SchedEventInterval.to_string(),
            SchedPolicyExpr: "1h".to_string(),
            ..TimerSpec::default()
        },
        ..TimerRecord::default()
    }
}

/// 断言操作失败，且错误经展示后的文本与 Go 版契约一致。
fn assert_message<T>(result: Result<T, astersql_timer_api::TimerError>, expected: &str) {
    match result {
        Ok(_) => panic!("operation unexpectedly succeeded"),
        Err(error) => assert_eq!(error.to_string(), expected),
    }
}

/// 等待单批监听响应，并校验其中唯一事件的类型和计时器 ID。
fn assert_event(watcher: &WatchTimerChan, event_type: i8, timer_id: &str) {
    let response = watcher
        .recv_timeout(Duration::from_secs(2))
        .expect("watch event was not delivered");
    assert_eq!(response.Events.len(), 1);
    assert_eq!(response.Events[0].Tp, event_type);
    assert_eq!(response.Events[0].TimerID, timer_id);
}

/// 忽略存储返回顺序，按业务键比较记录集合。
fn assert_record_ids(store: &TimerStore, ctx: &Context, expected: &[&str]) {
    let mut actual = store
        .List(ctx, None)
        .unwrap()
        .into_iter()
        .map(|record| record.Key.clone())
        .collect::<Vec<_>>();
    actual.sort();
    let mut expected = expected
        .iter()
        .map(|key| (*key).to_string())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
/// 覆盖内存存储的增删改查、输入校验和乐观并发检查。
fn test_mem_timer_store_crud_update_delete_and_validation() {
    let ctx = Context::background();
    let store = NewMemoryTimerStore();
    assert!(store.WatchSupported());
    assert!(store.List(&ctx, None).unwrap().is_empty());

    let template = record("n1", "/path/to/key");
    let id = store.Create(&ctx, Some(template.clone())).unwrap();
    assert!(!id.is_empty());
    let mut expected = template;
    expected.ID = id.clone();
    expected.Version = 1;
    expected.EventStatus = SchedEventIdle.to_string();
    let got = store.GetByID(&ctx, &id).unwrap();
    expected.Location = got.Location.clone();
    expected.CreateTime = got.CreateTime;
    assert_eq!(got, expected);
    assert_ne!(got.CreateTime, None);
    assert_eq!(
        store.GetByKey(&ctx, "n1", "/path/to/key").unwrap(),
        expected
    );
    assert_eq!(store.GetByID(&ctx, "noexist"), Err(ErrTimerNotExist));
    assert_eq!(store.GetByKey(&ctx, "n1", "noexist"), Err(ErrTimerNotExist));
    assert_eq!(
        store.GetByKey(&ctx, "n2", "/path/to/ke"),
        Err(ErrTimerNotExist)
    );
    assert_message(
        store.Create(&ctx, Some(record("n1", "/path/to/key"))),
        "timer already exists",
    );

    let mut invalid = TimerRecord::default();
    assert_message(
        store.Create(&ctx, Some(invalid.clone())),
        "field 'Namespace' should not be empty",
    );
    invalid.Namespace = "n1".to_string();
    assert_message(
        store.Create(&ctx, Some(invalid.clone())),
        "field 'Key' should not be empty",
    );
    invalid.Key = "k1".to_string();
    assert_message(
        store.Create(&ctx, Some(invalid.clone())),
        "field 'SchedPolicyType' should not be empty",
    );
    invalid.SchedPolicyType = SchedEventInterval.to_string();
    invalid.SchedPolicyExpr = "1x".to_string();
    assert_message(
        store.Create(&ctx, Some(invalid.clone())),
        "schedule event configuration is not valid: invalid schedule event expr '1x': unknown unit x",
    );
    invalid.SchedPolicyExpr = "1h".to_string();
    invalid.TimeZone = "tidb".to_string();
    assert_message(
        store.Create(&ctx, Some(invalid)),
        "Unknown or incorrect time zone: 'tidb'",
    );

    let mut preset_id = record("n1", "preset-id");
    preset_id.ID = "provided".to_string();
    assert_message(
        store.Create(&ctx, Some(preset_id)),
        "ID should not be specified when create record",
    );
    let mut preset_version = record("n1", "preset-version");
    preset_version.Version = 1;
    assert_message(
        store.Create(&ctx, Some(preset_version)),
        "Version should not be specified when create record",
    );

    // 使用相对当前时间的时间戳，避免运行环境的固定时钟假设影响字段往返校验。
    let event_start = astersql_timer_api::now_timestamp() - Duration::from_secs(3600);
    let watermark = astersql_timer_api::now_timestamp() - Duration::from_secs(1800);
    let original = store.GetByID(&ctx, &id).unwrap();
    let event_id = "event-1".to_string();
    store
        .Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                Tags: NewOptionalVal(vec!["l1".to_string(), "l2".to_string()]),
                TimeZone: NewOptionalVal("UTC".to_string()),
                SchedPolicyExpr: NewOptionalVal("2h".to_string()),
                ManualRequest: NewOptionalVal(ManualRequest {
                    ManualRequestID: "req1".to_string(),
                    ManualRequestTime: Some(event_start),
                    ManualTimeout: Duration::from_secs(60),
                    ManualProcessed: true,
                    ManualEventID: "event1".to_string(),
                }),
                EventStatus: NewOptionalVal(SchedEventTrigger.to_string()),
                EventID: NewOptionalVal(event_id.clone()),
                EventData: NewOptionalVal(b"eventdata1".to_vec()),
                EventStart: NewOptionalVal(Some(event_start)),
                EventExtra: NewOptionalVal(EventExtra {
                    EventManualRequestID: "req2".to_string(),
                    EventWatermark: Some(watermark),
                }),
                Watermark: NewOptionalVal(Some(watermark)),
                SummaryData: NewOptionalVal(b"summary1".to_vec()),
                CheckVersion: NewOptionalVal(original.Version),
                CheckEventID: NewOptionalVal(String::new()),
                ..TimerUpdate::default()
            }),
        )
        .unwrap();
    let updated = store.GetByID(&ctx, &id).unwrap();
    assert_eq!(updated.Version, original.Version + 1);
    assert_eq!(updated.TimeZone, "UTC");
    assert_eq!(updated.Tags, vec!["l1", "l2"]);
    assert_eq!(updated.SchedPolicyExpr, "2h");
    assert_eq!(updated.EventStatus, SchedEventTrigger);
    assert_eq!(updated.EventID, event_id);
    assert_eq!(
        updated.EventStart.unwrap().timestamp(),
        event_start.timestamp()
    );
    assert_eq!(
        updated.Watermark.unwrap().timestamp(),
        watermark.timestamp()
    );
    assert_eq!(updated.ManualRequest.ManualRequestID, "req1");
    assert_eq!(updated.EventExtra.EventManualRequestID, "req2");

    store
        .Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                Tags: NewOptionalVal(vec!["l3".to_string()]),
                ..TimerUpdate::default()
            }),
        )
        .unwrap();
    assert_eq!(store.GetByID(&ctx, &id).unwrap().Tags, vec!["l3"]);
    store
        .Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                ManualRequest: NewOptionalVal(ManualRequest {
                    ManualRequestID: "req3".to_string(),
                    ..ManualRequest::default()
                }),
                EventExtra: NewOptionalVal(EventExtra {
                    EventManualRequestID: "req4".to_string(),
                    ..EventExtra::default()
                }),
                ..TimerUpdate::default()
            }),
        )
        .unwrap();
    let updated = store.GetByID(&ctx, &id).unwrap();
    assert_eq!(updated.ManualRequest.ManualRequestID, "req3");
    assert_eq!(updated.EventExtra.EventManualRequestID, "req4");

    // Go 契约允许通过 OptionalVal 显式清空可空/零值字段，并恢复默认时区。
    store
        .Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                TimeZone: NewOptionalVal(String::new()),
                Tags: NewOptionalVal(Vec::new()),
                ManualRequest: NewOptionalVal(ManualRequest::default()),
                EventStatus: NewOptionalVal(SchedEventIdle.to_string()),
                EventID: NewOptionalVal(String::new()),
                EventData: NewOptionalVal(Vec::new()),
                EventStart: NewOptionalVal(None),
                EventExtra: NewOptionalVal(EventExtra::default()),
                Watermark: NewOptionalVal(None),
                SummaryData: NewOptionalVal(Vec::new()),
                ..TimerUpdate::default()
            }),
        )
        .unwrap();
    let cleared = store.GetByID(&ctx, &id).unwrap();
    assert!(cleared.TimeZone.is_empty());
    assert!(cleared.Tags.is_empty());
    assert_eq!(cleared.ManualRequest, ManualRequest::default());
    assert_eq!(cleared.EventStatus, SchedEventIdle);
    assert!(cleared.EventID.is_empty());
    assert!(cleared.EventData.is_empty());
    assert_eq!(cleared.EventStart, None);
    assert_eq!(cleared.EventExtra, EventExtra::default());
    assert_eq!(cleared.Watermark, None);
    assert!(cleared.SummaryData.is_empty());

    // 版本与事件 ID 是独立的条件更新护栏，任一不匹配都必须拒绝写入。
    let version = cleared.Version;
    assert_eq!(
        store.Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                SchedPolicyExpr: NewOptionalVal("2h".to_string()),
                CheckVersion: NewOptionalVal(version + 1),
                ..TimerUpdate::default()
            }),
        ),
        Err(ErrVersionNotMatch)
    );
    assert_eq!(store.GetByID(&ctx, &id).unwrap(), cleared);
    assert_eq!(
        store.Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                SchedPolicyExpr: NewOptionalVal("2h".to_string()),
                CheckEventID: NewOptionalVal("wrong".to_string()),
                ..TimerUpdate::default()
            }),
        ),
        Err(ErrEventIDNotMatch)
    );
    assert_eq!(store.GetByID(&ctx, &id).unwrap(), cleared);
    assert_message(
        store.Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                SchedPolicyExpr: NewOptionalVal("2x".to_string()),
                ..TimerUpdate::default()
            }),
        ),
        "schedule event configuration is not valid: invalid schedule event expr '2x': unknown unit x",
    );
    assert_eq!(store.GetByID(&ctx, &id).unwrap(), cleared);
    assert_message(
        store.Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                TimeZone: NewOptionalVal("invalid".to_string()),
                ..TimerUpdate::default()
            }),
        ),
        "Unknown or incorrect time zone: 'invalid'",
    );
    assert_eq!(store.GetByID(&ctx, &id).unwrap(), cleared);
    assert_message(
        store.Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                TimeZone: NewOptionalVal("tidb".to_string()),
                ..TimerUpdate::default()
            }),
        ),
        "Unknown or incorrect time zone: 'tidb'",
    );
    assert_eq!(store.GetByID(&ctx, &id).unwrap(), cleared);

    assert!(store.Delete(&ctx, &id).unwrap());
    assert_eq!(store.GetByID(&ctx, &id), Err(ErrTimerNotExist));
    assert!(!store.Delete(&ctx, &id).unwrap());
    store.Close();
}

#[test]
/// 验证键前缀、标签及布尔组合条件与 Go 版本的筛选语义一致。
fn test_mem_timer_store_list_conditions_match_go() {
    let ctx = Context::background();
    let store = NewMemoryTimerStore();
    for (namespace, key, tags, expr) in [
        ("n1", "/path/to/key1", vec!["tag1"], "1h"),
        ("n1", "/path/to/key2", vec!["tag1", "tag2"], "2h"),
        ("n2", "/path/to/another", vec!["tag2", "tag3"], "3h"),
    ] {
        let mut value = record(namespace, key);
        value.Tags = tags.into_iter().map(str::to_string).collect();
        value.SchedPolicyExpr = expr.to_string();
        store.Create(&ctx, Some(value)).unwrap();
    }

    assert_record_ids(
        &store,
        &ctx,
        &["/path/to/key1", "/path/to/key2", "/path/to/another"],
    );
    let prefix = TimerCond {
        Key: NewOptionalVal("/path/to/k".to_string()),
        KeyPrefix: true,
        ..TimerCond::default()
    };
    assert_eq!(store.List(&ctx, Some(&prefix)).unwrap().len(), 2);
    let exact = TimerCond {
        Key: NewOptionalVal("/path/to/k".to_string()),
        ..TimerCond::default()
    };
    assert!(store.List(&ctx, Some(&exact)).unwrap().is_empty());
    let namespace_key = TimerCond {
        Namespace: NewOptionalVal("n1".to_string()),
        Key: NewOptionalVal("/path/to/key2".to_string()),
        ..TimerCond::default()
    };
    assert_eq!(store.List(&ctx, Some(&namespace_key)).unwrap().len(), 1);
    let tag2 = TimerCond {
        Tags: NewOptionalVal(vec!["tag2".to_string()]),
        ..TimerCond::default()
    };
    assert_eq!(store.List(&ctx, Some(&tag2)).unwrap().len(), 2);
    let tag1_tag3 = TimerCond {
        Tags: NewOptionalVal(vec!["tag1".to_string(), "tag3".to_string()]),
        ..TimerCond::default()
    };
    assert!(store.List(&ctx, Some(&tag1_tag3)).unwrap().is_empty());
    let tag2_tag3 = TimerCond {
        Tags: NewOptionalVal(vec!["tag2".to_string(), "tag3".to_string()]),
        ..TimerCond::default()
    };
    let matched = store.List(&ctx, Some(&tag2_tag3)).unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].Key, "/path/to/another");

    let mismatched_namespace_key = TimerCond {
        Namespace: NewOptionalVal("n2".to_string()),
        Key: NewOptionalVal("/path/to/key2".to_string()),
        ..TimerCond::default()
    };
    assert!(
        store
            .List(&ctx, Some(&mismatched_namespace_key))
            .unwrap()
            .is_empty()
    );

    let and = And(vec![
        Arc::new(TimerCond {
            Namespace: NewOptionalVal("n1".to_string()),
            ..TimerCond::default()
        }),
        Arc::new(tag2.clone()),
    ]);
    assert_eq!(store.List(&ctx, Some(&and)).unwrap().len(), 1);
    let not_and = Not(Arc::new(and));
    assert_eq!(store.List(&ctx, Some(&not_and)).unwrap().len(), 2);
    let or = astersql_timer_api::Or(vec![
        Arc::new(TimerCond {
            Key: NewOptionalVal("/path/to/key2".to_string()),
            ..TimerCond::default()
        }),
        Arc::new(TimerCond {
            Tags: NewOptionalVal(vec!["tag3".to_string()]),
            ..TimerCond::default()
        }),
    ]);
    assert_eq!(store.List(&ctx, Some(&or)).unwrap().len(), 2);
    assert_eq!(store.List(&ctx, Some(&Not(Arc::new(or)))).unwrap().len(), 1);
    store.Close();
}

#[test]
/// 验证存储写操作产生对应事件，取消上下文后监听通道停止接收。
fn test_mem_timer_store_watch_and_context_cleanup() {
    let store = NewMemoryTimerStore();
    let (ctx, cancel) = Context::with_cancel();
    let watcher = store.Watch(&ctx);
    let id = store.Create(&ctx, Some(record("n1", "watch"))).unwrap();
    assert_event(&watcher, WatchTimerEventCreate, &id);
    store
        .Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                SchedPolicyExpr: NewOptionalVal("2h".to_string()),
                ..TimerUpdate::default()
            }),
        )
        .unwrap();
    assert_event(&watcher, WatchTimerEventUpdate, &id);
    assert!(store.Delete(&ctx, &id).unwrap());
    assert_event(&watcher, WatchTimerEventDelete, &id);

    cancel.cancel();
    assert!(watcher.recv_timeout(Duration::from_secs(2)).is_err());
    store.Close();
}

#[test]
/// 验证内存通知器向多个监听者保持广播顺序，并正确处理取消和关闭。
fn test_mem_notifier_broadcast_order_close_and_cancel() {
    let notifier = NewMemTimerWatchEventNotifier();
    let (ctx1, cancel1) = Context::with_cancel();
    let ctx2 = Context::background();
    let watcher1 = notifier.Watch(&ctx1);
    let watcher2 = notifier.Watch(&ctx2);
    // 等待两个后台监听循环完成注册，避免首批广播与注册并发造成偶发漏收。
    std::thread::sleep(Duration::from_millis(30));

    notifier.Notify(WatchTimerEventCreate, "1");
    notifier.Notify(WatchTimerEventCreate, "2");
    notifier.Notify(WatchTimerEventUpdate, "1");
    notifier.Notify(WatchTimerEventDelete, "2");
    for (event_type, timer_id) in [
        (WatchTimerEventCreate, "1"),
        (WatchTimerEventCreate, "2"),
        (WatchTimerEventUpdate, "1"),
        (WatchTimerEventDelete, "2"),
    ] {
        assert_event(&watcher1, event_type, timer_id);
        assert_event(&watcher2, event_type, timer_id);
    }

    // 取消单个监听者不应影响仍然存活的其他监听者。
    cancel1.cancel();
    notifier.Notify(WatchTimerEventCreate, "3");
    assert!(watcher1.recv_timeout(Duration::from_secs(2)).is_err());
    assert_event(&watcher2, WatchTimerEventCreate, "3");

    // 关闭后既要终止现有监听，也不得让新监听收到后续通知。
    notifier.Close();
    assert!(watcher2.recv_timeout(Duration::from_secs(2)).is_err());
    let closed_watcher = notifier.Watch(&Context::background());
    notifier.Notify(WatchTimerEventCreate, "4");
    assert!(
        closed_watcher
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
}

#[test]
/// 验证时区解析以及水位线、事件开始时间在创建和更新后的往返结果。
fn test_mem_timer_store_timezone_and_time_fields() {
    let ctx = Context::background();
    let store = NewMemoryTimerStore();
    let time1 = astersql_timer_api::now_timestamp() - Duration::from_secs(3600);
    let time2 = astersql_timer_api::now_timestamp() - Duration::from_secs(7200);
    let id = store
        .Create(
            &ctx,
            Some(TimerRecord {
                TimerSpec: TimerSpec {
                    Namespace: "default".to_string(),
                    Key: "test1".to_string(),
                    TimeZone: "UTC".to_string(),
                    SchedPolicyType: SchedEventInterval.to_string(),
                    SchedPolicyExpr: "1h".to_string(),
                    Watermark: Some(time1),
                    ..TimerSpec::default()
                },
                EventStatus: SchedEventTrigger.to_string(),
                EventStart: Some(time2),
                ..TimerRecord::default()
            }),
        )
        .unwrap();
    let timer = store.GetByID(&ctx, &id).unwrap();
    assert_eq!(timer.TimeZone, "UTC");
    assert_eq!(timer.Watermark.unwrap().timestamp(), time1.timestamp());
    assert_eq!(timer.EventStart.unwrap().timestamp(), time2.timestamp());
    assert!(matches!(
        timer.Location,
        Some(astersql_timer_api::TimerLocation::Named(_))
    ));

    store
        .Update(
            &ctx,
            &id,
            Some(TimerUpdate {
                Watermark: NewOptionalVal(Some(time2)),
                EventStart: NewOptionalVal(Some(time1)),
                TimeZone: NewOptionalVal("Europe/Berlin".to_string()),
                ..TimerUpdate::default()
            }),
        )
        .unwrap();
    let timer = store.GetByID(&ctx, &id).unwrap();
    assert_eq!(timer.TimeZone, "Europe/Berlin");
    assert_eq!(timer.Watermark.unwrap().timestamp(), time2.timestamp());
    assert_eq!(timer.EventStart.unwrap().timestamp(), time1.timestamp());
    assert!(matches!(
        timer.Location,
        Some(astersql_timer_api::TimerLocation::Named(_))
    ));
    store.Close();
}
