// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// MaxOneRow 物理算子：保证子计划最多输出一行。
//
// 常用于标量子查询（scalar subquery）：若返回超过一行则运行时报错；
// 枚举物理计划时会把子节点 ExpectedCnt 设为 2，以便统计与代价感知“最多两行探测”。

use base::{ContextRef, PhysicalPlan};
use logicalop::LogicalPlan as _;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 对应 Go `PhysicalMaxOneRow`，本身无额外状态，仅包装 Schema 生产者。
pub struct PhysicalMaxOneRow {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
}

impl PhysicalMaxOneRow {
    /// 构造 TypeMaxOneRow 空壳算子。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeMaxOneRow,
                0,
            )),
        }
    }

    /// 绑定统计信息、查询块偏移，并登记唯一子节点的物理属性需求。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        child: property::PhysicalProperty,
    ) -> Self {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetSCtx(ctx);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetTP(plancodec::TypeMaxOneRow);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetQueryBlockOffset(offset);
        base::PhysicalPlan::set_stats(&mut self.PhysicalSchemaProducer.BasePhysicalPlan, stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(vec![Box::new(child)]);
        self
    }

    /// 克隆到新上下文并复制 Schema。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
        })
    }

    /// 估算内存占用（委托 Schema 生产者）。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
    }

    /// EXPLAIN 无额外字段，返回空串。
    pub fn ExplainInfo(&self) -> String {
        String::new()
    }
}

/// 从 LogicalMaxOneRow 枚举物理候选：不接受排序项或 Flash/MPP 属性。
pub fn ExhaustPhysicalPlans4LogicalMaxOneRow(
    logical: &logicalop::LogicalMaxOneRow,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    // 需要有序输出或 TiFlash 属性时无法实现 MaxOneRow。
    if !required.IsSortItemEmpty() || required.IsFlashProp() {
        if let Some(ctx) = logical.SCtx() {
            ctx.GetSessionVars().RaiseWarningWhenMPPEnforced(
                "MPP mode may be blocked because operator `MaxOneRow` is not supported now.",
            );
        }
        return Vec::new();
    }
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let mut child = property::PhysicalProperty::default();
    // 期望子节点最多探测 2 行：第 2 行用于触发“多于一行”错误路径。
    child.ExpectedCnt = 2.0;
    child.CTEProducerStatus = required.CTEProducerStatus;
    child.NoCopPushDown = required.NoCopPushDown;
    let mut plan = PhysicalMaxOneRow::New(ctx.clone());
    plan.PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    vec![Box::new(plan.Init(
        ctx,
        logical.StatsInfo().cloned().unwrap_or_default(),
        logical.QueryBlockOffset(),
        child,
    ))]
}
