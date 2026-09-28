// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 数据源类物理算子的 Implementation：表/索引扫描与 Reader。
//
// 覆盖 TableDual、MemTableScan（零代价）、TableReader / TableScan、
// IndexReader / IndexScan。Reader 代价计入网络传输并按 Coprocessor
// worker 数摊薄；Scan 代价按行数 × 行宽 × 扫描因子（IndexScan 另加 Seek）。

use astersql_expression::Column;
use astersql_meta_model::TableInfo;
use astersql_planner_core_base::PhysicalPlan;
use astersql_planner_memo::ImplementationRef;
use astersql_statistics::HistColl;

use crate::{
    AttachChildren, BaseImpl, IndexScanCostPlan, PlanAccess, ReaderCostPlan, TableScanCostPlan,
    impl_implementation,
};

/// 生成「自身代价恒为 0」的 Implementation 类型与构造器（如 Dual / MemTable）。
macro_rules! zero_cost_implementation {
    ($name:ident, $constructor:ident) => {
        /// 零代价物理实现包装：仅持有计划节点与 BaseImpl。
        pub struct $name {
            pub(crate) base: BaseImpl,
            plan_node: Box<dyn PlanAccess>,
        }

        /// 由物理计划节点构造零代价 Implementation。
        pub fn $constructor(plan: Box<dyn PlanAccess>) -> $name {
            $name {
                base: BaseImpl::default(),
                plan_node: plan,
            }
        }

        impl $name {
            /// 固定返回 0；与 Go 一致，不覆盖外部通过 SetCost 写入的缓存值。
            fn calc_cost(&self, _out_count: f64, _children: &[ImplementationRef]) -> f64 {
                0.0
            }
            fn plan(&self) -> &dyn PhysicalPlan {
                self.plan_node.Plan()
            }
            fn attach_children(&mut self, children: &[ImplementationRef]) {
                AttachChildren(self.plan_node.PlanMut(), children);
            }
            fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
                self.base.GetCostLimit(cost_limit, children)
            }
        }

        impl_implementation!($name);
    };
}

// TableDual：空表/常量行源，无扫描开销。
zero_cost_implementation!(TableDualImpl, NewTableDualImpl);
// MemTableScan：内存表扫描，本实现中按零代价处理。
zero_cost_implementation!(MemTableScanImpl, NewMemTableScanImpl);

/// TableReader：从存储拉取表扫描结果到 TiDB 的物理实现。
/// 代价 ≈ (网络传输 + 子扫描代价) / Coprocessor worker 数。
pub struct TableReaderImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn ReaderCostPlan>,
    /// 表元信息，用于网络因子等。
    table_info: TableInfo,
    /// 列统计直方图集合，用于估算行宽。
    histograms: HistColl,
}

/// 构造 TableReader Implementation。
pub fn NewTableReaderImpl(
    plan: Box<dyn ReaderCostPlan>,
    table_info: TableInfo,
    histograms: HistColl,
) -> TableReaderImpl {
    TableReaderImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        table_info,
        histograms,
    }
}

impl TableReaderImpl {
    /// 网络代价 = 输出行数 × 网络因子 × 行宽，再与子代价相加后按 worker 摊薄。
    fn calc_cost(&self, out_count: f64, children: &[ImplementationRef]) -> f64 {
        let width = self
            .plan_node
            .AverageRowSize(&self.histograms, self.plan_node.Plan(), false);
        let network = out_count * self.plan_node.NetworkFactor(&self.table_info) * width;
        let workers = self.plan_node.CopIteratorWorkers() as f64;
        let cost = (network + children[0].borrow().GetCost()) / workers;
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    /// 子节点代价上限按 worker 数放大（并发可容纳更高子代价）。
    fn cost_limit(&self, cost_limit: f64, _children: &[ImplementationRef]) -> f64 {
        let workers = self.plan_node.CopIteratorWorkers() as f64;
        if f64::MAX / workers < cost_limit {
            f64::MAX
        } else {
            cost_limit * workers
        }
    }
}

impl_implementation!(TableReaderImpl);

/// TableScan：存储层表扫描物理实现；代价 = 行数 × 扫描因子 × 行宽。
pub struct TableScanImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn TableScanCostPlan>,
    histograms: HistColl,
    /// 扫描投影列，用于行宽估算。
    columns: Vec<Column>,
}

