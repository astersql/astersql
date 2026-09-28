// Copyright 2026 AsterSQL.

// 解析器代码生成器的渲染回归测试。
//
// 使用包含空产生式、显式动作与运算符优先级的最小文法，验证相同输入会生成
// 逐字节一致的 Rust 源码，并检查关键生成项存在、内部规约元数据数量保持对应。

use crate::{GeneratedParser, Grammar, render_rust};

#[test]
fn render_is_byte_reproducible() {
    // 同时覆盖有动作和无动作的分支，避免确定性检查只经过最简单的生成路径。
    let grammar = Grammar::parse(
        r#"
%start statement;
%token IDENT 256;
%token PLUS 43 "+";
%left PLUS;
%%
statement : expression @action;
expression : ε @action
           | IDENT
           | expression PLUS IDENT @action;
"#,
    )
    .expect("test grammar should parse");

    // 直接比较最终字节，可捕获空白、生成项顺序等不影响语义但破坏可复现性的漂移。
    let first = render_rust(&grammar).expect("first render should succeed");
    let second = render_rust(&grammar).expect("second render should succeed");
    assert_eq!(first.as_bytes(), second.as_bytes());

    // 除确认关键生成项存在外，还要求按规约编号并行维护的两组元数据长度一致。
    let generated = GeneratedParser::build(&grammar).expect("parser data should build");
    assert!(first.contains("pub mod token"));
    assert!(first.contains("XLAT"));
    assert!(first.contains("SYMBOL_NAMES"));
    assert!(first.contains("REDUCTIONS"));
    assert!(first.contains("PARSE_TABLE"));
    assert!(first.contains("enum RuleId"));
    assert!(first.contains("RULE_IDS_BY_REDUCTION"));
    assert!(first.contains("ACTION_REQUIRED_BY_REDUCTION"));
    assert_eq!(
        generated.rule_ids_by_reduction.len(),
        generated.action_required_by_reduction.len()
    );
}
