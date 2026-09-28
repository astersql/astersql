// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Direct parity tests for the statement-context behavior exercised by
//! `stmtctx_test.go`. Cross-crate SQL execution and hint-handler integration
//! remain in their owning crates; this module tests the real stmtctx API.

use std::collections::{HashMap, HashSet};

use astersql_sessionctx_stmtctx::*;

#[test]
fn test_statement_context_push_down_flags() {
    let cases: Vec<(Box<dyn Fn(&mut StatementContext)>, u64)> = vec![
        (Box::new(|sc| sc.InInsertStmt = true), FlagInInsertStmt),
        (
            Box::new(|sc| sc.InUpdateStmt = true),
            FlagInUpdateOrDeleteStmt,
        ),
        (
            Box::new(|sc| sc.InDeleteStmt = true),
            FlagInUpdateOrDeleteStmt,
        ),
        (Box::new(|sc| sc.InSelectStmt = true), FlagInSelectStmt),
        (
            Box::new(|sc| sc.SetTypeFlags(sc.TypeFlags().WithIgnoreTruncateErr(true))),
            FlagIgnoreTruncate,
        ),
        (
            Box::new(|sc| sc.SetTypeFlags(sc.TypeFlags().WithTruncateAsWarning(true))),
            FlagTruncateAsWarning | FlagOverflowAsWarning,
        ),
        (
            Box::new(|sc| sc.SetTypeFlags(sc.TypeFlags().WithIgnoreZeroInDate(true))),
            FlagIgnoreZeroInDate,
        ),
        (
            Box::new(|sc| {
                let mut levels = sc.ErrLevels();
                levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] = errctx::Level::LevelWarn;
                sc.SetErrLevels(levels);
            }),
            FlagDividedByZeroAsWarning,
        ),
        (Box::new(|sc| sc.InLoadDataStmt = true), FlagInLoadDataStmt),
    ];

    for (configure, expected) in cases {
        let mut sc = NewStmtCtx();
        sc.SetErrLevels([errctx::Level::LevelError; errctx::errGroupCount]);
        configure(&mut sc);
        assert_eq!(expected, sc.PushDownFlags());
    }

    let mut sc = NewStmtCtx();
    sc.SetErrLevels([errctx::Level::LevelError; errctx::errGroupCount]);
    sc.InSelectStmt = true;
    sc.SetTypeFlags(sc.TypeFlags().WithTruncateAsWarning(true));
    assert_eq!(
        FlagInSelectStmt | FlagTruncateAsWarning | FlagOverflowAsWarning,
        sc.PushDownFlags()
    );

    let mut sc = NewStmtCtx();
    sc.SetErrLevels([errctx::Level::LevelError; errctx::errGroupCount]);
    let mut levels = sc.ErrLevels();
    levels[errctx::ErrGroup::ErrGroupDividedByZero as usize] = errctx::Level::LevelWarn;
    sc.SetErrLevels(levels);
    sc.SetTypeFlags(sc.TypeFlags().WithIgnoreTruncateErr(true));
    assert_eq!(
        FlagDividedByZeroAsWarning | FlagIgnoreTruncate,
        sc.PushDownFlags()
    );

    let mut sc = NewStmtCtx();
    sc.SetErrLevels([errctx::Level::LevelError; errctx::errGroupCount]);
    sc.InUpdateStmt = true;
    sc.InLoadDataStmt = true;
    sc.SetTypeFlags(sc.TypeFlags().WithIgnoreZeroInDate(true));
    assert_eq!(
        FlagInUpdateOrDeleteStmt | FlagIgnoreZeroInDate | FlagInLoadDataStmt,
        sc.PushDownFlags()
    );
}

