// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 语句上下文综合迁移期单元测试。
//
// 对照 Go 原子行为与边界：任务 ID 分配、引用计数冻结、预留 RowID、
// 语句缓存、行计数/告警/重试复位、逻辑计划构建状态存取、下推 flags、
// 过期读 TSO 记忆化、统计格式化、SQL digest 与完整 Reset。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use astersql_sessionctx_stmtctx::*;

/// 并发分配唯一 TaskID，并验证 ReferenceCount 增/减/冻结/解冻的 CAS 语义。
#[test]
fn task_ids_and_reference_count_preserve_go_atomic_behavior() {
    let mut handles = Vec::new();
    for _ in 0..8 {
        handles.push(thread::spawn(|| AllocateTaskID()));
    }
    let mut ids: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(8, ids.len());

    let refs = ReferenceCount::default();
    assert!(refs.TryIncrease());
    assert!(!refs.TryFreeze());
    refs.Decrease();
    assert!(refs.TryFreeze());
    assert!(!refs.TryIncrease());
    refs.UnFreeze();
    assert!(refs.TryIncrease());
}

#[test]
fn retry_reset_clears_all_go_fields_through_shared_statement_context() {
    let sc = Arc::new(NewStmtCtx());
    sc.AddFoundRows(9);
    sc.AddAffectedRows(3);
    sc.AppendWarning(errors::NewNoStackError("first attempt warning"));
    sc.TableIDs.lock().unwrap().push(81);
    sc.IndexNames.lock().unwrap().push("t:i".into());
    sc.ReservedRowIDAlloc.lock().unwrap().Reset(1, 3);
    let first = sc.GetOrInitDistSQLFromCache(|| cache_value(1_u64));
    assert_eq!(*cache_downcast_ref::<u64>(&first).unwrap(), 1);
    let old_task_id = sc.TaskID.load(Ordering::Acquire);
    sc.ResetForRetry();
    assert_eq!(sc.FoundRows(), 0);
    assert_eq!(sc.AffectedRows(), 0);
    assert!(sc.GetWarnings().is_empty());
    assert!(sc.TableIDs.lock().unwrap().is_empty());
    assert!(sc.IndexNames.lock().unwrap().is_empty());
    assert!(sc.ReservedRowIDAlloc.lock().unwrap().Exhausted());
    assert!(sc.TaskID.load(Ordering::Acquire) > old_task_id);
    let second = sc.GetOrInitDistSQLFromCache(|| cache_value(2_u64));
    assert_eq!(*cache_downcast_ref::<u64>(&second).unwrap(), 2);
}

/// 验证预留 RowID 耗尽边界，以及语句缓存 GetOrEvaluate / Reset / GetOrStore。
#[test]
fn reserved_row_ids_and_statement_cache_match_go_boundaries() {
    let mut reserved = ReservedRowIDAlloc::default();
    assert!(reserved.Exhausted());
    assert_eq!((0, false), reserved.Consume());
    reserved.Reset(12, 15);
    assert_eq!((13, true), reserved.Consume());
    assert_eq!((14, true), reserved.Consume());
    assert_eq!((15, true), reserved.Consume());
    assert_eq!((0, false), reserved.Consume());

    let sc = NewStmtCtx();
    let calls = AtomicUsize::new(0);
    let first = sc
        .GetOrEvaluateStmtCache(StmtNowTsCacheKey, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(cache_value(42_u64))
        })
        .unwrap();
    let second = sc
        .GetOrEvaluateStmtCache(StmtNowTsCacheKey, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(cache_value(99_u64))
        })
        .unwrap();
    assert_eq!(42, *cache_downcast_ref::<u64>(&first).unwrap());
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(1, calls.load(Ordering::SeqCst));
    sc.ResetInStmtCache(StmtNowTsCacheKey);
    let third = sc.GetOrStoreStmtCache(StmtNowTsCacheKey, cache_value(7_u64));
    assert_eq!(7, *cache_downcast_ref::<u64>(&third).unwrap());
}

