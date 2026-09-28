// Copyright 2026 AsterSQL.

//! LALR(1) 自动机构造的单元测试。
//!
//! 覆盖可空性与 FIRST 集计算、闭包前瞻符传播、相同 LR(0) 核心合并，
//! 以及状态编号和转移顺序的确定性，避免生成的解析表随构建次数发生漂移。

use std::collections::BTreeSet;

use crate::{Automaton, Grammar, ItemRule, Terminal};

/// 解析测试用的最小语法，并将语法错误直接归因于测试夹具。
fn grammar(source: &str) -> Grammar {
    Grammar::parse(source).expect("test grammar should parse")
}

/// 将终结符统一为稳定名称，便于按集合比较而不依赖内部表示。
fn names(values: impl IntoIterator<Item = Terminal>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|terminal| terminal.name().to_owned())
        .collect()
}

#[test]
/// 验证跨多层非终结符传播的可空性和 FIRST 集，并确认动作项不参与文法符号计算。
fn automaton_builds_nullable_first_sets() {
    let grammar = grammar(
        r#"
%start root;
%token ITEM 256 "item";
%token TAIL 257;
%%
root : chain @action tail;
chain : link;
link : ε | "item";
tail : ε | TAIL;
"#,
    );

    let automaton = Automaton::build(&grammar);

    for symbol in ["root", "chain", "link", "tail"] {
        assert!(automaton.first.is_nullable(symbol), "{symbol} is nullable");
    }
    assert_eq!(
        names(automaton.first.terminals("root").iter().cloned()),
        BTreeSet::from(["ITEM".to_owned(), "TAIL".to_owned()])
    );
    assert_eq!(
        names(automaton.first.terminals("chain").iter().cloned()),
        BTreeSet::from(["ITEM".to_owned()])
    );
}

#[test]
/// 验证闭包项会同时继承可空后缀的 FIRST 集与父项前瞻符。
fn automaton_closure_propagates_lookahead() {
    let grammar = grammar(
        r#"
%start root;
%token ITEM 256;
%token CLOSE 257;
%%
root : optional suffix;
optional : ε | ITEM;
suffix : ε | CLOSE;
"#,
    );
    let optional_rules: BTreeSet<_> = grammar
        .productions
        .iter()
        .filter(|rule| rule.lhs == "optional")
        .map(|rule| rule.rule_id.clone())
        .collect();

    let automaton = Automaton::build(&grammar);
    let initial = &automaton.states[0];
    let closure_items: Vec<_> = initial
        .items
        .iter()
        .filter(|item| {
            matches!(&item.rule, ItemRule::Production(rule) if optional_rules.contains(rule))
                && item.dot == 0
        })
        .collect();

    assert_eq!(closure_items.len(), 2);
    for item in closure_items {
        assert_eq!(
            names(item.lookaheads.iter().cloned()),
            BTreeSet::from(["$end".to_owned(), "CLOSE".to_owned()])
        );
    }
}

#[test]
/// 验证具有相同 LR(0) 核心的状态被合并，且各产生式保留前瞻符并集。
fn automaton_merges_lalr_cores() {
    let grammar = grammar(
        r#"
%start root;
%token A 256;
%token B 257;
%token C 258;
%token D 259;
%token E 260;
%%
root : A left D | B left E | A right E | B right D;
left : C;
right : C;
"#,
    );
    let left = grammar
        .productions
        .iter()
        .find(|rule| rule.lhs == "left")
        .unwrap();
    let right = grammar
        .productions
        .iter()
        .find(|rule| rule.lhs == "right")
        .unwrap();

    let automaton = Automaton::build(&grammar);
    let merged = automaton
        .states
        .iter()
        .find(|state| {
            state.items.iter().any(|item| {
                item.rule == ItemRule::Production(left.rule_id.clone())
                    && item.dot == left.rhs.len()
            }) && state.items.iter().any(|item| {
                item.rule == ItemRule::Production(right.rule_id.clone())
                    && item.dot == right.rhs.len()
            })
        })
        .expect("equal LR(0) cores should be merged");

    for rule in [&left.rule_id, &right.rule_id] {
        let item = merged
            .items
            .iter()
            .find(|item| item.rule == ItemRule::Production(rule.clone()) && item.dot == 1)
            .unwrap();
        assert_eq!(
            names(item.lookaheads.iter().cloned()),
            BTreeSet::from(["D".to_owned(), "E".to_owned()])
        );
    }
}

#[test]
/// 固定状态和转移快照，并通过重复构造验证编号、顺序及前瞻符传播均确定。
fn automaton_state_order_is_deterministic() {
    let grammar = grammar(
        r#"
%start expression;
%token IDENT 256;
%token PLUS 257;
%token STAR 258;
%token OPEN 259;
%token CLOSE 260;
%%
expression : expression PLUS term | term;
term : term STAR factor | factor;
factor : OPEN expression CLOSE | IDENT;
"#,
    );

    let first = Automaton::build(&grammar);
    let expected = format!("{first:#?}");
    let transition_snapshot: Vec<Vec<(String, usize)>> = first
        .states
        .iter()
        .map(|state| {
            state
                .transitions
                .iter()
                .map(|(symbol, target)| {
                    let name = match symbol {
                        crate::Symbol::Terminal(terminal) => terminal.name(),
                        crate::Symbol::Nonterminal(nonterminal) => nonterminal,
                    };
                    (name.to_owned(), *target)
                })
                .collect()
        })
        .collect();
    assert_eq!(
        transition_snapshot,
        vec![
            vec![
                ("IDENT".into(), 1),
                ("OPEN".into(), 2),
                ("expression".into(), 3),
                ("factor".into(), 4),
                ("term".into(), 5),
            ],
            vec![],
            vec![
                ("IDENT".into(), 1),
                ("OPEN".into(), 2),
                ("expression".into(), 6),
                ("factor".into(), 4),
                ("term".into(), 5),
            ],
            vec![("PLUS".into(), 7)],
            vec![],
            vec![("STAR".into(), 8)],
            vec![("CLOSE".into(), 9), ("PLUS".into(), 7)],
            vec![
                ("IDENT".into(), 1),
                ("OPEN".into(), 2),
                ("factor".into(), 4),
                ("term".into(), 10),
            ],
            vec![
                ("IDENT".into(), 1),
                ("OPEN".into(), 2),
                ("factor".into(), 11),
            ],
            vec![],
            vec![("STAR".into(), 8)],
            vec![],
        ]
    );
    for _ in 0..32 {
        let actual = Automaton::build(&grammar);
        assert_eq!(format!("{actual:#?}"), expected);
        assert!(
            actual
                .states
                .iter()
                .enumerate()
                .all(|(index, state)| state.id == index)
        );
        for state in &actual.states {
            assert!(state.items.iter().all(|item| !item.lookaheads.is_empty()));
            let symbols: Vec<_> = state.transitions.keys().collect();
            assert!(symbols.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}
