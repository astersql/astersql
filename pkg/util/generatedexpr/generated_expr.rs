// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 生成列表达式解析与简单列名解析。
//
// 对应 Go `pkg/util/generatedexpr`：`ParseExpression` 将表达式包进
// `SELECT` 再经 TiDB 解析器取出字段表达式；`SimpleResolveName` 对照
// 表元数据校验表达式中出现的列名（大小写不敏感，经 `CIStr.L`）。

use crate::{ast, charset, errors, model, parser, parser_core::mysql, parserutil};

/// TiDB 语法错误文案前缀，与 Go 侧 `parser` 报错包装一致。
const SYNTAX_ERROR_PREFIX: &str = "You have an error in your SQL syntax; check the manual that corresponds to your TiDB version for the right syntax to use";

/// 按表列清单解析表达式中的列引用；未知列返回 Go 风格错误。
struct NameResolver<'a> {
    table_info: &'a model::TableInfo,
}

impl NameResolver<'_> {
    /// 在 `table_info.Columns` 中按小写名查找列；找不到则报错。
    fn resolve_column(&self, column: &ast::ColumnName) -> Result<(), errors::Error> {
        if self
            .table_info
            .Columns
            .iter()
            .any(|candidate| candidate.Name.L == column.Name.L)
        {
            return Ok(());
        }

        Err(errors::New(format!(
            "can't find column {} in {}",
            column.Name.O, self.table_info.Name.O
        )))
    }

    /// 依次解析表达式切片中的每个节点。
    fn resolve_exprs(&self, expressions: &[ast::ExprNode]) -> Result<(), errors::Error> {
        expressions
            .iter()
            .try_for_each(|expression| self.resolve_expr(expression))
    }

    /// 解析 ORDER BY / GROUP BY 等 `ByItem` 列表中的表达式。
    fn resolve_items(&self, items: &[ast::ByItem]) -> Result<(), errors::Error> {
        items
            .iter()
            .try_for_each(|item| self.resolve_expr(&item.Expr))
    }

    /// 解析窗口规格：PARTITION BY、ORDER BY 以及 frame 起止表达式。
    fn resolve_window(&self, spec: &ast::WindowSpec) -> Result<(), errors::Error> {
        self.resolve_items(&spec.PartitionBy)?;
        self.resolve_items(&spec.OrderBy)?;
        if let Some(frame) = &spec.Frame {
            if let Some(expression) = &frame.Extent.Start.Expr {
                self.resolve_expr(expression)?;
            }
            if let Some(expression) = &frame.Extent.End.Expr {
                self.resolve_expr(expression)?;
            }
        }
        Ok(())
    }

    /// 深度遍历 SELECT 语句子树中的投影、过滤、分组、排序、LIMIT、窗口等表达式。
    fn resolve_node(&self, node: &dyn ast::Node) -> Result<(), errors::Error> {
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            for field in &select.Fields.Fields {
                if let Some(expression) = &field.Expr {
                    self.resolve_expr(expression)?;
                }
            }
            if let Some(expression) = &select.Where {
                self.resolve_expr(expression)?;
            }
            self.resolve_items(&select.GroupBy)?;
            if let Some(expression) = &select.Having {
                self.resolve_expr(expression)?;
            }
            self.resolve_items(&select.OrderBy)?;
            if let Some(limit) = &select.Limit {
                if let Some(expression) = &limit.Count {
                    self.resolve_expr(expression)?;
                }
                if let Some(expression) = &limit.Offset {
                    self.resolve_expr(expression)?;
                }
            }
            for row in &select.Lists {
                self.resolve_exprs(&row.Values)?;
            }
            for spec in &select.WindowSpecs {
                self.resolve_window(spec)?;
            }
            for child in &select.children {
                self.resolve_node(child.as_ref())?;
            }
        }
        Ok(())
    }

    /// 按 `ExprKind` 递归下降：列名走 `resolve_column`，复合节点展开子表达式。
    fn resolve_expr(&self, expression: &ast::ExprNode) -> Result<(), errors::Error> {
        use ast::ExprKind;

        match &expression.Kind {
            ExprKind::Column(column) => self.resolve_column(column),
            ExprKind::Variable { Value, .. } => Value
                .as_deref()
                .map_or(Ok(()), |value| self.resolve_expr(value)),
            ExprKind::Function { Args, .. } | ExprKind::Row(Args) => self.resolve_exprs(Args),
            ExprKind::AggregateFunction { Args, Order, .. } => {
                self.resolve_exprs(Args)?;
                self.resolve_items(Order)
            }
            ExprKind::Binary { L, R, .. } | ExprKind::CompareSubquery { L, R, .. } => {
                self.resolve_expr(L)?;
                self.resolve_expr(R)
            }
            ExprKind::Unary { V, .. }
            | ExprKind::Parentheses(V)
            | ExprKind::ExistsSubquery { Sel: V, .. }
            | ExprKind::IsTruth { Expr: V, .. }
            | ExprKind::IsNull { Expr: V, .. }
            | ExprKind::Collate { Expr: V, .. }
            | ExprKind::Cast { Expr: V, .. }
            | ExprKind::JSONSumCrc32 { Expr: V, .. } => self.resolve_expr(V),
            ExprKind::InList { Expr, List, .. } => {
                self.resolve_expr(Expr)?;
                self.resolve_exprs(List)
            }
            ExprKind::Between {
                Expr, Left, Right, ..
            } => {
                self.resolve_expr(Expr)?;
                self.resolve_expr(Left)?;
                self.resolve_expr(Right)
            }
            ExprKind::Like { Expr, Pattern, .. } | ExprKind::Regexp { Expr, Pattern, .. } => {
                self.resolve_expr(Expr)?;
                self.resolve_expr(Pattern)
            }
            ExprKind::MatchAgainst { Against, .. } => self.resolve_expr(Against),
            ExprKind::Case {
                Value,
                WhenClauses,
                ElseClause,
            } => {
                if let Some(value) = Value {
                    self.resolve_expr(value)?;
                }
                for clause in WhenClauses {
                    self.resolve_expr(&clause.Expr)?;
                    self.resolve_expr(&clause.Result)?;
                }
                if let Some(value) = ElseClause {
                    self.resolve_expr(value)?;
                }
                Ok(())
            }
            ExprKind::WindowFunction { Args, Spec, .. } => {
                self.resolve_exprs(Args)?;
                self.resolve_window(Spec)
            }
            ExprKind::Subquery { Query, .. } => Query
                .with_node(|node| self.resolve_node(node))
                .unwrap_or(Ok(())),
            ExprKind::InSubquery { Expr, Sel, .. } => {
                self.resolve_expr(Expr)?;
                self.resolve_expr(Sel)
            }
            // 字面量、默认值、参数标记等不含列引用，直接通过。
            ExprKind::Value(_)
            | ExprKind::IntroducedValue { .. }
            | ExprKind::NamedDefault(_)
            | ExprKind::MaxValue
            | ExprKind::TimeUnit(_)
            | ExprKind::GetFormatSelector(_)
            | ExprKind::TrimDirection(_)
            | ExprKind::TableName(_)
            | ExprKind::ParamMarker { .. }
            | ExprKind::DefaultValue => Ok(()),
        }
    }
}