/// 验证受影响行数（外键触发器内不计）、各类行计数、告警与 ResetForRetry。
#[test]
fn counters_warnings_and_retry_reset_match_go_behavior() {
    let mut sc = NewStmtCtx();
    sc.AddAffectedRows(2);
    sc.InHandleForeignKeyTrigger
        .store(true, std::sync::atomic::Ordering::Relaxed);
    sc.AddAffectedRows(9);
    sc.InHandleForeignKeyTrigger
        .store(false, std::sync::atomic::Ordering::Relaxed);
    sc.AddFoundRows(3);
    sc.AddRecordRows(4);
    sc.AddDeletedRows(5);
    sc.AddUpdatedRows(6);
    sc.AddCopiedRows(7);
    sc.AddTouchedRows(8);
    sc.SetMessage("done");
    sc.AppendWarning(errors::NewNoStackError("warn"));
    sc.AppendNote(errors::NewNoStackError("note"));
    sc.AppendError(errors::NewNoStackError("error"));

    assert_eq!(2, sc.AffectedRows());
    assert_eq!((3, 4, 5, 6, 7, 8), sc.RowCounters());
    assert_eq!("done", sc.GetMessage());
    assert_eq!((1, 3), sc.NumErrorWarnings());
    sc.InShowWarning = true;
    assert_eq!(0, sc.WarningCount());

    let old_task_id = sc.TaskID.load(std::sync::atomic::Ordering::Acquire);
    sc.ResetForRetry();
    assert_eq!((0, 0, 0, 0, 0, 0), sc.RowCounters());
    assert_eq!(0, sc.AffectedRows());
    assert_eq!("", sc.GetMessage());
    assert_eq!(0, sc.GetWarnings().len());
    assert!(sc.TaskID.load(std::sync::atomic::Ordering::Acquire) > old_task_id);
}

/// 验证 Save/RestoreLogicalPlanBuildState 回滚构建期字段并保留 baseline。
#[test]
fn logical_plan_build_state_restores_every_go_field() {
    let mut sc = NewStmtCtx();
    sc.AppendWarning(errors::NewNoStackError("baseline"));
    sc.AppendExtraWarning(errors::NewNoStackError("extra"));
    sc.SetLogicalPlanTables(vec![TableEntry {
        DB: "test".into(),
        Table: "t".into(),
    }]);
    sc.InsertLogicalPlanTableStats(42, cache_value("stats".to_owned()));
    sc.InsertLogicalPlanLockTableID(1);
    sc.SetUseDynamicPruneMode(true);
    sc.SetLogicalPlanViewDepth(2);
    sc.InsertLogicalPlanColumnReference(7);
    sc.PlanCacheTracker
        .SetCacheType(PlanCacheType::SessionNonPrepared);
    sc.PlanCacheTracker.SetForcePlanCache(true);
    sc.PlanCacheTracker.SetAlwaysWarnSkipCache(true);
    sc.PlanCacheTracker.EnablePlanCache();
    let state = sc.SaveLogicalPlanBuildState();

    sc.AppendWarning(errors::NewNoStackError("candidate"));
    sc.AppendExtraWarning(errors::NewNoStackError("candidate extra"));
    sc.SetLogicalPlanTables(Vec::new());
    sc.ClearLogicalPlanTableStats();
    sc.InsertLogicalPlanLockTableID(2);
    sc.SetUseDynamicPruneMode(false);
    sc.SetLogicalPlanViewDepth(9);
    sc.InsertLogicalPlanColumnReference(9);
    sc.PlanCacheTracker
        .SetCacheType(PlanCacheType::SessionPrepared);
    sc.PlanCacheTracker.SetForcePlanCache(false);
    sc.PlanCacheTracker.SetAlwaysWarnSkipCache(false);
    sc.PlanCacheTracker.SetSkipPlanCache("candidate reason");
    sc.RestoreLogicalPlanBuildState(&state);

    assert_eq!(1, sc.GetWarnings().len());
    assert_eq!(1, sc.GetExtraWarnings().len());
    assert_eq!(
        vec![TableEntry {
            DB: "test".into(),
            Table: "t".into()
        }],
        sc.LogicalPlanTables()
    );
    assert!(sc.ContainsLogicalPlanTableStats(42));
    assert_eq!(HashSet::from([1]), sc.LogicalPlanLockTableIDs());
    assert!(sc.UseDynamicPartitionPrune());
    assert_eq!(2, sc.LogicalPlanViewDepth());
    assert!(sc.HasLogicalPlanColumnReference(7));
    assert!(!sc.HasLogicalPlanColumnReference(9));
    assert_eq!(
        (
            true,
            PlanCacheType::SessionNonPrepared,
            String::new(),
            true,
            true
        ),
        sc.PlanCacheTracker.Save()
    );

    sc.SetLogicalPlanTables(Vec::new());
    sc.InsertLogicalPlanColumnReference(99);
    sc.RestoreLogicalPlanBuildState(&state);
    assert_eq!(1, sc.LogicalPlanTables().len());
    assert!(sc.HasLogicalPlanColumnReference(7));
    assert!(!sc.HasLogicalPlanColumnReference(99));
}

