// Copyright 2026 AsterSQL.

use super::{Parser, Rhs, RuleId, yyLexer, yySymType};
use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::sync::OnceLock;

// Read the consumed token's original span, not reconstructed SQL or CIStr.L.
// This local constructor leaves NewCIStr and all MySQL name semantics intact.
fn identifier(
    parser: &Parser,
    symbol: &yySymType,
) -> (parser_ast::CIStr, Option<parser_ast::PostgreSQLIdentifier>) {
    if parser.sql_dialect() != parser_ast::SqlDialect::PostgreSQL {
        return (parser_ast::NewCIStr(&symbol.ident), None);
    }
    let start = symbol.offset as usize;
    let end = symbol.end_offset;
    let raw = &parser.src[start..end];
    let quoted = raw.starts_with('"') || raw.starts_with('`');
    let decoded = symbol.ident.clone();
    let lookup = if quoted {
        decoded.clone()
    } else {
        decoded.to_ascii_lowercase()
    };
    // CIStr.L remains the native lowercase compatibility key. PostgreSQL's
    // distinct lookup identity lives in the typed segment consumed at its
    // name boundary, not in the shared catalog's lowercase field.
    let name = parser_ast::NewCIStr(&decoded);
    (
        name,
        Some(parser_ast::PostgreSQLIdentifier {
            Original: raw.to_owned(),
            Decoded: decoded,
            Quoted: quoted,
            Lookup: lookup,
            Span: (start, end),
        }),
    )
}

fn column_name(parser: &Parser, symbols: &[&yySymType]) -> parser_ast::ColumnName {
    let mut names = Vec::new();
    let mut parts = Vec::new();
    for symbol in symbols {
        let (name, part) = identifier(parser, symbol);
        names.push(name);
        parts.extend(part);
    }
    let name = names.pop().unwrap_or_default();
    let table = names.pop().unwrap_or_default();
    let schema = names.pop().unwrap_or_default();
    parser_ast::ColumnName {
        Schema: schema,
        Table: table,
        Name: name,
        PostgreSQLParts: parts,
    }
}

mod admin;
mod ddl;
mod dml;
mod expression;
mod misc;
mod mview;
mod query;
mod security;

#[cfg(test)]
mod ddl_aster_unit_test;
#[cfg(test)]
mod dml_aster_unit_test;
#[cfg(test)]
mod query_expression_aster_unit_test;
#[cfg(test)]
mod remaining_aster_unit_test;

/// Mutable state available while applying one semantic action.
pub(crate) struct Context<'a> {
    pub(super) output: &'a mut yySymType,
    pub(super) parser_state: &'a mut Parser,
    pub(super) lexer: &'a mut dyn yyLexer,
}

impl<'a> Context<'a> {
    pub(crate) fn new(
        output: &'a mut yySymType,
        parser_state: &'a mut Parser,
        lexer: &'a mut dyn yyLexer,
    ) -> Self {
        Self {
            output,
            parser_state,
            lexer,
        }
    }
}

impl Index<usize> for Rhs<'_> {
    type Output = yySymType;

    fn index(&self, position: usize) -> &Self::Output {
        self.borrow(position).expect("semantic RHS position")
    }
}

impl IndexMut<usize> for Rhs<'_> {
    fn index_mut(&mut self, position: usize) -> &mut Self::Output {
        self.borrow_mut(position).expect("semantic RHS position")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Mview,
    Ddl,
    Dml,
    Expression,
    Query,
    Security,
    Admin,
    Misc,
}

fn identify_owner(rule_id: RuleId) -> Option<Owner> {
    if mview::owns(rule_id) {
        Some(Owner::Mview)
    } else if ddl::owns(rule_id) {
        Some(Owner::Ddl)
    } else if dml::owns(rule_id) {
        Some(Owner::Dml)
    } else if expression::owns(rule_id) {
        Some(Owner::Expression)
    } else if query::owns(rule_id) {
        Some(Owner::Query)
    } else if security::owns(rule_id) {
        Some(Owner::Security)
    } else if admin::owns(rule_id) {
        Some(Owner::Admin)
    } else if misc::owns(rule_id) {
        Some(Owner::Misc)
    } else {
        None
    }
}

fn owner(rule_id: RuleId) -> Option<Owner> {
    // Generated rules and their ownership are immutable. Resolve the module
    // once, rather than repeating all preceding modules' string matches for
    // every reduction (notably every expression in a large VALUES batch).
    static OWNERS: OnceLock<HashMap<&'static str, Owner>> = OnceLock::new();
    OWNERS
        .get_or_init(|| {
            super::RULE_IDS_BY_REDUCTION
                .iter()
                .filter_map(|&rule| identify_owner(rule).map(|owner| (rule.as_str(), owner)))
                .collect()
        })
        .get(rule_id.as_str())
        .copied()
}

/// Dispatch a stable generated rule identifier without exposing numeric rules.
pub(crate) fn apply(rule_id: RuleId, rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    match owner(rule_id) {
        Some(Owner::Mview) => mview::apply(rule_id, rhs, context),
        Some(Owner::Ddl) => {
            ddl::apply(rule_id, rhs, context).expect("owned DDL rule has an action")
        }
        Some(Owner::Dml) => {
            dml::apply(rule_id, rhs, context).expect("owned DML rule has an action")
        }
        Some(Owner::Expression) => {
            expression::apply(rule_id, rhs, context).expect("owned expression rule has an action")
        }
        Some(Owner::Query) => {
            query::apply(rule_id, rhs, context).expect("owned query rule has an action")
        }
        Some(Owner::Security) => {
            security::apply(rule_id, rhs, context).expect("owned security rule has an action")
        }
        Some(Owner::Admin) => {
            admin::apply(rule_id, rhs, context).expect("owned admin rule has an action")
        }
        Some(Owner::Misc) => {
            misc::apply(rule_id, rhs, context).expect("owned misc rule has an action")
        }
        None => Ok(false),
    }
}

pub(crate) fn has_semantic_action(rule_id: RuleId) -> bool {
    owner(rule_id).is_some()
}
