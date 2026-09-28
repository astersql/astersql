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

// `ORDER BY` 对应的逻辑排序算子。
//
// 在逻辑计划中表示按一组表达式排序输入行。优化阶段可与 TopN/Limit 合并下推，
// 并据此推导有序属性（Possible Properties），供物理实现选择排序或利用已有序流。

use crate::{
    BaseLogicalPlan, ByItems, Column, CorrelatedColumn, LogicalPlan, LogicalPlanRef, LogicalTopN,
    NewBaseLogicalPlan, Result, Schema,
};
use expression::Expression as _;
use std::any::Any;
use std::collections::{HashMap, HashSet};

/// 子树可提供的排序属性信息。
///
/// `Orders` 为多组有序列前缀；`HasTiFlash` 标记路径上是否触及 TiFlash（列存引擎）扫描。
#[derive(Clone, Default)]
pub struct SortProperties {
    /// 可能的有序列组合（每组是一个排序前缀）。
    pub Orders: Vec<Vec<Column>>,
    /// 是否存在 TiFlash 访问路径。
    pub HasTiFlash: bool,
}

/// 逻辑排序算子：按 `ByItems` 中的表达式对子节点输出排序。
pub struct LogicalSort {
    /// 基类逻辑计划（子节点、上下文等）。
    pub BaseLogicalPlan: BaseLogicalPlan,
    /// `ORDER BY` 项列表（表达式 + 升/降序）。
    pub ByItems: Vec<ByItems>,
}

impl Default for LogicalSort {
    fn default() -> Self {
        Self {
            BaseLogicalPlan: BaseLogicalPlan::default(),
            ByItems: Vec::new(),
        }
    }
}

impl LogicalSort {
    /// 初始化基类计划，算子名为 `"Sort"`；`offset` 为查询块偏移。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Sort", offset);
        self
    }

    /// 生成 EXPLAIN 中展示的排序表达式文本。
    pub fn ExplainInfo(&self) -> String {
        let context = self.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        self.ByItems
            .iter()
            .map(|item| {
                let expression = item.Expr.StringWithCtx(context.map(|ctx| ctx as _), "");
                if item.Desc {
                    format!("{expression}:desc")
                } else {
                    expression
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// 按列 HashCode 替换排序表达式中的列引用（列裁剪/重写后同步更新）。
    pub fn ReplaceExprColumns(&mut self, replace: &HashMap<Vec<u8>, Column>) {
        for mut item in std::mem::take(&mut self.ByItems) {
            item.Expr = rule_util::ResolveExprAndReplace(item.Expr, replace);
            self.ByItems.push(item);
        }
    }

    /// 列裁剪：先去重/丢弃无用排序项，再把父层与排序所需列一并下推给子节点。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let (items, columns) = pruneSortByItems(std::mem::take(&mut self.ByItems));
        self.ByItems = items;
        let mut used = parent_used_cols.to_vec();
        used.extend(columns);
        if let Some(child) = self.BaseLogicalPlan.Children_mut().first_mut() {
            child.PruneColumns(&used)?;
        }
        Ok(())
    }

    /// 将 TopN（带 Limit 的排序）下推：纯 Limit 时把本算子的 ByItems 注入 TopN 再继续下推。
    pub fn PushDownTopN(&mut self, top_n: Option<LogicalTopN>) -> Option<LogicalPlanRef> {
        let Some(mut top_n) = top_n else {
            return self.BaseLogicalPlan.PushDownTopN(None);
        };
        // 纯 Limit（无 ORDER BY）可吸收当前 Sort 的排序键，合并为 TopN
        if top_n.IsLimit() {
            top_n.ByItems = self.ByItems.clone();
        }
        self.BaseLogicalPlan
            .Children_mut()
            .first_mut()
            .and_then(|child| child.PushDownTopN(Some(Box::new(top_n))))
    }

    /// 根据排序键推导可能的有序属性；仅连续的列引用构成有序前缀。
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

    /// 从排序表达式中提取相关列（Correlated Column，外层引用）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.ByItems
            .iter()
            .flat_map(|item| expression::ExtractCorColumns(item.Expr.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 收集排序表达式引用的全部列。
    pub fn GetUsedCols(&self) -> Vec<Column> {
        self.ByItems
            .iter()
            .flat_map(|item| expression::ExtractColumns(item.Expr.as_ref()))
            .cloned()
            .collect()
    }
}

impl LogicalPlan for LogicalSort {
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
    fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        LogicalSort::PruneColumns(self, parent_used_cols)
    }
}

/// 裁剪排序项：按表达式 HashCode 去重，并丢弃无列依赖的运行时常量表达式。
///
/// 返回保留的 ByItems 以及这些项引用的列集合。
pub fn pruneSortByItems(items: Vec<ByItems>) -> (Vec<ByItems>, Vec<Column>) {
    let mut seen = HashSet::new();
    let mut kept = Vec::with_capacity(items.len());
    let mut used = Vec::new();
    for item in items {
        let hash = item.Expr.HashCode();
        // 相同表达式只保留第一次出现
        if !seen.insert(hash) {
            continue;
        }
        let columns = expression::ExtractColumns(item.Expr.as_ref());
        // 运行时常量且不引用列时，排序键无实际效果，可丢弃
        if columns.is_empty() && expression::IsRuntimeConstExpr(item.Expr.as_ref()) {
            continue;
        }
        if !columns.is_empty() && sortExpressionHasNullType(item.Expr.as_ref()) {
            continue;
        }
        used.extend(columns.into_iter().cloned());
        kept.push(item);
    }
    (kept, used)
}

/// 判断带列依赖的排序表达式是否具有 NULL 返回类型。
fn sortExpressionHasNullType(expr: &dyn expression::Expression) -> bool {
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        return column
            .RetType
            .as_ref()
            .is_some_and(|field_type| field_type.GetType() == expression::mysql::TypeNull);
    }
    if let Some(correlated) = expr.as_any().downcast_ref::<CorrelatedColumn>() {
        return correlated
            .column
            .RetType
            .as_ref()
            .is_some_and(|field_type| field_type.GetType() == expression::mysql::TypeNull);
    }
    if let Some(function) = expr.as_any().downcast_ref::<expression::ScalarFunction>() {
        return function.GetStaticType().GetType() == expression::mysql::TypeNull;
    }
    false
}

/// 从 ByItems 中提取连续的纯列引用前缀，作为可能的有序属性。
///
/// 一旦遇到非列表达式即停止（`map_while`），因为其后无法保证列序。
pub fn getPossiblePropertyFromByItems(items: &[ByItems]) -> Vec<Column> {
    items
        .iter()
        .map_while(|item| item.Expr.as_any().downcast_ref::<Column>().cloned())
        .collect()
}