/// 验证 PushDownFlags 编码与 InitFromPBFlagAndTz 解码在边界上互逆。
#[test]
fn push_down_flags_and_pb_initialization_are_inverse_at_go_boundaries() {
    let mut sc = NewStmtCtx();
    sc.InSelectStmt = true;
    sc.InLoadDataStmt = true;
    sc.InRestrictedSQL = true;
    sc.SetTypeFlags(
        DefaultStmtFlags
            .WithTruncateAsWarning(true)
            .WithIgnoreZeroInDate(true),
    );
    let flags = sc.PushDownFlags();
    assert_eq!(
        FlagTruncateAsWarning
            | FlagOverflowAsWarning
            | FlagIgnoreZeroInDate
            | FlagDividedByZeroAsWarning
            | FlagInSelectStmt
            | FlagInLoadDataStmt
            | FlagInRestrictedSQL,
        flags
    );

    let mut decoded = NewStmtCtx();
    decoded.InitFromPBFlagAndTz(flags, chrono_tz::Asia::Shanghai);
    assert!(decoded.InSelectStmt);
    assert!(decoded.TypeFlags().TruncateAsWarning());
    assert!(decoded.TypeFlags().IgnoreZeroInDate());
    assert_eq!(chrono_tz::Asia::Shanghai, decoded.TimeZone());
}

/// 验证过期读 TSO：成功值记忆化，错误不缓存以便重试。
#[test]
fn stale_tso_provider_is_memoized_but_errors_are_not_cached() {
    let sc = NewStmtCtx();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_eval = Arc::clone(&calls);
    sc.SetStaleTSOProviderIfNotExist(move || {
        let call = calls_for_eval.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Err(errors::NewNoStackError("temporary"))
        } else {
            Ok(123)
        }
    });
    assert!(sc.GetStaleTSO().is_err());
    assert_eq!(123, sc.GetStaleTSO().unwrap());
    assert_eq!(123, sc.GetStaleTSO().unwrap());
    assert_eq!(2, calls.load(Ordering::SeqCst));
}

/// 验证 UsedStats 的 EXPLAIN 截断格式与慢日志按 ID 排序输出。
#[test]
fn stats_formats_sort_ids_limit_explain_and_keep_slow_log_order() {
    let mut stats = UsedStatsInfoForTable {
        Name: "t2".into(),
        Version: 10,
        RealtimeCount: 2000,
        ModifyCount: 3,
        ..Default::default()
    };
    stats.IndexStatsLoadStatus =
        HashMap::from([(3, "onlyHistRemained".into()), (1, "allLoaded".into())]);
    stats.ColumnStatsLoadStatus =
        HashMap::from([(4, "onlyCmsEvicted".into()), (2, "onlyCmsEvicted".into())]);
    assert_eq!(
        "stats:partial[ID 1:allLoaded, ID 3:onlyHistRemained, ID 2:onlyCmsEvicted...(more: 1 onlyCmsEvicted)]",
        stats.FormatForExplain()
    );
    let mut out = Vec::new();
    stats.WriteToSlowLog(&mut out).unwrap();
    assert_eq!(
        "t2:stats_meta_version=10[realtime_count=2000;modify_count=3][ID 1:allLoaded,ID 3:onlyHistRemained][ID 2:onlyCmsEvicted,ID 4:onlyCmsEvicted]",
        String::from_utf8(out).unwrap()
    );

    let pseudo = UsedStatsInfoForTable {
        Name: "p".into(),
        Version: 0,
        ..Default::default()
    };
    assert_eq!("stats:pseudo", pseudo.FormatForExplain());
}

