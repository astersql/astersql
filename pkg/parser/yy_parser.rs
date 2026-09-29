// Copyright 2015 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// SQL 解析器入口与 yacc 辅助逻辑。
//
// 本模块对齐 Go 的 `yy_parser.go`：对外暴露 `Parser`、解析错误码、字面量转换
// 与 `ParseParam`（连接字符集/排序规则等可变参数），对内衔接词法器 `Scanner`
// 与 goyacc 生成的 `yyParse`。解析结果为 AST 语句列表；语法错误映射为与
// MySQL 兼容的错误码与 near/line 诊断信息。

// 本文件对齐 pkg/parser/yy_parser.go，保留解析器入口、状态与辅助函数的顺序。

use std::any::Any as YyAny;
use std::sync::LazyLock as YyLazyLock;

// 下列错误值对应 Go 包级 parser 错误；构造方式保留 terror 错误类与 MySQL 错误码的映射。
/// 语法错误（MySQL ErrSyntax）。
pub static ErrSyntax: YyLazyLock<Box<terror::Error>> =
    YyLazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrSyntax as isize)));
/// 解析错误（MySQL ErrParse）。
pub static ErrParse: YyLazyLock<Box<terror::Error>> =
    YyLazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrParse as isize)));
/// 未知字符集。
pub static ErrUnknownCharacterSet: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrUnknownCharacterSet as isize))
});
/// 未知排序规则。
pub static ErrUnknownCollation: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassDDL.NewStd(terror::ErrCode(mysql::ErrUnknownCollation as isize))
});
/// YEAR 列长度非法。
pub static ErrInvalidYearColumnLength: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrInvalidYearColumnLength as isize))
});
/// 函数/语句参数错误。
pub static ErrWrongArguments: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWrongArguments as isize))
});
/// 字段终止符非法。
pub static ErrWrongFieldTerminators: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWrongFieldTerminators as isize))
});
/// 显示宽度过大。
pub static ErrTooBigDisplayWidth: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrTooBigDisplaywidth as isize))
});
/// 精度过大。
pub static ErrTooBigPrecision: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrTooBigPrecision as isize))
});
/// 未知 ALTER LOCK 选项。
pub static ErrUnknownAlterLock: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrUnknownAlterLock as isize))
});
/// 未知 ALTER ALGORITHM 选项。
pub static ErrUnknownAlterAlgorithm: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrUnknownAlterAlgorithm as isize))
});
/// 取值非法。
pub static ErrWrongValue: YyLazyLock<Box<terror::Error>> =
    YyLazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWrongValue as isize)));
/// 已弃用语法警告（有替代写法）。
pub static ErrWarnDeprecatedSyntax: YyLazyLock<Box<terror::Error>> = YyLazyLock::new(|| {
    terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWarnDeprecatedSyntax as isize))
});
/// 已弃用语法警告（无替代写法）。
pub static ErrWarnDeprecatedSyntaxNoReplacement: YyLazyLock<Box<terror::Error>> =
    YyLazyLock::new(|| {
        terror::ClassParser.NewStd(terror::ErrCode(
            mysql::ErrWarnDeprecatedSyntaxNoReplacement as isize,
        ))
    });
/// 用法错误。
pub static ErrWrongUsage: YyLazyLock<Box<terror::Error>> =
    YyLazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWrongUsage as isize)));
/// 数据库名非法。
pub static ErrWrongDBName: YyLazyLock<Box<terror::Error>> =
    YyLazyLock::new(|| terror::ClassParser.NewStd(terror::ErrCode(mysql::ErrWrongDBName as isize)));

// MySQL 特殊注释的三个正则保持 Go 的匹配边界；实际 Regex 初始化留给模块接线阶段。
/// 匹配 `/*!` / `/*!Mnnnnnn` 与 `*/` 片段，用于扫描特殊注释边界。
pub static SpecFieldPattern: YyLazyLock<regex::Regex> =
    YyLazyLock::new(|| regex::Regex::new(r"(\/\*!(M?[0-9]{5,6})?|\*\/)").unwrap());
