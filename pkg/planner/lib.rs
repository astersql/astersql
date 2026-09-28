// Copyright 2026 AsterSQL.

// 优化器（Planner）crate 根：再导出逻辑/物理计划优化入口与会话服务。
//
// 执行计划（execution plan）是优化器把 SQL 编译成可执行算子树的结果；
// 本模块把 `optimize` 子模块中的 Optimize 相关类型与回调安装函数暴露给上层。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 逻辑与物理计划优化核心实现（会话服务、计划缓存、诊断运行时等）。
mod optimize;
/// 再导出 Optimize 入口、会话服务、计划摘要与代价查询等公共 API。
pub use optimize::{
    DefaultOptimizeSessionService, InstallDefaultOptimizeCallbacks, InstallOptimizeCallbacks,
    InstallPlannerDiagnosticRuntime, InstallReadOnlyAdmissionCallbacks,
    LogicalPlanSessionStateService, MatchedPlanBinding, NonPreparedPlanCacheService, Optimize,
    OptimizeAstNode, OptimizeAstNodeNoCache, OptimizeExecStmt, OptimizeForForeignKeyCascade,
    OptimizeRuntimeService, OptimizeSessionService, PlannerDiagnosticRuntime,
    calculatePlanDigestFunc, genBriefPlanWithSCtx, queryPlanCost, recordRelevantOptVarsAndFixes,
};
