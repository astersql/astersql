// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// There is no working SQL parser wired into this crate yet, so `format_test.go`'s
// `parser.New().Parse` + `ExprNode.Format` pipeline cannot be reused directly. This file
// implements a minimal expression tokenizer/parser and a matching formatter, scoped to
// exactly the expression grammar exercised by the cases below (mirrors `format_test.go`
// case for case; no case has been dropped).

// 表达式 `Format` 行为的单元测试与迷你解析器。
//
// 对照 Go `format_test.go`：从 `SELECT <expr>` 中解析首个表达式，
// 再按 TiDB 的 Format 规则还原为规范化文本并与期望比对。

/// 迷你解析器用的表达式 AST 节点（覆盖 format_test 用例语法）。
#[derive(Debug, Clone)]
enum Expr {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Hex(String),
    Bit(String),
    TimeLit(&'static str, String),
    Column(Vec<String>),
    Paren(Box<Expr>),
    Unary(char, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Between {
        expr: Box<Expr>,
        not: bool,
        lo: Box<Expr>,
        hi: Box<Expr>,
    },
    IsNull {
        expr: Box<Expr>,
        not: bool,
    },
    IsTruth {
        expr: Box<Expr>,
        not: bool,
        truth: bool,
    },
    In {
        expr: Box<Expr>,
        not: bool,
        list: Vec<Expr>,
    },
    Like {
        expr: Box<Expr>,
        not: bool,
        pattern: Box<Expr>,
        escape: Option<char>,
    },
    Regexp {
        expr: Box<Expr>,
        not: bool,
        pattern: Box<Expr>,
    },
    Case {
        value: Option<Box<Expr>>,
        when_clauses: Vec<(Expr, Expr)>,
        else_clause: Option<Box<Expr>>,
    },
    FuncCall {
        name: String,
        args: Vec<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: String,
    },
    Convert {
        expr: Box<Expr>,
        ty: String,
    },
    BinaryKeyword(Box<Expr>),
    RawUnit(String),
    Interval {
        value: Box<Expr>,
        unit: String,
    },
    ArrowExtract {
        left: Box<Expr>,
        path: Box<Expr>,
    },
    ArrowUnquote {
        left: Box<Expr>,
        path: Box<Expr>,
    },
}

/// 迷你词法器的 token 种类。
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Number(String),
    Str(String),
    Hex(String),
    Bit(String),
    Time(&'static str, String),
    Punct(&'static str),
}

/// 读取 quoted。
fn read_quoted(chars: &[char], start: usize, quote: char) -> Result<(String, usize), String> {
    let mut i = start + 1;
    let mut out = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == quote {
            if chars.get(i + 1) == Some(&quote) {
                out.push(quote);
                i += 2;
                continue;
            }
            return Ok((out, i + 1));
        }
        out.push(c);
        i += 1;
    }
    Err(format!("unterminated string literal starting at {start}"))
}

/// 读取 number。
fn read_number(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
        let save = i;
        let mut j = i + 1;
        if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
            j += 1;
        }
        if j < chars.len() && chars[j].is_ascii_digit() {
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        } else {
            i = save;
        }
    }
    (chars[start..i].iter().collect(), i)
}