static specCodeStart: YyLazyLock<regex::Regex> =
    YyLazyLock::new(|| regex::Regex::new(r"^\/\*!(M?[0-9]{5,6})?[ \t]*").unwrap());
static specCodeEnd: YyLazyLock<regex::Regex> =
    YyLazyLock::new(|| regex::Regex::new(r"[ \t]*\*\/$").unwrap());

/// 剥离版本化特殊注释的首尾标记，保留其中 SQL（对应 Go TrimComment）。
// TrimComment 对应 Go 同名函数：剥离版本化特殊注释的首尾标记，保留其中 SQL。
pub fn TrimComment(txt: &str) -> String {
    let txt = specCodeStart.replace_all(txt, "");
    specCodeEnd.replace_all(&txt, "").into_owned()
}

/// 解析器公开配置：窗口函数、DOUBLE 严格检查与是否跳过位置记录。
// ParserConfig 对应 Go 的公开配置；三个开关分别控制窗口函数、DOUBLE 严格检查与位置记录。
pub struct ParserConfig {
    pub EnableWindowFunction: bool,
    pub EnableStrictDoubleTypeCheck: bool,
    pub SkipPositionRecording: bool,
}

// Allow the statement wrappers surrounding a 10,000-level expression.
const MAX_AST_DEPTH_STMT_OVERHEAD: usize = 64;
const MAX_AST_DEPTH: usize = 10_000 + MAX_AST_DEPTH_STMT_OVERHEAD;

// The generated reducer can recursively clone a growing expression before an
// AST exists. Reject the token patterns that make such a chain before parsing.
// The AST visitor below remains the authoritative check for all other shapes.
fn check_expression_depth_before_parse(scanner: &Scanner, sql: &str) -> Result<(), errors::Error> {
    if sql.len() <= MAX_AST_DEPTH {
        return Ok(());
    }
    let mut probe = scanner.InheritScanner(sql.to_owned());
    let mut case_depth = 0usize;
    let mut unary_depth = 0usize;
    let mut binary_depth = 0usize;
    let mut chain_operator = 0;
    let mut previous_operand = false;
    let mut pending_binary = false;
    loop {
        let token = probe.Lex(&mut yySymType::default()) as isize;
        if token == 0 || token == token::invalid {
            break;
        }
        if token == token::caseKwd {
            case_depth += 1;
            if case_depth > MAX_AST_DEPTH {
                return ast_depth_error(MAX_AST_DEPTH);
            }
        } else if token == token::end {
            case_depth = case_depth.saturating_sub(1);
        }

        if token == '!' as isize && !previous_operand {
            unary_depth += 1;
            if unary_depth > MAX_AST_DEPTH {
                return ast_depth_error(MAX_AST_DEPTH);
            }
            continue;
        }
        unary_depth = 0;

        let operand = matches!(token, token::intLit | token::floatLit | token::identifier);
        if operand {
            if pending_binary {
                binary_depth += 1;
                if binary_depth > MAX_AST_DEPTH {
                    return ast_depth_error(MAX_AST_DEPTH);
                }
            } else {
                binary_depth = 0;
            }
            previous_operand = true;
            pending_binary = false;
        } else if token == '+' as isize && previous_operand {
            if chain_operator != token {
                binary_depth = 0;
                chain_operator = token;
            }
            previous_operand = false;
            pending_binary = true;
        } else {
            previous_operand = false;
            pending_binary = false;
            binary_depth = 0;
            chain_operator = 0;
        }
    }
    Ok(())
}

fn ast_depth_error(limit: usize) -> Result<(), errors::Error> {
    Err(ErrParse.GenWithStackByArgs(&[
        "AST nesting depth exceeds maximum".into(),
        limit.to_string().into(),
    ]))
}

struct AstDepthChecker {
    depth: usize,
    limit: usize,
    exceeded: bool,
}

