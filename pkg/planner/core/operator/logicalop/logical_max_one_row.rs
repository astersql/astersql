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

// 逻辑 MaxOneRow 算子：断言子树至多产出一行。
//
// 用于标量子查询等场景；空输入会物化为单行 NULL，因此 Schema 上
// 不能向上暴露子列的 NOT NULL 标志。

use crate::{
    AttachSelectionToPlan, BaseLogicalPlan, Expression, LogicalPlan, NewBaseLogicalPlan,
    PredicatePushDownPlan, Result, Schema, StatsInfo,
};
use std::any::Any;
use std::collections::HashMap;

/// LogicalMaxOneRow checks that its child produces no more than one row.
/// 检查子节点最多产出一行（标量子查询语义屏障）。
pub struct LogicalMaxOneRow {
    pub BaseLogicalPlan: BaseLogicalPlan,
}

/// 默认空 MaxOneRow 节点。
impl Default for LogicalMaxOneRow {
    fn default() -> Self {
        Self {
            BaseLogicalPlan: BaseLogicalPlan::default(),
        }
    }
}

impl LogicalMaxOneRow {
    /// 初始化基类逻辑计划，算子名为 MaxOneRow。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "MaxOneRow", offset);
        self
    }

    /// Empty input is materialized as one NULL row, so child NOT NULL flags
    /// cannot be exposed above this operator.
    /// 空输入物化为单行 NULL，故清除子 Schema 的 NOT NULL 标志后再向上暴露。
    pub fn Schema(&self) -> Schema {
        let mut schema = self.Children()[0].Schema().Clone();
        let end = schema.Columns.len();
        planner_util::ResetNotNullFlag(&mut schema, 0, end);
        schema
    }

    /// MaxOneRow is a semantic barrier: parent predicates must stay above it.
    /// 语义屏障：父侧谓词必须留在本算子之上，不可下推穿越。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        if let Some(child) = self.Children_mut().first_mut() {
            let residual = PredicatePushDownPlan(child, Vec::new())?;
            AttachSelectionToPlan(child, residual)?;
        }
        Ok(predicates)
    }

    /// 统计恒为单行：使用 getSingletonStats。
    pub fn DeriveStats(&mut self, self_schema: &Schema, reloads: &[bool]) -> (StatsInfo, bool) {
        let reload = reloads.len() == 1 && reloads[0];
        if !reload && let Some(stats) = LogicalPlan::StatsInfo(self) {
            return (stats.clone(), false);
        }
        let stats = getSingletonStats(self_schema);
        LogicalPlan::SetStats(self, stats.clone());
        (stats, true)
    }
}

/// LogicalPlan trait 委托到 LogicalMaxOneRow 具体实现。
impl LogicalPlan for LogicalMaxOneRow {
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

    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        Self::PredicatePushDown(self, predicates)
    }
}

/// Exists and MaxOneRow produce at most one row, hence every output NDV is 1.
/// Exists/MaxOneRow 至多一行，故输出列 NDV 均为 1。
pub fn getSingletonStats(schema: &Schema) -> StatsInfo {
    let ColNDVs = schema
        .Columns
        .iter()
        .map(|column| (column.UniqueID, 1.0))
        .collect::<HashMap<_, _>>();
    StatsInfo {
        RowCount: 1.0,
        ColNDVs,
        ..StatsInfo::default()
    }
}
