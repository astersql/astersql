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

// Timer 客户端行为测试（对应 Go client_test）。
//
// 覆盖 Get/Update Option、默认客户端 CRUD 与关事件约束，
// 以及手动触发在注入冲突下的重试与失败路径。

use super::*;
use chrono::{Duration, TimeZone, Utc};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::time::Duration as StdDuration;

/// 构造带标签与时区的 interval 测试规格（默认未显式 Enable）。
fn interval_spec(key: &str) -> TimerSpec {
    TimerSpec {
        Key: key.to_string(),
        SchedPolicyType: SchedEventInterval.to_string(),
        SchedPolicyExpr: "1h".to_string(),
        TimeZone: "Asia/Shanghai".to_string(),
        Data: b"data1".to_vec(),
        Tags: vec!["l1".to_string(), "l2".to_string()],
        ..TimerSpec::default()
    }
}

#[test]
/// 验证 WithKey/WithKeyPrefix/WithID/WithTag 对 TimerCond 的副作用。
fn test_get_timer_option() {
    let mut cond = TimerCond::default();
    assert!(cond.FieldsSet(&[]).is_empty());
    assert!(!cond.Key.Present());

    WithKey("k1".to_string())(&mut cond);
    assert_eq!(cond.Key.Get(), Some(&"k1".to_string()));
    assert!(!cond.KeyPrefix);
    assert_eq!(cond.FieldsSet(&[]), vec!["Key"]);

    WithKeyPrefix("k2".to_string())(&mut cond);
    assert_eq!(cond.Key.Get(), Some(&"k2".to_string()));
    assert!(cond.KeyPrefix);
    assert_eq!(cond.FieldsSet(&[]), vec!["Key"]);

    WithKey("k3".to_string())(&mut cond);
    assert_eq!(cond.Key.Get(), Some(&"k3".to_string()));
    assert!(!cond.KeyPrefix);
    assert_eq!(cond.FieldsSet(&[]), vec!["Key"]);

    assert!(!cond.ID.Present());
    WithID("id1".to_string())(&mut cond);
    assert_eq!(cond.ID.Get(), Some(&"id1".to_string()));
    assert_eq!(cond.FieldsSet(&[]), vec!["ID", "Key"]);

    assert!(!cond.Tags.Present());
    WithTag(vec!["l1".to_string(), "l2".to_string()])(&mut cond);
    assert_eq!(
        cond.Tags.Get(),
        Some(&vec!["l1".to_string(), "l2".to_string()])
    );
    assert_eq!(cond.FieldsSet(&[]), vec!["ID", "Key", "Tags"]);
}

#[test]
/// 验证各 WithSet* Option 设置字段及 FieldsSet 顺序。
fn test_update_timer_option() {
    let mut update = TimerUpdate::default();
    assert_eq!(update, TimerUpdate::default());
    assert!(!update.Enable.Present());

    WithSetEnable(true)(&mut update);
    assert_eq!(update.Enable.Get(), Some(&true));
    assert_eq!(update.FieldsSet(&[]), vec!["Enable"]);
    WithSetEnable(false)(&mut update);
    assert_eq!(update.Enable.Get(), Some(&false));
    assert_eq!(update.FieldsSet(&[]), vec!["Enable"]);

    assert!(!update.SchedPolicyType.Present());
    assert!(!update.SchedPolicyExpr.Present());
    WithSetSchedExpr(SchedEventInterval.to_string(), "3h".to_string())(&mut update);
    assert_eq!(
        update.SchedPolicyType.Get(),
        Some(&SchedEventInterval.to_string())
    );
    assert_eq!(update.SchedPolicyExpr.Get(), Some(&"3h".to_string()));
    assert_eq!(
        update.FieldsSet(&[]),
        vec!["Enable", "SchedPolicyType", "SchedPolicyExpr"]
    );
    WithSetSchedExpr(SchedEventInterval.to_string(), "1h".to_string())(&mut update);
    assert_eq!(update.SchedPolicyExpr.Get(), Some(&"1h".to_string()));

    assert!(!update.Watermark.Present());
    let watermark = Utc
        .timestamp_opt(1234, 5678)
        .single()
        .unwrap()
        .fixed_offset();
    WithSetWatermark(watermark)(&mut update);
    assert_eq!(update.Watermark.Get(), Some(&Some(watermark)));
    assert_eq!(
        update.FieldsSet(&[]),
        vec!["Enable", "SchedPolicyType", "SchedPolicyExpr", "Watermark"]
    );

    assert!(!update.SummaryData.Present());
    WithSetSummaryData(b"hello".to_vec())(&mut update);
    assert_eq!(update.SummaryData.Get(), Some(&b"hello".to_vec()));
    assert_eq!(
        update.FieldsSet(&[]),
        vec![
            "Enable",
            "SchedPolicyType",
            "SchedPolicyExpr",
            "Watermark",
            "SummaryData",
        ]
    );

    assert!(!update.Tags.Present());
    WithSetTags(vec![])(&mut update);
    assert_eq!(update.Tags.Get(), Some(&vec![]));
    WithSetTags(vec!["l1".to_string(), "l2".to_string()])(&mut update);
    assert_eq!(
        update.Tags.Get(),
        Some(&vec!["l1".to_string(), "l2".to_string()])
    );
    assert_eq!(
        update.FieldsSet(&[]),
        vec![
            "Tags",
            "Enable",
            "SchedPolicyType",
            "SchedPolicyExpr",
            "Watermark",
            "SummaryData",
        ]
    );

    assert!(!update.TimeZone.Present());
    WithSetTimeZone("UTC".to_string())(&mut update);
    assert_eq!(update.TimeZone.Get(), Some(&"UTC".to_string()));
    assert_eq!(
        update.FieldsSet(&[]),
        vec![
            "Tags",
            "Enable",
            "TimeZone",
            "SchedPolicyType",
            "SchedPolicyExpr",
            "Watermark",
            "SummaryData",
        ]
    );
}

