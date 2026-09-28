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

// Aster 侧 Timer 客户端补充单元测试。
//
// 对照 Go 行为验证 Option 构建、客户端生命周期、条件组合、
// 以及手动触发在版本冲突下的重试逻辑。

use crate::*;
use chrono::{Duration, Utc};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration as StdDuration;

/// 构造启用状态的 interval 策略测试规格。
fn interval_spec(key: &str) -> TimerSpec {
    TimerSpec {
        Key: key.to_string(),
        Tags: vec!["l1".to_string(), "l2".to_string()],
        Data: b"data1".to_vec(),
        TimeZone: "Asia/Shanghai".to_string(),
        SchedPolicyType: SchedEventInterval.to_string(),
        SchedPolicyExpr: "1h".to_string(),
        Enable: true,
        ..TimerSpec::default()
    }
}

#[test]
/// 验证查询/更新 Option 与 TimerSpec 校验错误信息与 Go 一致。
fn timer_options_and_validation_match_go() {
    let mut cond = TimerCond::default();
    WithKey("k1".to_string())(&mut cond);
    assert_eq!(cond.Key.Get(), Some(&"k1".to_string()));
    assert!(!cond.KeyPrefix);
    WithKeyPrefix("k".to_string())(&mut cond);
    WithID("id1".to_string())(&mut cond);
    WithTag(vec!["l1".to_string(), "l2".to_string()])(&mut cond);
    assert_eq!(cond.FieldsSet(&[]), vec!["ID", "Key", "Tags"]);

    let mut update = TimerUpdate::default();
    WithSetEnable(false)(&mut update);
    WithSetSchedExpr(SchedEventInterval.to_string(), "3h".to_string())(&mut update);
    WithSetTimeZone("UTC".to_string())(&mut update);
    assert_eq!(
        update.FieldsSet(&[]),
        vec!["Enable", "TimeZone", "SchedPolicyType", "SchedPolicyExpr"]
    );

    let mut spec = TimerSpec::default();
    assert_eq!(
        spec.Validate().unwrap_err().to_string(),
        "field 'Namespace' should not be empty"
    );
    spec.Namespace = "n1".to_string();
    assert_eq!(
        spec.Validate().unwrap_err().to_string(),
        "field 'Key' should not be empty"
    );
    spec.Key = "k1".to_string();
    assert_eq!(
        spec.Validate().unwrap_err().to_string(),
        "field 'SchedPolicyType' should not be empty"
    );
    spec.SchedPolicyType = SchedEventInterval.to_string();
    spec.SchedPolicyExpr = "1x".to_string();
    assert!(
        spec.Validate()
            .unwrap_err()
            .to_string()
            .contains("invalid schedule event expr '1x'")
    );
}

