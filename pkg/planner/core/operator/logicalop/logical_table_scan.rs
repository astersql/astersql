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

// 逻辑表扫描（Table Scan）算子。
//
// 表示按主键/表路径访问一张物理表：持有数据源、句柄列（Handle/PK）、访问条件、
// 范围（Range）与存储类型。统计信息优先取自访问路径的 `CountAfterAccess`。

use crate::*;
use expression::Expression as _;
use std::any::Any;

/// 对基表做全表或范围扫描的逻辑算子。
pub struct LogicalTableScan {
    /// Schema 与基类逻辑计划。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 表数据源引用（含列、统计、访问路径）。
    pub Source: Option<DataSourceRef>,
    /// 句柄列（Handle Cols）：主键或隐式 `_tidb_rowid`，用于定位行。
    pub HandleCols: Option<Box<dyn HandleCols>>,
    /// 可用于构造扫描范围的访问条件（Access Conditions）。
    pub AccessConds: Vec<Expression>,
    /// 无法下推为范围、需在扫描后过滤的表级条件。
    pub TableFilters: Vec<Expression>,
    /// 根据访问条件推导出的键范围列表。
    pub Ranges: Vec<ranger::Range>,
    /// 目标存储（TiKV / TiFlash 等）。
    pub StoreType: kv::StoreType,
}

impl Default for LogicalTableScan {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            Source: None,
            HandleCols: None,
            AccessConds: Vec::new(),
            TableFilters: Vec::new(),
            Ranges: Vec::new(),
            StoreType: kv::StoreType::TiKV,
        }
    }
}

impl LogicalTableScan {
    /// 初始化算子名为 `"TableScan"`。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "TableScan", offset);
        self
    }

    /// EXPLAIN：表名、主键列、访问条件个数。
    pub fn ExplainInfo(&self) -> String {
        let Some(source) = &self.Source else {
            return "table scan".to_owned();
        };
        let source = source.borrow();
        let mut result = source.ExplainInfo();
        if let Some(handle) = &source.HandleCols {
            let parameters = self.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
            result.push_str(&format!(
                ", pk col:{}",
                handle.StringWithCtx(parameters.map(|ctx| ctx as _), "")
            ));
        }
        if !self.AccessConds.is_empty() {
            let parameters = self.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
            let conditions = self
                .AccessConds
                .iter()
                .map(|condition| condition.StringWithCtx(parameters.map(|ctx| ctx as _), ""))
                .collect::<Vec<_>>()
                .join(", ");
            result.push_str(&format!(", cond:{conditions}"));
        }
        result
    }

    /// 从数据源复制主键/唯一键信息到本算子 schema。
    pub fn BuildKeyInfo(&mut self) {
        let Some(source) = &self.Source else { return };
        source.borrow_mut().BuildKeyInfo();
        let source = source.borrow();
        self.LogicalSchemaProducer.Schema_mut().PKOrUK = source.Schema().PKOrUK.clone();
        self.LogicalSchemaProducer.Schema_mut().NullableUK = source.Schema().NullableUK.clone();
    }

    /// 推导统计：默认用表级统计，若存在表路径且 `CountAfterAccess > 0` 则覆盖行数。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let mut stats = self
            .Source
            .as_ref()
            .map(|source| source.borrow().TableStats.clone())
            .unwrap_or_default();
        // 表访问路径在过滤后的基数估计更准确时，覆盖默认表行数
        if let Some(path_count) = self.Source.as_ref().and_then(|source| {
            source
                .borrow()
                .PossibleAccessPaths
                .iter()
                .find(|path| path.IsTablePath())
                .map(|path| path.CountAfterAccess)
        }) && path_count > 0.0
        {
            stats.RowCount = path_count;
        }
        for ndv in stats.ColNDVs.values_mut() {
            *ndv = ndv.min(stats.RowCount);
        }
        if let Some(handle) = &self.HandleCols {
            let context = self
                .SCtx()
                .ok_or_else(|| PlannerError("LogicalTableScan has no plan context".to_owned()))?;
            let field_type = handle
                .GetCol(0)
                .and_then(|column| column.RetType.as_ref())
                .ok_or_else(|| PlannerError("table scan handle column has no type".to_owned()))?;
            let mut ranger_context = context.GetRangerCtx().clone();
            self.Ranges = ranger::BuildTableRange(
                self.AccessConds.clone(),
                &mut ranger_context,
                field_type,
                0,
            )
            .map_err(|error| PlannerError(error.to_string()))?
            .0
            .0;
        } else {
            let is_unsigned = self.Source.as_ref().is_some_and(|source| {
                let source = source.borrow();
                source.TableInfo.PKIsHandle
                    && source
                        .TableInfo
                        .GetPkColInfo()
                        .is_some_and(|column| mysql::r#type::HasUnsignedFlag(column.GetFlag()))
            });
            self.Ranges = ranger::FullIntRange(is_unsigned).0;
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 以句柄列顺序作为可能的有序属性；并标记是否含 TiFlash 路径。
    pub fn PreparePossibleProperties(&self) -> PossiblePropertiesInfo {
        let order = self
            .HandleCols
            .as_ref()
            .map(|handle| {
                handle
                    .IterColumns2()
                    .map(|(_, column)| column.clone())
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .collect();
        PossiblePropertiesInfo {
            Orders: order,
            HasTiFlash: self
                .Source
                .as_ref()
                .is_some_and(|source| source.borrow().HasTiFlash())
                && self
                    .SCtx()
                    .is_some_and(|context| context.GetSessionVars().IsMPPAllowed()),
        }
    }
}

impl LogicalPlan for LogicalTableScan {
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
        LogicalTableScan::ExplainInfo(self)
    }
    fn BuildKeyInfo(&mut self) {
        LogicalTableScan::BuildKeyInfo(self)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        LogicalTableScan::DeriveStats(self, reload)
    }
}
