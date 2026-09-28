// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Memo 包测试用最小 PlanContext。
//
// 仅为分配计划节点 ID 与内置函数用量计数提供实现；
// 会话变量、表达式求值、range 构建等路径在本包单测中不应触达，
// 触达则 panic，避免引入无关依赖。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// 测试用计划上下文：仅支持递增分配 plan id 与函数用量计数。
struct TestPlanContext(AtomicI32, core_base::BuiltinFunctionUsageCounter);

impl core_base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        // 原子自增，返回新 ID（从 1 起）
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("memo unit tests do not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("memo unit tests do not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("memo unit tests do not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("memo unit tests do not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &core_base::BuildPBContext {
        panic!("memo unit tests do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

/// 构造共享的测试 PlanContext，供本包其它单测初始化逻辑算子。
pub(crate) fn context() -> core_base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        core_base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// 冒烟：连续 alloc_plan_id 得到 1、2。
#[test]
fn TestMain() {
    let context = context();
    assert_eq!(context.alloc_plan_id(), 1);
    assert_eq!(context.alloc_plan_id(), 2);
}
