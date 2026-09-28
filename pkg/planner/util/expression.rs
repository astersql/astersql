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

// 规划器侧表达式回调注入点。
//
// 在 PlanContext（规划会话上下文）下求值或改写 AST 表达式时，
// 通过包级可写回调打破 crate 循环依赖：上层在启动时注册实现，
// 本模块只持有类型擦除后的函数指针槽位。

use std::sync::{Arc, RwLock};

/// 在规划上下文中求值 AST 表达式，得到 Datum（运行期标量值）。
pub type EvalAstExprWithPlanCtxFn = dyn Fn(
        &dyn plan_base::PlanContext,
        &dyn parser_ast::ast::ExprNode,
    ) -> Result<expression::types::Datum, expression::Error>
    + Send
    + Sync;

/// 在规划上下文中按 Schema / 列名切片改写 AST，得到可执行表达式盒子。
pub type RewriteAstExprWithPlanCtxFn = dyn Fn(
        &dyn plan_base::PlanContext,
        &dyn parser_ast::ast::ExprNode,
        &expression::Schema,
        types::metadata::NameSlice,
        bool,
    ) -> Result<expression::ExprBox, expression::Error>
    + Send
    + Sync;

/// 已注册的 AST 求值回调；未安装时为 None。
pub static EvalAstExprWithPlanCtx: RwLock<Option<Arc<EvalAstExprWithPlanCtxFn>>> =
    RwLock::new(None);
/// 已注册的 AST 改写回调；未安装时为 None。
pub static RewriteAstExprWithPlanCtx: RwLock<Option<Arc<RewriteAstExprWithPlanCtxFn>>> =
    RwLock::new(None);

/// 安装 AST 求值回调（覆盖此前注册）。
pub fn SetEvalAstExprWithPlanCtx(callback: Arc<EvalAstExprWithPlanCtxFn>) {
    *EvalAstExprWithPlanCtx
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(callback);
}

/// 安装 AST 改写回调（覆盖此前注册）。
pub fn SetRewriteAstExprWithPlanCtx(callback: Arc<RewriteAstExprWithPlanCtxFn>) {
    *RewriteAstExprWithPlanCtx
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(callback);
}
