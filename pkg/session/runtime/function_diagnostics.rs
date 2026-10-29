// Copyright 2026 AsterSQL.
//! Function-resolution diagnostics retain the parser's original byte span.
use super::*;
use astersql_parser_ast::Node;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgFunctionError {
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
    let ast::ExprKind::Function { FnName, Schema, .. } = &expression.Kind else {
        unreachable!()
    };
    let name = if Schema.L.is_empty() {
        FnName.L.clone()
    } else {
        format!("{}.{}", Schema.L, FnName.L)
    };
    let arguments = types
        .iter()
        .map(|tp| match *tp {
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
