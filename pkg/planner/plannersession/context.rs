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

// 规划器会话扩展上下文：空值拒绝检查、事务预热与只读用户变量。
//
// `PlanCtxExtended` 包装会话上下文，提供 GetNullRejectCheckExprCtx、
// AdviseTxnWarmup，以及只读用户变量映射的存取与 Reset。
// 只读用户变量映射只在一次规划过程中生效，`Reset` 后必须完全清空。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// 表达式上下文在语句生命周期内共享；实现携带规划器使用的求值状态。
/// Expression contexts are shared for a session statement lifetime.
/// Implementations contain the evaluator state used by the planner.
pub trait ExprContext: Send + Sync {}

/// 标记“处于空值拒绝检查”的表达式上下文包装器。
#[derive(Clone)]
pub struct NullRejectCheckExprContext {
    /// 被包装的底层表达式上下文。
    ExprContext: Arc<dyn ExprContext>,
}

impl NullRejectCheckExprContext {
    /// 返回底层表达式上下文的克隆引用。
    pub fn ExprContext(&self) -> Arc<dyn ExprContext> {
        Arc::clone(&self.ExprContext)
    }

    /// 恒为 true：表示当前处于 null-reject 检查路径。
    pub fn IsInNullRejectCheck(&self) -> bool {
        true
    }
}

/// 将普通 ExprContext 包装为 NullRejectCheckExprContext。
pub fn WithNullRejectCheck(ctx: Arc<dyn ExprContext>) -> NullRejectCheckExprContext {
    NullRejectCheckExprContext { ExprContext: ctx }
}

/// 规划器会话扩展层错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannerSessionError {
    /// 人类可读错误信息。
    pub Message: String,
}

impl PlannerSessionError {
    /// 由任意可转 String 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            Message: message.into(),
        }
    }
}

impl fmt::Display for PlannerSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.Message)
    }
}

impl std::error::Error for PlannerSessionError {}

/// 规划器扩展所需的两项会话操作；生产会话将 AdviseTxnWarmup 委托给事务管理器。
/// The two session operations used by the planner extension.
/// A production session delegates `AdviseTxnWarmup` to its transaction manager.
pub trait SessionContext: Send + Sync {
    /// 获取会话表达式上下文。
    fn GetExprCtx(&self) -> Arc<dyn ExprContext>;
    /// 建议事务预热（提前准备事务相关资源）。
    fn AdviseTxnWarmup(&self) -> Result<(), PlannerSessionError>;
}

/// 挂在会话上的规划扩展状态：null-reject 上下文与只读用户变量。
pub struct PlanCtxExtended {
    /// 底层会话上下文。
    sctx: Arc<dyn SessionContext>,
    /// 构造时固定的 null-reject 表达式上下文包装。
    nullRejectCheckExprCtx: NullRejectCheckExprContext,
    /// 只读用户变量名集合；None 表示未设置。
    readonlyUserVars: Option<HashMap<String, ()>>,
}

/// 由会话上下文构造 PlanCtxExtended，并固定 null-reject 包装。
pub fn NewPlanCtxExtended(sctx: Arc<dyn SessionContext>) -> PlanCtxExtended {
    let null_reject_check_expr_ctx = WithNullRejectCheck(sctx.GetExprCtx());
    PlanCtxExtended {
        sctx,
        nullRejectCheckExprCtx: null_reject_check_expr_ctx,
        readonlyUserVars: None,
    }
}

impl PlanCtxExtended {
    /// 返回 null-reject 表达式上下文；debug 下断言底层 ExprContext 未在构造后被替换。
    pub fn GetNullRejectCheckExprCtx(&self) -> &NullRejectCheckExprContext {
        // This mirrors the Go intest assertion: replacing the expression
        // context after construction would make planner state inconsistent.
        // 对应 Go intest 断言：构造后替换表达式上下文会使规划状态不一致。
        debug_assert!(Arc::ptr_eq(
            &self.nullRejectCheckExprCtx.ExprContext,
            &self.sctx.GetExprCtx(),
        ));
        &self.nullRejectCheckExprCtx
    }

    /// 委托会话做事务预热。
    pub fn AdviseTxnWarmup(&self) -> Result<(), PlannerSessionError> {
        self.sctx.AdviseTxnWarmup()
    }

    /// 设置只读用户变量名映射。
    pub fn SetReadonlyUserVarMap(&mut self, readonlyUserVars: HashMap<String, ()>) {
        self.readonlyUserVars = Some(readonlyUserVars);
    }

    /// 获取只读用户变量名映射。
    pub fn GetReadonlyUserVarMap(&self) -> Option<&HashMap<String, ()>> {
        self.readonlyUserVars.as_ref()
    }

    /// 清除只读用户变量映射。
    pub fn Reset(&mut self) {
        self.readonlyUserVars = None;
    }
}
