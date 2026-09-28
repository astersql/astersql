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

// Timer 存储层辅助类型的单元测试。
//
// 覆盖 OptionalVal、TimerCond / TimerUpdate 字段反射、条件匹配（含前缀与标签）、
// And/Or/Not 组合条件，以及 TimerUpdate.apply 的版本/事件 ID 校验与字段写入。

use super::*;
use chrono::{Duration, TimeZone, Utc};
use std::sync::Arc;
use std::time::Duration as StdDuration;

/// 测试用简单值类型，验证 OptionalVal 嵌套 Option 的 Present/Get 语义。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Foo {
    v: i32,
}

/// 验证 OptionalVal 的 Set/Get/Clear/Present，含 Option 嵌套场景。
#[test]
fn test_field_optional() {
    let mut opt1: OptionalVal<String> = OptionalVal::default();
    assert!(!opt1.Present());
    assert_eq!(opt1.Get(), None);
    opt1.Set("a1".to_string());
    assert!(opt1.Present());
    assert_eq!(opt1.Get(), Some(&"a1".to_string()));
    opt1.Set("a2".to_string());
    assert_eq!(opt1.Get(), Some(&"a2".to_string()));
    opt1.Clear();
    assert!(!opt1.Present());
    assert_eq!(opt1.Get(), None);

    let mut opt2: OptionalVal<Option<Foo>> = OptionalVal::default();
    let foo = Foo { v: 1 };
    assert_eq!(opt2.Get(), None);
    opt2.Set(Some(foo));
    assert!(opt2.Present());
    assert_eq!(opt2.Get(), Some(&Some(foo)));
    opt2.Set(None);
    assert!(opt2.Present());
    assert_eq!(opt2.Get(), Some(&None));
    opt2.Clear();
    assert!(!opt2.Present());
    assert_eq!(opt2.Get(), None);
}

/// 验证 TimerCond / TimerUpdate 的 FieldsSet 与 Clear 反射行为。
#[test]
fn test_fields_reflect() {
    let mut cond = TimerCond::default();
    assert!(cond.FieldsSet(&[]).is_empty());
    cond.Key.Set("k1".to_string());
    assert_eq!(cond.FieldsSet(&[]), vec!["Key"]);
    cond.ID.Set("22".to_string());
    assert_eq!(cond.FieldsSet(&[]), vec!["ID", "Key"]);
    assert_eq!(cond.FieldsSet(&["ID"]), vec!["Key"]);
    cond.Key.Clear();
    assert_eq!(cond.FieldsSet(&[]), vec!["ID"]);
    cond.KeyPrefix = true;
    cond.Clear();
    assert!(cond.FieldsSet(&[]).is_empty());
    assert!(!cond.KeyPrefix);

    let mut update = TimerUpdate::default();
    assert!(update.FieldsSet(&[]).is_empty());
    update.Watermark.Set(Some(Utc::now().fixed_offset()));
    assert_eq!(update.FieldsSet(&[]), vec!["Watermark"]);
    update.Enable.Set(true);
    assert_eq!(update.FieldsSet(&[]), vec!["Enable", "Watermark"]);
    assert_eq!(update.FieldsSet(&["Enable"]), vec!["Watermark"]);
    update.Watermark.Clear();
    assert_eq!(update.FieldsSet(&[]), vec!["Enable"]);
    update.Clear();
    assert!(update.FieldsSet(&[]).is_empty());
}

/// 构造带可选 ID/Namespace/Key/Tags 的 TimerCond 测试夹具。
fn timer_cond(
    id: Option<&str>,
    namespace: Option<&str>,
    key: Option<&str>,
    key_prefix: bool,
    tags: Option<Vec<&str>>,
) -> TimerCond {
    TimerCond {
        ID: id
            .map(|value| NewOptionalVal(value.to_string()))
            .unwrap_or_default(),
        Namespace: namespace
            .map(|value| NewOptionalVal(value.to_string()))
            .unwrap_or_default(),
        Key: key
            .map(|value| NewOptionalVal(value.to_string()))
            .unwrap_or_default(),
        KeyPrefix: key_prefix,
        Tags: tags
            .map(|values| NewOptionalVal(values.into_iter().map(str::to_string).collect()))
            .unwrap_or_default(),
    }
}

