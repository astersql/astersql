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

// 逻辑 `UNION ALL` 算子：按位置对齐合并多个分支的结果行（不去重）。
//
// 谓词可下推到各分支；列裁剪保持各分支的对齐输出；统计上将各分支行数与
// 同一输出列 ID 的 NDV 累加。函数依赖（FD）取各分支 NOT NULL 与等价类的公共部分。

use crate::*;
use std::any::Any;

/// `UNION ALL` 逻辑算子：多子节点结果纵向拼接。
#[derive(Default)]
pub struct LogicalUnionAll {
    /// Schema 与基类逻辑计划（输出列由各分支按位置对齐）。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
}

impl LogicalUnionAll {
    /// 初始化算子名为 `"Union"`。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Union", offset);
        self
    }

    /// 将谓词分别下推到每个分支，并在分支上物化未消化的 Selection。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        for index in 0..self.Children().len() {
            // Every branch receives its own expression objects, matching Go's slice clone.
            // 每个分支拿到独立的谓词副本，避免共享可变状态
            let branch = predicates.iter().cloned().collect();
            let remained = PredicatePushDownPlan(&mut self.Children_mut()[index], branch)?;
            let child = std::mem::replace(
                &mut self.Children_mut()[index],
                Box::new(LogicalTableDual::default()),
            );
            AddSelection(self, index, child, remained)?;
        }
        Ok(Vec::new())
    }

    /// 按输出列是否被父层使用，对齐裁剪所有子节点对应下标的列。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let schema_columns = self.Schema().Columns.clone();
        let mut used = schema_columns
            .iter()
            .map(|column| {
                parent_used_cols
                    .iter()
                    .any(|used| used.UniqueID == column.UniqueID)
            })
            .collect::<Vec<_>>();
        let has_been_used = used.iter().any(|value| *value);
        let child_used = if has_been_used {
            parent_used_cols.to_vec()
        } else {
            used.fill(true);
            schema_columns.clone()
        };
        for child in self.Children_mut() {
            child.PruneColumns(&child_used)?;
        }
        let mut index = 0;
        self.Schema_mut().Columns.retain(|_| {
            let keep = used[index];
            index += 1;
            keep
        });
        if has_been_used {
            let output_schema = self.Schema().Clone();
            let context = self.SCtx().cloned();
            let query_block_offset = self.QueryBlockOffset();
            for child in self.Children_mut() {
                if output_schema.Columns.len() < child.Schema().Columns.len() {
                    let context = context.clone().ok_or_else(|| {
                        PlannerError("initialized Union must retain planner context".into())
                    })?;
                    let old_child = std::mem::replace(child, Box::new(LogicalTableDual::default()));
                    let mut projection = LogicalProjection {
                        Exprs: output_schema
                            .Columns
                            .iter()
                            .cloned()
                            .map(|column| Box::new(column) as Expression)
                            .collect(),
                        ..LogicalProjection::default()
                    }
                    .Init(context, query_block_offset);
                    projection.SetSchema(output_schema.Clone());
                    projection.SetChildren(vec![old_child]);
                    *child = Box::new(projection);
                }
            }
        }
        Ok(())
    }

    /// 对每个分支下推 `offset + count` 的 TopN，并保留原 TopN 在 Union 之上。
    pub fn PushDownTopN(&mut self, mut top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        let pushed_top_n = top_n.as_ref().map(|plan| {
            let top_n = plan
                .as_any()
                .downcast_ref::<LogicalTopN>()
                .expect("LogicalUnionAll::PushDownTopN expects LogicalTopN");
            (
                top_n.Count.wrapping_add(top_n.Offset),
                top_n.PreferLimitToCop,
                top_n.ByItems.clone(),
                top_n
                    .SCtx()
                    .cloned()
                    .expect("initialized TopN must retain planner context"),
                top_n.QueryBlockOffset(),
            )
        });
        for child in self.Children_mut() {
            let branch_top_n = pushed_top_n.as_ref().map(
                |(count, prefer_limit_to_cop, by_items, ctx, query_block_offset)| {
                    Box::new(
                        LogicalTopN {
                            Count: *count,
                            PreferLimitToCop: *prefer_limit_to_cop,
                            ByItems: by_items.clone(),
                            ..LogicalTopN::default()
                        }
                        .Init(ctx.clone(), *query_block_offset),
                    ) as LogicalPlanRef
                },
            );
            if let Some(pushed) = child.PushDownTopN(branch_top_n) {
                *child = pushed;
            }
        }
        let current = std::mem::take(self);
        if let Some(plan) = top_n.as_mut() {
            plan.SetChildren(vec![Box::new(current)]);
            top_n
        } else {
            Some(Box::new(current))
        }
    }

    /// 累加各分支行数与按输出列 ID 对齐的列 NDV。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let output = self.Schema().Columns.clone();
        let mut result = StatsInfo::default();
        for child in self.Children_mut() {
            let child_stats = child.DeriveStats(reload)?.0;
            result.RowCount += child_stats.RowCount;
            for output_column in &output {
                *result.ColNDVs.entry(output_column.UniqueID).or_default() += child_stats
                    .ColNDVs
                    .get(&output_column.UniqueID)
                    .copied()
                    .unwrap_or_default();
            }
        }
        self.SetStats(result.clone());
        Ok((result, true))
    }

    /// 可能属性（如 TiFlash）委托基类汇总子节点标记。
    pub fn PreparePossibleProperties(&mut self, children_have_tiflash: &[bool]) -> bool {
        self.base_mut()
            .PreparePossibleProperties(children_have_tiflash)
    }

    /// 提取函数依赖：NOT NULL 取交集，等价类取各分支公共部分。
    pub fn ExtractFD(&mut self) -> fd::FDSet {
        let mut child_sets = Vec::new();
        for child in self.Children_mut() {
            child_sets.push(child.base_mut().ExtractFD().clone());
        }
        let refs = child_sets.iter().collect::<Vec<_>>();
        let mut result = fd::FDSet::default();
        let mut not_null = intset::NewFastIntSet(
            self.Schema()
                .Columns
                .iter()
                .map(|column| column.UniqueID as i32)
                .collect(),
        );
        for set in &child_sets {
            not_null.IntersectionWith(&set.NotNullCols);
        }
        result.MakeNotNull(not_null);
        for class in fd::FindCommonEquivClasses(&refs) {
            result.AddEquivalenceUnion(class);
        }
        self.base_mut().SetFDs(result.clone());
        result
    }
}

impl LogicalPlan for LogicalUnionAll {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalSchemaProducer.BaseLogicalPlan
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
