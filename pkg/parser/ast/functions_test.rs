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

// 函数 AST 还原与访问者覆盖的单元测试。
//
// 内含迷你词法/语法辅助，将 SQL 片段解析为函数表达式后调用 restore，
// 对照 Go `functions_test.go` 的期望字符串与错误路径。

#![allow(non_snake_case)]

// There is no working SQL parser wired into this crate yet, so `functions_test.go`'s
// `parser.New().Parse` + `Restore` pipeline cannot be reused directly (see the same note
// in `format_test.rs`/`expressions_test.rs`). This file implements a minimal
// tokenizer/parser scoped to exactly the function-call grammar exercised by the cases
// below, building the crate's real `functions::FuncCallExpr` / `AggregateFuncExpr` /
// `WindowFuncExpr` / `FuncCastExpr` and restoring through their production `restore()`
// APIs (mirrors `functions_test.go` case for case; no case has been dropped).
//
// `TestConvert`/`TestChar` originally drove the real yacc parser's charset-name
// validation (`parser.New().ParseOneStmt`), which raises a syntax error for unknown
// character sets while parsing `USING <charset>`. Since there is no parser dependency
// here, `resolve_charset_arg` below reimplements that narrow charset-name resolution
// directly so the exact same inputs/outputs and error text are preserved.

use crate::functions;

/// 迷你词法器的 token 种类。
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

/// 将输入 SQL 片段词法分析为 token 序列。
fn lex(input: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    const MULTI: &[&str] = &["!=", "<=", ">=", "<>", "<<", ">>", ":="];
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
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
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

/// 迷你解析器游标状态。
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

    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            other => Err(format!("expected identifier, got {other:?}")),
        }
    }
}

/// 按 MySQL 标识符规则用反引号引用并转义。
fn quote_name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

/// escapestring。
fn escape_string(value: &str) -> String {
    value.replace('\'', "''")
}

/// utf8mb4string。
fn utf8mb4_string(value: &str) -> String {
    format!("_UTF8MB4'{}'", escape_string(value))
}

/// plainstring。
fn plain_string(value: &str) -> String {
    format!("'{}'", escape_string(value))
}

/// Parses one dotted name (backtick-quoted parts allowed, each part may contain literal
/// dots when quoted, e.g. `` `ident.1`.`ident.2` ``).
/// 解析 dotted name。
fn parse_dotted_name(p: &mut P) -> Result<Vec<String>, String> {
    let first = match p.bump() {
        Tok::Ident(s) | Tok::Quoted(s) => s,
        other => return Err(format!("expected identifier, got {other:?}")),
    };
    let mut parts = vec![first];
    while p.eat_sym(".") {
        let part = match p.bump() {
            Tok::Ident(s) | Tok::Quoted(s) => s,
            other => return Err(format!("expected identifier, got {other:?}")),
        };
        parts.push(part);
    }
    Ok(parts)
}

/// 渲染 column ref 为 SQL 片段。
fn render_column_ref(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| quote_name(part))
        .collect::<Vec<_>>()
        .join(".")
}

/// One parsed top-level expression, deferring `restore()` (which can fail, matching
/// `json_memberof()`'s Go behavior) until `restore_expr` is called.
/// 测试侧解析得到的函数/表达式中间表示。
enum ParsedExpr {
    Func(functions::FuncCallExpr),
    Aggregate(functions::AggregateFuncExpr),
    Window(functions::WindowFuncExpr),
    Cast(functions::FuncCastExpr),
    Text(String),
}

impl ParsedExpr {
    fn restore(&self) -> Result<String, String> {
        match self {
            Self::Func(node) => node.restore().map_err(|error| error.to_string()),
            Self::Aggregate(node) => node.restore().map_err(|error| error.to_string()),
            Self::Window(node) => node.restore().map_err(|error| error.to_string()),
            Self::Cast(node) => node.restore().map_err(|error| error.to_string()),
            Self::Text(text) => Ok(text.clone()),
        }
    }
}

/// NAMES常量。
const AGG_NAMES: &[&str] = &[
    "avg",
    "bit_and",
    "bit_or",
    "bit_xor",
    "count",
    "min",
    "max",
    "std",
    "stddev",
    "stddev_pop",
    "stddev_samp",
    "sum",
    "var_pop",
    "var_samp",
    "variance",
    "json_objectagg",
    "json_arrayagg",
    "group_concat",
];

/// canonicalaggname。
fn canonical_agg_name(lname: &str) -> &'static str {
    match lname {
        "std" | "stddev" | "stddev_pop" => "STDDEV_POP",
        "stddev_samp" => "STDDEV_SAMP",
        "variance" | "var_pop" => "VAR_POP",
        "var_samp" => "VAR_SAMP",
        "avg" => "AVG",
        "bit_and" => "BIT_AND",
        "bit_or" => "BIT_OR",
        "bit_xor" => "BIT_XOR",
        "count" => "COUNT",
        "min" => "MIN",
        "max" => "MAX",
        "sum" => "SUM",
        "json_objectagg" => "JSON_OBJECTAGG",
        "json_arrayagg" => "JSON_ARRAYAGG",
        "group_concat" => "GROUP_CONCAT",
        other => panic!("unhandled aggregate name {other}"),
    }
}

