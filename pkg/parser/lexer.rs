// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// SQL 词法扫描器（Lexer/Scanner），对齐 Go `pkg/parser/lexer.go`。
//
// 将输入 SQL 切分为 token，处理字面量、注释、优化器 Hint、版本注释（`/*!...*/`）
// 与 TiDB 特性注释（`/*T![...]`），以及客户端/连接字符集转换。
// `Pos` 记录行、列与字节偏移，供语法错误定位。

// 本文件对齐 pkg/parser/lexer.go，按原顺序保留 SQL 词法扫描器的状态与分支。
// 实现处理字符串分词、编码转换和错误收集。
// Pos 对应 Go 的 token 位置，Offset 是 SQL 字节偏移，Line/Col 用于报错。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Token 在源 SQL 中的位置：行号、列号与字节偏移。
pub struct Pos {
    pub Line: i32,
    pub Col: i32,
    pub Offset: i32,
}

// Scanner 对应 yyLexer 实现；字段顺序与 Go 一致，保存编码、诊断、提示和最近关键字状态。
/// yyLexer 实现：持有 reader、编码、诊断、SQL Mode 与关键字历史。
pub struct Scanner {
    r: reader,
    buf: Vec<u8>,
    pub(super) client: charset::encoding::EncodingRef,
    pub(super) connection: charset::encoding::EncodingRef,
    errs: Vec<errors::Error>,
    warns: Vec<errors::Error>,
    stmtStartPos: i32,
    // 位于 /*! ... */ 内时忽略孤立的结束符。
    inBangComment: bool,
    pub(super) sqlMode: mysql::SQLMode,
    // 窗口函数关键字会改变部分旧 SQL 的标识符解释，因此由调用方显式开启。
    supportWindowFunc: bool,
    pub(super) skipPositionRecording: bool,
    lastScanOffset: i32,
    // 三层关键字历史用于区分 FOR UPDATE 与 CREATE BINDING FOR UPDATE 后的 hint。
    lastKeyword: i32,
    lastKeyword2: i32,
    lastKeyword3: i32,
    pub(super) lastHintPos: Pos,
    identifierDot: bool,
    keepHint: bool,
}

impl Scanner {
    // Errors 沿用 Go 返回顺序：先 warning，后 error。
    /// 返回 (warnings, errors)，顺序与 Go 一致。
    pub fn Errors(&self) -> (&[errors::Error], &[errors::Error]) {
        (&self.warns, &self.errs)
    }

    // reset 为新 SQL 重置 reader 与瞬时诊断，但保留调用方配置的 SQL mode 等开关。
    /// 为新 SQL 重置扫描状态，保留已配置的 SQL Mode 等开关。
    pub(super) fn reset(&mut self, sql: String) {
        self.client = charset::encoding::FindEncoding(mysql::DefaultCharset);
        self.connection = charset::encoding::FindEncoding(mysql::DefaultCharset);
        self.r = reader::new(sql);
        self.buf.clear();
        self.errs.clear();
        self.warns.clear();
        self.stmtStartPos = 0;
        self.inBangComment = false;
        self.lastKeyword = 0;
        self.identifierDot = false;
    }

    // stmtText 截取当前语句，并按 Go 逻辑只修剪边界上的一个换行。
    /// 截取当前语句文本，并按 Go 逻辑修剪边界换行。
    fn stmtText(&mut self) -> String {
        let mut statement_end = self.r.pos().Offset as usize;
        if statement_end > 0 && self.r.s.as_bytes()[statement_end - 1] == b'\n' {
            statement_end -= 1;
        }
        if (self.stmtStartPos as usize) < self.r.s.len()
            && self.r.s.as_bytes()[self.stmtStartPos as usize] == b'\n'
        {
            self.stmtStartPos += 1;
        }
        let text = self.r.s[self.stmtStartPos as usize..statement_end].to_owned();
        self.stmtStartPos = statement_end as i32;
        text
    }

    // Errorf 对应 yyLexer 的格式化错误；附近 SQL 最多保留 2048 字节，避免错误消息失控。
    /// 构造带附近源码片段的格式化解析错误。
    pub fn Errorf(&self, detail: &str) -> errors::Error {
        let rest = &self.r.s[self.lastScanOffset as usize..];
        let (near, length) = if rest.len() > 2048 {
            (&rest[..2048], format!("(total length {})", rest.len()))
        } else {
            (rest, String::new())
        };
        errors::Errorf(
            "line %d column %d near \"%s\"%s %s",
            &[
                self.r.p.Line.into(),
                self.r.p.Col.into(),
                near.into(),
                detail.into(),
                length.into(),
            ],
        )
    }

