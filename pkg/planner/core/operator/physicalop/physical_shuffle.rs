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

// 物理 Shuffle 算子：在并行执行中按哈希/范围重分区数据。
//
// 切分主线程、工作线程与取数线程的计划边界；下方大段注释保留 Go 完整字段语义，
// 当前可执行骨架见 `use crate::physical_common_plans` 之后。

/// 对应 Go 的 PhysicalShuffle：记录主线程、工作线程和取数线程之间的计划切分边界。
/// `Tails` 是工作线程内最后的计划，`DataSources` 是 Shuffle 后负责取数的首个计划。
// pub struct PhysicalShuffle {
//     pub BasePhysicalPlan: BasePhysicalPlan,
//     pub Concurrency: i32,
//     pub Tails: Vec<Box<dyn base::PhysicalPlan>>,
//     pub DataSources: Vec<Box<dyn base::PhysicalPlan>>,
//     pub SplitterType: PartitionSplitterType,
//     pub ByItemArrays: Vec<Vec<Box<dyn expression::Expression>>>,
// }
//
// impl PhysicalShuffle {
/// 对应 Go 的值接收者 Init：创建基础物理计划，并依次写入子计划属性和统计信息。
//     pub fn Init(
//         mut self,
//         ctx: base::PlanContext,
//         stats: *mut property::StatsInfo,
//         offset: i32,
//         props: Vec<*mut property::PhysicalProperty>,
//     ) -> Box<Self> {
// Go 把 `&p` 交给 NewBasePhysicalPlan；Box 保留返回新计划地址的所有权形状。
//         self.BasePhysicalPlan = NewBasePhysicalPlan(ctx, plancodec::TypeShuffle, &self, offset);
//         self.SetChildrenReqProps(props);
//         self.SetStats(stats);
//         Box::new(self)
//     }
//
/// 对应 Go 的 MemoryUsage：累计自身、切片容量、子计划和分区表达式占用。
//     pub fn MemoryUsage(&self) -> i64 {
//         let mut sum = self.BasePhysicalPlan.MemoryUsage()
//             + size::SizeOfInt * 2
//             + size::SizeOfSlice * (3 + self.ByItemArrays.capacity() as i64)
//             + (self.Tails.capacity() + self.DataSources.capacity()) as i64 * size::SizeOfInterface;
//
// Go 对接口切片逐项调用 MemoryUsage；这里保留动态分发和累计顺序。
//         for plan in &self.Tails {
//             sum += plan.MemoryUsage();
//         }
//         for plan in &self.DataSources {
//             sum += plan.MemoryUsage();
//         }
//         for exprs in &self.ByItemArrays {
//             sum += exprs.capacity() as i64 * size::SizeOfInterface;
//             for expr in exprs {
//                 sum += expr.MemoryUsage();
//             }
//         }
//         sum
//     }
//
/// 对应 Go 的 ExplainInfo：输出并发度以及各 DataSource 的 ExplainID。
//     pub fn ExplainInfo(&self) -> String {
//         let explain_ids: Vec<_> = self.DataSources.iter().map(|plan| plan.ExplainID()).collect();
//         format!(
//             "execution info: concurrency:{}, data sources:{:?}",
//             self.Concurrency, explain_ids
//         )
//     }
//
/// 对应 Go 的 ResolveIndices：先解析基础计划，再按 DataSource 的 Schema 解析分区表达式。
//     pub fn ResolveIndices(&mut self) -> Result<(), Error> {
//         self.BasePhysicalPlan.ResolveIndices()?;
//
// 每个 DataSource 对应一组 HashByItems；表达式取值基于 DataSource，而不是 children[0]。
//         for (i, exprs) in self.ByItemArrays.iter_mut().enumerate() {
//             let schema = self.DataSources[i].Schema();
//             for expr in exprs.iter_mut() {
// Go 会用解析后的表达式覆盖原槽位，并在首个错误处立即返回。
//                 *expr = expr.ResolveIndices(schema)?;
//             }
//         }
//         Ok(())
//     }
// }
// */
use crate::physical_common_plans::{
    PhysicalExpr, PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats,
};

