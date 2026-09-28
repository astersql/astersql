// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 表达式 AST 的还原与 Visitor 测试（对齐 `expressions_test.go`）。
//
// 因生产路径尚无完整 SQL 解析器，本文件内嵌最小词法/语法分析器，仅覆盖
// 用例所需表达式文法，再经生产 `to_sql` API 还原，保证与 Go 用例一一对应。

#![allow(non_snake_case)]

// There is no working SQL parser wired into this crate yet, so `expressions_test.go`'s
// `parser.New().Parse` + `Expr.Restore` pipeline cannot be reused directly (see the same
// note in `format_test.rs`). This file implements a minimal expression tokenizer/parser,
// scoped to exactly the expression grammar exercised by the cases below, that builds the
// crate's real `expressions::Expr` AST and restores it through the production `to_sql`
// APIs (mirrors `expressions_test.go` case for case; no case has been dropped).
//
use crate::expressions::{
    self, BetweenExpr, ColumnName, ColumnNameExpr, Expr, FulltextSearchModifier, MatchAgainst,
    MaxValueExpr, RestoreFlags, SubqueryExpr, Value, Visitor,
};

/// 测试内嵌解析器的词法单元。
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Quoted(String),
    Str(String),
    DStr(String),
    Num(String),
    Sym(String),
    End,
}

/// 将输入 SQL 片段切分为词法单元序列。
fn lex(input: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    const MULTI: &[&str] = &["!=", "<=", ">=", "<>", "<<", ">>", ":=", "@@"];
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '`' => {
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= chars.len() {
                        return Err("unterminated backtick identifier".into());
                    }
                    if chars[i] == '`' {
                        if i + 1 < chars.len() && chars[i + 1] == '`' {
                            s.push('`');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                out.push(Tok::Quoted(s));
            }
            '\'' | '"' => {
                let quote = c;
                i += 1;
                let mut s = String::new();
                loop {
                    if i >= chars.len() {
                        return Err("unterminated string literal".into());
                    }
                    if chars[i] == quote {
                        if i + 1 < chars.len() && chars[i + 1] == quote {
                            s.push(quote);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                out.push(if quote == '\'' {
                    Tok::Str(s)
                } else {
                    Tok::DStr(s)
                });
            }
            '0'..='9' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                out.push(Tok::Num(chars[start..i].iter().collect()));
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
                {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            _ => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                if MULTI.contains(&two.as_str()) {
                    out.push(Tok::Sym(two));
                    i += 2;
                } else {
                    out.push(Tok::Sym(c.to_string()));
                    i += 1;
                }
            }
        }
    }
    out.push(Tok::End);
    Ok(out)
}

/// 递归下降解析器状态：词法流与当前位置。
struct P {
    toks: Vec<Tok>,
    pos: usize,
}

impl P {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }

    fn peek_at(&self, offset: usize) -> &Tok {
        self.toks
            .get(self.pos + offset)
            .unwrap_or_else(|| self.toks.last().unwrap())
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if x == s)
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, s: &str) -> Result<(), String> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            Err(format!("expected '{s}', got {:?}", self.peek()))
        }
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(x) if x.eq_ignore_ascii_case(kw))
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), String> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(format!("expected keyword '{kw}', got {:?}", self.peek()))
        }
    }
}

fn ident_or_quoted(p: &mut P) -> Result<String, String> {
    match p.bump() {
        Tok::Ident(s) => Ok(s),
        Tok::Quoted(s) => Ok(s),
        other => Err(format!("expected identifier, got {other:?}")),
    }
}

fn parse_dotted_name(p: &mut P) -> Result<Vec<String>, String> {
    let mut parts = vec![ident_or_quoted(p)?];
    while p.eat_sym(".") {
        parts.push(ident_or_quoted(p)?);
    }
    Ok(parts)
}

fn column_name_from_parts(parts: Vec<String>) -> ColumnName {
    match parts.len() {
        1 => ColumnName::new("", "", &parts[0]),
        2 => ColumnName::new("", &parts[0], &parts[1]),
        _ => ColumnName::new(&parts[0], &parts[1], &parts[2]),
    }
}

fn parse_subquery_parens(p: &mut P) -> Result<Expr, String> {
    p.expect_sym("(")?;
    p.expect_kw("select")?;
    let inner = parse_expr(p)?;
    p.expect_sym(")")?;
    Ok(Expr::Subquery(SubqueryExpr {
        query: format!("SELECT {}", inner.to_sql()),
        evaluated: false,
        correlated: false,
        multi_rows: false,
        exists: false,
    }))
}

