// Copyright 2026 AsterSQL.
//! Local stubs for `tools/tazel` (bazel buildtools + minimal go_test editing).
//!
//! 该模块不是 Bazel buildtools 的完整 Rust 移植，而是 `tools/tazel`
//! 为了保持 Go 工具主流程可运行而准备的一层最小适配面。
//! 它只抽象出当前补丁流程真正会访问的概念：
//! - BUILD 文件里的一组规则；
//! - `go_test` 规则上的少数几个属性；
//! - 解析、改写、格式化三个与磁盘文本交互的入口。
//!
//! 解析器只暴露 `tazel` 用到的窄 API，但写回时保留所有未修改的 Starlark
//! 文本与全部规则；这保证局部属性编辑不会破坏真实 BUILD 文件。

use std::collections::HashMap;

/// Go `github.com/bazelbuild/buildtools/build` — slim go_test attribute editor.
pub mod build {
    use super::*;

    #[derive(Clone, Debug)]
    struct AttrSpan {
        item_start: usize,
        item_end: usize,
        value_start: usize,
        value_end: usize,
    }

    /// 解析后的 BUILD 文件视图。
    ///
    /// `rules` 只保存当前桩关心的规则子集；
    /// `original` 保留原始文本，供“未识别/无需改写”场景直接回传。
    #[derive(Clone, Debug, Default)]
    pub struct File {
        pub rules: Vec<Rule>,
        pub original: Vec<u8>,
    }

    /// 单条规则的最小表示。
    ///
    /// 这里只保留 `tazel` 会读写的几个字段，目的是复刻 Go 工具用到的
    /// API 形状，而不是表达 BUILD 规则的完整属性集合。
    #[derive(Clone, Debug, Default)]
    pub struct Rule {
        pub kind: String,
        pub timeout: String,
        pub flaky: String,
        pub shard_count: String,
        start: usize,
        end: usize,
        timeout_span: Option<AttrSpan>,
        flaky_span: Option<AttrSpan>,
        shard_count_span: Option<AttrSpan>,
        initial_timeout: String,
        initial_flaky: String,
        initial_shard_count: String,
    }

    impl File {
        /// 返回匹配 `kind` 的规则集合。
        ///
        /// 虽然当前调用方通常只取第一个 `go_test`，这里仍返回一个列表，
        /// 以便接口外形继续贴近 Go buildtools 的 `Rules` 调用方式。
        pub fn Rules(&mut self, kind: &str) -> Vec<&mut Rule> {
            self.rules.iter_mut().filter(|r| r.kind == kind).collect()
        }
    }

    impl Rule {
        /// 读取字符串字面量属性。
        ///
        /// 当前只有 `timeout` 走这个分支；未知属性返回空串，
        /// 让上层可以像 Go 版本一样用“空值表示缺失属性”。
        pub fn AttrString(&self, name: &str) -> &str {
            match name {
                "timeout" => &self.timeout,
                _ => "",
            }
        }

        /// 读取非字符串 token 形式的属性。
        ///
        /// `flaky = True` 和 `shard_count = 3` 都按原 token 文本存储，
        /// 因为补丁逻辑只关心“是否存在以及应写回什么”，不做进一步求值。
        pub fn AttrLiteral(&self, name: &str) -> &str {
            match name {
                "flaky" => &self.flaky,
                "shard_count" => &self.shard_count,
                _ => "",
            }
        }

        /// 写入目标属性。
        ///
        /// 通过 `BuildExpr` 统一字符串和字面量两种输入，复用 Go 侧
        /// “不同表达式节点写入同一个 SetAttr 入口”的调用习惯。
        pub fn SetAttr(&mut self, name: &str, expr: impl BuildExpr) {
            match name {
                "timeout" => self.timeout = expr.into_token(),
                "flaky" => self.flaky = expr.into_token(),
                "shard_count" => self.shard_count = expr.into_token(),
                _ => {}
            }
        }

        /// 删除已知属性。
        ///
        /// 目前只需要清掉 `shard_count`；其它属性未建模删除逻辑，
        /// 反映的是当前工具真实需求，而不是通用 BUILD 编辑能力。
        pub fn DelAttr(&mut self, name: &str) {
            if name == "shard_count" {
                self.shard_count.clear();
            }
        }
    }

