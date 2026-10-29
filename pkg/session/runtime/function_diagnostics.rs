// Copyright 2026 AsterSQL.
//! Function-resolution diagnostics retain the parser's original byte span.
use super::*;

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
