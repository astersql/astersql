// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Optimizer Hint（优化器提示）的 LALR 解析核心，对照 `hintparser.go` 生成物。
//
// 将 `/*+ ... */` 注释中的 hint token 经 shift/reduce（移进/归约）构造成
// `TableOptimizerHint` AST。大型只读状态表由 `yyhint_tables` 承接；本文件
// 保留索引、偏移、冲突恢复与规约语义动作。

// 解析内存中的 optimizer hint token 并构造 AST。
// 大型只读状态表由 yyhint_tables 模块接口承接；本文件完整保留其索引、偏移、shift/reduce 与错误恢复语义。
// yyhintSymType 对应 goyacc 的语义栈单元；字段顺序与 Go 生成物一致。
/// goyacc 语义栈单元：存放当前 token 的标识、数值、hint AST 与 LEADING 列表等。
#[derive(Default, Clone)]
pub(super) struct yyhintSymType {
    yys: i32,
    offset: i32,
    ident: String,
    number: u64,
    hint: Option<Box<ast::TableOptimizerHint>>,
    hints: Vec<Box<ast::TableOptimizerHint>>,
    table: ast::HintTable,
    modelIdents: Vec<ast::CIStr>,
    leadingList: Option<Box<ast::LeadingList>>,
    // Go interface{} 保存 *HintTable 或 *LeadingList；Rust 用显式 enum 避免运行时类型漂移。
    leadingElement: Option<LeadingElement>,
}

/// LEADING hint 中的表名或嵌套列表元素。
#[derive(Clone)]
enum LeadingElement { Table(ast::HintTable), List(ast::LeadingList) }

impl From<LeadingElement> for ast::LeadingItem {
    fn from(value: LeadingElement) -> Self {
        match value { LeadingElement::Table(table) => Self::Table(table), LeadingElement::List(list) => Self::List(list) }
    }
}

/// 扩展错误表查找键：解析状态 + 期望符号。
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct yyhintXError { state: i32, xsym: i32 }

/// Pure Rust parsergen uses zero as the end-of-input token.
const yyhintEOFCode: i32 = 0;

pub use tokens::*;

// yyhint_tables 对应生成物中的 XLAT、SymNames、Reductions、XErrors 和 ParseTab 静态数据。
// 数据访问保持边界检查，缺失 token 会映射到符号表宽度之外，与 Go 逻辑一致。
/// 生成解析表的只读访问层：XLAT、符号名、归约、动作表与扩展错误文案。
mod yyhint_tables {
    use super::{
        GENERATED_HINT_LEGACY_RULES, GENERATED_HINT_PARSE_TABLE, GENERATED_HINT_REDUCTIONS,
        GENERATED_HINT_SYMBOL_NAMES, GENERATED_HINT_XLAT, yyhintXError,
    };
    /// 将 lexer token 映射为解析表列下标。
    pub fn xlat(token: i32) -> Option<usize> {
        GENERATED_HINT_XLAT
            .binary_search_by_key(&token, |(number, _)| *number)
            .ok()
            .map(|index| GENERATED_HINT_XLAT[index].1)
    }
    /// 按列下标取符号显示名。
    pub fn symbol_name(index: usize) -> &'static str { GENERATED_HINT_SYMBOL_NAMES[index] }
    /// 符号表宽度；未知 token 映射到此宽度之外以触发错误路径。
    pub fn symbol_count() -> usize { GENERATED_HINT_SYMBOL_NAMES.len() }
    /// 按规则号取归约描述。
    pub fn reduction(index: usize) -> (usize, usize, usize) {
        let (symbol, components) = GENERATED_HINT_REDUCTIONS[index];
        (symbol, components, GENERATED_HINT_LEGACY_RULES[index])
    }
    /// 查动作表：正数移进、负数归约、零表示错误/默认。
    pub fn action(state: usize, symbol: usize) -> i32 {
        GENERATED_HINT_PARSE_TABLE
            .get(state)
            .and_then(|row| row.binary_search_by_key(&symbol, |(column, _)| *column).ok().map(|index| row[index].1))
            .unwrap_or(0)
    }
    /// 按 (状态, 符号) 查扩展错误提示。
    pub fn error_message(_key: yyhintXError) -> Option<&'static str> { None }
}

