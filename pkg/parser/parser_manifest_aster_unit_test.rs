// Copyright 2026 AsterSQL.

// 解析器文法清单与 Rust 生成表之间的契约测试。
//
// 本文件分别加载主解析器和 Hint 解析器的 `.astergram` 清单，逐条核对生成归约的
// 左部符号、右部长度及语义动作标记，并检查规则签名与稳定 ID 不会发生碰撞。

use crate::parsergen_grammar::{Grammar, ProductionItem};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// 从当前 crate 的 `grammar` 目录加载并解析指定文法清单。
///
/// 失败信息保留完整路径，便于区分清单缺失与清单语法错误。
fn load_manifest(name: &str) -> Grammar {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("grammar")
        .join(name);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read Rust grammar manifest {}: {error}", path.display()));
    Grammar::parse(&source)
        .unwrap_or_else(|error| panic!("parse Rust grammar manifest {}: {error}", path.display()))
}

/// 统计归约时实际从栈中弹出的文法符号数。
///
/// 中间语义动作只描述执行位置，不占用右部符号槽，因此不能计入归约长度。
fn rhs_symbol_count(production: &crate::parsergen_grammar::Production) -> usize {
    production
        .rhs
        .iter()
        .filter(|item| !matches!(item, ProductionItem::Action { .. }))
        .count()
}

/// 主文法清单必须与生成解析器的归约元数据逐条一致。
#[test]
fn parser_manifest_main_matches_rust_reductions() {
    let grammar = load_manifest("main.astergram");
    assert_eq!(
        grammar.productions.len(),
        super::super::GENERATED_MAIN_REDUCTIONS.len()
    );
    for (generated_rule, ((reduction, legacy_rule), action_required)) in
        super::super::GENERATED_MAIN_REDUCTIONS
            .iter()
            .zip(super::super::GENERATED_MAIN_LEGACY_RULES)
            .zip(super::super::GENERATED_MAIN_ACTION_REQUIRED)
            .enumerate()
    {
        let production = &grammar.productions[*legacy_rule - 1];
        assert_eq!(
            production.lhs,
            super::super::GENERATED_MAIN_SYMBOL_NAMES[reduction.0],
            "main generated rule {generated_rule} LHS"
        );
        assert_eq!(
            rhs_symbol_count(production),
            reduction.1,
            "main generated rule {generated_rule} RHS length"
        );
        assert_eq!(
            production.requires_action, *action_required,
            "main generated rule {generated_rule} action marker"
        );
    }
}

/// Hint 文法清单必须与 Hint 解析器的生成符号表和归约表逐条一致。
#[test]
fn parser_manifest_hint_matches_rust_reductions() {
    let grammar = load_manifest("hint.astergram");
    assert_eq!(
        grammar.productions.len(),
        super::GENERATED_HINT_REDUCTIONS.len()
    );
    for (generated_rule, ((reduction, legacy_rule), action_required)) in
        super::GENERATED_HINT_REDUCTIONS
            .iter()
            .zip(super::GENERATED_HINT_LEGACY_RULES)
            .zip(super::GENERATED_HINT_ACTION_REQUIRED)
            .enumerate()
    {
        let production = &grammar.productions[*legacy_rule - 1];
        assert_eq!(
            production.lhs,
            super::GENERATED_HINT_SYMBOL_NAMES[reduction.0],
            "hint generated rule {generated_rule} LHS"
        );
        assert_eq!(
            rhs_symbol_count(production),
            reduction.1,
            "hint generated rule {generated_rule} RHS length"
        );
        assert_eq!(
            production.requires_action, *action_required,
            "hint generated rule {generated_rule} action marker"
        );
    }
}

/// 两份清单都必须保留唯一、可稳定寻址的规则身份。
///
/// 同时固定主文法确实声明了优先级及产生式级覆盖，避免清单迁移时静默丢失冲突消解信息。
#[test]
fn parser_manifest_has_unique_rule_ids() {
    let main = load_manifest("main.astergram");
    let hint = load_manifest("hint.astergram");

    assert!(!main.precedence.is_empty(), "main precedence declarations");
    assert!(
        main.productions
            .iter()
            .any(|production| production.precedence.is_some()),
        "main production precedence overrides"
    );

    for (name, grammar) in [("main", main), ("hint", hint)] {
        let mut signatures = HashSet::new();
        let mut rule_ids = HashSet::new();
        for production in &grammar.productions {
            assert!(
                signatures.insert(production.signature.clone()),
                "duplicate {name} signature {}",
                production.signature
            );
            assert!(
                rule_ids.insert(production.rule_id.clone()),
                "duplicate {name} RuleId {}",
                production.rule_id
            );
        }
    }
}