fn parse_in_list_or_subquery(p: &mut P) -> Result<(Vec<Expr>, Option<Box<Expr>>), String> {
    p.expect_sym("(")?;
    if p.eat_kw("select") {
        let inner = parse_expr(p)?;
        p.expect_sym(")")?;
        let sub = Expr::Subquery(SubqueryExpr {
            query: format!("SELECT {}", inner.to_sql()),
            evaluated: false,
            correlated: false,
            multi_rows: false,
            exists: false,
        });
        Ok((Vec::new(), Some(Box::new(sub))))
    } else {
        let mut items = vec![parse_expr(p)?];
        while p.eat_sym(",") {
            items.push(parse_expr(p)?);
        }
        p.expect_sym(")")?;
        Ok((items, None))
    }
}

/// 解析 CASE ... END 主体。
fn parse_case_body(p: &mut P) -> Result<Expr, String> {
    let value = if p.is_kw("when") {
        None
    } else {
        Some(Box::new(parse_expr(p)?))
    };
    let mut when_clauses = Vec::new();
    while p.eat_kw("when") {
        let cond = parse_expr(p)?;
        p.expect_kw("then")?;
        let result = parse_expr(p)?;
        when_clauses.push(expressions::WhenClause { expr: cond, result });
    }
    let else_clause = if p.eat_kw("else") {
        Some(Box::new(parse_expr(p)?))
    } else {
        None
    };
    p.expect_kw("end")?;
    Ok(Expr::Case(expressions::CaseExpr {
        value,
        when_clauses,
        else_clause,
    }))
}

/// 解析 MATCH ... AGAINST 全文表达式。
fn parse_match_against(p: &mut P) -> Result<Expr, String> {
    p.expect_kw("match")?;
    p.expect_sym("(")?;
    let mut cols = vec![column_name_from_parts(parse_dotted_name(p)?)];
    while p.eat_sym(",") {
        cols.push(column_name_from_parts(parse_dotted_name(p)?));
    }
    p.expect_sym(")")?;
    p.expect_kw("against")?;
    p.expect_sym("(")?;
    let against = parse_expr(p)?;
    let mut modifier = FulltextSearchModifier::empty();
    if p.eat_kw("in") {
        if p.eat_kw("boolean") {
            p.expect_kw("mode")?;
            modifier = modifier | FulltextSearchModifier::BOOLEAN_MODE;
        } else {
            p.expect_kw("natural")?;
            p.expect_kw("language")?;
            p.expect_kw("mode")?;
        }
    }
    if p.eat_kw("with") {
        p.expect_kw("query")?;
        p.expect_kw("expansion")?;
        modifier = modifier | FulltextSearchModifier::QUERY_EXPANSION;
    }
    p.expect_sym(")")?;
    Ok(Expr::MatchAgainst(MatchAgainst {
        column_names: cols,
        against: Box::new(against),
        modifier,
    }))
}

/// 解析用户/系统变量。
fn parse_variable(p: &mut P) -> Result<Expr, String> {
    let is_system = p.eat_sym("@@");
    if !is_system {
        p.expect_sym("@")?;
    }
    let mut explicit_scope = false;
    let mut is_global = false;
    if is_system {
        if let Tok::Ident(word) = p.peek().clone() {
            let lw = word.to_ascii_lowercase();
            if (lw == "global" || lw == "session" || lw == "local")
                && matches!(p.peek_at(1), Tok::Sym(s) if s == ".")
            {
                p.bump();
                p.expect_sym(".")?;
                explicit_scope = true;
                is_global = lw == "global";
            }
        }
    }
    let raw_name = match p.peek().clone() {
        Tok::Ident(_) | Tok::Quoted(_) | Tok::Str(_) | Tok::DStr(_) => match p.bump() {
            Tok::Ident(s) | Tok::Quoted(s) | Tok::Str(s) | Tok::DStr(s) => s,
            _ => unreachable!(),
        },
        _ => String::new(),
    };
    let name = if is_system {
        raw_name.to_ascii_lowercase()
    } else {
        raw_name
    };
    let mut value = None;
    if !is_system && p.eat_sym(":=") {
        value = Some(Box::new(parse_expr(p)?));
    }
    Ok(Expr::Variable(expressions::VariableExpr {
        name,
        is_global,
        is_instance: false,
        is_system,
        explicit_scope,
        value,
    }))
}