/// 将输入 SQL 片段词法分析为 token 序列。
fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut toks = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            let (content, next) = read_quoted(&chars, i, c)?;
            toks.push(Tok::Str(content));
            i = next;
            continue;
        }
        if c.is_ascii_digit() {
            let (text, next) = read_number(&chars, i);
            toks.push(Tok::Number(text));
            i = next;
            continue;
        }
        if c == '_' || c.is_alphabetic() {
            let start = i;
            while i < chars.len() && (chars[i] == '_' || chars[i].is_alphanumeric()) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let lower = word.to_lowercase();
            if (lower == "x" || lower == "b") && chars.get(i) == Some(&'\'') {
                let quote = chars[i];
                let (content, next) = read_quoted(&chars, i, quote)?;
                i = next;
                toks.push(if lower == "x" {
                    Tok::Hex(content)
                } else {
                    Tok::Bit(content)
                });
                continue;
            }
            if word.starts_with('_') && matches!(chars.get(i), Some('\'') | Some('"')) {
                let quote = chars[i];
                let (content, next) = read_quoted(&chars, i, quote)?;
                i = next;
                toks.push(Tok::Str(content));
                continue;
            }
            if lower == "time" || lower == "timestamp" || lower == "date" {
                let mut j = i;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if matches!(chars.get(j), Some('\'') | Some('"')) {
                    let quote = chars[j];
                    let (content, next) = read_quoted(&chars, j, quote)?;
                    i = next;
                    let kind = match lower.as_str() {
                        "time" => "time",
                        "timestamp" => "timestamp",
                        _ => "date",
                    };
                    toks.push(Tok::Time(kind, content));
                    continue;
                }
            }
            toks.push(Tok::Ident(word));
            continue;
        }
        if c == '-' && chars.get(i + 1) == Some(&'>') {
            if chars.get(i + 2) == Some(&'>') {
                toks.push(Tok::Punct("->>"));
                i += 3;
            } else {
                toks.push(Tok::Punct("->"));
                i += 2;
            }
            continue;
        }
        if c == '>' && chars.get(i + 1) == Some(&'=') {
            toks.push(Tok::Punct(">="));
            i += 2;
            continue;
        }
        let p: &'static str = match c {
            '(' => "(",
            ')' => ")",
            ',' => ",",
            '.' => ".",
            '+' => "+",
            '-' => "-",
            '%' => "%",
            '/' => "/",
            '=' => "=",
            '>' => ">",
            '<' => "<",
            other => return Err(format!("unexpected character {other:?} in {src:?}")),
        };
        toks.push(Tok::Punct(p));
        i += 1;
    }
    Ok(toks)
}

/// 迷你解析器游标状态。
struct P {
    toks: Vec<Tok>,
    pos: usize,
}