/// 分区切分策略：哈希或范围。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PartitionSplitterType {
    #[default]
    /// 按哈希键拆分到各 worker。
    Hash,
    /// 按已排序键的区间拆分。
    Range,
}
/// 当前可执行的 Shuffle 骨架：并发度、尾部计划、数据源、切分方式与分区表达式。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhysicalShuffle {
    /// 并行 worker 数量，必须大于 0。
    pub concurrency: usize,
    /// 各工作线程内最后的计划，对应 Go `Tails`。
    pub tails: Vec<PhysicalPlanNode>,
    /// Shuffle 后负责取数的数据源计划列表。
    pub data_sources: Vec<PhysicalPlanNode>,
    /// 哈希或范围分区策略。
    pub splitter_type: PartitionSplitterType,
    /// 每个数据源各自的分区键表达式，对应 Go `ByItemArrays`。
    pub by_item_arrays: Vec<Vec<PhysicalExpr>>,
    /// 输出列 UniqueID 列表。
    pub schema: Vec<i64>,
}
impl PhysicalShuffle {
    /// 按 Go 的容量口径累计切片、计划与分区表达式。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self
                .tails
                .iter()
                .map(PhysicalPlanNode::memory_usage)
                .sum::<i64>()
            + self
                .data_sources
                .iter()
                .map(PhysicalPlanNode::memory_usage)
                .sum::<i64>()
            + self
                .by_item_arrays
                .iter()
                .map(|items| (items.capacity() * std::mem::size_of::<PhysicalExpr>()) as i64)
                .sum::<i64>()
    }
    /// EXPLAIN：与 Go 一样列出并发度和各 DataSource 的 ExplainID。
    pub fn explain_info(&self) -> String {
        let explain_ids = self
            .data_sources
            .iter()
            .map(|source| source.id.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "execution info: concurrency:{}, data sources:[{}]",
            self.concurrency, explain_ids
        )
    }
    /// 每组 ByItems 必须相对其对应 DataSource 的 Schema 解析。
    pub fn resolve_indices(&mut self) -> Result<(), String> {
        if self.by_item_arrays.len() > self.data_sources.len() {
            return Err("shuffle by-item arrays exceed data sources".to_owned());
        }
        for (items, source) in self.by_item_arrays.iter_mut().zip(&self.data_sources) {
            items
                .iter_mut()
                .try_for_each(|expr| expr.resolve_indices(&source.schema))?;
        }
        Ok(())
    }
    /// 组装 PhysicalPlanNode：data_sources 在前，所有 worker tails 追加在后。
    pub fn into_plan(
        self,
        stats: Stats,
        property: PhysicalProperty,
    ) -> Result<PhysicalPlanNode, String> {
        if self.concurrency == 0 {
            return Err("shuffle concurrency must be positive".into());
        }
        let mut children = self.data_sources;
        children.extend(self.tails);
        Ok(PhysicalPlanNode {
            id: children.iter().map(|child| child.id).max().unwrap_or(0) + 1,
            kind: PhysicalKind::Shuffle,
            schema: self.schema,
            children,
            stats,
            required_properties: vec![property],
        })
    }
}
/// Shuffle 接收端占位：执行器侧 worker 通过 receiver_index 对接。
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalShuffleReceiverStub {
    /// 接收端在 Shuffle 通道中的下标。
    pub receiver_index: usize,
    /// 输出列 UniqueID 列表。
    pub schema: Vec<i64>,
    /// Receiver 所属的数据源计划，对应 Go 的可空接口字段。
    pub data_source: Option<Box<PhysicalPlanNode>>,
}
impl PhysicalShuffleReceiverStub {
    /// 估算内存：结构体 + schema 容量。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + (self.schema.capacity() * 8) as i64
            + self
                .data_source
                .as_ref()
                .map_or(0, |source| source.memory_usage())
    }
    /// 转为无孩子的 ShuffleReceiver 计划节点。
    pub fn into_plan(self, stats: Stats) -> PhysicalPlanNode {
        PhysicalPlanNode {
            id: self.receiver_index as i64,
            kind: PhysicalKind::ShuffleReceiver,
            schema: self.schema,
            children: Vec::new(),
            stats,
            required_properties: Vec::new(),
        }
    }
}
