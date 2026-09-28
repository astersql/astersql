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

// Schema 生产者：为物理/简单计划节点缓存并暴露输出列结构（Schema）。
//
// Schema 描述算子输出的列集合；多数物理算子通过本模块统一读写，避免每个算子各自维护。

use base::ContextRef;
use baseimpl::{NewBasePlan, Plan};
use expression::Schema;
use std::sync::Arc;
use types::metadata::NameSlice;

use crate::BasePhysicalPlan;

/// Stores a schema for physical operators which produce one directly.
/// 物理算子侧的 Schema 生产者：内嵌 `BasePhysicalPlan`，并按需惰性生成或显式设置 Schema。
pub struct PhysicalSchemaProducer {
    schema: Option<Arc<Schema>>,
    pub BasePhysicalPlan: BasePhysicalPlan,
}

impl PhysicalSchemaProducer {
    /// 用已有基础物理计划构造生产者，Schema 初始为空，待首次访问或显式设置。
    pub fn New(base: BasePhysicalPlan) -> Self {
        Self {
            schema: None,
            BasePhysicalPlan: base,
        }
    }

    /// 返回当前 Schema；若尚未设置则按孩子数量惰性推导。
    pub fn Schema(&mut self) -> &Schema {
        if self.schema.is_none() {
            // 单孩子时复用孩子输出 Schema；多孩子或无孩子时退化为空 Schema。
            self.schema = if self.BasePhysicalPlan.Children().len() == 1 {
                Some(Arc::new(
                    self.BasePhysicalPlan.Children()[0].schema().Clone(),
                ))
            } else {
                Some(Arc::new(expression::NewSchema(Vec::new())))
            };
        }
        self.schema.as_deref().expect("schema initialized")
    }

    /// 只读查看已缓存的 Schema，不触发惰性初始化。
    pub fn SchemaRef(&self) -> Option<&Schema> {
        self.schema.as_deref()
    }

    /// 显式写入输出 Schema，覆盖惰性推导结果。
    pub fn SetSchema(&mut self, schema: Schema) {
        self.schema = Some(Arc::new(schema));
    }

    /// 将表达式列下标解析委托给基础物理计划（ResolveIndices：把列引用映射到孩子 Schema 位置）。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.BasePhysicalPlan.ResolveIndices()
    }

    /// 按 Go 口径估算本节点内存：基础计划 + Schema 指针槽位。
    pub fn MemoryUsage(&self) -> i64 {
        self.BasePhysicalPlan.MemoryUsage() + std::mem::size_of::<usize>() as i64
    }

    /// 为计划缓存克隆：与 Go 一样共享 Schema，并用新会话上下文重建基础计划。
    pub fn CloneForPlanCacheWithSelf(&self, new_ctx: ContextRef) -> Option<Self> {
        Some(Self {
            schema: self.schema.clone(),
            BasePhysicalPlan: self.BasePhysicalPlan.CloneWithNewCtx(new_ctx).ok()?,
        })
    }

    /// 普通克隆：先确保 Schema 已初始化再拷贝，失败时向上传播错误。
    pub fn CloneWithSelf(&mut self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let schema = self.Schema().Clone();
        Ok(Self {
            schema: Some(Arc::new(schema)),
            BasePhysicalPlan: self.BasePhysicalPlan.CloneWithNewCtx(new_ctx)?,
        })
    }
}

/// Schema producer used by simple non-physical plans.
/// 简单（非物理）计划侧的 Schema 生产者：额外维护输出列名（NameSlice）。
pub struct SimpleSchemaProducer {
    schema: Option<Arc<Schema>>,
    names: NameSlice,
    pub Plan: Plan,
}

impl SimpleSchemaProducer {
    /// 按会话上下文、计划类型与查询块偏移构造空 Schema/列名的简单生产者。
    pub fn New(ctx: ContextRef, tp: impl Into<String>, offset: i32) -> Self {
        Self {
            schema: None,
            names: NameSlice(Vec::new()),
            Plan: NewBasePlan(ctx, tp, offset),
        }
    }

    /// 为计划缓存浅拷贝列名并共享 Schema，用新上下文重建底层 Plan。
    pub fn CloneSelfForPlanCache(&self, new_ctx: ContextRef) -> Self {
        Self {
            schema: self.schema.clone(),
            names: self.names.Shallow(),
            Plan: self.Plan.CloneWithNewCtx(new_ctx),
        }
    }

    /// 返回输出列名的浅拷贝。
    pub fn OutputNames(&self) -> NameSlice {
        self.names.Shallow()
    }

    /// 设置输出列名。
    pub fn SetOutputNames(&mut self, names: NameSlice) {
        self.names = names;
    }

    /// 返回 Schema；未设置时惰性创建空 Schema。
    pub fn Schema(&mut self) -> &Schema {
        &*self
            .schema
            .get_or_insert_with(|| Arc::new(expression::NewSchema(Vec::new())))
    }

    /// 只读查看已缓存 Schema。
    pub fn SchemaRef(&self) -> Option<&Schema> {
        self.schema.as_deref()
    }

    /// 显式设置 Schema。
    pub fn SetSchema(&mut self, schema: Schema) {
        self.schema = Some(Arc::new(schema));
    }

    /// 同时写入 Schema 与输出列名，保持二者一致。
    pub fn SetSchemaAndNames(&mut self, schema: Schema, names: NameSlice) {
        self.schema = Some(Arc::new(schema));
        self.names = names;
    }

    /// 按 Go 字段口径估算底层 Plan、Schema、列名切片及各列名占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.Plan.MemoryUsage()
            + std::mem::size_of::<usize>() as i64
            + std::mem::size_of::<Vec<usize>>() as i64
            + self.names.0.capacity() as i64 * std::mem::size_of::<usize>() as i64
            + self.schema.as_deref().map_or(0, Schema::MemoryUsage)
            + self
                .names
                .0
                .iter()
                .flatten()
                .map(|name| name.MemoryUsage())
                .sum::<i64>()
    }

    /// 简单计划无表达式列引用需解析，直接成功返回。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        Ok(())
    }
}
