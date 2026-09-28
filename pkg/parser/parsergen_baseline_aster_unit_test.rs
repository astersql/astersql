// Copyright 2026 AsterSQL.

// Parsergen 生成结果与仓库内 Rust 解析表的基线一致性测试。
//
// 本文件分别加载主语法和 Hint 语法，用独立的小型 LR 驱动器回放现有解析表，
// 再将移进、规约、接受或报错轨迹与 `GeneratedParser` 的输出逐步比较；同时检查
// 规约编号、右部符号数量及语义动作标记，防止生成器与运行时表结构悄然偏离。
// 这些辅助函数为 Rust 自研；接受/拒绝契约参照 parser.y 的 SelectStmtBasic、
// StatementList 及 hintparser.y 的 TableOptimizerHintOpt，而非 Go 内部表布局。

use astersql_parsergen::{GeneratedParser, Grammar, ProductionItem, RuleId, TraceStep};
use std::fs;
use std::path::PathBuf;

/// 从当前 parser crate 的 `grammar` 目录加载并解析指定 Aster 语法。
fn load_grammar(name: &str) -> Grammar {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("grammar")
        .join(name);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read Rust grammar {}: {error}", path.display()));
    Grammar::parse(&source)
        .unwrap_or_else(|error| panic!("parse Rust grammar {}: {error}", path.display()))
}

/// 按语法中的终结符名称取得传给解析表的外部 token 编号。
fn token(grammar: &Grammar, name: &str) -> u32 {
    grammar
        .tokens
        .iter()
        .find(|token| token.name == name)
        .unwrap_or_else(|| panic!("missing grammar token {name}"))
        .number
}

/// 直接驱动主解析器现有 Rust 表，得到不依赖 parsergen 实现的基准轨迹。
fn main_baseline_trace(grammar: &Grammar, tokens: &[u32]) -> Vec<TraceStep> {
    let mut trace = Vec::new();
    let mut states = vec![0usize];
    let mut input = tokens
        .iter()
        .copied()
        .map(|token| token as isize)
        .chain(std::iter::once(0));
    let mut lookahead = input.next().expect("EOF is always present");

    for _ in 0..100_000 {
        let state = *states.last().expect("parser stack is never empty");
        let symbol = super::super::GENERATED_MAIN_XLAT
            .binary_search_by_key(&lookahead, |(number, _)| *number)
            .ok()
            .map(|index| super::super::GENERATED_MAIN_XLAT[index].1)
            .unwrap_or(super::super::GENERATED_MAIN_SYMBOL_NAMES.len());
        let action = super::super::GENERATED_MAIN_PARSE_TABLE
            .get(state)
            .and_then(|row| {
                row.binary_search_by_key(&symbol, |(column, _)| *column)
                    .ok()
                    .map(|index| row[index].1)
            })
            .unwrap_or(0);
        if action == super::super::GENERATED_MAIN_ACCEPT {
            trace.push(TraceStep::Accept);
            return trace;
        }
        // 0 表示错误；正数和负数分别用“编号加一”编码移进状态与规约编号。
        if action > 0 {
            trace.push(TraceStep::Shift);
            states.push((action - 1) as usize);
            lookahead = input.next().expect("baseline shifted past EOF");
        } else if action < 0 {
            let generated_rule = (-action - 1) as usize;
            let reduction = super::super::GENERATED_MAIN_REDUCTIONS[generated_rule];
            // 规约先弹出右部符号对应状态，再用栈底状态和左部符号查询 goto。
            states.truncate(states.len() - reduction.1);
            let base = *states.last().expect("reduction leaves a base state");
            let goto = super::super::GENERATED_MAIN_PARSE_TABLE
                .get(base)
                .and_then(|row| {
                    row.binary_search_by_key(&reduction.0, |(column, _)| *column)
                        .ok()
                        .map(|index| row[index].1)
                })
                .unwrap_or(0);
            let rule = super::super::GENERATED_MAIN_LEGACY_RULES[generated_rule];
            assert!(goto > 0, "baseline main goto for rule {rule}");
            states.push((goto - 1) as usize);
            trace.push(TraceStep::Reduce(
                grammar.productions[rule - 1].rule_id.clone(),
            ));
        } else {
            trace.push(TraceStep::Error);
            return trace;
        }
    }
    panic!("baseline main trace did not terminate")
}