    // AppendError/AppendWarn 忽略 Go nil；由 Option 显式表达这一点。
    /// 追加 error；忽略 None（对应 Go nil）。
    pub fn AppendError(&mut self, err: impl Into<Option<errors::Error>>) {
        if let Some(err) = err.into() {
            self.errs.push(err);
        }
    }
    /// 追加 warning；忽略 None。
    pub fn AppendWarn(&mut self, err: impl Into<Option<errors::Error>>) {
        if let Some(err) = err.into() {
            self.warns.push(err);
        }
    }

    // convert2System 将客户端编码解码为系统 utf8mb4；替换型解码错误仅记 warning。
    /// 将客户端编码解码为系统 utf8mb4；替换型错误仅记 warning。
    fn convert2System(&mut self, tok: i32, lit: String) -> (i32, String) {
        let mut destination = Vec::with_capacity(lit.len());
        let utf8 = match self.client.Transform(
            &mut destination,
            lit.as_bytes(),
            charset::encoding::OpDecodeReplace,
        ) {
            Ok(value) => value,
            Err(error) => {
                let output = error.output().to_vec();
                self.AppendWarn(errors::New(error.to_string()));
                output
            }
        };
        (tok, String::from_utf8_lossy(&utf8).into_owned())
    }

    // convert2Connection 先解码再按连接编码回写；严格模式且编码相同的错误返回 invalid。
    /// 解码后再按连接编码回写；严格模式下同源编码错误可返回 invalid。
    fn convert2Connection(&mut self, tok: i32, lit: String) -> (i32, String) {
        if mysql::IsUTF8Charset(self.client.Name()) {
            return (tok, lit);
        }
        let mut destination = Vec::with_capacity(lit.len());
        let mut utf8 = match self.client.Transform(
            &mut destination,
            lit.as_bytes(),
            charset::encoding::OpDecodeReplace,
        ) {
            Ok(value) => value,
            Err(error) => {
                let output = error.output().to_vec();
                self.AppendError(errors::New(error.to_string()));
                if self.sqlMode.HasStrictMode() && self.client.Tp() == self.connection.Tp() {
                    return (invalid, lit);
                }
                self.lastErrorAsWarn();
                output
            }
        };
        if self.client.Tp() != self.connection.Tp() {
            let mut converted = Vec::with_capacity(utf8.len());
            utf8 = self
                .connection
                .Transform(&mut converted, &utf8, charset::encoding::OpReplaceNoErr)
                .unwrap_or_else(|error| error.output().to_vec());
        }
        (tok, String::from_utf8_lossy(&utf8).into_owned())
    }

    // 预读 token 时复制 reader，完成关键字识别后恢复，保证主扫描位置不变。
    /// 预读下一 token 后恢复 reader，供复合关键字判断。
    fn getNextToken(&mut self) -> i32 {
        let saved = self.r.clone();
        let (mut tok, pos, lit) = self.scan();
        if tok == token::identifier {
            tok = self.handleIdent(&mut yySymType::default());
        }
        if tok == token::identifier {
            let keyword = self.isTokenIdentifier(&lit, pos.Offset);
            if keyword != 0 {
                tok = keyword;
            }
        }
        self.r = saved;
        tok
    }

    /// 预读后续两个 token 后恢复 reader。
    fn getNextTwoTokens(&mut self) -> (i32, i32) {
        let saved = self.r.clone();
        let (mut first_token, pos1, lit1) = self.scan();
        if first_token == token::identifier {
            first_token = self.handleIdent(&mut yySymType::default());
        }
        if first_token == token::identifier {
            let keyword = self.isTokenIdentifier(&lit1, pos1.Offset);
            if keyword != 0 {
                first_token = keyword;
            }
        }
        let (mut second_token, pos2, lit2) = self.scan();
        if second_token == token::identifier {
            second_token = self.handleIdent(&mut yySymType::default());
        }
        if second_token == token::identifier {
            let keyword = self.isTokenIdentifier(&lit2, pos2.Offset);
            if keyword != 0 {
                second_token = keyword;
            }
        }
        self.r = saved;
        (first_token, second_token)
    }