// yyhintLexer 与 Go 接口一一对应；AppendError/AppendWarn 保留诊断累积语义。
/// hint lexer 协议：取词、格式化错误、累积 error/warn，可选 Extended 钩子。
trait yyhintLexer {
    fn Lex(&mut self, lval: &mut yyhintSymType) -> i32;
    fn Errorf(&mut self, format: &str, args: &[&dyn std::fmt::Display]) -> Error;
    fn AppendError(&mut self, error: Error);
    fn AppendWarn(&mut self, warning: Error);
    fn Errors(&self) -> (&[Error], &[Error]);
    fn as_extended(&mut self) -> Option<&mut dyn yyhintLexerEx> { None }
}

/// 扩展 lexer：规约后回调，返回 true 可提前终止解析。
trait yyhintLexerEx: yyhintLexer {
    fn Reduced(&mut self, rule: usize, state: i32, lval: &mut yyhintSymType) -> bool;
}

/// 将 token 编号转为可读符号名，未知则回退为数字字符串。
fn yyhintSymName(token: i32) -> String {
    yyhint_tables::xlat(token).map(|index| yyhint_tables::symbol_name(index).to_owned()).unwrap_or_else(|| token.to_string())
}

// yyhintlex1 把 lexer 的非正 token 统一转换为 EOF，并保留 debug 观测点。
/// 包装 `Lex`：非正 token 归一为 EOF，并在高调试级别打印。
fn yyhintlex1(lexer: &mut dyn yyhintLexer, lval: &mut yyhintSymType) -> i32 {
    let mut token = lexer.Lex(lval);
    if token <= 0 { token = yyhintEOFCode; }
    if yyhintDebug() >= 3 { eprintln!("lex {}({:#x} {}), lval offset: {}", yyhintSymName(token), token, token, lval.offset); }
    token
}

// yyhintParse 是生成 parser 的 shift/reduce 循环。栈扩容、错误恢复、规约 goto 和 Reduced hook 均保留。
/// 生成 parser 的主循环：移进、错误报告、归约后 goto，以及 Reduced hook。
fn yyhintParse(lexer: &mut dyn yyhintLexer, parser: &mut hintParser) -> i32 {
    parser.yylval = yyhintSymType::default();
    let mut states = vec![0i32];
    let mut values = vec![yyhintSymType::default()];
    let mut lookahead = -1i32;
    let mut shifted_state = 0i32;

    loop {
        let state = *states.last().expect("goyacc state stack is never empty");
        if lookahead < 0 {
            parser.yylval = yyhintSymType::default();
            lookahead = yyhintlex1(lexer, &mut parser.yylval);
        }
        let xsym = yyhint_tables::xlat(lookahead).unwrap_or_else(yyhint_tables::symbol_count);
        let action = yyhint_tables::action(state as usize, xsym);

        if action == GENERATED_HINT_ACCEPT {
            parser.cache = values;
            return 0;
        }

        if action > 0 {
            // Go 将 yylval 写入 yyS[yyp+1]，随后把新状态写入同一槽位。
            let mut value = parser.yylval.clone();
            value.yys = action - 1;
            values.push(value);
            states.push(action - 1);
            lookahead = -1;
            shifted_state = action - 1;
            continue;
        }
        if action == 0 {
            let candidates = [
                yyhintXError { state, xsym: xsym as i32 }, yyhintXError { state, xsym: -1 },
                yyhintXError { state: shifted_state, xsym: xsym as i32 }, yyhintXError { state: shifted_state, xsym: -1 },
            ];
            let _message = candidates.into_iter().find_map(yyhint_tables::error_message).unwrap_or("syntax error");
            let error = lexer.Errorf("", &[]);
            lexer.AppendError(error);
            parser.cache = values;
            return 1;
        }

        let generated_reduction = (-action - 1) as usize;
        let (reduction_symbol, reduction_components, rule) =
            yyhint_tables::reduction(generated_reduction);
        let previous_top = values.len() - 1;
        // goyacc points yyVAL at yyS[yyp+1], which preserves the first RHS value
        // for productions without an explicit `$$ = ...` action.
        let mut reduced_value = if reduction_components == 0 {
            yyhintSymType::default()
        } else {
            values[previous_top + 1 - reduction_components].clone()
        };
        if reduce_hint(rule, &values, previous_top, &mut reduced_value, lexer, parser) != 0 {
            parser.cache = values;
            return 1;
        }
        states.truncate(states.len() - reduction_components);
        values.truncate(values.len() - reduction_components);
        let base_state = *states.last().expect("reduction leaves a base state");
        let encoded_goto = yyhint_tables::action(base_state as usize, reduction_symbol);
        if encoded_goto <= 0 {
            let error = lexer.Errorf("invalid parser goto", &[]);
            lexer.AppendError(error);
            parser.cache = values;
            return 1;
        }
        let next_state = encoded_goto - 1;
        if !parser.lexer.scanner.skipPositionRecording { let offset = reduced_value.offset; yyhintSetOffset(&mut reduced_value, offset); }
        reduced_value.yys = next_state;
        if let Some(extended) = lexer.as_extended() {
            if extended.Reduced(rule, state, &mut reduced_value) { parser.cache = values; return -1; }
        }
        states.push(next_state);
        values.push(reduced_value);
    }
}

