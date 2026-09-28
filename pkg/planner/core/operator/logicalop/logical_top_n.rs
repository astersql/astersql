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

// 逻辑 TopN 算子：排序后取前 N 行（可带 Offset）。
//
// 对应 SQL 中 `ORDER BY ... LIMIT ... OFFSET ...` 的组合语义。若无排序键则退化为
// 纯 Limit。挂接子节点时，对 Dual 可直接截断行数；纯 Limit 会改写为 `LogicalLimit`。

use crate::{
    BaseLogicalPlan, ByItems, Column, CorrelatedColumn, LogicalLimit, LogicalPlan, LogicalPlanRef,
    LogicalSchemaProducer, LogicalTableDual, NewBaseLogicalPlan, Result, Schema, SortItem,
    SortProperties, StatsInfo, getPossiblePropertyFromByItems, pruneSortByItems,
};
use expression::Expression as _;
use std::any::Any;
use std::collections::HashMap;

/// TopN：按 `ByItems` 排序后跳过 `Offset` 行再取 `Count` 行。
#[derive(Default)]
pub struct LogicalTopN {
    /// Schema 与基类逻辑计划。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 排序键；为空时本算子等价于 Limit。
    pub ByItems: Vec<ByItems>,
    /// 分区内 TopN 的分区键（如窗口/分区限制场景）。
    pub PartitionBy: Vec<SortItem>,
    /// 跳过的行数（OFFSET）。
    pub Offset: u64,
    /// 保留的行数（LIMIT count）。
    pub Count: u64,
    pub OffsetParam: Option<usize>,
    pub CountParam: Option<usize>,
    /// 是否倾向于把 Limit 下推到 Coprocessor（存储层）。
    pub PreferLimitToCop: bool,
}

impl LogicalTopN {
    /// 初始化算子名为 `"TopN"`。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "TopN", offset);
        self
    }

    /// EXPLAIN：分区键、排序键、offset/count。
    pub fn ExplainInfo(&self) -> String {
        let parameters = self.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        let mut explanation = String::new();
        if !self.PartitionBy.is_empty() {
            explanation.push_str("partition by ");
            explanation.push_str(
                &self
                    .PartitionBy
                    .iter()
                    .map(|item| {
                        expression::StringerWithCtx::StringWithCtx(
                            &item.Col,
                            parameters.map(|ctx| ctx as _),
                            "",
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.PartitionBy.is_empty() && !self.ByItems.is_empty() {
            explanation.push_str("order by ");
        }
        explanation.push_str(
            &self
                .ByItems
                .iter()
                .map(|item| {
                    let expression = item.Expr.StringWithCtx(parameters.map(|ctx| ctx as _), "");
                    if item.Desc {
                        format!("{expression}:desc")
                    } else {
                        expression
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
        explanation.push_str(&format!(", offset:{}, count:{}", self.Offset, self.Count));
        explanation
    }

    /// 按 HashCode 替换排序表达式中的列引用。
    pub fn ReplaceExprColumns(&mut self, replace: &HashMap<Vec<u8>, Column>) {
        for mut item in std::mem::take(&mut self.ByItems) {
            item.Expr = rule_util::ResolveExprAndReplace(item.Expr, replace);
            self.ByItems.push(item);
        }
    }

    /// 列裁剪：保留父可见列与排序所需列，下推后同步 schema 并内联投影。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let visible = parent_used_cols.to_vec();
        let (items, columns) = pruneSortByItems(std::mem::take(&mut self.ByItems));
        self.ByItems = items;
        let mut used = visible.clone();
        used.extend(columns);
        let schema = if let Some(child) = self.Children_mut().first_mut() {
            child.PruneColumns(&used)?;
            Some(child.Schema().Clone())
        } else {
            None
        };
        if let Some(schema) = schema {
            self.SetSchema(schema);
        }
        self.LogicalSchemaProducer.InlineProjection(&visible);
        Ok(())
    }

    /// 构建键信息；`Count == 1` 时标记 MaxOneRow。
    pub fn BuildKeyInfo(&mut self) {
        self.LogicalSchemaProducer.BuildKeyInfo();
        if self.Count == 1 {
            self.SetMaxOneRow(true);
        }
    }

    /// 由子节点统计推导 Limit 后基数（`property::DeriveLimitStats`）。
    pub fn DeriveStats(
        &mut self,
        child_stats: &[StatsInfo],
        reloads: &[bool],
    ) -> Result<(StatsInfo, bool)> {
        let reload = reloads.len() == 1 && reloads[0];
        if !reload && let Some(stats) = LogicalPlan::StatsInfo(self) {
            return Ok((stats.clone(), false));
        }
        let child = child_stats
            .first()
            .ok_or_else(|| crate::PlannerError("TopN requires child statistics".into()))?;
        let stats = property::DeriveLimitStats(child, self.Count as f64);
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 按排序键推导有序属性。
    pub fn PreparePossibleProperties(
        &mut self,
        _schema: &Schema,
        infos: &[SortProperties],
    ) -> SortProperties {
        let has_tiflash = infos.first().is_some_and(|info| info.HasTiFlash);
        let columns = getPossiblePropertyFromByItems(&self.ByItems);
        SortProperties {
            Orders: if columns.is_empty() {
                Vec::new()
            } else {
                vec![columns]
            },
            HasTiFlash: has_tiflash,
        }
    }

    /// 提取排序表达式中的相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.ByItems
            .iter()
            .flat_map(|item| expression::ExtractCorColumns(item.Expr.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 返回分区键切片。
    pub fn GetPartitionBy(&self) -> &[SortItem] {
        &self.PartitionBy
    }

    /// 无排序键时视为纯 Limit。
    pub fn IsLimit(&self) -> bool {
        self.ByItems.is_empty()
    }

    /// 挂接子节点：对 Dual 直接截断行数；纯 Limit 改写为 `LogicalLimit`；否则挂到自身下。
    pub fn AttachChild(mut self, mut child: LogicalPlanRef) -> LogicalPlanRef {
        // Dual 无真实扫描：在常量行数上直接应用 offset/count
        if let Some(dual) = child.as_any_mut().downcast_mut::<LogicalTableDual>() {
            let rows = dual.RowCount.max(0) as u64;
            dual.RowCount = if rows < self.Offset {
                0
            } else {
                (rows - self.Offset).min(self.Count) as i32
            };
            return child;
        }
        // 无 ORDER BY 时退化为 LogicalLimit，便于后续下推
        if self.IsLimit() {
            let query_block_offset = self.QueryBlockOffset();
            let ctx = self
                .SCtx()
                .cloned()
                .expect("initialized TopN must retain planner context");
            let mut limit = LogicalLimit {
                Count: self.Count,
                Offset: self.Offset,
                CountParam: self.CountParam,
                OffsetParam: self.OffsetParam,
                PreferLimitToCop: self.PreferLimitToCop,
                PartitionBy: self.PartitionBy,
                ..LogicalLimit::default()
            }
            .Init(ctx, query_block_offset);
            limit.SetChildren(vec![child]);
            return Box::new(limit);
        }
        self.SetChildren(vec![child]);
        Box::new(self)
    }
}

impl LogicalPlan for LogicalTopN {
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

    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        LogicalTopN::PruneColumns(self, columns)
    }
}