/// 将解析错误包装为带 TiDB 手册提示前缀的语法错误。
fn syntax_error(error: errors::Error) -> errors::Error {
    // Go util.SyntaxError preserves parser terror errors and otherwise converts
    // the diagnostic to parser.ErrParse (MySQL 1064).
    if errors::HasStack(&error)
        && errors::Cause(Some(&error))
            .is_some_and(|cause| cause.downcast_ref::<parser::TerrorError>().is_some())
    {
        return error;
    }
    parser::ErrParse.GenWithStackByArgs(&[
        SYNTAX_ERROR_PREFIX.to_owned().into(),
        error.to_string().into(),
    ])
}

/// Parses an expression through the TiDB parser, matching the Go select-wrapper path.
/// 将表达式包进 `select <expr>`，用默认字符集/排序规则解析，再取出唯一投影字段。
pub fn ParseExpression(expression: &str) -> Result<ast::ExprNode, errors::Error> {
    parse_expression(expression, None)
}

/// Parses metadata expressions under the SQL mode used when they were defined.
pub fn ParseExpressionWithSQLMode(
    expression: &str,
    sql_mode: mysql::SQLMode,
) -> Result<ast::ExprNode, errors::Error> {
    parse_expression(expression, Some(sql_mode))
}

fn parse_expression(
    expression: &str,
    sql_mode: Option<mysql::SQLMode>,
) -> Result<ast::ExprNode, errors::Error> {
    let sql = format!("select {expression}");
    let (charset, collation) = charset::GetDefaultCharsetAndCollate();
    let charset = parser::CharsetConnection(charset);
    let collation = parser::CollationConnection(collation);
    let mut parser = parserutil::GetParser();
    if let Some(mode) = sql_mode {
        parser.SetSQLMode(mode);
    }
    let parsed = parser.ParseSQL(&sql, &[&charset, &collation]);
    parserutil::DestroyParser(parser);

    let (mut statements, _) = parsed.map_err(syntax_error)?;
    let statement = statements
        .drain(..)
        .next()
        .expect("successful SELECT parse must return one statement");
    let select = statement
        .into_any()
        .downcast::<ast::SelectStmt>()
        .expect("expression wrapper must parse as SELECT");
    Ok(select
        .Fields
        .Fields
        .into_iter()
        .next()
        .and_then(|field| field.Expr)
        .expect("expression SELECT must contain one expression field"))
}

/// Resolves every column-name expression against the supplied table metadata.
/// 对表达式树做列名解析；成功则原样返回节点（不改写 AST）。
pub fn SimpleResolveName(
    node: ast::ExprNode,
    table_info: &model::TableInfo,
) -> Result<ast::ExprNode, errors::Error> {
    NameResolver { table_info }.resolve_expr(&node)?;
    Ok(node)
}
