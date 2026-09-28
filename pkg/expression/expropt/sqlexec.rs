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

// SQL 受限执行器（SQLExecutor）的可选求值属性（Optional Eval Prop）。
//
// 将“在表达式求值期间执行受限 SQL”能力挂到 `EvalContext`：
// Provider 负责供给执行器，Reader 从上下文取出。对应 Go 的
// `sqlexec.SQLExecutor` 窄接口，避免把完整 SQL/KV 实现拉进本叶子包。

use std::any::Any;
use std::sync::Arc;

use crate::*;

/// The same narrow method exposed by Go's SQLExecutor interface.
///
/// Associated types bind directly to context.Context, sqlexec.OptionFuncAlias,
/// chunk.Row, and resolve.ResultField in the package integration task without
/// pulling the native SQL/KV implementation into this leaf harness.
///
/// 与 Go `SQLExecutor` 对齐的窄接口：仅暴露 `exec_restricted_sql`。
/// 关联类型对应 Context、OptionFuncAlias、Row、ResultField，由上层集成绑定。
pub trait SQLExecutor {
    type Context;
    type OptionFuncAlias;
    type Row;
    type ResultField;

    /// 在受限模式下执行 SQL，返回行集与结果列元数据（ResultField）。
    fn exec_restricted_sql(
        &self,
        ctx: &Self::Context,
        opts: &[Self::OptionFuncAlias],
        sql: &str,
        args: &[Box<dyn Any>],
    ) -> anyhow::Result<(Vec<Self::Row>, Vec<Self::ResultField>)>;
}

/// 可选属性 Provider：惰性工厂，按需构造可共享的 `SQLExecutor`。
pub struct SQLExecutorPropProvider<T: SQLExecutor + Send + Sync + 'static> {
    provider: Box<dyn Fn() -> anyhow::Result<Arc<T>> + Send + Sync>,
}

impl<T: SQLExecutor + Send + Sync + 'static> SQLExecutorPropProvider<T> {
    /// 用闭包工厂构造 Provider；失败时由 `call` 向上传递错误。
    pub fn new<F>(provider: F) -> Self
    where
        F: Fn() -> anyhow::Result<Arc<T>> + Send + Sync + 'static,
    {
        Self {
            provider: Box::new(provider),
        }
    }

    /// 调用工厂取得一次 `Arc<T>` 执行器实例。
    pub fn call(&self) -> anyhow::Result<Arc<T>> {
        (self.provider)()
    }
}

impl<T: SQLExecutor + Send + Sync + 'static> exprctx::OptionalEvalPropProvider
    for SQLExecutorPropProvider<T>
{
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropSQLExecutor.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 可选属性 Reader：声明依赖 `OptPropSQLExecutor` 并从求值上下文取出执行器。
pub struct SQLExecutorPropReader;

impl RequireOptionalEvalProps for SQLExecutorPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropSQLExecutor.AsPropKeySet()
    }
}

impl SQLExecutorPropReader {
    /// 从可选属性上下文取得 `SQLExecutor`；缺失或类型不匹配则返回错误。
    pub fn get_sql_executor<T, C>(&self, ctx: &C) -> anyhow::Result<Arc<T>>
    where
        T: SQLExecutor + Send + Sync + 'static,
        C: OptionalEvalPropContext + ?Sized,
    {
        let provider =
            get_prop_provider::<SQLExecutorPropProvider<T>, _>(ctx, exprctx::OptPropSQLExecutor)?;
        provider.call()
    }
}
