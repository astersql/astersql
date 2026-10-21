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

//! SQL allowlist used by the user-facing diagnostic endpoint.

use std::any::Any;
use std::collections::HashSet;

use astersql_parser_ast as ast;

/// Returns whether a statement is safe to execute for an ordinary client while
/// diagnostic mode is enabled. Internal restricted SQL bypasses this predicate
/// at the session boundary.
pub(crate) fn is_diagnostic_sql_allowed(
    statement: &dyn ast::Node,
    current_database: &str,
    is_view: impl Fn(&str, &str) -> bool,
) -> bool {
    if statement.as_any().is::<ast::SelectStmt>() || statement.as_any().is::<ast::SetOprStmt>() {
        let mut checker = DiagnosticSqlChecker::new(true, current_database, is_view);
        statement.accept(&mut checker);
        return !checker.denied && checker.has_table;
    }
    if statement.as_any().is::<ast::UseStmt>() {
        return true;
    }
    if let Some(show) = statement.as_any().downcast_ref::<ast::ShowStmt>() {
        if !diagnostic_read_only_show_type(show.Tp) {
            return false;
        }
        let mut checker = DiagnosticSqlChecker::new(false, current_database, is_view);
        statement.accept(&mut checker);
        return !checker.denied;
    }
    if let Some(explain) = statement.as_any().downcast_ref::<ast::ExplainStmt>() {
        if explain.analyze {
            return false;
        }
        let Some(child) = explain.stmt.as_deref() else {
            return false;
        };
        if let Some(select) = child.as_any().downcast_ref::<ast::SelectStmt>() {
            if select.Kind != ast::SelectStmtKind::Select {
                return false;
            }
        } else if !child.as_any().is::<ast::SetOprStmt>() {
            return false;
        }
        let mut checker = DiagnosticSqlChecker::new(false, current_database, is_view);
        statement.accept(&mut checker);
        return !checker.denied;
    }
    false
}

struct DiagnosticSqlChecker<F> {
    denied: bool,
    only_allow_system_tables: bool,
    has_table: bool,
    current_database: String,
    cte_scopes: Vec<HashSet<String>>,
    is_view: F,
}

impl<F> DiagnosticSqlChecker<F>
where
    F: Fn(&str, &str) -> bool,
{
    fn new(only_allow_system_tables: bool, current_database: &str, is_view: F) -> Self {
        Self {
            denied: false,
            only_allow_system_tables,
            has_table: false,
            current_database: current_database.to_ascii_lowercase(),
            cte_scopes: Vec::new(),
            is_view,
        }
    }

    fn enter_cte(&mut self, cte: &ast::CommonTableExpression) {
        if self.only_allow_system_tables && cte.IsRecursive {
            if let Some(scope) = self.cte_scopes.last_mut() {
                scope.insert(cte.Name.L.clone());
            }
        }
    }

    fn leave_cte(&mut self, cte: &ast::CommonTableExpression) {
        if self.only_allow_system_tables {
            if let Some(scope) = self.cte_scopes.last_mut() {
                scope.insert(cte.Name.L.clone());
            }
        }
    }
}

