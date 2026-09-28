// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// SQL 规范化（normalize）与摘要（digest）实现。
//
// 数据库内核常把语义相同但字面量不同的 SQL（如 `SELECT 1` 与 `SELECT 2`）
// 归一成同一模板（字面量替换为 `?`），再对模板做 SHA-256，得到 digest。
// Digest 用于慢查询归类、SQL Binding（绑定执行计划）匹配、以及日志脱敏。
//
// 本模块提供轻量词法扫描器与 [`SqlDigester`] 状态机，对齐 Go
// `pkg/parser/digester.go`：支持 OFF/ON/MARKER 脱敏模式、绑定专用 IN/ROW
// 列表折叠，以及可选保留优化器 hint。

// 本文件对齐 pkg/parser/digester.go 的 SQL 规范化与摘要状态机。
// 实现扫描调用方传入的 SQL 并在内存中生成 SHA-256。
use sha2::{Digest as ShaDigestTrait, Sha256};

/// 脱敏关闭：原样返回 SQL。
const REDACT_LOG_DISABLE: &str = "OFF";
/// 脱敏开启：字面量替换为 `?` / `...`。
const REDACT_LOG_ENABLE: &str = "ON";
/// MARKER 模式：字面量包在 ‹› 中保留可读值。
const REDACT_LOG_MARKER: &str = "MARKER";

/// 非法/未识别 token。
const INVALID: i32 = -100;
/// 普通标识符。
const IDENTIFIER: i32 = 1;
/// 反引号包裹的标识符。
const QUOTED_IDENTIFIER: i32 = 2;
/// 字符串字面量。
const STRING_LIT: i32 = 3;
/// 位串字面量（0b...）。
const BIT_LIT: i32 = 4;
/// 预处理参数占位符 `?`。
const PARAM_MARKER: i32 = 5;
/// NULL / `\N`。
const NULL: i32 = 6;
/// 整型字面量。
const INT_LIT: i32 = 7;
/// 小数字面量。
const DEC_LIT: i32 = 8;
/// 浮点字面量。
const FLOAT_LIT: i32 = 9;
/// 十六进制字面量（0x...）。
const HEX_LIT: i32 = 10;
/// `@` 用户变量前缀标识。
const SINGLE_AT_IDENTIFIER: i32 = 11;
/// `_charset` 字符集引导符（如 `_utf8mb4`）。
const UNDERSCORE_CS: i32 = 12;
/// 优化器 hint 注释 `/*+ ... */`。
const HINT_COMMENT: i32 = 13;
/// 关键字或内置函数名。
const KEYWORD: i32 = 14;
/// `@@global.var` / `@@session.var` 系统变量。
const DOUBLE_AT_IDENTIFIER: i32 = 15;

/// 扫描位置：记录当前 token 在 SQL 中的起始字节偏移。
#[derive(Clone, Copy)]
struct ScanPos {
    /// 字节偏移。
    offset: usize,
}

/// 仅用于规范化的轻量词法扫描器。
///
/// 刻意镜像 Go sqlDigester 消费的 token 形态，不引入完整解析器 AST 状态。
/// A normalization-only scanner. It deliberately mirrors the token shapes
/// consumed by Go's sqlDigester without pulling parser AST state into this
/// small, reusable component.
struct Scanner {
    /// 展开特殊注释后的 SQL 文本。
    sql: String,
    /// 当前扫描字节偏移。
    offset: usize,
    /// 是否保留 `/*+` hint 注释内容。
    keep_hint: bool,
}

impl Scanner {
    /// 构造扫描器；初始不展开特殊注释（由 `reset` 负责）。
    fn new(sql: &str) -> Self {
        Self {
            sql: sql.to_owned(),
            offset: 0,
            keep_hint: false,
        }
    }

    /// 重置扫描文本：先展开 `/*!` / `/*T!` 版本注释，再归零偏移。
    fn reset(&mut self, sql: &str) {
        self.sql.clear();
        self.sql.push_str(&expand_special_comments(sql));
        self.offset = 0;
    }

    /// 设置是否保留优化器 hint 注释。
    fn set_keep_hint(&mut self, keep: bool) {
        self.keep_hint = keep;
    }
    /// 是否已扫描到文本末尾。
    fn eof(&self) -> bool {
        self.offset >= self.sql.len()
    }

    /// 扫描 @ 变量名：支持普通、单/双引号和反引号形式。
    fn scan_variable_name(&mut self) -> Option<String> {
        let bytes = self.sql.as_bytes();
        let start = self.offset;
        let quote = *bytes.get(start)?;
        if matches!(quote, b'\'' | b'"' | b'`') {
            self.offset += 1;
            let mut value = String::new();
            while self.offset < bytes.len() {
                let byte = bytes[self.offset];
                self.offset += 1;
                if byte == quote {
                    if self.offset < bytes.len() && bytes[self.offset] == quote {
                        value.push(quote as char);
                        self.offset += 1;
                        continue;
                    }
                    return Some(value);
                }
                if byte == b'\\' && quote != b'`' && self.offset < bytes.len() {
                    value.push(bytes[self.offset] as char);
                    self.offset += 1;
                } else {
                    value.push(byte as char);
                }
            }
            return None;
        }

        while self.offset < bytes.len()
            && (is_ident_byte(bytes[self.offset]) || matches!(bytes[self.offset], b'.' | b'$'))
        {
            self.offset += 1;
        }
        (self.offset != start).then(|| self.sql[start..self.offset].to_owned())
    }