/// 验证单条件 Match 与 And 组合对 TimerRecord 的匹配结果。
#[test]
fn test_timer_record_cond() {
    let timer = TimerRecord {
        ID: "123".to_string(),
        TimerSpec: TimerSpec {
            Namespace: "n1".to_string(),
            Key: "/path/to/key".to_string(),
            Tags: vec!["tagA1".to_string(), "tagA2".to_string()],
            ..TimerSpec::default()
        },
        ..TimerRecord::default()
    };
    let cases = vec![
        (timer_cond(Some("123"), None, None, false, None), true),
        (timer_cond(Some("1"), None, None, false, None), false),
        (timer_cond(None, Some("n1"), None, false, None), true),
        (timer_cond(None, Some("n2"), None, false, None), false),
        (
            timer_cond(None, None, Some("/path/to/key"), false, None),
            true,
        ),
        (
            timer_cond(None, None, Some("/path/to/"), false, None),
            false,
        ),
        (timer_cond(None, None, Some("/path/to/"), true, None), true),
        (timer_cond(None, None, Some("/path/to2"), true, None), false),
        (timer_cond(None, None, None, false, Some(vec![])), true),
        (
            timer_cond(None, None, None, false, Some(vec!["tagA"])),
            false,
        ),
        (
            timer_cond(None, None, None, false, Some(vec!["tagA1"])),
            true,
        ),
        (
            timer_cond(None, None, None, false, Some(vec!["tagA1", "tagA2"])),
            true,
        ),
        (
            timer_cond(None, None, None, false, Some(vec!["tagA1", "tagB1"])),
            false,
        ),
    ];
    for (condition, expected) in cases {
        assert_eq!(condition.Match(&timer), expected);
    }

    let both_match = And(vec![
        Arc::new(timer_cond(Some("123"), None, None, false, None)),
        Arc::new(timer_cond(None, None, Some("/path/to/key"), false, None)),
    ]);
    assert!(both_match.Match(&timer));
    let key_mismatch = And(vec![
        Arc::new(timer_cond(Some("123"), None, None, false, None)),
        Arc::new(timer_cond(None, None, Some("/path/to/"), false, None)),
    ]);
    assert!(!key_mismatch.Match(&timer));
}

/// 将 TimerCond 装箱为 Arc<dyn Cond>，供组合条件测试复用。
fn boxed(condition: TimerCond) -> Arc<dyn Cond> {
    Arc::new(condition)
}

/// 验证 And / Or / Not 逻辑组合条件的真值表。
#[test]
fn test_operator_cond() {
    let timer = TimerRecord {
        ID: "123".to_string(),
        TimerSpec: TimerSpec {
            Namespace: "n1".to_string(),
            Key: "/path/to/key".to_string(),
            ..TimerSpec::default()
        },
        ..TimerRecord::default()
    };
    let c1 = boxed(timer_cond(Some("123"), None, None, false, None));
    let c2 = boxed(timer_cond(Some("456"), None, None, false, None));
    let c3 = boxed(timer_cond(None, Some("n1"), None, false, None));
    let c4 = boxed(timer_cond(None, Some("n2"), None, false, None));

    assert!(And(vec![c1.clone(), c3.clone()]).Match(&timer));
    assert!(!And(vec![c1.clone(), c2.clone(), c3.clone()]).Match(&timer));
    assert!(!Or(vec![c2.clone(), c4.clone()]).Match(&timer));
    assert!(Or(vec![c2.clone(), c1.clone(), c4.clone()]).Match(&timer));
    assert!(!Not(Arc::new(And(vec![c1.clone(), c3.clone()]))).Match(&timer));
    assert!(Not(Arc::new(And(vec![c1.clone(), c2.clone(), c3.clone()]))).Match(&timer));
    assert!(Not(Arc::new(Or(vec![c2.clone(), c4.clone()]))).Match(&timer));
    assert!(!Not(Arc::new(Or(vec![c2.clone(), c1.clone(), c4.clone()]))).Match(&timer));
    assert!(!Not(c1).Match(&timer));
    assert!(Not(c2).Match(&timer));
}