impl parser_ast::InPlaceVisitor for AstDepthChecker {
    fn enter(&mut self, node: &mut dyn parser_ast::Node) -> bool {
        self.depth += 1;
        if self.depth > self.limit {
            self.exceeded = true;
            return true;
        }
        // A chain of parenthesized expressions has one node per pair of
        // parentheses. Walk that spine iteratively to avoid consuming the
        // Rust call stack before reaching the Go depth limit.
        if let Some(expr) = node.as_any_mut().downcast_mut::<parser_ast::ExprNode>() {
            if let parser_ast::ExprKind::Parentheses(inner) = &mut expr.Kind {
                let mut inner = inner.as_mut();
                let mut skipped = 0;
                while matches!(inner.Kind, parser_ast::ExprKind::Parentheses(_)) {
                    self.depth += 1;
                    skipped += 1;
                    if self.depth > self.limit {
                        self.exceeded = true;
                        break;
                    }
                    let parser_ast::ExprKind::Parentheses(next) = &mut inner.Kind else {
                        unreachable!()
                    };
                    inner = next.as_mut();
                }
                if !self.exceeded {
                    parser_ast::Node::accept_in_place(inner, self);
                }
                self.depth -= skipped;
                return true;
            }
        }
        false
    }

    fn leave(&mut self, _node: &mut dyn parser_ast::Node) -> bool {
        self.depth -= 1;
        !self.exceeded
    }
}

pub(crate) fn check_ast_depth_limit(
    statement: &mut dyn parser_ast::Node,
    limit: usize,
) -> Result<(), errors::Error> {
    let mut checker = AstDepthChecker {
        depth: 0,
        limit,
        exceeded: false,
    };
    parser_ast::Walk(statement, &mut checker);
    if checker.exceeded {
        return ast_depth_error(limit);
    }
    Ok(())
}


/// 一次 SQL 解析会话的状态：字符集、词法器、AST 结果与 yacc 符号缓存。
// Parser 保存一次解析所需的连接字符集、词法器、结果以及 yacc 临时值。
// Go 的切片复用和指针字段在这里保留为 Vec/Option 形状，以表达生命周期和可空语义。
pub struct Parser {
    charset: String,
    collation: String,
    result: Vec<Box<dyn parser_ast::Node>>,
    reducedStatementCount: usize,
    allStatementsSemanticallyComplete: bool,
    src: String,
    lexer: Scanner,
    hintParser: Option<Box<hintParser>>,
    lastWarnings: Vec<errors::Error>,
    explicitCharset: bool,
    strictDoubleFieldType: bool,
    enableMariaDB: bool,
    cache: Vec<yySymType>,
    yylval: yySymType,
    yyVAL: Option<Box<yySymType>>,
}

// yySetOffset 只在规约值中已有表达式时记录原始文本偏移。
fn yySetOffset(yyVAL: &mut yySymType, offset: i32) {
    if let Some(expr) = yyVAL.expr.as_mut() {
        expr.SetOriginTextPosition(offset);
    }
}

// hint 语法当前不记录偏移；空函数与 Go 生成器回调签名对应。
fn yyhintSetOffset(_yyVAL: &mut yyhintSymType, _offset: i32) {}

// stmtTexter 对应 Go 的包内接口，用于取得语句文本。
trait stmtTexter {
    fn stmtText(&self) -> String;
}

/// 创建默认 SQL 模式的解析器，并预分配 200 个 yacc 符号缓存。
// New 创建默认 SQL 模式的解析器，并预分配 200 个 yacc 符号缓存。
pub fn New() -> Box<Parser> {
    // Rust 通过显式 crate 依赖链接 Go 中由 init 注册的 parser driver。
    parser_test_driver::init_test_driver();

    let mut parser_state = Box::new(Parser {
        charset: String::new(),
        collation: String::new(),
        result: Vec::new(),
        reducedStatementCount: 0,
        allStatementsSemanticallyComplete: true,
        src: String::new(),
        lexer: Scanner::default(),
        hintParser: None,
        lastWarnings: Vec::new(),
        explicitCharset: false,
        strictDoubleFieldType: false,
        enableMariaDB: false,
        cache: Vec::with_capacity(200),
        yylval: yySymType::default(),
        yyVAL: None,
    });
    parser_state.reset();
    parser_state
}