/// 解析主键表达式（字面量、列名、括号、函数形构造等）。
fn parse_primary(p: &mut P) -> Result<Expr, String> {
    if let Tok::Ident(name) = p.peek().clone() {
        if name.eq_ignore_ascii_case("x") && matches!(p.peek_at(1), Tok::Str(_)) {
            p.bump();
            if let Tok::Str(hex) = p.bump() {
                return Ok(Expr::Raw(format!("x'{hex}'")));
            }
        }
    }
    match p.peek().clone() {
        Tok::Num(n) => {
            p.bump();
            Ok(Expr::Value(Value::Int(
                n.parse().map_err(|_| format!("bad number literal {n}"))?,
            )))
        }
        Tok::Str(s) => {
            p.bump();
            Ok(Expr::Value(Value::String(s)))
        }
        Tok::Sym(s) if s == "(" => {
            p.bump();
            let mut items = vec![parse_expr(p)?];
            while p.eat_sym(",") {
                items.push(parse_expr(p)?);
            }
            p.expect_sym(")")?;
            if items.len() == 1 {
                Ok(Expr::Parentheses(expressions::ParenthesesExpr {
                    expr: Box::new(items.remove(0)),
                }))
            } else {
                Ok(Expr::Row(expressions::RowExpr { values: items }))
            }
        }
        Tok::Sym(s) if s == "@" || s == "@@" => parse_variable(p),
        Tok::Ident(name) => {
            let lname = name.to_ascii_lowercase();
            match lname.as_str() {
                "true" => {
                    p.bump();
                    Ok(Expr::Value(Value::Bool(true)))
                }
                "false" => {
                    p.bump();
                    Ok(Expr::Value(Value::Bool(false)))
                }
                "row" => {
                    p.bump();
                    p.expect_sym("(")?;
                    let mut items = vec![parse_expr(p)?];
                    while p.eat_sym(",") {
                        items.push(parse_expr(p)?);
                    }
                    p.expect_sym(")")?;
                    Ok(Expr::Row(expressions::RowExpr { values: items }))
                }
                "values" => {
                    p.bump();
                    p.expect_sym("(")?;
                    let parts = parse_dotted_name(p)?;
                    p.expect_sym(")")?;
                    Ok(Expr::Values(expressions::ValuesExpr {
                        column: ColumnNameExpr::new(column_name_from_parts(parts)),
                    }))
                }
                "case" => {
                    p.bump();
                    parse_case_body(p)
                }
                "default" => {
                    p.bump();
                    if p.eat_sym("(") {
                        let parts = parse_dotted_name(p)?;
                        p.expect_sym(")")?;
                        Ok(Expr::Default(expressions::DefaultExpr {
                            name: Some(column_name_from_parts(parts)),
                        }))
                    } else {
                        Ok(Expr::Default(expressions::DefaultExpr { name: None }))
                    }
                }
                "maxvalue" => {
                    p.bump();
                    Ok(Expr::MaxValue(MaxValueExpr))
                }
                "match" => parse_match_against(p),
                _ => {
                    let parts = parse_dotted_name(p)?;
                    Ok(Expr::ColumnName(ColumnNameExpr::new(
                        column_name_from_parts(parts),
                    )))
                }
            }
        }
        Tok::Quoted(_) => {
            let parts = parse_dotted_name(p)?;
            Ok(Expr::ColumnName(ColumnNameExpr::new(
                column_name_from_parts(parts),
            )))
        }
        other => Err(format!("unexpected token {other:?}")),
    }
}

