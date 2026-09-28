// Copyright 2026 AsterSQL.

use super::super::RULE_IDS_BY_REDUCTION;
use super::*;
use crate::parsergen_grammar::Grammar;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

const DML_NONTERMINALS: &[&str] = &[
    "Assignment",
    "AssignmentList",
    "CancelImportStmt",
    "CharsetOpt",
    "ColumnNameOrUserVarListOpt",
    "ColumnNameOrUserVarListOptWithBrackets",
    "ColumnNameOrUserVariable",
    "ColumnNameOrUserVariableList",
    "ColumnSetValueList",
    "DeleteFromStmt",
    "DeleteWithoutUsingStmt",
    "DeleteWithUsingStmt",
    "DryRunOptions",
    "FieldItem",
    "FieldItemList",
    "Fields",
    "FieldTerminator",
    "FormatOpt",
    "IgnoreLines",
    "ImportFromSelectStmt",
    "ImportIntoStmt",
    "InsertIntoStmt",
    "InsertRowAliasOpt",
    "InsertValues",
    "Lines",
    "LinesTerminated",
    "LoadDataOption",
    "LoadDataOptionList",
    "LoadDataOptionListOpt",
    "LoadDataSetItem",
    "LoadDataSetList",
    "LoadDataSetSpecOpt",
    "LoadDataStmt",
    "LocalOpt",
    "LowPriorityOpt",
    "NonTransactionalDMLStmt",
    "OnDuplicateKeyUpdate",
    "OptionalShardColumn",
    "QuickOptional",
    "ReplaceIntoStmt",
    "ReturningClause",
    "Starting",
    "UpdateStmt",
    "UpdateStmtNoWith",
];

fn main_grammar() -> Grammar {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("grammar")
        .join("main.astergram");
    let source = fs::read_to_string(path).expect("read main Rust grammar");
    Grammar::parse(&source).expect("parse main Rust grammar")
}

#[test]
fn dml_rule_coverage() {
    let grammar = main_grammar();
    let dml_lhs = DML_NONTERMINALS.iter().copied().collect::<HashSet<_>>();
    let expected = grammar
        .productions
        .iter()
        .filter(|production| {
            production.requires_action && dml_lhs.contains(production.lhs.as_str())
        })
        .map(|production| production.rule_id.as_str())
        .collect::<HashSet<_>>();

    assert_eq!(expected.len(), 95, "DML RuleId inventory changed");

    let mut actual = HashSet::new();
    for rule_id in RULE_IDS_BY_REDUCTION.iter().copied() {
        let ddl = ddl::owns(rule_id);
        let dml = dml::owns(rule_id);
        let expression = expression::owns(rule_id);
        let query = query::owns(rule_id);
        if dml {
            actual.insert(rule_id.as_str());
        }
        assert!(
            usize::from(ddl) + usize::from(dml) + usize::from(expression) + usize::from(query) <= 1,
            "rule has duplicate semantic owners: {}",
            rule_id.as_str()
        );
    }

    assert_eq!(actual, expected, "DML semantic action ownership changed");
}

#[test]
fn dml_module_has_no_numeric_fallback() {
    let source = include_str!("dml.rs");
    assert!(!source.contains("legacy_rule_number"));
    assert!(!source.contains("apply_numeric"));
    assert!(!source.contains("rhs_index"));
}
