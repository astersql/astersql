// Copyright 2026 AsterSQL.

use super::super::RULE_IDS_BY_REDUCTION;
use super::*;
use crate::parsergen_grammar::Grammar;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

const EXPRESSION_NONTERMINALS: &[&str] = &[
    "AnyOrAll",
    "ArrayKwdOpt",
    "BetweenOrNotOp",
    "BitExpr",
    "BoolPri",
    "BuggyDefaultFalseDistinctOpt",
    "BuiltinFunction",
    "CastType",
    "CharsetName",
    "CollationName",
    "ColumnName",
    "ColumnNameList",
    "CompareOp",
    "DefaultFalseDistinctOpt",
    "DefaultOrExpression",
    "DefaultOrExpressionList",
    "DefaultTrueDistinctOpt",
    "DistinctOpt",
    "ElseOpt",
    "Expression",
    "ExpressionList",
    "ExpressionListOpt",
    "ExpressionOpt",
    "FulltextSearchModifierOpt",
    "FuncDatetimePrec",
    "FuncDatetimePrecList",
    "FuncDatetimePrecListOpt",
    "FunctionCallGeneric",
    "FunctionCallKeyword",
    "FunctionCallNonKeyword",
    "FunctionNameSequence",
    "GetFormatSelector",
    "IlikeOrNotOp",
    "InOrNotOp",
    "Int64Num",
    "IsOrNotOp",
    "LengthNum",
    "LikeOrIlikeEscapeOpt",
    "LikeOrNotOp",
    "Literal",
    "MaxValueOrExpression",
    "MaxValueOrExpressionList",
    "NextValueForSequence",
    "NextValueForSequenceParentheses",
    "NowSymOptionFraction",
    "NowSymOptionFractionParentheses",
    "OptGConcatSeparator",
    "PgArrayElements",
    "PgArrayList",
    "PgArrayRow",
    "PgArrayRows",
    "PredicateExpr",
    "RegexpOrNotOp",
    "SignedLiteral",
    "SignedNum",
    "SimpleExpr",
    "SimpleIdent",
    "StringLiteral",
    "SumExpr",
    "SystemVariable",
    "TimeUnit",
    "TimestampUnit",
    "TrimDirection",
    "UserVariable",
    "WhenClause",
    "WhenClauseList",
];

const QUERY_NONTERMINALS: &[&str] = &[
    "AsOfClause",
    "AsOfClauseOpt",
    "ByItem",
    "ByList",
    "CommonTableExpr",
    "EscapedTableRef",
    "ExprOrDefault",
    "FetchFirstOpt",
    "Field",
    "FieldAsName",
    "FieldAsNameOpt",
    "FieldList",
    "GroupByClause",
    "HavingClause",
    "IndexHint",
    "IndexHintList",
    "IndexHintListOpt",
    "IndexHintScope",
    "IndexHintType",
    "IndexNameList",
    "JoinTable",
    "JoinType",
    "LimitClause",
    "LimitOption",
    "OfTablesOpt",
    "OptExistingWindowName",
    "OptFromFirstLast",
    "OptLeadLagInfo",
    "OptLLDefault",
    "OptNullTreatment",
    "OptOrder",
    "OptPartitionClause",
    "OptWindowFrameClause",
    "OptWindowingClause",
    "OptWindowOrderByClause",
    "Order",
    "OrderBy",
    "OrderByOptional",
    "PartitionNameListOpt",
    "Priority",
    "PriorityOpt",
    "RepeatableOpt",
    "RowStmt",
    "RowValue",
    "SelectLockOpt",
    "SelectStmt",
    "SelectStmtBasic",
    "SelectStmtFieldList",
    "SelectStmtFromDualTable",
    "SelectStmtFromTable",
    "SelectStmtGroup",
    "SelectStmtIntoOption",
    "SelectStmtLimit",
    "SelectStmtLimitOpt",
    "SelectStmtOpt",
    "SelectStmtOpts",
    "SelectStmtOptsList",
    "SelectStmtSQLCache",
    "SelectStmtWithClause",
    "SetOpr",
    "SetOprClause",
    "SetOprClauseList",
    "SetOprStmt",
    "SetOprStmtWithLimitOrderBy",
    "SetOprStmtWoutLimitOrderBy",
    "SubSelect",
    "TableAliasRefList",
    "TableAsName",
    "TableAsNameOpt",
    "TableAsNameOptDelete",
    "TableFactor",
    "TableName",
    "TableNameList",
    "TableNameOptWild",
    "TableOptimizerHints",
    "TableOptimizerHintsOpt",
    "TableRefs",
    "TableRefsClause",
    "TableSampleMethodOpt",
    "TableSampleOpt",
    "TableSampleUnitOpt",
    "Values",
    "ValuesOpt",
    "ValuesStmtList",
    "WhereClause",
    "WhereClauseOptional",
    "WindowClauseOptional",
    "WindowDefinition",
    "WindowDefinitionList",
    "WindowFrameBetween",
    "WindowFrameBound",
    "WindowFrameExtent",
    "WindowFrameStart",
    "WindowFrameUnits",
    "WindowFuncCall",
    "WindowingClause",
    "WindowName",
    "WindowNameOrSpec",
    "WindowSpec",
    "WindowSpecDetails",
    "WithClause",
    "WithList",
    "WithRollupClause",
];

