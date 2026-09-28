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

// 逻辑 Limit 算子：OFFSET/COUNT 截断，以及可选分区 Limit。
//
// 作为谓词下推屏障；可将自身转换为 TopN 以继续向下推送。

use crate::{
    AttachSelectionToPlan, BaseLogicalPlan, Column, Expression, LogicalPlan, LogicalPlanRef,
    LogicalSchemaProducer, LogicalTopN, NewBaseLogicalPlan, PredicatePushDownPlan, Result,
    SortItem, StatsInfo,
};
use std::any::Any;

#[derive(Default)]
/// 逻辑 Limit：Offset/Count，可选 PartitionBy 与下推偏好。
pub struct LogicalLimit {
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    pub PartitionBy: Vec<SortItem>,
    pub Offset: u64,
    pub Count: u64,
    pub OffsetParam: Option<usize>,
    pub CountParam: Option<usize>,
    pub PreferLimitToCop: bool,
    pub IsPartial: bool,
}

impl LogicalLimit {
    /// 初始化基类逻辑计划，算子名为 Limit。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Limit", offset);
        self
    }

    /// 生成 EXPLAIN：可选 partition by，以及 offset/count。
    pub fn ExplainInfo(&self) -> String {
        let partition = self
            .PartitionBy
            .iter()
            .map(SortItem::String)
            .collect::<Vec<_>>()
            .join(", ");
        if partition.is_empty() {
            format!("offset:{}, count:{}", self.Offset, self.Count)
        } else {
            format!(
                "partition by {partition}, offset:{}, count:{}",
                self.Offset, self.Count
            )
        }
    }

    /// Go-compatible 24-byte layout: physical type, query block, offset, count.
    /// 与 Go 兼容的 24 字节布局：物理类型、查询块偏移、offset、count。
    pub fn HashCode(&self) -> [u8; 24] {
        let mut result = [0_u8; 24];
        let type_id = plancodec::TypeStringToPhysicalID(plancodec::TypeLimit) as u32;
        result[0..4].copy_from_slice(&type_id.to_be_bytes());
        result[4..8].copy_from_slice(&(self.QueryBlockOffset() as u32).to_be_bytes());
        result[8..16].copy_from_slice(&self.Offset.to_be_bytes());
        result[16..24].copy_from_slice(&self.Count.to_be_bytes());
        result
    }

    /// 谓词不下穿 Limit：清空下推到子树后将残留 Selection 挂回子节点。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        if let Some(child) = self.Children_mut().first_mut() {
            let residual = PredicatePushDownPlan(child, Vec::new())?;
            AttachSelectionToPlan(child, residual)?;
        }
        Ok(predicates)
    }

    /// 向子树裁剪列后同步 Schema，并内联投影到父侧可见列。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let visible = parent_used_cols.to_vec();
        let schema = if let Some(child) = self.Children_mut().first_mut() {
            child.PruneColumns(parent_used_cols)?;
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

    /// 构建键信息；Count==1 时标记 MaxOneRow。
    pub fn BuildKeyInfo(&mut self) {
        self.LogicalSchemaProducer.BuildKeyInfo();
        if self.Count == 1 {
            self.SetMaxOneRow(true);
        }
    }

    /// 将自身转为 TopN 下推到子树，再挂接上层 TopN。
    pub fn PushDownTopN(&mut self, upper: Option<LogicalTopN>) -> Option<LogicalPlanRef> {
        let child = self.TakeChildren().into_iter().next()?;
        let local = self.convertToTopN();
        let mut pushed = child;
        if let Some(plan) = pushed.PushDownTopN(Some(Box::new(local))) {
            pushed = plan;
        }
        Some(match upper {
            Some(top_n) => top_n.AttachChild(pushed),
            None => pushed,
        })
    }

    /// 由子统计与 Count 推导 Limit 后的行数估计。
    pub fn DeriveStats(&mut self, child_stats: &StatsInfo, reloads: &[bool]) -> (StatsInfo, bool) {
        let reload = reloads.first().copied().unwrap_or(false);
        if !reload && let Some(stats) = LogicalPlan::StatsInfo(self) {
            return (stats.clone(), false);
        }
        let stats = property::DeriveLimitStats(child_stats, self.Count as f64);
        self.SetStats(stats.clone());
        (stats, true)
    }

    /// 返回分区 Limit 的分区键（若有）。
    pub fn GetPartitionBy(&self) -> &[SortItem] {
        &self.PartitionBy
    }

    /// 将 Limit 参数映射为等价 LogicalTopN。
    fn convertToTopN(&self) -> LogicalTopN {
        let ctx = self
            .SCtx()
            .cloned()
            .expect("initialized Limit must retain planner context");
        LogicalTopN {
            Offset: self.Offset,
            Count: self.Count,
            OffsetParam: self.OffsetParam,
            CountParam: self.CountParam,
            PreferLimitToCop: self.PreferLimitToCop,
            ..LogicalTopN::default()
        }
        .Init(ctx, self.QueryBlockOffset())
    }
}

/// LogicalPlan trait 委托到 LogicalLimit 具体实现。
impl LogicalPlan for LogicalLimit {
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

    fn HashCode(&self) -> Vec<u8> {
        LogicalLimit::HashCode(self).to_vec()
    }

    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        LogicalLimit::PredicatePushDown(self, predicates)
    }

    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        LogicalLimit::PruneColumns(self, columns)
    }

    fn BuildKeyInfo(&mut self) {
        LogicalLimit::BuildKeyInfo(self)
    }

    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        let (child_stats, child_reloaded) = match self.Children_mut().first_mut() {
            Some(child) => child.DeriveStats(reload)?,
            None => (StatsInfo::default(), reload),
        };
        Ok(LogicalLimit::DeriveStats(
            self,
            &child_stats,
            &[child_reloaded],
        ))
    }
}
