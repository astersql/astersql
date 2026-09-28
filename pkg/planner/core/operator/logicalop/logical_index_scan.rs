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

// 逻辑索引扫描（LogicalIndexScan）算子。
//
// 表示沿二级索引或主键索引读取的访问路径；可能双读（Double Read：
// 先读索引再回表），并携带访问条件、索引过滤与 Range。

use crate::*;
use std::any::Any;

/// 逻辑索引扫描：绑定 DataSource、索引元数据与可访问 Range。
pub struct LogicalIndexScan {
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    pub Source: Option<DataSourceRef>,
    pub IsDoubleRead: bool,
    pub EqCondCount: usize,
    pub AccessConds: Vec<Expression>,
    pub IndexFilters: Vec<Expression>,
    pub Ranges: Vec<ranger::Range>,
    pub Index: model::IndexInfo,
    pub Columns: Vec<model::ColumnInfo>,
    pub FullIdxCols: Vec<Column>,
    pub FullIdxColLens: Vec<isize>,
    pub IdxCols: Vec<Column>,
    pub IdxColLens: Vec<isize>,
    pub NoncacheableReason: String,
}

/// 默认空索引扫描（无 Source、无条件）。
impl Default for LogicalIndexScan {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            Source: None,
            IsDoubleRead: false,
            EqCondCount: 0,
            AccessConds: Vec::new(),
            IndexFilters: Vec::new(),
            Ranges: Vec::new(),
            Index: model::IndexInfo::default(),
            Columns: Vec::new(),
            FullIdxCols: Vec::new(),
            FullIdxColLens: Vec::new(),
            IdxCols: Vec::new(),
            IdxColLens: Vec::new(),
            NoncacheableReason: String::new(),
        }
    }
}