fn main_grammar() -> Grammar {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("grammar")
        .join("main.astergram");
    let source = fs::read_to_string(path).expect("read main Rust grammar");
    Grammar::parse(&source).expect("parse main Rust grammar")
}

#[test]
fn query_expression_rule_coverage() {
    let grammar = main_grammar();
    let expression_lhs = EXPRESSION_NONTERMINALS
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let query_lhs = QUERY_NONTERMINALS.iter().copied().collect::<HashSet<_>>();
    assert!(expression_lhs.is_disjoint(&query_lhs));

    let expression_expected = grammar
        .productions
        .iter()
        .filter(|production| {
            production.requires_action && expression_lhs.contains(production.lhs.as_str())
        })
        .map(|production| production.rule_id.as_str())
        .collect::<HashSet<_>>();
    let query_expected = grammar
        .productions
        .iter()
        .filter(|production| {
            production.requires_action && query_lhs.contains(production.lhs.as_str())
        })
        .map(|production| production.rule_id.as_str())
        .collect::<HashSet<_>>();

    assert_eq!(
        expression_expected.len(),
        296,
        "expression RuleId inventory changed"
    );
    assert_eq!(query_expected.len(), 237, "query RuleId inventory changed");
    assert!(expression_expected.is_disjoint(&query_expected));

    for rule_id in RULE_IDS_BY_REDUCTION.iter().copied() {
        let ddl = ddl::owns(rule_id);
        let expression = expression::owns(rule_id);
        let query = query::owns(rule_id);
        if expression_expected.contains(rule_id.as_str()) {
            assert!(
                expression,
                "expression rule is not owned: {}",
                rule_id.as_str()
            );
        }
        if query_expected.contains(rule_id.as_str()) {
            assert!(query, "query rule is not owned: {}", rule_id.as_str());
        }
        assert!(
            usize::from(ddl) + usize::from(expression) + usize::from(query) <= 1,
            "rule has duplicate semantic owners: {}",
            rule_id.as_str()
        );
    }
}

#[test]
fn query_expression_modules_have_no_numeric_fallback() {
    for source in [include_str!("query.rs"), include_str!("expression.rs")] {
        assert!(!source.contains("legacy_rule_number"));
        assert!(!source.contains("apply_numeric"));
        assert!(!source.contains("rhs_index"));
    }
}

/// SignedLiteral must retain NumLiteral's datum and type, as Go's grammar
/// passes the original numeric value to ast.NewValueExpr. Stringifying it
/// makes DEFAULT -1 restore as -'1' and breaks mysql.analyze_options writes.
#[test]
fn signed_numeric_defaults_keep_numliteral_datum_and_type() {
    fn default_expression(sql: &str) -> parser_ast::ExprNode {
        let statement = crate::New().ParseOneStmt(sql, "", "").unwrap();
        let column = if let Some(create) = statement
            .as_any()
            .downcast_ref::<parser_ast::CreateTableStmt>()
        {
            &create.Cols[0]
        } else {
            &statement
                .as_any()
                .downcast_ref::<parser_ast::AlterTableStmt>()
                .unwrap()
                .Specs[0]
                .NewColumns[0]
        };
        column
            .Options
            .iter()
            .find(|option| option.Tp == parser_ast::ColumnOptionType::DefaultValue)
            .unwrap()
            .Expr
            .as_ref()
            .unwrap()
            .clone()
    }
    for number in ["1", "18446744073709551615", "1.25", "1e3"] {
        let plain = default_expression(&format!("create table t(a double default {number})"));
        let parser_ast::ExprKind::Value(expected) = plain.Kind else {
            panic!("expected numeric literal")
        };
        assert!(!matches!(expected.Datum, parser_ast::ValueDatum::String(_)));
        for sign in ["+", "-"] {
            for sql in [
                format!("create table t(a double default {sign}{number})"),
                format!("alter table t add column a double default {sign}{number}"),
            ] {
                let signed = default_expression(&sql);
                let parser_ast::ExprKind::Unary { Op, V } = signed.Kind else {
                    panic!("expected signed literal: {sql}")
                };
                assert_eq!(Op, sign);
                let parser_ast::ExprKind::Value(actual) = V.Kind else {
                    panic!("expected numeric operand: {sql}")
                };
                assert_eq!(actual.Datum, expected.Datum, "{sql}");
                assert_eq!(actual.Type, expected.Type, "{sql}");
            }
        }
    }
}