// reduce_hint 对应生成文件 switch r；这里按语义类别保留全部有动作的规约。
/// 按规则号执行语义动作，填充 `out` 或写入 `parser.result`；返回非 0 表示失败。
fn reduce_hint(rule: usize, stack: &[yyhintSymType], top: usize, out: &mut yyhintSymType, lexer: &mut dyn yyhintLexer, parser: &mut hintParser) -> i32 {
    // `s(back)` 取距栈顶 back 个槽位的语义值，对应 Go 的 yyS[yyp-back]。
    let s = |back: usize| &stack[top - back];
    match rule {
        1 => parser.result = s(0).hints.clone(),
        2 => if let Some(hint) = &s(0).hint { out.hints = vec![hint.clone()]; },
        3 => { out.hints = s(2).hints.clone(); if let Some(hint) = &s(0).hint { out.hints.push(hint.clone()); } }
        4 => out.hints = s(0).hints.clone(),
        5 => { out.hints = s(2).hints.clone(); out.hints.extend(s(0).hints.clone()); }
        // 未支持的 hint 形态：尽量找回大写标识符名，发 warning 且不产出 AST。
        6 | 7 | 8 | 11 | 13 | 26 | 27 | 28 | 29 => {
            let back = if matches!(rule, 13 | 26) { 4 } else if matches!(rule, 28 | 29) { 5 } else { 3 };
            let name = if s(back).ident.is_empty() {
                stack[..=top]
                    .iter()
                    .rev()
                    .find(|value| {
                        !value.ident.is_empty()
                            && value.ident.chars().all(|ch| ch == '_' || ch.is_ascii_uppercase())
                    })
                    .map(|value| value.ident.clone())
                    .unwrap_or_default()
            } else {
                s(back).ident.clone()
            };
            lexer.AppendWarn(ErrWarnOptimizerHintUnsupportedHint.FastGenByArgs(&[name.into()]));
            out.hint = None;
        }
        9 | 12 => {
            let mut h = s(1).hint.clone().unwrap(); h.HintName = ast::NewCIStr(&s(3).ident); out.hint = Some(h);
        }
        10 => {
            let list = s(1).leadingList.clone().unwrap();
            let tables = ast::FlattenLeadingList(&list);
            out.hint = Some(Box::new(ast::TableOptimizerHint { HintName: ast::NewCIStr(&s(4).ident), QBName: ast::NewCIStr(&s(2).ident), HintData: ast::HintData::Leading(*list), Tables: tables, ..Default::default() }));
        }
        14 => out.hint = Some(new_data_hint(&s(4).ident, &s(2).ident, ast::HintData::Unsigned(s(1).number))),
        15 => out.hint = Some(new_data_hint(&s(4).ident, &s(2).ident, ast::HintData::Signed(s(1).number as i64))),
        16 => out.hint = Some(Box::new(ast::TableOptimizerHint { HintName: ast::NewCIStr(&s(5).ident), HintData: ast::HintData::SetVar(ast::HintSetVar { VarName: s(3).ident.clone(), Value: s(1).ident.clone() }), ..Default::default() })),
        17 => out.hint = Some(new_data_hint(&s(3).ident, "", ast::HintData::Name(s(1).ident.clone()))),
        18 | 23 => out.hint = Some(new_data_hint(&s(3).ident, &s(1).ident, ast::HintData::None)),
        19 => { let mut h = new_data_hint(&s(5).ident, &s(3).ident, ast::HintData::None); h.Tables = s(1).hint.as_ref().unwrap().Tables.clone(); out.hint = Some(h); }
        20 => {
            let unit = s(1).number;
            if s(2).number <= i64::MAX as u64 / unit {
                out.hint = Some(new_data_hint(&s(5).ident, &s(3).ident, ast::HintData::Signed((s(2).number * unit) as i64)));
            } else {
                lexer.AppendError(ErrWarnMemoryQuotaOverflow.GenWithStackByArgs(&[i64::MAX.into()]));
                parser.lastErrorAsWarn(); out.hint = None;
            }
        }
        21 => out.hint = Some(new_data_hint(&s(5).ident, "", ast::HintData::TimeRange(ast::HintTimeRange { From: s(3).ident.clone(), To: s(1).ident.clone() }))),
        22 => { let mut h = s(1).hint.clone().unwrap(); h.HintName = ast::NewCIStr(&s(4).ident); h.QBName = ast::NewCIStr(&s(2).ident); out.hint = Some(h); }
        24 => out.hint = Some(new_data_hint(&s(0).ident, "", ast::HintData::None)),
        25 => out.hint = Some(new_data_hint(&s(4).ident, &s(2).ident, ast::HintData::CIStr(ast::NewCIStr(&s(1).ident)))),
        30 => { out.hints = s(1).hints.clone(); for h in &mut out.hints { h.HintName = ast::NewCIStr(&s(4).ident); h.QBName = ast::NewCIStr(&s(2).ident); } }
        31 => out.hints = s(0).hint.clone().into_iter().collect(),
        32 => { out.hints = s(2).hints.clone(); out.hints.extend(s(0).hint.clone()); }
        33 => { let mut h = s(1).hint.clone().unwrap(); h.HintData = ast::HintData::CIStr(ast::NewCIStr(&s(3).ident)); out.hint = Some(h); }
        34 => out.leadingList = Some(Box::new(ast::LeadingList { Items: vec![s(0).leadingElement.clone().unwrap().into()] })),
        35 => { let mut leading_items = s(2).leadingList.clone().unwrap(); leading_items.Items.push(s(0).leadingElement.clone().unwrap().into()); out.leadingList = Some(leading_items); }
        36 => out.leadingElement = Some(LeadingElement::Table(s(0).table.clone())),
        37 => out.leadingElement = Some(LeadingElement::List(*s(1).leadingList.clone().unwrap())),
        38 => out.ident.clear(),
        42 => out.modelIdents.clear(),
        43 => out.modelIdents = s(1).modelIdents.clone(),
        44 => out.modelIdents = vec![ast::NewCIStr(&s(0).ident)],
        45 => { out.modelIdents = s(2).modelIdents.clone(); out.modelIdents.push(ast::NewCIStr(&s(0).ident)); }
        47 => out.hint = Some(Box::new(ast::TableOptimizerHint { QBName: ast::NewCIStr(&s(0).ident), ..Default::default() })),
        48 | 49 | 52 | 53 | 56 | 57 | 59 | 60 => reduce_table_and_index(rule, s, out),
        50 => out.table = ast::HintTable { TableName: ast::NewCIStr(&s(2).ident), QBName: ast::NewCIStr(&s(1).ident), PartitionList: s(0).modelIdents.clone(), ..Default::default() },
        51 => out.table = ast::HintTable { DBName: ast::NewCIStr(&s(4).ident), TableName: ast::NewCIStr(&s(2).ident), QBName: ast::NewCIStr(&s(1).ident), PartitionList: s(0).modelIdents.clone() },
        54 => out.table = ast::HintTable { TableName: ast::NewCIStr(&s(1).ident), QBName: ast::NewCIStr(&s(0).ident), ..Default::default() },
        55 => out.table = ast::HintTable { QBName: ast::NewCIStr(&s(0).ident), ..Default::default() },
        68 | 71 => out.ident = s(0).number.to_string(),
        69 => out.ident = s(0).ident.clone(),
        70 => out.ident = format!("-{}", s(0).ident),
        72 => {
            if s(0).number > 9_223_372_036_854_775_808 {
                let error = lexer.Errorf("the Signed Value should be at the range of [-9223372036854775808, 9223372036854775807].", &[]);
                lexer.AppendError(error);
                return 1;
            }
            out.ident = if s(0).number == 9_223_372_036_854_775_808 { i64::MIN.to_string() } else { (-(s(0).number as i64)).to_string() };
        }
        73 => out.number = 1024 * 1024,
        74 => out.number = 1024 * 1024 * 1024,
        75 => out.hint = Some(new_data_hint("", "", ast::HintData::Boolean(true))),
        76 => out.hint = Some(new_data_hint("", "", ast::HintData::Boolean(false))),
        _ => {}
    }
    0
}