/// 使用 Hint 解析器自己的表类型和常量，执行与主解析器相同的基准回放。
fn hint_baseline_trace(grammar: &Grammar, tokens: &[u32]) -> Vec<TraceStep> {
    let mut trace = Vec::new();
    let mut states = vec![0usize];
    let mut input = tokens
        .iter()
        .copied()
        .map(|token| token as i32)
        .chain(std::iter::once(0));
    let mut lookahead = input.next().expect("EOF is always present");

    for _ in 0..10_000 {
        let state = *states.last().expect("parser stack is never empty");
        let symbol = super::GENERATED_HINT_XLAT
            .binary_search_by_key(&lookahead, |(number, _)| *number)
            .ok()
            .map(|index| super::GENERATED_HINT_XLAT[index].1)
            .unwrap_or(super::GENERATED_HINT_SYMBOL_NAMES.len());
        let action = super::GENERATED_HINT_PARSE_TABLE
            .get(state)
            .and_then(|row| {
                row.binary_search_by_key(&symbol, |(column, _)| *column)
                    .ok()
                    .map(|index| row[index].1)
            })
            .unwrap_or(0);
        if action == super::GENERATED_HINT_ACCEPT {
            trace.push(TraceStep::Accept);
            return trace;
        }
        // Hint 表沿用主解析表相同的稀疏存储与动作编码规则。
        if action > 0 {
            trace.push(TraceStep::Shift);
            states.push((action - 1) as usize);
            lookahead = input.next().expect("baseline shifted past EOF");
        } else if action < 0 {
            let generated_rule = (-action - 1) as usize;
            let reduction = super::GENERATED_HINT_REDUCTIONS[generated_rule];
            states.truncate(states.len() - reduction.1);
            let base = *states.last().expect("reduction leaves a base state");
            let goto = super::GENERATED_HINT_PARSE_TABLE
                .get(base)
                .and_then(|row| {
                    row.binary_search_by_key(&reduction.0, |(column, _)| *column)
                        .ok()
                        .map(|index| row[index].1)
                })
                .unwrap_or(0);
            let rule = super::GENERATED_HINT_LEGACY_RULES[generated_rule];
            assert!(goto > 0, "baseline hint goto for rule {rule}");
            states.push((goto - 1) as usize);
            trace.push(TraceStep::Reduce(
                grammar.productions[rule - 1].rule_id.clone(),
            ));
        } else {
            trace.push(TraceStep::Error);
            return trace;
        }
    }
    panic!("baseline hint trace did not terminate")
}

/// 仅提取轨迹中的规约规则，供测试断言特殊产生式确实被经过。
fn reduced_rule_ids(trace: &[TraceStep]) -> impl Iterator<Item = &RuleId> {
    trace.iter().filter_map(|step| match step {
        TraceStep::Reduce(rule) => Some(rule),
        _ => None,
    })
}

/// 核对生成器的规约元数据仍与语法产生式一一对应。
fn assert_metadata_matches(grammar: &Grammar, generated: &GeneratedParser) {
    assert_eq!(
        generated.rule_ids_by_reduction.len(),
        generated.reductions.len()
    );
    assert_eq!(
        generated.action_required_by_reduction.len(),
        generated.reductions.len()
    );
    for (index, rule_id) in generated.rule_ids_by_reduction.iter().enumerate() {
        let production = grammar
            .productions
            .iter()
            .find(|production| &production.rule_id == rule_id)
            .expect("rendered rule originates from grammar");
        // 语义动作不占解析栈位置，因此规约长度只统计真正的语法符号。
        let component_count = production
            .rhs
            .iter()
            .filter(|item| !matches!(item, ProductionItem::Action { .. }))
            .count();
        assert_eq!(generated.reductions[index].components, component_count);
        assert_eq!(
            generated.action_required_by_reduction[index],
            production.requires_action
        );
    }
}