    // Lex 是主入口：扫描原始 token，处理复合关键字、字面量与字符集转换，并写入语义值。
    /// 主词法入口：扫描、合并复合关键字、转换字面量并写入语义值。
    pub fn Lex(&mut self, v: &mut yySymType) -> i32 {
        let (mut tok, mut pos, mut lit) = self.scan();
        self.lastScanOffset = pos.Offset;
        self.lastKeyword3 = self.lastKeyword2;
        self.lastKeyword2 = self.lastKeyword;
        self.lastKeyword = 0;
        v.offset = pos.Offset;
        v.ident = lit.clone();
        if tok == token::identifier {
            tok = self.handleIdent(v);
        }
        if tok == token::identifier {
            let keyword = self.isTokenIdentifier(&lit, pos.Offset);
            if keyword != 0 {
                tok = keyword;
                self.lastKeyword = keyword;
            }
        }
        if self.sqlMode.HasANSIQuotesMode()
            && tok == token::stringLit
            && self.r.s.as_bytes().get(v.offset as usize) == Some(&b'"')
        {
            tok = token::identifier;
        }
        if tok == token::pipes && !self.sqlMode.HasPipesAsConcatMode() {
            return token::pipesAsOr;
        }
        if tok == token::not && self.sqlMode.HasHighNotPrecedenceMode() {
            return token::not2;
        }

        // AS OF/MEMBER OF 在语法层是单 token，这里消费第二个词并合并原文。
        if (tok == token::r#as || tok == token::member) && self.getNextToken() == token::of {
            (_, pos, lit) = self.scan();
            v.ident = format!("{} {}", v.ident, lit);
            self.lastScanOffset = pos.Offset;
            v.offset = pos.Offset;
            self.lastKeyword = if tok == token::r#as {
                token::asof
            } else {
                token::memberof
            };
            return self.lastKeyword;
        }
        if tok == token::to {
            let (t1, t2) = self.getNextTwoTokens();
            if (t1 == token::timestampType && t2 == token::stringLit)
                || (t1 == token::tsoType && t2 == intLit)
            {
                (_, pos, lit) = self.scan();
                v.ident = format!("{} {}", v.ident, lit);
                self.lastKeyword = if t1 == token::timestampType {
                    token::toTimestamp
                } else {
                    token::toTSO
                };
                self.lastScanOffset = pos.Offset;
                v.offset = pos.Offset;
                return self.lastKeyword;
            }
        }
        // 消除 DEFINED NULL BY xxx OPTIONALLY ENCLOSED BY 的 shift/reduce 冲突。
        if tok == token::optionally && self.getNextTwoTokens() == (token::enclosed, token::by) {
            let (_, _, middle) = self.scan();
            let (_, pos2, tail) = self.scan();
            v.ident = format!("{} {} {}", v.ident, middle, tail);
            self.lastKeyword = token::optionallyEnclosedBy;
            self.lastScanOffset = pos2.Offset;
            v.offset = pos2.Offset;
            return token::optionallyEnclosedBy;
        }