/// 验证 SQL digest 记忆化、SET_VAR 恢复、统计计数与语句标签解析。
#[test]
fn sql_digest_setters_used_stats_and_labels_preserve_go_memoization() {
    let mut sc = NewStmtCtx();
    sc.OriginalSQL = "select 1".into();
    let (normalized, digest) = sc.SQLDigest();
    assert!(!normalized.is_empty());
    assert!(!digest.String().is_empty());
    sc.OriginalSQL = "select 2".into();
    assert_eq!(normalized, sc.SQLDigest().0);
    sc.ResetSQLDigest("select * from t");
    assert_ne!(normalized, sc.SQLDigest().0);

    sc.AddSetVarHintRestore("x", "old");
    sc.AddSetVarHintRestore("x", "new");
    assert_eq!(Some(&"old".to_owned()), sc.SetVarHintRestore.get("x"));

    let used = sc.GetUsedStatsInfo(true).unwrap();
    used.RecordUsedInfo(
        8,
        Arc::new(UsedStatsInfoForTable {
            IndexStatsLoadStatus: HashMap::from([(1, "partial".into())]),
            ColumnStatsLoadStatus: HashMap::from([(2, "partial".into())]),
            ..Default::default()
        }),
    );
    assert_eq!(2, sc.RecordedStatsLoadStatusCnt());

    let ctx = WithStmtLabel(StmtLabelContext::default(), "custom");
    assert_eq!("custom", GetStmtLabel(&ctx, &StatementKind::Select));
    assert_eq!(
        "Select",
        GetStmtLabel(&StmtLabelContext::default(), &StatementKind::Select)
    );
}

/// 验证完整 Reset 清空语句态，但保留可复用的逻辑计划映射与相关表 ID。
#[test]
fn full_reset_clears_statement_state_and_preserves_reused_maps() {
    let mut sc = NewStmtCtx();
    let original_id = sc.CtxID();
    sc.SetLogicalPlanTables(vec![TableEntry {
        DB: "test".into(),
        Table: "t".into(),
    }]);
    sc.InsertLogicalPlanLockTableID(7);
    sc.InsertLogicalPlanTableStats(9, cache_value(1_u8));
    sc.SetUseDynamicPruneMode(true);
    sc.SetLogicalPlanViewDepth(3);
    sc.InsertLogicalPlanColumnReference(5);
    sc.RelatedTableIDs.insert(11);
    sc.InRestrictedSQL = true;
    sc.StmtType = "Insert".into();
    sc.AppendWarning(errors::NewNoStackError("before reset"));
    assert!(sc.Reset());
    assert!(sc.CtxID() > original_id);
    assert_eq!(chrono_tz::UTC, sc.TimeZone());
    assert_eq!(DefaultStmtFlags, sc.TypeFlags());
    assert!(!sc.InRestrictedSQL);
    assert_eq!("", sc.StmtType);
    assert!(sc.GetWarnings().is_empty());
    assert!(sc.LogicalPlanLockTableIDs().contains(&7));
    assert!(sc.ContainsLogicalPlanTableStats(9));
    assert!(sc.RelatedTableIDs.contains(&11));
    assert!(sc.LogicalPlanTables().is_empty());
    assert!(!sc.UseDynamicPartitionPrune());
    assert_eq!(0, sc.LogicalPlanViewDepth());
    assert!(!sc.HasLogicalPlanColumnReference(5));
}

#[test]
fn dist_sql_cache_reset_preserves_statement_counters_and_warnings() {
    let sc = NewStmtCtx();
    sc.AddFoundRows(9);
    sc.AddAffectedRows(3);
    sc.AppendWarning(errors::NewNoStackError("keep current warning"));
    let first = sc.GetOrInitDistSQLFromCache(|| cache_value(1_u64));
    sc.ResetDistSQLFromCache();
    let second = sc.GetOrInitDistSQLFromCache(|| cache_value(2_u64));
    assert_eq!(*cache_downcast_ref::<u64>(&first).unwrap(), 1);
    assert_eq!(*cache_downcast_ref::<u64>(&second).unwrap(), 2);
    assert_eq!(sc.FoundRows(), 9);
    assert_eq!(sc.AffectedRows(), 3);
    assert_eq!(sc.GetWarnings().len(), 1);
}

#[test]
fn engine_round_signals_mark_and_reset_together() {
    let sc = NewStmtCtx();
    sc.MarkAlternativeLogicalPlanMixedStorageEngines();
    sc.MarkAlternativeLogicalPlanMissingTiFlashPath();
    sc.MarkAlternativeLogicalPlanHasStoreTypeHint();
    assert_eq!(sc.AlternativeLogicalPlanEngineSignals(), (true, true, true));
    sc.ResetAlternativeRoundSignals();
    assert_eq!(
        sc.AlternativeLogicalPlanEngineSignals(),
        (false, false, false)
    );
}