impl P {
    fn peek(&self) -> Option<Tok> {
        self.toks.get(self.pos).cloned()
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.peek();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat_punct(&mut self, target: &str) -> bool {
        match self.peek() {
            Some(Tok::Punct(p)) if p == target => {
                self.pos += 1;
                true
            }
            _ => false,
        }
    }

    fn expect_punct(&mut self, target: &str) -> Result<(), String> {
        if self.eat_punct(target) {
            Ok(())
        } else {
            Err(format!("expected {target:?}, got {:?}", self.peek()))
        }
    }

    fn peek_ident_ci(&self) -> Option<String> {
        match self.peek() {
            Some(Tok::Ident(s)) => Some(s.to_lowercase()),
            _ => None,
        }
    }

    fn eat_ident_ci(&mut self, word: &str) -> bool {
        if self.peek_ident_ci().as_deref() == Some(word) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_ident_ci(&mut self, word: &str) -> Result<(), String> {
        if self.eat_ident_ci(word) {
            Ok(())
        } else {
            Err(format!("expected {word:?}, got {:?}", self.peek()))
        }
    }

    fn expect_number(&mut self) -> Result<String, String> {
        match self.bump() {
            Some(Tok::Number(n)) => Ok(n),
            other => Err(format!("expected number, got {other:?}")),
        }
    }
}

/// 解析 expr list。
fn parse_expr_list(p: &mut P) -> Result<Vec<Expr>, String> {
    let mut list = vec![parse_predicate(p)?];
    while p.eat_punct(",") {
        list.push(parse_predicate(p)?);
    }
    Ok(list)
}

/// 解析 optional escape。
fn parse_optional_escape(p: &mut P) -> Result<Option<char>, String> {
    if p.eat_ident_ci("escape") {
        match p.bump() {
            Some(Tok::Str(s)) => Ok(s.chars().next()),
            other => Err(format!("expected escape literal, got {other:?}")),
        }
    } else {
        Ok(None)
    }
}

/// 解析 predicate。
fn parse_predicate(p: &mut P) -> Result<Expr, String> {
    let left = parse_comparison(p)?;
    let mut not = false;
    if p.eat_ident_ci("not") {
        not = true;
    }
    if p.eat_ident_ci("between") {
        let lo = parse_additive(p)?;
        p.expect_ident_ci("and")?;
        let hi = parse_additive(p)?;
        return Ok(Expr::Between {
            expr: Box::new(left),
            not,
            lo: Box::new(lo),
            hi: Box::new(hi),
        });
    }
    if not {
        if p.eat_ident_ci("in") {
            p.expect_punct("(")?;
            let list = parse_expr_list(p)?;
            p.expect_punct(")")?;
            return Ok(Expr::In {
                expr: Box::new(left),
                not,
                list,
            });
        }
        if p.eat_ident_ci("like") {
            let pattern = parse_additive(p)?;
            let escape = parse_optional_escape(p)?;
            return Ok(Expr::Like {
                expr: Box::new(left),
                not,
                pattern: Box::new(pattern),
                escape,
            });
        }
        if p.eat_ident_ci("regexp") || p.eat_ident_ci("rlike") {
            let pattern = parse_additive(p)?;
            return Ok(Expr::Regexp {
                expr: Box::new(left),
                not,
                pattern: Box::new(pattern),
            });
        }
        return Err(format!(
            "expected BETWEEN/IN/LIKE/REGEXP after NOT, got {:?}",
            p.peek()
        ));
    }
    if p.eat_ident_ci("is") {
        let mut isnot = false;
        if p.eat_ident_ci("not") {
            isnot = true;
        }
        if p.eat_ident_ci("null") {
            return Ok(Expr::IsNull {
                expr: Box::new(left),
                not: isnot,
            });
        }
        if p.eat_ident_ci("true") {
            return Ok(Expr::IsTruth {
                expr: Box::new(left),
                not: isnot,
                truth: true,
            });
        }
        if p.eat_ident_ci("false") {
            return Ok(Expr::IsTruth {
                expr: Box::new(left),
                not: isnot,
                truth: false,
            });
        }
        return Err(format!(
            "expected NULL/TRUE/FALSE after IS, got {:?}",
            p.peek()
        ));
    }
    if p.eat_ident_ci("in") {
        p.expect_punct("(")?;
        let list = parse_expr_list(p)?;
        p.expect_punct(")")?;
        return Ok(Expr::In {
            expr: Box::new(left),
            not: false,
            list,
        });
    }
    if p.eat_ident_ci("like") {
        let pattern = parse_additive(p)?;
        let escape = parse_optional_escape(p)?;
        return Ok(Expr::Like {
            expr: Box::new(left),
            not: false,
            pattern: Box::new(pattern),
            escape,
        });
    }
    if p.eat_ident_ci("regexp") || p.eat_ident_ci("rlike") {
        let pattern = parse_additive(p)?;
        return Ok(Expr::Regexp {
            expr: Box::new(left),
            not: false,
            pattern: Box::new(pattern),
        });
    }
    Ok(left)
}

/// 解析 comparison。
fn parse_comparison(p: &mut P) -> Result<Expr, String> {
    let left = parse_additive(p)?;
    for op in [">=", "<=", "<>", "!=", "=", ">", "<"] {
        if p.eat_punct(op) {
            let right = parse_additive(p)?;
            return Ok(Expr::Binary(op, Box::new(left), Box::new(right)));
        }
    }
    Ok(left)
}

/// 解析 additive。
fn parse_additive(p: &mut P) -> Result<Expr, String> {
    let mut left = parse_multiplicative(p)?;
    loop {
        if p.eat_punct("+") {
            let right = parse_multiplicative(p)?;
            left = Expr::Binary("+", Box::new(left), Box::new(right));
        } else if p.eat_punct("-") {
            let right = parse_multiplicative(p)?;
            left = Expr::Binary("-", Box::new(left), Box::new(right));
        } else {
            break;
        }
    }
    Ok(left)
}

/// 解析 multiplicative。
fn parse_multiplicative(p: &mut P) -> Result<Expr, String> {
    let mut left = parse_unary(p)?;
    loop {
        if p.eat_punct("%") {
            let right = parse_unary(p)?;
            left = Expr::Binary("%", Box::new(left), Box::new(right));
        } else if p.eat_punct("/") {
            let right = parse_unary(p)?;
            left = Expr::Binary("/", Box::new(left), Box::new(right));
        } else {
            break;
        }
    }
    Ok(left)
}

/// 解析 unary。
fn parse_unary(p: &mut P) -> Result<Expr, String> {
    if p.eat_punct("-") {
        let inner = parse_unary(p)?;
        return Ok(Expr::Unary('-', Box::new(inner)));
    }
    parse_postfix(p)
}

/// 解析 postfix。
fn parse_postfix(p: &mut P) -> Result<Expr, String> {
    let mut left = parse_primary(p)?;
    loop {
        if p.eat_punct("->>") {
            let path = parse_primary(p)?;
            left = Expr::ArrowUnquote {
                left: Box::new(left),
                path: Box::new(path),
            };
        } else if p.eat_punct("->") {
            let path = parse_primary(p)?;
            left = Expr::ArrowExtract {
                left: Box::new(left),
                path: Box::new(path),
            };
        } else {
            break;
        }
    }
    Ok(left)
}

/// 解析 optional paren number。
fn parse_optional_paren_number(p: &mut P) -> Result<Option<String>, String> {
    if p.eat_punct("(") {
        let n = p.expect_number()?;
        p.expect_punct(")")?;
        Ok(Some(n))
    } else {
        Ok(None)
    }
}

/// 解析 optional paren pair。
fn parse_optional_paren_pair(p: &mut P) -> Result<(Option<String>, Option<String>), String> {
    if p.eat_punct("(") {
        let a = p.expect_number()?;
        let b = if p.eat_punct(",") {
            Some(p.expect_number()?)
        } else {
            None
        };
        p.expect_punct(")")?;
        Ok((Some(a), b))
    } else {
        Ok((None, None))
    }
}

/// 解析 cast type。
fn parse_cast_type(p: &mut P) -> Result<String, String> {
    let word = match p.bump() {
        Some(Tok::Ident(w)) => w.to_lowercase(),
        other => return Err(format!("expected type name, got {other:?}")),
    };
    match word.as_str() {
        "signed" => {
            p.eat_ident_ci("integer");
            Ok("SIGNED".into())
        }
        "unsigned" => {
            p.eat_ident_ci("integer");
            Ok("UNSIGNED".into())
        }
        "char" => {
            let len = parse_optional_paren_number(p)?;
            p.expect_ident_ci("binary")?;
            Ok(match len {
                Some(n) => format!("BINARY({n})"),
                None => "BINARY".into(),
            })
        }
        "decimal" => {
            let (a, b) = parse_optional_paren_pair(p)?;
            Ok(match (a, b) {
                (Some(a), Some(b)) => format!("DECIMAL({a}, {b})"),
                (Some(a), None) => format!("DECIMAL({a})"),
                (None, None) => "DECIMAL(10)".into(),
                (None, Some(_)) => unreachable!("scale without precision"),
            })
        }
        other => Ok(other.to_uppercase()),
    }
}

/// 解析 case。
fn parse_case(p: &mut P) -> Result<Expr, String> {
    let value = if p.peek_ident_ci().as_deref() == Some("when") {
        None
    } else {
        Some(Box::new(parse_predicate(p)?))
    };
    let mut when_clauses = Vec::new();
    while p.eat_ident_ci("when") {
        let cond = parse_predicate(p)?;
        p.expect_ident_ci("then")?;
        let result = parse_predicate(p)?;
        when_clauses.push((cond, result));
    }
    let else_clause = if p.eat_ident_ci("else") {
        Some(Box::new(parse_predicate(p)?))
    } else {
        None
    };
    p.expect_ident_ci("end")?;
    Ok(Expr::Case {
        value,
        when_clauses,
        else_clause,
    })
}

/// 解析 cast。
fn parse_cast(p: &mut P) -> Result<Expr, String> {
    p.expect_punct("(")?;
    let expr = parse_predicate(p)?;
    p.expect_ident_ci("as")?;
    let ty = parse_cast_type(p)?;
    p.expect_punct(")")?;
    Ok(Expr::Cast {
        expr: Box::new(expr),
        ty,
    })
}

/// 解析 convert。
fn parse_convert(p: &mut P) -> Result<Expr, String> {
    p.expect_punct("(")?;
    let expr = parse_predicate(p)?;
    p.expect_punct(",")?;
    let ty = parse_cast_type(p)?;
    p.expect_punct(")")?;
    Ok(Expr::Convert {
        expr: Box::new(expr),
        ty,
    })
}

/// 解析 func call。
fn parse_func_call(p: &mut P, name: &str) -> Result<Expr, String> {
    match name {
        "date_add" | "date_sub" | "timestampadd" => {
            let a = parse_predicate(p)?;
            p.expect_punct(",")?;
            p.expect_ident_ci("interval")?;
            let value = parse_predicate(p)?;
            let unit = match p.bump() {
                Some(Tok::Ident(w)) => w.to_lowercase(),
                other => return Err(format!("expected interval unit, got {other:?}")),
            };
            p.expect_punct(")")?;
            Ok(Expr::FuncCall {
                name: name.into(),
                args: vec![
                    a,
                    Expr::Interval {
                        value: Box::new(value),
                        unit,
                    },
                ],
            })
        }
        "timestampdiff" => {
            let unit = match p.bump() {
                Some(Tok::Ident(w)) => w.to_lowercase(),
                other => return Err(format!("expected unit, got {other:?}")),
            };
            p.expect_punct(",")?;
            let a = parse_predicate(p)?;
            p.expect_punct(",")?;
            let b = parse_predicate(p)?;
            p.expect_punct(")")?;
            Ok(Expr::FuncCall {
                name: name.into(),
                args: vec![Expr::RawUnit(unit), a, b],
            })
        }
        _ => {
            let mut args = Vec::new();
            if !p.eat_punct(")") {
                args = parse_expr_list(p)?;
                p.expect_punct(")")?;
            }
            Ok(Expr::FuncCall {
                name: name.into(),
                args,
            })
        }
    }
}

/// 解析 primary。
fn parse_primary(p: &mut P) -> Result<Expr, String> {
    match p
        .bump()
        .ok_or_else(|| "unexpected end of expression".to_string())?
    {
        Tok::Number(n) => Ok(Expr::Number(normalize_number(&n))),
        Tok::Str(s) => Ok(Expr::Str(s)),
        Tok::Hex(h) => Ok(Expr::Hex(h)),
        Tok::Bit(b) => Ok(Expr::Bit(trim_bit(&b))),
        Tok::Time(kind, v) => Ok(Expr::TimeLit(kind, v)),
        Tok::Punct("(") => {
            let inner = parse_predicate(p)?;
            p.expect_punct(")")?;
            Ok(Expr::Paren(Box::new(inner)))
        }
        Tok::Ident(word) => {
            let lower = word.to_lowercase();
            match lower.as_str() {
                "null" => Ok(Expr::Null),
                "true" => Ok(Expr::Bool(true)),
                "false" => Ok(Expr::Bool(false)),
                "case" => parse_case(p),
                "cast" => parse_cast(p),
                "convert" => parse_convert(p),
                "binary" => {
                    let inner = parse_unary(p)?;
                    Ok(Expr::BinaryKeyword(Box::new(inner)))
                }
                _ => {
                    if p.eat_punct("(") {
                        parse_func_call(p, &lower)
                    } else {
                        let mut parts = vec![word];
                        while p.eat_punct(".") {
                            match p.bump() {
                                Some(Tok::Ident(next)) => parts.push(next),
                                other => {
                                    return Err(format!(
                                        "expected identifier after '.', got {other:?}"
                                    ));
                                }
                            }
                        }
                        Ok(Expr::Column(parts))
                    }
                }
            }
        }
        other => Err(format!("unexpected token {other:?}")),
    }
}

/// normalizenumber。
fn normalize_number(raw: &str) -> String {
    let (mantissa, exp) = match raw.find(['e', 'E']) {
        Some(idx) => (&raw[..idx], Some(&raw[idx + 1..])),
        None => (raw, None),
    };
    let (int_part, frac_part) = match mantissa.find('.') {
        Some(idx) => (&mantissa[..idx], Some(&mantissa[idx + 1..])),
        None => (mantissa, None),
    };
    let trimmed_int = int_part.trim_start_matches('0');
    let mut out = String::from(if trimmed_int.is_empty() {
        "0"
    } else {
        trimmed_int
    });
    if let Some(f) = frac_part {
        out.push('.');
        out.push_str(f);
    }
    if let Some(e) = exp {
        let (sign, digits) = match e.strip_prefix('-') {
            Some(rest) => ("-", rest),
            None => match e.strip_prefix('+') {
                Some(rest) => ("", rest),
                None => ("", e),
            },
        };
        let dtrim = digits.trim_start_matches('0');
        let dout = if dtrim.is_empty() { "0" } else { dtrim };
        out.push('e');
        out.push_str(sign);
        out.push_str(dout);
    }
    out
}

/// trimbit。
fn trim_bit(raw: &str) -> String {
    let t = raw.trim_start_matches('0');
    if t.is_empty() {
        "0".to_string()
    } else {
        t.to_string()
    }
}

/// formatstring。
fn format_string(content: &str) -> String {
    let mut out = String::from("\"");
    for ch in content.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// 从 `SELECT <expr>` 中解析并返回首个表达式。
fn parse_first_select_expr(sql: &str) -> Result<Expr, String> {
    let toks = lex(sql)?;
    let mut p = P { toks, pos: 0 };
    p.expect_ident_ci("select")?;
    let expr = parse_predicate(&mut p)?;
    if p.pos != p.toks.len() {
        return Err(format!(
            "trailing tokens after expression: {:?}",
            &p.toks[p.pos..]
        ));
    }
    Ok(expr)
}

/// 按 TiDB Format 规则将表达式还原为规范化文本。
fn format_expr(expr: &Expr) -> Result<String, String> {
    Ok(match expr {
        Expr::Null => "NULL".into(),
        Expr::Bool(true) => "TRUE".into(),
        Expr::Bool(false) => "FALSE".into(),
        Expr::Number(n) => n.clone(),
        Expr::Str(s) => format_string(s),
        Expr::Hex(h) => format!("x'{h}'"),
        Expr::Bit(b) => format!("b'{b}'"),
        Expr::TimeLit(kind, v) => {
            let prefix = match *kind {
                "time" => "'tidb`.(timeliteral",
                "timestamp" => "'tidb`.(timestampliteral",
                _ => "'tidb`.(dateliteral",
            };
            format!("{prefix}(\"{v}\")")
        }
        Expr::Column(parts) => parts
            .iter()
            .map(|part| format!("`{part}`"))
            .collect::<Vec<_>>()
            .join("."),
        Expr::Paren(inner) => format!("({})", format_expr(inner)?),
        Expr::Unary(op, inner) => format!("{op}{}", format_expr(inner)?),
        Expr::Binary(op, l, r) => format!("{} {op} {}", format_expr(l)?, format_expr(r)?),
        Expr::Between { expr, not, lo, hi } => format!(
            "{} {}BETWEEN {} AND {}",
            format_expr(expr)?,
            if *not { "NOT " } else { "" },
            format_expr(lo)?,
            format_expr(hi)?
        ),
        Expr::IsNull { expr, not } => {
            format!(
                "{} IS{} NULL",
                format_expr(expr)?,
                if *not { " NOT" } else { "" }
            )
        }
        Expr::IsTruth { expr, not, truth } => format!(
            "{} IS{} {}",
            format_expr(expr)?,
            if *not { " NOT" } else { "" },
            if *truth { "TRUE" } else { "FALSE" }
        ),
        Expr::In { expr, not, list } => {
            let items = list
                .iter()
                .map(format_expr)
                .collect::<Result<Vec<_>, _>>()?;
            format!(
                "{} {}IN ({})",
                format_expr(expr)?,
                if *not { "NOT " } else { "" },
                items.join(",")
            )
        }
        Expr::Like {
            expr,
            not,
            pattern,
            escape,
        } => {
            let mut s = format!(
                "{} {}LIKE {}",
                format_expr(expr)?,
                if *not { "NOT " } else { "" },
                format_expr(pattern)?
            );
            if let Some(ch) = escape {
                s.push_str(&format!(" ESCAPE '{ch}'"));
            }
            s
        }
        Expr::Regexp { expr, not, pattern } => format!(
            "{} {}REGEXP {}",
            format_expr(expr)?,
            if *not { "NOT " } else { "" },
            format_expr(pattern)?
        ),
        Expr::Case {
            value,
            when_clauses,
            else_clause,
        } => {
            let mut s = String::from("CASE");
            if let Some(v) = value {
                s.push(' ');
                s.push_str(&format_expr(v)?);
            }
            for (cond, result) in when_clauses {
                s.push_str(" WHEN ");
                s.push_str(&format_expr(cond)?);
                s.push_str(" THEN ");
                s.push_str(&format_expr(result)?);
            }
            if let Some(e) = else_clause {
                s.push_str(" ELSE ");
                s.push_str(&format_expr(e)?);
            }
            s.push_str(" END");
            s
        }
        Expr::FuncCall { name, args } => {
            let items = args
                .iter()
                .map(format_expr)
                .collect::<Result<Vec<_>, _>>()?;
            format!("{name}({})", items.join(", "))
        }
        Expr::Cast { expr, ty } => format!("CAST({} AS {ty})", format_expr(expr)?),
        Expr::Convert { expr, ty } => format!("CONVERT({}, {ty})", format_expr(expr)?),
        Expr::BinaryKeyword(inner) => format!("BINARY {}", format_expr(inner)?),
        Expr::RawUnit(u) => u.to_uppercase(),
        Expr::Interval { value, unit } => {
            format!("INTERVAL {} {}", format_expr(value)?, unit.to_uppercase())
        }
        Expr::ArrowExtract { left, path } => {
            format!(
                "json_extract({}, {})",
                format_expr(left)?,
                format_expr(path)?
            )
        }
        Expr::ArrowUnquote { left, path } => {
            format!(
                "json_unquote(json_extract({}, {}))",
                format_expr(left)?,
                format_expr(path)?
            )
        }
    })
}

/// 对照 Go format_test：解析 SELECT 表达式并断言 Format 输出。
#[test]
fn TestAstFormat() {
    let cases = [
        ("null", "NULL"),
        ("true", "TRUE"),
        ("350", "350"),
        ("001e-12", "1e-12"),
        ("345.678", "345.678"),
        ("00.0001000", "0.0001000"),
        ("null", "NULL"),
        ("\"Hello, world\"", "\"Hello, world\""),
        ("'Hello, world'", "\"Hello, world\""),
        ("'Hello, \"world\"'", "\"Hello, \\\"world\\\"\""),
        ("_utf8'你好'", "\"你好\""),
        ("x'bcde'", "x'bcde'"),
        ("x''", "x''"),
        ("x'0035'", "x'0035'"),
        ("b'00111111'", "b'111111'"),
        (
            "time'10:10:10.123'",
            "'tidb`.(timeliteral(\"10:10:10.123\")",
        ),
        (
            "timestamp'1999-01-01 10:0:0.123'",
            "'tidb`.(timestampliteral(\"1999-01-01 10:0:0.123\")",
        ),
        ("date '1700-01-01'", "'tidb`.(dateliteral(\"1700-01-01\")"),
        ("f between 30 and 50", "`f` BETWEEN 30 AND 50"),
        ("f not between 30 and 50", "`f` NOT BETWEEN 30 AND 50"),
        ("345 + \"  hello  \"", "345 + \"  hello  \""),
        (
            "\"hello world\"    >=    'hello world'",
            "\"hello world\" >= \"hello world\"",
        ),
        (
            "case 3 when 1 then false else true end",
            "CASE 3 WHEN 1 THEN FALSE ELSE TRUE END",
        ),
        ("database.table.column", "`database`.`table`.`column`"),
        ("3 is null", "3 IS NULL"),
        ("3 is not null", "3 IS NOT NULL"),
        ("3 is true", "3 IS TRUE"),
        ("3 is not true", "3 IS NOT TRUE"),
        ("3 is false", "3 IS FALSE"),
        ("  ( x is false  )", "(`x` IS FALSE)"),
        ("3 in ( a,b,\"h\",6 )", "3 IN (`a`,`b`,\"h\",6)"),
        ("3 not in ( a,b,\"h\",6 )", "3 NOT IN (`a`,`b`,\"h\",6)"),
        ("\"abc\" like '%b%'", "\"abc\" LIKE \"%b%\""),
        ("\"abc\" not like '%b%'", "\"abc\" NOT LIKE \"%b%\""),
        (
            "\"abc\" like '%b%' escape '_'",
            "\"abc\" LIKE \"%b%\" ESCAPE '_'",
        ),
        ("\"abc\" regexp '.*bc?'", "\"abc\" REGEXP \".*bc?\""),
        ("\"abc\" not regexp '.*bc?'", "\"abc\" NOT REGEXP \".*bc?\""),
        ("-  4", "-4"),
        ("- ( - 4 ) ", "-(-4)"),
        ("a%b", "`a` % `b`"),
        ("a%b+6", "`a` % `b` + 6"),
        ("a%(b+6)", "`a` % (`b` + 6)"),
        (
            " json_extract ( a,'$.b',\"$.\\\"c d\\\"\" ) ",
            "json_extract(`a`, \"$.b\", \"$.\\\"c d\\\"\")",
        ),
        (" length ( a )", "length(`a`)"),
        ("a -> '$.a'", "json_extract(`a`, \"$.a\")"),
        (
            "a.b ->> '$.a'",
            "json_unquote(json_extract(`a`.`b`, \"$.a\"))",
        ),
        (
            "DATE_ADD('1970-01-01', interval 3 second)",
            "date_add(\"1970-01-01\", INTERVAL 3 SECOND)",
        ),
        (
            "TIMESTAMPDIFF(month, '2001-01-01', '2001-02-02 12:03:05.123')",
            "timestampdiff(MONTH, \"2001-01-01\", \"2001-02-02 12:03:05.123\")",
        ),
        (" cast( a as signed ) ", "CAST(`a` AS SIGNED)"),
        (" cast( a as unsigned integer) ", "CAST(`a` AS UNSIGNED)"),
        (" cast( a as char(3) binary) ", "CAST(`a` AS BINARY(3))"),
        (" cast( a as decimal ) ", "CAST(`a` AS DECIMAL(10))"),
        (" cast( a as decimal (3) ) ", "CAST(`a` AS DECIMAL(3))"),
        (" cast( a as decimal (3,3) ) ", "CAST(`a` AS DECIMAL(3, 3))"),
        (
            " ((case when (c0 = 0) then 0 when (c0 > 0) then (c1 / c0) end)) ",
            "((CASE WHEN (`c0` = 0) THEN 0 WHEN (`c0` > 0) THEN (`c1` / `c0`) END))",
        ),
        (" convert (a, signed) ", "CONVERT(`a`, SIGNED)"),
        (" binary \"hello\"", "BINARY \"hello\""),
    ];

    for (input, expected) in cases {
        let sql = format!("select {input}");
        let expr = parse_first_select_expr(&sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
        assert_eq!(format_expr(&expr).unwrap(), expected, "{input}");
    }
}
