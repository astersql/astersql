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

// Optimizer Hint 词法扫描与公开解析入口，对照 `hintparserimpl.go`。
//
// 在通用 `Scanner` 之上映射 hint 文法 token，维护 `SET_VAR` 值上下文，
// 并暴露 `ParseHint` 解析 `/*+ ... */` 注释中的提示列表与诊断信息。

// 这里只扫描传入字符串并构造 hint AST。
// 与生成状态机 yyhintParse、mysql SQLMode、terror 错误类的跨文件引用保留原调用形状。
// 以下错误值与 Go 的 parser terror.ClassParser 标准错误一一对应。
/// 不支持的 hint 名告警（对应 mysql.ErrWarnOptimizerHintUnsupportedHint）。
static ErrWarnOptimizerHintUnsupportedHint: std::sync::LazyLock<Box<terror::Error>> = std::sync::LazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWarnOptimizerHintUnsupportedHint as isize)));
/// 非法 token 告警。
static ErrWarnOptimizerHintInvalidToken: std::sync::LazyLock<Box<terror::Error>> = std::sync::LazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWarnOptimizerHintInvalidToken as isize)));
/// 内存配额溢出告警。
static ErrWarnMemoryQuotaOverflow: std::sync::LazyLock<Box<terror::Error>> = std::sync::LazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWarnMemoryQuotaOverflow as isize)));
/// hint 语法解析错误告警。
static ErrWarnOptimizerHintParseError: std::sync::LazyLock<Box<terror::Error>> = std::sync::LazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWarnOptimizerHintParseError as isize)));
/// 整型字面量非法告警。
static ErrWarnOptimizerHintInvalidInteger: std::sync::LazyLock<Box<terror::Error>> = std::sync::LazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWarnOptimizerHintInvalidInteger as isize)));
/// hint 位置非法告警。
static ErrWarnOptimizerHintWrongPos: std::sync::LazyLock<Box<terror::Error>> = std::sync::LazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWarnOptimizerHintWrongPos as isize)));

// hintScanner 嵌入通用 Scanner，并额外记录 SET_VAR 值的词法上下文。
/// 嵌入通用 Scanner，并跟踪 SET_VAR(name = value) 的词法状态。
#[derive(Default)]
struct hintScanner {
    scanner: Scanner,
    setVarValueState: hintSetVarValueState,
}

// SET_VAR(name = value) 允许 decimal/float；其他 hint（如 QB_NAME(1.5)）仍应拒绝这些 token。
/// SET_VAR 值位置的有限状态机；仅 ExpectValue/AfterSign 时接受小数。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum hintSetVarValueState {
    #[default]
    None,
    AfterSetVar,
    AfterLParen,
    AfterName,
    ExpectValue,
    AfterSign,
}

impl hintScanner {
    // Errorf 先让通用 Scanner 附加位置信息，再包装成 optimizer hint 语法错误。
    /// 附加源位置后包装为 Optimizer hint syntax error。
    fn Errorf(&mut self, format: &str, args: &[&dyn std::fmt::Display]) -> Error {
        let rendered = if args.is_empty() { format.to_owned() } else {
            format!("{}: {}", format, args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>().join(", "))
        };
        let inner = self.scanner.Errorf(&rendered);
        ErrParse.GenWithStackByArgs(&["Optimizer hint syntax error at".into(), inner.to_string().into()])
    }

    /// 当前是否处于可接受 SET_VAR 数值字面量的状态。
    fn acceptSetVarNumericValue(&self) -> bool {
        matches!(self.setVarValueState, hintSetVarValueState::ExpectValue | hintSetVarValueState::AfterSign)
    }

