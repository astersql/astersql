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

// `SHOW DDL JOBS` 对应的逻辑算子（Logical Operator）。
//
// 逻辑算子是优化器中尚未选定物理实现的计划节点。本算子表示查询 DDL
// （Data Definition Language，如 CREATE/ALTER）任务列表的语句，不扫描用户表，
// 统计信息按近似常量行数推导。

use crate::{BaseLogicalPlan, LogicalPlan, LogicalSchemaProducer, Result, StatsInfo};
use std::any::Any;

/// `SHOW DDL JOBS` 的逻辑计划节点。
///
/// `JobNumber` 限制返回的最近任务条数；schema 由 `LogicalSchemaProducer` 持有。
#[derive(Default)]
pub struct LogicalShowDDLJobs {
    /// 内嵌的 schema 生产者（含 BaseLogicalPlan 与输出列描述）。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 需要展示的 DDL 任务数量上限。
    pub JobNumber: i64,
}

impl LogicalShowDDLJobs {
    /// 用会话上下文初始化基类逻辑计划，算子名为 `"ShowDDLJobs"`。
    pub fn Init(mut self, ctx: base::ContextRef) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan =
            crate::NewBaseLogicalPlan(ctx, "ShowDDLJobs", 0);
        self
    }

    /// 推导本算子的统计信息（Cardinality Estimation，基数估计）。
    ///
    /// 无表扫描：行数固定为 1，各列 NDV（Number of Distinct Values）也记为 1。
    /// `reload=false` 且已有缓存时直接复用。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        // 未强制重算且已有缓存统计时，直接返回缓存
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let mut stats = StatsInfo {
            RowCount: 1.0,
            ..StatsInfo::default()
        };
        // 为输出 schema 中每一列写入常数 NDV
        for column in &self.Schema().Columns {
            stats.ColNDVs.insert(column.UniqueID, 1.0);
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }
}

impl LogicalPlan for LogicalShowDDLJobs {
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
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        LogicalShowDDLJobs::DeriveStats(self, reload)
    }
}
