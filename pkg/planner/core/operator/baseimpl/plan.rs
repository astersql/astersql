// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 逻辑/物理算子共用的基础 `Plan` 实现。
//
// 持有规划上下文、算子类型名 `tp`、唯一 ID、查询块偏移与统计信息指针；
// 计划缓存（plan cache）克隆默认拒绝，由具体算子显式覆盖。

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

/// 共享规划上下文，对应 Go 的接口值。
/// Shared planning context corresponding to Go's interface value.
pub type PlanContextRef = base::ContextRef;

/// 被逻辑/物理算子内嵌，承载公共元数据。
/// `Plan` is embedded by logical and physical operators and owns their common metadata.
#[derive(Clone)]
pub struct Plan {
    ctx: PlanContextRef,
    // Go 的计划缓存克隆故意共享该统计信息指针。
    // Go's plan-cache clone deliberately shares this pointer.
    stats: Option<Arc<property::StatsInfo>>,
    tp: String,
    id: i32,
    qb_block: i32,
    pub NoncacheableReason: String,
}

struct ExplainId<'a>(&'a Plan);

impl Display for ExplainId<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        if self.0.ctx.ignore_explain_id_suffix() {
            formatter.write_str(&self.0.tp)
        } else {
            write!(formatter, "{}_{}", self.0.tp, self.0.id)
        }
    }
}

/// 创建基础计划，并从会话级上下文分配下一个计划 ID。
/// Creates a base plan and allocates the next ID from the session-scoped context.
pub fn NewBasePlan(ctx: PlanContextRef, tp: impl Into<String>, qb_block: i32) -> Plan {
    let id = ctx.alloc_plan_id();
    Plan {
        ctx,
        stats: None,
        tp: tp.into(),
        id,
        qb_block,
        NoncacheableReason: String::new(),
    }
}

impl Plan {
    /// 为 Cascades 优化器重置类型与 ID，保留上下文与查询块归属。
    /// Reinitializes the cascades identity while preserving context and query-block ownership.
    pub fn ReAlloc4Cascades(&mut self, tp: impl Into<String>) {
        self.tp = tp.into();
        self.id = self.ctx.alloc_plan_id();
        self.stats = None;
    }

    /// 返回规划会话上下文。
    pub fn SCtx(&self) -> &PlanContextRef {
        &self.ctx
    }

    /// 替换规划会话上下文。
    pub fn SetSCtx(&mut self, ctx: PlanContextRef) {
        self.ctx = ctx;
    }

    /// 基类不持有输出列名；产生 Schema 的算子会覆盖此行为。
    /// Base plans do not own output names; schema-producing operators override this behavior.
    pub fn OutputNames(&self) -> types::metadata::NameSlice {
        types::metadata::NameSlice(Vec::new())
    }

    /// 基类忽略输出列名设置。
    pub fn SetOutputNames(&mut self, _names: types::metadata::NameSlice) {}

    /// 基类无表达式列可替换。
    pub fn ReplaceExprColumns(&mut self, _replace: &HashMap<String, expression::Column>) {}

    /// 返回计划节点唯一 ID。
    pub fn ID(&self) -> i32 {
        self.id
    }

    /// 设置计划节点 ID。
    pub fn SetID(&mut self, id: i32) {
        self.id = id;
    }

    /// 返回代价估计用的统计信息（StatsInfo）。
    pub fn StatsInfo(&self) -> Option<&property::StatsInfo> {
        self.stats.as_deref()
    }

    /// 基类 Explain 附加信息占位为 `N/A`。
    pub fn ExplainInfo(&self) -> String {
        "N/A".to_owned()
    }

    /// 生成 Explain 中的算子标识（类型名或 `类型_ID`）。
    pub fn ExplainID(&self, _is_child_of_inl: &[bool]) -> Box<dyn Display + '_> {
        Box::new(ExplainId(self))
    }

    /// 返回算子类型名（TP）。
    pub fn TP(&self, _normalized: &[bool]) -> String {
        self.tp.clone()
    }

    /// 设置算子类型名。
    pub fn SetTP(&mut self, tp: impl Into<String>) {
        self.tp = tp.into();
    }

    /// 返回查询块（query block）偏移。
    pub fn QueryBlockOffset(&self) -> i32 {
        self.qb_block
    }

    /// 更新查询块归属；物理算子的 `Init` 应复用构造阶段已分配的计划身份。
    pub fn SetQueryBlockOffset(&mut self, qb_block: i32) {
        self.qb_block = qb_block;
    }

    /// 绑定统计信息。
    pub fn SetStats(&mut self, stats: Option<Arc<property::StatsInfo>>) {
        self.stats = stats;
    }

    /// 估算本节点元数据内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        PlanSize + self.tp.len() as i64
    }

    /// 克隆元数据并浅共享统计信息，仅替换规划上下文。
    /// Clones all metadata and shallow-shares statistics, replacing only the planning context.
    pub fn CloneWithNewCtx(&self, new_ctx: PlanContextRef) -> Plan {
        let mut cloned = self.clone();
        cloned.ctx = new_ctx;
        cloned
    }

    /// 基类拒绝计划缓存克隆；具体算子需显式支持。
    /// Base plans reject plan-cache cloning; concrete operators opt in explicitly.
    pub fn CloneForPlanCache(
        &self,
        _new_ctx: PlanContextRef,
    ) -> (Option<Box<dyn base::Plan>>, bool) {
        (None, false)
    }

    /// 保留首次不可缓存原因，标识最早的可缓存性违规。
    /// Preserves the first reason because it identifies the earliest cacheability violation.
    pub fn SetNoncacheableReason(&mut self, reason: impl Into<String>) {
        if self.NoncacheableReason.is_empty() {
            self.NoncacheableReason = reason.into();
        }
    }

    /// 返回不可放入计划缓存的原因。
    pub fn GetNoncacheableReason(&self) -> String {
        self.NoncacheableReason.clone()
    }
}

/// Rust 基类 Plan 的静态大小，对应 Go 的 `unsafe.Sizeof(Plan{})`。
/// Static size of the Rust base-plan value, corresponding to Go's `unsafe.Sizeof(Plan{})`.
pub const PlanSize: i64 = std::mem::size_of::<Plan>() as i64;
