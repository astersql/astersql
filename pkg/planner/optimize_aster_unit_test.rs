// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 顶层 Optimize 编排的 AsterSQL 单元测试。
//
// 覆盖只读准入分类、非预处理计划缓存资格判定、缓存命中/未命中、
// ScopeRestore、替代逻辑计划轮次信号、FastPlan / Binding / Baseline 演进，
// 以及 DefaultOptimizeSessionService 对五组逻辑构建状态的反复恢复。

use super::*;

/// 对 AST 节点调用只读准入分类辅助。
fn decision<T: ast::Node>(statement: T) -> ReadOnlyAdmission {
    classifyReadOnlyAdmission(&statement)
}

/// 只读模式下白名单语句应一律 Allowed（SET/ANALYZE/USE/SHOW/Binding/Prepare/Begin/Rollback）。
#[test]
fn read_only_mode_keeps_go_statement_whitelist() {
    assert_eq!(
        decision(ast::SetStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::AnalyzeTableStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::UseStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::ShowStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::CreateBindingStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::DropBindingStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::PrepareStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::BeginStmt::default()),
        ReadOnlyAdmission::Allowed
    );
    assert_eq!(
        decision(ast::RollbackStmt::default()),
        ReadOnlyAdmission::Allowed
    );
}

/// Commit / Select / Insert 分别映射为 Commit、Ast(true)、Ast(false)。
#[test]
fn read_only_mode_distinguishes_commit_reads_and_writes() {
    assert_eq!(
        decision(ast::CommitStmt::default()),
        ReadOnlyAdmission::Commit
    );
    assert_eq!(
        decision(ast::SelectStmt::default()),
        ReadOnlyAdmission::Ast(true)
    );
    assert_eq!(
        decision(ast::InsertStmt::default()),
        ReadOnlyAdmission::Ast(false)
    );
}

/// Commit 决策应传播事务检查的布尔结果与错误。
#[test]
fn commit_decision_propagates_transaction_result_and_error() {
    assert!(resolveReadOnlyAdmission(ReadOnlyAdmission::Commit, || Ok(true)).unwrap());
    assert!(!resolveReadOnlyAdmission(ReadOnlyAdmission::Commit, || Ok(false)).unwrap());

    let error = resolveReadOnlyAdmission(ReadOnlyAdmission::Commit, || {
        Err(planner_error("rollback failed"))
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "rollback failed");
}

/// 非 Commit 决策不得调用事务检查回调。
#[test]
fn non_commit_decisions_do_not_touch_transaction() {
    let mut called = false;
    let allowed = resolveReadOnlyAdmission(ReadOnlyAdmission::Allowed, || {
        called = true;
        Ok(false)
    })
    .unwrap();
    assert!(allowed);
    assert!(!called);

    let rejected = resolveReadOnlyAdmission(ReadOnlyAdmission::Ast(false), || {
        called = true;
        Ok(true)
    })
    .unwrap();
    assert!(!rejected);
    assert!(!called);
}

/// 非预处理缓存初始资格判定与 Go 条件一致（开关、受限 SQL、多语句等）。
#[test]
fn non_prepared_cache_initial_qualification_matches_go() {
    let mut vars = SessionVars::new();
    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false),
        InitialNonPreparedCacheEligibility::Bypass
    );

    vars.EnableNonPreparedPlanCache = true;
    vars.DisableTxnAutoRetry = true;
    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false),
        InitialNonPreparedCacheEligibility::Eligible
    );

    vars.StmtCtx.InRestrictedSQL = true;
    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false),
        InitialNonPreparedCacheEligibility::Bypass
    );
    vars.StmtCtx.InRestrictedSQL = false;

    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, true),
        InitialNonPreparedCacheEligibility::Bypass
    );

    vars.DisableTxnAutoRetry = false;
    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false),
        InitialNonPreparedCacheEligibility::Bypass
    );
    vars.DisableTxnAutoRetry = true;

    vars.InMultiStmts = true;
    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, true, false),
        InitialNonPreparedCacheEligibility::Bypass
    );
    vars.InMultiStmts = false;

    assert_eq!(
        initialNonPreparedCacheEligibility(&vars, false, false),
        InitialNonPreparedCacheEligibility::Bypass
    );
}