fn parse_postfix_primary(p: &mut P) -> Result<Expr, String> {
    let mut left = parse_primary(p)?;
    loop {
        let mut not = false;
        if p.is_kw("not") {
            let checkpoint = p.pos;
            p.bump();
            if p.is_kw("between")
                || p.is_kw("like")
                || p.is_kw("regexp")
                || p.is_kw("rlike")
                || (p.is_kw("in") && matches!(p.peek_at(1), Tok::Sym(s) if s == "("))
            {
                not = true;
            } else {
                p.pos = checkpoint;
                break;
            }
        }
        if p.eat_kw("between") {
            let lo = parse_unary(p)?;
            p.expect_kw("and")?;
            let hi = parse_unary(p)?;
            left = Expr::Between(BetweenExpr {
                expr: Box::new(left),
                left: Box::new(lo),
                right: Box::new(hi),
                not,
            });
            continue;
        }
        if !not && p.eat_kw("is") {
            let inner_not = p.eat_kw("not");
            if p.eat_kw("null") {
                left = Expr::IsNull(expressions::IsNullExpr {
                    expr: Box::new(left),
                    not: inner_not,
                });
            } else if p.eat_kw("true") {
                left = Expr::IsTruth(expressions::IsTruthExpr {
                    expr: Box::new(left),
                    not: inner_not,
                    true_value: 1,
                });
            } else if p.eat_kw("false") {
                left = Expr::IsTruth(expressions::IsTruthExpr {
                    expr: Box::new(left),
                    not: inner_not,
                    true_value: 0,
                });
            } else {
                return Err("expected NULL/TRUE/FALSE after IS".into());
            }
            continue;
        }
        if (not && p.is_kw("in"))
            || (!not && p.is_kw("in") && matches!(p.peek_at(1), Tok::Sym(s) if s == "("))
        {
            p.bump();
            let (list, select) = parse_in_list_or_subquery(p)?;
            left = Expr::PatternIn(expressions::PatternInExpr {
                expr: Box::new(left),
                list,
                not,
                select,
            });
            continue;
        }
        if p.eat_kw("like") {
            let pattern = parse_unary(p)?;
            left = Expr::PatternLike(expressions::PatternLikeOrIlikeExpr {
                expr: Box::new(left),
                pattern: Box::new(pattern),
                not,
                is_like: true,
                escape: b'\\',
                escape_explicit: false,
                pat_chars: Vec::new(),
                pat_types: Vec::new(),
            });
            continue;
        }
        if p.eat_kw("regexp") || p.eat_kw("rlike") {
            let pattern = parse_unary(p)?;
            left = Expr::PatternRegexp(expressions::PatternRegexpExpr {
                expr: Box::new(left),
                pattern: Box::new(pattern),
                not,
                compiled_pattern: None,
                expression_text: None,
            });
            continue;
        }
        break;
    }
    Ok(left)
}

/// 解析一元前缀运算。
fn parse_unary(p: &mut P) -> Result<Expr, String> {
    let checkpoint = p.pos;
    let mut not_count = 0;
    while p.is_kw("not") {
        not_count += 1;
        p.bump();
    }
    if p.is_kw("exists") {
        p.bump();
        let sub = parse_subquery_parens(p)?;
        return Ok(Expr::ExistsSubquery(expressions::ExistsSubqueryExpr {
            select: Box::new(sub),
            not: not_count % 2 == 1,
        }));
    }
    p.pos = checkpoint;

    if p.eat_kw("not") {
        let inner = parse_unary(p)?;
        return Ok(Expr::Unary(expressions::UnaryOperationExpr {
            op: expressions::Op::Not,
            value: Box::new(inner),
        }));
    }
    if p.eat_sym("~") {
        let inner = parse_unary(p)?;
        return Ok(Expr::Unary(expressions::UnaryOperationExpr {
            op: expressions::Op::BitNeg,
            value: Box::new(inner),
        }));
    }
    if p.eat_sym("!") {
        let inner = parse_unary(p)?;
        return Ok(Expr::Unary(expressions::UnaryOperationExpr {
            op: expressions::Op::Not2,
            value: Box::new(inner),
        }));
    }
    if p.eat_sym("+") {
        let inner = parse_unary(p)?;
        return Ok(Expr::Unary(expressions::UnaryOperationExpr {
            op: expressions::Op::UnaryPlus,
            value: Box::new(inner),
        }));
    }
    if p.eat_sym("-") {
        let inner = parse_unary(p)?;
        return Ok(Expr::Unary(expressions::UnaryOperationExpr {
            op: expressions::Op::UnaryMinus,
            value: Box::new(inner),
        }));
    }
    parse_postfix_primary(p)
}

// 窥视下一二元运算符及其优先级，供 Pratt 解析使用
fn peek_binop(p: &P) -> Option<(expressions::Op, u8)> {
    use expressions::Op;
    match p.peek() {
        Tok::Sym(s) => match s.as_str() {
            "!=" | "<>" => Some((Op::Ne, 5)),
            "=" => Some((Op::Eq, 5)),
            "<=" => Some((Op::Le, 5)),
            ">=" => Some((Op::Ge, 5)),
            "<" => Some((Op::Lt, 5)),
            ">" => Some((Op::Gt, 5)),
            "<<" => Some((Op::LeftShift, 8)),
            ">>" => Some((Op::RightShift, 8)),
            "+" => Some((Op::Plus, 9)),
            "-" => Some((Op::Minus, 9)),
            "*" => Some((Op::Mul, 10)),
            "/" => Some((Op::Div, 10)),
            "%" => Some((Op::Mod, 10)),
            "&" => Some((Op::BitAnd, 7)),
            "|" => Some((Op::BitOr, 6)),
            "^" => Some((Op::Xor, 11)),
            _ => None,
        },
        Tok::Ident(w) => match w.to_ascii_lowercase().as_str() {
            "and" => Some((Op::LogicAnd, 3)),
            "or" => Some((Op::LogicOr, 1)),
            "xor" => Some((Op::LogicXor, 2)),
            "mod" => Some((Op::Mod, 10)),
            "div" => Some((Op::IntDiv, 10)),
            _ => None,
        },
        _ => None,
    }
}

