// Copyright 2026 AsterSQL.

//! 解析表构建与稀疏编码的单元测试。
//!
//! 覆盖优先级和结合性驱动的移进/归约冲突消解、无法消解时的诊断信息，
//! 以及动作表和跳转表合并编码后的列稳定性与结果确定性。

use crate::{
    Automaton, ENCODED_ACCEPT, Grammar, ItemRule, ParseTable, TableCell, TableColumn, Terminal,
};

/// 解析测试用文法；失败表示测试夹具本身无效。
fn grammar(source: &str) -> Grammar {
    Grammar::parse(source).expect("test grammar should parse")
}

/// 从文法依次构建 LALR 自动机和无冲突解析表。
fn table(source: &str) -> (Grammar, ParseTable) {
    let grammar = grammar(source);
    let automaton = Automaton::build(&grammar);
    let table = ParseTable::build(&grammar, &automaton).expect("table should build");
    (grammar, table)
}

/// 查找指定产生式已完成归约的状态。
///
/// 语义动作不消耗输入符号，因此计算点位置时必须将其排除。
fn completed_state(grammar: &Grammar, table: &ParseTable, signature_fragment: &str) -> usize {
    let production = grammar
        .productions
        .iter()
        .find(|production| production.signature.contains(signature_fragment))
        .expect("production should exist");
    table
        .states()
        .iter()
        .find(|state| {
            state.items.iter().any(|item| {
                item.rule == crate::ItemRule::Production(production.rule_id.clone())
                    && item.dot
                        == production
                            .rhs
                            .iter()
                            .filter(|item| !matches!(item, crate::ProductionItem::Action { .. }))
                            .count()
            })
        })
        .expect("completed production state should exist")
        .id
}

#[test]
/// 验证声明的优先级、结合性和 `%prec` 覆盖能够消解移进/归约冲突。
fn table_resolves_declared_shift_reduce_conflicts() {
    let source = r#"
%start expression;
%token NUMBER 256;
%token PLUS 257;
%token POWER 258;
%token LT 259;
%token UMINUS 260;
%left PLUS;
%right POWER;
%nonassoc LT;
%right UMINUS;
%%
expression : NUMBER
           | expression PLUS expression
           | expression POWER expression
           | expression LT expression
           | PLUS expression %prec UMINUS;
"#;
    let (grammar, table) = table(source);

    // 同级左结合运算符在再次遇到自身时先归约。
    let plus_state = completed_state(&grammar, &table, "expression PLUS expression");
    assert!(matches!(
        table.action(plus_state, &Terminal::Token("PLUS".into())),
        TableCell::Reduce(_)
    ));

    // 同级右结合运算符在再次遇到自身时继续移进。
    let power_state = completed_state(&grammar, &table, "expression POWER expression");
    assert!(matches!(
        table.action(power_state, &Terminal::Token("POWER".into())),
        TableCell::Shift(_)
    ));

    // 非结合运算符不允许连续出现，冲突位置应落入显式错误单元格。
    let nonassoc_state = completed_state(&grammar, &table, "expression LT expression");
    assert_eq!(
        table.action(nonassoc_state, &Terminal::Token("LT".into())),
        TableCell::Error
    );

    // 优先级不同时，高优先级运算符决定先移进还是先归约。
    assert!(matches!(
        table.action(power_state, &Terminal::Token("PLUS".into())),
        TableCell::Reduce(_)
    ));
    assert!(matches!(
        table.action(plus_state, &Terminal::Token("POWER".into())),
        TableCell::Shift(_)
    ));

    // `%prec UMINUS` 使用替代符号的优先级，而不是产生式首个终结符的优先级。
    let override_state = completed_state(&grammar, &table, "PLUS expression %prec 'UMINUS'");
    assert!(matches!(
        table.action(override_state, &Terminal::Token("PLUS".into())),
        TableCell::Reduce(_)
    ));
}