#[test]
fn test_logical_plan_build_state_restore() {
    let sc = NewStmtCtx();
    sc.AppendWarning(errors::NewNoStackError("baseline warning"));
    sc.AppendExtraWarning(errors::NewNoStackError("baseline extra warning"));
    sc.SetLogicalPlanTables(vec![TableEntry {
        DB: "test".into(),
        Table: "t".into(),
    }]);
    sc.InsertLogicalPlanTableStats(42, cache_value("baseline stats".to_owned()));
    sc.InsertLogicalPlanLockTableID(1);
    sc.SetUseDynamicPruneMode(true);
    sc.SetLogicalPlanViewDepth(2);
    sc.InsertLogicalPlanColumnReference(7);
    sc.PlanCacheTracker
        .SetCacheType(PlanCacheType::SessionNonPrepared);
    sc.PlanCacheTracker.EnablePlanCache();
    let state = sc.SaveLogicalPlanBuildState();

    sc.AppendWarning(errors::NewNoStackError("candidate warning"));
    sc.SetLogicalPlanTables(vec![TableEntry {
        DB: "candidate".into(),
        Table: "t2".into(),
    }]);
    sc.ClearLogicalPlanTableStats();
    sc.InsertLogicalPlanLockTableID(2);
    sc.SetUseDynamicPruneMode(false);
    sc.SetLogicalPlanViewDepth(9);
    sc.InsertLogicalPlanColumnReference(9);
    sc.PlanCacheTracker.SetSkipPlanCache("candidate reason");
    sc.RestoreLogicalPlanBuildState(&state);

    assert_eq!(1, sc.GetWarnings().len());
    assert_eq!(
        "baseline warning",
        sc.GetWarnings()[0].Err.as_ref().unwrap().to_string()
    );
    assert_eq!(1, sc.GetExtraWarnings().len());
    assert_eq!("test", sc.LogicalPlanTables()[0].DB);
    assert!(sc.ContainsLogicalPlanTableStats(42));
    assert_eq!(HashSet::from([1]), sc.LogicalPlanLockTableIDs());
    assert!(sc.UseDynamicPartitionPrune());
    assert_eq!(2, sc.LogicalPlanViewDepth());
    assert!(sc.HasLogicalPlanColumnReference(7));
    assert!(!sc.HasLogicalPlanColumnReference(9));
    assert!(sc.PlanCacheTracker.UseCache());
    assert_eq!("", sc.PlanCacheTracker.PlanCacheUnqualified());
}

#[test]
fn test_new_stmt_ctx_and_time_zone() {
    let mut sc = NewStmtCtx();
    assert_eq!(DefaultStmtFlags, sc.TypeFlags());
    assert_eq!(chrono_tz::UTC, sc.TimeZone());
    sc.AppendWarning(errors::NewNoStackError("err1"));
    assert_eq!(
        "err1",
        sc.GetWarnings()[0].Err.as_ref().unwrap().to_string()
    );
    sc.SetTimeZone(chrono_tz::Asia::Shanghai);
    assert_eq!(chrono_tz::Asia::Shanghai, sc.TimeZone());
    let fixed = NewStmtCtxWithTimeZone(chrono_tz::America::New_York);
    assert_eq!(chrono_tz::America::New_York, fixed.TimeZone());
}

#[test]
fn test_set_stmt_ctx_type_flags_and_err_ctx() {
    let mut sc = NewStmtCtx();
    sc.SetErrLevels([errctx::Level::LevelError; errctx::errGroupCount]);
    let flags = DefaultStmtFlags
        .WithAllowNegativeToUnsigned(true)
        .WithSkipSACIICheck(true);
    sc.SetTypeFlags(flags);
    assert_eq!(flags, sc.TypeFlags());
    assert_eq!(
        errctx::Level::LevelError,
        sc.ErrLevels()[errctx::ErrGroup::ErrGroupTruncate as usize]
    );

    let flags = DefaultStmtFlags
        .WithSkipSACIICheck(true)
        .WithSkipUTF8Check(true)
        .WithTruncateAsWarning(true);
    sc.SetTypeFlags(flags);
    assert_eq!(
        errctx::Level::LevelWarn,
        sc.ErrLevels()[errctx::ErrGroup::ErrGroupTruncate as usize]
    );

    let mut replacement = [errctx::Level::LevelError; errctx::errGroupCount];
    replacement[errctx::ErrGroup::ErrGroupAutoIncReadFailed as usize] = errctx::Level::LevelIgnore;
    sc.SetErrLevels(replacement);
    assert_eq!(
        errctx::Level::LevelWarn,
        sc.ErrLevels()[errctx::ErrGroup::ErrGroupTruncate as usize]
    );
    assert_eq!(
        errctx::Level::LevelIgnore,
        sc.ErrLevels()[errctx::ErrGroup::ErrGroupAutoIncReadFailed as usize]
    );
}

