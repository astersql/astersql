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

// 物理 TableDual 算子：无真实表扫描的“虚表”，只产出固定行数（常为 0 或 1）。
//
// 对应 MySQL DUAL / 无 FROM 子句常量查询；优化器用它表示不依赖存储的行源。

use base::{ContextRef, PhysicalPlan};
use logicalop::LogicalPlan as _;
use types::metadata::NameSlice;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 物理 Dual：Schema 生产者 + 产出行数 `RowCount`。
pub struct PhysicalTableDual {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 虚表产出的行数（通常 0 表示空结果，1 表示单行常量投影）。
    pub RowCount: i32,
    // Dual may be initialized while building a point-get plan, so it owns names itself.
    names: NameSlice,
}

impl PhysicalTableDual {
    pub(crate) fn cache_names(&self) -> &NameSlice {
        &self.names
    }

    pub(crate) fn restore_cached_names(&mut self, names: NameSlice) {
        self.names = names;
    }

    /// 按上下文与行数构造 TypeDual 物理节点。
    pub fn New(ctx: ContextRef, row_count: i32) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeDual,
                0,
            )),
            RowCount: row_count,
            names: NameSlice(Vec::new()),
        }
    }

    /// 绑定统计信息与查询块偏移后返回自身。
    pub fn Init(mut self, ctx: ContextRef, stats: property::StatsInfo, offset: i32) -> Self {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetSCtx(ctx);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetTP(plancodec::TypeDual);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetQueryBlockOffset(offset);
        base::PhysicalPlan::set_stats(&mut self.PhysicalSchemaProducer.BasePhysicalPlan, stats);
        self
    }

    /// 克隆 Schema 生产者与行数到新会话上下文。
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
            RowCount: self.RowCount,
            names: NameSlice(
                self.names
                    .0
                    .iter()
                    .map(|name| name.as_ref().map(|name| std::sync::Arc::new(name.Clone())))
                    .collect(),
            ),
        })
    }

    /// 返回 Dual 自身持有的输出字段名。
    pub fn OutputNames(&self) -> NameSlice {
        self.names.Shallow()
    }

    /// 设置 Dual 自身持有的输出字段名。
    pub fn SetOutputNames(&mut self, names: NameSlice) {
        self.names = names;
    }

    /// EXPLAIN 输出形如 `rows:N`。
    pub fn ExplainInfo(&self) -> String {
        format!("rows:{}", self.RowCount)
    }

    /// 估算内存：Schema 生产者 + RowCount 字段。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + std::mem::size_of::<i32>() as i64
            + std::mem::size_of::<Vec<usize>>() as i64
            + self.names.0.capacity() as i64 * std::mem::size_of::<usize>() as i64
            + self
                .names
                .0
                .iter()
                .flatten()
                .map(|name| name.MemoryUsage())
                .sum::<i64>()
    }
}

/// 由逻辑 TableDual 生成物理 Dual；有 IndexJoin 属性或需排序且多行时放弃。
pub fn ExhaustPhysicalPlans4LogicalTableDual(
    logical: &logicalop::LogicalTableDual,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    // IndexJoin 属性或“多行 + 排序要求”无法由 Dual 直接满足。
    if required.IndexJoinProp.is_some() || (!required.IsSortItemEmpty() && logical.RowCount > 1) {
        return Vec::new();
    }
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let mut dual = PhysicalTableDual::New(ctx.clone(), logical.RowCount);
    dual.PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    vec![Box::new(dual.Init(
        ctx,
        logical.StatsInfo().cloned().unwrap_or_default(),
        logical.QueryBlockOffset(),
    ))]
}
