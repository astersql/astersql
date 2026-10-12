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

// CTE Table 物理叶子：扫描某个 CTE 临时存储（由生产者分配的 storage id），
// 本身不提供排序属性，只在 Root 任务上生成候选。

// --- 可运行的简化实现 ---
use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats};

#[derive(Clone, Debug, PartialEq)]
/// 读取 CTE 临时存储的物理表扫描节点。
pub struct PhysicalCteTable {
    /// 关联 CTE 生产者分配的存储编号。
    pub id_for_storage: i64,
    /// 种子侧统计信息。
    pub seed_statistics: Stats,
    /// 输出列 ID 列表。
    pub schema: Vec<i64>,
}
impl PhysicalCteTable {
    /// EXPLAIN 标识具体 CTE 存储。
    pub fn explain_info(&self) -> String {
        format!("Scan on CTE_{}", self.id_for_storage)
    }
    /// 静态内存估算：结构体 + schema 容量。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64 + (self.schema.capacity() * 8) as i64
    }
}
/// 为逻辑 CTE 表选优：仅接受无序属性，生成的候选始终是 Root 计划。
pub fn find_best_task_for_cte_table(
    table: &PhysicalCteTable,
    property: &PhysicalProperty,
) -> Result<Option<PhysicalPlanNode>, String> {
    // Go 不检查请求的 TaskTp；CTE 表扫描只拒绝自身无法提供的顺序属性，
    // 并把成功候选统一挂到 RootTask。
    if !property.sort_items.is_empty() {
        return Ok(None);
    }
    Ok(Some(PhysicalPlanNode {
        id: table.id_for_storage,
        kind: PhysicalKind::CteTable {
            id: table.id_for_storage,
        },
        schema: table.schema.clone(),
        children: Vec::new(),
        stats: table.seed_statistics.clone(),
        required_properties: Vec::new(),
    }))
}

/// Go's concrete CTETable occurrence retained by the physical-plan RU bridge.
pub struct PhysicalCTETable {
    pub PhysicalSchemaProducer: crate::PhysicalSchemaProducer,
    pub IDForStorage: i32,
}
impl PhysicalCTETable {
    pub fn New(ctx: base::ContextRef, storage_id: i32) -> Self {
        Self {
            PhysicalSchemaProducer: crate::PhysicalSchemaProducer::New(
                crate::BasePhysicalPlan::New(ctx, "CTETable", 0),
            ),
            IDForStorage: storage_id,
        }
    }
    pub fn Clone(&self, ctx: base::ContextRef) -> Result<Self, expression::Error> {
        let mut producer = crate::PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            IDForStorage: self.IDForStorage,
        })
    }
}
