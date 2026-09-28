// Copyright 2026 AsterSQL.

//! `.astergram` 文法文件的数据模型与纯 Rust 解析器。
//!
//! 该格式只保存生成解析器所需的文法元数据，不直接嵌入可执行的语义动作代码：
//!
//! ```text
//! %start statement;
//! %token IDENT 256;
//! %token PLUS 43 "+";
//! %left PLUS;
//! %%
//! statement : expression @action;
//! expression : ε | expression PLUS @action IDENT @action;
//! ```
//!
//! 末尾的 `@action` 表示产生式需要语义动作；出现在后续右部符号之前的
//! `@action` 表示规则中间动作，其从零开始的符号位置会写入产生式规范签名。

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

/// 文法源码中的半开区间；行号和列号均从 1 开始。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

impl SourceSpan {
    fn through(self, end: SourceSpan) -> Self {
        Self {
            start: self.start,
            end: end.end,
            line: self.line,
            column: self.column,
        }
    }
}

/// 文法声明的终结符，数值编号在文法内固定且必须唯一。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub name: String,
    pub number: u32,
    pub literal: Option<String>,
    pub span: SourceSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 优先级声明的结合性；`PrecedenceOnly` 只赋予优先级，不指定结合方向。
pub enum Associativity {
    Left,
    Right,
    NonAssoc,
    PrecedenceOnly,
}

/// 优先级声明中使用的记号，可以是终结符名称或带引号的字面量。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PrecedenceSymbol {
    Name(String),
    Literal(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 一组同级优先级记号；声明顺序决定递增的 `level`。
pub struct Precedence {
    pub level: usize,
    pub associativity: Associativity,
    pub symbols: Vec<PrecedenceSymbol>,
    pub span: SourceSpan,
}

/// 产生式右部的一个有序元素。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProductionItem {
    Symbol(String),
    Literal(String),
    /// 位于右部符号之间的规则中间动作，位置按已出现的符号数从零计数。
    Action {
        position: usize,
    },
}

/// 仅由产生式规范签名推导出的稳定标识，不受声明顺序或进程哈希随机化影响。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleId(String);

