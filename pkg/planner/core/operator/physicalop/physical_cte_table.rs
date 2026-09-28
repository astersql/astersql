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

/// PhysicalCTETable 对应 Go 的同名结构，表示读取某个 CTE 临时存储的物理叶子节点。
// pub struct PhysicalCTETable {
//     pub physical_schema_producer: PhysicalSchemaProducer,
/// IDForStorage 用来把扫描节点关联到 CTE 生产者分配的存储编号。
//     pub IDForStorage: i32,
// }
//
// impl PhysicalCTETable {
/// Init 对应 Go 的值接收者初始化：仅安装计划类型、上下文与统计信息。
/// Rust 消费 self 来模拟 Go 返回新结构地址的行为，不分配执行期资源。
//     pub fn Init(mut self, ctx: base::PlanContext, stats: &property::StatsInfo) -> Self {
//         self.physical_schema_producer.Plan =
//             baseimpl::NewBasePlan(ctx, plancodec::TypeCTETable, 0);
//         self.physical_schema_producer.SetStats(stats);
//         self
//     }
//
/// ExplainInfo 保留 Go 的说明文本格式，供 EXPLAIN 标识具体 CTE 存储。
//     pub fn ExplainInfo(&self) -> String {
//         format!("Scan on CTE_{}", self.IDForStorage)
//     }
//
/// MemoryUsage 对应 Go 的静态内存估算，只累计基类和一个 int 字段。
//     pub fn MemoryUsage(&self) -> i64 {
//         self.physical_schema_producer.MemoryUsage() + size::SizeOfInt
//     }
// }
//
/// findBestTask4LogicalCTETable 对应逻辑 CTE 表的物理任务选优入口。
/// 它只构造计划树；不会读取 CTE 数据，也不会触发任何存储 IO。
// pub fn findBestTask4LogicalCTETable(
//     super_plan: &dyn base::LogicalPlan,
//     prop: &property::PhysicalProperty,
// ) -> Result<Box<dyn base::Task>, errors::Error> {
//     if prop.IndexJoinProp.is_some() {
// 与 Go 一致，即使强制 hint 也不能让 CTE 表参与 Index Join 内表路径。
//         return Ok(Box::new(base::InvalidTask));
//     }
//
//     let (_, logical_cte_table) =
//         base::GetGEAndLogicalOp::<logicalop::LogicalCTETable>(super_plan);
//     if !prop.IsSortItemEmpty() {
// CTE 表扫描自身不提供顺序属性，要求有序时直接返回无效任务。
//         return Ok(Box::new(base::InvalidTask));
//     }
//
//     let mut physical_cte_table = PhysicalCTETable {
//         physical_schema_producer: PhysicalSchemaProducer::default(),
//         IDForStorage: logical_cte_table.IDForStorage,
//     }
//     .Init(logical_cte_table.SCtx(), logical_cte_table.StatsInfo());
//     physical_cte_table
//         .physical_schema_producer
//         .SetSchema(logical_cte_table.Schema());
//
// Go 在这里把物理叶子挂到 RootTask；执行阶段才会真正消费 CTE 存储。
//     let mut root_task = RootTask::default();
//     root_task.SetPlan(Box::new(physical_cte_table));
//     Ok(Box::new(root_task))
// }
// */

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