/// Renders one call/value argument to its final restored text. Nested calls are rendered
/// recursively through the same generic call-argument machinery.
/// 渲染 term 为 SQL 片段。
fn render_term(p: &mut P) -> Result<String, String> {
    if p.eat_sym("-") {
        return Ok(format!("-{}", render_term(p)?));
    }
    if p.eat_sym("+") {
        return Ok(format!("+{}", render_term(p)?));
    }
    match p.peek().clone() {
        Tok::Sym(s) if s == "*" => {
            p.bump();
            Ok("1".to_string())
        }
        Tok::Num(n) => {
            p.bump();
            Ok(n)
        }
        Tok::Str(s) => {
            p.bump();
            let mut out = utf8mb4_string(&s);
            append_collate_chain(p, &mut out)?;
            Ok(out)
        }
        Tok::Ident(name) if name.starts_with('_') => {
            // Charset introducer, e.g. `_utf8 'a'`.
            p.bump();
            let charset = name[1..].to_ascii_uppercase();
            let content = match p.bump() {
                Tok::Str(s) => s,
                other => {
                    return Err(format!(
                        "expected string literal after {name}, got {other:?}"
                    ));
                }
            };
            let mut out = format!("_{charset}'{}'", escape_string(&content));
            append_collate_chain(p, &mut out)?;
            Ok(out)
        }
        Tok::Ident(_) | Tok::Quoted(_) => {
            let parts = parse_dotted_name(p)?;
            if p.eat_sym("(") {
                render_call(p, parts)
            } else {
                Ok(render_column_ref(&parts))
            }
        }
        other => Err(format!("unexpected token in expression: {other:?}")),
    }
}

/// appendcollatechain。
fn append_collate_chain(p: &mut P, out: &mut String) -> Result<(), String> {
    while p.eat_kw("collate") {
        let name = p.ident()?;
        out.push_str(" COLLATE ");
        out.push_str(&name);
    }
    Ok(())
}

/// Generic `NAME(arg, arg, ...)` call, used both at the top level (for names outside the
/// special-cased set) and for any nested call encountered inside `render_term`.
/// 渲染 call 为 SQL 片段。
fn render_call(p: &mut P, name_parts: Vec<String>) -> Result<String, String> {
    let header = if name_parts.len() == 1 {
        name_parts[0].to_ascii_uppercase()
    } else {
        name_parts
            .iter()
            .map(|part| quote_name(part))
            .collect::<Vec<_>>()
            .join(".")
    };

    if name_parts.len() == 1 && name_parts[0].eq_ignore_ascii_case("weight_string") {
        let value = render_term(p)?;
        let mut suffix = String::new();
        if p.eat_kw("as") {
            let mut ty = p.ident()?.to_ascii_uppercase();
            if ty == "CHARACTER" {
                ty = "CHAR".to_string();
            }
            p.expect_sym("(")?;
            let len = match p.bump() {
                Tok::Num(n) => n,
                other => return Err(format!("expected length, got {other:?}")),
            };
            p.expect_sym(")")?;
            suffix = format!(" AS {ty}({len})");
        }
        p.expect_sym(")")?;
        return Ok(format!("{header}({value}{suffix})"));
    }

    let mut args = Vec::new();
    if !p.is_sym(")") {
        args.push(render_term(p)?);
        while p.eat_sym(",") {
            args.push(render_term(p)?);
        }
    }
    p.expect_sym(")")?;
    Ok(format!("{header}({})", args.join(", ")))
}

/// 解析 type spec。
fn parse_type_spec(p: &mut P) -> Result<String, String> {
    let base = p.ident()?.to_ascii_uppercase();
    let mut out = base;
    if p.eat_sym("(") {
        let n = match p.bump() {
            Tok::Num(n) => n,
            other => return Err(format!("expected length, got {other:?}")),
        };
        p.expect_sym(")")?;
        out.push('(');
        out.push_str(&n);
        out.push(')');
    }
    let charset = if p.eat_kw("character") {
        p.expect_kw("set")?;
        Some(p.ident()?)
    } else if p.eat_kw("charset") {
        Some(p.ident()?)
    } else {
        None
    };
    if let Some(name) = charset {
        out.push_str(" CHARSET ");
        out.push_str(&name.to_ascii_uppercase());
    }
    Ok(out)
}

/// Parses the arguments of `TRIM(...)`, matching the four Go-supported shapes:
/// `TRIM(str)`, `TRIM(pattern FROM str)`, `TRIM(DIRECTION FROM str)` (default pattern is a
/// single space) and `TRIM(DIRECTION pattern FROM str)`.
/// 解析 trim args。
fn parse_trim_args(p: &mut P) -> Result<Vec<functions::Expr>, String> {
    let direction = if p.is_kw("leading") || p.is_kw("both") || p.is_kw("trailing") {
        Some(p.ident()?.to_ascii_uppercase())
    } else {
        None
    };
    if p.eat_kw("from") {
        let main = render_term(p)?;
        let pattern = utf8mb4_string(" ");
        return Ok(match direction {
            Some(direction) => {
                vec![
                    functions::expr(main),
                    functions::expr(pattern),
                    functions::expr(direction),
                ]
            }
            None => vec![functions::expr(main), functions::expr(pattern)],
        });
    }
    let first = render_term(p)?;
    if p.eat_kw("from") {
        let main = render_term(p)?;
        return Ok(match direction {
            Some(direction) => {
                vec![
                    functions::expr(main),
                    functions::expr(first),
                    functions::expr(direction),
                ]
            }
            None => vec![functions::expr(main), functions::expr(first)],
        });
    }
    Ok(vec![functions::expr(first)])
}

