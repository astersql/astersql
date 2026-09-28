// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 简单表达式解析与字段名查找。
//
// 对应 Go `simple_rewriter.go`。把单表达式文本包成 SELECT 再解析为内部 Expression，
// 并按库/表/列名在字段名切片中解析唯一列下标。

use crate::*;

/// 兼容旧外部仓库的入口；新调用应直接使用 ParseSimpleExpr 与 WithTableInfo。
pub fn ParseSimpleExprWithTableInfo(
    ctx: &dyn BuildContext,
    expression_text: &str,
    table_info: &model::TableInfo,
) -> Result<Box<dyn Expression>, Error> {
    ParseSimpleExpr(ctx, expression_text, vec![WithTableInfo("", table_info)])
}

/// 把简单表达式包装成 SELECT 后交给会话 SQLParser 或默认 parser，再构建内部 Expression。
pub fn ParseSimpleExpr<'a>(
    ctx: &dyn BuildContext,
    expression_text: &str,
    options: Vec<BuildOption<'a>>,
) -> Result<Box<dyn Expression>, Error> {
    if expression_text.is_empty() {
        intest::Assert(false, &[]);
        // 理论上调用方不会传空串；仍返回清晰错误，避免后续直接索引空 AST。
        return Err(errors::New("expression should not be an empty string"));
    }

    let sql = format!("select {}", expression_text);
    let (statements, warnings) = match ctx.ParseSQL(&sql) {
        Some(result) => result?,
        None => parser::New().ParseSQL(&sql, &[])?,
    };
    // 语法告警不会阻断构建，按 Go 行为转换后追加到 EvalContext。
    for warning in warnings {
        ctx.GetEvalCtx().AppendWarning(warning);
    }

    let select = statements
        .first()
        .and_then(|statement| statement.as_any().downcast_ref::<ast::SelectStmt>())
        .ok_or_else(|| errors::New("simple expression parser did not return SELECT"))?;
    let expression = select
        .Fields
        .Fields
        .first()
        .and_then(|field| field.Expr.as_ref())
        .ok_or_else(|| errors::New("simple expression parser returned an empty field list"))?;
    BuildSimpleExpr(ctx, expression, options)
}

/// 在字段名切片中按可用性、列名、可选库名和表名查找唯一列。
pub fn FindFieldName(
    names: &types::NameSlice,
    ast_column: &ast::ColumnName,
) -> Result<Option<usize>, Error> {
    let db_name = &ast_column.Schema.L;
    let table_name = &ast_column.Table.L;
    let column_name = &ast_column.Name.L;
    let mut found: Option<usize> = None;

    for (index, name) in names.0.iter().enumerate() {
        let Some(name) = name.as_deref() else {
            continue;
        };
        // 不可显式引用或列名不匹配时尽早跳过，避免错误参与歧义判断。
        if name.NotExplicitUsable || name.ColName.L != *column_name {
            continue;
        }
        if (!db_name.is_empty() && *db_name != name.DBName.L)
            || (!table_name.is_empty() && *table_name != name.TblName.L)
        {
            continue;
        }

        if let Some(previous) = found {
            // redundant 与非 redundant 同名时优先保留非冗余列；两个有效列则返回 non-unique 错误。
            let previous_name = names.0[previous]
                .as_deref()
                .expect("a previously matched field name must still be present");
            if previous_name.Redundant || name.Redundant {
                if !name.Redundant {
                    found = Some(index);
                }
                continue;
            }
            let qualified_name = [
                (!ast_column.Schema.L.is_empty()).then_some(ast_column.Schema.L.as_str()),
                (!ast_column.Table.L.is_empty()).then_some(ast_column.Table.L.as_str()),
                Some(ast_column.Name.L.as_str()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(".");
            return Err(errors::New(format!(
                "Column '{}' in field list is ambiguous",
                qualified_name
            )));
        }
        found = Some(index);
    }
    Ok(found)
}

/// 仅按列名返回首个匹配下标；None 对应 Go 的 -1。
pub fn FindFieldNameIdxByColName(names: &[types::FieldName], column_name: &str) -> Option<usize> {
    names.iter().position(|name| name.ColName.L == column_name)
}
