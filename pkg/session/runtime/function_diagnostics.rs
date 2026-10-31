// Copyright 2026 AsterSQL.
//! Function-resolution diagnostics retain the parser's original byte span.
use super::*;
use astersql_parser_ast::Node;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgFunctionError {
    pub detail: Option<&'static str>,
    pub state: &'static str,
    pub message: String,
    pub hint: Option<&'static str>,
    pub position: Option<usize>,
}
impl std::fmt::Display for PgFunctionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PgFunctionError {}

pub(super) fn no_matching_function(expression: &ast::ExprNode, types: &[u8]) -> SessionError {
    let name = match &expression.Kind {
        ast::ExprKind::Function { FnName, Schema, .. } if !Schema.L.is_empty() => {
            format!("{}.{}", Schema.L, FnName.L)
        }
        ast::ExprKind::Function { FnName, .. } => FnName.L.clone(),
        ast::ExprKind::WindowFunction { Name, .. } => Name.to_ascii_lowercase(),
        _ => unreachable!(),
    };
    let arguments = types
        .iter()
        .map(|tp| match *tp {
            astersql_parser_mysql::r#type::TypeTiny => "boolean",
            astersql_parser_mysql::r#type::TypeShort => "smallint",
            astersql_parser_mysql::r#type::TypeNewDecimal => "numeric",
            astersql_parser_mysql::r#type::TypeDouble => "double precision",
            astersql_parser_mysql::r#type::TypeLong => "integer",
            astersql_parser_mysql::r#type::TypeLonglong => "bigint",
            astersql_parser_mysql::r#type::TypeVarString
            | astersql_parser_mysql::r#type::TypeVarchar
            | astersql_parser_mysql::r#type::TypeString => "text",
            astersql_parser_mysql::r#type::TypeDatetime => "timestamp without time zone",
            _ => "unknown",
        })
        .collect::<Vec<_>>()
        .join(", ");
    let error = PgFunctionError {
        detail: None,
        state: "42883",
        message: format!("function {name}({arguments}) does not exist"),
        hint: Some(
            "No function matches the given name and argument types. You might need to add explicit type casts.",
        ),
        position: (expression.OriginTextPosition > 0)
            .then_some(expression.OriginTextPosition as usize),
    };
    SessionError::with_source(error.to_string(), astersql_errors::SharedError::new(error))
}

#[derive(Clone, Debug)]
pub(crate) struct MysqlHiddenFunction(pub String);
impl std::fmt::Display for MysqlHiddenFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "function {} does not exist", self.0)
    }
}
impl std::error::Error for MysqlHiddenFunction {}

impl ConcreteSession {
    pub(super) fn normalize_mysql_function_parse_error(&self, error: SessionError) -> SessionError {
        let error = if self.sql_dialect() == ast::SqlDialect::MySQL {
            if let Some(name) = error.mysql_hidden_function() {
                let qualified = format!("{}.{}", self.current_database(), name);
                let error = astersql_util_dbterror::ClassExpression
                    .NewStd(astersql_errno::errcode::ErrSpDoesNotExist)
                    .GenWithStackByArgs(&["FUNCTION".into(), qualified.into()]);
                SessionError::with_source(error.to_string(), error)
            } else {
                error
            }
        } else {
            error
        };
        self.record_mysql_function_parse_error(&error);
        error
    }
}

