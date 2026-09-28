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

// Ranger 建 range 所用上下文：类型/错误/表达式上下文与计划缓存、回退处理钩子。
//
// 对应 Go `RangerContext`。`Detach` 用静态表达式上下文脱离会话，便于并行建 range；
// 计划缓存（Plan Cache）跳过与 range 超限回退经可选 handler 上报。

use std::collections::HashMap;
use std::sync::Arc;

use crate::{contextutil, errctx, exprctx, types};

/// A shared Go `exprctx.BuildContext` interface value.
/// 共享的表达式构建上下文（对齐 Go `exprctx.BuildContext`）。
pub type BuildContextRef = Arc<dyn exprctx::BuildContext>;

// RangerContext is the context used to build range.
/// 构造索引 range 时的上下文：类型、错误级别、表达式、缓存与优化开关。
pub struct RangerContext<'a> {
    /// 类型转换与语句级类型标志上下文。
    pub TypeCtx: types::Context,
    /// 错误处理级别映射（严格/警告等）。
    pub ErrCtx: errctx::Context,
    /// 表达式求值/构建上下文。
    pub ExprCtx: BuildContextRef,
    /// Go embeds pointers here, so detached contexts keep the same handler identity.
    /// Range 超限回退处理器；Detach 后仍共享同一 handler 身份。
    pub RangeFallbackHandler: Option<&'a contextutil::RangeFallbackHandler<'a>>,
    /// 计划缓存跳过跟踪器。
    pub PlanCacheTracker: Option<&'a contextutil::PlanCacheTracker>,
    /// 优化器 fix-control 键值（按 fix id）。
    pub OptimizerFixControl: HashMap<u64, String>,
    /// 是否允许使用 range 相关缓存。
    pub UseCache: bool,
    /// 是否将 NULL 视为可建点 range 的值。
    pub RegardNULLAsPoint: bool,
    /// 前缀索引单次扫描优化开关（影响 IS NULL 等保留策略）。
    pub OptPrefixIndexSingleScan: bool,
}

impl Clone for RangerContext<'_> {
    fn clone(&self) -> Self {
        Self {
            TypeCtx: self.TypeCtx.clone(),
            ErrCtx: self.ErrCtx.clone(),
            ExprCtx: Arc::clone(&self.ExprCtx),
            RangeFallbackHandler: self.RangeFallbackHandler,
            PlanCacheTracker: self.PlanCacheTracker,
            OptimizerFixControl: self.OptimizerFixControl.clone(),
            UseCache: self.UseCache,
            RegardNULLAsPoint: self.RegardNULLAsPoint,
            OptPrefixIndexSingleScan: self.OptPrefixIndexSingleScan,
        }
    }
}

impl<'a> RangerContext<'a> {
    /// 通过 PlanCacheTracker 标记跳过计划缓存并附带原因。
    pub fn SetSkipPlanCache(&self, reason: &str) {
        if let Some(tracker) = self.PlanCacheTracker {
            tracker.SetSkipPlanCache(reason);
        }
    }

    /// 记录因 range 大小超限而回退的情况。
    pub fn RecordRangeFallback(&self, rangeMaxSize: i64) {
        if let Some(handler) = self.RangeFallbackHandler {
            handler.RecordRangeFallback(rangeMaxSize);
        }
    }

    // Detach detaches this context from the session context.
    //
    // NOTE: Though this session context can be used parallelly with this context after calling
    // it, the `StatementContext` cannot. The session context should create a new `StatementContext`
    // before executing another statement.
    //
    /// 脱离会话：换入静态 ExprCtx，并深拷贝 OptimizerFixControl（对齐 maps.Clone）。
    ///
    /// 注意：Detach 后会话可与本上下文并行，但 StatementContext 不可共享，需新建后再执行语句。
    pub fn Detach(&self, staticExprCtx: BuildContextRef) -> Box<RangerContext<'a>> {
        let mut newCtx = self.clone();
        newCtx.ExprCtx = staticExprCtx;
        // Match maps.Clone even if Clone's implementation changes independently later.
        // 显式再 clone map，保证与 Go maps.Clone 一样脱离可变共享。
        newCtx.OptimizerFixControl = self.OptimizerFixControl.clone();
        Box::new(newCtx)
    }
}