impl LogicalIndexScan {
    /// 初始化基类逻辑计划，算子名为 IndexScan。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "IndexScan", offset);
        self
    }

    /// 生成 EXPLAIN 文本：表信息、索引列名与访问条件。
    pub fn ExplainInfo(&self) -> String {
        let Some(source) = &self.Source else {
            return "index scan".to_owned();
        };
        let source = source.borrow();
        // 隐藏列用生成表达式字符串，普通列用索引列名。
        let columns = self
            .Index
            .Columns
            .iter()
            .filter_map(|index_column| {
                source
                    .TableInfo
                    .Columns
                    .get(index_column.Offset as usize)
                    .map(|column| {
                        if column.Hidden {
                            column.GeneratedExprString.clone()
                        } else {
                            index_column.Name.O.clone()
                        }
                    })
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut result = source.ExplainInfo();
        if !columns.is_empty() {
            result.push_str(&format!(", index:{columns}"));
        }
        if !self.AccessConds.is_empty() {
            let parameters = self.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
            let conditions = self
                .AccessConds
                .iter()
                .map(|expr| expr.StringWithCtx(parameters.map(|ctx| ctx as _), ""))
                .collect::<Vec<_>>()
                .join(", ");
            result.push_str(&format!(", cond:{conditions}"));
        }
        result
    }

    /// 按 Source 的全部索引路径重建键信息，并补充整数句柄主键。
    pub fn BuildKeyInfo(&mut self) {
        let Some(source) = &self.Source else { return };
        let source = source.borrow();
        let mut strong = Vec::new();
        let mut nullable = Vec::new();
        for path in &source.AllPossibleAccessPaths {
            if path.IsTablePath() {
                continue;
            }
            if let Some(index) = &path.Index {
                let (unique_key, new_key) =
                    rule_util::CheckIndexCanBeKey(index, &self.Columns, self.Schema());
                if let Some(new_key) = new_key {
                    strong.push(new_key);
                } else if let Some(unique_key) = unique_key {
                    nullable.push(unique_key);
                }
            }
        }
        if let Some(handle) = self.GetPKIsHandleCol(self.Schema()) {
            strong.push(vec![handle]);
        }
        let schema = self.LogicalSchemaProducer.Schema_mut();
        schema.PKOrUK = strong;
        schema.NullableUK = nullable;
    }

    /// 推导统计：优先用匹配索引路径的 CountAfterAccess，并钳制 NDV。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let mut stats = self
            .Source
            .as_ref()
            .map(|source| source.borrow().TableStats.clone())
            .unwrap_or_default();
        if let Some(path_count) = self.Source.as_ref().and_then(|source| {
            source
                .borrow()
                .PossibleAccessPaths
                .iter()
                .find(|path| {
                    path.Index
                        .as_ref()
                        .is_some_and(|index| index.ID == self.Index.ID)
                })
                .map(|path| path.CountAfterAccess)
        }) && path_count > 0.0
        {
            stats.RowCount = path_count;
        }
        for ndv in stats.ColNDVs.values_mut() {
            *ndv = ndv.min(stats.RowCount);
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 枚举索引后缀有序性（跳过等式前缀后的列序）。
    pub fn PreparePossibleProperties(&self) -> PossiblePropertiesInfo {
        let mut orders = Vec::new();
        if !self.IdxCols.is_empty() {
            for offset in 0..=self.EqCondCount.min(self.IdxCols.len() - 1) {
                orders.push(self.IdxCols[offset..].to_vec());
            }
        }
        PossiblePropertiesInfo {
            Orders: orders,
            HasTiFlash: self
                .Source
                .as_ref()
                .is_some_and(|source| source.borrow().HasTiFlash())
                && self
                    .SCtx()
                    .is_some_and(|context| context.GetSessionVars().IsMPPAllowed()),
        }
    }

    /// 判断物理属性的排序需求是否与索引列序匹配。
    pub fn MatchIndexProp(&self, prop: &property::PhysicalProperty) -> bool {
        if prop.SortItems.is_empty() {
            return true;
        }
        if !prop.AllSameOrder().0 {
            return false;
        }
        // 允许在等式前缀内滑动起点，再匹配剩余索引列与 SortItems。
        let eval_ctx = self.SCtx().map(|context| context.GetExprCtx().GetEvalCtx());
        for (offset, column) in self.IdxCols.iter().enumerate() {
            let matches = eval_ctx.map_or_else(
                || column.UniqueID == prop.SortItems[0].Col.UniqueID,
                |ctx| column.EqualByExprAndID(ctx, &prop.SortItems[0].Col),
            );
            if matches {
                return matchIndicesPropWithCtx(
                    &self.IdxCols[offset..],
                    &self.IdxColLens[offset..],
                    &prop.SortItems,
                    eval_ctx,
                );
            }
            if offset >= self.EqCondCount {
                break;
            }
        }
        false
    }

    /// 当表以整数句柄为主键时，返回 Schema 中对应的主键列。
    pub fn GetPKIsHandleCol(&self, schema: &Schema) -> Option<Column> {
        let source = self.Source.as_ref()?.borrow();
        if !source.TableInfo.PKIsHandle {
            return None;
        }
        self.Columns.iter().find_map(|info| {
            mysql::r#type::HasPriKeyFlag(info.GetFlag())
                .then(|| {
                    schema
                        .Columns
                        .iter()
                        .find(|column| column.ID == info.ID)
                        .cloned()
                })
                .flatten()
        })
    }
}

/// 检查索引列前缀（全长）是否覆盖所需 SortItem 序列。
pub fn matchIndicesProp(idx_cols: &[Column], col_lens: &[isize], prop_items: &[SortItem]) -> bool {
    matchIndicesPropWithCtx(idx_cols, col_lens, prop_items, None)
}

fn matchIndicesPropWithCtx(
    idx_cols: &[Column],
    col_lens: &[isize],
    prop_items: &[SortItem],
    eval_ctx: Option<&dyn expression::EvalContext>,
) -> bool {
    idx_cols.len() >= prop_items.len()
        && col_lens.len() >= prop_items.len()
        && prop_items.iter().enumerate().all(|(index, item)| {
            col_lens[index] == expression::types::UnspecifiedLength as isize
                && eval_ctx.map_or_else(
                    || idx_cols[index].UniqueID == item.Col.UniqueID,
                    |ctx| item.Col.EqualByExprAndID(ctx, &idx_cols[index]),
                )
        })
}

/// LogicalPlan trait 委托到 LogicalIndexScan 具体实现。
impl LogicalPlan for LogicalIndexScan {
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
        LogicalIndexScan::ExplainInfo(self)
    }
    fn BuildKeyInfo(&mut self) {
        LogicalIndexScan::BuildKeyInfo(self)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        LogicalIndexScan::DeriveStats(self, reload)
    }
}
