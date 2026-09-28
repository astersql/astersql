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

// 逻辑 CTE 表算子（LogicalCTETable）。
//
// 表示 CTE 在迭代中作为“存储表”被引用的一侧：持有种子计划的统计信息（SeedStat）
// 与模式（SeedSchema），供扫描 CTE 中间结果时复用基数估计。
// CTE（Common Table Expression）即 WITH 子句定义的公共表表达式。

use crate::{BaseLogicalPlan, LogicalPlan, LogicalSchemaProducer, Result, Schema, StatsInfo};
use std::any::Any;
use std::sync::{Arc, RwLock};

/// CTE 存储表逻辑节点：从共享 SeedStat 推导统计，不自行扫描底层物理表。
pub struct LogicalCTETable {
    /// 产出 schema 的公共嵌入字段。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 与 LogicalCTE 共享的种子统计（可写锁更新）。
    pub SeedStat: Arc<RwLock<StatsInfo>>,
    /// CTE 名称，用于 Explain/调试。
    pub Name: String,
    /// 存储侧标识，关联 CTE 定义与物化缓冲。
    pub IDForStorage: i32,
    /// 种子部分输出 schema。
    pub SeedSchema: Schema,
}

/// 默认空 CTE 表节点。
impl Default for LogicalCTETable {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            SeedStat: Arc::new(RwLock::new(StatsInfo::default())),
            Name: String::new(),
            IDForStorage: 0,
            SeedSchema: expression::NewSchema(Vec::new()),
        }
    }
}

impl LogicalCTETable {
    /// 初始化基类逻辑计划，类型标记为 CTETable，offset 为查询块偏移。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan =
            crate::NewBaseLogicalPlan(ctx, "CTETable", offset);
        self
    }

    /// 从共享 SeedStat 读取并缓存统计信息；`reload` 为 false 且已有缓存则直接返回。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        // 命中已推导统计则跳过，避免重复加锁读 SeedStat。
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let stats = self
            .SeedStat
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.SetStats(stats.clone());
        Ok((stats, true))
    }
}

/// 实现 LogicalPlan：委托基类访问，DeriveStats 走本类型实现。
impl LogicalPlan for LogicalCTETable {
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
        LogicalCTETable::DeriveStats(self, reload)
    }
}
