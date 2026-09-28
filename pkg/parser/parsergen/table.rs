// Copyright 2026 AsterSQL.

//! 语法分析器 action/goto 表的构造与确定性稀疏编码。
//!
//! 构表阶段始终以类型化单元表示动作，先按文法优先级消解冲突，再由
//! [`ParseTable::encode`] 集中转换为整数编码。这样，构造和诊断过程无需依赖
//! 数值哨兵，生成结果也能保持稳定顺序。

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::{
    Associativity, Automaton, Grammar, ItemRule, PrecedenceSymbol, Production, ProductionItem,
    RuleId, State, Symbol, Terminal,
};

/// 编码表中表示显式接受动作的整数哨兵。
pub const ENCODED_ACCEPT: i32 = i32::MIN;

/// 一个尚未编码的语法分析表单元。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableCell {
    /// 移进到指定状态。
    Shift(usize),
    /// 按指定产生式归约。
    Reduce(RuleId),
    /// 接受输入。
    Accept,
    /// 语法错误；也用于查询不存在的表项。
    Error,
    /// 非终结符转换到指定状态。
    Goto(usize),
}

/// 合并后的 action/goto 编码表中的稳定列标识。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TableColumn {
    /// action 区域的终结符列。
    Terminal(Terminal),
    /// goto 区域的非终结符列。
    Nonterminal(String),
}

/// 构造或编码语法分析表时产生的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableError {
    /// 冲突所在状态；纯编码错误没有该信息。
    pub state: Option<usize>,
    /// 触发冲突的向前看终结符。
    pub lookahead: Option<Terminal>,
    /// 尚未消解的候选动作描述，按稳定顺序保存。
    pub candidates: Vec<String>,
    /// 面向调用方的完整错误消息。
    pub message: String,
}

impl TableError {
    fn conflict(state: usize, lookahead: &Terminal, candidates: Vec<String>) -> Self {
        let message = format!(
            "unresolved parser conflict in state {state} on lookahead '{}': {}",
            lookahead.name(),
            candidates.join("; ")
        );
        Self {
            state: Some(state),
            lookahead: Some(lookahead.clone()),
            candidates,
            message,
        }
    }

    fn encoding(message: impl Into<String>) -> Self {
        Self {
            state: None,
            lookahead: None,
            candidates: Vec::new(),
            message: message.into(),
        }
    }
}

impl fmt::Display for TableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TableError {}

/// 已完成冲突消解、但尚未转换为整数的语法分析表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseTable {
    states: Vec<State>,
    actions: Vec<BTreeMap<Terminal, TableCell>>,
    gotos: Vec<BTreeMap<String, TableCell>>,
    columns: Vec<TableColumn>,
    reduction_rules: Vec<RuleId>,
}

impl ParseTable {
    /// 将 LALR 状态图转换成类型化的 action 与 goto 单元。
    pub fn build(grammar: &Grammar, automaton: &Automaton) -> Result<Self, TableError> {
        let productions: BTreeMap<_, _> = grammar
            .productions
            .iter()
            .map(|production| (production.rule_id.clone(), production))
            .collect();
        let precedence = PrecedenceIndex::new(grammar);
        let mut candidates =
            vec![BTreeMap::<Terminal, BTreeSet<Candidate>>::new(); automaton.states.len()];
        let mut gotos = vec![BTreeMap::new(); automaton.states.len()];

        // 转移先产生移进/goto 候选，完成项目再补充归约或接受候选。
        for state in &automaton.states {
            for (symbol, target) in &state.transitions {
                match symbol {
                    Symbol::Terminal(terminal) => {
                        candidates[state.id]
                            .entry(terminal.clone())
                            .or_default()
                            .insert(Candidate::Shift(*target));
                    }
                    Symbol::Nonterminal(nonterminal) => {
                        gotos[state.id].insert(nonterminal.clone(), TableCell::Goto(*target));
                    }
                }
            }

            for item in &state.items {
                match &item.rule {
                    ItemRule::AugmentedStart if item.dot == 1 => {
                        candidates[state.id]
                            .entry(Terminal::End)
                            .or_default()
                            .insert(Candidate::Accept);
                    }
                    ItemRule::Production(rule) => {
                        let production = productions
                            .get(rule)
                            .expect("automaton rules originate from the grammar");
                        if item.dot == production_symbol_count(production) {
                            for lookahead in &item.lookaheads {
                                candidates[state.id]
                                    .entry(lookahead.clone())
                                    .or_default()
                                    .insert(Candidate::Reduce(rule.clone()));
                            }
                        }
                    }
                    ItemRule::AugmentedStart => {}
                }
            }
        }

        // 所有候选收集完毕后统一消解，避免遍历顺序影响冲突判定。
        let mut actions = vec![BTreeMap::new(); automaton.states.len()];
        for (state, row) in candidates.into_iter().enumerate() {
            for (lookahead, candidates) in row {
                let cell =
                    resolve_candidates(state, &lookahead, &candidates, &productions, &precedence)?;
                actions[state].insert(lookahead, cell);
            }
        }

        let columns = stable_columns(grammar);
        let mut reduction_rules: Vec<_> = grammar
            .productions
            .iter()
            .map(|production| production.rule_id.clone())
            .collect();
        reduction_rules.sort();
        reduction_rules.dedup();

        Ok(Self {
            states: automaton.states.clone(),
            actions,
            gotos,
            columns,
            reduction_rules,
        })
    }

