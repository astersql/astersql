// Copyright 2026 AsterSQL.

//! 确定性地计算可空属性与 FIRST 集，并构造 LALR(1) 自动机。
//!
//! 构造过程先生成 LR(0) 的闭包/GOTO 图，再沿项目和状态转换传播向前看符号直至
//! 不动点。这样可直接合并具有相同核心的项目，而无须物化规模更大的规范 LR(1) 图。

use std::collections::{BTreeMap, BTreeSet};

use crate::{Grammar, ProductionItem, RuleId};

/// 文法终结符；字面量引用在规范化时统一转换为对应的词法单元名称。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Terminal {
    /// 解析器内部使用的输入结束标记。
    End,
    /// 由 `%token` 声明的终结符。
    Token(String),
}

impl Terminal {
    /// 返回稳定且便于展示的终结符名称。
    pub fn name(&self) -> &str {
        match self {
            Self::End => "$end",
            Self::Token(name) => name,
        }
    }
}

/// 自动机状态转换使用的规范化文法符号。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Symbol {
    Terminal(Terminal),
    Nonterminal(String),
}

/// 各文法非终结符的可空属性与 FIRST 集计算结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstSets {
    nullable: BTreeSet<String>,
    terminals: BTreeMap<String, BTreeSet<Terminal>>,
    empty: BTreeSet<Terminal>,
}

impl FirstSets {
    /// 判断 `nonterminal` 是否能推导出空串。
    pub fn is_nullable(&self, nonterminal: &str) -> bool {
        self.nullable.contains(nonterminal)
    }

    /// 返回 `nonterminal` 的 FIRST 终结符集合；名称未知时返回空集。
    pub fn terminals(&self, nonterminal: &str) -> &BTreeSet<Terminal> {
        self.terminals.get(nonterminal).unwrap_or(&self.empty)
    }

    fn compute(grammar: &NormalizedGrammar) -> Self {
        let mut result = Self {
            nullable: BTreeSet::new(),
            terminals: grammar
                .productions_by_lhs
                .keys()
                .cloned()
                .map(|name| (name, BTreeSet::new()))
                .collect(),
            empty: BTreeSet::new(),
        };

        // 反复扫描产生式，直到可空属性和 FIRST 集都不再增长。
        loop {
            let mut changed = false;
            for production in grammar.productions.iter().skip(1) {
                let mut rhs_nullable = true;
                let mut discovered = BTreeSet::new();
                for symbol in &production.rhs {
                    match symbol {
                        Symbol::Terminal(terminal) => {
                            discovered.insert(terminal.clone());
                            rhs_nullable = false;
                            break;
                        }
                        Symbol::Nonterminal(nonterminal) => {
                            discovered.extend(
                                result
                                    .terminals
                                    .get(nonterminal)
                                    .into_iter()
                                    .flatten()
                                    .cloned(),
                            );
                            if !result.nullable.contains(nonterminal) {
                                rhs_nullable = false;
                                break;
                            }
                        }
                    }
                }

                let lhs_first = result
                    .terminals
                    .get_mut(&production.lhs)
                    .expect("every production lhs is a nonterminal");
                let old_len = lhs_first.len();
                lhs_first.extend(discovered);
                changed |= lhs_first.len() != old_len;
                if rhs_nullable {
                    changed |= result.nullable.insert(production.lhs.clone());
                }
            }
            if !changed {
                return result;
            }
        }
    }

    fn sequence_with_lookahead(
        &self,
        symbols: &[Symbol],
        lookaheads: &BTreeSet<Terminal>,
    ) -> BTreeSet<Terminal> {
        // 依次合并可空前缀的 FIRST 集；整段均可空时才继承原向前看集合。
        let mut result = BTreeSet::new();
        for symbol in symbols {
            match symbol {
                Symbol::Terminal(terminal) => {
                    result.insert(terminal.clone());
                    return result;
                }
                Symbol::Nonterminal(nonterminal) => {
                    result.extend(self.terminals(nonterminal).iter().cloned());
                    if !self.is_nullable(nonterminal) {
                        return result;
                    }
                }
            }
        }
        result.extend(lookaheads.iter().cloned());
        result
    }
}

/// LR 项目引用的文法规则。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemRule {
    AugmentedStart,
    Production(RuleId),
}