#[test]
fn test_reset_stmt_ctx_and_id() {
    let mut sc = NewStmtCtx();
    let first_id = sc.CtxID();
    sc.SetTimeZone(chrono_tz::Asia::Shanghai);
    sc.SetTypeFlags(
        DefaultStmtFlags
            .WithIgnoreTruncateErr(true)
            .WithAllowNegativeToUnsigned(true)
            .WithSkipSACIICheck(true),
    );
    sc.AppendWarning(errors::NewNoStackError("err1"));
    sc.InRestrictedSQL = true;
    sc.StmtType = "Insert".into();

    assert!(sc.Reset());
    assert!(sc.CtxID() > first_id);
    assert_eq!(chrono_tz::UTC, sc.TimeZone());
    assert_eq!(DefaultStmtFlags, sc.TypeFlags());
    assert!(!sc.InRestrictedSQL);
    assert!(sc.StmtType.is_empty());
    assert!(sc.GetWarnings().is_empty());
}

#[test]
fn test_reserved_row_id_alloc() {
    let mut alloc = ReservedRowIDAlloc::default();
    assert!(alloc.Exhausted());
    assert_eq!((0, false), alloc.Consume());
    alloc.Reset(12, 15);
    assert!(!alloc.Exhausted());
    assert_eq!((13, true), alloc.Consume());
    assert_eq!((14, true), alloc.Consume());
    assert_eq!((15, true), alloc.Consume());
    assert!(alloc.Exhausted());
    assert_eq!((0, false), alloc.Consume());
}

#[test]
fn test_used_stats_info_for_table_write_to_slow_log() {
    let pseudo = UsedStatsInfoForTable {
        Name: "t1".into(),
        Version: 0,
        RealtimeCount: 1000,
        ModifyCount: 100,
        ..Default::default()
    };
    let mut out = Vec::new();
    pseudo.WriteToSlowLog(&mut out).unwrap();
    assert_eq!(
        "t1:stats_meta_version=pseudo[realtime_count=1000;modify_count=100]",
        String::from_utf8(out).unwrap()
    );

    let mut stats = UsedStatsInfoForTable {
        Name: "t2".into(),
        Version: 10,
        RealtimeCount: 2000,
        ..Default::default()
    };
    stats.IndexStatsLoadStatus = HashMap::from([(1, "allLoaded".into())]);
    stats.ColumnStatsLoadStatus = HashMap::from([(2, "onlyCmsEvicted".into())]);
    let mut out = Vec::new();
    stats.WriteToSlowLog(&mut out).unwrap();
    assert_eq!(
        "t2:stats_meta_version=10[realtime_count=2000;modify_count=0][ID 1:allLoaded][ID 2:onlyCmsEvicted]",
        String::from_utf8(out).unwrap()
    );
}

/// Benchmark-shaped helper corresponding to Go's `BenchmarkErrCtx`.
pub fn benchmark_err_ctx(iterations: usize) {
    let sc = NewStmtCtx();
    for _ in 0..iterations {
        let _ = sc.ErrCtx();
    }
}

#[test]
fn test_plan_caches_support_shared_accessor_and_keep_nil_digest_semantics() {
    let stmt = NewStmtCtx();
    let digest = task_parser::digester_impl::NewDigest(vec![0xab, 0xcd]);
    stmt.SetPlanDigest("normalized", Some(digest.clone()));
    stmt.SetPlanDigest("ignored", None);
    assert_eq!(stmt.GetPlanDigest(), ("normalized".into(), Some(digest)));
    let flat: CacheValue = std::sync::Arc::new("flat plan".to_owned());
    stmt.SetFlatPlan(Some(flat.clone()));
    assert!(std::sync::Arc::ptr_eq(&flat, &stmt.GetFlatPlan().unwrap()));
    stmt.SetFlatPlan(None);
    assert!(stmt.GetFlatPlan().is_none());
}
