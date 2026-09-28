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
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 一个不依赖 Yacc、没有规约编号和状态表的 SQL parser 小型示例。
// 语句使用递归下降解析，表达式使用 Pratt parser 处理优先级。
//
// Pratt parser（算符优先级解析）用绑定力决定二元运算符的结合顺序，
// 无需为每层优先级单独写一套递归下降函数。

use std::fmt;

/// 词法单元种类：关键字、字面量、运算符与结束标记。
#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Select,
    Create,
    Table,
    Date,
    Time,
    Timestamp,
    Identifier(String),
    Number(i64),
    String(String),
    Plus,
    Minus,
    Star,
    Slash,
    LeftParen,
    RightParen,
    Comma,
    Semicolon,
    End,
}

/// 带源码字节偏移的词法单元，报错时用于定位。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    offset: usize,
}

/// 顶层语句 AST：仅演示 SELECT 表达式列表与 CREATE TABLE。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Statement {
    Select(Vec<Expr>),
    CreateTable { name: String },
}

/// DATE / TIME / TIMESTAMP 类型字面量标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    Date,
    Time,
    Timestamp,
}

/// 表达式 AST：标识符、字面量、一元/二元运算。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Identifier(String),
    Number(i64),
    String(String),
    TypedLiteral {
        kind: LiteralKind,
        value: String,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
}

/// 一元加减运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnaryOp {
    Plus,
    Minus,
}

/// 二元四则运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// 解析错误：记录失败位置与说明。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParseError {
    offset: usize,
    message: String,
}

impl ParseError {
    /// 构造带字节偏移的解析错误。
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.offset)
    }
}

/// 将 SQL 文本切分为 Token 序列，末尾追加 End。
fn lex(sql: &str) -> Result<Vec<Token>, ParseError> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut offset = 0;

    // 逐字节扫描：空白跳过，单字符运算符直接产出，字符串/数字/标识符按规则吞并。
    while offset < bytes.len() {
        if bytes[offset].is_ascii_whitespace() {
            offset += 1;
            continue;
        }

        let start = offset;
        let kind = match bytes[offset] {
            b'+' => {
                offset += 1;
                TokenKind::Plus
            }
            b'-' => {
                offset += 1;
                TokenKind::Minus
            }
            b'*' => {
                offset += 1;
                TokenKind::Star
            }
            b'/' => {
                offset += 1;
                TokenKind::Slash
            }
            b'(' => {
                offset += 1;
                TokenKind::LeftParen
            }
            b')' => {
                offset += 1;
                TokenKind::RightParen
            }
            b',' => {
                offset += 1;
                TokenKind::Comma
            }
            b';' => {
                offset += 1;
                TokenKind::Semicolon
            }
            b'\'' => {
                offset += 1;
                let value_start = offset;
                while offset < bytes.len() && bytes[offset] != b'\'' {
                    offset += 1;
                }
                if offset == bytes.len() {
                    return Err(ParseError::new(start, "unterminated string literal"));
                }
                let value = sql[value_start..offset].to_owned();
                offset += 1;
                TokenKind::String(value)
            }
            byte if byte.is_ascii_digit() => {
                offset += 1;
                while offset < bytes.len() && bytes[offset].is_ascii_digit() {
                    offset += 1;
                }
                let number = sql[start..offset]
                    .parse::<i64>()
                    .map_err(|_| ParseError::new(start, "integer literal is out of range"))?;
                TokenKind::Number(number)
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                offset += 1;
                while offset < bytes.len()
                    && (bytes[offset].is_ascii_alphanumeric() || bytes[offset] == b'_')
                {
                    offset += 1;
                }
                let word = &sql[start..offset];
                match word.to_ascii_uppercase().as_str() {
                    "SELECT" => TokenKind::Select,
                    "CREATE" => TokenKind::Create,
                    "TABLE" => TokenKind::Table,
                    "DATE" => TokenKind::Date,
                    "TIME" => TokenKind::Time,
                    "TIMESTAMP" => TokenKind::Timestamp,
                    _ => TokenKind::Identifier(word.to_owned()),
                }
            }
            _ => {
                return Err(ParseError::new(
                    start,
                    format!("unexpected character {:?}", bytes[start] as char),
                ));
            }
        };
        tokens.push(Token {
            kind,
            offset: start,
        });
    }

    tokens.push(Token {
        kind: TokenKind::End,
        offset: sql.len(),
    });
    Ok(tokens)
}

