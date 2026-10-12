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

// 物理算子：索引归并连接（PhysicalIndexMergeJoin）。
//
// 要求两侧按连接键有序（或对 outer 先排序），再以归并方式推进匹配；
// 适合索引已提供序的场景。

use crate::physical_common_plans::{PhysicalKind, PhysicalPlanNode};
#[derive(Clone, Debug, PartialEq)]
/// 索引归并连接骨架：键序映射、比较函数名、是否需 outer 排序与降序。
pub struct LegacyPhysicalIndexMergeJoin {
    /// 外表物理计划。
    pub outer: PhysicalPlanNode,
    /// 内表（索引有序）物理计划。
    pub inner: PhysicalPlanNode,
    /// 连接键下标按索引顺序重排后的映射。
    pub key_offset_order: Vec<usize>,
    /// outer 键与 inner 键的比较函数标识。
    pub compare_functions: Vec<String>,
    /// 两条 outer 行之间的键比较函数标识（用于排序）。
    pub outer_compare_functions: Vec<String>,
    /// 为 true 时执行前需对 outer 按连接键排序。
    pub need_outer_sort: bool,
    /// 内表是否按降序扫描。
    pub descending: bool,
    /// 并发度，用于代价分摊。
    pub concurrency: usize,
}
impl LegacyPhysicalIndexMergeJoin {
    /// 生成 EXPLAIN；归一化时隐藏具体键序细节。
    pub fn explain_info(&self, normalized: bool) -> String {
        if normalized {
            format!(
                "index merge join, key count:{}, outer sort:{}",
                self.key_offset_order.len(),
                self.need_outer_sort
            )
        } else {
            format!(
                "index merge join, key order:{:?}, outer sort:{}, desc:{}",
                self.key_offset_order, self.need_outer_sort, self.descending
            )
        }
    }
    /// 估算代价；若需 outer 排序则附加 n log n 项。
    pub fn cost(
        &self,
        outer_count: f64,
        inner_count: f64,
        outer_cost: f64,
        inner_cost: f64,
        cpu_factor: f64,
    ) -> f64 {
        // outer 无序时按比较排序代价计入。
        let sorting = if self.need_outer_sort {
            outer_count.max(1.0).log2() * outer_count * cpu_factor
        } else {
            0.0
        };
        outer_cost
            + inner_cost
            + (outer_count + inner_count) * cpu_factor / self.concurrency.max(1) as f64
            + sorting
    }
    /// 合并 Schema，组装 IndexMergeJoin 节点。
    pub fn attach_to_task(&self) -> PhysicalPlanNode {
        let mut schema = self.outer.schema.clone();
        schema.extend(&self.inner.schema);
        PhysicalPlanNode {
            id: self.outer.id.max(self.inner.id) + 1,
            kind: PhysicalKind::IndexMergeJoin,
            schema,
            children: vec![self.outer.clone(), self.inner.clone()],
            stats: self.outer.stats.clone(),
            required_properties: Vec::new(),
        }
    }
    /// 计入结构体、键序向量与比较函数字符串容量。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + ((self.key_offset_order.capacity() * std::mem::size_of::<usize>())
                + (self.compare_functions.capacity() + self.outer_compare_functions.capacity())
                    * std::mem::size_of::<String>()
                + self
                    .compare_functions
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>()
                + self
                    .outer_compare_functions
                    .iter()
                    .map(String::capacity)
                    .sum::<usize>()) as i64
    }
}

/// Go-compatible index join retaining the complete execution contract.
pub struct PhysicalIndexMergeJoin {
    pub PhysicalIndexJoin: crate::PhysicalIndexJoin,
    pub KeyOff2KeyOffOrderByIdx: Vec<i32>,
    pub CompareFuncs: Vec<crate::JoinCompareFunc>,
    pub OuterCompareFuncs: Vec<crate::JoinCompareFunc>,
    pub NeedOuterSort: bool,
    pub Desc: bool,
}
impl std::ops::Deref for PhysicalIndexMergeJoin {
    type Target = crate::PhysicalIndexJoin;
    fn deref(&self) -> &Self::Target {
        &self.PhysicalIndexJoin
    }
}
impl std::ops::DerefMut for PhysicalIndexMergeJoin {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.PhysicalIndexJoin
    }
}
impl PhysicalIndexMergeJoin {
    pub fn New(mut join: crate::PhysicalIndexJoin) -> Self {
        join.BasePhysicalJoin
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetTP("IndexMergeJoin");
        Self {
            PhysicalIndexJoin: join,
            KeyOff2KeyOffOrderByIdx: Vec::new(),
            CompareFuncs: Vec::new(),
            OuterCompareFuncs: Vec::new(),
            NeedOuterSort: false,
            Desc: false,
        }
    }
    pub fn ExplainInfo(&self) -> String {
        self.PhysicalIndexJoin.ExplainInfoInternal(false, true)
    }
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.PhysicalIndexJoin.ExplainInfoInternal(true, true)
    }
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalIndexJoin.MemoryUsage()
            + (self.KeyOff2KeyOffOrderByIdx.capacity() * std::mem::size_of::<i32>()) as i64
            + ((self.CompareFuncs.capacity() + self.OuterCompareFuncs.capacity())
                * std::mem::size_of::<crate::JoinCompareFunc>()) as i64
            + (std::mem::size_of::<Vec<i32>>()
                + 2 * std::mem::size_of::<Vec<crate::JoinCompareFunc>>()
                + 2 * std::mem::size_of::<bool>()) as i64
    }
    pub fn Clone(&self, context: base::ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            PhysicalIndexJoin: self.PhysicalIndexJoin.Clone(context)?,
            KeyOff2KeyOffOrderByIdx: self.KeyOff2KeyOffOrderByIdx.clone(),
            CompareFuncs: self.CompareFuncs.clone(),
            OuterCompareFuncs: self.OuterCompareFuncs.clone(),
            NeedOuterSort: self.NeedOuterSort,
            Desc: self.Desc,
        })
    }
}