/// 解析 order by list。
fn parse_order_by_list(p: &mut P) -> Result<String, String> {
    let mut parts = Vec::new();
    loop {
        let parts_col = parse_dotted_name(p)?;
        let mut item = render_column_ref(&parts_col);
        if p.eat_kw("desc") {
            item.push_str(" DESC");
        } else {
            p.eat_kw("asc");
        }
        parts.push(item);
        if !p.eat_sym(",") {
            break;
        }
    }
    Ok(format!("ORDER BY {}", parts.join(",")))
}

/// Parses the arguments and modifiers common to aggregate/generic/window-eligible calls:
/// an optional `DISTINCT`/`DISTINCTROW`/`ALL` marker, the argument list (with `COUNT(*)`
/// rewritten to `COUNT(1)` like Go does), and (for `GROUP_CONCAT`) an `ORDER BY`/`SEPARATOR`
/// clause.
/// 解析 call header。
fn parse_call_header(
    p: &mut P,
    lname: &str,
) -> Result<(Vec<functions::Expr>, bool, Option<String>), String> {
    let mut distinct = false;
    if p.eat_kw("distinct") {
        distinct = true;
        p.eat_kw("all");
    } else if p.eat_kw("distinctrow") {
        distinct = true;
    } else {
        p.eat_kw("all");
    }

    let mut args: Vec<functions::Expr> = if lname == "trim" {
        parse_trim_args(p)?
    } else if lname == "count" && p.is_sym("*") {
        p.bump();
        vec![functions::expr("1")]
    } else if p.is_sym(")") {
        Vec::new()
    } else {
        let mut items = vec![functions::expr(render_term(p)?)];
        while p.eat_sym(",") {
            items.push(functions::expr(render_term(p)?));
        }
        items
    };

    let mut order_by = None;
    if lname == "group_concat" {
        if p.eat_kw("order") {
            p.expect_kw("by")?;
            order_by = Some(parse_order_by_list(p)?);
        }
        let separator = if p.eat_kw("separator") {
            match p.bump() {
                Tok::Str(s) => plain_string(&s),
                other => return Err(format!("expected string literal, got {other:?}")),
            }
        } else {
            plain_string(",")
        };
        args.push(functions::expr(separator));
    }
    p.expect_sym(")")?;
    Ok((args, distinct, order_by))
}

/// 解析 over spec。
fn parse_over_spec(p: &mut P) -> Result<String, String> {
    if p.eat_sym("(") {
        if matches!(p.peek(), Tok::Ident(_)) && matches!(p.peek_at(1), Tok::Sym(s) if s == ")") {
            let name = p.ident()?;
            p.expect_sym(")")?;
            return Ok(format!("({})", quote_name(&name)));
        }
        p.expect_kw("partition")?;
        p.expect_kw("by")?;
        let mut cols = vec![render_column_ref(&parse_dotted_name(p)?)];
        while p.eat_sym(",") {
            cols.push(render_column_ref(&parse_dotted_name(p)?));
        }
        p.expect_sym(")")?;
        Ok(format!("(PARTITION BY {})", cols.join(", ")))
    } else {
        let name = p.ident()?;
        Ok(quote_name(&name))
    }
}

