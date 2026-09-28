// Copyright 2026 AsterSQL.

use super::{Parser, Rhs, RuleId, yyLexer, yySymType};
use std::ops::{Index, IndexMut};

mod admin;
mod ddl;
mod dml;
mod expression;
mod misc;
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

/// Dispatch a stable generated rule identifier without exposing numeric rules.
pub(crate) fn apply(rule_id: RuleId, rhs: Rhs<'_>, context: Context<'_>) -> Result<bool, isize> {
    if ddl::owns(rule_id) {
        ddl::apply(rule_id, rhs, context).expect("owned DDL rule has an action")
    } else if dml::owns(rule_id) {
        dml::apply(rule_id, rhs, context).expect("owned DML rule has an action")
    } else if expression::owns(rule_id) {
        expression::apply(rule_id, rhs, context).expect("owned expression rule has an action")
    } else if query::owns(rule_id) {
        query::apply(rule_id, rhs, context).expect("owned query rule has an action")
    } else if security::owns(rule_id) {
        security::apply(rule_id, rhs, context).expect("owned security rule has an action")
    } else if admin::owns(rule_id) {
        admin::apply(rule_id, rhs, context).expect("owned admin rule has an action")
    } else if misc::owns(rule_id) {
        misc::apply(rule_id, rhs, context).expect("owned misc rule has an action")
    } else {
        Ok(false)
    }
}

pub(crate) fn has_semantic_action(rule_id: RuleId) -> bool {
    ddl::owns(rule_id)
        || dml::owns(rule_id)
        || expression::owns(rule_id)
        || query::owns(rule_id)
        || security::owns(rule_id)
        || admin::owns(rule_id)
        || misc::owns(rule_id)
}