    /// 扫描下一个 token，返回 (kind, 起始位置, 字面量文本)。
    ///
    /// 跳过空白与普通注释；在 `keep_hint` 时把 `/*+` 作为 HINT_COMMENT 返回。
    fn scan(&mut self) -> (i32, ScanPos, String) {
        // 跳过空白、行注释与块注释（可选保留 hint）。
        loop {
            while self.offset < self.sql.len()
                && self.sql.as_bytes()[self.offset].is_ascii_whitespace()
            {
                self.offset += 1;
            }
            let start = self.offset;
            if self.eof() {
                return (0, ScanPos { offset: start }, String::new());
            }
            let rest = &self.sql[start..];
            let dash_comment = rest.starts_with("--")
                && rest
                    .get(2..)
                    .and_then(|tail| tail.chars().next())
                    .is_none_or(char::is_whitespace);
            if dash_comment || rest.starts_with('#') {
                self.offset += rest.find('\n').unwrap_or(rest.len());
                continue;
            }
            if rest.starts_with("/*") {
                let Some(end) = rest.find("*/").map(|at| at + 2) else {
                    self.offset = self.sql.len();
                    return (INVALID, ScanPos { offset: start }, String::new());
                };
                self.offset += end;
                if rest.starts_with("/*+") && self.keep_hint {
                    return (
                        HINT_COMMENT,
                        ScanPos { offset: start },
                        rest[..end].to_owned(),
                    );
                }
                continue;
            }
            break;
        }

        let start = self.offset;
        let bytes = self.sql.as_bytes();
        let first = bytes[start];
        // 字符串字面量：支持转义与引号加倍。
        if first == b'\'' || first == b'"' {
            let quote = first;
            self.offset += 1;
            let mut value = String::new();
            while self.offset < bytes.len() {
                let byte = bytes[self.offset];
                self.offset += 1;
                if byte == quote {
                    if self.offset < bytes.len() && bytes[self.offset] == quote {
                        value.push(quote as char);
                        self.offset += 1;
                        continue;
                    }
                    return (STRING_LIT, ScanPos { offset: start }, value);
                }
                if byte == b'\\' && self.offset < bytes.len() {
                    value.push(bytes[self.offset] as char);
                    self.offset += 1;
                } else {
                    value.push(byte as char);
                }
            }
            return (INVALID, ScanPos { offset: start }, String::new());
        }
        // 反引号标识符：`` 表示字面反引号。
        if first == b'`' {
            self.offset += 1;
            let mut value = String::new();
            while self.offset < bytes.len() {
                let byte = bytes[self.offset];
                self.offset += 1;
                if byte == b'`' {
                    if self.offset < bytes.len() && bytes[self.offset] == b'`' {
                        value.push('`');
                        self.offset += 1;
                        continue;
                    }
                    return (QUOTED_IDENTIFIER, ScanPos { offset: start }, value);
                }
                value.push(byte as char);
            }
            return (INVALID, ScanPos { offset: start }, String::new());
        }
        // 用户变量与系统变量；@@ 变量必须作为单个 token 保留原始连接形状。
        if first == b'@' {
            self.offset += 1;
            if bytes.get(self.offset) == Some(&b'@') {
                self.offset += 1;
                let mut prefix = "";
                for candidate in ["global.", "session.", "local."] {
                    let end = self.offset + candidate.len();
                    if self
                        .sql
                        .get(self.offset..end)
                        .is_some_and(|value| value.eq_ignore_ascii_case(candidate))
                    {
                        prefix = candidate;
                        self.offset = end;
                        break;
                    }
                }
                let quoted =
                    matches!(self.sql.as_bytes().get(self.offset), Some(b'\'' | b'"' | b'`'));
                let name = self.scan_variable_name();
                if quoted && name.is_none() {
                    return (INVALID, ScanPos { offset: start }, String::new());
                }
                return (
                    DOUBLE_AT_IDENTIFIER,
                    ScanPos { offset: start },
                    format!("@@{prefix}{}", name.unwrap_or_default()),
                );
            }
            let quoted = matches!(
                self.sql.as_bytes().get(self.offset),
                Some(b'\'' | b'"' | b'`')
            );
            let name = self.scan_variable_name();
            if quoted && name.is_none() {
                return (INVALID, ScanPos { offset: start }, String::new());
            }
            return (
                SINGLE_AT_IDENTIFIER,
                ScanPos { offset: start },
                name.unwrap_or_default(),
            );
        }
        if first == b'?' {
            self.offset += 1;
            return (PARAM_MARKER, ScanPos { offset: start }, "?".to_owned());
        }
        // MySQL 客户端 `\N` 表示 NULL。
        if first == b'\\' && bytes.get(start + 1) == Some(&b'N') {
            self.offset += 2;
            return (NULL, ScanPos { offset: start }, "null".to_owned());
        }
        // 数字字面量：再按前缀/形态细分为 HEX/BIT/FLOAT/DEC/INT。
        if first.is_ascii_digit()
            || (first == b'.' && bytes.get(start + 1).is_some_and(u8::is_ascii_digit))
        {
            let mut numeric_identifier = false;
            if first == b'0' && matches!(bytes.get(start + 1), Some(b'x' | b'X')) {
                self.offset += 2;
                let digits = self.offset;
                while self.offset < bytes.len() && bytes[self.offset].is_ascii_hexdigit() {
                    self.offset += 1;
                }
                numeric_identifier = self.offset == digits;
                if self.offset < bytes.len() && is_ident_byte(bytes[self.offset]) {
                    while self.offset < bytes.len() && is_ident_byte(bytes[self.offset]) {
                        self.offset += 1;
                    }
                    numeric_identifier = true;
                }
            } else if first == b'0' && matches!(bytes.get(start + 1), Some(b'b' | b'B')) {
                self.offset += 2;
                let digits = self.offset;
                while self.offset < bytes.len() && matches!(bytes[self.offset], b'0' | b'1') {
                    self.offset += 1;
                }
                numeric_identifier = self.offset == digits;
                if self.offset < bytes.len() && is_ident_byte(bytes[self.offset]) {
                    while self.offset < bytes.len() && is_ident_byte(bytes[self.offset]) {
                        self.offset += 1;
                    }
                    numeric_identifier = true;
                }
            } else {
                self.offset = start;
                while self.offset < bytes.len() && bytes[self.offset].is_ascii_digit() {
                    self.offset += 1;
                }
                let mut is_float = false;
                if bytes.get(self.offset) == Some(&b'.') {
                    is_float = true;
                    self.offset += 1;
                    while self.offset < bytes.len() && bytes[self.offset].is_ascii_digit() {
                        self.offset += 1;
                    }
                }
                if matches!(bytes.get(self.offset), Some(b'e' | b'E')) {
                    let exponent = self.offset;
                    self.offset += 1;
                    if matches!(bytes.get(self.offset), Some(b'+' | b'-')) {
                        self.offset += 1;
                    }
                    let digits = self.offset;
                    while self.offset < bytes.len() && bytes[self.offset].is_ascii_digit() {
                        self.offset += 1;
                    }
                    if self.offset == digits {
                        if is_float {
                            self.offset = exponent;
                        } else {
                            self.offset = exponent + 1;
                            while self.offset < bytes.len() && is_ident_byte(bytes[self.offset]) {
                                self.offset += 1;
                            }
                            numeric_identifier = true;
                        }
                    } else {
                        is_float = true;
                    }
                }
                if !numeric_identifier
                    && !is_float
                    && self.offset < bytes.len()
                    && is_ident_byte(bytes[self.offset])
                {
                    while self.offset < bytes.len() && is_ident_byte(bytes[self.offset]) {
                        self.offset += 1;
                    }
                    numeric_identifier = true;
                }
            }
            let literal = self.sql[start..self.offset].to_owned();
            let lower = literal.to_ascii_lowercase();
            let kind = if numeric_identifier {
                IDENTIFIER
            } else if lower.starts_with("0x") {
                if lower.len() == 2 || !lower[2..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    INVALID
                } else {
                    HEX_LIT
                }
            } else if lower.starts_with("0b") {
                if lower.len() == 2 || !lower[2..].bytes().all(|byte| matches!(byte, b'0' | b'1')) {
                    INVALID
                } else {
                    BIT_LIT
                }
            } else if lower.contains('e') {
                FLOAT_LIT
            } else if lower.contains('.') {
                DEC_LIT
            } else {
                INT_LIT
            };
            return (kind, ScanPos { offset: start }, literal);
        }
        // 标识符起始：字母、下划线或高位字节（非 ASCII）。
        if is_ident_start(first) {
            self.offset += 1;
            while self.offset < bytes.len() && is_ident_byte(bytes[self.offset]) {
                self.offset += 1;
            }
            let literal = self.sql[start..self.offset].to_owned();
            return (IDENTIFIER, ScanPos { offset: start }, literal);
        }

        // 多字符运算符优先匹配，避免被单字符路径拆开。
        for operator in ["<=>", "->>", ">=", "<=", "!=", "<>", "||", "&&", ":=", "->"] {
            if self.sql[start..].starts_with(operator) {
                self.offset += operator.len();
                return (
                    operator.as_bytes()[0] as i32,
                    ScanPos { offset: start },
                    operator.to_owned(),
                );
            }
        }
        let ch = self.sql[start..].chars().next().unwrap();
        self.offset += ch.len_utf8();
        (ch as i32, ScanPos { offset: start }, ch.to_string())
    }

