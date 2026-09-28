// Copyright 2026 AsterSQL.

// parsergen 文法描述解析器的单元测试。
//
// 这里集中固定声明、产生式和内嵌动作的解析契约，并检查稳定规则标识及带源码位置的
// 错误诊断，避免后续生成器改动悄然改变文法输入格式或诊断语义。

use crate::{Associativity, Grammar, GrammarErrorKind, ProductionItem};

/// 验证最小完整文法可同时解析起始符、token、优先级、空产生式与内嵌动作位置。
#[test]
fn grammar_parses_complete_minimal_spec() {
    let source = r#"
%start statement;
%token IDENT 256;
%token PLUS 43 "+";
%left PLUS;
%%
statement : expression @action;
expression : ε
           | expression PLUS @action IDENT @action;
"#;

    let grammar = Grammar::parse(source).expect("minimal grammar should parse");

    assert_eq!(grammar.start, "statement");
    assert_eq!(grammar.tokens.len(), 2);
    assert_eq!(grammar.tokens[0].name, "IDENT");
    assert_eq!(grammar.tokens[0].number, 256);
    assert_eq!(grammar.tokens[1].literal.as_deref(), Some("+"));
    assert_eq!(grammar.precedence.len(), 1);
    assert_eq!(grammar.precedence[0].associativity, Associativity::Left);
    assert_eq!(grammar.productions.len(), 3);

    let empty = &grammar.productions[1];
    assert!(empty.rhs.is_empty());
    assert!(empty.signature.contains('ε'));

    let with_actions = &grammar.productions[2];
    assert_eq!(
        with_actions.rhs,
        vec![
            ProductionItem::Symbol("expression".into()),
            ProductionItem::Symbol("PLUS".into()),
            ProductionItem::Action { position: 2 },
            ProductionItem::Symbol("IDENT".into()),
        ]
    );
    assert!(with_actions.requires_action);
    assert!(with_actions.signature.contains("@2"));
    assert!(!with_actions.rule_id.as_str().is_empty());
    assert!(with_actions.span.line > 0);
}

/// 重复 token 名称应在重复声明处报错，并在消息中保留首次声明的位置。
#[test]
fn grammar_rejects_duplicate_tokens() {
    let source = r#"
%start input;
%token IDENT 256;
%token IDENT 257;
%%
input : IDENT;
"#;

    let error = Grammar::parse(source).expect_err("duplicate token must fail");
    assert_eq!(error.kind, GrammarErrorKind::DuplicateToken);
    assert_eq!(error.span.line, 4);
    assert!(error.message.contains("IDENT"));
    assert!(error.message.contains("line 3"));
}

/// 规则标识只由规范化产生式签名决定，不应受声明或产生式排列顺序影响。
#[test]
fn grammar_rule_ids_are_reorder_stable() {
    let first = r#"
%start expression;
%token IDENT 256;
%token PLUS 43 "+";
%%
expression : IDENT @action
           | expression PLUS IDENT @action;
"#;
    let reordered = r#"
%token PLUS 43 "+";
%token IDENT 256;
%start expression;
%%
expression : expression PLUS IDENT @action
           | IDENT @action;
"#;

    let first = Grammar::parse(first).unwrap();
    let reordered = Grammar::parse(reordered).unwrap();
    let mut first_rules: Vec<_> = first
        .productions
        .iter()
        .map(|rule| (rule.signature.clone(), rule.rule_id.clone()))
        .collect();
    let mut reordered_rules: Vec<_> = reordered
        .productions
        .iter()
        .map(|rule| (rule.signature.clone(), rule.rule_id.clone()))
        .collect();
    first_rules.sort();
    reordered_rules.sort();

    assert_eq!(first_rules, reordered_rules);
}

/// 空分支与转义字面量都应还原为生成器消费的结构化产生式项。
#[test]
fn grammar_supports_empty_alternatives_and_escaped_literals() {
    let source = r#"
%start input;
%token QUOTE 34 "\"";
%token NEWLINE 10 "\n";
%right QUOTE NEWLINE;
%%
input :
      | "\"" @action
      | "\n";
"#;

    let grammar = Grammar::parse(source).unwrap();
    assert_eq!(grammar.tokens[0].literal.as_deref(), Some("\""));
    assert_eq!(grammar.tokens[1].literal.as_deref(), Some("\n"));
    assert!(grammar.productions[0].rhs.is_empty());
    assert_eq!(
        grammar.productions[1].rhs,
        vec![ProductionItem::Literal("\"".into())]
    );
    assert!(grammar.productions[1].requires_action);
}

/// token 编号必须唯一；仅内嵌动作不同的相同产生式也必须判为重复。
#[test]
fn grammar_rejects_duplicate_numbers_and_productions() {
    let duplicate_number = r#"
%start input;
%token FIRST 256;
%token SECOND 256;
%%
input : FIRST;
"#;
    let error = Grammar::parse(duplicate_number).unwrap_err();
    assert_eq!(error.kind, GrammarErrorKind::DuplicateTokenNumber);
    assert!(error.message.contains("FIRST"));

    let duplicate_production = r#"
%start input;
%token IDENT 256;
%%
input : IDENT | IDENT @action;
"#;
    let error = Grammar::parse(duplicate_production).unwrap_err();
    assert_eq!(error.kind, GrammarErrorKind::DuplicateProduction);
    assert!(error.message.contains("input -> IDENT"));
}

/// 未声明符号的诊断必须指向包含该引用的产生式源码位置。
#[test]
fn grammar_rejects_unknown_symbols_with_a_source_location() {
    let source = r#"
%start input;
%token IDENT 256;
%%
input : MISSING;
"#;

    let error = Grammar::parse(source).unwrap_err();
    assert_eq!(error.kind, GrammarErrorKind::UnknownSymbol);
    assert_eq!(error.span.line, 5);
    assert!(error.message.contains("MISSING"));
}