        match tok {
            t if t == intLit => return toInt(self, v, &lit),
            t if t == floatLit => return toFloat(self, v, &lit),
            t if t == decLit => return toDecimal(self, v, &lit),
            t if t == hexLit => return toHex(self, v, &lit),
            t if t == bitLit => return toBit(self, v, &lit),
            t if [
                token::singleAtIdentifier,
                token::doubleAtIdentifier,
                token::cast,
                token::extract,
            ]
            .contains(&t) =>
            {
                v.item = Some(Box::new(Value::String(lit)));
                return tok;
            }
            // Go leaves item nil for NULL, so LexLiteral falls back to the raw
            // identifier while grammar actions construct the NULL expression.
            t if t == token::null => v.item = None,
            t if t == quotedIdentifier || t == token::identifier => {
                tok = token::identifier;
                self.identifierDot = self.r.peek() == b'.';
                (tok, v.ident) = self.convert2System(tok, lit);
            }
            t if t == token::stringLit => (tok, v.ident) = self.convert2Connection(tok, lit),
            _ => {}
        }
        tok
    }

    // LexLiteral returns the dynamic semantic value just like Go's interface{}.
    // Keeping the concrete i64/u64/f64/decimal/binary value is important: reducing
    // everything to a string changes overflow and literal-driver behavior.
    /// 返回字面量的动态语义值（保留 i64/u64/f64/decimal 等具体类型）。
    pub fn LexLiteral(&mut self) -> Box<dyn std::any::Any> {
        let mut sym = yySymType::default();
        self.Lex(&mut sym);
        sym.item.take().unwrap_or_else(|| Box::new(sym.ident))
    }

    /// 设置 SQL Mode（影响引号、转义、NOT 优先级等）。
    pub fn SetSQLMode(&mut self, mode: mysql::SQLMode) {
        self.sqlMode = mode;
    }
    /// 获取当前 SQL Mode。
    pub fn GetSQLMode(&self) -> mysql::SQLMode {
        self.sqlMode
    }
    /// 是否将窗口函数名识别为专用 token。
    pub fn EnableWindowFunc(&mut self, val: bool) {
        self.supportWindowFunc = val;
    }
    /// 控制是否在非常规位置仍保留优化器 Hint。
    fn setKeepHint(&mut self, val: bool) {
        self.keepHint = val;
    }

    // InheritScanner 只继承 Go 中列出的配置；诊断、偏移和 buffer 都从空状态开始。
    /// 继承父 Scanner 配置，诊断与缓冲从空状态开始。
    pub fn InheritScanner(&self, sql: String) -> Scanner {
        let mut child = Scanner::empty(reader::new(sql));
        child.client = self.client;
        child.sqlMode = self.sqlMode;
        child.supportWindowFunc = self.supportWindowFunc;
        child
    }

    /// 处理 `_charset` introducer 标识符。
    fn handleIdent(&self, lval: &mut yySymType) -> i32 {
        if !lval.ident.starts_with('_') {
            return token::identifier;
        }
        if let Some(cs) = charset::charset::GetCharsetInfoForIntroducer(&lval.ident[1..]) {
            lval.ident = cs.Name;
            token::underscoreCS
        } else {
            token::identifier
        }
    }

    /// 跳过空白并返回下一非空白字节。
    fn skipWhitespace(&mut self) -> u8 {
        self.r.incAsLongAs(|b| (b as char).is_whitespace())
    }

    // scan 先跳空白与扩展标识符，再沿 ruleTable trie 找最长的起始规则。
    /// 底层扫描：空白、扩展标识符或 ruleTable trie 最长匹配。
    fn scan(&mut self) -> (i32, Pos, String) {
        let mut ch = self.r.peek();
        if (ch as char).is_whitespace() {
            ch = self.skipWhitespace();
        }
        let pos = self.r.pos();
        if self.r.eof() {
            return (0, pos, String::new());
        }
        if isIdentExtend(ch) {
            return scanIdentifier(self);
        }
        let mut node: &trieNode = &ruleTable;
        while !self.r.eof() {
            let Some(child) = node.childs.get(ch as usize).and_then(|n| n.as_ref()) else {
                break;
            };
            node = child;
            if let Some(f) = node.function {
                return f(self);
            }
            self.r.inc();
            ch = self.r.peek();
        }
        (node.token, pos, self.r.data(&pos))
    }

    // scanString 处理双引号/单引号、成对引号与反斜杠转义；未闭合字符串返回 invalid。
    /// 扫描引号字符串（含成对引号与反斜杠转义）。
    fn scanString(&mut self) -> (i32, Pos, String) {
        let pos = self.r.pos();
        let ending = self.r.readByte();
        self.buf.clear();
        while !self.r.eof() {
            let at = self.r.pos();
            if self.r.skipRune(self.client) {
                self.buf.extend(self.r.data(&at).bytes());
                continue;
            }
            let ch = self.r.readByte();
            if ch == ending {
                if self.r.peek() != ending {
                    return (
                        token::stringLit,
                        pos,
                        String::from_utf8_lossy(&self.buf).into_owned(),
                    );
                }
                self.r.inc();
                self.buf.push(ch);
            } else if ch == b'\\' && !self.sqlMode.HasNoBackslashEscapesMode() {
                if self.r.eof() {
                    break;
                }
                self.handleEscape(self.r.peek());
                self.r.inc();
            } else {
                self.buf.push(ch);
            }
        }
        (invalid, pos, String::new())
    }

    // handleEscape 调用 parser/util 的 MySQL 转义表；\% 与 \_ 会保留反斜杠。
    /// 按 MySQL 转义表写入缓冲。
    fn handleEscape(&mut self, b: u8) {
        self.buf.extend(util::UnescapeChar(b));
    }

    /// 消费八进制数字。
    fn scanOct(&mut self) {
        self.r.incAsLongAs(|b| (b'0'..=b'7').contains(&b));
    }
    /// 消费十六进制数字。
    fn scanHex(&mut self) {
        self.r.incAsLongAs(|b| b.is_ascii_hexdigit());
    }
    /// 消费二进制位串数字。
    fn scanBit(&mut self) {
        self.r.incAsLongAs(|b| b == b'0' || b == b'1');
    }

    // scanFloat 从 beg 重扫 D1.D2eD3；指数不合法时按 Go 规则退回 identifier。
    /// 扫描浮点/十进制字面量；非法指数按标识符回退。
    fn scanFloat(&mut self, beg: &Pos) -> (i32, Pos, String) {
        self.r.updatePos(*beg);
        self.scanDigits();
        let mut ch = self.r.peek();
        if ch == b'.' {
            self.r.inc();
            self.scanDigits();
            ch = self.r.peek();
        }
        let tok = if ch == b'e' || ch == b'E' {
            self.r.inc();
            ch = self.r.peek();
            if ch == b'-' || ch == b'+' {
                self.r.inc();
            }
            if isDigit(self.r.peek()) {
                self.scanDigits();
                floatLit
            } else {
                self.r.updatePos(*beg);
                self.r.incAsLongAs(isIdentChar);
                token::identifier
            }
        } else {
            decLit
        };
        (tok, *beg, self.r.data(beg))
    }

    /// 消费连续十进制数字并返回切片。
    fn scanDigits(&mut self) -> String {
        let pos = self.r.pos();
        self.r.incAsLongAs(isDigit);
        self.r.data(&pos)
    }

    // scanVersionDigits 仅在位数达到 min 后接受；不足时整体回滚。
    /// 扫描版本注释中的数字，不足 min 则回滚。
    fn scanVersionDigits(&mut self, min: usize, max: usize) {
        let pos = self.r.pos();
        for i in 0..max {
            if isDigit(self.r.peek()) {
                self.r.inc();
            } else if i < min {
                self.r.updatePos(pos);
                return;
            } else {
                break;
            }
        }
    }

    // scanFeatureIDs 解析 /*T![f1,f2] 的特性列表；任何格式错误都恢复到起点。
    /// 解析 `/*T![id,...]` 特性 ID 列表。
    fn scanFeatureIDs(&mut self) -> Option<Vec<String>> {
        let pos = self.r.pos();
        let mut state = 0;
        let mut current_id = String::new();
        let mut ids = Vec::new();
        while !self.r.eof() {
            let ch = self.r.peek();
            self.r.inc();
            match state {
                0 if ch == b'[' => state = 1,
                1 if isIdentChar(ch) => {
                    current_id.push(ch as char);
                    state = 2;
                }
                2 if isIdentChar(ch) => current_id.push(ch as char),
                2 if ch == b',' => {
                    ids.push(std::mem::take(&mut current_id));
                    state = 1;
                }
                2 if ch == b']' => {
                    ids.push(current_id);
                    return Some(ids);
                }
                _ => {
                    self.r.updatePos(pos);
                    return None;
                }
            }
        }
        self.r.updatePos(pos);
        None
    }

    // 把最后一个编码 error 降级为 warning，保持两组诊断的相对顺序。
    /// 将最近一次 error 降级为 warning。
    pub(super) fn lastErrorAsWarn(&mut self) {
        if self.errs.last().is_some_and(|error| {
            let message = error.to_string();
            message.contains("parentheses nesting depth exceeds maximum")
                || message.contains("AST nesting depth exceeds maximum")
        }) {
            return;
        }
        if let Some(err) = self.errs.pop() {
            self.warns.push(err);
        }
    }

    // empty 集中构造零状态；实际依赖类型的默认值待模块接线时替换。
    /// 构造零状态 Scanner。
    fn empty(r: reader) -> Scanner {
        Scanner {
            r,
            buf: Vec::new(),
            client: charset::encoding::FindEncoding(mysql::DefaultCharset),
            connection: charset::encoding::FindEncoding(mysql::DefaultCharset),
            errs: Vec::new(),
            warns: Vec::new(),
            stmtStartPos: 0,
            inBangComment: false,
            sqlMode: mysql::SQLMode::default(),
            supportWindowFunc: false,
            skipPositionRecording: false,
            lastScanOffset: 0,
            lastKeyword: 0,
            lastKeyword2: 0,
            lastKeyword3: 0,
            lastHintPos: Pos::default(),
            identifierDot: false,
            keepHint: false,
        }
    }
}