    /// 判断标识符是否应提升为 KEYWORD（关键字或内置函数）。
    ///
    /// 限定名（点号两侧）永不提升；内置函数仅在紧跟 `(` 时提升。
    fn token_identifier(&self, word: &str, offset: usize) -> Option<i32> {
        // Keep qualified names as identifiers. This matches Scanner.isTokenIdentifier:
        // neither side of `.` is eligible for keyword/function-token promotion.
        if self.sql.as_bytes().get(self.offset) == Some(&b'.')
            || self.sql.as_bytes()[..offset]
                .iter()
                .rev()
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(&b'.')
        {
            return None;
        }

        // Go promotes built-in function names only when `(` immediately follows.
        // The digester scanner runs with the default SQL mode, so IGNORE_SPACE does
        // not apply here.
        if self.sql.as_bytes().get(self.offset) == Some(&b'(') && is_builtin_function(word) {
            return Some(KEYWORD);
        }
        is_keyword(word).then_some(KEYWORD)
    }
}

/// 标识符起始字节判定。
fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}
/// 标识符后续字节判定（含数字与 `$`）。
fn is_ident_byte(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit() || byte == b'$'
}

/// 是否为非窗口函数关键字（窗口函数名保持标识符以便区分）。
fn is_keyword(word: &str) -> bool {
    !is_window_function(word)
        && super::keywords::Keywords
            .iter()
            .any(|keyword| keyword.Word.eq_ignore_ascii_case(word))
}