    /// 能被写入规则属性的最小表达式抽象。
    ///
    /// 它不保留源代码位置信息或 AST 结构，只负责产出最终 token，
    /// 因此更像“写回时的参数封装器”而不是完整语法节点。
    pub trait BuildExpr {
        fn into_token(self) -> String;
    }

    /// 双引号字符串属性的输入载体。
    pub struct StringExpr {
        pub Value: String,
    }

    impl BuildExpr for StringExpr {
        fn into_token(self) -> String {
            self.Value
        }
    }

    /// 字面量属性的输入载体。
    ///
    /// 这里直接保存 token 文本，例如 `True` 或 `3`，
    /// 避免桩层承担不必要的语义检查责任。
    pub struct LiteralExpr {
        pub Token: String,
    }

    impl BuildExpr for LiteralExpr {
        fn into_token(self) -> String {
            self.Token
        }
    }

    /// Parse BUILD text and retain exact source ranges for every `go_test` call.
    pub fn ParseBuild(_name: &str, data: Vec<u8>) -> Result<File, String> {
        let text = std::str::from_utf8(&data).map_err(|err| err.to_string())?;
        validate_starlark(text)?;
        let mut rules = Vec::new();
        for (start, end) in find_calls(text, "go_test")? {
            let timeout_span = find_attr(text, start, end, "timeout");
            let flaky_span = find_attr(text, start, end, "flaky");
            let shard_count_span = find_attr(text, start, end, "shard_count");
            let timeout = timeout_span
                .as_ref()
                .and_then(|span| parse_string(&text[span.value_start..span.value_end]))
                .unwrap_or_default();
            let flaky = flaky_span
                .as_ref()
                .map(|span| text[span.value_start..span.value_end].trim().to_string())
                .unwrap_or_default();
            let shard_count = shard_count_span
                .as_ref()
                .map(|span| text[span.value_start..span.value_end].trim().to_string())
                .unwrap_or_default();
            let mut rule = Rule {
                kind: "go_test".into(),
                start,
                end,
                timeout_span,
                flaky_span,
                shard_count_span,
                initial_timeout: timeout.clone(),
                initial_flaky: flaky.clone(),
                initial_shard_count: shard_count.clone(),
                timeout,
                flaky,
                shard_count,
                ..Default::default()
            };
            rules.push(rule);
        }
        Ok(File {
            rules,
            original: data,
        })
    }

    /// 与 Go buildtools API 形状对齐的改写钩子。
    pub fn Rewrite(_file: &mut File) {}

    /// Apply changed attributes while preserving every unrelated source byte.
    pub fn Format(file: &File) -> Vec<u8> {
        let Ok(mut out) = String::from_utf8(file.original.clone()) else {
            return file.original.clone();
        };
        for rule in file.rules.iter().rev() {
            let replacement = format_rule(&out[rule.start..rule.end], rule);
            out.replace_range(rule.start..rule.end, &replacement);
        }
        out.into_bytes()
    }

    fn format_rule(original: &str, rule: &Rule) -> String {
        let mut out = original.to_string();
        let mut edits: Vec<(usize, usize, String)> = Vec::new();
        collect_edit(
            &mut edits,
            rule.timeout_span.as_ref(),
            &rule.initial_timeout,
            &rule.timeout,
            |value| format!("\"{}\"", value.replace('"', "\\\"")),
        );
        collect_edit(
            &mut edits,
            rule.flaky_span.as_ref(),
            &rule.initial_flaky,
            &rule.flaky,
            str::to_string,
        );
        collect_edit(
            &mut edits,
            rule.shard_count_span.as_ref(),
            &rule.initial_shard_count,
            &rule.shard_count,
            str::to_string,
        );

        let close = original.rfind(')').unwrap_or(original.len());
        let mut inserted = String::new();
        if rule.timeout_span.is_none() && !rule.timeout.is_empty() {
            inserted.push_str(&format!("    timeout = \"{}\",\n", rule.timeout));
        }
        if rule.flaky_span.is_none() && !rule.flaky.is_empty() {
            inserted.push_str(&format!("    flaky = {},\n", rule.flaky));
        }
        if rule.shard_count_span.is_none() && !rule.shard_count.is_empty() {
            inserted.push_str(&format!("    shard_count = {},\n", rule.shard_count));
        }
        if !inserted.is_empty() {
            let open = original.find('(').unwrap_or(0);
            let body = &original[open + 1..close];
            let trimmed = body.trim_end();
            let prefix = if trimmed.is_empty() {
                if body.contains('\n') { "" } else { "\n" }
            } else if trimmed.ends_with(',') {
                if body.ends_with('\n') { "" } else { "\n" }
            } else {
                ",\n"
            };
            inserted.insert_str(0, prefix);
            edits.push((rule.start + close, rule.start + close, inserted));
        }
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
        for (start, end, replacement) in edits {
            out.replace_range(start - rule.start..end - rule.start, &replacement);
        }
        out
    }