/// 构造带 HintName/QBName/HintData 的 `TableOptimizerHint` 盒子。
fn new_data_hint(name: &str, qb: &str, data: ast::HintData) -> Box<ast::TableOptimizerHint> {
    Box::new(ast::TableOptimizerHint { HintName: ast::NewCIStr(name), QBName: ast::NewCIStr(qb), HintData: data, ..Default::default() })
}

// 表/索引规约共享追加逻辑，分别对应 Go case 48、49、52、53、56、57、59、60。
/// 表列表与索引列表相关规约的共享追加逻辑。
fn reduce_table_and_index<'a>(rule: usize, s: impl Fn(usize) -> &'a yyhintSymType, out: &mut yyhintSymType) {
    match rule {
        48 => out.hint = Some(Box::new(ast::TableOptimizerHint { Tables: vec![s(0).table.clone()], QBName: ast::NewCIStr(&s(1).ident), ..Default::default() })),
        49 | 52 => { let mut h = s(2).hint.clone().unwrap(); h.Tables.push(s(0).table.clone()); out.hint = Some(h); }
        53 => out.hint = Some(Box::new(ast::TableOptimizerHint { Tables: vec![s(0).table.clone()], ..Default::default() })),
        56 => { let mut h = s(0).hint.clone().unwrap(); h.Tables = vec![s(2).table.clone()]; h.QBName = ast::NewCIStr(&s(3).ident); out.hint = Some(h); }
        57 => out.hint = Some(Box::new(ast::TableOptimizerHint::default())),
        59 => out.hint = Some(Box::new(ast::TableOptimizerHint { Indexes: vec![ast::NewCIStr(&s(0).ident)], ..Default::default() })),
        60 => { let mut h = s(2).hint.clone().unwrap(); h.Indexes.push(ast::NewCIStr(&s(0).ident)); out.hint = Some(h); }
        _ => unreachable!(),
    }
}

/// 调试级别；当前固定为 0，与 Go 侧默认关闭 verbose 一致。
fn yyhintDebug() -> i32 { 0 }