impl RuleId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 一个展开后的产生式备选分支，包含规范签名和由其生成的稳定规则 ID。
pub struct Production {
    pub lhs: String,
    pub rhs: Vec<ProductionItem>,
    pub precedence: Option<PrecedenceSymbol>,
    pub requires_action: bool,
    pub span: SourceSpan,
    pub signature: String,
    pub rule_id: RuleId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 完整文法模型：起始符、终结符、优先级声明和全部产生式。
pub struct Grammar {
    pub start: String,
    pub start_span: SourceSpan,
    pub tokens: Vec<Token>,
    pub precedence: Vec<Precedence>,
    pub productions: Vec<Production>,
}

impl Grammar {
    /// 解析并校验一份 `.astergram` 源码。
    pub fn parse(source: &str) -> Result<Self, GrammarError> {
        Parser::new(source)?.parse()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 文法解析和静态校验可能返回的错误类别。
pub enum GrammarErrorKind {
    InvalidCharacter,
    InvalidEscape,
    UnterminatedString,
    UnexpectedToken,
    UnknownDirective,
    MissingStart,
    DuplicateStart,
    DuplicateToken,
    ReservedTokenNumber,
    DuplicateTokenNumber,
    DuplicateTokenLiteral,
    DuplicatePrecedence,
    DuplicateProduction,
    MissingGrammarSection,
    MissingProduction,
    UnknownStartSymbol,
    UnknownSymbol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 带源码位置的文法错误，便于调用方生成稳定、可定位的诊断信息。
pub struct GrammarError {
    pub kind: GrammarErrorKind,
    pub message: String,
    pub span: SourceSpan,
}

impl GrammarError {
    fn new(kind: GrammarErrorKind, message: impl Into<String>, span: SourceSpan) -> Self {
        Self {
            kind,
            message: message.into(),
            span,
        }
    }
}

impl fmt::Display for GrammarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at line {}, column {}",
            self.message, self.span.line, self.span.column
        )
    }
}

impl Error for GrammarError {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LexemeKind {
    Directive(String),
    Sections,
    Identifier(String),
    Number(u32),
    Quoted(String),
    Colon,
    Pipe,
    Semicolon,
    Action,
    Epsilon,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Lexeme {
    kind: LexemeKind,
    span: SourceSpan,
}

struct Lexer<'a> {
    source: &'a str,
    offset: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    fn tokenize(mut self) -> Result<Vec<Lexeme>, GrammarError> {
        let mut lexemes = Vec::new();
        loop {
            self.skip_layout();
            let start = self.mark();
            let Some(current) = self.peek() else {
                lexemes.push(Lexeme {
                    kind: LexemeKind::End,
                    span: self.span_from(start),
                });
                return Ok(lexemes);
            };

            let kind = match current {
                '%' => {
                    self.bump();
                    if self.peek() == Some('%') {
                        self.bump();
                        LexemeKind::Sections
                    } else {
                        let directive = self.take_identifier();
                        if directive.is_empty() {
                            return Err(GrammarError::new(
                                GrammarErrorKind::InvalidCharacter,
                                "expected a directive name after '%'",
                                self.span_from(start),
                            ));
                        }
                        LexemeKind::Directive(directive)
                    }
                }
                '@' => {
                    self.bump();
                    let marker = self.take_identifier();
                    if marker != "action" {
                        return Err(GrammarError::new(
                            GrammarErrorKind::UnexpectedToken,
                            format!("unknown action marker '@{marker}'"),
                            self.span_from(start),
                        ));
                    }
                    LexemeKind::Action
                }
                ':' => {
                    self.bump();
                    LexemeKind::Colon
                }
                '|' => {
                    self.bump();
                    LexemeKind::Pipe
                }
                ';' => {
                    self.bump();
                    LexemeKind::Semicolon
                }
                'ε' => {
                    self.bump();
                    LexemeKind::Epsilon
                }
                '\'' | '"' => LexemeKind::Quoted(self.take_quoted(current, start)?),
                value if value.is_ascii_digit() => {
                    let digits = self.take_while(|value| value.is_ascii_digit());
                    let number = digits.parse::<u32>().map_err(|_| {
                        GrammarError::new(
                            GrammarErrorKind::UnexpectedToken,
                            format!("token number '{digits}' does not fit in u32"),
                            self.span_from(start),
                        )
                    })?;
                    LexemeKind::Number(number)
                }
                value if is_identifier_start(value) => {
                    LexemeKind::Identifier(self.take_identifier())
                }
                _ => {
                    self.bump();
                    return Err(GrammarError::new(
                        GrammarErrorKind::InvalidCharacter,
                        format!("invalid character '{current}'"),
                        self.span_from(start),
                    ));
                }
            };
            lexemes.push(Lexeme {
                kind,
                span: self.span_from(start),
            });
        }
    }

    fn take_quoted(
        &mut self,
        quote: char,
        start: (usize, usize, usize),
    ) -> Result<String, GrammarError> {
        self.bump();
        let mut value = String::new();
        loop {
            let Some(current) = self.bump() else {
                return Err(GrammarError::new(
                    GrammarErrorKind::UnterminatedString,
                    "unterminated quoted symbol",
                    self.span_from(start),
                ));
            };
            match current {
                current if current == quote => return Ok(value),
                '\\' => {
                    let escape_start = self.mark();
                    let Some(escaped) = self.bump() else {
                        return Err(GrammarError::new(
                            GrammarErrorKind::UnterminatedString,
                            "unterminated escape sequence",
                            self.span_from(start),
                        ));
                    };
                    let decoded = match escaped {
                        '\\' => '\\',
                        '\'' => '\'',
                        '"' => '"',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '0' => '\0',
                        _ => {
                            return Err(GrammarError::new(
                                GrammarErrorKind::InvalidEscape,
                                format!("unsupported escape sequence '\\{escaped}'"),
                                self.span_from(escape_start),
                            ));
                        }
                    };
                    value.push(decoded);
                }
                '\n' | '\r' => {
                    return Err(GrammarError::new(
                        GrammarErrorKind::UnterminatedString,
                        "quoted symbols cannot cross a line",
                        self.span_from(start),
                    ));
                }
                _ => value.push(current),
            }
        }
    }

