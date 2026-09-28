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

// 物理优化中的执行任务抽象：RootTask 与 CopTask。
//
// Task 描述算子挂接后的执行单元：Root 在 TiDB 进程内跑，Cop 下推到 TiKV/TiFlash。
// 本文件提供 Root/Cop 结构、根条件传递、索引侧收尾，以及虚拟列展开遍历。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use expression::ExprBox;
use property::{MPPPartitionColumn, MPPPartitionType};

use crate::{BasePhysicalPlan, PhysicalSelection, PhysicalTableScan};

/// 物理算子挂接过程中产生的 Root 任务（见上英文）。
/// Root task produced while attaching physical operators.
pub struct RootTask {
    /// 当前任务根上的物理计划。
    plan: Box<dyn PhysicalPlan>,
    /// 来源任务（例如由 Cop 提升而来）。
    source: Option<Box<dyn Task>>,
    /// 挂接/优化过程收集的警告。
    warnings: Vec<expression::Error>,
    /// 只能在 Root 执行的残余过滤条件。
    pub RootTaskConds: Vec<ExprBox>,
    /// Root 条件的选择率估计。
    pub RootTaskSelectivity: f64,
    /// 条件是否源自数据源下推路径。
    pub FromDataSource: bool,
    /// 当前 MPP fragment 对父 Join 可见的分区布局。
    mpp_partition_type: MPPPartitionType,
    /// 当前 MPP fragment 对父 Join 可见的 Hash 列。
    mpp_hash_cols: Vec<MPPPartitionColumn>,
}

impl RootTask {
    /// 构造 RootTask，默认选择率为 1.0。
    pub fn New(plan: Box<dyn PhysicalPlan>, source: Option<Box<dyn Task>>) -> Self {
        Self {
            plan,
            source,
            warnings: Vec::new(),
            RootTaskConds: Vec::new(),
            RootTaskSelectivity: 1.0,
            FromDataSource: false,
            mpp_partition_type: property::AnyType,
            mpp_hash_cols: Vec::new(),
        }
    }

    /// 构造带 MPP 分区元数据的任务。
    pub fn NewWithMpp(
        plan: Box<dyn PhysicalPlan>,
        source: Option<Box<dyn Task>>,
        partition_type: MPPPartitionType,
        hash_cols: Vec<MPPPartitionColumn>,
    ) -> Self {
        let mut task = Self::New(plan, source);
        task.mpp_partition_type = partition_type;
        task.mpp_hash_cols = hash_cols;
        task
    }

    /// 只读访问计划。
    pub fn GetPlan(&self) -> &dyn PhysicalPlan {
        self.plan.as_ref()
    }
    /// 可变访问计划。
    pub fn GetPlanMut(&mut self) -> &mut dyn PhysicalPlan {
        self.plan.as_mut()
    }
    /// 替换任务上的计划。
    pub fn SetPlan(&mut self, plan: Box<dyn PhysicalPlan>) {
        self.plan = plan;
    }
}

/// Task trait：计数、复制、合法性与内存。
impl Task for RootTask {
    fn count(&self) -> f64 {
        self.plan.stats_count()
    }
    fn copy(&self) -> Box<dyn Task> {
        let context = self.plan.s_ctx().clone();
        Box::new(Self {
            plan: self
                .plan
                .clone_physical(context)
                .expect("root task plan clone"),
            source: self.source.as_ref().map(|source| source.copy()),
            warnings: self.warnings.clone(),
            RootTaskConds: self.RootTaskConds.clone(),
            RootTaskSelectivity: self.RootTaskSelectivity,
            FromDataSource: self.FromDataSource,
            mpp_partition_type: self.mpp_partition_type,
            mpp_hash_cols: self
                .mpp_hash_cols
                .iter()
                .map(MPPPartitionColumn::Clone)
                .collect(),
        })
    }
    fn plan(&self) -> &dyn PhysicalPlan {
        self.plan.as_ref()
    }
    fn plan_mut(&mut self) -> &mut dyn PhysicalPlan {
        self.plan.as_mut()
    }
    fn invalid(&self) -> bool {
        self.source.as_ref().is_some_and(|source| source.invalid())
    }
    fn convert_to_root_task(&self, _ctx: ContextRef) -> Box<dyn Task> {
        self.copy()
    }
    fn memory_usage(&self) -> i64 {
        self.plan.memory_usage()
            + self
                .RootTaskConds
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
    }
    fn append_warning(&mut self, error: expression::Error) {
        self.warnings.push(error);
    }
    fn mpp_partition_type(&self) -> MPPPartitionType {
        self.mpp_partition_type
    }
    fn mpp_hash_cols(&self) -> Vec<MPPPartitionColumn> {
        self.mpp_hash_cols
            .iter()
            .map(MPPPartitionColumn::Clone)
            .collect()
    }
    fn set_mpp_partition(
        &mut self,
        partition_type: MPPPartitionType,
        hash_cols: Vec<MPPPartitionColumn>,
    ) {
        self.mpp_partition_type = partition_type;
        self.mpp_hash_cols = hash_cols;
    }
}