    /// 返回该表对应的状态图。
    pub fn states(&self) -> &[State] {
        &self.states
    }

    /// 查询 action；状态越界或表项缺失均按语法错误处理。
    pub fn action(&self, state: usize, lookahead: &Terminal) -> TableCell {
        self.actions
            .get(state)
            .and_then(|row| row.get(lookahead))
            .cloned()
            .unwrap_or(TableCell::Error)
    }

    /// 查询 goto；状态越界或表项缺失均按语法错误处理。
    pub fn goto(&self, state: usize, nonterminal: &str) -> TableCell {
        self.gotos
            .get(state)
            .and_then(|row| row.get(nonterminal))
            .cloned()
            .unwrap_or(TableCell::Error)
    }

    /// 将类型化单元编码为顺序稳定的稀疏整数行。
    ///
    /// `0` 表示错误，正数表示从 1 开始编号的移进/goto 状态，负数表示从 1
    /// 开始编号的归约序号，[`ENCODED_ACCEPT`] 表示接受。
    pub fn encode(&self) -> Result<EncodedTable, TableError> {
        let column_numbers: BTreeMap<_, _> = self
            .columns
            .iter()
            .cloned()
            .enumerate()
            .map(|(column, symbol)| (symbol, column))
            .collect();
        let reduction_numbers: BTreeMap<_, _> = self
            .reduction_rules
            .iter()
            .cloned()
            .enumerate()
            .map(|(number, rule)| (rule, number))
            .collect();
        let mut rows = Vec::with_capacity(self.states.len());

        for state in 0..self.states.len() {
            let mut entries = Vec::new();
            for (terminal, cell) in &self.actions[state] {
                let value = encode_cell(cell, &reduction_numbers)?;
                if value != 0 {
                    entries.push(SparseEntry {
                        column: column_numbers[&TableColumn::Terminal(terminal.clone())],
                        value,
                    });
                }
            }
            for (nonterminal, cell) in &self.gotos[state] {
                let value = encode_cell(cell, &reduction_numbers)?;
                if value != 0 {
                    entries.push(SparseEntry {
                        column: column_numbers[&TableColumn::Nonterminal(nonterminal.clone())],
                        value,
                    });
                }
            }
            // 稀疏行必须按列排序，既保证输出确定，也满足二分查询的前提。
            entries.sort_by_key(|entry| entry.column);
            rows.push(SparseRow { entries });
        }

        Ok(EncodedTable {
            columns: self.columns.clone(),
            rows,
            reduction_rules: self.reduction_rules.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Candidate {
    Shift(usize),
    Reduce(RuleId),
    Accept,
}

fn resolve_candidates(
    state: usize,
    lookahead: &Terminal,
    candidates: &BTreeSet<Candidate>,
    productions: &BTreeMap<RuleId, &Production>,
    precedence: &PrecedenceIndex,
) -> Result<TableCell, TableError> {
    if candidates.len() == 1 {
        return Ok(candidate_cell(candidates.first().expect("one candidate")));
    }

    let shift = candidates.iter().find_map(|candidate| match candidate {
        Candidate::Shift(target) => Some(*target),
        _ => None,
    });
    let reduces: Vec<_> = candidates
        .iter()
        .filter_map(|candidate| match candidate {
            Candidate::Reduce(rule) => Some(rule),
            _ => None,
        })
        .collect();
    let only_shift_reduce = candidates.len() == 2 && shift.is_some() && reduces.len() == 1;

    // 仅标准的一移进一归约冲突可由优先级解决；其他组合必须显式报错。
    if only_shift_reduce {
        let rule = reduces[0];
        if let Some(resolved) = precedence.resolve_shift_reduce(
            lookahead,
            productions
                .get(rule)
                .expect("reduce candidates originate from the grammar"),
        ) {
            return Ok(match resolved {
                ShiftReduceResolution::Shift => TableCell::Shift(shift.expect("shift candidate")),
                ShiftReduceResolution::Reduce => TableCell::Reduce(rule.clone()),
                ShiftReduceResolution::Error => TableCell::Error,
            });
        }
    }

    let labels = candidates
        .iter()
        .map(|candidate| candidate_label(candidate, productions))
        .collect();
    Err(TableError::conflict(state, lookahead, labels))
}

fn candidate_cell(candidate: &Candidate) -> TableCell {
    match candidate {
        Candidate::Shift(target) => TableCell::Shift(*target),
        Candidate::Reduce(rule) => TableCell::Reduce(rule.clone()),
        Candidate::Accept => TableCell::Accept,
    }
}

fn candidate_label(candidate: &Candidate, productions: &BTreeMap<RuleId, &Production>) -> String {
    match candidate {
        Candidate::Shift(target) => format!("shift to state {target}"),
        Candidate::Reduce(rule) => format!(
            "reduce rule '{}' ({rule})",
            productions
                .get(rule)
                .expect("reduce candidates originate from the grammar")
                .signature
        ),
        Candidate::Accept => "accept".to_owned(),
    }
}

fn production_symbol_count(production: &Production) -> usize {
    // 内嵌动作不消耗输入，因此不占 LR 项目中点位置对应的文法符号数。
    production
        .rhs
        .iter()
        .filter(|item| !matches!(item, ProductionItem::Action { .. }))
        .count()
}

#[derive(Clone, Copy)]
struct PrecedenceInfo {
    level: usize,
    associativity: Associativity,
}

struct PrecedenceIndex {
    /// 已声明的 token 名称，用于区分 RHS 中的终结符与非终结符。
    tokens: BTreeSet<String>,
    /// 字面量到 token 名称的反向索引。
    literal_tokens: BTreeMap<String, String>,
    /// token 对应的优先级和结合性。
    levels: BTreeMap<String, PrecedenceInfo>,
}

impl PrecedenceIndex {
    fn new(grammar: &Grammar) -> Self {
        let tokens: BTreeSet<_> = grammar
            .tokens
            .iter()
            .map(|token| token.name.clone())
            .collect();
        let literal_tokens: BTreeMap<_, _> = grammar
            .tokens
            .iter()
            .filter_map(|token| {
                token
                    .literal
                    .as_ref()
                    .map(|literal| (literal.clone(), token.name.clone()))
            })
            .collect();
        let mut levels = BTreeMap::new();
        for declaration in &grammar.precedence {
            for symbol in &declaration.symbols {
                let token = match symbol {
                    PrecedenceSymbol::Name(name) => name.clone(),
                    PrecedenceSymbol::Literal(literal) => literal_tokens[literal].clone(),
                };
                levels.insert(
                    token,
                    PrecedenceInfo {
                        level: declaration.level,
                        associativity: declaration.associativity,
                    },
                );
            }
        }
        Self {
            tokens,
            literal_tokens,
            levels,
        }
    }

    fn resolve_shift_reduce(
        &self,
        lookahead: &Terminal,
        production: &Production,
    ) -> Option<ShiftReduceResolution> {
        let Terminal::Token(lookahead) = lookahead else {
            return None;
        };
        let shift = self.levels.get(lookahead)?;
        let reduce = self.production_precedence(production)?;
        // 优先级较高的一方胜出；同级时再由结合性决定方向。
        Some(match shift.level.cmp(&reduce.level) {
            std::cmp::Ordering::Greater => ShiftReduceResolution::Shift,
            std::cmp::Ordering::Less => ShiftReduceResolution::Reduce,
            std::cmp::Ordering::Equal => match shift.associativity {
                Associativity::Left => ShiftReduceResolution::Reduce,
                Associativity::Right => ShiftReduceResolution::Shift,
                Associativity::NonAssoc => ShiftReduceResolution::Error,
                Associativity::PrecedenceOnly => return None,
            },
        })
    }

    fn production_precedence(&self, production: &Production) -> Option<&PrecedenceInfo> {
        if let Some(symbol) = &production.precedence {
            return self.levels.get(self.token_name(symbol)?);
        }
        // 未显式指定时，沿用产生式最右侧终结符的优先级，跳过内嵌动作。
        for item in production.rhs.iter().rev() {
            let token = match item {
                ProductionItem::Symbol(name) if self.tokens.contains(name) => Some(name.as_str()),
                ProductionItem::Literal(literal) => {
                    self.literal_tokens.get(literal).map(String::as_str)
                }
                ProductionItem::Symbol(_) | ProductionItem::Action { .. } => None,
            };
            if let Some(token) = token {
                return self.levels.get(token);
            }
        }
        None
    }

    fn token_name<'a>(&'a self, symbol: &'a PrecedenceSymbol) -> Option<&'a str> {
        match symbol {
            PrecedenceSymbol::Name(name) => Some(name),
            PrecedenceSymbol::Literal(literal) => {
                self.literal_tokens.get(literal).map(String::as_str)
            }
        }
    }
}

enum ShiftReduceResolution {
    Shift,
    Reduce,
    Error,
}

fn stable_columns(grammar: &Grammar) -> Vec<TableColumn> {
    // 列顺序固定为 EOF、按编号排序的 token、按名称排序的非终结符。
    let mut tokens: Vec<_> = grammar.tokens.iter().collect();
    tokens.sort_by_key(|token| (token.number, token.name.as_str()));
    let mut columns = vec![TableColumn::Terminal(Terminal::End)];
    columns.extend(
        tokens
            .into_iter()
            .map(|token| TableColumn::Terminal(Terminal::Token(token.name.clone()))),
    );
    let nonterminals: BTreeSet<_> = grammar
        .productions
        .iter()
        .map(|production| production.lhs.clone())
        .collect();
    columns.extend(nonterminals.into_iter().map(TableColumn::Nonterminal));
    columns
}

fn encode_cell(
    cell: &TableCell,
    reduction_numbers: &BTreeMap<RuleId, usize>,
) -> Result<i32, TableError> {
    match cell {
        TableCell::Error => Ok(0),
        TableCell::Accept => Ok(ENCODED_ACCEPT),
        TableCell::Shift(target) | TableCell::Goto(target) => {
            let value = target
                .checked_add(1)
                .and_then(|target| i32::try_from(target).ok())
                .ok_or_else(|| TableError::encoding(format!("state {target} does not fit i32")))?;
            Ok(value)
        }
        TableCell::Reduce(rule) => {
            let number = reduction_numbers
                .get(rule)
                .expect("encoded reductions originate from the grammar")
                .checked_add(1)
                .and_then(|number| i32::try_from(number).ok())
                .ok_or_else(|| {
                    TableError::encoding(format!("reduction rule {rule} does not fit i32"))
                })?;
            Ok(-number)
        }
    }
}

/// 稀疏行中一个带显式列位置的非零值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SparseEntry {
    /// 在完整编码表中的列号。
    pub column: usize,
    /// 按 [`ParseTable::encode`] 约定生成的动作值。
    pub value: i32,
}

/// 编码表中的一行，仅存储非零单元。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SparseRow {
    entries: Vec<SparseEntry>,
}

impl SparseRow {
    /// 返回按列号升序排列的非零单元。
    pub fn entries(&self) -> &[SparseEntry] {
        &self.entries
    }

    /// 查询指定列；稀疏行中不存在的列按错误值 `0` 处理。
    pub fn get(&self, column: usize) -> i32 {
        self.entries
            .binary_search_by_key(&column, |entry| entry.column)
            .ok()
            .map(|index| self.entries[index].value)
            .unwrap_or(0)
    }

    /// 按给定宽度展开为完整整数行。
    ///
    /// 若稀疏单元超出宽度，说明调用方提供的列宽与表结构不一致，此时直接失败。
    pub fn decode(&self, width: usize) -> Vec<i32> {
        let mut values = vec![0; width];
        for entry in &self.entries {
            assert!(entry.column < width, "sparse entry exceeds row width");
            values[entry.column] = entry.value;
        }
        values
    }
}

/// 顺序确定、可直接交给运行时代码生成器的整数表。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedTable {
    /// action 列与 goto 列的稳定布局。
    pub columns: Vec<TableColumn>,
    /// 与自动机状态编号一一对应的稀疏行。
    pub rows: Vec<SparseRow>,
    /// 负数归约序号所引用的稳定规则列表。
    pub reduction_rules: Vec<RuleId>,
}

impl EncodedTable {
    /// 查找符号在编码表中的列号。
    pub fn column(&self, symbol: &TableColumn) -> Option<usize> {
        self.columns.iter().position(|column| column == symbol)
    }

    /// 查询状态与符号对应的编码值；状态或列不存在时返回错误值 `0`。
    pub fn get(&self, state: usize, symbol: &TableColumn) -> i32 {
        self.column(symbol)
            .and_then(|column| self.rows.get(state).map(|row| row.get(column)))
            .unwrap_or(0)
    }
}