    fn collect_edit<F: Fn(&str) -> String>(
        edits: &mut Vec<(usize, usize, String)>,
        span: Option<&AttrSpan>,
        initial: &str,
        desired: &str,
        encode: F,
    ) {
        if initial == desired {
            return;
        }
        if let Some(span) = span {
            if desired.is_empty() {
                edits.push((span.item_start, span.item_end, String::new()));
            } else {
                edits.push((span.value_start, span.value_end, encode(desired)));
            }
        }
    }

    fn parse_string(value: &str) -> Option<String> {
        let value = value.trim();
        value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .map(str::to_string)
    }

    fn validate_starlark(text: &str) -> Result<(), String> {
        let bytes = text.as_bytes();
        let mut stack = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if skip_literal_or_comment(bytes, &mut i)? {
                continue;
            }
            match bytes[i] {
                b'(' | b'[' | b'{' => stack.push(bytes[i]),
                b')' | b']' | b'}' => {
                    let expected = match bytes[i] {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    if stack.pop() != Some(expected) {
                        return Err(format!("unbalanced delimiter at byte {i}"));
                    }
                }
                _ => {}
            }
            i += 1;
        }
        if stack.is_empty() {
            Ok(())
        } else {
            Err("unclosed delimiter".into())
        }
    }

    fn find_calls(text: &str, target: &str) -> Result<Vec<(usize, usize)>, String> {
        let bytes = text.as_bytes();
        let mut calls = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if skip_literal_or_comment(bytes, &mut i)? {
                continue;
            }
            if is_ident_start(bytes[i]) {
                let start = i;
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
                let mut open = i;
                skip_trivia(bytes, &mut open)?;
                if bytes.get(open) == Some(&b'(') {
                    let close = matching_paren(bytes, open)?;
                    let qualified = text[..start]
                        .trim_end_matches(|c: char| c.is_ascii_whitespace())
                        .ends_with('.');
                    if &text[start..i] == target && !qualified {
                        calls.push((start, close + 1));
                    }
                    // File.Rules ignores calls nested inside another call.
                    i = close + 1;
                }
            } else {
                i += 1;
            }
        }
        Ok(calls)
    }

    fn find_attr(text: &str, start: usize, end: usize, name: &str) -> Option<AttrSpan> {
        let bytes = text.as_bytes();
        let open = text[start..end].find('(')? + start;
        let mut depth = 0_i32;
        let mut i = open + 1;
        while i + 1 < end {
            if skip_literal_or_comment(bytes, &mut i).ok()? {
                continue;
            }
            match bytes[i] {
                b'(' | b'[' | b'{' => {
                    depth += 1;
                    i += 1;
                }
                b')' | b']' | b'}' if depth > 0 => {
                    depth -= 1;
                    i += 1;
                }
                c if depth == 0 && is_ident_start(c) => {
                    let ident_start = i;
                    i += 1;
                    while i < end && is_ident_continue(bytes[i]) {
                        i += 1;
                    }
                    if &text[ident_start..i] != name {
                        continue;
                    }
                    let mut equals = i;
                    skip_trivia(bytes, &mut equals).ok()?;
                    if bytes.get(equals) != Some(&b'=') {
                        continue;
                    }
                    let mut value_start = equals + 1;
                    skip_trivia(bytes, &mut value_start).ok()?;
                    let (value_end, comma_end) =
                        attr_value_end(bytes, value_start, end - 1).ok()?;
                    let line_start = text[..ident_start].rfind('\n').map_or(0, |p| p + 1);
                    let item_start = if text[line_start..ident_start].trim().is_empty() {
                        line_start
                    } else {
                        ident_start
                    };
                    let mut item_end = comma_end;
                    while item_end < end && matches!(bytes[item_end], b' ' | b'\t' | b'\r') {
                        item_end += 1;
                    }
                    if item_end < end && bytes[item_end] == b'\n' {
                        item_end += 1;
                    }
                    return Some(AttrSpan {
                        item_start,
                        item_end,
                        value_start,
                        value_end,
                    });
                }
                _ => i += 1,
            }
        }
        None
    }

    fn attr_value_end(bytes: &[u8], start: usize, close: usize) -> Result<(usize, usize), String> {
        let mut stack = Vec::new();
        let mut i = start;
        while i < close {
            if skip_literal_or_comment(bytes, &mut i)? {
                continue;
            }
            match bytes[i] {
                b'(' | b'[' | b'{' => stack.push(bytes[i]),
                b')' | b']' | b'}' if !stack.is_empty() => {
                    stack.pop();
                }
                b',' if stack.is_empty() => {
                    let value_end = trim_end(bytes, start, i);
                    return Ok((value_end, i + 1));
                }
                _ => {}
            }
            i += 1;
        }
        Ok((trim_end(bytes, start, close), close))
    }

    fn matching_paren(bytes: &[u8], open: usize) -> Result<usize, String> {
        let mut depth = 1;
        let mut i = open + 1;
        while i < bytes.len() {
            if skip_literal_or_comment(bytes, &mut i)? {
                continue;
            }
            match bytes[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        Err(format!("unclosed call at byte {open}"))
    }

    fn skip_trivia(bytes: &[u8], i: &mut usize) -> Result<(), String> {
        loop {
            while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
                *i += 1;
            }
            if !skip_comment(bytes, i)? {
                return Ok(());
            }
        }
    }

    fn skip_literal_or_comment(bytes: &[u8], i: &mut usize) -> Result<bool, String> {
        if skip_comment(bytes, i)? {
            return Ok(true);
        }
        let Some(&quote) = bytes.get(*i) else {
            return Ok(false);
        };
        if quote != b'"' && quote != b'\'' {
            return Ok(false);
        }
        let start = *i;
        let triple = bytes
            .get(*i..*i + 3)
            .is_some_and(|v| v == [quote, quote, quote]);
        *i += if triple { 3 } else { 1 };
        while *i < bytes.len() {
            if triple
                && bytes
                    .get(*i..*i + 3)
                    .is_some_and(|v| v == [quote, quote, quote])
            {
                *i += 3;
                return Ok(true);
            }
            if !triple && bytes[*i] == quote {
                *i += 1;
                return Ok(true);
            }
            if bytes[*i] == b'\\' {
                *i += 2;
            } else {
                *i += 1;
            }
        }
        Err(format!("unterminated string at byte {start}"))
    }

    fn skip_comment(bytes: &[u8], i: &mut usize) -> Result<bool, String> {
        if bytes.get(*i) == Some(&b'#') {
            while *i < bytes.len() && bytes[*i] != b'\n' {
                *i += 1;
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn trim_end(bytes: &[u8], start: usize, mut end: usize) -> usize {
        while end > start && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        end
    }

    fn is_ident_start(byte: u8) -> bool {
        byte == b'_' || byte.is_ascii_alphabetic()
    }

    fn is_ident_continue(byte: u8) -> bool {
        is_ident_start(byte) || byte.is_ascii_digit()
    }
}

/// Optional: expose empty map type alias used in docs.
/// 这里只是为了补齐文档和接口外观，当前 `tazel` 运行时并不依赖该别名。
pub type AttrMap = HashMap<String, String>;