/// `Scanner` 的默认空实例。
impl Default for Scanner {
    fn default() -> Self {
        Self::empty(reader::new(String::new()))
    }
}

/// 对接 Yacc 生成的 `yyLexer` trait。
impl yyLexer for Scanner {
    fn statement_text(&mut self) -> Option<(charset::encoding::EncodingRef, String)> {
        Some((self.client, self.stmtText()))
    }
    fn Lex(&mut self, lval: &mut yySymType) -> isize {
        Scanner::Lex(self, lval) as isize
    }
    fn Errorf(&self, format: &str, _args: &[&dyn std::any::Any]) -> ParserError {
        Scanner::Errorf(self, format)
    }
    fn AppendError(&mut self, err: ParserError) {
        self.errs.push(err);
    }
    fn AppendWarn(&mut self, err: ParserError) {
        self.warns.push(err);
    }
    fn LastErrorAsWarn(&mut self) {
        self.lastErrorAsWarn();
    }
    fn Errors(&self) -> (Vec<ParserError>, Vec<ParserError>) {
        (self.warns.clone(), self.errs.clone())
    }
    fn sql_mode_bits(&self) -> i64 {
        self.GetSQLMode().0
    }
    fn hint_position(&self) -> Pos {
        self.lastHintPos
    }
    fn skip_position_recording(&self) -> bool {
        self.skipPositionRecording
    }
}

// NewScanner 对应 Go 构造函数，并通过 reset 设置默认客户端/连接编码。
/// 构造 Scanner 并 `reset` 默认编码。
pub fn NewScanner(sql: String) -> Scanner {
    let mut scanner = Scanner::empty(reader::new(sql.clone()));
    scanner.reset(sql);
    scanner
}