/// 验证 TimerUpdate.apply：版本/事件 ID CAS 失败，以及批量字段写入与手动请求状态。
#[test]
fn test_timer_update() {
    let mut template = TimerRecord {
        ID: "123".to_string(),
        TimerSpec: TimerSpec {
            Namespace: "n1".to_string(),
            Key: "/path/to/key".to_string(),
            ..TimerSpec::default()
        },
        Version: 567,
        ..TimerRecord::default()
    };
    let mut timer = template.clone();

    // CheckVersion 与记录 Version 不一致时应拒绝更新。
    let mut update = TimerUpdate::default();
    update.Enable.Set(true);
    update.CheckVersion.Set(0);
    assert_eq!(update.apply(&timer).unwrap_err(), ErrVersionNotMatch);
    assert_eq!(timer, template);

    // CheckEventID 与记录 EventID 不一致时应拒绝更新。
    let mut update = TimerUpdate::default();
    update.Enable.Set(true);
    update.CheckEventID.Set("aa".to_string());
    assert_eq!(update.apply(&timer).unwrap_err(), ErrEventIDNotMatch);
    assert_eq!(timer, template);

    let now = Utc::now().fixed_offset();
    let request_time = Utc.timestamp_opt(123, 0).single().unwrap().fixed_offset();
    let event_watermark = Utc.timestamp_opt(456, 0).single().unwrap().fixed_offset();
    let manual_request = ManualRequest {
        ManualRequestID: "req1".to_string(),
        ManualRequestTime: Some(request_time),
        ManualTimeout: StdDuration::from_secs(60),
        ManualProcessed: true,
        ManualEventID: "event1".to_string(),
    };
    let event_extra = EventExtra {
        EventManualRequestID: "req".to_string(),
        EventWatermark: Some(event_watermark),
    };
    // 写入全部可选字段，校验 apply 结果且原 timer 未被原地修改。
    let mut update = TimerUpdate::default();
    update.Enable.Set(true);
    update.TimeZone.Set("UTC".to_string());
    update.SchedPolicyType.Set(SchedEventInterval.to_string());
    update.SchedPolicyExpr.Set("5h".to_string());
    update.Watermark.Set(Some(now));
    update.SummaryData.Set(b"summarydata1".to_vec());
    update.EventStatus.Set(SchedEventTrigger.to_string());
    update.EventID.Set("event1".to_string());
    update.EventData.Set(b"eventdata1".to_vec());
    update.EventStart.Set(Some(now + Duration::seconds(1)));
    update.Tags.Set(vec!["l1".to_string(), "l2".to_string()]);
    update.ManualRequest.Set(manual_request.clone());
    update.EventExtra.Set(event_extra.clone());
    assert_eq!(update.FieldsSet(&[]).len(), 13);

    let record = update.apply(&timer).unwrap();
    assert!(record.Enable);
    assert_eq!(record.TimeZone, "UTC");
    assert_eq!(record.Location, Some(parse_location("UTC").unwrap()));
    assert_eq!(record.SchedPolicyType, SchedEventInterval);
    assert_eq!(record.SchedPolicyExpr, "5h");
    assert_eq!(record.Watermark, Some(now));
    assert_eq!(record.SummaryData, b"summarydata1".to_vec());
    assert_eq!(record.EventStatus, SchedEventTrigger);
    assert_eq!(record.EventID, "event1");
    assert_eq!(record.EventData, b"eventdata1".to_vec());
    assert_eq!(record.EventStart, Some(now + Duration::seconds(1)));
    assert_eq!(record.Tags, vec!["l1", "l2"]);
    assert_eq!(record.ManualRequest, manual_request);
    assert!(!record.ManualRequest.IsManualRequesting());
    assert_eq!(record.EventExtra, event_extra);
    assert_eq!(timer, template);

    // 未 processed 的手动请求应使 IsManualRequesting 为 true。
    template = record.clone();
    timer = template.clone();
    let manual_request = ManualRequest {
        ManualRequestID: "req2".to_string(),
        ManualRequestTime: Some(Utc.timestamp_opt(789, 0).single().unwrap().fixed_offset()),
        ManualTimeout: StdDuration::from_secs(60),
        ..ManualRequest::default()
    };
    let event_extra = EventExtra {
        EventManualRequestID: "req2".to_string(),
        ..EventExtra::default()
    };
    let mut update = TimerUpdate::default();
    update.ManualRequest.Set(manual_request.clone());
    update.EventExtra.Set(event_extra.clone());
    let record = update.apply(&timer).unwrap();
    assert_eq!(record.ManualRequest, manual_request);
    assert!(record.ManualRequest.IsManualRequesting());
    assert_eq!(record.EventExtra, event_extra);
    assert_eq!(timer, template);

    // 清空手动请求与 EventExtra 后应回到非 requesting 状态。
    template = record.clone();
    timer = template.clone();
    let mut update = TimerUpdate::default();
    update.ManualRequest.Set(ManualRequest::default());
    update.EventExtra.Set(EventExtra::default());
    let record = update.apply(&timer).unwrap();
    assert_eq!(record.ManualRequest, ManualRequest::default());
    assert!(!record.ManualRequest.IsManualRequesting());
    assert_eq!(record.EventExtra, EventExtra::default());
    assert_eq!(timer, template);

    assert_eq!(TimerUpdate::default().apply(&timer).unwrap(), template);
}
