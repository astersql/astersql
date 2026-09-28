// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `util/context` 迁移补充单元测试。
//
// 覆盖跨线程唯一上下文 ID、StaticWarnHandler 操作、SQLWarn JSON 往返，
// 以及 PlanCacheTracker / RangeFallbackHandler 与 Go 分支语义对齐。

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use util_context::context::{GenContextID, contextIDGenerator};
use util_context::errors;
use util_context::plancache::{NewPlanCacheTracker, NewRangeFallbackHandler, PlanCacheType};
use util_context::warn::{
    NewFuncWarnAppenderForTest, NewStaticWarnHandler, NewStaticWarnHandlerWithHandler, SQLWarn,
    WarnAppender, WarnHandler, WarnHandlerExt, WarnLevelError, WarnLevelNote, WarnLevelWarning,
};

static CONTEXT_ID_TEST_LOCK: Mutex<()> = Mutex::new(());

fn messages(handler: &impl WarnHandlerExt) -> Vec<(String, String)> {
    // 提取 handler 中全部 warning 的 (Level, 错误文案) 对，便于断言。
    handler
        .GetWarnings()
        .into_iter()
        .map(|warn| (warn.Level, warn.Err.expect("warning error").to_string()))
        .collect()
}

#[test]
/// 多线程并发调用 `GenContextID` 应得到互不相同的正数 ID。
fn migration_context_ids_are_unique_across_threads() {
    let _guard = CONTEXT_ID_TEST_LOCK.lock().unwrap();
    let mut workers = Vec::new();
    for _ in 0..8 {
        workers.push(std::thread::spawn(|| {
            (0..128).map(|_| GenContextID()).collect::<Vec<_>>()
        }));
    }
    let ids: Vec<_> = workers
        .into_iter()
        .flat_map(|w| w.join().unwrap())
        .collect();
    assert_eq!(ids.len(), 1024);
    assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), 1024);
    assert!(ids.iter().all(|id| *id > 0));
}

#[test]
/// Go atomic.Uint64.Add 在最大值后回绕为 0，Rust 实现也不能在 debug 构建中 panic。
fn migration_context_id_wraps_like_go_atomic_uint64() {
    let _guard = CONTEXT_ID_TEST_LOCK.lock().unwrap();
    let previous = contextIDGenerator.swap(u64::MAX, Ordering::SeqCst);
    assert_eq!(GenContextID(), 0);
    contextIDGenerator.store(previous, Ordering::SeqCst);
}

#[test]
/// StaticWarnHandler：追加 Warning/Note/Error、计数、复制、截断、克隆与 Reset。
fn migration_static_warning_handler_matches_go_operations() {
    let handler = NewStaticWarnHandler(1);
    handler.AppendWarning(errors::NewNoStackError("warn0"));
    WarnHandlerExt::AppendNote(&handler, errors::NewNoStackError("note1"));
    handler.AppendError(errors::NewNoStackError("error2"));
    assert_eq!(handler.WarningCount(), 3);
    assert_eq!(
        messages(&handler),
        vec![
            (WarnLevelWarning.into(), "warn0".into()),
            (WarnLevelNote.into(), "note1".into()),
            (WarnLevelError.into(), "error2".into()),
        ]
    );
    assert_eq!(handler.NumErrorWarnings(), (1, 3));

    let copied = handler.CopyWarnings(Vec::with_capacity(8));
    assert_eq!(copied.len(), 3);
    let truncated = handler.TruncateWarnings(1);
    assert_eq!(truncated.len(), 2);
    assert_eq!(handler.WarningCount(), 1);

    let clone = NewStaticWarnHandlerWithHandler(Some(&handler));
    handler.Reset();
    assert_eq!(handler.WarningCount(), 0);
    assert_eq!(clone.WarningCount(), 1);
}

#[test]
/// 普通错误的 SQLWarn JSON 往返：序列化为 level+msg，反序列化后文案一致。
fn migration_sql_warning_json_round_trips_plain_error() {
    let original = SQLWarn {
        Level: WarnLevelWarning.into(),
        Err: Some(errors::New("any error")),
    };
    let bytes = original.MarshalJSON().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        serde_json::json!({
            "level": "Warning",
            "msg": "any error"
        })
    );
    let mut decoded = SQLWarn {
        Level: String::new(),
        Err: None,
    };
    decoded.UnmarshalJSON(&bytes).unwrap();
    assert_eq!(decoded.Level, original.Level);
    assert_eq!(decoded.Err.unwrap().to_string(), "any error");
}

