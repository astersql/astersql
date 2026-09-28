// Copyright 2026 AsterSQL.
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

// 全文检索（Full-Text Search，FTS）逻辑计划构建。
//
// 从解析器 AST 的 `SelectStmt` 构造 FTS 相关逻辑计划树（DataSource → Selection →
// TopN/Sort/Limit → Projection）。要求单表、TiFlash 副本可用；表达式经
// `RestoreFtsExpression` 还原为可读字符串写入算子信息。

#![allow(non_snake_case)]

use parser_ast_dependency as ast;

use crate::{FullTextIndex, PlanKind, PlanNode, StoreType};

#[derive(Clone, Debug, Eq, PartialEq)]
/// 构建 FTS 计划所需的表元数据：表 ID/名、列、全文索引与 TiFlash 可用性。
/// TiFlash 是 TiDB 的列存加速副本，FTS_MATCH_WORD 依赖其提供全文检索能力。
pub struct FtsTableMetadata {
    /// 表 ID。
    pub id: i64,
    /// 表名。
    pub name: String,
    /// 表列名列表。
    pub columns: Vec<String>,
    /// 表上已定义的全文索引。
    pub full_text_indexes: Vec<FullTextIndex>,
    /// 是否存在可用的 TiFlash 副本。
    pub tiflash_available: bool,
}

/// 从 SELECT 的 FROM 子句取出唯一物理表；多表/子查询暂不支持。
fn one_table(select: &ast::SelectStmt) -> Result<&ast::TableSource, String> {
    let from = select
        .From
        .as_ref()
        .ok_or_else(|| "FTS query requires a table".to_owned())?;
    if from.TableRefs.Right.is_some() {
        return Err("FTS query currently requires one physical table".to_owned());
    }
    match from.TableRefs.Left.as_deref() {
        Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none() => Ok(source),
        _ => Err("FTS query currently requires one physical table".to_owned()),
    }
}

/// 将常量字面量还原为 SQL 文本（字符串需转义单引号）。
fn literal(value: &ast::ValueExpr) -> String {
    match &value.Datum {
        ast::ValueDatum::String(value) | ast::ValueDatum::Decimal(value) => {
            format!("'{}'", value.replace('\'', "''"))
        }
        ast::ValueDatum::Bytes(value) => {
            format!("'{}'", String::from_utf8_lossy(value).replace('\'', "''"))
        }
        _ => value.text(),
    }
}

/// 将 FTS 相关表达式 AST 递归还原为 SQL 字符串，供计划算子信息使用。
pub fn RestoreFtsExpression(expression: &ast::ExprNode) -> Result<String, String> {
    Ok(match &expression.Kind {
        ast::ExprKind::Value(value) => literal(value),
        ast::ExprKind::Column(column) => column.Name.O.clone(),
        ast::ExprKind::Function {
            Schema,
            FnName,
            Args,
        } => {
            let arguments = Args
                .iter()
                .map(RestoreFtsExpression)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            if Schema.O.is_empty() {
                format!("{}({arguments})", FnName.O)
            } else {
                format!("{}.{}({arguments})", Schema.O, FnName.O)
            }
        }
        ast::ExprKind::Binary { Op, L, R } => {
            format!(
                "{} {} {}",
                RestoreFtsExpression(L)?,
                Op,
                RestoreFtsExpression(R)?
            )
        }
        ast::ExprKind::Unary { Op, V } => format!("{Op}{}", RestoreFtsExpression(V)?),
        ast::ExprKind::Parentheses(value) => format!("({})", RestoreFtsExpression(value)?),
        ast::ExprKind::ParamMarker { .. } => "?".to_owned(),
        _ => return Err("unsupported expression in FTS planner".to_owned()),
    })
}