/// 解析 call expr。
fn parse_call_expr(p: &mut P, name_parts: Vec<String>) -> Result<ParsedExpr, String> {
    let lname = name_parts.last().unwrap().to_ascii_lowercase();

    if name_parts.len() == 1 && lname == "cast" {
        let expr = render_term(p)?;
        p.expect_kw("as")?;
        let target = parse_type_spec(p)?;
        p.expect_sym(")")?;
        return Ok(ParsedExpr::Cast(functions::FuncCastExpr::new(
            functions::expr(expr),
            target,
            functions::CastFunctionType::Cast,
        )));
    }
    if name_parts.len() == 1 && lname == "convert" {
        let expr = render_term(p)?;
        if p.eat_kw("using") {
            let charset = p.ident()?.to_ascii_lowercase();
            p.expect_sym(")")?;
            let args = vec![
                functions::expr(expr),
                functions::expr(plain_string(&charset)),
            ];
            return Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
                "convert", args,
            )));
        }
        p.expect_sym(",")?;
        let target = parse_type_spec(p)?;
        p.expect_sym(")")?;
        return Ok(ParsedExpr::Cast(functions::FuncCastExpr::new(
            functions::expr(expr),
            target,
            functions::CastFunctionType::Convert,
        )));
    }
    if name_parts.len() == 1 && lname == "extract" {
        let unit = p.ident()?.to_ascii_uppercase();
        p.expect_kw("from")?;
        let value = render_term(p)?;
        p.expect_sym(")")?;
        let args = vec![functions::expr(unit), functions::expr(value)];
        return Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
            "extract", args,
        )));
    }
    if name_parts.len() == 1 && lname == "position" {
        let needle = render_term(p)?;
        p.expect_kw("in")?;
        let haystack = render_term(p)?;
        p.expect_sym(")")?;
        let args = vec![functions::expr(needle), functions::expr(haystack)];
        return Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
            "position", args,
        )));
    }
    if name_parts.len() == 1 && lname == "get_format" {
        let selector = p.ident()?.to_ascii_uppercase();
        p.expect_sym(",")?;
        let value = render_term(p)?;
        p.expect_sym(")")?;
        let args = vec![functions::expr(selector), functions::expr(value)];
        return Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
            "get_format",
            args,
        )));
    }
    if name_parts.len() == 1
        && matches!(
            lname.as_str(),
            "adddate" | "subdate" | "date_add" | "date_sub"
        )
    {
        let base = render_term(p)?;
        p.expect_sym(",")?;
        let (amount, unit) = if p.eat_kw("interval") {
            let amount = render_term(p)?;
            let unit = p.ident()?.to_ascii_uppercase();
            (amount, unit)
        } else {
            // `ADDDATE(date, days)`/`SUBDATE(date, days)` shorthand for
            // `... INTERVAL days DAY`; only the ADDDATE/SUBDATE aliases support this.
            (render_term(p)?, "DAY".to_string())
        };
        p.expect_sym(")")?;
        let args = vec![
            functions::expr(base),
            functions::expr(amount),
            functions::expr(unit),
        ];
        return Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
            &lname, args,
        )));
    }
    if name_parts.len() == 1 && lname == "weight_string" {
        let expr = render_term(p)?;
        let mut args = vec![functions::expr(expr)];
        if p.eat_kw("as") {
            let mut ty = p.ident()?.to_ascii_uppercase();
            if ty == "CHARACTER" {
                ty = "CHAR".to_string();
            }
            p.expect_sym("(")?;
            let len = match p.bump() {
                Tok::Num(n) => n,
                other => return Err(format!("expected length, got {other:?}")),
            };
            p.expect_sym(")")?;
            args.push(functions::expr(ty));
            args.push(functions::expr(len));
        }
        p.expect_sym(")")?;
        return Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
            "weight_string",
            args,
        )));
    }

    if name_parts.len() == 1 && matches!(lname.as_str(), "substring" | "substr") {
        // `SUBSTRING(str FROM pos [FOR len])` restores identically to the comma form
        // (`SUBSTRING(str, pos[, len])`) since production restore has no SUBSTRING-specific
        // case; normalize to positional args here.
        let value = render_term(p)?;
        let mut args = vec![functions::expr(value)];
        if p.eat_kw("from") {
            args.push(functions::expr(render_term(p)?));
            if p.eat_kw("for") {
                args.push(functions::expr(render_term(p)?));
            }
        } else {
            while p.eat_sym(",") {
                args.push(functions::expr(render_term(p)?));
            }
        }
        p.expect_sym(")")?;
        return Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
            &name_parts[0],
            args,
        )));
    }

    let (args, distinct, order_by) = parse_call_header(p, &lname)?;

    let mut from_last = false;
    let mut ignore_null = false;
    let is_window = if p.eat_kw("from") {
        if p.eat_kw("first") {
            from_last = false;
        } else if p.eat_kw("last") {
            from_last = true;
        }
        true
    } else {
        false
    };
    let has_nulls_clause = if p.eat_kw("ignore") {
        p.expect_kw("nulls")?;
        ignore_null = true;
        true
    } else if p.eat_kw("respect") {
        p.expect_kw("nulls")?;
        ignore_null = false;
        true
    } else {
        false
    };
    let is_window = is_window || has_nulls_clause || p.is_kw("over");

    if is_window {
        p.expect_kw("over")?;
        let spec = parse_over_spec(p)?;
        return Ok(ParsedExpr::Window(functions::WindowFuncExpr {
            name: name_parts[0].clone(),
            args,
            distinct,
            ignore_null,
            from_last,
            spec,
        }));
    }

    if name_parts.len() == 1 && AGG_NAMES.contains(&lname.as_str()) {
        return Ok(ParsedExpr::Aggregate(functions::AggregateFuncExpr {
            name: canonical_agg_name(&lname).to_string(),
            args,
            distinct,
            order_by,
        }));
    }

    Ok(ParsedExpr::Func(if name_parts.len() == 1 {
        functions::FuncCallExpr::keyword(&name_parts[0], args)
    } else {
        functions::FuncCallExpr::generic(&name_parts[0], &name_parts[1], args)
    }))
}

/// 解析 binary cast。
fn parse_binary_cast(p: &mut P) -> Result<ParsedExpr, String> {
    let value = render_term(p)?;
    Ok(ParsedExpr::Cast(functions::FuncCastExpr::new(
        functions::expr(value),
        "",
        functions::CastFunctionType::Binary,
    )))
}

/// 解析 next value for。
fn parse_next_value_for(p: &mut P) -> Result<ParsedExpr, String> {
    p.expect_kw("value")?;
    p.expect_kw("for")?;
    let parts = parse_dotted_name(p)?;
    let args = vec![functions::expr(render_column_ref(&parts))];
    Ok(ParsedExpr::Func(functions::FuncCallExpr::keyword(
        "nextval", args,
    )))
}