#[test]
/// 覆盖创建、查询、更新、关事件、手动触发、删除与 Watch 关闭的完整生命周期。
fn default_client_lifecycle_matches_go() {
    let ctx = Context::background();
    let store = NewMemoryTimerStore();
    let client = NewDefaultTimerClient(store.clone());
    let watch = store.Watch(&ctx);

    // 创建后应落到默认命名空间，EventStatus 为 Idle。
    let mut timer = client.CreateTimer(&ctx, interval_spec("k1")).unwrap();
    assert_eq!(timer.Namespace, DefaultStoreNamespace);
    assert_eq!(timer.EventStatus, SchedEventIdle);
    assert_eq!(timer.Version, 1);
    let created = watch.recv_timeout(StdDuration::from_secs(1)).unwrap();
    assert_eq!(created.Events[0].Tp, WatchTimerEventCreate);
    assert_eq!(created.Events[0].TimerID, timer.ID);

    assert_eq!(client.GetTimerByID(&ctx, &timer.ID).unwrap(), timer);
    assert_eq!(client.GetTimerByKey(&ctx, "k1").unwrap(), timer);
    assert_eq!(
        client
            .GetTimers(&ctx, vec![WithKeyPrefix("k".to_string())])
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        client
            .GetTimers(&ctx, vec![WithTag(vec!["missing".to_string()])])
            .unwrap()
            .len(),
        0
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
    let updated = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(updated.SchedPolicyExpr, "3h");
    assert!(updated.Version > timer.Version);

    // 模拟调度器写入进行中事件，再测 CloseTimerEvent 约束。
    let event_start = (Utc::now() - Duration::seconds(1)).fixed_offset();
    let mut event_update = TimerUpdate::default();
    event_update.EventStatus.Set(SchedEventTrigger.to_string());
    event_update.EventID.Set("event1".to_string());
    event_update.EventData.Set(b"d1".to_vec());
    event_update.SummaryData.Set(b"s1".to_vec());
    event_update.EventStart.Set(Some(event_start));
    store.Update(&ctx, &timer.ID, Some(event_update)).unwrap();

    assert_eq!(
        client
            .CloseTimerEvent(&ctx, &timer.ID, "event2", vec![])
            .unwrap_err(),
        ErrEventIDNotMatch
    );
    assert!(
        client
            .CloseTimerEvent(
                &ctx,
                &timer.ID,
                "event1",
                vec![WithSetSchedExpr(
                    SchedEventInterval.to_string(),
                    "1h".to_string()
                )],
            )
            .unwrap_err()
            .to_string()
            .contains("not allowed to update when close event")
    );
    client
        .CloseTimerEvent(&ctx, &timer.ID, "event1", vec![])
        .unwrap();
    timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(timer.EventStatus, SchedEventIdle);
    assert!(timer.EventID.is_empty());
    assert!(timer.EventStart.is_none());
    assert_eq!(timer.Watermark, Some(event_start));
    assert_eq!(timer.SummaryData, b"s1".to_vec());

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
    let request_id = client.ManualTriggerEvent(&ctx, &timer.ID).unwrap();
    let timer = client.GetTimerByID(&ctx, &timer.ID).unwrap();
    assert_eq!(timer.ManualRequest.ManualRequestID, request_id);
    assert_eq!(
        timer.ManualRequest.ManualTimeout,
        StdDuration::from_secs(120)
    );
    assert!(timer.ManualRequest.IsManualRequesting());

    assert!(client.DeleteTimer(&ctx, &timer.ID).unwrap());
    assert!(!client.DeleteTimer(&ctx, &timer.ID).unwrap());
    store.Close();
    // Close 后 Watch 通道应断开而非超时。
    loop {
        match watch.recv_timeout(StdDuration::from_secs(1)) {
            Ok(_) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                panic!("watch channel did not close")
            }
        }
    }
}

