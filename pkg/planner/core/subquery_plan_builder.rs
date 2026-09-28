// Copyright 2026 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// 子查询（subquery）计划构建辅助。
//
// 从表达式树抽取子查询节点，解析相关列（correlated column：外层查询
// 注入内层的列引用），并在 SELECT 列表中补齐相关列作为辅助投影字段，
// 以便后续解相关（decorrelation）与 Apply 改写。

use crate::logical_plan_builder::SelectField;
use crate::planbuilder::{PlanBuilder, Result};
use crate::task::{Expression, PlanKind, PlanNode};
use std::collections::HashSet;

/// 表达式访问器：收集子查询相关表达式。
#[derive(Clone, Debug, Default)]
pub struct subqueryExprExtractor {
    /// 已抽取的子查询表达式列表。
    pub exprs: Vec<Expression>,
}

impl subqueryExprExtractor {
    /// 进入节点：若是子查询表达式则收录并跳过其子树。
    pub fn Enter(&mut self, expression: &Expression) -> bool {
        if is_subquery_expression(expression) {
            self.exprs.push(expression.clone());
            return true;
        }
        false
    }
    /// 离开节点：始终继续遍历。
    pub fn Leave(&self, _expression: &Expression) -> bool {
        true
    }
}

/// 按表达式名前缀判定是否为 subquery / exists / compare / in 子查询。
fn is_subquery_expression(expression: &Expression) -> bool {
    let name = expression.name.to_ascii_lowercase();
    name.starts_with("subquery:")
        || name.starts_with("exists-subquery:")
        || name.starts_with("compare-subquery:")
        || name.starts_with("in-subquery:")
}

/// 三层限定列名：schema.table.column。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnName {
    pub schema: String,
    pub table: String,
    pub name: String,
}

/// 按列唯一 ID 在计划树标签中查找对应列名；Join/Apply 会再试 full-column 标签。
pub fn findColumnNameByUniqueID(plan: &PlanNode, uniqueID: i64) -> Option<ColumnName> {
    if let Some(name) = lookup_column_label(plan, "column", uniqueID) {
        return Some(name);
    }
    // Join/Apply 的输出列可能挂在 full-column 标签上。
    if matches!(
        plan.kind,
        PlanKind::HashJoin | PlanKind::MergeJoin | PlanKind::Apply
    ) {
        if let Some(name) = lookup_column_label(plan, "full-column", uniqueID) {
            return Some(name);
        }
    }
    // 一元链则向唯一子节点继续查找。
    if plan.children.len() == 1 {
        return findColumnNameByUniqueID(&plan.children[0], uniqueID);
    }
    None
}

/// 从 `labels` 键解析 `{prefix}:{id}:schema.table.name` 形式的列标签。
fn lookup_column_label(plan: &PlanNode, prefix: &str, unique_id: i64) -> Option<ColumnName> {
    let key = format!("{prefix}:{unique_id}:");
    plan.labels.keys().find_map(|label| {
        let rest = label.strip_prefix(&key)?;
        let mut parts = rest.splitn(3, '.');
        Some(ColumnName {
            schema: parts.next().unwrap_or_default().into(),
            table: parts.next().unwrap_or_default().into(),
            name: parts.next().unwrap_or(rest).into(),
        })
    })
}

/// 从表达式名中解析 `corr=<id>` 片段，得到相关列唯一 ID 列表。
fn correlated_ids(expression: &Expression) -> Vec<i64> {
    expression
        .name
        .split("corr=")
        .skip(1)
        .filter_map(|part| {
            let digits: String = part
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            digits.parse().ok()
        })
        .collect()
}

/// 判断 SELECT 字段是否已覆盖指定列（无别名、允许 schema/table 限定）。
fn field_matches_column(field: &SelectField, column: &ColumnName) -> bool {
    if field.alias.is_some() {
        return false;
    }
    let name = field.expr.name.to_ascii_lowercase();
    let col = column.name.to_ascii_lowercase();
    name == col
        || name == format!("{}.{}", column.table.to_ascii_lowercase(), col)
        || name
            == format!(
                "{}.{}.{}",
                column.schema.to_ascii_lowercase(),
                column.table.to_ascii_lowercase(),
                col
            )
}

impl PlanBuilder {
    /// 为子查询中的相关列追加辅助 SELECT 字段，避免投影裁剪后丢失外层引用。
    pub fn appendAuxiliaryFieldsForSubqueries(
        &mut self,
        plan: &PlanNode,
        selectFields: &[SelectField],
        nodes: &[Expression],
    ) -> Result<Vec<SelectField>> {
        let mut fields = selectFields.to_vec();
        let mut appended = HashSet::new();
        for node in nodes {
            let mut extractor = subqueryExprExtractor::default();
            if !extractor.Enter(node) {
                continue;
            }
            for expression in extractor.exprs {
                for unique_id in correlated_ids(&expression) {
                    let Some(column_name) = findColumnNameByUniqueID(plan, unique_id) else {
                        continue;
                    };
                    // 已在 SELECT 列表中则无需再追加。
                    if fields
                        .iter()
                        .any(|field| field_matches_column(field, &column_name))
                    {
                        continue;
                    }
                    // 按可用限定层级拼出列引用名，并用 HashSet 去重。
                    let qualified = if column_name.table.is_empty() {
                        column_name.name.clone()
                    } else if column_name.schema.is_empty() {
                        format!("{}.{}", column_name.table, column_name.name)
                    } else {
                        format!(
                            "{}.{}.{}",
                            column_name.schema, column_name.table, column_name.name
                        )
                    };
                    if appended.insert(qualified.clone()) {
                        fields.push(SelectField {
                            expr: Expression {
                                name: qualified,
                                column: None,
                                function_count: 0,
                                virtual_column: false,
                                return_type: None,
                            },
                            alias: None,
                            wildcard: false,
                            table_wildcard: None,
                        });
                    }
                }
            }
        }
        Ok(fields)
    }
}