/// 窗口函数及相关子句关键字表。
fn is_window_function(word: &str) -> bool {
    const WINDOW_TOKENS: &[&str] = &[
        "CUME_DIST",
        "DENSE_RANK",
        "FIRST_VALUE",
        "GROUPS",
        "LAG",
        "LAST_VALUE",
        "LEAD",
        "NTH_VALUE",
        "NTILE",
        "OVER",
        "PERCENT_RANK",
        "RANK",
        "ROW_NUMBER",
        "WINDOW",
    ];
    WINDOW_TOKENS
        .iter()
        .any(|name| name.eq_ignore_ascii_case(word))
}

/// 内置聚合/日期等函数名表（紧跟 `(` 时提升为 KEYWORD）。
fn is_builtin_function(word: &str) -> bool {
    const BUILTINS: &[&str] = &[
        "BIT_AND",
        "BIT_OR",
        "BIT_XOR",
        "CAST",
        "COUNT",
        "APPROX_COUNT_DISTINCT",
        "APPROX_PERCENTILE",
        "CURDATE",
        "CURTIME",
        "DATE_ADD",
        "DATE_SUB",
        "EXTRACT",
        "GROUP_CONCAT",
        "MAX",
        "MID",
        "MIN",
        "NOW",
        "POSITION",
        "STD",
        "STDDEV",
        "STDDEV_POP",
        "STDDEV_SAMP",
        "SUBSTR",
        "SUBSTRING",
        "SUM",
        "SUM_INT",
        "SYSDATE",
        "TRANSLATE",
        "TRIM",
        "VARIANCE",
        "VAR_POP",
        "VAR_SAMP",
    ];
    BUILTINS.iter().any(|name| name.eq_ignore_ascii_case(word))
}

/// 展开 MySQL/TiDB 版本条件注释：`/*!...*/` 与 `/*T![...] ...*/`。
///
/// 普通块注释原样保留；未闭合注释把剩余文本追加后返回。
fn expand_special_comments(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(start) = rest.find("/*") {
        output.push_str(&rest[..start]);
        let comment = &rest[start..];
        let Some(end) = comment.find("*/") else {
            output.push_str(comment);
            return output;
        };
        let body = &comment[2..end];
        if let Some(versioned) = body.strip_prefix('!') {
            output.push_str(
                versioned
                    .trim_start_matches(|ch: char| ch.is_ascii_digit())
                    .trim(),
            );
        } else if let Some(feature) = body.strip_prefix("T!") {
            let feature = feature.trim_start();
            if let Some(feature) = feature.strip_prefix('[') {
                if let Some(close) = feature.find(']') {
                    let ids = feature[..close].split(',').map(str::trim);
                    if ids
                        .clone()
                        .all(|id| matches!(id, "auto_rand" | "clustered_index"))
                    {
                        output.push_str(feature[close + 1..].trim());
                    }
                }
            } else {
                output.push_str(feature.trim());
            }
        } else {
            output.push_str(&comment[..end + 2]);
        }
        rest = &comment[end + 2..];
    }
    output.push_str(rest);
    output
}

fn charset_get_info(name: &str) -> Result<(), ()> {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "utf8" | "utf8mb4" | "binary" | "latin1" | "ascii"
    )
    .then_some(())
    .ok_or(())
}

// Digest 保存固定长度摘要字节及预先编码的十六进制字符串。
/// SQL digest：保存摘要字节与预编码的十六进制文本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Digest {
    /// 原始摘要字节（通常为 SHA-256 的 32 字节）。
    bytes: Vec<u8>,
    /// 小写十六进制编码缓存。
    text: String,
}

impl Digest {
    // NewDigest 对应 Go 构造函数；创建时一次性完成 hex 编码。
    /// 由摘要字节构造；创建时一次性完成 hex 编码。
    pub fn new(bytes: Vec<u8>) -> Self {
        let text = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Self { bytes, text }
    }

    // String 返回缓存的摘要十六进制文本。
    /// 返回缓存的摘要十六进制文本。
    pub fn string(&self) -> &str {
        &self.text
    }

    // Bytes 返回底层摘要字节；借用避免 Rust 侧无意义复制。
    /// 返回底层摘要字节的只读视图。
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    // String 保留 Go 导出方法名，语义与上面的 Rust 风格访问器一致。
    /// Go 风格导出方法名，同 [`Self::string`]。
    pub fn String(&self) -> &str {
        self.string()
    }

    // Bytes 保留 Go 导出方法名；返回摘要字节的只读视图。
    /// Go 风格导出方法名，同 [`Self::bytes`]。
    pub fn Bytes(&self) -> &[u8] {
        self.bytes()
    }
}