#[test]
/// 端到端验证默认客户端：创建、过滤查询、更新、关事件、手动触发与删除。
fn test_default_client() {
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let ctx = Context::background();
    let mut spec = interval_spec("k1");

    // 创建后 Namespace 应由客户端填为 default。
    let mut timer = client.CreateTimer(&ctx, spec.clone()).unwrap();
    spec.Namespace = DefaultStoreNamespace.to_string();
    assert!(!timer.ID.is_empty());
    assert_eq!(timer.TimerSpec, spec);
    assert_eq!(timer.TimeZone, "Asia/Shanghai");
    assert_eq!(timer.EventStatus, SchedEventIdle);
    assert!(timer.EventID.is_empty());
    assert!(timer.EventData.is_empty());
    assert!(timer.SummaryData.is_empty());

    assert_eq!(client.GetTimerByID(&ctx, &timer.ID).unwrap(), timer);
    assert_eq!(client.GetTimerByKey(&ctx, &timer.Key).unwrap(), timer);
    assert_eq!(
        client
            .GetTimers(&ctx, vec![WithKeyPrefix("k".to_string())])
            .unwrap(),
        vec![timer.clone()]
    );
    assert_eq!(
        client
            .GetTimers(&ctx, vec![WithTag(vec!["l1".to_string()])])
            .unwrap(),
        vec![timer.clone()]
    );
    assert_eq!(
        client
            .GetTimers(
                &ctx,
                vec![WithTag(vec!["l1".to_string(), "l2".to_string()])],
            )
            .unwrap(),
        vec![timer.clone()]
    );
    assert!(
        client
            .GetTimers(&ctx, vec![WithTag(vec!["l3".to_string()])])
            .unwrap()
            .is_empty()
    );

    client
        .UpdateTimer(
            &ctx,
            &timer.ID,
            vec![WithSetSchedExpr(
                SchedEventInterval.to_string(),
                "3h".to_string(),
            )],
        )
        .unwrap();
    timer.SchedPolicyExpr = "3h".to_string();
    let got = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert!(got.Version > timer.Version);
    timer.Version = got.Version;
    assert_eq!(got, timer);

    // 写入进行中事件，校验 EventID 不匹配与非法关事件字段。
    let event_start = (Utc::now() - Duration::seconds(1)).fixed_offset();
    let mut update = TimerUpdate::default();
    update.EventStatus.Set(SchedEventTrigger.to_string());
    update.EventID.Set("event1".to_string());
    update.EventData.Set(b"d1".to_vec());
    update.SummaryData.Set(b"s1".to_vec());
    update.EventStart.Set(Some(event_start));
    update.EventExtra.Set(EventExtra {
        EventManualRequestID: "req1".to_string(),
        EventWatermark: Some(Utc.timestamp_opt(456, 0).single().unwrap().fixed_offset()),
    });
    store.Update(&ctx, &timer.ID, Some(update)).unwrap();
    assert_eq!(
        client
            .CloseTimerEvent(&ctx, &timer.ID, "event2", vec![])
            .unwrap_err(),
        ErrEventIDNotMatch
    );
    assert_eq!(
        client
            .CloseTimerEvent(
                &ctx,
                &timer.ID,
                "event2",
                vec![WithSetSchedExpr(
                    SchedEventInterval.to_string(),
                    "1h".to_string(),
                )],
            )
            .unwrap_err()
            .to_string(),
        "The field(s) [SchedPolicyType, SchedPolicyExpr] are not allowed to update when close event"
    );

    client
        .CloseTimerEvent(&ctx, &timer.ID, "event1", vec![])
        .unwrap();
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(timer.EventStatus, SchedEventIdle);
    assert!(timer.EventID.is_empty());
    assert!(timer.EventData.is_empty());
    assert!(timer.EventStart.is_none());
    assert_eq!(timer.SummaryData, b"s1".to_vec());
    assert_eq!(
        timer.Watermark.unwrap().timestamp(),
        event_start.timestamp()
    );
    assert_eq!(timer.EventExtra, EventExtra::default());

    let mut update = TimerUpdate::default();
    update.EventID.Set("event1".to_string());
    update.EventData.Set(b"d1".to_vec());
    update.SummaryData.Set(b"s1".to_vec());
    store.Update(&ctx, &timer.ID, Some(update)).unwrap();
    // 关事件时可显式更新 Watermark 与 SummaryData。
    let watermark = (Utc::now() + Duration::hours(1)).fixed_offset();
    client
        .CloseTimerEvent(
            &ctx,
            &timer.ID,
            "event1",
            vec![
                WithSetWatermark(watermark),
                WithSetSummaryData(b"s2".to_vec()),
            ],
        )
        .unwrap();
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(timer.EventStatus, SchedEventIdle);
    assert!(timer.EventID.is_empty());
    assert!(timer.EventData.is_empty());
    assert!(timer.EventStart.is_none());
    assert_eq!(timer.SummaryData, b"s2".to_vec());
    assert_eq!(timer.Watermark.unwrap().timestamp(), watermark.timestamp());

    let mut update = TimerUpdate::default();
    update.EventID.Set("event1".to_string());
    update.EventData.Set(b"d1".to_vec());
    update.SummaryData.Set(b"s1".to_vec());
    store.Update(&ctx, &timer.ID, Some(update)).unwrap();
    assert_eq!(
        client
            .ManualTriggerEvent(&ctx, &timer.ID)
            .unwrap_err()
            .to_string(),
        // 事件未关闭时禁止手动触发。
        "manual trigger is not allowed when event is not closed"
    );

    client
        .CloseTimerEvent(&ctx, &timer.ID, "event1", vec![])
        .unwrap();
    client
        .UpdateTimer(&ctx, &timer.ID, vec![WithSetEnable(false)])
        .unwrap();
    assert_eq!(
        client
            .ManualTriggerEvent(&ctx, &timer.ID)
            .unwrap_err()
            .to_string(),
        "manual trigger is not allowed when timer is disabled"
    );

    client
        .UpdateTimer(&ctx, &timer.ID, vec![WithSetEnable(true)])
        .unwrap();
    let now = Utc::now().fixed_offset();
    let request_id = client.ManualTriggerEvent(&ctx, &timer.ID).unwrap();
    assert!(!request_id.is_empty());
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert!(!timer.ManualRequest.ManualRequestID.is_empty());
    let request_time = timer.ManualRequest.ManualRequestTime.unwrap();
    assert!(request_time.timestamp() >= now.timestamp());
    assert!(request_time - now < Duration::seconds(10));
    assert_eq!(timer.ManualRequest.ManualRequestID, request_id);
    assert_eq!(
        timer.ManualRequest.ManualTimeout,
        StdDuration::from_secs(120)
    );

    let manual_request = timer.ManualRequest.SetProcessed("event1".to_string());
    let mut update = TimerUpdate::default();
    update.ManualRequest.Set(manual_request.clone());
    update.EventExtra.Set(EventExtra {
        EventManualRequestID: manual_request.ManualRequestID.clone(),
        EventWatermark: timer.Watermark,
    });
    update.EventID.Set("event1".to_string());
    update.EventStart.Set(Some(Utc::now().fixed_offset()));
    update.EventStatus.Set(SchedEventTrigger.to_string());
    store.Update(&ctx, &timer.ID, Some(update)).unwrap();
    client
        .CloseTimerEvent(&ctx, &timer.ID, "event1", vec![])
        .unwrap();
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(timer.ManualRequest, manual_request);
    assert_eq!(timer.EventExtra, EventExtra::default());

    assert!(client.DeleteTimer(&ctx, &timer.ID).unwrap());
    assert!(!client.DeleteTimer(&ctx, &timer.ID).unwrap());
    store.Close();
}