    // updateSetVarValueState 对应 Go 状态机；任何不符合 SET_VAR 形状的 token 都立即清空上下文。
    /// 按刚产出的 token 推进或重置 SET_VAR 状态机。
    fn updateSetVarValueState(&mut self, token: i32) {
        use hintSetVarValueState::*;
        self.setVarValueState = match self.setVarValueState {
            None if token == hintSetVar => AfterSetVar,
            None => None,
            AfterSetVar if token == '(' as i32 => AfterLParen,
            AfterSetVar if token == hintSetVar => AfterSetVar,
            AfterSetVar => None,
            AfterLParen if token == ')' as i32 || token == ',' as i32 || token == '=' as i32 || token <= 0 => None,
            AfterLParen => AfterName,
            AfterName if token == '=' as i32 => ExpectValue,
            AfterName => None,
            ExpectValue if token == '+' as i32 || token == '-' as i32 => AfterSign,
            ExpectValue | AfterSign => None,
        };
    }

    // returnToken 集中更新 SET_VAR 上下文，确保 Lex 的每条返回路径状态一致。
    /// 返回 token 前统一刷新 SET_VAR 上下文。
    fn returnToken(&mut self, token: i32) -> i32 {
        self.updateSetVarValueState(token);
        token
    }

    // Lex 将通用 lexer token 映射成 hint grammar token，并填充对应语义值。
    /// 扫描下一 token，映射为 hint 文法终结符并写入 `lval`。
    fn Lex(&mut self, lval: &mut yyhintSymType) -> i32 {
        let (token, position, literal) = self.scanner.scan();
        self.scanner.lastScanOffset = position.Offset;
        let error_token_type: &str;

        match token {
            token::intLit => match literal.parse::<u64>() {
                Ok(number) => { lval.number = number; return self.returnToken(hintIntLit); }
                Err(_) => {
                    self.scanner.AppendError(ErrWarnOptimizerHintInvalidInteger.GenWithStackByArgs(&[literal.into()]));
                    return self.returnToken(hintInvalid);
                }
            },
            token::singleAtIdentifier => { lval.ident = literal; return self.returnToken(hintSingleAtIdentifier); }
            token::identifier => {
                lval.ident = literal.clone();
                // 大写后查 hintTokenMap，命中则返回专用 hint 关键字 token。
                if let Some((_, mapped)) = hintTokenMap.iter().find(|(name, _)| *name == literal.to_uppercase()) { return self.returnToken(*mapped); }
                return self.returnToken(hintIdentifier);
            }
            token::stringLit => {
                lval.ident = literal;
                // ANSI_QUOTES 下双引号表示 identifier；读取源字节只用于判别引号类型。
                if self.scanner.sqlMode.HasANSIQuotesMode() && self.scanner.r.s.as_bytes().get(position.Offset as usize) == Some(&b'"') {
                    return self.returnToken(hintIdentifier);
                }
                return self.returnToken(hintStringLit);
            }
            token::bitLit if literal.starts_with("0b") => { lval.ident = literal; return self.returnToken(hintIdentifier); }
            token::bitLit => error_token_type = "bit-value literal",
            token::hexLit if literal.starts_with("0x") => { lval.ident = literal; return self.returnToken(hintIdentifier); }
            token::hexLit => error_token_type = "hexadecimal literal",
            quotedIdentifier => { lval.ident = literal; return self.returnToken(hintIdentifier); }
            token::eq => return self.returnToken('=' as i32),
            token::floatLit | token::decLit if self.acceptSetVarNumericValue() => {
                lval.ident = literal;
                return self.returnToken(hintNumericLit);
            }
            token::floatLit => error_token_type = "floating point number",
            token::decLit => error_token_type = "decimal number",
            _ if token <= 0x7f => return self.returnToken(token),
            _ => error_token_type = "unknown token",
        }

        // 非法 token 同时记录人类可读分类、原文本和底层 token 编号，再交给 grammar 继续恢复。
        self.scanner.AppendError(ErrWarnOptimizerHintInvalidToken.GenWithStackByArgs(&[
            error_token_type.into(), literal.into(), token.into(),
        ]));
        self.returnToken(hintInvalid)
    }
}