#[test]
/// 结构化 terror.Error 的 JSON：走 `err` 字段而非 `msg`，往返后文案保留。
fn migration_sql_warning_json_preserves_structured_error() {
    let structured = errors::Normalize(
        "result undetermined",
        &[
            errors::RFCCodeText("global:2"),
            errors::MySQLErrorCode(1105),
        ],
    );
    let original = SQLWarn {
        Level: WarnLevelWarning.into(),
        Err: Some(errors::SharedError::new(structured)),
    };
    let expected_message = original.Err.as_ref().unwrap().to_string();
    let bytes = original.MarshalJSON().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("err").is_some());
    assert!(value.get("msg").is_none());

    let mut decoded = SQLWarn {
        Level: String::new(),
        Err: None,
    };
    decoded.UnmarshalJSON(&bytes).unwrap();
    assert_eq!(decoded.Level, WarnLevelWarning);
    assert_eq!(decoded.Err.unwrap().to_string(), expected_message);
}

#[test]
/// PlanCacheTracker：Enable/Skip/Force/Restore/Save 与告警文案对齐 Go。
fn migration_plan_cache_tracker_matches_go_branches() {
    let handler = Arc::new(NewStaticWarnHandler(0));
    let tracker = NewPlanCacheTracker(handler.clone());
    tracker.EnablePlanCache();
    // 启用缓存后 SetSkipPlanCache 应禁用并写入 skip prepared 告警。
    tracker.SetCacheType(PlanCacheType::SessionPrepared);
    tracker.SetSkipPlanCache("risky optimization");
    assert!(!tracker.UseCache());
    assert_eq!(tracker.PlanCacheUnqualified(), "risky optimization");
    assert_eq!(
        messages(handler.as_ref())[0].1,
        "skip prepared plan-cache: risky optimization"
    );

    tracker.Restore(
        // forcePlanCache 为真时跳过只告警不真正关闭 useCache。
        true,
        PlanCacheType::SessionNonPrepared,
        String::new(),
        true,
        false,
    );
    tracker.SetSkipPlanCache("forced risk");
    assert!(tracker.UseCache());
    assert_eq!(
        messages(handler.as_ref())[1].1,
        "force plan-cache: may use risky cached plan: forced risk"
    );

    tracker.Restore(
        // Save/Restore 快照应完整还原五元组状态。
        true,
        PlanCacheType::SessionNonPrepared,
        "saved".into(),
        false,
        true,
    );
    let snapshot = tracker.Save();
    tracker.SetSkipPlanCache("long in-list");
    assert!(!tracker.UseCache());
    assert_eq!(
        messages(handler.as_ref())[2].1,
        "skip non-prepared plan-cache: long in-list"
    );
    tracker.Restore(
        snapshot.0,
        snapshot.1,
        snapshot.2.clone(),
        snapshot.3,
        snapshot.4,
    );
    assert_eq!(tracker.Save(), snapshot);
}

#[test]
/// RangeFallback：多次 Record 只告警一次，但始终禁用计划缓存。
fn migration_range_fallback_warns_once_but_always_disables_cache() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let sink = observed.clone();
    let warning = NewFuncWarnAppenderForTest(move |level, err| {
        sink.lock()
            .unwrap()
            .push((level.to_owned(), err.to_string()));
    });
    let tracker_warnings = Arc::new(NewStaticWarnHandler(0));
    let tracker = NewPlanCacheTracker(tracker_warnings);
    tracker.EnablePlanCache();
    tracker.SetCacheType(PlanCacheType::SessionNonPrepared);
    let fallback = NewRangeFallbackHandler(&tracker, warning.as_ref());
    fallback.RecordRangeFallback(1024);
    // 第二次 Record 不应再追加 fallback 告警（Once）。
    fallback.RecordRangeFallback(2048);
    drop(fallback);
    assert!(!tracker.UseCache());
    assert_eq!(tracker.PlanCacheUnqualified(), "in-list is too long");
    let got = observed.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, WarnLevelWarning);
    assert!(got[0].1.contains("1024 bytes"));
}