#[test]
/// 验证下次触发时间、时区校验、And/Or/Not 条件、重复创建与取消 Watch。
fn policy_store_conditions_and_cancellation_match_go() {
    let watermark = Utc::now().fixed_offset();
    let mut record = TimerRecord {
        TimerSpec: TimerSpec {
            Namespace: "n1".to_string(),
            Key: "/path/to/key".to_string(),
            Tags: vec!["tagA1".to_string(), "tagA2".to_string()],
            SchedPolicyType: SchedEventInterval.to_string(),
            SchedPolicyExpr: "1h".to_string(),
            Watermark: Some(watermark),
            Enable: true,
            ..TimerSpec::default()
        },
        ..TimerRecord::default()
    };
    let (next, ok) = record.NextEventTime().unwrap();
    assert!(ok);
    assert_eq!(next.unwrap() - watermark, Duration::hours(1));
    record.Enable = false;
    assert_eq!(record.NextEventTime().unwrap(), (None, false));

    assert!(ValidateTimeZone("+0800").is_ok());
    assert!(ValidateTimeZone("Asia/Shanghai").is_ok());
    assert!(ValidateTimeZone("tidb").is_err());

    let id = "123".to_string();
    record.ID = id.clone();
    let id_cond = Arc::new(TimerCond {
        ID: NewOptionalVal(id),
        ..TimerCond::default()
    });
    let namespace_cond = Arc::new(TimerCond {
        Namespace: NewOptionalVal("n1".to_string()),
        ..TimerCond::default()
    });
    let wrong_cond = Arc::new(TimerCond {
        Namespace: NewOptionalVal("n2".to_string()),
        ..TimerCond::default()
    });
    assert!(And(vec![id_cond.clone(), namespace_cond]).Match(&record));
    assert!(!Or(vec![wrong_cond.clone()]).Match(&record));
    assert!(Not(wrong_cond).Match(&record));

    let store = NewMemoryTimerStore();
    let ctx = Context::background();
    let mut direct_spec = interval_spec("duplicate");
    direct_spec.Namespace = DefaultStoreNamespace.to_string();
    let created = store
        .Create(
            &ctx,
            Some(TimerRecord {
                TimerSpec: direct_spec.clone(),
                ..TimerRecord::default()
            }),
        )
        .unwrap();
    assert_eq!(
        store
            .Create(
                &ctx,
                Some(TimerRecord {
                    TimerSpec: direct_spec,
                    ..TimerRecord::default()
                }),
            )
            .unwrap_err(),
        ErrTimerExists
    );
    let mut stale = TimerUpdate::default();
    stale.CheckVersion.Set(0);
    assert_eq!(
        store.Update(&ctx, &created, Some(stale)).unwrap_err(),
        ErrVersionNotMatch
    );

    // 取消上下文后 Watch 接收端应断开。
    let (watch_ctx, cancel) = Context::with_cancel();
    let watch = store.Watch(&watch_ctx);
    cancel.cancel();
    assert_eq!(
        watch.recv_timeout(StdDuration::from_secs(1)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected)
    );
    store.Close();
}

/// 注入版本冲突的 Store 包装：Update 前先 bump Watermark 制造冲突。
struct InjectedCore {
    inner: TimerStore,
    conflicts: Arc<AtomicI32>,
}

impl TimerStoreCore for InjectedCore {
    fn Create(&self, ctx: &Context, record: Option<TimerRecord>) -> TimerResult<String> {
        self.inner.Create(ctx, record)
    }

    fn List(&self, ctx: &Context, cond: Option<&dyn Cond>) -> TimerResult<Vec<TimerRecord>> {
        self.inner.List(ctx, cond)
    }

    // conflicts>0 递减并先写一次；conflicts<0 则每次都冲突。
    fn Update(&self, ctx: &Context, timerID: &str, update: Option<TimerUpdate>) -> TimerResult<()> {
        let remaining = self.conflicts.load(Ordering::SeqCst);
        if remaining != 0 {
            if remaining > 0 {
                self.conflicts.fetch_sub(1, Ordering::SeqCst);
            }
            let mut bump = TimerUpdate::default();
            bump.Watermark.Set(Some(now_timestamp()));
            self.inner.Update(ctx, timerID, Some(bump))?;
        }
        self.inner.Update(ctx, timerID, update)
    }

    fn Delete(&self, ctx: &Context, timerID: &str) -> TimerResult<bool> {
        self.inner.Delete(ctx, timerID)
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
/// 手动触发在有限冲突后成功；持续冲突则返回 ErrVersionNotMatch。
fn manual_trigger_retries_version_conflicts_like_go() {
    let inner = NewMemoryTimerStore();
    let conflicts = Arc::new(AtomicI32::new(2));
    let store = TimerStore::from_core(InjectedCore {
        inner,
        conflicts: Arc::clone(&conflicts),
    });
    let mut client = NewDefaultTimerClient(store.clone());
    client.retryBackoff = 0;
    let ctx = Context::background();
    let timer = client.CreateTimer(&ctx, interval_spec("retry")).unwrap();

    assert!(
        !client
            .ManualTriggerEvent(&ctx, &timer.ID)
            .unwrap()
            .is_empty()
    );
    assert_eq!(conflicts.load(Ordering::SeqCst), 0);

    conflicts.store(-1, Ordering::SeqCst);
    assert_eq!(
        client.ManualTriggerEvent(&ctx, &timer.ID).unwrap_err(),
        ErrVersionNotMatch
    );
    store.Close();
}
