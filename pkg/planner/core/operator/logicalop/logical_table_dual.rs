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

// `Dual`（虚拟单行/空表）逻辑算子。
//
// 对应无真实表扫描的常量行源：常用于 `SELECT` 无 FROM、或优化后折叠为空/单行结果。
// `RowCount` 只能是 0 或 1，表示空结果或单行常量。

use crate::{
    BaseLogicalPlan, Column, Expression, LogicalPlan, LogicalSchemaProducer, Result, StatsInfo,
};
use std::any::Any;
use std::collections::HashSet;

/// 逻辑 Dual 表：不访问存储，按 `RowCount` 产出 0 或 1 行。
#[derive(Default)]
pub struct LogicalTableDual {
    /// Schema 与基类逻辑计划。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 输出行数；与 Go 一样由构造方保证为 0 或 1。
    pub RowCount: i32,
}

impl LogicalTableDual {
    /// 初始化算子名为 `"TableDual"`；`RowCount` 与 Go 一样由构造方保证为 0 或 1。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan =
            crate::NewBaseLogicalPlan(ctx, plancodec::TypeDual, offset);
        self
    }

    /// EXPLAIN 展示行数。
    pub fn ExplainInfo(&self) -> String {
        format!("rowcount:{}", self.RowCount)
    }

    /// 哈希码：物理类型、查询块偏移与 `RowCount` 的大端字节。
    pub fn HashCode(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(12);
        result.extend_from_slice(
            &(plancodec::TypeStringToPhysicalID(self.TP()) as u32).to_be_bytes(),
        );
        result.extend_from_slice(&(self.QueryBlockOffset() as u32).to_be_bytes());
        result.extend_from_slice(&(self.RowCount as u32).to_be_bytes());
        result
    }

    /// Dual 无子节点可下推谓词，原样返回谓词列表。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        Ok(predicates)
    }

    /// 按父节点使用列做内联投影裁剪输出 schema。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let used = self
            .SCtx()
            .map(|context| {
                expression::GetUsedList(
                    context.GetExprCtx().GetEvalCtx(),
                    parent_used_cols.to_vec(),
                    self.Schema(),
                )
            })
            .unwrap_or_else(|| {
                let used_ids: HashSet<_> = parent_used_cols
                    .iter()
                    .map(|column| column.UniqueID)
                    .collect();
                self.Schema()
                    .Columns
                    .iter()
                    .map(|column| used_ids.contains(&column.UniqueID))
                    .collect()
            });
        let columns = std::mem::take(&mut self.Schema_mut().Columns);
        self.Schema_mut().Columns = columns
            .into_iter()
            .enumerate()
            .filter_map(|(index, column)| used[index].then_some(column))
            .collect();
        Ok(())
    }

    /// 构建唯一键信息，并在 `RowCount == 1` 时标记最多一行（MaxOneRow）。
    pub fn BuildKeyInfo(&mut self) {
        self.LogicalSchemaProducer.BuildKeyInfo();
        self.SetMaxOneRow(self.RowCount == 1);
    }

    /// 按 `RowCount` 推导统计：行数与各列 NDV 均等于该值。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let mut stats = StatsInfo {
            RowCount: f64::from(self.RowCount),
            ..StatsInfo::default()
        };
        for column in &self.Schema().Columns {
            stats
                .ColNDVs
                .insert(column.UniqueID, f64::from(self.RowCount));
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }
}

impl LogicalPlan for LogicalTableDual {
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
    fn ExplainInfo(&self) -> String {
        LogicalTableDual::ExplainInfo(self)
    }
    fn HashCode(&self) -> Vec<u8> {
        LogicalTableDual::HashCode(self)
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        LogicalTableDual::PredicatePushDown(self, predicates)
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        LogicalTableDual::PruneColumns(self, columns)
    }
    fn BuildKeyInfo(&mut self) {
        LogicalTableDual::BuildKeyInfo(self)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        LogicalTableDual::DeriveStats(self, reload)
    }
}