/// 由摘要字节构造 [`Digest`]，对应 Go `NewDigest`。
pub fn NewDigest(bytes: Vec<u8>) -> Digest {
    Digest::new(bytes)
}

// with_digester 对应 sync.Pool 的 Get/Put 生命周期。
// Rust 每次创建独立实例，避免跨调用保留超大 SQL 缓冲区，也无需共享非线程安全的 Scanner。
/// 借用临时 [`SqlDigester`] 执行闭包（对齐 Go sync.Pool Get/Put 生命周期）。
fn with_digester<T>(f: impl FnOnce(&mut SqlDigester) -> T) -> T {
    let mut digester = SqlDigester::new();
    f(&mut digester)
}

// DigestHash 兼容已废弃 Go API：先规范化 SQL，再计算摘要。
/// 先规范化再计算摘要（兼容已废弃 Go API）。
pub fn DigestHash(sql: &str) -> Digest {
    with_digester(|digester| digester.do_digest(sql))
}

// DigestNormalized 对已经规范化的 SQL 直接计算摘要；调用方必须保证输入已经过 Normalize。
/// 对已规范化 SQL 直接计算摘要；调用方须保证输入已经过 Normalize。
pub fn DigestNormalized(normalized: &str) -> Digest {
    with_digester(|digester| digester.do_digest_normalized(normalized))
}

// Normalize 根据 OFF、ON、MARKER redact 模式生成规范 SQL。
/// 按 OFF/ON/MARKER 脱敏模式生成规范 SQL。
pub fn Normalize(sql: &str, redact: &str) -> String {
    if redact.is_empty() || redact == REDACT_LOG_DISABLE {
        return sql.to_owned();
    }
    with_digester(|digester| digester.do_normalize(sql, redact, false))
}

// NormalizeForBinding 应用绑定专用的 IN/ROW 列表归并规则。
/// 应用 SQL Binding 专用的 IN/ROW 列表归并规则。
pub fn NormalizeForBinding(sql: &str, for_plan_replayer_reload: bool) -> String {
    with_digester(|digester| {
        digester.do_normalize_for_binding(sql, false, for_plan_replayer_reload)
    })
}

// NormalizeKeepHint 规范化字面量但保留优化器 hint。
/// 规范化字面量但保留优化器 hint 文本。
pub fn NormalizeKeepHint(sql: &str) -> String {
    with_digester(|digester| digester.do_normalize(sql, REDACT_LOG_ENABLE, true))
}

// NormalizeDigest 一次扫描同时返回规范 SQL 和其 SHA-256 摘要。
/// 一次扫描同时返回规范 SQL 与其 SHA-256 摘要。
pub fn NormalizeDigest(sql: &str) -> (String, Digest) {
    with_digester(|digester| digester.do_normalize_digest(sql))
}

// NormalizeDigestForBinding 在绑定规则下同时返回规范 SQL 和摘要。
/// 在绑定规则下同时返回规范 SQL 和摘要。
pub fn NormalizeDigestForBinding(sql: &str) -> (String, Digest) {
    with_digester(|digester| digester.do_normalize_digest_for_binding(sql))
}

// SqlDigester 对应 Go sqlDigester，集中持有输出缓冲、词法扫描器、哈希器和 token 双端队列。
/// SQL 规范化与摘要状态机，集中持有缓冲、扫描器、哈希器与 token 队列。
pub struct SqlDigester {
    /// 规范化输出缓冲。
    buffer: String,
    /// 规范化专用词法扫描器。
    lexer: Scanner,
    /// SHA-256 哈希器。
    hasher: Sha256,
    /// 归并过程中的 token 双端队列。
    tokens: TokenDeque,
}

impl SqlDigester {
    /// 创建空状态的 digester。
    fn new() -> Self {
        Self {
            buffer: String::new(),
            lexer: Scanner::new(""),
            hasher: Sha256::new(),
            tokens: TokenDeque::default(),
        }
    }

    // finish_hash 对应 Go hasher.Sum(nil) 后 Reset，确保池化复用时不串联上一次输入。
    /// 取出哈希结果并重置哈希器，避免串联上一次输入。
    fn finish_hash(&mut self) -> Digest {
        let bytes = self.hasher.finalize_reset().to_vec();
        Digest::new(bytes)
    }

    /// 对已规范化文本直接更新哈希并返回 digest。
    fn do_digest_normalized(&mut self, normalized: &str) -> Digest {
        self.hasher.update(normalized.as_bytes());
        self.finish_hash()
    }

    /// 规范化后对缓冲计算 digest，并清空缓冲。
    fn do_digest(&mut self, sql: &str) -> Digest {
        self.normalize(sql, REDACT_LOG_ENABLE, false, false, false);
        self.hasher.update(self.buffer.as_bytes());
        self.buffer.clear();
        self.finish_hash()
    }

    /// 执行规范化并取出输出缓冲。
    fn do_normalize(&mut self, sql: &str, redact: &str, keep_hint: bool) -> String {
        self.normalize(sql, redact, keep_hint, false, false);
        std::mem::take(&mut self.buffer)
    }

    /// 绑定路径规范化（可切换 plan replayer reload 规则）。
    fn do_normalize_for_binding(&mut self, sql: &str, keep_hint: bool, reload: bool) -> String {
        self.normalize(sql, REDACT_LOG_ENABLE, keep_hint, true, reload);
        std::mem::take(&mut self.buffer)
    }