/// 控制注入冲突模式与 Update 调用计数。
struct RetryState {
    mode: AtomicI32,
    calls: AtomicUsize,
}

/// 在真正 Update 前按 mode 注入并发写，模拟版本冲突场景。
struct InjectedTimerStore {
    inner: TimerStore,
    state: Arc<RetryState>,
}

impl InjectedTimerStore {
    /// mode=0：前两次 bump；-1：每次 bump；-2：前两次 bump，第三次禁用。
    fn mutate_before_update(&self, ctx: &Context, timer_id: &str) -> TimerResult<()> {
        let call = self.state.calls.fetch_add(1, Ordering::SeqCst) + 1;
        match self.state.mode.load(Ordering::SeqCst) {
            0 if call < 3 => {
                let mut update = TimerUpdate::default();
                update.Watermark.Set(Some(Utc::now().fixed_offset()));
                self.inner.Update(ctx, timer_id, Some(update))?;
            }
            -1 => {
                let mut update = TimerUpdate::default();
                update.Watermark.Set(Some(Utc::now().fixed_offset()));
                self.inner.Update(ctx, timer_id, Some(update))?;
            }
            -2 if call < 3 => {
                let mut update = TimerUpdate::default();
                update.Watermark.Set(Some(Utc::now().fixed_offset()));
                self.inner.Update(ctx, timer_id, Some(update))?;
            }
            -2 if call == 3 => {
                let mut update = TimerUpdate::default();
                update.Enable.Set(false);
                self.inner.Update(ctx, timer_id, Some(update))?;
            }
            _ => {}
        }
        Ok(())
    }
}