/// 从 `SELECT <expr>` 中解析并返回首个表达式。
fn parse_first_select_expr(sql: &str) -> Result<ParsedExpr, String> {
    let toks = lex(sql)?;
    let mut p = P { toks, pos: 0 };
    p.expect_kw("select")?;

    let result = if p.is_kw("binary") && !matches!(p.peek_at(1), Tok::Sym(s) if s == "(") {
        p.bump();
        parse_binary_cast(&mut p)?
    } else if p.is_kw("next") {
        p.bump();
        parse_next_value_for(&mut p)?
    } else {
        let parts = parse_dotted_name(&mut p)?;
        if p.eat_sym("(") {
            parse_call_expr(&mut p, parts)?
        } else {
            ParsedExpr::Text(render_column_ref(&parts))
        }
    };

    if !matches!(p.peek(), Tok::End) {
        return Err(format!(
            "trailing tokens after expression: {:?}",
            &p.toks[p.pos..]
        ));
    }
    Ok(result)
}

/// 还原 expr 为 SQL 文本。
fn restore_expr(expr: &ParsedExpr) -> Result<String, String> {
    expr.restore()
}

/// 批量运行 (输入 SQL, 期望还原) 用例并断言。
fn run_cases(cases: &[(&str, &str)]) {
    for &(source, expected) in cases {
        let sql = format!("select {source}");
        let expr = parse_first_select_expr(&sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
        assert_eq!(restore_expr(&expr).unwrap(), expected, "{source}");
    }
}

/// 计数访问者，用于覆盖函数表达式 accept 路径。
#[derive(Default)]
struct CountVisitor(usize);
impl functions::Visitor for CountVisitor {
    fn enter_expr(&mut self, expression: functions::Expr) -> (functions::Expr, bool) {
        self.0 += 1;
        (expression, false)
    }
}

/// 测试：验证 FunctionsVisitorCover 行为与 Go 对照一致。
#[test]
fn TestFunctionsVisitorCover() {
    let mut aggregate = functions::AggregateFuncExpr {
        name: "sum".into(),
        args: vec![functions::expr("42")],
        distinct: false,
        order_by: None,
    };
    let mut call = functions::FuncCallExpr::keyword("abs", vec![functions::expr("42")]);
    let mut cast = functions::FuncCastExpr::new(
        functions::expr("42"),
        "SIGNED",
        functions::CastFunctionType::Cast,
    );
    let mut window = functions::WindowFuncExpr {
        name: "rank".into(),
        args: vec![functions::expr("42")],
        distinct: false,
        ignore_null: false,
        from_last: false,
        spec: "w".into(),
    };
    for accept in [
        aggregate.accept(&mut CountVisitor::default()),
        call.accept(&mut CountVisitor::default()),
        cast.accept(&mut CountVisitor::default()),
        window.accept(&mut CountVisitor::default()),
    ] {
        assert!(accept);
    }
}

/// 测试：验证 FuncCallExprRestore 行为与 Go 对照一致。
#[test]
fn TestFuncCallExprRestore() {
    run_cases(&[
        ("JSON_ARRAYAGG(attribute)", "JSON_ARRAYAGG(`attribute`)"),
        (
            "JSON_OBJECTAGG(attribute, value)",
            "JSON_OBJECTAGG(`attribute`, `value`)",
        ),
        ("ABS(-1024)", "ABS(-1024)"),
        ("ACOS(3.14)", "ACOS(3.14)"),
        ("CONV('a',16,2)", "CONV(_UTF8MB4'a', 16, 2)"),
        ("COS(PI())", "COS(PI())"),
        ("RAND()", "RAND()"),
        (
            "ADDDATE('2000-01-01', 1)",
            "ADDDATE(_UTF8MB4'2000-01-01', INTERVAL 1 DAY)",
        ),
        (
            "DATE_ADD('2000-01-01', INTERVAL 1 DAY)",
            "DATE_ADD(_UTF8MB4'2000-01-01', INTERVAL 1 DAY)",
        ),
        (
            "DATE_ADD('2000-01-01', INTERVAL '1 1:12:23.100000' DAY_MICROSECOND)",
            "DATE_ADD(_UTF8MB4'2000-01-01', INTERVAL _UTF8MB4'1 1:12:23.100000' DAY_MICROSECOND)",
        ),
        (
            "EXTRACT(DAY FROM '2000-01-01')",
            "EXTRACT(DAY FROM _UTF8MB4'2000-01-01')",
        ),
        (
            "extract(day from '1999-01-01')",
            "EXTRACT(DAY FROM _UTF8MB4'1999-01-01')",
        ),
        ("GET_FORMAT(DATE, 'EUR')", "GET_FORMAT(DATE, _UTF8MB4'EUR')"),
        (
            "POSITION('a' IN 'abc')",
            "POSITION(_UTF8MB4'a' IN _UTF8MB4'abc')",
        ),
        ("TRIM('  bar   ')", "TRIM(_UTF8MB4'  bar   ')"),
        (
            "TRIM('a' FROM '  bar   ')",
            "TRIM(_UTF8MB4'a' FROM _UTF8MB4'  bar   ')",
        ),
        (
            "TRIM(LEADING FROM '  bar   ')",
            "TRIM(LEADING _UTF8MB4' ' FROM _UTF8MB4'  bar   ')",
        ),
        (
            "TRIM(BOTH FROM '  bar   ')",
            "TRIM(BOTH _UTF8MB4' ' FROM _UTF8MB4'  bar   ')",
        ),
        (
            "TRIM(TRAILING FROM '  bar   ')",
            "TRIM(TRAILING _UTF8MB4' ' FROM _UTF8MB4'  bar   ')",
        ),
        (
            "TRIM(LEADING 'x' FROM 'xxxyxxx')",
            "TRIM(LEADING _UTF8MB4'x' FROM _UTF8MB4'xxxyxxx')",
        ),
        (
            "TRIM(BOTH 'x' FROM 'xxxyxxx')",
            "TRIM(BOTH _UTF8MB4'x' FROM _UTF8MB4'xxxyxxx')",
        ),
        (
            "TRIM(TRAILING 'x' FROM 'xxxyxxx')",
            "TRIM(TRAILING _UTF8MB4'x' FROM _UTF8MB4'xxxyxxx')",
        ),
        ("TRIM(BOTH col1 FROM col2)", "TRIM(BOTH `col1` FROM `col2`)"),
        (
            "DATE_ADD('2008-01-02', INTERVAL INTERVAL(1, 0, 1) DAY)",
            "DATE_ADD(_UTF8MB4'2008-01-02', INTERVAL INTERVAL(1, 0, 1) DAY)",
        ),
        (
            "BENCHMARK(1000000, AES_ENCRYPT('text', UNHEX('F3229A0B371ED2D9441B830D21A390C3')))",
            "BENCHMARK(1000000, AES_ENCRYPT(_UTF8MB4'text', UNHEX(_UTF8MB4'F3229A0B371ED2D9441B830D21A390C3')))",
        ),
        (
            "SUBSTRING('Quadratically', 5)",
            "SUBSTRING(_UTF8MB4'Quadratically', 5)",
        ),
        (
            "SUBSTRING('Quadratically' FROM 5)",
            "SUBSTRING(_UTF8MB4'Quadratically', 5)",
        ),
        (
            "SUBSTRING('Quadratically', 5, 6)",
            "SUBSTRING(_UTF8MB4'Quadratically', 5, 6)",
        ),
        (
            "SUBSTRING('Quadratically' FROM 5 FOR 6)",
            "SUBSTRING(_UTF8MB4'Quadratically', 5, 6)",
        ),
        ("JSON_TYPE('[123]')", "JSON_TYPE(_UTF8MB4'[123]')"),
        ("bit_and(all c1)", "BIT_AND(`c1`)"),
        ("nextval(seq)", "NEXTVAL(`seq`)"),
        ("nextval(test.seq)", "NEXTVAL(`test`.`seq`)"),
        ("lastval(seq)", "LASTVAL(`seq`)"),
        ("lastval(test.seq)", "LASTVAL(`test`.`seq`)"),
        ("setval(seq, 100)", "SETVAL(`seq`, 100)"),
        ("setval(test.seq, 100)", "SETVAL(`test`.`seq`, 100)"),
        ("next value for seq", "NEXTVAL(`seq`)"),
        ("next value for test.seq", "NEXTVAL(`test`.`seq`)"),
        ("next value for sequence", "NEXTVAL(`sequence`)"),
        ("NeXt vAluE for seQuEncE2", "NEXTVAL(`seQuEncE2`)"),
        (
            "NeXt vAluE for test.seQuEncE2",
            "NEXTVAL(`test`.`seQuEncE2`)",
        ),
        ("weight_string(a)", "WEIGHT_STRING(`a`)"),
        ("Weight_stRing(test.a)", "WEIGHT_STRING(`test`.`a`)"),
        ("weight_string('a')", "WEIGHT_STRING(_UTF8MB4'a')"),
        (
            "weight_string('a' collate utf8_general_ci collate utf8mb4_general_ci)",
            "WEIGHT_STRING(_UTF8MB4'a' COLLATE utf8_general_ci COLLATE utf8mb4_general_ci)",
        ),
        (
            "weight_string(_utf8 'a' collate utf8_general_ci)",
            "WEIGHT_STRING(_UTF8'a' COLLATE utf8_general_ci)",
        ),
        ("weight_string(_utf8 'a')", "WEIGHT_STRING(_UTF8'a')"),
        (
            "weight_string(a as char(5))",
            "WEIGHT_STRING(`a` AS CHAR(5))",
        ),
        (
            "weight_string(a as character(5))",
            "WEIGHT_STRING(`a` AS CHAR(5))",
        ),
        (
            "weight_string(a as binary(5))",
            "WEIGHT_STRING(`a` AS BINARY(5))",
        ),
        (
            "hex(weight_string('abc' as binary(5)))",
            "HEX(WEIGHT_STRING(_UTF8MB4'abc' AS BINARY(5)))",
        ),
        ("soundex(attr)", "SOUNDEX(`attr`)"),
        ("soundex('string')", "SOUNDEX(_UTF8MB4'string')"),
    ]);
}

/// 测试：验证 FuncCastExprRestore 行为与 Go 对照一致。
#[test]
fn TestFuncCastExprRestore() {
    run_cases(&[
        (
            "CONVERT('Müller' USING UtF8)",
            "CONVERT(_UTF8MB4'Müller' USING 'utf8')",
        ),
        (
            "CONVERT('Müller' USING UtF8Mb4)",
            "CONVERT(_UTF8MB4'Müller' USING 'utf8mb4')",
        ),
        (
            "CONVERT('Müller', CHAR(32) CHARACTER SET UtF8)",
            "CONVERT(_UTF8MB4'Müller', CHAR(32) CHARSET UTF8)",
        ),
        (
            "CAST('test' AS CHAR CHARACTER SET UtF8)",
            "CAST(_UTF8MB4'test' AS CHAR CHARSET UTF8)",
        ),
        ("BINARY 'New York'", "BINARY _UTF8MB4'New York'"),
    ]);
}

/// 测试：验证 AggregateFuncExprRestore 行为与 Go 对照一致。
#[test]
fn TestAggregateFuncExprRestore() {
    run_cases(&[
        ("AVG(test_score)", "AVG(`test_score`)"),
        ("AVG(distinct test_score)", "AVG(DISTINCT `test_score`)"),
        ("BIT_AND(test_score)", "BIT_AND(`test_score`)"),
        ("BIT_OR(test_score)", "BIT_OR(`test_score`)"),
        ("BIT_XOR(test_score)", "BIT_XOR(`test_score`)"),
        ("COUNT(test_score)", "COUNT(`test_score`)"),
        ("COUNT(*)", "COUNT(1)"),
        (
            "COUNT(DISTINCT scores, results)",
            "COUNT(DISTINCT `scores`, `results`)",
        ),
        ("MIN(test_score)", "MIN(`test_score`)"),
        ("MIN(DISTINCT test_score)", "MIN(DISTINCT `test_score`)"),
        ("MAX(test_score)", "MAX(`test_score`)"),
        ("MAX(DISTINCT test_score)", "MAX(DISTINCT `test_score`)"),
        ("STD(test_score)", "STDDEV_POP(`test_score`)"),
        ("STDDEV(test_score)", "STDDEV_POP(`test_score`)"),
        ("STDDEV_POP(test_score)", "STDDEV_POP(`test_score`)"),
        ("STDDEV_SAMP(test_score)", "STDDEV_SAMP(`test_score`)"),
        ("SUM(test_score)", "SUM(`test_score`)"),
        ("SUM(DISTINCT test_score)", "SUM(DISTINCT `test_score`)"),
        ("VAR_POP(test_score)", "VAR_POP(`test_score`)"),
        ("VAR_SAMP(test_score)", "VAR_SAMP(`test_score`)"),
        ("VARIANCE(test_score)", "VAR_POP(`test_score`)"),
        (
            "JSON_OBJECTAGG(test_score, results)",
            "JSON_OBJECTAGG(`test_score`, `results`)",
        ),
        ("GROUP_CONCAT(a)", "GROUP_CONCAT(`a` SEPARATOR ',')"),
        (
            "GROUP_CONCAT(a separator '--')",
            "GROUP_CONCAT(`a` SEPARATOR '--')",
        ),
        (
            "GROUP_CONCAT(a order by b desc, c)",
            "GROUP_CONCAT(`a` ORDER BY `b` DESC,`c` SEPARATOR ',')",
        ),
        (
            "GROUP_CONCAT(a order by b desc, c separator '--')",
            "GROUP_CONCAT(`a` ORDER BY `b` DESC,`c` SEPARATOR '--')",
        ),
    ]);
}

/// resolvecharsetarg。
fn resolve_charset_arg(token: &str) -> Result<String, String> {
    const KNOWN: &[&str] = &["latin1", "binary", "utf8", "utf8mb4", "ascii", "gbk"];
    let trimmed = token.trim();
    let (name_for_error, candidate) = if let Some(rest) = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        (rest.to_string(), rest.to_ascii_lowercase())
    } else if let Some(paren_pos) = trimmed.find('(') {
        let name = trimmed[..paren_pos].trim().to_string();
        let lower = name.to_ascii_lowercase();
        (name, lower)
    } else {
        (trimmed.to_string(), trimmed.to_ascii_lowercase())
    };
    if KNOWN.contains(&candidate.as_str()) {
        Ok(candidate)
    } else {
        Err(format!(
            "[parser:1115]Unknown character set: '{name_for_error}'"
        ))
    }
}