impl Parser {
    /// Returns warnings from the latest parse, including a failed parse.
    pub fn Warnings(&self) -> Vec<errors::Error> {
        self.lastWarnings.clone()
    }

    /// Go's three-result contract; the legacy Result entry point remains available.
    pub fn ParseSQLWithWarnings(
        &mut self,
        sql: &str,
        params: &[&dyn ParseParam],
    ) -> (
        Vec<Box<dyn parser_ast::Node>>,
        Vec<errors::Error>,
        Option<errors::Error>,
    ) {
        match self.ParseSQL(sql, params) {
            Ok((statements, warnings)) => (statements, warnings, None),
            Err(error) => (Vec::new(), self.Warnings(), Some(error)),
        }
    }

    // Reset 清空已复用的符号缓存，并恢复所有默认解析开关。
    pub fn Reset(&mut self) {
        for value in &mut self.cache {
            *value = yySymType::default();
        }
        self.reset();
    }

    // reset 对应 Go 的内部重置逻辑；GetSQLMode 的错误按原实现忽略。
    fn reset(&mut self) {
        self.explicitCharset = false;
        self.strictDoubleFieldType = false;
        self.EnableWindowFunc(true);
        self.SetStrictDoubleTypeCheck(true);
        let mode = mysql::GetSQLMode(mysql::DefaultSQLMode).unwrap_or_default();
        self.SetSQLMode(mode);
    }

    // SetMariaDB 控制扩展 MariaDB 语法模式。
    pub fn SetMariaDB(&mut self, enabled: bool) {
        self.enableMariaDB = enabled;
    }

    // SetStrictDoubleTypeCheck 控制 DOUBLE 字段类型的严格校验。
    pub fn SetStrictDoubleTypeCheck(&mut self, enabled: bool) {
        self.strictDoubleFieldType = enabled;
    }

    // SetParserConfig 一次应用三个公开配置，并把位置记录开关下沉到词法器。
    pub fn SetParserConfig(&mut self, config: ParserConfig) {
        self.EnableWindowFunc(config.EnableWindowFunction);
        self.SetStrictDoubleTypeCheck(config.EnableStrictDoubleTypeCheck);
        self.lexer.skipPositionRecording = config.SkipPositionRecording;
    }

    // ParseSQL 是主解析入口：重置参数和词法器、依次应用可变参数、调用 yacc，再收集警告与首个错误。
    pub fn ParseSQL(
        &mut self,
        sql: &str,
        params: &[&dyn ParseParam],
    ) -> Result<(Vec<Box<dyn parser_ast::Node>>, Vec<errors::Error>), errors::Error> {
        resetParams(self);
        self.lastWarnings.clear();
        self.lexer.reset(sql.to_owned());
        for param in params {
            // 参数按调用顺序生效；任一个失败都在启动语法分析前返回。
            if let Err(error) = param.ApplyOn(self) {
                self.lastWarnings.clear();
                return Err(error);
            }
        }
        self.src = sql.to_owned();
        self.result.clear();
        self.reducedStatementCount = 0;
        self.allStatementsSemanticallyComplete = true;

        check_expression_depth_before_parse(&self.lexer, sql)?;

        // Go always lets the package Scanner and generated goyacc machine own
        // syntax acceptance and diagnostics. Keep the temporary AST conversion
        // below, but never let the third-party converter broaden TiDB grammar.
        let mut lexer = std::mem::take(&mut self.lexer);
        let parse_status = yyParse(&mut lexer, self);
        let (warnings, parse_errors) = lexer.Errors();
        let warnings = warnings.to_vec();
        self.lastWarnings = warnings.clone();
        let parse_errors = parse_errors.to_vec();
        self.lexer = lexer;
        if parse_status != 0 || !parse_errors.is_empty() {
            return Err(parse_errors
                .into_iter()
                .next()
                .unwrap_or_else(|| ErrSyntax.GenWithStackByArgs(&[])));
        }
        if self.allStatementsSemanticallyComplete && self.result.len() == self.reducedStatementCount
        {
            for statement in &mut self.result {
                check_ast_depth_limit(statement.as_mut(), MAX_AST_DEPTH)?;
                parser_ast::SetFlag(statement.as_ref());
            }
            return Ok((std::mem::take(&mut self.result), warnings));
        }

        Err(errors::New(
            "goyacc semantic reduction produced no statement",
        ))
    }

