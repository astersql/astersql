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

// 计划缓存（Plan Cache）状态跟踪与 range 回退告警。
//
// 计划缓存复用已生成的执行计划以降低优化开销；本模块跟踪是否可用缓存、
// 缓存类型（预处理/非预处理）、强制缓存与跳过原因，并在 IN 列表过长时
// 通过 `RangeFallbackHandler` 降级告警。

#![allow(dead_code, non_snake_case)]

use crate::errors;
use crate::warn::WarnAppender;
use std::sync::{Arc, Mutex, Once};

/// 计划缓存类型：默认不缓存、会话预处理、会话非预处理。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanCacheType {
    /// 未启用或未知缓存类型。
    DefaultNoCache = 0,
    /// 会话级预处理语句计划缓存。
    SessionPrepared = 1,
    /// 会话级非预处理语句计划缓存。
    SessionNonPrepared = 2,
}

/// 计划缓存可变状态快照字段集合（受 Mutex 保护）。
#[derive(Clone)]
struct PlanCacheState {
    useCache: bool,
    cacheType: PlanCacheType,
    planCacheUnqualified: String,
    forcePlanCache: bool,
    alwaysWarnSkipCache: bool,
}

/// 线程安全的计划缓存跟踪器：维护启用标志并经 WarnAppender 报告跳过原因。
pub struct PlanCacheTracker {
    state: Mutex<PlanCacheState>,
    warnHandler: Arc<dyn WarnAppender + Send + Sync>,
}

impl PlanCacheTracker {
    /// 在已有缓存类型时仅发出跳过告警，不强制改写 useCache。
    pub fn WarnSkipPlanCache(&self, reason: &str) {
        let mut state = self
            .state
            .lock()
            .expect("plan cache tracker mutex poisoned");
        if state.cacheType != PlanCacheType::DefaultNoCache {
            self.warnSkipPlanCache(&mut state, reason);
        }
    }

    /// 标记跳过计划缓存：force 模式下只告警；否则关闭 useCache 并告警。
    pub fn SetSkipPlanCache(&self, reason: &str) {
        let mut state = self
            .state
            .lock()
            .expect("plan cache tracker mutex poisoned");
        // 已处于不可用状态则无需重复处理。
        if !state.useCache {
            return;
        }
        // 强制缓存：保留 useCache，仅警告可能使用风险计划。
        if state.forcePlanCache {
            self.warnHandler
                .AppendWarning(errors::NewNoStackError(format!(
                    "force plan-cache: may use risky cached plan: {reason}"
                )));
            return;
        }
        state.useCache = false;
        self.warnSkipPlanCache(&mut state, reason);
    }

    /// 按 cacheType 写入不合格原因并追加对应 skip 告警文案。
    fn warnSkipPlanCache(&self, state: &mut PlanCacheState, reason: &str) {
        state.planCacheUnqualified = reason.to_owned();
        match state.cacheType {
            PlanCacheType::DefaultNoCache => self
                .warnHandler
                .AppendWarning(errors::NewNoStackError("unknown cache type")),
            PlanCacheType::SessionPrepared => {
                self.warnHandler
                    .AppendWarning(errors::NewNoStackError(format!(
                        "skip prepared plan-cache: {reason}"
                    )))
            }
            PlanCacheType::SessionNonPrepared if state.alwaysWarnSkipCache => self
                .warnHandler
                .AppendWarning(errors::NewNoStackError(format!(
                    "skip non-prepared plan-cache: {reason}"
                ))),
            PlanCacheType::SessionNonPrepared => {}
        }
    }

    /// 非预处理路径是否总是对 skip 发出告警。
    pub fn SetAlwaysWarnSkipCache(&self, alwaysWarnSkipCache: bool) {
        self.state
            .lock()
            .expect("plan cache tracker mutex poisoned")
            .alwaysWarnSkipCache = alwaysWarnSkipCache;
    }

    /// 设置当前计划缓存类型。
    pub fn SetCacheType(&self, cacheType: PlanCacheType) {
        self.state
            .lock()
            .expect("plan cache tracker mutex poisoned")
            .cacheType = cacheType;
    }