    fn skip_layout(&mut self) {
        // 两种行注释都属于文法布局，不产生词法单元。
        loop {
            self.take_while(char::is_whitespace);
            if self.peek() == Some('#') {
                self.take_while(|value| value != '\n');
                continue;
            }
            if self.source[self.offset..].starts_with("//") {
                self.take_while(|value| value != '\n');
                continue;
            }
            break;
        }
    }

    fn take_identifier(&mut self) -> String {
        self.take_while(is_identifier_continue)
    }

    fn take_while(&mut self, predicate: impl Fn(char) -> bool) -> String {
        let mut value = String::new();
        while let Some(current) = self.peek() {
            if !predicate(current) {
                break;
            }
            self.bump();
            value.push(current);
        }
        value
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let current = self.peek()?;
        self.offset += current.len_utf8();
        if current == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(current)
    }

    fn mark(&self) -> (usize, usize, usize) {
        (self.offset, self.line, self.column)
    }

    fn span_from(&self, (start, line, column): (usize, usize, usize)) -> SourceSpan {
        SourceSpan {
            start,
            end: self.offset,
            line,
            column,
        }
    }
}

fn is_identifier_start(value: char) -> bool {
    value.is_ascii_alphabetic() || value == '_'
}

fn is_identifier_continue(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '_' | '-' | '.')
}

struct Parser {
    lexemes: Vec<Lexeme>,
    cursor: usize,
}

impl Parser {
    fn new(source: &str) -> Result<Self, GrammarError> {
        Ok(Self {
            lexemes: Lexer::new(source).tokenize()?,
            cursor: 0,
        })
    }