/// 一个 LALR 项目；属于同一 LR(0) 核心的向前看符号会合并到同一集合。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub rule: ItemRule,
    pub dot: usize,
    pub lookaheads: BTreeSet<Terminal>,
}

/// 一个编号稳定的 LALR 状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub id: usize,
    pub items: Vec<Item>,
    pub transitions: BTreeMap<Symbol, usize>,
}

/// 一份文法对应的完整 LALR 项目图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Automaton {
    pub first: FirstSets,
    pub states: Vec<State>,
}

impl Automaton {
    /// 根据已经校验的文法构造确定性的 LALR(1) 自动机。
    pub fn build(grammar: &Grammar) -> Self {
        let normalized = NormalizedGrammar::new(grammar);
        let first = FirstSets::compute(&normalized);
        let lr0 = Lr0Automaton::build(&normalized);
        let states = lr0.propagate_lookaheads(&normalized, &first);
        Self { first, states }
    }
}

#[derive(Clone, Debug)]
struct AutomatonProduction {
    rule: ItemRule,
    lhs: String,
    rhs: Vec<Symbol>,
}

struct NormalizedGrammar {
    productions: Vec<AutomatonProduction>,
    productions_by_lhs: BTreeMap<String, Vec<usize>>,
}

impl NormalizedGrammar {
    fn new(grammar: &Grammar) -> Self {
        let token_names: BTreeSet<_> = grammar
            .tokens
            .iter()
            .map(|token| token.name.clone())
            .collect();
        let literal_names: BTreeMap<_, _> = grammar
            .tokens
            .iter()
            .filter_map(|token| {
                token
                    .literal
                    .as_ref()
                    .map(|literal| (literal.clone(), token.name.clone()))
            })
            .collect();

        // 在用户产生式前加入增广开始规则；语义动作不消耗输入，因此不进入右部符号序列。
        let mut productions = vec![AutomatonProduction {
            rule: ItemRule::AugmentedStart,
            lhs: "$accept".to_owned(),
            rhs: vec![Symbol::Nonterminal(grammar.start.clone())],
        }];
        productions.extend(grammar.productions.iter().map(|production| {
            let rhs = production
                .rhs
                .iter()
                .filter_map(|item| match item {
                    ProductionItem::Symbol(name) if token_names.contains(name) => {
                        Some(Symbol::Terminal(Terminal::Token(name.clone())))
                    }
                    ProductionItem::Symbol(name) => Some(Symbol::Nonterminal(name.clone())),
                    ProductionItem::Literal(literal) => Some(Symbol::Terminal(Terminal::Token(
                        literal_names
                            .get(literal)
                            .expect("grammar validation resolves token literals")
                            .clone(),
                    ))),
                    ProductionItem::Action { .. } => None,
                })
                .collect();
            AutomatonProduction {
                rule: ItemRule::Production(production.rule_id.clone()),
                lhs: production.lhs.clone(),
                rhs,
            }
        }));

        let mut productions_by_lhs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, production) in productions.iter().enumerate().skip(1) {
            productions_by_lhs
                .entry(production.lhs.clone())
                .or_default()
                .push(index);
        }
        Self {
            productions,
            productions_by_lhs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ItemCore {
    production: usize,
    dot: usize,
}

type CoreSet = BTreeSet<ItemCore>;
type LookaheadSets = BTreeMap<ItemCore, BTreeSet<Terminal>>;

struct Lr0State {
    cores: CoreSet,
    transitions: BTreeMap<Symbol, usize>,
}

struct Lr0Automaton {
    states: Vec<Lr0State>,
}

impl Lr0Automaton {
    fn build(grammar: &NormalizedGrammar) -> Self {
        let initial = lr0_closure(
            BTreeSet::from([ItemCore {
                production: 0,
                dot: 0,
            }]),
            grammar,
        );
        // 按有序符号遍历并复用相同核心集合，使状态编号和输出可重复。
        let mut state_ids = BTreeMap::from([(initial.clone(), 0)]);
        let mut states = vec![Lr0State {
            cores: initial,
            transitions: BTreeMap::new(),
        }];

        let mut current = 0;
        while current < states.len() {
            let symbols: BTreeSet<_> = states[current]
                .cores
                .iter()
                .filter_map(|core| symbol_after_dot(*core, grammar).cloned())
                .collect();
            for symbol in symbols {
                let target = lr0_goto(&states[current].cores, &symbol, grammar);
                let target_id = if let Some(id) = state_ids.get(&target) {
                    *id
                } else {
                    let id = states.len();
                    state_ids.insert(target.clone(), id);
                    states.push(Lr0State {
                        cores: target,
                        transitions: BTreeMap::new(),
                    });
                    id
                };
                states[current].transitions.insert(symbol, target_id);
            }
            current += 1;
        }
        Self { states }
    }

    fn propagate_lookaheads(self, grammar: &NormalizedGrammar, first: &FirstSets) -> Vec<State> {
        let mut items: Vec<LookaheadSets> = self
            .states
            .iter()
            .map(|state| {
                state
                    .cores
                    .iter()
                    .copied()
                    .map(|core| (core, BTreeSet::new()))
                    .collect()
            })
            .collect();
        items[0]
            .get_mut(&ItemCore {
                production: 0,
                dot: 0,
            })
            .expect("initial state contains the augmented start item")
            .insert(Terminal::End);

        // 同时处理闭包内的向前看传播和跨 GOTO 边的移进传播，直到所有集合稳定。
        loop {
            let mut changed = false;
            for (state_id, state) in self.states.iter().enumerate() {
                let snapshot = items[state_id].clone();
                for (core, lookaheads) in &snapshot {
                    let Some(symbol) = symbol_after_dot(*core, grammar) else {
                        continue;
                    };
                    if let Symbol::Nonterminal(nonterminal) = symbol {
                        let production = &grammar.productions[core.production];
                        let propagated = first
                            .sequence_with_lookahead(&production.rhs[core.dot + 1..], lookaheads);
                        for production in grammar
                            .productions_by_lhs
                            .get(nonterminal)
                            .into_iter()
                            .flatten()
                        {
                            changed |= extend(
                                items[state_id]
                                    .get_mut(&ItemCore {
                                        production: *production,
                                        dot: 0,
                                    })
                                    .expect("LR(0) closure contains nonterminal productions"),
                                &propagated,
                            );
                        }
                    }

                    let target_state = state.transitions[symbol];
                    changed |= extend(
                        items[target_state]
                            .get_mut(&ItemCore {
                                production: core.production,
                                dot: core.dot + 1,
                            })
                            .expect("LR(0) goto contains the shifted item"),
                        lookaheads,
                    );
                }
            }
            if !changed {
                break;
            }
        }

        items
            .into_iter()
            .zip(self.states)
            .enumerate()
            .map(|(id, (items, lr0_state))| State {
                id,
                items: items
                    .into_iter()
                    .map(|(core, lookaheads)| Item {
                        rule: grammar.productions[core.production].rule.clone(),
                        dot: core.dot,
                        lookaheads,
                    })
                    .collect(),
                transitions: lr0_state.transitions,
            })
            .collect()
    }
}

fn lr0_closure(mut cores: CoreSet, grammar: &NormalizedGrammar) -> CoreSet {
    // 点后是非终结符时，将其每条产生式的起始项目加入闭包。
    loop {
        let mut changed = false;
        for core in cores.iter().copied().collect::<Vec<_>>() {
            let Some(Symbol::Nonterminal(nonterminal)) = symbol_after_dot(core, grammar) else {
                continue;
            };
            for production in grammar
                .productions_by_lhs
                .get(nonterminal)
                .into_iter()
                .flatten()
            {
                changed |= cores.insert(ItemCore {
                    production: *production,
                    dot: 0,
                });
            }
        }
        if !changed {
            return cores;
        }
    }
}

fn lr0_goto(cores: &CoreSet, symbol: &Symbol, grammar: &NormalizedGrammar) -> CoreSet {
    // 先把匹配符号的项目圆点右移一位，再对所得内核求闭包。
    let mut kernel = CoreSet::new();
    for core in cores {
        if symbol_after_dot(*core, grammar) == Some(symbol) {
            kernel.insert(ItemCore {
                production: core.production,
                dot: core.dot + 1,
            });
        }
    }
    lr0_closure(kernel, grammar)
}

fn extend(target: &mut BTreeSet<Terminal>, values: &BTreeSet<Terminal>) -> bool {
    let old_len = target.len();
    target.extend(values.iter().cloned());
    target.len() != old_len
}

fn symbol_after_dot(core: ItemCore, grammar: &NormalizedGrammar) -> Option<&Symbol> {
    grammar.productions[core.production].rhs.get(core.dot)
}