/// 处理 `X'...'` 十六进制字面量或标识符。
fn startWithXx(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.inc();
    if s.r.peek() == b'\'' {
        s.r.inc();
        s.scanHex();
        if s.r.peek() == b'\'' {
            s.r.inc();
            return (hexLit, pos, s.r.data(&pos));
        }
        return (invalid, pos, String::new());
    }
    s.r.updatePos(pos);
    scanIdentifier(s)
}

/// 处理 `N'...'` 国家字符集字面量前缀。
fn startWithNn(s: &mut Scanner) -> (i32, Pos, String) {
    let (mut tok, pos, mut lit) = scanIdentifier(s);
    if (lit == "N" || lit == "n") && s.r.peek() == b'\'' {
        tok = token::underscoreCS;
        lit = "utf8".into();
    }
    (tok, pos, lit)
}

/// 处理 `B'...'` 位串字面量或标识符。
fn startWithBb(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.inc();
    if s.r.peek() == b'\'' {
        s.r.inc();
        s.scanBit();
        if s.r.peek() == b'\'' {
            s.r.inc();
            return (bitLit, pos, s.r.data(&pos));
        }
        return (invalid, pos, String::new());
    }
    s.r.updatePos(pos);
    scanIdentifier(s)
}

// # 与合法的 -- 注释均消费到换行，再递归扫描下一个 token。
/// 井号行注释，消费至换行后继续扫描。
/// 井号行注释，消费至换行后继续扫描。
fn startWithSharp(s: &mut Scanner) -> (i32, Pos, String) {
    s.r.incAsLongAs(|b| b != b'\n');
    s.scan()
}

/// 处理 `--` 注释、JSON 路径算子 `->`/`->>` 或减号。
fn startWithDash(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    let rest = &s.r.s[pos.Offset as usize..];
    if rest.starts_with("--")
        && (rest.len() == 2
            || rest
                .as_bytes()
                .get(2)
                .is_some_and(|b| (*b as char).is_whitespace()))
    {
        s.r.incAsLongAs(|b| b != b'\n');
        return s.scan();
    }
    if rest.starts_with("->>") {
        s.r.incN(3);
        return (token::juss, pos, String::new());
    }
    if rest.starts_with("->") {
        s.r.incN(2);
        return (token::jss, pos, String::new());
    }
    s.r.inc();
    (b'-' as i32, pos, "-".into())
}

// startWithSlash 区分除号、版本注释、TiDB 特性注释、优化器 hint 与普通块注释。
/// 处理除号、版本/特性注释、优化器 Hint 与块注释。
fn startWithSlash(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.inc();
    if s.r.peek() != b'*' {
        return (b'/' as i32, pos, "/".into());
    }
    let mut optimizer_hint = false;
    let mut current_star = false;
    s.r.inc();
    match s.r.readByte() {
        b'!' => {
            s.scanVersionDigits(5, 5);
            s.inBangComment = true;
            return s.scan();
        }
        b'T' => {
            if s.r.peek() == b'!' {
                s.r.inc();
                if let Some(ids) = s.scanFeatureIDs() {
                    let feature_ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                    if tidbfeature::CanParseFeature(&feature_ids) {
                        s.inBangComment = true;
                        return s.scan();
                    }
                }
            }
        }
        b'+' => {
            if hintedTokens.contains(&s.lastKeyword) || s.keepHint {
                if s.lastKeyword2 == token::forKwd {
                    if s.lastKeyword3 == token::binding {
                        optimizer_hint = true;
                    } else {
                        s.warns.push(ParseErrorWith(&s.r.data(&pos), s.r.p.Line));
                    }
                } else {
                    optimizer_hint = true;
                }
            } else {
                s.AppendWarn(ErrWarnOptimizerHintWrongPos.GenWithStackByArgs(&[]));
            }
        }
        b'*' => current_star = true,
        _ => {}
    }
    // 块注释只在 */ 结束；连续星号需保留 current_star，EOF/未闭合记录语法错误。
    loop {
        if current_star || s.r.incAsLongAs(|b| b != b'*') == b'*' {
            match s.r.readByte() {
                b'/' => {
                    if optimizer_hint {
                        s.lastHintPos = pos;
                        return (token::hintComment, pos, s.r.data(&pos));
                    }
                    return s.scan();
                }
                b'*' => {
                    current_star = true;
                    continue;
                }
                _ => {
                    current_star = false;
                    continue;
                }
            }
        }
        s.errs.push(ParseErrorWith(&s.r.data(&pos), s.r.p.Line));
        return (0, pos, String::new());
    }
}

