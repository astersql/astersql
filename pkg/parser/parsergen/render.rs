// Copyright 2026 AsterSQL.

//! 确定性地渲染 Rust 解析器数据，并提供仅依赖解析表的追踪驱动器。
//!
//! 生成的源码包含可独立使用的 token 值、XLAT、符号名、归约信息、稀疏解析表行、
//! 稳定规则标识符及动作覆盖元数据；语义动作刻意留在本 crate 之外。

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::{
    Automaton, EncodedTable, Grammar, ParseTable, Production, ProductionItem, RuleId, TableCell,
    TableColumn, TableError, Terminal,
};

/// 一条已渲染的归约信息，其索引顺序与编码后的解析表一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedReduction {
    /// 产生式左部非终结符在解析表中的列号。
    pub symbol: usize,
    /// 归约时需要弹出的语法符号数，不包含语义动作。
    pub components: usize,
}

/// 与状态编号无关的解析行为，用于基线结果比较。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceStep {
    Shift,
    Reduce(RuleId),
    Accept,
    Error,
}

/// 从一份已验证语法构造出的完整解析器数据。
///
/// 公开字段用于生成自包含源码；私有字段保留未编码的解析表与产生式信息，供追踪器复现
/// shift/reduce/goto 流程。
#[derive(Clone, Debug)]
pub struct GeneratedParser {
    pub tokens: Vec<(String, u32)>,
    pub xlat: Vec<(u32, usize)>,
    pub symbol_names: Vec<String>,
    pub reductions: Vec<GeneratedReduction>,
    pub parse_table: EncodedTable,
    pub rule_ids_by_reduction: Vec<RuleId>,
    pub action_required_by_reduction: Vec<bool>,
    table: ParseTable,
    productions: BTreeMap<RuleId, GeneratedProduction>,
    terminals_by_number: BTreeMap<u32, Terminal>,
}

#[derive(Clone, Debug)]
struct GeneratedProduction {
    lhs: String,
    components: usize,
}

impl GeneratedParser {
    /// 不读取文件或调用外部工具，直接构造全部解析器数据。
    ///
    /// token、规则和表列的顺序均由语法及编码表确定，以保证相同输入产生稳定输出。
    pub fn build(grammar: &Grammar) -> Result<Self, RenderError> {
        let automaton = Automaton::build(grammar);
        let table = ParseTable::build(grammar, &automaton)?;
        let parse_table = table.encode()?;
        let productions: BTreeMap<_, _> = grammar
            .productions
            .iter()
            .map(|production| {
                (
                    production.rule_id.clone(),
                    GeneratedProduction {
                        lhs: production.lhs.clone(),
                        components: component_count(production),
                    },
                )
            })
            .collect();

        let tokens = sorted_tokens(grammar);
        let terminals_by_number = grammar
            .tokens
            .iter()
            .map(|token| (token.number, Terminal::Token(token.name.clone())))
            .collect();
        let xlat = tokens
            .iter()
            .map(|(name, number)| {
                let column = parse_table
                    .column(&TableColumn::Terminal(Terminal::Token(name.clone())))
                    .expect("every grammar token has a table column");
                (*number, column)
            })
            .collect();
        let symbol_names = parse_table.columns.iter().map(column_name).collect();

        let by_rule: BTreeMap<_, _> = grammar
            .productions
            .iter()
            .map(|production| (production.rule_id.clone(), production))
            .collect();
        let mut reductions = Vec::with_capacity(parse_table.reduction_rules.len());
        let mut action_required_by_reduction =
            Vec::with_capacity(parse_table.reduction_rules.len());
        for rule in &parse_table.reduction_rules {
            let production = by_rule
                .get(rule)
                .expect("encoded reductions originate from the grammar");
            let symbol = parse_table
                .column(&TableColumn::Nonterminal(production.lhs.clone()))
                .expect("every production lhs has a table column");
            reductions.push(GeneratedReduction {
                symbol,
                components: component_count(production),
            });
            action_required_by_reduction.push(production.requires_action);
        }

        Ok(Self {
            tokens,
            xlat,
            symbol_names,
            reductions,
            rule_ids_by_reduction: parse_table.reduction_rules.clone(),
            action_required_by_reduction,
            parse_table,
            table,
            productions,
            terminals_by_number,
        })
    }