    /// 同时产出规范 SQL 与 digest。
    fn do_normalize_digest(&mut self, sql: &str) -> (String, Digest) {
        self.normalize(sql, REDACT_LOG_ENABLE, false, false, false);
        let normalized = self.buffer.clone();
        self.hasher.update(self.buffer.as_bytes());
        self.buffer.clear();
        (normalized, self.finish_hash())
    }

    /// 绑定规则下同时产出规范 SQL 与 digest。
    fn do_normalize_digest_for_binding(&mut self, sql: &str) -> (String, Digest) {
        self.normalize(sql, REDACT_LOG_ENABLE, false, true, false);
        let normalized = self.buffer.clone();
        self.hasher.update(self.buffer.as_bytes());
        self.buffer.clear();
        (normalized, self.finish_hash())
    }

    // normalize 对应 Go 主扫描循环：依序做 hint、字面量、绑定规则和标识符归类，再统一渲染。
    fn normalize(
        &mut self,
        sql: &str,
        redact: &str,
        keep_hint: bool,
        for_binding: bool,
        reload: bool,
    ) {
        self.lexer.reset(sql);
        self.lexer.set_keep_hint(keep_hint);
        loop {
            let (kind, pos, literal) = self.lexer.scan();
            if kind == INVALID
                || (kind == 0 && self.lexer.eof())
                || pos.offset == self.lexer.sql.len()
                || (pos.offset + 1 == self.lexer.sql.len()
                    && self.lexer.sql.as_bytes().get(pos.offset) == Some(&b';'))
            {
                break;
            }
            let normalized_literal = if kind == HINT_COMMENT {
                literal
            } else {
                literal.to_lowercase()
            };
            let mut current = Token::new(kind, normalized_literal);
            if !keep_hint && self.reduce_optimizer_hint(&mut current) {
                continue;
            }
            self.reduce_lit(&mut current, redact, for_binding, reload);
            // plan replayer reload 与 binding 使用不同的 IN/ROW 折叠规则。
            if reload {
                self.replace_single_literal_with_in_list(&current);
            } else if for_binding {
                self.reduce_in_list_with_single_literal(&current);
                self.reduce_in_row_list_with_single_literal(&current);
            }

            // Go classifies identifiers only after literal/list reduction. In
            // particular, an unquoted `null` must still be reduced to `?`.
            // 字面量/列表归并之后再分类标识符：未加引号的 null 须先变成 `?`。
            if current.kind == IDENTIFIER {
                if current.literal.starts_with('_')
                    && charset_get_info(&current.literal[1..]).is_ok()
                {
                    current.kind = UNDERSCORE_CS;
                } else if let Some(keyword_kind) =
                    self.lexer.token_identifier(&current.literal, pos.offset)
                {
                    current.kind = keyword_kind;
                }
            }

            self.tokens.push_back(current);
        }

        // 将 token 队列渲染为规范 SQL 文本。
        self.lexer.reset("");
        for (index, token) in self.tokens.items.iter().enumerate() {
            let follows_at = index > 0
                && self.tokens.items[index - 1].kind == SINGLE_AT_IDENTIFIER
                && self.tokens.items[index - 1].literal.is_empty()
                && matches!(token.kind, QUOTED_IDENTIFIER | STRING_LIT);
            if index > 0 && !follows_at {
                self.buffer.push(' ');
            }
            match token.kind {
                SINGLE_AT_IDENTIFIER => {
                    self.buffer.push('@');
                    self.buffer.push_str(&token.literal);
                }
                UNDERSCORE_CS => self.buffer.push_str("(_charset)"),
                IDENTIFIER | QUOTED_IDENTIFIER => {
                    self.buffer.push('`');
                    self.buffer.push_str(&token.literal);
                    self.buffer.push('`');
                }
                _ => self.buffer.push_str(&token.literal),
            }
        }
        // 末尾有未闭合注释时补一个空格，对齐 Go 输出形状。
        if sql
            .rfind("/*")
            .is_some_and(|start| !sql[start..].contains("*/"))
            && !self.buffer.is_empty()
        {
            self.buffer.push(' ');
        }
        self.tokens.reset();
    }

    // reduce_optimizer_hint 删除 hint comment 与 force/use/ignore index(...)，并把 straight_join 归一为 join。
    fn reduce_optimizer_hint(&mut self, token: &mut Token) -> bool {
        if token.kind == HINT_COMMENT {
            return true;
        }
        if token.literal == "index" {
            if let Some(previous) = self.tokens.back(1).first() {
                if matches!(previous.literal.as_str(), "force" | "use" | "ignore") {
                    loop {
                        let (kind, _, literal) = self.lexer.scan();
                        if kind == INVALID || (kind == 0 && self.lexer.eof()) {
                            break;
                        }
                        if literal == ")" {
                            self.tokens.pop_back(1);
                            return true;
                        }
                    }
                    return false;
                }
            }
        }
        if token.literal == "straight_join" {
            token.literal = "join".to_owned();
        }
        false
    }