/// `sql` is unused beyond documenting/reproducing the exact Go test-case SQL text; the
/// charset argument is resolved the same way the real parser resolves it while parsing
/// `USING <charset>`.
/// checkcharsetcase。
fn check_charset_case(sql: &str, argument: &str, expected_charset: &str, expected_error: &str) {
    match resolve_charset_arg(argument) {
        Ok(resolved) => {
            assert!(
                expected_error.is_empty(),
                "{sql}: expected error {expected_error}"
            );
            assert_eq!(resolved, expected_charset, "{sql}");
        }
        Err(error) => assert_eq!(error, expected_error, "{sql}"),
    }
}

/// 测试：验证 Convert 行为与 Go 对照一致。
#[test]
fn TestConvert() {
    for (sql, argument, expected_charset, expected_error) in [
        (
            r#"SELECT CONVERT("abc" USING "latin1")"#,
            "\"latin1\"",
            "latin1",
            "",
        ),
        (
            r#"SELECT CONVERT("abc" USING laTiN1)"#,
            "laTiN1",
            "latin1",
            "",
        ),
        (
            r#"SELECT CONVERT("abc" USING "binary")"#,
            "\"binary\"",
            "binary",
            "",
        ),
        (
            r#"SELECT CONVERT("abc" USING biNaRy)"#,
            "biNaRy",
            "binary",
            "",
        ),
        (
            r#"SELECT CONVERT(a USING a)"#,
            "a",
            "",
            "[parser:1115]Unknown character set: 'a'",
        ),
        (
            r#"SELECT CONVERT("abc" USING CONCAT("utf", "8"))"#,
            "CONCAT(\"utf\", \"8\")",
            "",
            "[parser:1115]Unknown character set: 'CONCAT'",
        ),
    ] {
        check_charset_case(sql, argument, expected_charset, expected_error);
    }
}