    fn parse(mut self) -> Result<Grammar, GrammarError> {
        // `%%` 之前只接受声明，并在读取时同步建立唯一性索引，以便错误指向
        // 重复项而不是延迟到文法构造完成后才报告。
        let mut start: Option<(String, SourceSpan)> = None;
        let mut tokens = Vec::new();
        let mut precedence = Vec::new();
        let mut token_names: HashMap<String, SourceSpan> = HashMap::new();
        let mut token_numbers: HashMap<u32, (String, SourceSpan)> = HashMap::new();
        let mut token_literals: HashMap<String, (String, SourceSpan)> = HashMap::new();
        let mut precedence_symbols: HashMap<PrecedenceSymbol, SourceSpan> = HashMap::new();

        while !matches!(self.current().kind, LexemeKind::Sections | LexemeKind::End) {
            let directive = self.advance().clone();
            let LexemeKind::Directive(name) = &directive.kind else {
                return Err(self.unexpected("a declaration directive", &directive));
            };
            match name.as_str() {
                "start" => {
                    let (symbol, symbol_span) = self.expect_identifier("a start symbol")?;
                    self.expect_semicolon()?;
                    if let Some((_, previous)) = &start {
                        return Err(GrammarError::new(
                            GrammarErrorKind::DuplicateStart,
                            format!(
                                "duplicate start declaration; first declared at line {}",
                                previous.line
                            ),
                            directive.span,
                        ));
                    }
                    start = Some((symbol, symbol_span));
                }
                "token" => {
                    let (name, _) = self.expect_identifier("a token name")?;
                    let number_lexeme = self.advance().clone();
                    let LexemeKind::Number(number) = number_lexeme.kind else {
                        return Err(self.unexpected("a fixed token number", &number_lexeme));
                    };
                    let literal = match &self.current().kind {
                        LexemeKind::Quoted(literal) => {
                            let literal = literal.clone();
                            self.advance();
                            Some(literal)
                        }
                        _ => None,
                    };
                    let end = self.expect_semicolon()?;
                    let span = directive.span.through(end.span);

                    if let Some(previous) = token_names.get(&name) {
                        return Err(GrammarError::new(
                            GrammarErrorKind::DuplicateToken,
                            format!(
                                "duplicate token '{name}'; first declared at line {}",
                                previous.line
                            ),
                            directive.span,
                        ));
                    }
                    if number == 0 {
                        return Err(GrammarError::new(
                            GrammarErrorKind::ReservedTokenNumber,
                            "token number 0 is reserved for end-of-input",
                            number_lexeme.span,
                        ));
                    }
                    if let Some((previous_name, previous)) = token_numbers.get(&number) {
                        return Err(GrammarError::new(
                            GrammarErrorKind::DuplicateTokenNumber,
                            format!(
                                "token number {number} is already used by '{previous_name}' at line {}",
                                previous.line
                            ),
                            number_lexeme.span,
                        ));
                    }
                    if let Some(literal) = &literal
                        && let Some((previous_name, previous)) = token_literals.get(literal)
                    {
                        return Err(GrammarError::new(
                            GrammarErrorKind::DuplicateTokenLiteral,
                            format!(
                                "token literal {} is already used by '{previous_name}' at line {}",
                                display_literal(literal),
                                previous.line
                            ),
                            directive.span,
                        ));
                    }

                    token_names.insert(name.clone(), span);
                    token_numbers.insert(number, (name.clone(), span));
                    if let Some(literal) = &literal {
                        token_literals.insert(literal.clone(), (name.clone(), span));
                    }
                    tokens.push(Token {
                        name,
                        number,
                        literal,
                        span,
                    });
                }
                "left" | "right" | "nonassoc" | "precedence" => {
                    let associativity = match name.as_str() {
                        "left" => Associativity::Left,
                        "right" => Associativity::Right,
                        "nonassoc" => Associativity::NonAssoc,
                        _ => Associativity::PrecedenceOnly,
                    };
                    let mut symbols = Vec::new();
                    while !matches!(self.current().kind, LexemeKind::Semicolon | LexemeKind::End) {
                        let lexeme = self.advance().clone();
                        let symbol = match lexeme.kind {
                            LexemeKind::Identifier(name) => PrecedenceSymbol::Name(name),
                            LexemeKind::Quoted(literal) => PrecedenceSymbol::Literal(literal),
                            _ => return Err(self.unexpected("a precedence symbol", &lexeme)),
                        };
                        if let Some(previous) = precedence_symbols.get(&symbol) {
                            return Err(GrammarError::new(
                                GrammarErrorKind::DuplicatePrecedence,
                                format!(
                                    "duplicate precedence symbol {}; first declared at line {}",
                                    display_precedence_symbol(&symbol),
                                    previous.line
                                ),
                                lexeme.span,
                            ));
                        }
                        precedence_symbols.insert(symbol.clone(), lexeme.span);
                        symbols.push(symbol);
                    }
                    if symbols.is_empty() {
                        return Err(GrammarError::new(
                            GrammarErrorKind::UnexpectedToken,
                            format!("%{name} requires at least one symbol"),
                            directive.span,
                        ));
                    }
                    let end = self.expect_semicolon()?;
                    precedence.push(Precedence {
                        level: precedence.len() + 1,
                        associativity,
                        symbols,
                        span: directive.span.through(end.span),
                    });
                }
                _ => {
                    return Err(GrammarError::new(
                        GrammarErrorKind::UnknownDirective,
                        format!("unknown directive '%{name}'"),
                        directive.span,
                    ));
                }
            }
        }

        if matches!(self.current().kind, LexemeKind::End) {
            return Err(GrammarError::new(
                GrammarErrorKind::MissingGrammarSection,
                "missing '%%' grammar section separator",
                self.current().span,
            ));
        }
        self.advance();

        let Some((start, start_span)) = start else {
            return Err(GrammarError::new(
                GrammarErrorKind::MissingStart,
                "missing %start declaration",
                self.current().span,
            ));
        };

        let mut productions = Vec::new();
        let mut signatures: HashMap<String, SourceSpan> = HashMap::new();
        // 同一左部的 `|` 分支会分别展开成 Production；规范签名用于跨写法检测重复。
        while !matches!(self.current().kind, LexemeKind::End) {
            let (lhs, lhs_span) = self.expect_identifier("a production LHS")?;
            let colon = self.advance().clone();
            if !matches!(colon.kind, LexemeKind::Colon) {
                return Err(self.unexpected("':' after the production LHS", &colon));
            }

            loop {
                let (rhs, precedence, requires_action, alternative_end) =
                    self.parse_alternative()?;
                let signature = canonical_signature(&lhs, &rhs, precedence.as_ref());
                let span = lhs_span.through(alternative_end);
                if let Some(previous) = signatures.get(&signature) {
                    return Err(GrammarError::new(
                        GrammarErrorKind::DuplicateProduction,
                        format!(
                            "duplicate production '{signature}'; first declared at line {}",
                            previous.line
                        ),
                        span,
                    ));
                }
                signatures.insert(signature.clone(), span);
                productions.push(Production {
                    lhs: lhs.clone(),
                    rhs,
                    precedence,
                    requires_action,
                    span,
                    rule_id: rule_id(&signature),
                    signature,
                });

                match self.current().kind {
                    LexemeKind::Pipe => {
                        self.advance();
                    }
                    LexemeKind::Semicolon => {
                        self.advance();
                        break;
                    }
                    _ => {
                        let current = self.current().clone();
                        return Err(self.unexpected("'|' or ';' after a production", &current));
                    }
                }
            }
        }

        if productions.is_empty() {
            return Err(GrammarError::new(
                GrammarErrorKind::MissingProduction,
                "grammar must contain at least one production",
                self.current().span,
            ));
        }

        validate_references(&start, start_span, &tokens, &precedence, &productions)?;

        Ok(Grammar {
            start,
            start_span,
            tokens,
            precedence,
            productions,
        })
    }