    // reduce_lit 保留 Go 的归并顺序；较早规则命中后必须立即返回，避免重复改写 token 队列。
    fn reduce_lit(&mut self, current: &mut Token, redact: &str, for_binding: bool, reload: bool) {
        if !self.is_lit(current) {
            return;
        }
        if redact == REDACT_LOG_MARKER && !for_binding && !reload {
            if matches!(current.literal.as_str(), "?" | "*") {
                return;
            }
            // MARKER 模式把原字面量包在 ‹› 中，并将内部同类界符加倍转义。
            let mut marked = String::with_capacity(current.literal.len() + 2);
            marked.push('‹');
            for ch in current.literal.chars() {
                marked.push(ch);
                if matches!(ch, '‹' | '›') {
                    marked.push(ch);
                }
            }
            marked.push('›');
            current.literal = marked;
            return;
        }
        if current.literal == "*" {
            if self.is_star_param() {
                current.kind = GENERIC_SYMBOL;
                current.literal = "?".to_owned();
            }
            return;
        }
        if self.is_prefix_by_unary(current.kind) {
            self.tokens.pop_back(1);
        }
        if self.is_generic_list(self.tokens.back(2)) {
            self.tokens.pop_back(2);
            current.make_generic_list();
            return;
        }
        let charset_pop = self.is_generic_list_with_charset(self.tokens.back(4));
        if charset_pop != 0 {
            self.tokens.pop_back(charset_pop);
            current.make_generic_list();
            return;
        }
        if self.is_generic_lists(self.tokens.back(4)) {
            self.tokens.pop_back(4);
            current.make_generic_list();
            return;
        }
        if for_binding && self.is_generic_row_lists_with_in(self.tokens.back(9)) {
            self.tokens.pop_back(5);
            current.make_generic_list();
            return;
        }
        if current.kind == INT_LIT && self.is_order_or_group_by() {
            return;
        }
        current.kind = GENERIC_SYMBOL;
        current.literal = "?".to_owned();
    }

    fn is_generic_lists(&self, last: &[Token]) -> bool {
        last.len() >= 4
            && matches!(last[0].kind, GENERIC_SYMBOL | GENERIC_SYMBOL_LIST)
            && last[1].literal == ")"
            && Self::is_comma(&last[2])
            && last[3].literal == "("
    }

    // is_generic_row_lists_with_in 识别 In(Row(...), Row(...)) 的第二个 row 起点。
    /// 识别 `IN (ROW(...), ROW(...))` 中第二个 ROW 的起点模式。
    fn is_generic_row_lists_with_in(&self, last: &[Token]) -> bool {
        last.len() >= 9
            && Self::is_in_keyword(&last[0])
            && Self::is_left_paren(&last[1])
            && Self::is_row_keyword(&last[2])
            && Self::is_left_paren(&last[3])
            && matches!(last[4].kind, GENERIC_SYMBOL | GENERIC_SYMBOL_LIST)
            && Self::is_right_paren(&last[5])
            && Self::is_comma(&last[6])
            && Self::is_row_keyword(&last[7])
            && Self::is_left_paren(&last[8])
    }

    // replace_single_literal_with_in_list 为 plan replayer 把 IN (...) 改回 IN (?)，规避重放解析失败。
    fn replace_single_literal_with_in_list(&mut self, current: &Token) {
        let matches = {
            let last = self.tokens.back(5);
            last.len() == 5
                && Self::is_in_keyword(&last[0])
                && Self::is_left_paren(&last[1])
                && last[2..5].iter().all(|token| token.literal == ".")
                && Self::is_right_paren(current)
        };
        if matches {
            self.tokens.pop_back(3);
            self.tokens
                .push_back(Token::new(GENERIC_SYMBOL, "?".to_owned()));
        }
    }

    // reduce_in_list_with_single_literal 为 binding 把 IN (?) 扩展为 IN (...)。
    fn reduce_in_list_with_single_literal(&mut self, current: &Token) {
        let matches = {
            let last = self.tokens.back(3);
            last.len() == 3
                && Self::is_in_keyword(&last[0])
                && Self::is_left_paren(&last[1])
                && last[2].kind == GENERIC_SYMBOL
                && Self::is_right_paren(current)
        };
        if matches {
            self.tokens.pop_back(1);
            self.tokens
                .push_back(Token::new(GENERIC_SYMBOL_LIST, "...".to_owned()));
        }
    }

    // reduce_in_row_list_with_single_literal 把 In(Row(...)) 统一折叠为 In(...)。
    fn reduce_in_row_list_with_single_literal(&mut self, current: &Token) {
        let matches = {
            let last = self.tokens.back(6);
            last.len() == 6
                && Self::is_in_keyword(&last[0])
                && Self::is_left_paren(&last[1])
                && Self::is_row_keyword(&last[2])
                && Self::is_left_paren(&last[3])
                && matches!(last[4].kind, GENERIC_SYMBOL | GENERIC_SYMBOL_LIST)
                && Self::is_right_paren(&last[5])
                && Self::is_right_paren(current)
        };
        if matches {
            self.tokens.pop_back(4);
            self.tokens
                .push_back(Token::new(GENERIC_SYMBOL_LIST, "...".to_owned()));
        }
    }

