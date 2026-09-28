// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// 逻辑计划到最优物理任务路由的单元测试。
//
// 验证 `FindBestTask` 经 `InstallFindBestTaskRouter` 安装的路由器后，
// 能把 memo 中包装的规范逻辑计划（此处为 `LogicalLimit`）正确路由到回调。

use crate::{FindBestTask, InstallFindBestTaskRouter};
use logicalop::{LogicalLimit, LogicalPlan};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// 记录路由器是否观察到了包装在 memo GroupExpression 内的 LogicalLimit。
static ROUTED_CANONICAL_PLAN: AtomicBool = AtomicBool::new(false);

/// 仅提供计划 ID 分配的最小 PlanContext；其余上下文访问会 panic。
struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("route test does not access session variables")
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("route test does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("route test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("route test does not run null-reject checks")
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("route test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

/// 构造测试用的 PlanContext 引用。
fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

/// 测试路由器：确认输入是 memo 包装的 LogicalLimit，然后刻意返回错误以终止寻优。
fn record_route(
    plan: &mut dyn LogicalPlan,
    _: &property::PhysicalProperty,
) -> Result<Box<dyn base::Task>, expression::Error> {
    ROUTED_CANONICAL_PLAN.store(
        plan.as_any()
            .downcast_ref::<memo::GroupExpression>()
            .is_some()
            && plan
                .as_any()
                .downcast_ref::<memo::GroupExpression>()
                .expect("memo wrapper")
                .GetWrappedLogicalPlan()
                .as_any()
                .downcast_ref::<LogicalLimit>()
                .is_some(),
        Ordering::SeqCst,
    );
    Err(expression::errors::New("route observed"))
}

/// 安装路由器后，FindBestTask 应路由到规范逻辑计划并触发测试回调。
#[test]
fn find_best_task_routes_the_canonical_logical_plan() {
    let memo = memo::Memo::NewMemo(&[]);
    let expression = memo.NewGroupExpression(
        Box::new(LogicalLimit::default().Init(context(), 0)),
        Vec::new(),
    );
    InstallFindBestTaskRouter(record_route).expect("router is installed once by this test crate");

    let error = FindBestTask(
        &mut *expression.borrow_mut(),
        &property::PhysicalProperty::default(),
    )
    .err()
    .expect("test router deliberately stops after observing the type");
    assert_eq!(error.to_string(), "route observed");
    assert!(ROUTED_CANONICAL_PLAN.load(Ordering::SeqCst));
}