    // Parse 保留旧入口：空字符集或排序规则由对应 ParseParam 恢复为默认值。
    pub fn Parse(
        &mut self,
        sql: &str,
        charset: &str,
        collation: &str,
    ) -> Result<(Vec<Box<dyn parser_ast::Node>>, Vec<errors::Error>), errors::Error> {
        let charset_param = CharsetConnection(charset.to_owned());
        let collation_param = CollationConnection(collation.to_owned());
        self.ParseSQL(sql, &[&charset_param, &collation_param])
    }

    // lastErrorAsWarn 将词法器最后一个错误降级为警告。
    fn lastErrorAsWarn(&mut self) {
        self.lexer.lastErrorAsWarn();
    }

    // ParseOneStmt 强制输入只产生一条语句，否则返回 ErrSyntax。
    pub fn ParseOneStmt(
        &mut self,
        sql: &str,
        charset: &str,
        collation: &str,
    ) -> Result<Box<dyn parser_ast::Node>, errors::Error> {
        let charset_param = CharsetConnection(charset.to_owned());
        let collation_param = CollationConnection(collation.to_owned());
        let (mut stmts, _) = self.ParseSQL(sql, &[&charset_param, &collation_param])?;
        if stmts.len() != 1 {
            return Err(ErrSyntax.GenWithStackByArgs(&[]));
        }
        Ok(stmts.remove(0))
    }

    // SetSQLMode 将 SQL mode 原样交给词法器。
    pub fn SetSQLMode(&mut self, mode: mysql::SQLMode) {
        self.lexer.SetSQLMode(mode);
    }

    // EnableWindowFunc 控制窗口函数相关语法是否可被识别。
    pub fn EnableWindowFunc(&mut self, enabled: bool) {
        self.lexer.EnableWindowFunc(enabled);
    }
}

/// 生成与 MySQL 兼容的 near/line 解析错误，上下文先截到 ErrTextLength。
// ParseErrorWith 生成与 MySQL 兼容的 near/line 错误，并先把上下文截到 ErrTextLength。
pub fn ParseErrorWith(errstr: &str, lineno: i32) -> errors::Error {
    // Go truncates bytes before formatting. A split UTF-8 sequence is rendered
    // as a replacement character because Rust error messages require UTF-8.
    let bytes = &errstr.as_bytes()[..errstr.len().min(mysql::ErrTextLength)];
    let text = String::from_utf8_lossy(bytes);
    errors::Errorf(
        "near '%s' at line %d",
        &[text.as_ref().into(), lineno.into()],
    )
}

impl Parser {
    // startOffset 直接返回 yacc 符号的起始偏移。
    fn startOffset(&self, value: &yySymType) -> i32 {
        value.offset
    }

    // endOffset 按 Go 的字节索引语义向前跳过空白。
    fn endOffset(&self, value: &yySymType) -> i32 {
        let mut byte_offset = value.offset as usize;
        // Preserve Go exactly: parser.src[offset-1] indexes one byte and that
        // byte is converted to a rune before unicode.IsSpace. This can stop in
        // the middle of a multibyte whitespace sequence and callers rely on
        // the resulting byte offset.
        while byte_offset > 0 && char::from(self.src.as_bytes()[byte_offset - 1]).is_whitespace() {
            byte_offset -= 1;
        }
        byte_offset as i32
    }