/// 处理星号或 bang 注释结束符 `*/`。
fn startWithStar(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.inc();
    if s.inBangComment && s.r.peek() == b'/' {
        s.inBangComment = false;
        s.r.inc();
        return s.scan();
    }
    s.identifierDot = false;
    (b'*' as i32, pos, "*".into())
}

// @ 扫描用户变量；@@ 可带 global/session/local 前缀，字符串和反引号名称会重组原文本。
/// 扫描用户变量 `@` / 系统变量 `@@`。
fn startWithAt(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.inc();
    let (mut tok, mut lit) = scanIdentifierOrString(s);
    if tok == b'@' as i32 {
        s.r.inc();
        let stream = s.r.s[pos.Offset as usize + 2..].to_owned();
        let mut prefix = "";
        for candidate in ["global.", "session.", "local."] {
            if stream.len() >= candidate.len()
                && stream[..candidate.len()].eq_ignore_ascii_case(candidate)
            {
                prefix = candidate;
                s.r.incN(candidate.len());
                break;
            }
        }
        (tok, lit) = scanIdentifierOrString(s);
        if tok == token::stringLit || tok == quotedIdentifier {
            lit = format!("@@{}{}", prefix, lit);
            tok = token::doubleAtIdentifier;
        } else if tok == token::identifier {
            lit = s.r.data(&pos);
            tok = token::doubleAtIdentifier;
        }
    } else if tok != invalid {
        tok = token::singleAtIdentifier;
    }
    (tok, pos, lit)
}

/// 扫描普通标识符。
fn scanIdentifier(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.incAsLongAs(isIdentChar);
    (token::identifier, pos, s.r.data(&pos))
}

/// 按首字符扫描标识符、字符串或反引号名。
fn scanIdentifierOrString(s: &mut Scanner) -> (i32, String) {
    match s.r.peek() {
        b'\'' | b'"' => {
            let (t, _, l) = s.scanString();
            (t, l)
        }
        b'`' => {
            let (t, _, l) = scanQuotedIdent(s);
            (t, l)
        }
        ch if isUserVarChar(ch) => {
            let pos = s.r.pos();
            s.r.incAsLongAs(isUserVarChar);
            (token::identifier, s.r.data(&pos))
        }
        ch => (ch as i32, String::new()),
    }
}

/// 反引号标识符的内部 token 标记。
const quotedIdentifier: i32 = -token::identifier;

// scanQuotedIdent 去掉外层反引号，把 `` 还原成单个 `；多字节字符由 client 编码决定步长。
/// 扫描反引号标识符，还原成对 ``。
fn scanQuotedIdent(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.inc();
    s.buf.clear();
    while !s.r.eof() {
        let at = s.r.pos();
        if s.r.skipRune(s.client) {
            s.buf.extend(s.r.data(&at).bytes());
            continue;
        }
        let ch = s.r.readByte();
        if ch == b'`' {
            if s.r.peek() != b'`' {
                return (
                    quotedIdentifier,
                    pos,
                    String::from_utf8_lossy(&s.buf).into_owned(),
                );
            }
            s.r.inc();
        }
        s.buf.push(ch);
    }
    (invalid, pos, String::new())
}

/// 字符串字面量入口，委托 `scanString`。
fn startString(s: &mut Scanner) -> (i32, Pos, String) {
    s.scanString()
}

// lazyBuf 保留 Go 的延迟分配思想：无转义时直接切 reader，有转义时才读独立 buffer。
/// 延迟缓冲：无转义时直接切片，有转义时再分配。
struct lazyBuf<'a> {
    useBuf: bool,
    r: &'a reader,
    b: Vec<u8>,
    p: Pos,
}
impl lazyBuf<'_> {
    fn setUseBuf(&mut self, text: &str) {
        if !self.useBuf {
            self.useBuf = true;
            self.b.clear();
            self.b.extend(text.bytes());
        }
    }
    fn writeRune(&mut self, ch: char, width: usize) {
        if self.useBuf {
            if width > 1 {
                self.b.extend(ch.to_string().bytes());
            } else {
                self.b.push(ch as u8);
            }
        }
    }
    fn data(&self) -> String {
        if self.useBuf {
            String::from_utf8_lossy(&self.b).into_owned()
        } else {
            let raw = self.r.data(&self.p);
            raw[1..raw.len() - 1].to_owned()
        }
    }
}