    fn parse_alternative(
        &mut self,
    ) -> Result<
        (
            Vec<ProductionItem>,
            Option<PrecedenceSymbol>,
            bool,
            SourceSpan,
        ),
        GrammarError,
    > {
        // 尾部动作只设置 `requires_action`；规则中间动作还要作为带位置的右部元素，
        // 从而参与签名和后续自动机生成。ε 不进入右部，并且不能与其他符号混用。
        let mut rhs = Vec::new();
        let mut symbol_count = 0;
        let mut precedence = None;
        let mut requires_action = false;
        let mut saw_epsilon = false;
        let mut end = self.current().span;

        while !matches!(
            self.current().kind,
            LexemeKind::Pipe | LexemeKind::Semicolon | LexemeKind::End
        ) {
            let lexeme = self.advance().clone();
            end = lexeme.span;
            match lexeme.kind {
                LexemeKind::Identifier(symbol) => {
                    if saw_epsilon {
                        return Err(GrammarError::new(
                            GrammarErrorKind::UnexpectedToken,
                            "epsilon must be the only RHS symbol",
                            lexeme.span,
                        ));
                    }
                    rhs.push(ProductionItem::Symbol(symbol));
                    symbol_count += 1;
                }
                LexemeKind::Quoted(literal) => {
                    if saw_epsilon {
                        return Err(GrammarError::new(
                            GrammarErrorKind::UnexpectedToken,
                            "epsilon must be the only RHS symbol",
                            lexeme.span,
                        ));
                    }
                    rhs.push(ProductionItem::Literal(literal));
                    symbol_count += 1;
                }
                LexemeKind::Epsilon => {
                    if symbol_count > 0 || saw_epsilon {
                        return Err(GrammarError::new(
                            GrammarErrorKind::UnexpectedToken,
                            "epsilon must be the only RHS symbol",
                            lexeme.span,
                        ));
                    }
                    saw_epsilon = true;
                }
                LexemeKind::Action => {
                    requires_action = true;
                    if !matches!(
                        self.current().kind,
                        LexemeKind::Pipe | LexemeKind::Semicolon | LexemeKind::End
                    ) {
                        if saw_epsilon {
                            return Err(GrammarError::new(
                                GrammarErrorKind::UnexpectedToken,
                                "epsilon cannot precede a mid-rule action",
                                lexeme.span,
                            ));
                        }
                        rhs.push(ProductionItem::Action {
                            position: symbol_count,
                        });
                    }
                }
                LexemeKind::Directive(name) if name == "prec" => {
                    if precedence.is_some() {
                        return Err(GrammarError::new(
                            GrammarErrorKind::UnexpectedToken,
                            "a production may only declare one %prec override",
                            lexeme.span,
                        ));
                    }
                    let symbol = self.advance().clone();
                    end = symbol.span;
                    precedence = Some(match symbol.kind {
                        LexemeKind::Identifier(name) => PrecedenceSymbol::Name(name),
                        LexemeKind::Quoted(literal) => PrecedenceSymbol::Literal(literal),
                        _ => return Err(self.unexpected("a precedence symbol", &symbol)),
                    });
                }
                _ => return Err(self.unexpected("an RHS symbol or @action", &lexeme)),
            }
        }

        Ok((rhs, precedence, requires_action, end))
    }