    // parseHint 延迟创建 hintParser，并沿用当前 SQL mode 和词法器记录的 hint 位置。
    fn parseHint(
        &mut self,
        input: &str,
        mode: mysql::SQLMode,
        position: Pos,
    ) -> (Vec<Box<ast::TableOptimizerHint>>, Vec<errors::Error>) {
        if self.hintParser.is_none() {
            self.hintParser = Some(Box::new(newHintParser()));
        }
        self.hintParser
            .as_mut()
            .unwrap()
            .parse(input, mode, position)
    }
}

// toInt 按十进制解析无符号 64 位整数；超出范围时改走 DECIMAL，以保留超长 SQL 数字字面量。
fn toInt(lexer: &mut dyn yyLexer, lval: &mut yySymType, text: &str) -> i32 {
    match text.parse::<u64>() {
        Ok(value) => {
            // Go 根据 MaxInt64 决定动态值是 int64 还是 uint64。
            lval.item = Some(if value <= i64::MAX as u64 {
                Box::new(value as i64) as Box<dyn YyAny>
            } else {
                Box::new(value)
            });
            intLit
        }
        Err(err) if err.kind() == &std::num::IntErrorKind::PosOverflow => {
            toDecimal(lexer, lval, text)
        }
        Err(err) => {
            let message = format!("integer literal: {err}");
            lexer.AppendError(lexer.Errorf(&message, &[]));
            invalid
        }
    }
}

// toDecimal 创建高精度十进制；数据越界被降级为截断警告并用 MySQL 默认 DECIMAL 继续解析。
fn toDecimal(lexer: &mut dyn yyLexer, lval: &mut yySymType, text: &str) -> i32 {
    let decimal = match new_parser_decimal(text) {
        Ok(decimal) => decimal,
        Err(err) => {
            let message = format!("decimal literal: {err}");
            lexer.AppendError(lexer.Errorf(&message, &[]));
            parser_test_driver::MyDecimal::default()
        }
    };
    lval.item = Some(Box::new(decimal));
    decLit
}

fn new_parser_decimal(text: &str) -> Result<parser_test_driver::MyDecimal, String> {
    let mut decimal = parser_test_driver::MyDecimal::default();
    decimal.FromString(text.as_bytes())?;
    Ok(decimal)
}

// toFloat 解析 IEEE-754 双精度数；范围溢出使用类型非法错误，其他格式错误使用词法器错误。
fn toFloat(lexer: &mut dyn yyLexer, lval: &mut yySymType, text: &str) -> i32 {
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() => {
            lval.item = Some(Box::new(value));
            floatLit
        }
        Ok(_) => {
            let error =
                types::ErrIllegalValueForType.GenWithStackByArgs(&["double".into(), text.into()]);
            lexer.AppendError(error);
            invalid
        }
        Err(err) => {
            let message = format!("float literal: {err}");
            lexer.AppendError(lexer.Errorf(&message, &[]));
            invalid
        }
    }
}

// toHex 按 MySQL 十六进制字面量规则构造 AST 驱动值。
fn toHex(lexer: &mut dyn yyLexer, lval: &mut yySymType, text: &str) -> i32 {
    match parser_test_driver::NewHexLiteral(text) {
        Ok(value) => {
            lval.item = Some(Box::new(value));
            hexLit
        }
        Err(err) => {
            let message = format!("hex literal: {err}");
            lexer.AppendError(lexer.Errorf(&message, &[]));
            invalid
        }
    }
}

// toBit 按 MySQL bit literal 规则构造 AST 驱动值。
fn toBit(lexer: &mut dyn yyLexer, lval: &mut yySymType, text: &str) -> i32 {
    match parser_test_driver::NewBitLiteral(text) {
        Ok(value) => {
            lval.item = Some(Box::new(value));
            bitLit
        }
        Err(err) => {
            let message = format!("bit literal: {err}");
            lexer.AppendError(lexer.Errorf(&message, &[]));
            invalid
        }
    }
}