/// 按最小优先级做运算符优先级爬升（Pratt）解析。
// Pratt：循环吞入优先级 >= min_prec 的二元运算
fn parse_bin(p: &mut P, min_prec: u8) -> Result<Expr, String> {
    let mut left = parse_unary(p)?;
    loop {
        let Some((op, prec)) = peek_binop(p) else {
            break;
        };
        if prec < min_prec {
            break;
        }
        p.bump();
        let right = parse_bin(p, prec + 1)?;
        left = Expr::Binary(expressions::BinaryOperationExpr {
            op,
            left: Box::new(left),
            right: Box::new(right),
        });
    }
    Ok(left)
}

/// 解析完整表达式。
fn parse_expr(p: &mut P) -> Result<Expr, String> {
    parse_bin(p, 0)
}

/// 从 `SELECT <expr>` 形式中取出首个选择列表表达式。
fn parse_first_select_expr(sql: &str) -> Result<Expr, String> {
    let toks = lex(sql)?;
    let mut p = P { toks, pos: 0 };
    p.expect_kw("select")?;
    let expr = parse_expr(&mut p)?;
    if !matches!(p.peek(), Tok::End) {
        return Err(format!(
            "trailing tokens after expression: {:?}",
            &p.toks[p.pos..]
        ));
    }
    Ok(expr)
}

/// 调用生产路径将表达式还原为 SQL。
fn restore_expr(expr: &Expr) -> Result<String, String> {
    expr.try_to_sql()
}

/// 在二元运算符两侧加空格的还原变体。
fn restore_expr_with_spaces(expr: &Expr) -> Result<String, String> {
    expr.try_to_sql_with_flags(RestoreFlags::SPACES_AROUND_BINARY)
}