    /// 追踪一条 token 编号流，并自动在末尾补上输入结束符。
    ///
    /// 未知 token、非法归约/goto 或超过步数上限都会以 `Error` 结束，避免损坏的表使
    /// 追踪过程无限循环。
    pub fn trace(&self, tokens: &[u32]) -> Vec<TraceStep> {
        let mut input = tokens
            .iter()
            .map(|number| self.terminals_by_number.get(number).cloned())
            .chain(std::iter::once(Some(Terminal::End)));
        let mut lookahead = input.next().flatten();
        let mut states = vec![0usize];
        let mut trace = Vec::new();

        for _ in 0..1_000_000 {
            let Some(terminal) = lookahead.as_ref() else {
                trace.push(TraceStep::Error);
                return trace;
            };
            let state = *states.last().expect("parser stack is never empty");
            match self.table.action(state, terminal) {
                TableCell::Shift(target) => {
                    trace.push(TraceStep::Shift);
                    states.push(target);
                    lookahead = input.next().flatten();
                }
                TableCell::Reduce(rule) => {
                    let production = self
                        .productions
                        .get(&rule)
                        .expect("table reductions originate from the grammar");
                    if production.components >= states.len() {
                        trace.push(TraceStep::Error);
                        return trace;
                    }
                    states.truncate(states.len() - production.components);
                    let base = *states.last().expect("reduction leaves a base state");
                    let TableCell::Goto(target) = self.table.goto(base, &production.lhs) else {
                        trace.push(TraceStep::Error);
                        return trace;
                    };
                    states.push(target);
                    trace.push(TraceStep::Reduce(rule));
                }
                TableCell::Accept => {
                    trace.push(TraceStep::Accept);
                    return trace;
                }
                TableCell::Error | TableCell::Goto(_) => {
                    trace.push(TraceStep::Error);
                    return trace;
                }
            }
        }

        trace.push(TraceStep::Error);
        trace
    }

    /// Render already-built parser data without rebuilding the automaton.
    pub fn render_rust(&self) -> String {
        render_generated(self)
    }
}

/// 构造或渲染解析器数据时的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderError {
    message: String,
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RenderError {}

impl From<TableError> for RenderError {
    fn from(error: TableError) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

/// 将一份语法渲染为确定、可独立使用的 Rust 常量源码。
pub fn render_rust(grammar: &Grammar) -> Result<String, RenderError> {
    let generated = GeneratedParser::build(grammar)?;
    Ok(generated.render_rust())
}

fn render_generated(generated: &GeneratedParser) -> String {
    let variants = rule_variants(&generated.rule_ids_by_reduction);
    let mut output = String::from(
        "// Copyright 2026 AsterSQL.\n\n\
         // @generated by astersql-parsergen; do not edit.\n\
         pub const EOF_TOKEN: u32 = 0;\n\n\
         pub mod token {\n",
    );
    let token_identifiers = unique_token_identifiers(&generated.tokens);
    for ((_, number), identifier) in generated.tokens.iter().zip(token_identifiers) {
        output.push_str("    pub const ");
        output.push_str(&identifier);
        output.push_str(": u32 = ");
        output.push_str(&number.to_string());
        output.push_str(";\n");
    }
    output.push_str("}\n\n");

    output.push_str("pub static XLAT: &[(u32, usize)] = &[\n    (EOF_TOKEN, 0),\n");
    for (number, column) in &generated.xlat {
        output.push_str(&format!("    ({number}, {column}),\n"));
    }
    output.push_str("];\n\n");

    output.push_str("pub static SYMBOL_NAMES: &[&str] = &[\n");
    for name in &generated.symbol_names {
        output.push_str("    ");
        output.push_str(&format!("{name:?}"));
        output.push_str(",\n");
    }
    output.push_str("];\n\n");

    output.push_str(
        "#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n\
         pub enum RuleId {\n",
    );
    for variant in &variants {
        output.push_str("    ");
        output.push_str(variant);
        output.push_str(",\n");
    }
    output.push_str("}\n\nimpl RuleId {\n    pub const fn as_str(self) -> &'static str {\n        match self {\n");
    for (variant, rule) in variants.iter().zip(&generated.rule_ids_by_reduction) {
        output.push_str(&format!(
            "            Self::{variant} => {:?},\n",
            rule.as_str()
        ));
    }
    output.push_str("        }\n    }\n}\n\n");

    output.push_str("pub static RULE_IDS_BY_REDUCTION: &[RuleId] = &[\n");
    for variant in &variants {
        output.push_str(&format!("    RuleId::{variant},\n"));
    }
    output.push_str("];\n\n");

    output.push_str("pub static ACTION_REQUIRED_BY_REDUCTION: &[bool] = &[\n");
    for required in &generated.action_required_by_reduction {
        output.push_str(&format!("    {required},\n"));
    }
    output.push_str("];\n\n");

    output.push_str("pub static REDUCTIONS: &[(usize, usize)] = &[\n");
    for reduction in &generated.reductions {
        output.push_str(&format!(
            "    ({}, {}),\n",
            reduction.symbol, reduction.components
        ));
    }
    output.push_str("];\n\n");

    output.push_str("pub static PARSE_TABLE: &[&[(usize, i32)]] = &[\n");
    for row in &generated.parse_table.rows {
        output.push_str("    &[");
        for (index, entry) in row.entries().iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            output.push_str(&format!("({}, {})", entry.column, entry.value));
        }
        output.push_str("],\n");
    }
    output.push_str("];\n");
    output
}