/// 读取 LIMIT/OFFSET 字面量；缺省时返回 `default`，非非负整数则报错。
fn read_limit_value(value: Option<&ast::ExprNode>, default: u64) -> Result<u64, String> {
    let Some(value) = value else {
        return Ok(default);
    };
    let ast::ExprKind::Value(value) = &value.Kind else {
        return Err("LIMIT must be a non-negative integer".to_owned());
    };
    match value.Datum {
        ast::ValueDatum::Uint64(value) => Ok(value),
        ast::ValueDatum::Int64(value) if value >= 0 => Ok(value as u64),
        _ => Err("LIMIT must be a non-negative integer".to_owned()),
    }
}

/// 从规范解析器 AST 构建 FTS 逻辑计划片段。
/// 不接受预渲染 SQL 或预先算好的 EXPLAIN 行。
/// Builds the FTS portion of a logical plan from the canonical parser AST.
/// It does not accept pre-rendered SQL or precomputed EXPLAIN rows.
pub fn BuildFullTextPlan(
    select: &ast::SelectStmt,
    table: &FtsTableMetadata,
) -> Result<PlanNode, String> {
    let source = one_table(select)?;
    if !source.Source.Name.L.eq_ignore_ascii_case(&table.name) {
        return Err(format!("table {} was not found", source.Source.Name.O));
    }
    if !table.tiflash_available {
        return Err("FTS_MATCH_WORD() requires an available TiFlash replica".to_owned());
    }

    let mut plan = PlanNode::New(
        1,
        PlanKind::DataSource {
            table: table.name.clone(),
            alias: (!source.AsName.O.is_empty()).then(|| source.AsName.O.clone()),
            partition_id: None,
        },
        Vec::new(),
    );
    // FTS 扫描必须落在 TiFlash 存储引擎上。
    plan.store_type = StoreType::TiFlash;
    plan.access_object = table.name.clone();

    // WHERE 谓词包装为 Selection（过滤）节点。
    if let Some(condition) = &select.Where {
        plan = PlanNode::New(
            2,
            PlanKind::Selection {
                conditions: vec![RestoreFtsExpression(condition)?],
            },
            vec![plan],
        );
    }

    // 有 ORDER BY：若同时有 LIMIT 则用 TopN（排序取前 N），否则用 Sort。
    if !select.OrderBy.is_empty() {
        let by_items = select
            .OrderBy
            .iter()
            .map(|item| {
                RestoreFtsExpression(&item.Expr).map(|value| {
                    if item.Desc {
                        format!("{value} DESC")
                    } else {
                        value
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(limit) = &select.Limit {
            let offset = read_limit_value(limit.Offset.as_ref(), 0)?;
            let count = read_limit_value(limit.Count.as_ref(), u64::MAX)?;
            plan = PlanNode::New(
                3,
                PlanKind::TopN {
                    by_items,
                    offset,
                    count,
                },
                vec![plan],
            );
        } else {
            let mut sort = PlanNode::New(3, PlanKind::Sort, vec![plan]);
            sort.operator_info = by_items.join(", ");
            plan = sort;
        }
    // 仅有 LIMIT、无 ORDER BY：直接挂 Limit 节点。
    } else if let Some(limit) = &select.Limit {
        let offset = read_limit_value(limit.Offset.as_ref(), 0)?;
        let count = read_limit_value(limit.Count.as_ref(), u64::MAX)?;
        plan = PlanNode::New(3, PlanKind::Limit { offset, count }, vec![plan]);
    }

    // 非通配符投影列：再包一层 Projection。
    let explicit_fields: Vec<_> = select
        .Fields
        .Fields
        .iter()
        .filter(|field| field.WildCard.is_none())
        .collect();
    if !explicit_fields.is_empty() {
        let mut projection = PlanNode::New(4, PlanKind::Projection, vec![plan]);
        projection.operator_info = explicit_fields
            .iter()
            .map(|field| {
                field
                    .Expr
                    .as_ref()
                    .ok_or_else(|| "SELECT field has no expression".to_owned())
                    .and_then(RestoreFtsExpression)
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        plan = projection;
    }
    Ok(plan)
}

/// 返回表元数据中的全文索引切片。
pub fn FullTextIndexes(table: &FtsTableMetadata) -> &[FullTextIndex] {
    &table.full_text_indexes
}