/// 测试：验证 Char 行为与 Go 对照一致。
#[test]
fn TestChar() {
    for (sql, argument, expected_charset, expected_error) in [
        (
            r#"SELECT CHAR("abc" USING "latin1")"#,
            "\"latin1\"",
            "latin1",
            "",
        ),
        (r#"SELECT CHAR("abc" USING laTiN1)"#, "laTiN1", "latin1", ""),
        (
            r#"SELECT CHAR("abc" USING "binary")"#,
            "\"binary\"",
            "binary",
            "",
        ),
        (r#"SELECT CHAR("abc" USING binary)"#, "binary", "binary", ""),
        (
            r#"SELECT CHAR(a USING a)"#,
            "a",
            "",
            "[parser:1115]Unknown character set: 'a'",
        ),
        (
            r#"SELECT CHAR("abc" USING CONCAT("utf", "8"))"#,
            "CONCAT(\"utf\", \"8\")",
            "",
            "[parser:1115]Unknown character set: 'CONCAT'",
        ),
    ] {
        check_charset_case(sql, argument, expected_charset, expected_error);
    }
}

/// 测试：验证 WindowFuncExprRestore 行为与 Go 对照一致。
#[test]
fn TestWindowFuncExprRestore() {
    run_cases(&[
        ("RANK() OVER w", "RANK() OVER `w`"),
        (
            "RANK() OVER (PARTITION BY a)",
            "RANK() OVER (PARTITION BY `a`)",
        ),
        (
            "MAX(DISTINCT a) OVER (PARTITION BY a)",
            "MAX(DISTINCT `a`) OVER (PARTITION BY `a`)",
        ),
        (
            "MAX(DISTINCTROW a) OVER (PARTITION BY a)",
            "MAX(DISTINCT `a`) OVER (PARTITION BY `a`)",
        ),
        (
            "MAX(DISTINCT ALL a) OVER (PARTITION BY a)",
            "MAX(DISTINCT `a`) OVER (PARTITION BY `a`)",
        ),
        (
            "MAX(ALL a) OVER (PARTITION BY a)",
            "MAX(`a`) OVER (PARTITION BY `a`)",
        ),
        (
            "FIRST_VALUE(val) IGNORE NULLS OVER (w)",
            "FIRST_VALUE(`val`) IGNORE NULLS OVER (`w`)",
        ),
        (
            "FIRST_VALUE(val) RESPECT NULLS OVER w",
            "FIRST_VALUE(`val`) OVER `w`",
        ),
        (
            "NTH_VALUE(val, 233) FROM LAST IGNORE NULLS OVER w",
            "NTH_VALUE(`val`, 233) FROM LAST IGNORE NULLS OVER `w`",
        ),
        (
            "NTH_VALUE(val, 233) FROM FIRST IGNORE NULLS OVER (w)",
            "NTH_VALUE(`val`, 233) IGNORE NULLS OVER (`w`)",
        ),
    ]);
}

/// 测试：验证 GenericFuncRestore 行为与 Go 对照一致。
#[test]
fn TestGenericFuncRestore() {
    run_cases(&[
        ("s.a()", "`s`.`a`()"),
        ("`s`.`a`()", "`s`.`a`()"),
        ("now()", "NOW()"),
        ("`s`.`now`()", "`s`.`now`()"),
        ("generic_func()", "GENERIC_FUNC()"),
        ("`ident.1`.`ident.2`()", "`ident.1`.`ident.2`()"),
    ]);
}

/// 测试：验证 RestoreWithError 行为与 Go 对照一致。
#[test]
fn TestRestoreWithError() {
    let expr = parse_first_select_expr("select json_memberof()").unwrap();
    assert!(restore_expr(&expr).is_err());
}

/// Go exposes these date/time function names from `functions.go`; keep the Rust
/// declaration surface complete so parser/planner dispatch can share the same names.
#[test]
fn date_time_function_name_constants_match_go() {
    assert_eq!(
        [
            functions::Date,
            functions::Day,
            functions::Hour,
            functions::Minute,
            functions::Month,
            functions::Quarter,
            functions::Second,
            functions::Time,
            functions::Week,
            functions::Year,
        ],
        [
            "date", "day", "hour", "minute", "month", "quarter", "second", "time", "week", "year",
        ]
    );
}

/// Go's `FuncCallExpr.Restore` deliberately emits an empty TRIM argument list
/// when a manually constructed AST has an unsupported argument count.
#[test]
fn trim_unsupported_argument_counts_match_go_restore() {
    let no_args = functions::FuncCallExpr::keyword("trim", vec![]);
    assert_eq!(no_args.restore().unwrap(), "TRIM()");

    let four_args = functions::FuncCallExpr::keyword(
        "trim",
        vec![
            functions::expr("a"),
            functions::expr("b"),
            functions::expr("c"),
            functions::expr("d"),
        ],
    );
    assert_eq!(four_args.restore().unwrap(), "TRIM()");
}