    fn expect_identifier(&mut self, expected: &str) -> Result<(String, SourceSpan), GrammarError> {
        let lexeme = self.advance().clone();
        match lexeme.kind {
            LexemeKind::Identifier(value) => Ok((value, lexeme.span)),
            _ => Err(self.unexpected(expected, &lexeme)),
        }
    }

    fn expect_semicolon(&mut self) -> Result<Lexeme, GrammarError> {
        let lexeme = self.advance().clone();
        if matches!(lexeme.kind, LexemeKind::Semicolon) {
            Ok(lexeme)
        } else {
            Err(self.unexpected("';' after the declaration", &lexeme))
        }
    }

    fn current(&self) -> &Lexeme {
        &self.lexemes[self.cursor]
    }

    fn advance(&mut self) -> &Lexeme {
        let current = self.cursor;
        if !matches!(self.lexemes[current].kind, LexemeKind::End) {
            self.cursor += 1;
        }
        &self.lexemes[current]
    }

    fn unexpected(&self, expected: &str, actual: &Lexeme) -> GrammarError {
        GrammarError::new(
            GrammarErrorKind::UnexpectedToken,
            format!(
                "expected {expected}, found {}",
                describe_lexeme(&actual.kind)
            ),
            actual.span,
        )
    }
}

fn validate_references(
    start: &str,
    start_span: SourceSpan,
    tokens: &[Token],
    precedence: &[Precedence],
    productions: &[Production],
) -> Result<(), GrammarError> {
    // 语法结构解析完成后统一解析名称：起始符必须是非终结符，优先级只能引用
    // 已声明终结符，而产生式右部可以引用终结符或其他产生式定义的非终结符。
    let token_names: HashSet<_> = tokens.iter().map(|token| token.name.as_str()).collect();
    let token_literals: HashSet<_> = tokens
        .iter()
        .filter_map(|token| token.literal.as_deref())
        .collect();
    let nonterminals: HashSet<_> = productions
        .iter()
        .map(|production| production.lhs.as_str())
        .collect();

    if !nonterminals.contains(start) {
        return Err(GrammarError::new(
            GrammarErrorKind::UnknownStartSymbol,
            format!("start symbol '{start}' has no production"),
            start_span,
        ));
    }

    for declaration in precedence {
        for symbol in &declaration.symbols {
            let known = match symbol {
                PrecedenceSymbol::Name(name) => token_names.contains(name.as_str()),
                PrecedenceSymbol::Literal(literal) => token_literals.contains(literal.as_str()),
            };
            if !known {
                return Err(GrammarError::new(
                    GrammarErrorKind::UnknownSymbol,
                    format!(
                        "precedence symbol {} is not a declared token",
                        display_precedence_symbol(symbol)
                    ),
                    declaration.span,
                ));
            }
        }
    }

    for production in productions {
        if let Some(symbol) = &production.precedence
            && !precedence
                .iter()
                .flat_map(|declaration| &declaration.symbols)
                .any(|declared| declared == symbol)
        {
            return Err(GrammarError::new(
                GrammarErrorKind::UnknownSymbol,
                format!(
                    "production '{}' uses undeclared precedence {}",
                    production.signature,
                    display_precedence_symbol(symbol)
                ),
                production.span,
            ));
        }
        for item in &production.rhs {
            let known = match item {
                ProductionItem::Symbol(name) => {
                    token_names.contains(name.as_str()) || nonterminals.contains(name.as_str())
                }
                ProductionItem::Literal(literal) => token_literals.contains(literal.as_str()),
                ProductionItem::Action { .. } => true,
            };
            if !known {
                return Err(GrammarError::new(
                    GrammarErrorKind::UnknownSymbol,
                    format!(
                        "production '{}' references undeclared symbol {}",
                        production.signature,
                        display_production_item(item)
                    ),
                    production.span,
                ));
            }
        }
    }

    Ok(())
}

fn canonical_signature(
    lhs: &str,
    rhs: &[ProductionItem],
    precedence: Option<&PrecedenceSymbol>,
) -> String {
    // 规范签名显式保留 ε、中间动作位置和 `%prec` 覆盖，避免语义不同的规则碰撞。
    let mut signature = format!("{lhs} ->");
    if rhs.is_empty() {
        signature.push_str(" ε");
    } else {
        for item in rhs {
            signature.push(' ');
            match item {
                ProductionItem::Symbol(symbol) => signature.push_str(symbol),
                ProductionItem::Literal(literal) => {
                    signature.push_str(&display_literal(literal));
                }
                ProductionItem::Action { position } => {
                    signature.push('@');
                    signature.push_str(&position.to_string());
                }
            }
        }
    }
    if let Some(precedence) = precedence {
        signature.push_str(" %prec ");
        signature.push_str(&display_precedence_symbol(precedence));
    }
    signature
}

fn rule_id(signature: &str) -> RuleId {
    let mut prefix = String::new();
    for current in signature.chars() {
        if current.is_ascii_alphanumeric() {
            prefix.push(current.to_ascii_lowercase());
        } else if !prefix.ends_with('_') {
            prefix.push('_');
        }
        if prefix.len() >= 48 {
            break;
        }
    }
    while prefix.ends_with('_') {
        prefix.pop();
    }
    if prefix.is_empty() {
        prefix.push_str("rule");
    }

    // FNV-1a 实现紧凑且结果确定，明确不依赖进程哈希随机化或产生式顺序。
    let mut digest = 0xcbf29ce484222325_u64;
    for byte in signature.as_bytes() {
        digest ^= u64::from(*byte);
        digest = digest.wrapping_mul(0x100000001b3);
    }
    RuleId(format!("{prefix}--{digest:016x}"))
}

fn display_literal(literal: &str) -> String {
    let mut displayed = String::from("\"");
    for current in literal.chars() {
        match current {
            '\\' => displayed.push_str("\\\\"),
            '"' => displayed.push_str("\\\""),
            '\n' => displayed.push_str("\\n"),
            '\r' => displayed.push_str("\\r"),
            '\t' => displayed.push_str("\\t"),
            '\0' => displayed.push_str("\\0"),
            _ => displayed.push(current),
        }
    }
    displayed.push('"');
    displayed
}

fn display_precedence_symbol(symbol: &PrecedenceSymbol) -> String {
    match symbol {
        PrecedenceSymbol::Name(name) => format!("'{name}'"),
        PrecedenceSymbol::Literal(literal) => display_literal(literal),
    }
}

fn display_production_item(item: &ProductionItem) -> String {
    match item {
        ProductionItem::Symbol(symbol) => format!("'{symbol}'"),
        ProductionItem::Literal(literal) => display_literal(literal),
        ProductionItem::Action { position } => format!("'@{position}'"),
    }
}

fn describe_lexeme(kind: &LexemeKind) -> String {
    match kind {
        LexemeKind::Directive(name) => format!("'%{name}'"),
        LexemeKind::Sections => "'%%'".into(),
        LexemeKind::Identifier(name) => format!("identifier '{name}'"),
        LexemeKind::Number(number) => format!("number {number}"),
        LexemeKind::Quoted(literal) => display_literal(literal),
        LexemeKind::Colon => "':'".into(),
        LexemeKind::Pipe => "'|'".into(),
        LexemeKind::Semicolon => "';'".into(),
        LexemeKind::Action => "'@action'".into(),
        LexemeKind::Epsilon => "'ε'".into(),
        LexemeKind::End => "end of input".into(),
    }
}
