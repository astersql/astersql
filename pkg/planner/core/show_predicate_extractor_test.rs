// Copyright 2026 AsterSQL.

use super::*;
use crate::ast::{ColumnName, ExprKind, ExprNode, ShowStmt, ShowStmtType};

fn like(pattern: ExprNode, escape: &str) -> ExprNode {
    ExprNode {
        node_text: Default::default(),
        Kind: ExprKind::Like {
            Expr: Box::new(ExprNode::Value(String::new())),
            Pattern: Box::new(pattern),
            Not: false,
            Escape: escape.to_owned(),
            Explicit: true,
            IsLike: true,
            Type: Default::default(),
        },
        OriginTextPosition: 0,
        Flag: Default::default(),
    }
}

fn show(kind: ShowStmtType, pattern: Option<ExprNode>) -> ShowStmt {
    ShowStmt {
        Tp: kind,
        Pattern: pattern,
        ..Default::default()
    }
}

#[test]
fn extracts_exact_and_wildcard_patterns_like_go() {
    let mut exact = newShowBaseExtractor(show(
        ShowStmtType::Columns,
        Some(like(ExprNode::Value("AbC".into()), "\\")),
    ));
    assert!(exact.Extract());
    assert_eq!(exact.Field(), "abc");
    assert_eq!(exact.ExplainInfo(), "field:[abc]");
    assert!(exact.FieldPatternLike().is_none());

    let mut wildcard = newShowBaseExtractor(show(
        ShowStmtType::Tables,
        Some(like(ExprNode::Value("Ab_%".into()), "\\")),
    ));
    assert!(wildcard.Extract());
    assert_eq!(wildcard.Field(), "");
    assert_eq!(wildcard.ExplainInfo(), "table_pattern:[ab_%]");
    let matcher = wildcard.FieldPatternLike().expect("compiled wildcard");
    assert!(matcher.DoMatch("ab_cd"));
    assert!(!matcher.DoMatch("AB_cd"));
    assert!(!matcher.DoMatch("AX"));
}

#[test]
fn rejects_column_pattern_and_extracts_describe_column() {
    let column_expr = ExprNode::Column(ColumnName {
        Name: crate::ast::NewCIStr("abc"),
        ..Default::default()
    });
    let mut invalid =
        newShowBaseExtractor(show(ShowStmtType::Columns, Some(like(column_expr, "\\"))));
    assert!(!invalid.Extract());

    let mut describe = newShowBaseExtractor(ShowStmt {
        Tp: ShowStmtType::Columns,
        Column: Some(ColumnName {
            Name: crate::ast::NewCIStr("MiXeD"),
            ..Default::default()
        }),
        ..Default::default()
    });
    assert!(describe.Extract());
    assert_eq!(describe.Field(), "mixed");
}

#[test]
fn explain_keys_and_empty_state_match_go() {
    for (kind, key) in [
        (ShowStmtType::Variables, "field"),
        (ShowStmtType::Columns, "field"),
        (ShowStmtType::Tables, "table"),
        (ShowStmtType::TableStatus, "table"),
        (ShowStmtType::Databases, "database"),
        (ShowStmtType::Collation, "collation"),
        (ShowStmtType::StatsHealthy, "db_name"),
    ] {
        let mut extractor = newShowBaseExtractor(show(
            kind,
            Some(like(ExprNode::Value("VALUE".into()), "\\")),
        ));
        assert!(extractor.Extract());
        assert_eq!(extractor.ExplainInfo(), format!("{key}:[value]"));
    }

    let mut empty = newShowBaseExtractor(show(ShowStmtType::Plugins, None));
    assert!(!empty.Extract());
    assert_eq!(empty.ExplainInfo(), "");
    assert_eq!(empty.Field(), "");
    assert!(empty.FieldPatternLike().is_none());
}