/// 扫描数字字面量（含 0x/0b/八进制/浮点回退）。
fn startWithNumber(s: &mut Scanner) -> (i32, Pos, String) {
    if s.identifierDot {
        return scanIdentifier(s);
    }
    let pos = s.r.pos();
    let mut tok = intLit;
    let first = s.r.readByte();
    if first == b'0' {
        match s.r.peek() {
            b'0'..=b'7' => {
                s.r.inc();
                s.scanOct();
            }
            b'x' | b'X' => {
                s.r.inc();
                let p1 = s.r.pos();
                s.scanHex();
                let p2 = s.r.pos();
                if p1 == p2 || isDigit(s.r.peek()) {
                    s.r.incAsLongAs(isIdentChar);
                    return (token::identifier, pos, s.r.data(&pos));
                }
                tok = hexLit;
            }
            b'b' => {
                s.r.inc();
                let p1 = s.r.pos();
                s.scanBit();
                let p2 = s.r.pos();
                if p1 == p2 || isDigit(s.r.peek()) {
                    s.r.incAsLongAs(isIdentChar);
                    return (token::identifier, pos, s.r.data(&pos));
                }
                tok = bitLit;
            }
            b'.' => return s.scanFloat(&pos),
            b'B' => {
                s.r.incAsLongAs(isIdentChar);
                return (token::identifier, pos, s.r.data(&pos));
            }
            _ => {}
        }
    }
    s.scanDigits();
    let ch = s.r.peek();
    if ch == b'.' || ch == b'e' || ch == b'E' {
        return s.scanFloat(&pos);
    }
    if !s.r.eof() && isIdentChar(ch) {
        s.r.incAsLongAs(isIdentChar);
        return (token::identifier, pos, s.r.data(&pos));
    }
    (tok, pos, s.r.data(&pos))
}

/// 扫描点号或 `.digits` 小数。
fn startWithDot(s: &mut Scanner) -> (i32, Pos, String) {
    let pos = s.r.pos();
    s.r.inc();
    if s.identifierDot {
        return (b'.' as i32, pos, ".".into());
    }
    if isDigit(s.r.peek()) {
        let result = s.scanFloat(&pos);
        return if result.0 == token::identifier {
            (invalid, result.1, result.2)
        } else {
            result
        };
    }
    (b'.' as i32, pos, ".".into())
}

// reader 按字节移动但同步维护行列；编码相关的多字节跳过由 skipRune 完成。
#[derive(Clone)]
/// 按字节推进并维护行列的 SQL 输入读取器。
struct reader {
    s: String,
    p: Pos,
    l: usize,
}
impl reader {
    fn new(s: String) -> reader {
        let l = s.len();
        reader {
            s,
            p: Pos {
                Line: 1,
                Col: 0,
                Offset: 0,
            },
            l,
        }
    }
    fn eof(&self) -> bool {
        self.p.Offset as usize >= self.l
    }
    fn peek(&self) -> u8 {
        if self.eof() {
            0
        } else {
            self.s.as_bytes()[self.p.Offset as usize]
        }
    }
    fn inc(&mut self) {
        if self.s.as_bytes()[self.p.Offset as usize] == b'\n' {
            self.p.Line += 1;
            self.p.Col = 0;
        }
        self.p.Offset += 1;
        self.p.Col += 1;
    }
    fn incN(&mut self, n: usize) {
        for _ in 0..n {
            self.inc();
        }
    }
    fn readByte(&mut self) -> u8 {
        let ch = self.peek();
        if !self.eof() {
            self.inc();
        }
        ch
    }
    fn pos(&self) -> Pos {
        self.p
    }
    fn updatePos(&mut self, pos: Pos) {
        self.p = pos;
    }
    fn data(&self, from: &Pos) -> String {
        self.s[from.Offset as usize..self.p.Offset as usize].to_owned()
    }
    fn incAsLongAs(&mut self, test: impl Fn(u8) -> bool) -> u8 {
        loop {
            let ch = self.peek();
            if !test(ch) {
                return ch;
            }
            if self.eof() {
                return 0;
            }
            self.inc();
        }
    }
    // 非 ASCII 字节优先沿用 Go 的 Encoding.MbLen。Rust 输入始终是合法 UTF-8
    // String；当客户端编码宽度会落在 UTF-8 字符中间时，改按实际字符宽度移动，
    // 避免后续按 token 字节范围切片时越过字符边界。
    fn skipRune(&mut self, enc: charset::encoding::EncodingRef) -> bool {
        if self.peek().is_ascii() {
            return false;
        }
        let offset = self.p.Offset as usize;
        let encoded_width = enc.MbLen(&self.s.as_bytes()[offset..]);
        let encoded_end = offset.saturating_add(encoded_width);
        let width = if encoded_width > 0
            && encoded_end <= self.s.len()
            && self.s.is_char_boundary(encoded_end)
        {
            encoded_width
        } else {
            self.s[offset..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0)
        };
        self.incN(width);
        width > 0
    }
}

/// EOF 哨兵位置。
const eof: Pos = Pos {
    Line: -1,
    Col: -1,
    Offset: -1,
};
