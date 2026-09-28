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

// 咨询锁（advisory lock）可选求值属性。
//
// 对应 Go `expropt` 中与 `GET_LOCK` / `RELEASE_LOCK` 相关的上下文注入：
// 通过 `OptionalEvalPropProvider` 把会话侧锁操作挂到表达式求值上下文，
// 内置函数再经 `AdvisoryLockPropReader` 取出具体实现。

use std::sync::Arc;

use crate::*;

/// 会话侧咨询锁操作接口；由 session / domain 实现后注入表达式。
pub trait AdvisoryLockContext: Send + Sync {
    /// 按名称获取咨询锁，`timeout` 为秒级等待上限。
    fn get_advisory_lock(&self, name: &str, timeout: i64) -> anyhow::Result<()>;
    /// 查询锁是否被占用，返回占用连接 ID（0 表示未占用）。
    fn is_used_advisory_lock(&self, name: &str) -> u64;
    /// 释放指定名称的咨询锁；成功返回 true。
    fn release_advisory_lock(&self, name: &str) -> bool;
    /// 释放当前会话持有的全部咨询锁，返回释放个数。
    fn release_all_advisory_locks(&self) -> i32;
}

/// 将 `AdvisoryLockContext` 包装为可选求值属性提供者。
pub struct AdvisoryLockPropProvider {
    context: Arc<dyn AdvisoryLockContext>,
}

impl AdvisoryLockPropProvider {
    /// 用任意实现了 `AdvisoryLockContext` 的对象构造提供者。
    pub fn new<T: AdvisoryLockContext + 'static>(context: Arc<T>) -> Self {
        Self { context }
    }
}

impl AdvisoryLockContext for AdvisoryLockPropProvider {
    fn get_advisory_lock(&self, name: &str, timeout: i64) -> anyhow::Result<()> {
        self.context.get_advisory_lock(name, timeout)
    }

    fn is_used_advisory_lock(&self, name: &str) -> u64 {
        self.context.is_used_advisory_lock(name)
    }

    fn release_advisory_lock(&self, name: &str) -> bool {
        self.context.release_advisory_lock(name)
    }

    fn release_all_advisory_locks(&self) -> i32 {
        self.context.release_all_advisory_locks()
    }
}

impl exprctx::OptionalEvalPropProvider for AdvisoryLockPropProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropAdvisoryLock.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 声明依赖 `OptPropAdvisoryLock`，并从上下文取出提供者。
pub struct AdvisoryLockPropReader;

impl RequireOptionalEvalProps for AdvisoryLockPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropAdvisoryLock.AsPropKeySet()
    }
}

impl AdvisoryLockPropReader {
    /// 从可选属性上下文取得咨询锁提供者；缺失则报错。
    pub fn advisory_lock_ctx<'a, C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &'a C,
    ) -> anyhow::Result<&'a AdvisoryLockPropProvider> {
        get_prop_provider(ctx, exprctx::OptPropAdvisoryLock)
    }
}