impl TimerStoreCore for InjectedTimerStore {
    fn Create(&self, ctx: &Context, record: Option<TimerRecord>) -> TimerResult<String> {
        self.inner.Create(ctx, record)
    }

    fn List(&self, ctx: &Context, cond: Option<&dyn Cond>) -> TimerResult<Vec<TimerRecord>> {
        self.inner.List(ctx, cond)
    }

    fn Update(
        &self,
        ctx: &Context,
        timer_id: &str,
        update: Option<TimerUpdate>,
    ) -> TimerResult<()> {
        self.mutate_before_update(ctx, timer_id)?;
        self.inner.Update(ctx, timer_id, update)
    }

    fn Delete(&self, ctx: &Context, timer_id: &str) -> TimerResult<bool> {
        self.inner.Delete(ctx, timer_id)
    }

    fn WatchSupported(&self) -> bool {
        self.inner.WatchSupported()
    }

    fn Watch(&self, ctx: &Context) -> WatchTimerChan {
        self.inner.Watch(ctx)
    }

    fn Close(&self) {
        self.inner.Close();
    }
}

#[test]
/// 验证手动触发重试次数、耗尽返回版本错误、中途禁用返回业务错误。
fn test_default_client_manual_trigger_retry() {
    let inner = NewMemoryTimerStore();
    let state = Arc::new(RetryState {
        mode: AtomicI32::new(0),
        calls: AtomicUsize::new(0),
    });
    let store = TimerStore::from_core(InjectedTimerStore {
        inner,
        state: Arc::clone(&state),
    });
    let mut client = NewDefaultTimerClient(store.clone());
    client.retryBackoff = 0;
    let ctx = Context::background();
    let mut spec = interval_spec("k1");
    spec.Enable = true;
    let timer = client.CreateTimer(&ctx, spec).unwrap();

    let request_id = client.ManualTriggerEvent(&ctx, &timer.ID).unwrap();
    assert!(!request_id.is_empty());
    assert_eq!(state.calls.load(Ordering::SeqCst), 3);

    state.mode.store(-1, Ordering::SeqCst);
    state.calls.store(0, Ordering::SeqCst);
    assert_eq!(
        client.ManualTriggerEvent(&ctx, &timer.ID).unwrap_err(),
        ErrVersionNotMatch
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), clientMaxRetry);

    state.mode.store(-2, Ordering::SeqCst);
    state.calls.store(0, Ordering::SeqCst);
    assert_eq!(
        client
            .ManualTriggerEvent(&ctx, &timer.ID)
            .unwrap_err()
            .to_string(),
        "manual trigger is not allowed when timer is disabled"
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 3);
    store.Close();
}