// getUint64FromNUM 对应 Go 的类型 switch，只接受 i64/u64，其他动态类型返回零。
fn getUint64FromNUM(num: &dyn YyAny) -> u64 {
    if let Some(value) = num.downcast_ref::<i64>() {
        *value as u64
    } else if let Some(value) = num.downcast_ref::<u64>() {
        *value
    } else {
        0
    }
}

// getInt64FromNUM 只接受 i64；其余类型返回原 Go 的有符号 64 位范围错误文本。
fn getInt64FromNUM(num: &dyn YyAny) -> (i64, String) {
    if let Some(value) = num.downcast_ref::<i64>() {
        return (*value, String::new());
    }
    let value = if let Some(value) = num.downcast_ref::<u64>() {
        value.to_string()
    } else if let Some(value) = num.downcast_ref::<i32>() {
        value.to_string()
    } else if let Some(value) = num.downcast_ref::<u32>() {
        value.to_string()
    } else if let Some(value) = num.downcast_ref::<isize>() {
        value.to_string()
    } else if let Some(value) = num.downcast_ref::<usize>() {
        value.to_string()
    } else if let Some(value) = num.downcast_ref::<String>() {
        errors::Errorf("%d", &[value.clone().into()]).to_string()
    } else if let Some(value) = num.downcast_ref::<&str>() {
        errors::Errorf("%d", &[(*value).into()]).to_string()
    } else if let Some(value) = num.downcast_ref::<bool>() {
        errors::Errorf("%d", &[(*value).into()]).to_string()
    } else {
        "<non-numeric>".to_owned()
    };
    (
        -1,
        format!(
            "{} is out of range [–9223372036854775808,9223372036854775807]",
            value
        ),
    )
}

// resetParams 在每次 ParseSQL 前恢复连接字符集和排序规则默认值。
fn resetParams(parser: &mut Parser) {
    parser.charset = mysql::DefaultCharset.to_owned();
    parser.collation = mysql::DefaultCollationName.to_owned();
}

/// 可变解析参数接口：可修改 parser，也可拒绝非法参数（对应 Go ParseParam）。
// ParseParam 对应 Go 的可变解析参数接口；实现可以修改 parser，也可以拒绝非法参数。
pub trait ParseParam {
    fn ApplyOn(&self, parser: &mut Parser) -> Result<(), errors::Error>;
}

/// 未显式标注字符集的字面量所使用的连接字符集。
// CharsetConnection 表示未显式标注字符集的字面量所使用的连接字符集。
pub struct CharsetConnection(pub String);

impl ParseParam for CharsetConnection {
    // 空值恢复 MySQL 默认字符集；词法器 encoding 仍按调用参数查找，与 Go 行为一致。
    fn ApplyOn(&self, parser: &mut Parser) -> Result<(), errors::Error> {
        parser.charset = if self.0.is_empty() {
            mysql::DefaultCharset.to_owned()
        } else {
            self.0.clone()
        };
        parser.lexer.connection = charset::encoding::FindEncoding(&self.0);
        Ok(())
    }
}

/// 未显式标注排序规则的字面量所使用的连接排序规则。
// CollationConnection 表示未显式标注排序规则的字面量所使用的连接排序规则。
pub struct CollationConnection(pub String);

impl ParseParam for CollationConnection {
    fn ApplyOn(&self, parser: &mut Parser) -> Result<(), errors::Error> {
        parser.collation = if self.0.is_empty() {
            mysql::DefaultCollationName.to_owned()
        } else {
            self.0.clone()
        };
        Ok(())
    }
}

/// SQL 输入本身的字符集，词法阶段据此解码为 UTF-8。
// CharsetClient 指定 SQL 输入本身的字符集，用于在词法阶段解码为 UTF-8。
pub struct CharsetClient(pub String);

impl ParseParam for CharsetClient {
    fn ApplyOn(&self, parser: &mut Parser) -> Result<(), errors::Error> {
        parser.lexer.client = charset::encoding::FindEncoding(&self.0);
        Ok(())
    }
}
