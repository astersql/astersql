// Copyright 2026 AsterSQL.

// 规划器相关语句状态的迁移期单元测试。
//
// 覆盖 `StatementHints` 的 ForceNthPlan / WriteSlowLog 原子交换语义，
// 以及备选逻辑计划标志与已用统计加载状态的共享读写。

use super::*;

/// 验证 hint 状态：ForceNthPlan 交换/还原与 WriteSlowLog 存取语义。
#[test]
fn planner_hint_state_preserves_swap_and_restore_semantics() {
    let context = NewStmtCtx();
    assert_eq!(context.StmtHints.ForceNthPlan(), -1);
    assert!(!context.StmtHints.WriteSlowLog());
    assert_eq!(context.StmtHints.SwapForceNthPlan(7), -1);
    assert_eq!(context.StmtHints.SwapForceNthPlan(-1), 7);
    context.StmtHints.StoreWriteSlowLog(true);
    assert!(context.StmtHints.WriteSlowLog());
    context.StmtHints.StoreWriteSlowLog(false);
    assert!(!context.StmtHints.WriteSlowLog());
}

/// 验证备选逻辑计划偏好标志与 UsedStats 加载状态可在上下文中共享读写。
#[test]
fn planner_alternative_and_stats_state_is_shared() {
    let context = NewStmtCtx();
    assert!(!context.AlternativeLogicalPlanPreferCorrelate());
    context.MarkAlternativeLogicalPlanPreferCorrelate();
    assert!(context.AlternativeLogicalPlanPreferCorrelate());

    context.RecordUsedStatsLoadStatus(1, 2, true, "loaded".to_owned());
    assert_eq!(
        context.UsedStatsLoadStatus().get(&(1, 2, true)),
        Some(&"loaded".to_owned())
    );
}