/// Analyze explicit constant integer inputs while retaining their original spans.
pub(super) fn constant_integer_input_error(expression: &ast::ExprNode) -> Option<SessionError> {
    struct Inputs(Option<SessionError>);
    impl ast::Visitor for Inputs {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if self.0.is_some() {
                return true;
            }
            if let Some(ast::ExprNode {
                Kind: ast::ExprKind::Cast { Expr, Tp, .. },
                ..
            }) = node.as_any().downcast_ref::<ast::ExprNode>()
                && Tp.GetType() == astersql_parser_mysql::r#type::TypeLong
                && let ast::ExprKind::Value(value) = &Expr.Kind
                && let ast::ValueDatum::String(text) = &value.Datum
                && let Err(error) = astersql_expression::pg_cast_int4(Some(text))
            {
                let error = PgFunctionError {
                    detail: None,
                    state: error.sqlstate(),
                    message: error.to_string(),
                    hint: None,
                    position: (Expr.OriginTextPosition > 0)
                        .then_some(Expr.OriginTextPosition as usize),
                };
                self.0 = Some(SessionError::with_source(
                    error.to_string(),
                    astersql_errors::SharedError::new(error),
                ));
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let ast::ExprKind::Function { Args, .. } = &expression.Kind else {
        unreachable!()
    };
    let mut inputs = Inputs(None);
    for arg in Args {
        arg.accept(&mut inputs);
    }
    inputs.0
}

/// Analyze explicit constant input conversions before resolving a hidden name.
/// This never constructs or evaluates the MySQL-only function body.
pub(super) fn exclusive_function_error(expression: &ast::ExprNode) -> SessionError {
    if let Some(error) = constant_integer_input_error(expression) {
        return error;
    }
    let ast::ExprKind::Function { Args, .. } = &expression.Kind else {
        unreachable!()
    };
    let types = Args
        .iter()
        .map(|arg| match &arg.Kind {
            ast::ExprKind::Cast { Tp, .. } => Tp.GetType(),
            _ => astersql_parser_mysql::r#type::TypeNull,
        })
        .collect::<Vec<_>>();
    no_matching_function(expression, &types)
}

/// Validate only the four exclusive functions on original PG ASTs, before a
/// relation adapter restores SQL and would discard their original byte spans.
pub fn ValidatePostgreSQLExclusiveFunctions(
    statements: &[Box<dyn ast::Node>],
) -> SessionResult<()> {
    struct Exclusive(Option<SessionError>);
    impl ast::Visitor for Exclusive {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if self.0.is_some() {
                return true;
            }
            if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>()
                && let Some(error) = ordered_set_direct_error(select)
            {
                self.0 = Some(error);
                return true;
            }
            if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>()
                && matches!(&expr.Kind, ast::ExprKind::Function { FnName, .. } if matches!(FnName.L.as_str(), "percentile_disc" | "percentile_cont"))
            {
                let ast::ExprKind::Function { Args, .. } = &expr.Kind else {
                    unreachable!()
                };
                let types = Args
                    .iter()
                    .map(|arg| match &arg.Kind {
                        ast::ExprKind::Cast { Tp, .. } => Tp.GetType(),
                        ast::ExprKind::Value(value) => value.Type.GetType(),
                        _ => astersql_parser_mysql::r#type::TypeNull,
                    })
                    .collect::<Vec<_>>();
                self.0 = Some(no_matching_function(expr, &types));
                return true;
            }
            if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>()
                && let Err(error) = super::relational_value::pg_offset_window_signature(expr)
            {
                self.0 = Some(error);
                return true;
            }
            if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>()
                && matches!(&expr.Kind, ast::ExprKind::Function { FnName, .. } if matches!(FnName.L.as_str(), "ifnull" | "date_format" | "json_extract" | "tidb_version"))
            {
                self.0 = Some(exclusive_function_error(expr));
                return true;
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut visitor = Exclusive(None);
    for statement in statements {
        statement.accept(&mut visitor);
    }
    visitor.0.map_or(Ok(()), Err)
}

/// Validate direct arguments on the original AST, before relation restoration
/// changes aliases or byte positions. The execution adapter retains its own
/// resolved group-key check for callers that bypass the protocol preflight.
fn ordered_set_direct_error(select: &ast::SelectStmt) -> Option<SessionError> {
    struct Calls<'a> {
        select: &'a ast::SelectStmt,
        error: Option<SessionError>,
    }
    impl ast::ExprNodeVisitor for Calls<'_> {
        fn Enter(&mut self, expr: &ast::ExprNode) -> (ast::ExprNode, bool) {
            if self.error.is_some() {
                return (expr.clone(), true);
            }
            if let ast::ExprKind::AggregateFunction { Name, Args, .. } = &expr.Kind
                && matches!(Name.as_str(), "percentile_disc" | "percentile_cont")
                && Args.len() == 1
            {
                struct Direct<'a> {
                    groups: &'a [ast::ByItem],
                    invalid: Option<ast::ExprNode>,
                }
                impl ast::ExprNodeVisitor for Direct<'_> {
                    fn Enter(&mut self, expr: &ast::ExprNode) -> (ast::ExprNode, bool) {
                        if let ast::ExprKind::Column(column) = &expr.Kind {
                            let grouped = self.groups.iter().any(|item| matches!(&item.Expr.Kind,
                                ast::ExprKind::Column(key) if key.Name.L == column.Name.L
                                    && (key.Table.L.is_empty() || column.Table.L.is_empty() || key.Table.L == column.Table.L)));
                            if !grouped && self.invalid.is_none() {
                                self.invalid = Some(expr.clone());
                            }
                        }
                        (expr.clone(), false)
                    }
                    fn Leave(&mut self, expr: &ast::ExprNode) -> (ast::ExprNode, bool) {
                        (expr.clone(), true)
                    }
                }
                let mut direct = Direct {
                    groups: &self.select.GroupBy,
                    invalid: None,
                };
                Args[0].Accept(&mut direct);
                if let Some(column_expr) = direct.invalid {
                    let ast::ExprKind::Column(column) = &column_expr.Kind else {
                        unreachable!()
                    };
                    let qualifier = if !column.Table.L.is_empty() {
                        column.Table.L.as_str()
                    } else if let Some(from) = &self.select.From
                        && from.TableRefs.Right.is_none()
                        && let Some(ast::ResultSetNode::TableSource(source)) =
                            from.TableRefs.Left.as_deref()
                    {
                        if source.AsName.L.is_empty() {
                            source.Source.Name.L.as_str()
                        } else {
                            source.AsName.L.as_str()
                        }
                    } else {
                        ""
                    };
                    let name = if qualifier.is_empty() {
                        column.Name.L.clone()
                    } else {
                        format!("{qualifier}.{}", column.Name.L)
                    };
                    let diagnostic = PgFunctionError {
                        state: "42803",
                        message: format!(
                            "column \"{name}\" must appear in the GROUP BY clause or be used in an aggregate function"
                        ),
                        detail: Some(
                            "Direct arguments of an ordered-set aggregate must use only grouped columns.",
                        ),
                        hint: None,
                        position: Some(column_expr.OriginTextPosition.max(0) as usize),
                    };
                    self.error = Some(SessionError::with_source(
                        diagnostic.to_string(),
                        astersql_errors::SharedError::new(diagnostic),
                    ));
                }
            }
            (expr.clone(), false)
        }
        fn Leave(&mut self, expr: &ast::ExprNode) -> (ast::ExprNode, bool) {
            (expr.clone(), true)
        }
    }
    let mut calls = Calls {
        select,
        error: None,
    };
    for field in &select.Fields.Fields {
        if let Some(expr) = &field.Expr {
            expr.Accept(&mut calls);
        }
    }
    calls.error
}