fn render_digest(source: &str) -> u64 {
    source
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |digest, byte| {
            (digest ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn assert_render_is_stable(name: &str, generated: &GeneratedParser) {
    let first = generated.render_rust();
    let second = generated.render_rust();
    assert_eq!(first.as_bytes(), second.as_bytes());
    let first_digest = render_digest(&first);
    let second_digest = render_digest(&second);
    assert_eq!(first_digest, second_digest);
    eprintln!(
        "{name} render fnv1a64={first_digest:016x}, bytes={}",
        first.len()
    );
}

#[test]
fn parsergen_main_tables_match_rust_baseline() {
    let grammar = load_grammar("main.astergram");
    let generated = GeneratedParser::build(&grammar).expect("main parser data should build");
    assert_metadata_matches(&grammar, &generated);
    assert_render_is_stable("main", &generated);

    // 终态独立于两套实现，防止双方相同的错误轨迹被当作一致性证据。
    let cases = [
        (
            vec![token(&grammar, "selectKwd"), token(&grammar, "intLit")],
            TraceStep::Accept,
        ),
        (
            vec![
                token(&grammar, "selectKwd"),
                token(&grammar, "intLit"),
                token(&grammar, "literal_3b"),
            ],
            TraceStep::Accept,
        ),
        (
            vec![token(&grammar, "selectKwd"), token(&grammar, "literal_29")],
            TraceStep::Error,
        ),
        (vec![], TraceStep::Accept),
        (vec![token(&grammar, "selectKwd")], TraceStep::Error),
        (vec![u32::MAX], TraceStep::Error),
        (
            vec![
                token(&grammar, "selectKwd"),
                token(&grammar, "intLit"),
                u32::MAX,
            ],
            TraceStep::Error,
        ),
    ];
    for (tokens, expected) in cases {
        let baseline = main_baseline_trace(&grammar, &tokens);
        let actual = generated.trace(&tokens);
        assert_eq!(actual.last(), Some(&expected), "main token flow {tokens:?}");
        assert_eq!(actual, baseline, "main token flow {tokens:?}");
    }

    // 普通 SELECT 路径还应覆盖纯动作产生式，避免零长度规约被生成器遗漏。
    let normal = main_baseline_trace(
        &grammar,
        &[token(&grammar, "selectKwd"), token(&grammar, "intLit")],
    );
    assert!(normal.contains(&TraceStep::Accept));
    assert!(reduced_rule_ids(&normal).any(|rule| {
        let production = grammar
            .productions
            .iter()
            .find(|production| &production.rule_id == rule)
            .unwrap();
        production
            .rhs
            .iter()
            .all(|item| matches!(item, ProductionItem::Action { .. }))
    }));
}

#[test]
fn parsergen_hint_tables_match_rust_baseline() {
    let grammar = load_grammar("hint.astergram");
    let generated = GeneratedParser::build(&grammar).expect("hint parser data should build");
    assert_metadata_matches(&grammar, &generated);
    assert_render_is_stable("hint", &generated);

    let cases = [
        (
            vec![
                token(&grammar, "hintJoinFixedOrder"),
                token(&grammar, "literal_28"),
                token(&grammar, "literal_29"),
            ],
            TraceStep::Accept,
        ),
        (
            vec![
                token(&grammar, "hintHashJoin"),
                token(&grammar, "literal_28"),
                token(&grammar, "hintIdentifier"),
                token(&grammar, "literal_29"),
            ],
            TraceStep::Accept,
        ),
        (vec![token(&grammar, "hintWriteSlowLog")], TraceStep::Accept),
        (
            vec![
                token(&grammar, "hintJoinFixedOrder"),
                token(&grammar, "literal_29"),
            ],
            TraceStep::Error,
        ),
        (vec![], TraceStep::Error),
        (
            vec![
                token(&grammar, "hintJoinFixedOrder"),
                token(&grammar, "literal_28"),
            ],
            TraceStep::Error,
        ),
        (vec![u32::MAX], TraceStep::Error),
        (
            vec![token(&grammar, "hintWriteSlowLog"), u32::MAX],
            TraceStep::Error,
        ),
    ];
    for (tokens, expected) in cases {
        let baseline = hint_baseline_trace(&grammar, &tokens);
        let actual = generated.trace(&tokens);
        assert_eq!(actual.last(), Some(&expected), "hint token flow {tokens:?}");
        assert_eq!(actual, baseline, "hint token flow {tokens:?}");
    }
}