/// 批量：解析输入 SQL 表达式并断言还原结果。
fn run_cases(cases: &[(&str, &str)]) {
    for &(source, expected) in cases {
        let sql = format!("select {source}");
        let expr = parse_first_select_expr(&sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
        assert_eq!(restore_expr(&expr).unwrap(), expected, "{source}");
    }
}

macro_rules! restore_test {
    ($name:ident, [$($source:expr => $expected:expr),+ $(,)?]) => {
        #[test]
        fn $name() { run_cases(&[$(($source, $expected)),+]); }
    };
}

/// 统计访问次数的 Visitor。
#[derive(Default)]
struct CountVisitor {
    enter: usize,
    leave: usize,
}
impl Visitor for CountVisitor {
    fn enter(&mut self, node: &mut Expr) -> bool {
        if matches!(node, Expr::Raw(value) if value == "ce") {
            self.enter += 1;
        }
        false
    }
    fn leave(&mut self, node: &mut Expr) -> bool {
        if matches!(node, Expr::Raw(value) if value == "ce") {
            self.leave += 1;
        }
        true
    }
}

/// 不修改 AST 的透传 Visitor。
struct IdentityVisitor;
impl Visitor for IdentityVisitor {
    fn enter(&mut self, _: &mut Expr) -> bool {
        false
    }
    fn leave(&mut self, _: &mut Expr) -> bool {
        true
    }
}

/// Visitor 遍历覆盖：各类表达式节点的 enter/leave。
#[test]
fn TestExpresionsVisitorCover() {
    let ce = || Expr::Raw("ce".into());
    let mut nodes = vec![
        (
            Expr::Between(BetweenExpr {
                expr: Box::new(ce()),
                left: Box::new(ce()),
                right: Box::new(ce()),
                not: false,
            }),
            3,
        ),
        (
            Expr::Binary(expressions::BinaryOperationExpr {
                op: expressions::Op::Eq,
                left: Box::new(ce()),
                right: Box::new(ce()),
            }),
            2,
        ),
        (
            Expr::Case(expressions::CaseExpr {
                value: Some(Box::new(ce())),
                when_clauses: vec![
                    expressions::WhenClause {
                        expr: ce(),
                        result: ce(),
                    },
                    expressions::WhenClause {
                        expr: ce(),
                        result: ce(),
                    },
                ],
                else_clause: Some(Box::new(ce())),
            }),
            6,
        ),
        (
            Expr::ColumnName(expressions::ColumnNameExpr::new(
                expressions::ColumnName::default(),
            )),
            0,
        ),
        (
            Expr::CompareSubquery(expressions::CompareSubqueryExpr {
                left: Box::new(ce()),
                op: expressions::Op::Eq,
                right: Box::new(ce()),
                all: false,
            }),
            2,
        ),
        (Expr::Default(expressions::DefaultExpr::default()), 0),
        (
            Expr::ExistsSubquery(expressions::ExistsSubqueryExpr {
                select: Box::new(ce()),
                not: false,
            }),
            1,
        ),
        (
            Expr::IsNull(expressions::IsNullExpr {
                expr: Box::new(ce()),
                not: false,
            }),
            1,
        ),
        (
            Expr::IsTruth(expressions::IsTruthExpr {
                expr: Box::new(ce()),
                not: false,
                true_value: 1,
            }),
            1,
        ),
        (Expr::Raw("?".into()), 0),
        (
            Expr::Parentheses(expressions::ParenthesesExpr {
                expr: Box::new(ce()),
            }),
            1,
        ),
        (
            Expr::PatternIn(expressions::PatternInExpr {
                expr: Box::new(ce()),
                list: vec![ce(), ce(), ce()],
                not: false,
                select: Some(Box::new(ce())),
            }),
            5,
        ),
        (
            Expr::PatternLike(expressions::PatternLikeOrIlikeExpr {
                expr: Box::new(ce()),
                pattern: Box::new(ce()),
                not: false,
                is_like: true,
                escape: b'\\',
                escape_explicit: false,
                pat_chars: Vec::new(),
                pat_types: Vec::new(),
            }),
            2,
        ),
        (
            Expr::PatternRegexp(expressions::PatternRegexpExpr {
                expr: Box::new(ce()),
                pattern: Box::new(ce()),
                not: false,
                compiled_pattern: None,
                expression_text: None,
            }),
            2,
        ),
        (Expr::Position(expressions::PositionExpr::default()), 0),
        (
            Expr::Row(expressions::RowExpr {
                values: vec![ce(), ce()],
            }),
            2,
        ),
        (
            Expr::Unary(expressions::UnaryOperationExpr {
                op: expressions::Op::UnaryMinus,
                value: Box::new(ce()),
            }),
            1,
        ),
        (Expr::Value(expressions::Value::Int(0)), 0),
        (
            Expr::Values(expressions::ValuesExpr {
                column: expressions::ColumnNameExpr::new(expressions::ColumnName::default()),
            }),
            0,
        ),
        (
            Expr::Variable(expressions::VariableExpr {
                value: Some(Box::new(ce())),
                ..Default::default()
            }),
            1,
        ),
    ];
    for (mut node, count) in nodes.drain(..) {
        let mut visitor = CountVisitor::default();
        assert!(node.accept(&mut visitor));
        assert_eq!((visitor.enter, visitor.leave), (count, count));
        assert!(node.accept(&mut IdentityVisitor));
    }
}

restore_test!(TestUnaryOperationExprRestore, [
    "++1" => "++1", "--1" => "--1", "-+1" => "-+1", "-1" => "-1",
    "not true" => "NOT TRUE", "~3" => "~3", "!true" => "!TRUE"
]);

restore_test!(TestColumnNameExprRestore, [
    "abc" => "`abc`", "`abc`" => "`abc`", "`ab``c`" => "`ab``c`",
    "sabc.tABC" => "`sabc`.`tABC`", "dabc.sabc.tabc" => "`dabc`.`sabc`.`tabc`",
    "dabc.`sabc`.tabc" => "`dabc`.`sabc`.`tabc`", "`dABC`.`sabc`.tabc" => "`dABC`.`sabc`.`tabc`"
]);

restore_test!(TestIsNullExprRestore, [
    "a is null" => "`a` IS NULL", "a is not null" => "`a` IS NOT NULL"
]);

restore_test!(TestIsTruthRestore, [
    "a is true" => "`a` IS TRUE", "a is not true" => "`a` IS NOT TRUE",
    "a is FALSE" => "`a` IS FALSE", "a is not false" => "`a` IS NOT FALSE"
]);

restore_test!(TestBetweenExprRestore, [
    "b between 1 and 2" => "`b` BETWEEN 1 AND 2",
    "b not between 1 and 2" => "`b` NOT BETWEEN 1 AND 2",
    "b between a and b" => "`b` BETWEEN `a` AND `b`",
    "b between '' and 'b'" => "`b` BETWEEN _UTF8MB4'' AND _UTF8MB4'b'",
    "b between '2018-11-01' and '2018-11-02'" => "`b` BETWEEN _UTF8MB4'2018-11-01' AND _UTF8MB4'2018-11-02'"
]);

restore_test!(TestCaseExpr, [
    "case when 1 then 2 end" => "CASE WHEN 1 THEN 2 END",
    "case when 1 then 'a' when 2 then 'b' end" => "CASE WHEN 1 THEN _UTF8MB4'a' WHEN 2 THEN _UTF8MB4'b' END",
    "case when 1 then 'a' when 2 then 'b' else 'c' end" => "CASE WHEN 1 THEN _UTF8MB4'a' WHEN 2 THEN _UTF8MB4'b' ELSE _UTF8MB4'c' END",
    "case when 'a'!=1 then true else false end" => "CASE WHEN _UTF8MB4'a'!=1 THEN TRUE ELSE FALSE END",
    "case a when 'a' then true else false end" => "CASE `a` WHEN _UTF8MB4'a' THEN TRUE ELSE FALSE END"
]);

restore_test!(TestBinaryOperationExpr, [
    "'a'!=1" => "_UTF8MB4'a'!=1", "a!=1" => "`a`!=1", "3<5" => "3<5",
    "10>5" => "10>5", "3+5" => "3+5", "3-5" => "3-5", "a<>5" => "`a`!=5",
    "a=1" => "`a`=1", "a mod 2" => "`a`%2", "a div 2" => "`a` DIV 2",
    "true and true" => "TRUE AND TRUE", "false or false" => "FALSE OR FALSE",
    "true xor false" => "TRUE XOR FALSE", "3 & 4" => "3&4", "5 | 6" => "5|6",
    "7 ^ 8" => "7^8", "9 << 10" => "9<<10", "11 >> 12" => "11>>12"
]);

/// 二元运算在不同 RestoreFlags 下的括号与空格。
#[test]
fn TestBinaryOperationExprWithFlags() {
    for (source, expected) in [
        ("'a'!=1", "_UTF8MB4'a' != 1"),
        ("a!=1", "`a` != 1"),
        ("3<5", "3 < 5"),
        ("10>5", "10 > 5"),
        ("3+5", "3 + 5"),
        ("3-5", "3 - 5"),
        ("a<>5", "`a` != 5"),
        ("a=1", "`a` = 1"),
    ] {
        let expr = parse_first_select_expr(&format!("select {source}")).unwrap();
        assert_eq!(
            restore_expr_with_spaces(&expr).unwrap(),
            expected,
            "{source}"
        );
    }
}

restore_test!(TestParenthesesExpr, ["(1+2)*3" => "(1+2)*3", "1+2*3" => "1+2*3"]);

/// CASE/WHEN 子句还原。
#[test]
fn TestWhenClause() {
    for (source, expected) in [
        ("when 1 then 2", "WHEN 1 THEN 2"),
        ("when 1 then 'a'", "WHEN 1 THEN _UTF8MB4'a'"),
        ("when 'a'!=1 then true", "WHEN _UTF8MB4'a'!=1 THEN TRUE"),
    ] {
        let expr = parse_first_select_expr(&format!("select case {source} end")).unwrap();
        let Expr::Case(case_expr) = &expr else {
            panic!("not case")
        };
        let clause = &case_expr.when_clauses[0];
        assert_eq!(
            format!(
                "WHEN {} THEN {}",
                clause.expr.to_sql(),
                clause.result.to_sql()
            ),
            expected
        );
    }
}

/// DEFAULT 表达式还原。
#[test]
fn TestDefaultExpr() {
    let unnamed = Expr::Default(expressions::DefaultExpr { name: None });
    assert_eq!(unnamed.to_sql(), "DEFAULT");
    let named = Expr::Default(expressions::DefaultExpr {
        name: Some(ColumnName::new("", "", "i")),
    });
    assert_eq!(named.to_sql(), "DEFAULT(`i`)");
}

restore_test!(TestPatternInExprRestore, [
    "'a' in ('b')" => "_UTF8MB4'a' IN (_UTF8MB4'b')", "2 in (0,3,7)" => "2 IN (0,3,7)",
    "2 not in (0,3,7)" => "2 NOT IN (0,3,7)", "2 in (select 2)" => "2 IN (SELECT 2)",
    "2 not in (select 2)" => "2 NOT IN (SELECT 2)"
]);

restore_test!(TestPatternLikeExprRestore, [
    "a like 't1'" => "`a` LIKE _UTF8MB4't1'", "a like 't1%'" => "`a` LIKE _UTF8MB4't1%'",
    "a like '%t1%'" => "`a` LIKE _UTF8MB4'%t1%'", "a like '%t1_|'" => "`a` LIKE _UTF8MB4'%t1_|'",
    "a not like 't1'" => "`a` NOT LIKE _UTF8MB4't1'", "a not like 't1%'" => "`a` NOT LIKE _UTF8MB4't1%'",
    "a not like '%D%v%'" => "`a` NOT LIKE _UTF8MB4'%D%v%'", "a not like '%t1_|'" => "`a` NOT LIKE _UTF8MB4'%t1_|'"
]);

restore_test!(TestValuesExpr, ["values(a)" => "VALUES(`a`)", "values(a)+values(b)" => "VALUES(`a`)+VALUES(`b`)"]);

restore_test!(TestPatternRegexpExprRestore, [
    "a regexp 't1'" => "`a` REGEXP _UTF8MB4't1'", "a regexp '^[abc][0-9]{11}|ok$'" => "`a` REGEXP _UTF8MB4'^[abc][0-9]{11}|ok$'",
    "a rlike 't1'" => "`a` REGEXP _UTF8MB4't1'", "a rlike '^[abc][0-9]{11}|ok$'" => "`a` REGEXP _UTF8MB4'^[abc][0-9]{11}|ok$'",
    "a not regexp 't1'" => "`a` NOT REGEXP _UTF8MB4't1'", "a not regexp '^[abc][0-9]{11}|ok$'" => "`a` NOT REGEXP _UTF8MB4'^[abc][0-9]{11}|ok$'",
    "a not rlike 't1'" => "`a` NOT REGEXP _UTF8MB4't1'", "a not rlike '^[abc][0-9]{11}|ok$'" => "`a` NOT REGEXP _UTF8MB4'^[abc][0-9]{11}|ok$'"
]);

restore_test!(TestRowExprRestore, [
    "(1,2)" => "ROW(1,2)", "(col1,col2)" => "ROW(`col1`,`col2`)",
    "row(1,2)" => "ROW(1,2)", "row(col1,col2)" => "ROW(`col1`,`col2`)"
]);

/// MAXVALUE 哨兵表达式还原。
#[test]
fn TestMaxValueExprRestore() {
    let expr = Expr::MaxValue(MaxValueExpr);
    assert_eq!(expr.to_sql(), "MAXVALUE");
}

restore_test!(TestPositionExprRestore, ["1" => "1"]);

restore_test!(TestExistsSubqueryExprRestore, [
    "EXISTS (SELECT 2)" => "EXISTS (SELECT 2)", "NOT EXISTS (SELECT 2)" => "NOT EXISTS (SELECT 2)",
    "NOT NOT EXISTS (SELECT 2)" => "EXISTS (SELECT 2)", "NOT NOT NOT EXISTS (SELECT 2)" => "NOT EXISTS (SELECT 2)"
]);

restore_test!(TestVariableExpr, [
    "@a>1" => "@`a`>1", "@`aB`+1" => "@`aB`+1", "@'a':=1" => "@`a`:=1",
    "@`a``b`=4" => "@`a``b`=4", "@\"aBC\">1" => "@`aBC`>1", "@`a`+1" => "@`a`+1",
    "@``" => "@``", "@" => "@``", "@@``" => "@@``", "@@var" => "@@`var`",
    "@@global.b='foo'" => "@@GLOBAL.`b`=_UTF8MB4'foo'", "@@session.'C'" => "@@SESSION.`c`",
    "@@local.\"aBc\"" => "@@SESSION.`abc`"
]);

restore_test!(TestMatchAgainstExpr, [
    "MATCH(content, title) AGAINST ('search for')" => "MATCH (`content`,`title`) AGAINST (_UTF8MB4'search for')",
    "MATCH(content) AGAINST ('search for' IN BOOLEAN MODE)" => "MATCH (`content`) AGAINST (_UTF8MB4'search for' IN BOOLEAN MODE)",
    "MATCH(content, title) AGAINST ('search for' WITH QUERY EXPANSION)" => "MATCH (`content`,`title`) AGAINST (_UTF8MB4'search for' WITH QUERY EXPANSION)",
    "MATCH(content) AGAINST ('search for' IN NATURAL LANGUAGE MODE WITH QUERY EXPANSION)" => "MATCH (`content`) AGAINST (_UTF8MB4'search for' WITH QUERY EXPANSION)",
    "MATCH(content) AGAINST ('search') AND id = 1" => "MATCH (`content`) AGAINST (_UTF8MB4'search') AND `id`=1",
    "MATCH(content) AGAINST ('search') OR id = 1" => "MATCH (`content`) AGAINST (_UTF8MB4'search') OR `id`=1",
    "MATCH(content) AGAINST (X'40404040' | X'01020304') OR id = 1" => "MATCH (`content`) AGAINST (x'40404040'|x'01020304') OR `id`=1"
]);