    // is_prefix_by_unary 区分一元正负号与二元加减；仅数值字面量参与判断。
    fn is_prefix_by_unary(&self, current_kind: i32) -> bool {
        if !Self::is_num_lit(current_kind) {
            return false;
        }
        let last = self.tokens.back(1);
        if last.is_empty() || !matches!(last[0].literal.as_str(), "-" | "+") {
            return false;
        }
        let last_two = self.tokens.back(2);
        if last_two.len() < 2 {
            return true;
        }
        // 前一个符号处于“表达式起始”位置时，+/- 视为一元前缀。
        matches!(
            last_two[0].literal.as_str(),
            "(" | "," | "+" | "-" | ">=" | "is" | "<=" | "=" | "<" | ">" | "select"
        )
    }

    /// 识别 `, ?` / `, ...` 形态的列表延续。
    fn is_generic_list(&self, last: &[Token]) -> bool {
        last.len() >= 2
            && Self::is_comma(&last[1])
            && matches!(last[0].kind, GENERIC_SYMBOL | GENERIC_SYMBOL_LIST)
    }

    /// 识别带 `_charset` 引导的列表延续，返回应弹出的 token 数。
    fn is_generic_list_with_charset(&self, mut last: &[Token]) -> usize {
        if last.len() < 3 {
            return 0;
        }
        let mut to_pop = 0;
        if last.len() >= 4 {
            if last[0].kind == UNDERSCORE_CS {
                to_pop = 1;
            }
            last = &last[1..];
        }
        if last[2].kind == UNDERSCORE_CS && self.is_generic_list(&last[..2]) {
            to_pop + 3
        } else {
            0
        }
    }

    // is_order_or_group_by 跳过逗号分隔的序号列表，并兼容 group by (1, 2) 的括号。
    fn is_order_or_group_by(&self) -> bool {
        let mut count = 2;
        let mut last;
        loop {
            last = self.tokens.back(count);
            if last.len() < 2 {
                return false;
            }
            if !Self::is_comma(&last[1]) {
                break;
            }
            count += 2;
        }
        if last[1].literal == "(" {
            last = self.tokens.back(count + 1);
            if last.len() < 2 {
                return false;
            }
        }
        matches!(last[0].literal.as_str(), "order" | "group") && last[1].literal == "by"
    }

    fn is_star_param(&self) -> bool {
        self.tokens
            .back(1)
            .first()
            .is_some_and(|token| token.literal == "(")
    }

    /// 判断 token 是否为可归并的字面量（含 `*` 与未加引号 null）。
    fn is_lit(&self, token: &Token) -> bool {
        Self::is_num_lit(token.kind)
            || matches!(token.kind, STRING_LIT | BIT_LIT | PARAM_MARKER | NULL)
            || token.literal == "*"
            || (token.kind == IDENTIFIER && token.literal.eq_ignore_ascii_case("null"))
    }

    /// 是否为数值类字面量 kind。
    fn is_num_lit(kind: i32) -> bool {
        matches!(kind, INT_LIT | DEC_LIT | FLOAT_LIT | HEX_LIT)
    }

    fn is_comma(token: &Token) -> bool {
        token.literal == ","
    }
    fn is_left_paren(token: &Token) -> bool {
        token.literal == "("
    }
    fn is_right_paren(token: &Token) -> bool {
        token.literal == ")"
    }
    fn is_in_keyword(token: &Token) -> bool {
        token.literal == "in"
    }
    fn is_row_keyword(token: &Token) -> bool {
        token.literal == "row"
    }
}

// Token 对应 Go token，kind 保存 lexer token 编号，literal 保存小写后的文本。
/// 规范化过程中的 token：kind 为词法编号，literal 为小写文本。
#[derive(Clone, Debug)]
pub struct Token {
    /// 词法 kind（含 GENERIC_SYMBOL 等合成 kind）。
    kind: i32,
    /// 规范化后的字面量文本。
    literal: String,
}

impl Token {
    /// 构造 token。
    fn new(kind: i32, literal: String) -> Self {
        Self { kind, literal }
    }
    /// 折叠为通用列表占位 `...`。
    fn make_generic_list(&mut self) {
        self.kind = GENERIC_SYMBOL_LIST;
        self.literal = "...".to_owned();
    }
}

// TokenDeque 对应 Go tokenDeque；back 返回尾部切片，长度不足时返回空切片。
/// Token 双端队列；`back` 在长度不足时返回空切片。
#[derive(Default)]
pub struct TokenDeque {
    /// 内部顺序存储，尾部为最近入队 token。
    items: Vec<Token>,
}

impl TokenDeque {
    /// 清空队列。
    fn reset(&mut self) {
        self.items.clear();
    }
    /// 入队到尾部。
    fn push_back(&mut self, token: Token) {
        self.items.push(token);
    }
    /// 弹出尾部 `count` 个 token；不足则返回空向量。
    fn pop_back(&mut self, count: usize) -> Vec<Token> {
        if self.items.len() < count {
            return Vec::new();
        }
        self.items.split_off(self.items.len() - count)
    }
    /// 查看尾部 `count` 个 token；不足则返回空切片。
    fn back(&self, count: usize) -> &[Token] {
        if self.items.len() < count {
            &[]
        } else {
            &self.items[self.items.len() - count..]
        }
    }
}

// genericSymbol 与 genericSymbolList 使用 lexer 不会产生的负值，分别表示“?”和“...”。
/// 合成 kind：通用占位符 `?`。
const GENERIC_SYMBOL: i32 = -1;
/// 合成 kind：通用列表占位 `...`。
const GENERIC_SYMBOL_LIST: i32 = -2;
