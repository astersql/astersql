// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 逻辑 UnionScan 算子：合并事务本地缓冲行与快照扫描行。
//
// 在事务（Transaction）内，未提交的写落在本地 MemBuffer；读已提交数据走快照
// （Snapshot）扫描。UnionScan 将两侧按句柄列合并，并应用相同过滤条件，保证
// 读己之写（Read Your Own Writes）语义。

use crate::{
    BaseLogicalPlan, Column, Expression, HandleCols, LogicalPlan, LogicalTableDual,
    NewBaseLogicalPlan, PredicatePushDownPlan, Result, Schema,
};
use std::any::Any;

/// 物理表 ID 的哨兵值，用于标识额外的物理表 ID 列。
const EXTRA_PHYSICAL_TABLE_ID: i64 = -3;

/// UnionScan 子树可继承的有序属性。
#[derive(Clone, Default)]
pub struct UnionScanProperties {
    /// 子节点可能提供的有序列组合。
    pub Orders: Vec<Vec<Column>>,
    /// 是否含 TiFlash 路径（UnionScan 本身通常置 false）。
    pub HasTiFlash: bool,
}

/// Merges transaction-local rows with snapshot rows.
///
/// 将事务本地未提交行与快照扫描结果按句柄合并的逻辑算子。
pub struct LogicalUnionScan {
    /// 基类逻辑计划。
    pub BaseLogicalPlan: BaseLogicalPlan,
    /// 需同时施加于本地缓冲与快照侧的过滤条件。
    pub Conditions: Vec<Expression>,
    /// 用于定位/去重行的句柄列（主键或 row id）。
    pub HandleCols: Box<dyn HandleCols>,
}

impl Default for LogicalUnionScan {
    fn default() -> Self {
        Self {
            BaseLogicalPlan: BaseLogicalPlan::default(),
            Conditions: Vec::new(),
            HandleCols: Box::new(planner_util::IntHandleCols::default()),
        }
    }
}

impl LogicalUnionScan {
    /// 初始化算子名为 `"UnionScan"`。
    pub fn Init(mut self, ctx: base::ContextRef, query_block_offset: i32) -> Self {
        self.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "UnionScan", query_block_offset);
        self
    }

    /// EXPLAIN：排序后的条件文本与句柄列描述。
    pub fn ExplainInfo(&self) -> String {
        let parameters = self.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        let mut conditions = self
            .Conditions
            .iter()
            .map(|expr| expr.StringWithCtx(parameters.map(|ctx| ctx as _), ""))
            .collect::<Vec<_>>();
        conditions.sort_unstable();
        format!(
            "conds:{}, handle:{}",
            conditions.join(", "),
            self.HandleCols.StringWithCtx(None, "")
        )
    }

    /// 谓词下推：含虚拟列的条件留在上层；其余下推并记录到 `Conditions`。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        // 虚拟列条件无法安全下推到存储侧，拆出后保留在本层之上
        let (with_virtual, without_virtual): (Vec<_>, Vec<_>) = predicates
            .into_iter()
            .partition(|expr| expression::ContainVirtualColumn(std::slice::from_ref(expr)));
        let pushed_conditions = without_virtual.clone();
        let child = &mut self.BaseLogicalPlan.Children_mut()[0];
        let mut retained = PredicatePushDownPlan(child, without_virtual)?;
        // 子节点已折叠为 Dual（空/常量）时不再保留条件
        if child.as_any().is::<LogicalTableDual>() {
            return Ok(Vec::new());
        }
        self.Conditions = pushed_conditions;
        retained.extend(with_virtual);
        Ok(retained)
    }

    /// 列裁剪：父层列 + 句柄列 + 物理表 ID 列 + 条件引用列一并下推，并同步 schema。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let mut used = parent_used_cols.to_vec();
        used.extend(self.HandleCols.IterColumns().cloned());
        used.extend(
            self.Schema()
                .Columns
                .iter()
                .filter(|column| column.ID == EXTRA_PHYSICAL_TABLE_ID)
                .cloned(),
        );
        used.extend(
            expression::ExtractColumnsFromExpressions(&self.Conditions, None)
                .into_iter()
                .cloned(),
        );
        self.BaseLogicalPlan.Children_mut()[0].PruneColumns(&used)?;
        Ok(())
    }

    /// 继承首个子节点的有序属性；UnionScan 本身不走 TiFlash。
    pub fn PreparePossibleProperties(
        &mut self,
        _schema: &Schema,
        children: &[Option<UnionScanProperties>],
    ) -> UnionScanProperties {
        self.BaseLogicalPlan.PreparePossibleProperties(&[]);
        let orders = children
            .first()
            .and_then(Option::as_ref)
            .map(|child| child.Orders.clone())
            .unwrap_or_default();
        UnionScanProperties {
            Orders: orders,
            HasTiFlash: false,
        }
    }
}

impl LogicalPlan for LogicalUnionScan {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn base(&self) -> &BaseLogicalPlan {
        &self.BaseLogicalPlan
    }

    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.BaseLogicalPlan
    }

    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        Self::PruneColumns(self, columns)
    }
}