fn sorted_tokens(grammar: &Grammar) -> Vec<(String, u32)> {
    let mut tokens: Vec<_> = grammar
        .tokens
        .iter()
        .map(|token| (token.name.clone(), token.number))
        .collect();
    tokens.sort_by(|left, right| (left.1, &left.0).cmp(&(right.1, &right.0)));
    tokens
}

fn component_count(production: &Production) -> usize {
    // 语义动作不占解析栈位置，因此归约长度只统计真正的语法符号。
    production
        .rhs
        .iter()
        .filter(|item| !matches!(item, ProductionItem::Action { .. }))
        .count()
}

fn column_name(column: &TableColumn) -> String {
    match column {
        TableColumn::Terminal(terminal) => terminal.name().to_owned(),
        TableColumn::Nonterminal(name) => name.clone(),
    }
}

fn rule_variants(rules: &[RuleId]) -> Vec<String> {
    // 优先从稳定规则标识符的摘要生成枚举名；摘要不可用或重复时再使用确定性后缀。
    let mut used = BTreeSet::new();
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let digest = rule
                .as_str()
                .rsplit_once("--")
                .map(|(_, digest)| digest)
                .unwrap_or(rule.as_str());
            let base = format!(
                "R{}",
                digest
                    .chars()
                    .filter(|current| current.is_ascii_hexdigit())
                    .collect::<String>()
                    .to_ascii_uppercase()
            );
            let base = if base == "R" {
                format!("R{index}")
            } else {
                base
            };
            let mut variant = base.clone();
            let mut suffix = 2usize;
            while !used.insert(variant.clone()) {
                variant = format!("{base}_{suffix}");
                suffix += 1;
            }
            variant
        })
        .collect()
}

fn rust_token_identifier(name: &str) -> String {
    let mut identifier = String::new();
    for (index, current) in name.chars().enumerate() {
        let valid = if index == 0 {
            current.is_ascii_alphabetic() || current == '_'
        } else {
            current.is_ascii_alphanumeric() || current == '_'
        };
        if valid {
            identifier.push(current);
        } else {
            identifier.push_str(&format!("_u{:X}_", u32::from(current)));
        }
    }
    if identifier.is_empty() {
        identifier.push_str("TOKEN_EMPTY");
    }

    // 路径关键字不能写成原始标识符，其余 Rust 关键字则使用 `r#` 形式保留原名。
    match identifier.as_str() {
        "self" | "Self" | "super" | "crate" => format!("TOKEN_{name}"),
        identifier if is_rust_keyword(identifier) => format!("r#{identifier}"),
        _ => identifier,
    }
}

fn unique_token_identifiers(tokens: &[(String, u32)]) -> Vec<String> {
    let mut used = BTreeSet::new();
    tokens
        .iter()
        .map(|(name, _)| {
            let base = rust_token_identifier(name);
            let mut identifier = base.clone();
            let mut suffix = 2usize;
            while !used.insert(identifier.clone()) {
                identifier = format!("{base}_{suffix}");
                suffix += 1;
            }
            identifier
        })
        .collect()
}

fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "static"
            | "struct"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "gen"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
}