/// 构造 TableScan Implementation。
pub fn NewTableScanImpl(
    plan: Box<dyn TableScanCostPlan>,
    columns: Vec<Column>,
    histograms: HistColl,
) -> TableScanImpl {
    TableScanImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        histograms,
        columns,
    }
}

impl TableScanImpl {
    /// 按升/降序选择扫描因子后计算扫描代价。
    fn calc_cost(&self, out_count: f64, _children: &[ImplementationRef]) -> f64 {
        let width = self
            .plan_node
            .AverageRowSize(&self.histograms, &self.columns);
        let factor = if self.plan_node.Descending() {
            self.plan_node.DescScanFactor()
        } else {
            self.plan_node.ScanFactor()
        };
        let cost = out_count * factor * width;
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(TableScanImpl);

/// IndexReader：从存储拉取索引扫描结果；代价模型与 TableReader 类似，行宽按索引计算。
pub struct IndexReaderImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn ReaderCostPlan>,
    table_info: TableInfo,
    histograms: HistColl,
}

/// 构造 IndexReader Implementation。
pub fn NewIndexReaderImpl(
    plan: Box<dyn ReaderCostPlan>,
    table_info: TableInfo,
    histograms: HistColl,
) -> IndexReaderImpl {
    IndexReaderImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        table_info,
        histograms,
    }
}

impl IndexReaderImpl {
    /// 与 TableReader 相同公式，但 `AverageRowSize(..., true)` 按索引列宽。
    fn calc_cost(&self, out_count: f64, children: &[ImplementationRef]) -> f64 {
        let child = children[0].borrow();
        let width = self
            .plan_node
            .AverageRowSize(&self.histograms, child.GetPlan(), true);
        let network = out_count * self.plan_node.NetworkFactor(&self.table_info) * width;
        let workers = self.plan_node.CopIteratorWorkers() as f64;
        let cost = (network + child.GetCost()) / workers;
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    fn cost_limit(&self, cost_limit: f64, _children: &[ImplementationRef]) -> f64 {
        let workers = self.plan_node.CopIteratorWorkers() as f64;
        if f64::MAX / workers < cost_limit {
            f64::MAX
        } else {
            cost_limit * workers
        }
    }
}

impl_implementation!(IndexReaderImpl);

/// IndexScan：索引范围扫描；代价含扫描项与各 range 的 Seek 开销。
pub struct IndexScanImpl {
    pub(crate) base: BaseImpl,
    plan_node: Box<dyn IndexScanCostPlan>,
    histograms: HistColl,
}

/// 构造 IndexScan Implementation。
pub fn NewIndexScanImpl(plan: Box<dyn IndexScanCostPlan>, histograms: HistColl) -> IndexScanImpl {
    IndexScanImpl {
        base: BaseImpl::default(),
        plan_node: plan,
        histograms,
    }
}

impl IndexScanImpl {
    /// 代价 = 行数 × 行宽 × 扫描因子 + range 段数 × Seek 因子。
    fn calc_cost(&self, out_count: f64, _children: &[ImplementationRef]) -> f64 {
        let row_size = self.plan_node.AverageRowSize(&self.histograms);
        let scan_factor = if self.plan_node.Descending() {
            self.plan_node.DescScanFactor()
        } else {
            self.plan_node.ScanFactor()
        };
        let cost = out_count * row_size * scan_factor
            + self.plan_node.RangeCount() as f64 * self.plan_node.SeekFactor();
        self.base.SetCost(cost);
        cost
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan_node.Plan()
    }
    fn attach_children(&mut self, children: &[ImplementationRef]) {
        AttachChildren(self.plan_node.PlanMut(), children);
    }
    fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
        self.base.GetCostLimit(cost_limit, children)
    }
}

impl_implementation!(IndexScanImpl);