/// 挂接完成后的任务别名。
pub type AttachedTask = RootTask;

/// Reader 构造前 Cop 任务收尾所需字段（见上英文）。
/// The cop-task fields used by task finalization before reader construction.
pub struct CopTask {
    /// 表侧下推计划。
    pub TablePlan: Option<Box<dyn PhysicalPlan>>,
    /// 索引侧下推计划。
    pub IndexPlan: Option<Box<dyn PhysicalPlan>>,
    /// 无法下推、留给 Root Selection 的条件。
    pub RootTaskConds: Vec<ExprBox>,
    /// 索引侧是否已 FinishIndexPlan。
    pub IndexPlanFinished: bool,
}

/// 默认空 CopTask。
impl Default for CopTask {
    fn default() -> Self {
        Self {
            TablePlan: None,
            IndexPlan: None,
            RootTaskConds: Vec::new(),
            IndexPlanFinished: false,
        }
    }
}

impl CopTask {
    /// 把 Root 条件与选择率拷到新 RootTask，直至挂上 PhysicalSelection。
    /// Preserves root-only conditions and their selectivity until PhysicalSelection is attached.
    pub fn HandleRootTaskConds(&self, new_task: &mut RootTask, selectivity: Option<f64>) {
        if self.RootTaskConds.is_empty() {
            return;
        }
        let selectivity = selectivity.filter(|value| value.is_finite()).unwrap_or(0.8);
        let context = new_task.plan.s_ctx().clone();
        let offset = new_task.plan.query_block_offset();
        let child = std::mem::replace(
            &mut new_task.plan,
            Box::new(BasePhysicalPlan::New(context.clone(), "", offset)),
        );
        let stats = child
            .stats_info()
            .Scale(context.GetSessionVars(), selectivity);
        let mut selection =
            PhysicalSelection::New(context.clone()).Init(context, stats, offset, Vec::new());
        selection.Conditions = self
            .RootTaskConds
            .iter()
            .map(|expr| expr.CloneExpr())
            .collect();
        selection.FromDataSource = true;
        selection.set_children(vec![child]);
        new_task.SetPlan(Box::new(selection));
        new_task.RootTaskConds = self
            .RootTaskConds
            .iter()
            .map(|expr| expr.CloneExpr())
            .collect();
        new_task.RootTaskSelectivity = selectivity;
        new_task.FromDataSource = true;
    }

    /// 标记索引侧完成，并把索引统计拷到表侧（保留表侧 StatsVersion）。
    /// Marks the index side complete and transfers its statistics to the table side,
    /// retaining the table scan's original statistics version.
    pub fn FinishIndexPlan(&mut self) {
        if self.IndexPlanFinished {
            return;
        }
        self.IndexPlanFinished = true;
        let (Some(table), Some(index)) = (&mut self.TablePlan, &self.IndexPlan) else {
            // 两侧都存在时才迁移统计。
            return;
        };
        let version = table.stats_info().StatsVersion;
        let mut stats = index.stats_info().clone();
        stats.StatsVersion = version;
        table.set_stats(stats);
    }

    /// 多孩子（MPP 分支）返回 TiFlash，否则返回叶子扫描的 StoreType。
    /// Returns TiFlash for branching MPP trees, otherwise the leaf scan's store type.
    pub fn GetStoreType(&self) -> kv::StoreType {
        let Some(mut plan) = self.TablePlan.as_deref() else {
            return kv::StoreType::TiKV;
        };
        loop {
            let children = plan.children();
            if children.len() > 1 {
                // 分叉计划视为 TiFlash/MPP 树。
                return kv::StoreType::TiFlash;
            }
            let Some(child) = children.first().copied() else {
                break;
            };
            plan = child;
        }
        if let Some(scan) = plan.as_any().downcast_ref::<PhysicalTableScan>() {
            return scan.StoreType;
        }
        plan.as_any()
            .downcast_ref::<BasePhysicalPlan>()
            .and_then(BasePhysicalPlan::StoreType)
            .unwrap_or(kv::StoreType::TiKV)
    }
}

/// 深度优先遍历基类计划树；visitor 返回 true 时停止向下。
/// Walks a foundation plan tree depth-first, stopping descent when the visitor handles a node.
pub fn TryExpandVirtualColumn(
    plan: &mut BasePhysicalPlan,
    visitor: &mut dyn FnMut(&mut dyn PhysicalPlan) -> bool,
) {
    fn walk(plan: &mut dyn PhysicalPlan, visitor: &mut dyn FnMut(&mut dyn PhysicalPlan) -> bool) {
        if visitor(plan) {
            return;
        }
        let mut children = plan
            .children()
            .into_iter()
            .map(|child| {
                child
                    .clone_physical(child.s_ctx().clone())
                    .expect("physical child clone for virtual-column traversal")
            })
            .collect::<Vec<_>>();
        for child in &mut children {
            walk(child.as_mut(), visitor);
        }
        plan.set_children(children);
    }
    walk(plan, visitor);
}