#[test]
/// 验证缺少优先级声明的冲突不会被静默选择，并携带定位所需上下文。
fn table_reports_unresolved_conflicts() {
    let shift_reduce = grammar(
        r#"
%start expression;
%token NUMBER 256;
%token PLUS 257;
%%
expression : NUMBER | expression PLUS expression;
"#,
    );
    let error = ParseTable::build(&shift_reduce, &Automaton::build(&shift_reduce))
        .expect_err("undeclared shift/reduce conflict must fail");
    assert!(error.message.contains("state"));
    assert!(error.message.contains("PLUS"));
    assert!(
        error
            .message
            .contains("expression -> expression PLUS expression")
    );

    let reduce_reduce = grammar(
        r#"
%start root;
%token VALUE 256;
%%
root : left | right;
left : VALUE;
right : VALUE;
"#,
    );
    let error = ParseTable::build(&reduce_reduce, &Automaton::build(&reduce_reduce))
        .expect_err("reduce/reduce conflict must fail");
    assert!(error.message.contains("state"));
    assert!(error.message.contains("$end"));
    assert!(error.message.contains("left -> VALUE"));
    assert!(error.message.contains("right -> VALUE"));
}

#[test]
/// 验证稀疏整数编码保留完整列布局及动作、跳转语义。
fn table_sparse_encoding_preserves_columns() {
    let (_, table) = table(
        r#"
%start root;
%token FIRST 256;
%token UNUSED 257;
%token LAST 258;
%%
root : FIRST LAST;
"#,
    );
    let encoded = table.encode().expect("table should encode");
    assert_eq!(encoded.columns[0], TableColumn::Terminal(Terminal::End));
    // 即使某个终结符没有非零动作，也必须占据稳定列，避免后续列发生偏移。
    assert_eq!(
        encoded.columns[2],
        TableColumn::Terminal(Terminal::Token("UNUSED".into()))
    );

    let last_column = encoded
        .column(&TableColumn::Terminal(Terminal::Token("LAST".into())))
        .unwrap();
    let row = encoded
        .rows
        .iter()
        .find(|row| row.get(last_column) > 0)
        .unwrap();
    let dense = row.decode(encoded.columns.len());
    assert_eq!(dense[last_column], row.get(last_column));
    // 稀疏行省略的列在随机访问和还原为稠密行时都应解释为错误值零。
    assert_eq!(
        dense[encoded
            .column(&TableColumn::Terminal(Terminal::Token("UNUSED".into())))
            .unwrap()],
        0
    );
    // 条目按列递增是二分查询和确定性输出的共同前提。
    assert!(
        row.entries()
            .windows(2)
            .all(|pair| pair[0].column < pair[1].column)
    );

    assert!(matches!(table.goto(0, "root"), TableCell::Goto(_)));
    // 增广开始规则完成的状态只在输入结束时接受，并编码为专用哨兵值。
    let accept_state = table
        .states()
        .iter()
        .find(|state| {
            state
                .items
                .iter()
                .any(|item| item.rule == ItemRule::AugmentedStart && item.dot == 1)
        })
        .unwrap()
        .id;
    assert_eq!(
        table.action(accept_state, &Terminal::End),
        TableCell::Accept
    );
    assert_eq!(
        encoded.get(accept_state, &TableColumn::Terminal(Terminal::End)),
        ENCODED_ACCEPT
    );
}

#[test]
/// 验证相同文法重复构建时产生完全一致的列、稀疏行和归约编号。
fn table_encoding_is_deterministic() {
    let grammar = grammar(
        r#"
%start expression;
%token NUMBER 300;
%token PLUS 301;
%left PLUS;
%%
expression : NUMBER | expression PLUS expression;
"#,
    );
    let first = ParseTable::build(&grammar, &Automaton::build(&grammar))
        .unwrap()
        .encode()
        .unwrap();
    let second = ParseTable::build(&grammar, &Automaton::build(&grammar))
        .unwrap()
        .encode()
        .unwrap();
    assert_eq!(first, second);
}