impl<F> ast::Visitor for DiagnosticSqlChecker<F>
where
    F: Fn(&str, &str) -> bool,
{
    fn enter(&mut self, node: &dyn ast::Node) -> bool {
        if self.only_allow_system_tables
            && (node.as_any().is::<ast::SelectStmt>()
                || node.as_any().is::<ast::SetOprStmt>()
                || node.as_any().is::<ast::SetOprSelectList>())
        {
            self.cte_scopes.push(HashSet::new());
        }

        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            if select.Kind != ast::SelectStmtKind::Select
                || select.lock_info.is_some()
                || select.SelectIntoOpt.is_some()
                || select
                    .TableHints
                    .iter()
                    .chain(select.SelectStmtOpts.TableHints.iter())
                    .any(|hint| hint.HintName.L == "set_var")
            {
                self.denied = true;
            }
        }
        self.denied
    }

    fn leave(&mut self, node: &dyn ast::Node) -> bool {
        if self.only_allow_system_tables
            && (node.as_any().is::<ast::SelectStmt>()
                || node.as_any().is::<ast::SetOprStmt>()
                || node.as_any().is::<ast::SetOprSelectList>())
        {
            self.cte_scopes.pop();
        }
        !self.denied
    }

    fn enter_embedded(&mut self, input: &dyn Any) -> bool {
        if let Some(cte) = input.downcast_ref::<ast::CommonTableExpression>() {
            self.enter_cte(cte);
        } else if let Some(hint) = input.downcast_ref::<ast::TableOptimizerHint>() {
            if hint.HintName.L == "set_var" {
                self.denied = true;
            }
        } else if let Some(expression) = input.downcast_ref::<ast::ExprKind>() {
            match expression {
                ast::ExprKind::Variable { Value: Some(_), .. } => self.denied = true,
                ast::ExprKind::Function { FnName, .. }
                    if diagnostic_side_effect_function(&FnName.L) =>
                {
                    self.denied = true;
                }
                ast::ExprKind::Subquery { .. } if !self.only_allow_system_tables => {
                    self.denied = true;
                }
                _ => {}
            }
        }
        self.denied
    }

    fn leave_embedded(&mut self, input: &dyn Any) -> bool {
        if let Some(cte) = input.downcast_ref::<ast::CommonTableExpression>() {
            self.leave_cte(cte);
        }
        !self.denied
    }

    fn enter_table_name(&mut self, table: &ast::TableName) -> bool {
        if !self.only_allow_system_tables {
            return false;
        }
        if table.Schema.L.is_empty()
            && self
                .cte_scopes
                .iter()
                .rev()
                .any(|scope| scope.contains(&table.Name.L))
        {
            return false;
        }

        let database = if table.Schema.L.is_empty() {
            self.current_database.as_str()
        } else {
            table.Schema.L.as_str()
        };
        if !astersql_meta_metadef::IsMemDB(database) && !astersql_meta_metadef::IsSystemDB(database)
        {
            self.denied = true;
            return true;
        }

        self.has_table = true;
        if (self.is_view)(database, &table.Name.L) {
            self.denied = true;
        }
        self.denied
    }
}

fn diagnostic_side_effect_function(name: &str) -> bool {
    matches!(
        name,
        "get_lock"
            | "release_lock"
            | "release_all_locks"
            | "last_insert_id"
            | "nextval"
            | "lastval"
            | "setval"
            | "sleep"
            | "setvar"
    )
}

fn diagnostic_read_only_show_type(statement_type: ast::ShowStmtType) -> bool {
    matches!(
        statement_type,
        ast::ShowStmtType::Engines
            | ast::ShowStmtType::Databases
            | ast::ShowStmtType::Tables
            | ast::ShowStmtType::TableStatus
            | ast::ShowStmtType::Columns
            | ast::ShowStmtType::Warnings
            | ast::ShowStmtType::Charset
            | ast::ShowStmtType::Variables
            | ast::ShowStmtType::Status
            | ast::ShowStmtType::Collation
            | ast::ShowStmtType::CreateTable
            | ast::ShowStmtType::CreateView
            | ast::ShowStmtType::CreateUser
            | ast::ShowStmtType::CreateSequence
            | ast::ShowStmtType::CreatePlacementPolicy
            | ast::ShowStmtType::CreateDatabase
            | ast::ShowStmtType::CreateResourceGroup
            | ast::ShowStmtType::CreateProcedure
            | ast::ShowStmtType::Grants
            | ast::ShowStmtType::MaskingPolicies
            | ast::ShowStmtType::Triggers
            | ast::ShowStmtType::ProcedureStatus
            | ast::ShowStmtType::FunctionStatus
            | ast::ShowStmtType::Index
            | ast::ShowStmtType::ProcessList
            | ast::ShowStmtType::OpenTables
            | ast::ShowStmtType::Events
            | ast::ShowStmtType::Plugins
            | ast::ShowStmtType::Privileges
            | ast::ShowStmtType::Errors
            | ast::ShowStmtType::Builtins
            | ast::ShowStmtType::MasterStatus
            | ast::ShowStmtType::BinlogStatus
            | ast::ShowStmtType::ReplicaStatus
    )
}