/// 递归下降语句解析器，内嵌 Pratt 表达式解析。
struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser {
    /// 从词法结果构造解析器，游标从 0 开始。
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, cursor: 0 }
    }

    /// 解析单条语句，允许可选分号，并要求到达 End。
    fn parse(mut self) -> Result<Statement, ParseError> {
        let statement = self.parse_statement()?;
        if self.at(&TokenKind::Semicolon) {
            self.bump();
        }
        self.expect(&TokenKind::End)?;
        Ok(statement)
    }

    /// 按首 token 分派到 SELECT 或 CREATE TABLE。
    fn parse_statement(&mut self) -> Result<Statement, ParseError> {
        match self.current_kind() {
            TokenKind::Select => self.parse_select(),
            TokenKind::Create => self.parse_create_table(),
            _ => Err(self.error("expected SELECT or CREATE TABLE")),
        }
    }

    /// 解析 `SELECT expr [, expr]*`。
    fn parse_select(&mut self) -> Result<Statement, ParseError> {
        self.expect(&TokenKind::Select)?;
        let mut expressions = vec![self.parse_expression(0)?];
        while self.at(&TokenKind::Comma) {
            self.bump();
            expressions.push(self.parse_expression(0)?);
        }
        Ok(Statement::Select(expressions))
    }

    /// 解析 `CREATE TABLE ident`（演示用，无列定义）。
    fn parse_create_table(&mut self) -> Result<Statement, ParseError> {
        self.expect(&TokenKind::Create)?;
        self.expect(&TokenKind::Table)?;
        let name = self.take_identifier()?;
        Ok(Statement::CreateTable { name })
    }

    // Pratt parser：新增二元运算符时，只需在 infix_binding_power 增加一项。
    /// 按最小绑定力解析表达式；左结合通过抬高右侧递归下限实现。
    fn parse_expression(&mut self, minimum_power: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_prefix_expression()?;

        loop {
            let Some((operator, left_power, right_power)) =
                infix_binding_power(self.current_kind())
            else {
                break;
            };
            if left_power < minimum_power {
                break;
            }

            self.bump();
            let right = self.parse_expression(right_power)?;
            left = Expr::Binary {
                left: Box::new(left),
                op: operator,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// 解析前缀表达式：字面量、一元加减、括号或类型字面量。
    fn parse_prefix_expression(&mut self) -> Result<Expr, ParseError> {
        let token = self.bump().clone();
        match token.kind {
            TokenKind::Identifier(name) => Ok(Expr::Identifier(name)),
            TokenKind::Number(value) => Ok(Expr::Number(value)),
            TokenKind::String(value) => Ok(Expr::String(value)),
            TokenKind::Date => self.parse_typed_literal(LiteralKind::Date),
            TokenKind::Time => self.parse_typed_literal(LiteralKind::Time),
            TokenKind::Timestamp => self.parse_typed_literal(LiteralKind::Timestamp),
            TokenKind::Plus => Ok(Expr::Unary {
                op: UnaryOp::Plus,
                expr: Box::new(self.parse_expression(5)?),
            }),
            TokenKind::Minus => Ok(Expr::Unary {
                op: UnaryOp::Minus,
                expr: Box::new(self.parse_expression(5)?),
            }),
            TokenKind::LeftParen => {
                let expression = self.parse_expression(0)?;
                self.expect(&TokenKind::RightParen)?;
                Ok(expression)
            }
            _ => Err(ParseError::new(token.offset, "expected expression")),
        }
    }

    /// 解析 `DATE|TIME|TIMESTAMP '...'` 形式的类型字面量。
    fn parse_typed_literal(&mut self, kind: LiteralKind) -> Result<Expr, ParseError> {
        let token = self.bump().clone();
        let TokenKind::String(value) = token.kind else {
            return Err(ParseError::new(
                token.offset,
                "typed literal requires a string",
            ));
        };
        Ok(Expr::TypedLiteral { kind, value })
    }

    /// 消费一个 Identifier token 并返回其名称。
    fn take_identifier(&mut self) -> Result<String, ParseError> {
        let token = self.bump().clone();
        match token.kind {
            TokenKind::Identifier(name) => Ok(name),
            _ => Err(ParseError::new(token.offset, "expected identifier")),
        }
    }

    /// 当前游标处的 Token。
    fn current(&self) -> &Token {
        &self.tokens[self.cursor]
    }

    /// 当前 Token 的种类。
    fn current_kind(&self) -> &TokenKind {
        &self.current().kind
    }

    /// 判断当前 Token 是否等于期望种类。
    fn at(&self, expected: &TokenKind) -> bool {
        self.current_kind() == expected
    }

    /// 消费当前 Token；已在 End 时不前进游标。
    fn bump(&mut self) -> &Token {
        let index = self.cursor;
        if !matches!(self.tokens[index].kind, TokenKind::End) {
            self.cursor += 1;
        }
        &self.tokens[index]
    }

    /// 要求当前为期望 Token，否则报错。
    fn expect(&mut self, expected: &TokenKind) -> Result<(), ParseError> {
        if self.at(expected) {
            self.bump();
            Ok(())
        } else {
            Err(self.error(format!("expected {expected:?}")))
        }
    }

    /// 在当前 Token 偏移处构造解析错误。
    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError::new(self.current().offset, message)
    }
}

/// 中缀运算符的左右绑定力；乘除高于加减，同级左结合。
fn infix_binding_power(token: &TokenKind) -> Option<(BinaryOp, u8, u8)> {
    match token {
        TokenKind::Plus => Some((BinaryOp::Add, 1, 2)),
        TokenKind::Minus => Some((BinaryOp::Subtract, 1, 2)),
        TokenKind::Star => Some((BinaryOp::Multiply, 3, 4)),
        TokenKind::Slash => Some((BinaryOp::Divide, 3, 4)),
        _ => None,
    }
}

/// 词法 + 语法一体化入口：成功返回 Statement AST。
fn parse(sql: &str) -> Result<Statement, ParseError> {
    Parser::new(lex(sql)?).parse()
}

/// 演示 SELECT 表达式与 CREATE TABLE 两条样例 SQL。
fn main() {
    for sql in [
        "SELECT DATE '2026-07-13', price + 2 * 3;",
        "CREATE TABLE users;",
    ] {
        match parse(sql) {
            Ok(statement) => println!("SQL: {sql}\nAST: {statement:#?}\n"),
            Err(error) => eprintln!("SQL: {sql}\nERROR: {error}\n"),
        }
    }
}