/// hint_only 策略无 hint 时旁路；cacheable=false 禁止查找。
#[test]
fn non_prepared_cache_hint_and_cacheability_bypass() {
    assert!(shouldBypassHintOnlyStrategy(
        vardef::TiDBPlanCacheStrategyHintOnly,
        false
    ));
    assert!(!shouldBypassHintOnlyStrategy(
        vardef::TiDBPlanCacheStrategyHintOnly,
        true
    ));
    assert!(!cacheabilityAllowsLookup(&NonPreparedCacheability {
        cacheable: false,
        reason: "unsupported statement".to_owned(),
    }));
}

/// 旁路警告仅在 EXPLAIN FORMAT=plan_cache 时写入 StmtCtx。
#[test]
fn non_prepared_cache_bypass_warning_is_explain_plan_cache_only() {
    let mut vars = SessionVars::new();
    appendPlanCacheBypassWarning(&vars, "unsupported statement");
    assert_eq!(vars.StmtCtx.WarningCount(), 0);

    vars.StmtCtx.InExplainStmt = true;
    vars.StmtCtx.ExplainFormat = types::ExplainFormatPlanCache.to_owned();
    appendPlanCacheBypassWarning(&vars, "unsupported statement");
    let warnings = vars.StmtCtx.GetWarnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].Err.as_ref().unwrap().to_string(),
        "skip non-prepared plan-cache: unsupported statement"
    );
    let shared = SessionVars::new();
    shared
        .StmtCtx
        .SetExplainContext(true, true, types::ExplainFormatRU);
    assert!(!shouldWarnPlanCacheBypass(&shared));
    shared
        .StmtCtx
        .SetExplainContext(true, false, types::ExplainFormatPlanCache);
    assert!(shouldWarnPlanCacheBypass(&shared));
}