impl yyhintLexer for hintScanner {
    fn Lex(&mut self, lval: &mut yyhintSymType) -> i32 { hintScanner::Lex(self, lval) }
    fn Errorf(&mut self, format: &str, args: &[&dyn std::fmt::Display]) -> Error { hintScanner::Errorf(self, format, args) }
    fn AppendError(&mut self, error: Error) { self.scanner.AppendError(error); }
    fn AppendWarn(&mut self, warning: Error) { self.scanner.AppendWarn(warning); }
    fn Errors(&self) -> (&[Error], &[Error]) { self.scanner.Errors() }
}

// hintParser 保存 lexer、最终结果和可复用的 goyacc 语义栈，减少重复解析分配。
/// 持有 lexer、解析结果与可复用语义栈的 hint 解析器。
pub(super) struct hintParser {
    lexer: hintScanner,
    result: Vec<Box<ast::TableOptimizerHint>>,
    cache: Vec<yyhintSymType>,
    yylval: yyhintSymType,
    yyVAL: Option<usize>,
}

/// 构造预分配语义栈容量约为 50 的 hintParser。
pub(super) fn newHintParser() -> hintParser {
    hintParser {
        lexer: hintScanner::default(), result: Vec::new(),
        cache: vec![yyhintSymType::default(); 50], yylval: yyhintSymType::default(), yyVAL: None,
    }
}

impl hintParser {
    // parse 跳过开头 /*+ 三字节，重置 scanner/SQL mode/位置，再调用生成 parser。
    /// 解析 hint 正文：跳过 `/*+`，设置 SQLMode 与位置，调用 `yyhintParse`。
    pub(super) fn parse(&mut self, input: &str, sqlMode: mysql::SQLMode, initPos: Pos) -> (Vec<Box<ast::TableOptimizerHint>>, Vec<Error>) {
        self.result.clear();
        self.lexer.scanner.reset(input[3..].to_owned());
        self.lexer.setVarValueState = hintSetVarValueState::None;
        self.lexer.scanner.SetSQLMode(sqlMode);
        self.lexer.scanner.r.updatePos(Pos {
            Line: initPos.Line,
            // 已跳过最初的 /*+，列号同步前移 3；offset 对切片后的输入重新从 0 计数。
            Col: initPos.Col + 3,
            Offset: 0,
        });
        // 保留结尾 */ 供 warning 定位，但扫描时按 bang comment 规则跳过。
        self.lexer.scanner.inBangComment = true;
        let mut lexer = std::mem::take(&mut self.lexer);
        yyhintParse(&mut lexer, self);
        self.lexer = lexer;

        // 有 error 时只返回 errors；否则返回 warnings，与 Go 诊断优先级一致。
        let (warnings, errors) = self.lexer.scanner.Errors();
        let diagnostics = if errors.is_empty() { warnings.to_vec() } else { errors.to_vec() };
        (std::mem::take(&mut self.result), diagnostics)
    }

    /// 将不支持的 hint 名追加为 warning。
    fn warnUnsupportedHint(&mut self, name: String) {
        let warning = ErrWarnOptimizerHintUnsupportedHint.FastGenByArgs(&[name.into()]);
        self.lexer.scanner.warns.push(warning);
    }

    // lastErrorAsWarn 用于 memory quota overflow：把刚追加的错误降级为 warning。
    /// 把最近一次错误降级为 warning（如 MEMORY_QUOTA 溢出）。
    fn lastErrorAsWarn(&mut self) { self.lexer.scanner.lastErrorAsWarn(); }
}

// ParseHint 是公开入口，解析 `/*+ ... */` optimizer hint 并返回 AST 与诊断。
/// 公开入口：解析 `/*+ ... */` optimizer hint，返回 AST 列表与诊断。
pub fn ParseHint(input: &str, sqlMode: mysql::SQLMode, initPos: Pos) -> (Vec<Box<ast::TableOptimizerHint>>, Vec<Error>) {
    newHintParser().parse(input, sqlMode, initPos)
}