    /// 是否强制继续使用计划缓存（跳过时只告警不关闭）。
    pub fn SetForcePlanCache(&self, forcePlanCache: bool) {
        self.state
            .lock()
            .expect("plan cache tracker mutex poisoned")
            .forcePlanCache = forcePlanCache;
    }

    /// 启用计划缓存（将 useCache 置真）。
    pub fn EnablePlanCache(&self) {
        self.state
            .lock()
            .expect("plan cache tracker mutex poisoned")
            .useCache = true;
    }

    /// 导出当前五元组状态，供嵌套逻辑 Save/Restore。
    pub fn Save(&self) -> (bool, PlanCacheType, String, bool, bool) {
        let state = self
            .state
            .lock()
            .expect("plan cache tracker mutex poisoned");
        (
            state.useCache,
            state.cacheType,
            state.planCacheUnqualified.clone(),
            state.forcePlanCache,
            state.alwaysWarnSkipCache,
        )
    }

    /// 用给定五元组整体覆盖内部状态。
    pub fn Restore(
        &self,
        useCache: bool,
        cacheType: PlanCacheType,
        planCacheUnqualified: String,
        forcePlanCache: bool,
        alwaysWarnSkipCache: bool,
    ) {
        *self
            .state
            .lock()
            .expect("plan cache tracker mutex poisoned") = PlanCacheState {
            useCache,
            cacheType,
            planCacheUnqualified,
            forcePlanCache,
            alwaysWarnSkipCache,
        };
    }

    /// 当前是否应使用计划缓存。
    pub fn UseCache(&self) -> bool {
        self.state
            .lock()
            .expect("plan cache tracker mutex poisoned")
            .useCache
    }

    /// 返回计划缓存不合格/跳过原因文案。
    pub fn PlanCacheUnqualified(&self) -> String {
        self.state
            .lock()
            .expect("plan cache tracker mutex poisoned")
            .planCacheUnqualified
            .clone()
    }
}

/// 构造默认关闭缓存的 `PlanCacheTracker`，绑定告警追加器。
pub fn NewPlanCacheTracker(warnHandler: Arc<dyn WarnAppender + Send + Sync>) -> PlanCacheTracker {
    PlanCacheTracker {
        state: Mutex::new(PlanCacheState {
            useCache: false,
            cacheType: PlanCacheType::DefaultNoCache,
            planCacheUnqualified: String::new(),
            forcePlanCache: false,
            alwaysWarnSkipCache: false,
        }),
        warnHandler,
    }
}

/// IN 列表过长导致 range 构建回退时的处理器：禁用缓存且告警至多一次。
pub struct RangeFallbackHandler<'a> {
    planCacheTracker: &'a PlanCacheTracker,
    warnHandler: &'a dyn WarnAppender,
    reportRangeFallbackWarning: Once,
}

impl RangeFallbackHandler<'_> {
    /// 记录 range 回退：始终 SetSkipPlanCache，告警文案仅首次发出。
    pub fn RecordRangeFallback(&self, rangeMaxSize: i64) {
        self.planCacheTracker
            .SetSkipPlanCache("in-list is too long");
        // Once 保证同一 handler 生命周期内只报告一次容量超限告警。
        self.reportRangeFallbackWarning.call_once(|| {
            self.warnHandler.AppendWarning(errors::NewNoStackError(format!(
                "Memory capacity of {rangeMaxSize} bytes for 'tidb_opt_range_max_size' exceeded when building ranges. Less accurate ranges such as full range are chosen"
            )));
        });
    }
}

/// 构造绑定 tracker 与 warnHandler 的 `RangeFallbackHandler`。
pub fn NewRangeFallbackHandler<'a>(
    planCacheTracker: &'a PlanCacheTracker,
    warnHandler: &'a dyn WarnAppender,
) -> RangeFallbackHandler<'a> {
    RangeFallbackHandler {
        planCacheTracker,
        warnHandler,
        reportRangeFallbackWarning: Once::new(),
    }
}
