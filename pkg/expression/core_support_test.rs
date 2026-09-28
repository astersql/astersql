// Copyright 2026 AsterSQL.

use crate::*;

#[test]
fn logical_operator_set_matches_go_util_table() {
    for name in [
        ast::LT,
        ast::GE,
        ast::GT,
        ast::LE,
        ast::EQ,
        ast::NE,
        ast::UnaryNot,
        ast::Like,
        ast::LogicAnd,
        ast::LogicOr,
        ast::LogicXor,
        ast::In,
        ast::IsNull,
        ast::IsFalsity,
        ast::IsTruthWithoutNull,
        ast::IsTruthWithNull,
        ast::NullEQ,
        ast::Regexp,
    ] {
        assert!(
            logicalOps.contains_key(name),
            "missing Go logical operator {name}"
        );
    }
    assert_eq!(logicalOps.len(), 18);
}

#[test]
fn function_trait_sets_match_go_exactly() {
    assert_eq!(unFoldableFunctions.len(), 18);
    assert_eq!(
        DisableFoldFunctions.keys().copied().collect::<Vec<_>>(),
        [ast::Benchmark]
    );
    for name in [
        ast::If,
        ast::Ifnull,
        ast::Case,
        ast::LogicAnd,
        ast::LogicOr,
        ast::Coalesce,
        ast::Interval,
    ] {
        assert!(
            TryFoldFunctions.contains_key(name),
            "missing Go try-fold function {name}"
        );
    }
    assert_eq!(TryFoldFunctions.len(), 7);
    assert!(noopFuncs.is_empty());
}