/// 测试用非预处理缓存语句桩。
struct TestCachedStatement(u8);
impl NonPreparedCachedStatement for TestCachedStatement {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 缓存命中时不得创建或写回。
#[test]
fn non_prepared_cache_hit_skips_creation() {
    let cached: NonPreparedCachedStatementRef = Arc::new(TestCachedStatement(1));
    let expected = Arc::clone(&cached);
    let mut created = false;
    let mut stored = false;
    let selected = chooseCachedStatement(
        Some(cached),
        || {
            created = true;
            Ok(Arc::new(TestCachedStatement(2)))
        },
        |_| {
            stored = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(Arc::ptr_eq(&selected, &expected));
    assert!(!created);
    assert!(!stored);
}

/// 缓存未命中时创建一次并写回，返回同一引用。
#[test]
fn non_prepared_cache_miss_creates_and_stores_once() {
    let mut create_count = 0;
    let mut stored = None;
    let selected = chooseCachedStatement(
        None,
        || {
            create_count += 1;
            Ok(Arc::new(TestCachedStatement(7)) as NonPreparedCachedStatementRef)
        },
        |statement| {
            stored = Some(statement);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(create_count, 1);
    let stored = stored.unwrap();
    assert!(Arc::ptr_eq(&selected, &stored));
    assert_eq!(
        selected
            .as_ref()
            .as_any()
            .downcast_ref::<TestCachedStatement>()
            .unwrap()
            .0,
        7
    );
}

/// ScopeRestore 在成功路径与提前错误路径均会执行恢复闭包。
#[test]
fn scope_restore_runs_on_success_and_early_error_paths() {
    use std::cell::Cell;

    let state = Cell::new(false);
    {
        state.set(true);
        let _restore = ScopeRestore::new(|| state.set(false));
        assert!(state.get());
    }
    assert!(!state.get());

    let state = Cell::new(false);
    let operation = || -> Result<(), expression::Error> {
        state.set(true);
        let _restore = ScopeRestore::new(|| state.set(false));
        Err(planner_error("optimization failed"))
    };
    assert!(operation().is_err());
    assert!(!state.get());
}

/// 替代逻辑计划各轮次启用条件与 Go 信号组合一致。
#[test]
fn alternative_round_eligibility_matches_go_conditions() {
    let mut vars = SessionVars::new();
    let all = AlternativeSignals {
        decorrelated_apply: true,
        same_order_index_join: false,
        order_aware_join_reorder: true,
        prefer_correlate: true,
        semi_join_rewrite: true,
        fts_like_fallback: true,
        predicate_context_match: true,
    };
    assert!(!shouldTryNonDecorrelationRound(&vars, all));
    assert!(!shouldTryOrderAwareReorderRound(&vars, all));
    assert!(!shouldTryCorrelateRound(&vars, all));
    assert!(!shouldTrySemiJoinRewriteRound(&vars, all));
    assert!(!shouldTryFtsLikeFallbackRound(&vars, all));

    vars.EnableAlternativeLogicalPlans = true;
    assert!(shouldTryNonDecorrelationRound(&vars, all));
    assert!(shouldTryOrderAwareReorderRound(&vars, all));
    assert!(shouldTryCorrelateRound(&vars, all));
    assert!(shouldTrySemiJoinRewriteRound(&vars, all));
    assert!(shouldTryFtsLikeFallbackRound(&vars, all));

    // same_order_index_join 阻止非解相关轮次。
    let same_order = AlternativeSignals {
        same_order_index_join: true,
        ..all
    };
    assert!(!shouldTryNonDecorrelationRound(&vars, same_order));
    vars.EnableSemiJoinRewrite = true;
    assert!(!shouldTrySemiJoinRewriteRound(&vars, all));

    let predicate_only = AlternativeSignals {
        fts_like_fallback: false,
        predicate_context_match: true,
        ..Default::default()
    };
    assert!(shouldTryFtsLikeFallbackRound(&vars, predicate_only));
}

/// FastPlan 仅在隔离读引擎包含 TiKV 时尝试。
#[test]
fn fast_plan_is_attempted_only_when_tikv_isolation_is_available() {
    let mut called = false;
    let result = tryFastPlanIfTiKV(false, || {
        called = true;
        Ok(Some(7))
    })
    .unwrap();
    assert_eq!(result, None);
    assert!(!called);

    let result = tryFastPlanIfTiKV(true, || {
        called = true;
        Ok(Some(7))
    })
    .unwrap();
    assert_eq!(result, Some(7));
    assert!(called);
}

/// 计划绑定需要开启 baseline、是语句且匹配成功三者同时成立。
#[test]
fn binding_requires_baseline_statement_and_match() {
    assert!(shouldUsePlanBinding(true, true, true));
    assert!(!shouldUsePlanBinding(false, true, true));
    assert!(!shouldUsePlanBinding(true, false, true));
    assert!(!shouldUsePlanBinding(true, true, false));
}

/// Baseline 演进资格与 Go 条件一致（evolve、无 limit、SELECT、无 READ_FROM_STORAGE）。
#[test]
fn baseline_evolution_matches_go_eligibility() {
    assert!(shouldTryBaselineEvolution(true, u64::MAX, true, false));
    assert!(!shouldTryBaselineEvolution(false, u64::MAX, true, false));
    assert!(!shouldTryBaselineEvolution(true, 100, true, false));
    assert!(!shouldTryBaselineEvolution(true, u64::MAX, false, false));
    assert!(!shouldTryBaselineEvolution(true, u64::MAX, true, true));
}

/// 快照测试用的最小 PlanContext 实现。
struct SnapshotPlanContext {
    vars: SessionVars,
}

impl base::PlanContext for SnapshotPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.vars.AllocNewPlanID()
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &SessionVars {
        &self.vars
    }
    fn GetExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("snapshot test does not evaluate expressions")
    }
    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        panic!("snapshot test does not build ranges")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn astersql_planner_planctx::exprctx::ExprContext {
        panic!("snapshot test does not evaluate expressions")
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("snapshot test does not build protobuf")
    }
    fn BuiltinFunctionUsageInc(&self, _: &str) {}
}

/// DefaultOptimizeSessionService 可反复恢复五组逻辑计划构建状态。
#[test]
fn default_session_service_restores_all_five_groups_repeatedly() {
    use std::time::Duration;

    let context = SnapshotPlanContext {
        vars: SessionVars::new(),
    };
    context
        .vars
        .StmtCtx
        .SetLogicalPlanTables(vec![astersql_sessionctx_stmtctx::TableEntry {
            DB: "test".into(),
            Table: "t".into(),
        }]);
    context
        .vars
        .PlannerSelectBlockAsName
        .Store(Some(vec![ast::HintTable::default()]));
    context.vars.RegisterScalarSubQ(41_i32);
    context.vars.InsertExtendedColumnUniqueID("hash", 51);
    context.vars.RestoreRewritePhaseInfo(RewritePhaseInfo {
        DurationRewrite: Duration::from_millis(61),
        DurationPreprocessSubQuery: Duration::from_millis(7),
        PreprocessSubQueries: 1,
    });
    let context: base::ContextRef = Arc::new(context);
    let service = DefaultOptimizeSessionService::new(());
    let snapshot = service.save_logical_plan_build_state(&context);

    // 清空后再 restore，循环两次确认幂等。
    for _ in 0..2 {
        let vars = context.GetSessionVars();
        vars.StmtCtx.SetLogicalPlanTables(Vec::new());
        vars.PlannerSelectBlockAsName.Store(None);
        vars.RestoreScalarSubQueries(Vec::new());
        vars.RestoreExtendedColumnUniqueIDs(HashMap::new());
        vars.RestoreRewritePhaseInfo(RewritePhaseInfo::default());
        service.restore_logical_plan_build_state(&context, snapshot.as_ref());

        assert_eq!(vars.StmtCtx.LogicalPlanTables().len(), 1);
        assert_eq!(vars.PlannerSelectBlockAsName.Load().unwrap().len(), 1);
        vars.WithScalarSubQueries(|values| {
            assert_eq!(*values[0].as_ref().downcast_ref::<i32>().unwrap(), 41)
        });
        assert_eq!(vars.ExtendedColumnUniqueID("hash"), Some(51));
        assert_eq!(
            vars.SnapshotRewritePhaseInfo().DurationRewrite,
            Duration::from_millis(61)
        );
    }
}

/// 每次 Go buildLogicalPlan 开始前都清空改写期的瞬态映射与计时状态。
#[test]
fn logical_plan_build_reset_clears_go_transient_state() {
    use std::time::Duration;

    let vars = SessionVars::new();
    vars.RegisterScalarSubQ(7_i32);
    vars.InsertExtendedColumnUniqueID("stale", 11);
    vars.RestoreRewritePhaseInfo(RewritePhaseInfo {
        DurationRewrite: Duration::from_secs(1),
        DurationPreprocessSubQuery: Duration::from_secs(2),
        PreprocessSubQueries: 3,
    });

    resetLogicalPlanBuildState(&vars);

    vars.WithScalarSubQueries(|values| assert!(values.is_empty()));
    assert_eq!(vars.ExtendedColumnUniqueID("stale"), None);
    assert_eq!(vars.SnapshotRewritePhaseInfo(), RewritePhaseInfo::default());
}

/// planIDFunc 拒绝非 Plan 值；仅从缓存值中提取合法 Plan ID。
#[test]
fn plan_id_callback_rejects_non_plan_values() {
    assert_eq!(planIDFunc(None), (0, false));
    let value = astersql_sessionctx_stmtctx::cache_value(7_i32);
    assert_eq!(planIDFromCacheValue(&value), None);
}
