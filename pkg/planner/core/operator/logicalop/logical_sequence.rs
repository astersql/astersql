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

// 逻辑算子：序列执行（LogicalSequence）。
//
// 对应 WITH 子句中 CTE（公用表表达式）物化后按序执行多段子计划的节点。
// Schema、谓词下推、列裁剪与统计均委托给最后一个子节点（主查询）。

use crate::*;
use std::any::Any;

/// 序列逻辑算子：前序子节点为 CTE 定义，末子节点为主查询。
#[derive(Default)]
pub struct LogicalSequence {
    /// 基类逻辑计划。
    pub BaseLogicalPlan: BaseLogicalPlan,
}

impl LogicalSequence {
    /// 初始化为 Sequence 节点。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Sequence", offset);
        self
    }
    /// Schema 取自最后一个子节点（主查询输出）。
    pub fn Schema(&self) -> &Schema {
        self.Children()
            .last()
            .map(|child| child.Schema())
            .unwrap_or_else(|| self.BaseLogicalPlan.Schema())
    }
    /// 谓词仅下推到主查询子节点。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let child = self.Children_mut().last_mut().ok_or_else(|| {
            PlannerError("LogicalSequence requires a main-query child".to_owned())
        })?;
        PredicatePushDownPlan(child, predicates)
    }
    /// 列裁剪仅作用于主查询子节点。
    pub fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        self.Children_mut()
            .last_mut()
            .ok_or_else(|| PlannerError("LogicalSequence requires a main-query child".to_owned()))?
            .PruneColumns(columns)
    }
    /// 统计信息取自主查询并缓存到本节点。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        let child = self.Children_mut().last_mut().ok_or_else(|| {
            PlannerError("LogicalSequence requires a main-query child".to_owned())
        })?;
        let (stats, changed) = child.DeriveStats(reload)?;
        self.SetStats(stats.clone());
        Ok((stats, changed))
    }
    /// 可能物理属性准备委托基类。
    pub fn PreparePossibleProperties(&mut self, children_have_tiflash: &[bool]) -> bool {
        self.base_mut()
            .PreparePossibleProperties(children_have_tiflash)
    }
}

impl LogicalPlan for LogicalSequence {
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
    fn Schema(&self) -> &Schema {
        Self::Schema(self)
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        Self::PredicatePushDown(self, predicates)
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        Self::PruneColumns(self, columns)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        Self::DeriveStats(self, reload)
    }
}
