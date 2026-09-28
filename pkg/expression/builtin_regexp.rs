// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// MySQL/TiDB 正则内建函数的标量与向量化内核。
//
// 实现 REGEXP_LIKE / REGEXP_SUBSTR / REGEXP_INSTR / REGEXP_REPLACE，
// 含 match type 标志解析、常量模式记忆化编译、UTF-8 与 binary collation
// 路径，以及替换串中 `\0`..`\9` 捕获组指令解析，对应 Go `builtin_regexp.go`。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use regex::{Regex, bytes::Regex as BytesRegex};
use thiserror::Error;

use crate::builtin_regexp_util_kernel::check_out_range_pos;

/// 参数列表中 pattern 参数的下标。
pub const PATTERN_IDX: usize = 1;
/// REGEXP_REPLACE 中 replacement 参数下标。
pub const REPLACEMENT_IDX: usize = 2;
/// REGEXP_LIKE 可选 match_type 参数下标。
pub const REGEXP_LIKE_MATCH_TYPE_IDX: usize = 2;
/// REGEXP_SUBSTR 可选 match_type 参数下标。
pub const REGEXP_SUBSTR_MATCH_TYPE_IDX: usize = 4;
/// REGEXP_INSTR 可选 match_type 参数下标。
pub const REGEXP_INSTR_MATCH_TYPE_IDX: usize = 5;
/// REGEXP_REPLACE 可选 match_type 参数下标。
pub const REGEXP_REPLACE_MATCH_TYPE_IDX: usize = 5;

/// 忽略大小写标志。
const FLAG_I: char = 'i';
/// 强制大小写敏感（取消 i）。
const FLAG_C: char = 'c';
/// 多行模式标志。
const FLAG_M: char = 'm';
/// 让 `.` 匹配换行的标志。
const FLAG_S: char = 's';

#[derive(Clone, Debug, Error, PartialEq, Eq)]
/// 正则内建相关错误（非法 match type、越界、编译失败等）。
pub enum RegexpError {
    #[error("Invalid match type")]
    InvalidMatchType,
    #[error("Index out of bounds in regular expression search")]
    InvalidIndex,
    #[error("Incorrect arguments to regexp_instr: return_option must be 1 or 0")]
    InvalidReturnOption,
    #[error("Substitution number is out of range")]
    InvalidSubstitution,
    #[error("Not support binary collation so far")]
    BinaryCollationUnsupported,
    #[error("Empty pattern is invalid")]
    EmptyPattern,
    #[error("regular expression compilation failed: {0}")]
    Compile(String),
    #[error("regular expression cache lock is poisoned")]
    CachePoisoned,
}

#[derive(Debug)]
/// 同时持有文本与字节两种引擎，供 UTF-8 / binary 路径共用。
pub struct CompiledRegexp {
    text: Regex,
    bytes: BytesRegex,
}

/// 记忆化缓存项：编译成功或失败都会被缓存。
type CachedRegexp = Result<Arc<CompiledRegexp>, RegexpError>;

#[derive(Clone, Debug)]
/// 正则编译与按 context_id 记忆化的公共底座。
pub struct RegexpBase {
    pattern_constant: bool,
    match_type_constant: bool,
    case_insensitive_collation: bool,
    memorized_regexp: Arc<Mutex<HashMap<u64, CachedRegexp>>>,
}

impl RegexpBase {
    /// 构造底座；常量 pattern/match_type 时才允许记忆化。
    pub fn new(
        pattern_constant: bool,
        match_type_constant: bool,
        case_insensitive_collation: bool,
    ) -> Self {
        Self {
            pattern_constant,
            match_type_constant,
            case_insensitive_collation,
            memorized_regexp: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 仅当 pattern（及可选 match_type）均为常量时可记忆化。
    pub fn can_memorize_regexp(&self, has_match_type_argument: bool) -> bool {
        self.pattern_constant && (!has_match_type_argument || self.match_type_constant)
    }

    /// 解析 match type 标志并编译；空 pattern 直接报错。
    pub fn build_regexp(
        &self,
        pattern: &str,
        match_type: &str,
    ) -> Result<Arc<CompiledRegexp>, RegexpError> {
        if pattern.is_empty() {
            return Err(RegexpError::EmptyPattern);
        }
        let flags = get_regexp_match_type(match_type, self.case_insensitive_collation)?;
        let pattern = normalize_go_perl_classes(pattern);
        let source = if flags.is_empty() {
            pattern
        } else {
            format!("(?{flags}){pattern}")
        };
        let text = Regex::new(&source).map_err(|err| RegexpError::Compile(err.to_string()))?;
        let bytes =
            BytesRegex::new(&source).map_err(|err| RegexpError::Compile(err.to_string()))?;
        Ok(Arc::new(CompiledRegexp { text, bytes }))
    }

    /// 默认假定存在 match_type 参数的记忆化/编译入口。
    pub fn get_regexp(&self, context_id: u64, pattern: &str, match_type: &str) -> CachedRegexp {
        self.get_regexp_with_argument(context_id, pattern, match_type, true)
    }

    /// 按是否可记忆化选择直编译或查/写 context_id 缓存。
    pub fn get_regexp_with_argument(
        &self,
        context_id: u64,
        pattern: &str,
        match_type: &str,
        has_match_type_argument: bool,
    ) -> CachedRegexp {
        if !self.can_memorize_regexp(has_match_type_argument) {
            return self.build_regexp(pattern, match_type);
        }
        let mut cache = self
            .memorized_regexp
            .lock()
            .map_err(|_| RegexpError::CachePoisoned)?;
        if let Some(cached) = cache.get(&context_id) {
            return cached.clone();
        }
        let compiled = self.build_regexp(pattern, match_type);
        cache.insert(context_id, compiled.clone());
        compiled
    }

    /// 向量化路径尝试一次记忆化编译；行数为 0 或不可记忆化则返回 (None, false)。
    pub fn try_vec_memorized_regexp(
        &self,
        context_id: u64,
        pattern: &str,
        match_type: &str,
        has_match_type_argument: bool,
        row_count: usize,
    ) -> Result<(Option<Arc<CompiledRegexp>>, bool), RegexpError> {
        if row_count == 0 || !self.can_memorize_regexp(has_match_type_argument) {
            return Ok((None, false));
        }
        if pattern.is_empty() {
            return Err(RegexpError::EmptyPattern);
        }
        self.get_regexp_with_argument(context_id, pattern, match_type, has_match_type_argument)
            .map(|regexp| (Some(regexp), true))
    }

    /// 当前记忆化缓存条目数（锁中毒时返回 0）。
    pub fn cache_len(&self) -> usize {
        self.memorized_regexp
            .lock()
            .map(|cache| cache.len())
            .unwrap_or_default()
    }
}

/// 解析 MySQL match flags：ci collation 默认带 i；用户输入中最右 c/i 生效。
/// Resolves MySQL match flags. A case-insensitive collation supplies `i` by
/// default; within user input the rightmost `c`/`i` decision wins. `m` and `s`
/// are independent set flags.
pub fn get_regexp_match_type(
    user_input_match_type: &str,
    case_insensitive_collation: bool,
) -> Result<String, RegexpError> {
    let mut flags = HashSet::new();
    if case_insensitive_collation {
        flags.insert(FLAG_I);
    }
    for flag in user_input_match_type.chars() {
        match flag {
            FLAG_C => {
                flags.remove(&FLAG_I);
            }
            FLAG_I | FLAG_M | FLAG_S => {
                flags.insert(flag);
            }
            _ => return Err(RegexpError::InvalidMatchType),
        }
    }
    Ok([FLAG_I, FLAG_M, FLAG_S]
        .into_iter()
        .filter(|flag| flags.contains(flag))
        .collect())
}

/// Go's regexp package gives Perl character classes ASCII semantics, while
/// Rust regex enables Unicode classes by default. Spell the classes out and
/// scope word boundaries to ASCII so the rest retains Go's Unicode semantics.
fn normalize_go_perl_classes(pattern: &str) -> String {
    let mut normalized = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            normalized.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            normalized.push('\\');
            break;
        };
        match escaped {
            'd' => normalized.push_str("[0-9]"),
            'D' => normalized.push_str("[^0-9]"),
            'w' => normalized.push_str("[A-Za-z0-9_]"),
            'W' => normalized.push_str("[^A-Za-z0-9_]"),
            's' => normalized.push_str("[\\t\\n\\f\\r ]"),
            'S' => normalized.push_str("[^\\t\\n\\f\\r ]"),
            'b' => normalized.push_str("(?-u:\\b)"),
            'B' => normalized.push_str("(?-u:\\B)"),
            _ => {
                normalized.push('\\');
                normalized.push(escaped);
            }
        }
    }
    normalized
}

#[derive(Clone, Debug)]
/// 对外正则引擎：封装 RegexpBase，并区分 binary collation 路径。
pub struct RegexpEngine {
    base: RegexpBase,
    binary_collation: bool,
}

impl RegexpEngine {
    /// 非常量模式的 UTF-8 引擎。
    pub fn new(case_insensitive_collation: bool) -> Self {
        Self {
            base: RegexpBase::new(false, false, case_insensitive_collation),
            binary_collation: false,
        }
    }

    /// 指定 pattern/match_type 是否常量，以启用记忆化。
    pub fn with_constants(
        case_insensitive_collation: bool,
        pattern_constant: bool,
        match_type_constant: bool,
    ) -> Self {
        Self {
            base: RegexpBase::new(
                pattern_constant,
                match_type_constant,
                case_insensitive_collation,
            ),
            binary_collation: false,
        }
    }

    /// binary collation 引擎；部分 UTF-8 API 会拒绝调用。
    pub fn new_binary() -> Self {
        Self {
            base: RegexpBase::new(false, false, false),
            binary_collation: true,
        }
    }

    /// 是否处于 binary collation 模式。
    pub const fn is_binary_collation(&self) -> bool {
        self.binary_collation
    }

    /// REGEXP_LIKE：匹配返回 1，否则 0。
    pub fn regexp_like(
        &self,
        expression: &str,
        pattern: &str,
        match_type: &str,
    ) -> Result<i64, RegexpError> {
        let regexp = self.base.build_regexp(pattern, match_type)?;
        Ok(i64::from(regexp.text.is_match(expression)))
    }

    /// 带 context 记忆化的 REGEXP_LIKE。
    pub fn regexp_like_cached(
        &self,
        context_id: u64,
        expression: &str,
        pattern: &str,
        match_type: &str,
        has_match_type_argument: bool,
    ) -> Result<i64, RegexpError> {
        let regexp = self.base.get_regexp_with_argument(
            context_id,
            pattern,
            match_type,
            has_match_type_argument,
        )?;
        Ok(i64::from(regexp.text.is_match(expression)))
    }

    /// 向量化 REGEXP_LIKE：任一分量为 NULL 则该行结果为 NULL。
    pub fn regexp_like_vec(
        &self,
        rows: &[(Option<&str>, Option<&str>, Option<&str>)],
    ) -> Result<Vec<Option<i64>>, RegexpError> {
        rows.iter()
            .map(
                |(expression, pattern, match_type)| match (expression, pattern, match_type) {
                    (Some(expression), Some(pattern), Some(match_type)) => {
                        self.regexp_like(expression, pattern, match_type).map(Some)
                    }
                    _ => Ok(None),
                },
            )
            .collect()
    }

    /// REGEXP_SUBSTR：从 position 起找第 occurrence 次匹配子串。
    pub fn regexp_substr(
        &self,
        expression: &str,
        pattern: &str,
        position: i64,
        occurrence: i64,
        match_type: &str,
    ) -> Result<Option<String>, RegexpError> {
        // binary collation 下拒绝走 UTF-8 字符语义 API。
        if self.binary_collation {
            return Err(RegexpError::BinaryCollationUnsupported);
        }
        let (_, trimmed) = trim_utf8_at_position(expression, position, true)?;
        let occurrence = occurrence.max(1) as usize;
        let regexp = self.base.build_regexp(pattern, match_type)?;
        Ok(regexp
            .text
            .find_iter(trimmed)
            .nth(occurrence - 1)
            .map(|matched| matched.as_str().to_owned()))
    }

    /// 向量化 REGEXP_SUBSTR。
    pub fn regexp_substr_vec(
        &self,
        rows: &[Option<RegexpSubstrArgs<'_>>],
    ) -> Result<Vec<Option<String>>, RegexpError> {
        rows.iter()
            .map(|row| match row {
                Some(row) => self.regexp_substr(
                    row.expression,
                    row.pattern,
                    row.position,
                    row.occurrence,
                    row.match_type,
                ),
                None => Ok(None),
            })
            .collect()
    }

    /// binary 路径 SUBSTR：匹配字节以 `0x` 大写十六进制返回。
    pub fn regexp_substr_binary(
        &self,
        expression: &[u8],
        pattern: &str,
        position: i64,
        occurrence: i64,
        match_type: &str,
    ) -> Result<Option<String>, RegexpError> {
        let trimmed = trim_bytes_at_position(expression, position, true)?;
        let occurrence = occurrence.max(1) as usize;
        let regexp = self.base.build_regexp(pattern, match_type)?;
        Ok(regexp
            .bytes
            .find_iter(trimmed)
            .nth(occurrence - 1)
            .map(|matched| format!("0x{}", upper_hex(matched.as_bytes()))))
    }

    /// REGEXP_INSTR：返回第 occurrence 次匹配的起/止字符位置（return_option）。
    pub fn regexp_instr(
        &self,
        expression: &str,
        pattern: &str,
        position: i64,
        occurrence: i64,
        return_option: i64,
        match_type: &str,
    ) -> Result<i64, RegexpError> {
        if self.binary_collation {
            return Err(RegexpError::BinaryCollationUnsupported);
        }
        if return_option != 0 && return_option != 1 {
            return Err(RegexpError::InvalidReturnOption);
        }
        let (_, trimmed) = trim_utf8_for_instr(expression, position)?;
        let occurrence = occurrence.max(1) as usize;
        let regexp = self.base.build_regexp(pattern, match_type)?;
        let Some(matched) = regexp.text.find_iter(trimmed).nth(occurrence - 1) else {
            return Ok(0);
        };
        let byte_position = if return_option == 0 {
            matched.start()
        } else {
            matched.end()
        };
        Ok(trimmed[..byte_position].chars().count() as i64 + position)
    }

    /// 向量化 REGEXP_INSTR。
    pub fn regexp_instr_vec(
        &self,
        rows: &[Option<RegexpInstrArgs<'_>>],
    ) -> Result<Vec<Option<i64>>, RegexpError> {
        rows.iter()
            .map(|row| match row {
                Some(row) => self
                    .regexp_instr(
                        row.expression,
                        row.pattern,
                        row.position,
                        row.occurrence,
                        row.return_option,
                        row.match_type,
                    )
                    .map(Some),
                None => Ok(None),
            })
            .collect()
    }

    /// binary 路径 INSTR：位置按字节偏移计算。
    pub fn regexp_instr_binary(
        &self,
        expression: &[u8],
        pattern: &str,
        position: i64,
        occurrence: i64,
        return_option: i64,
        match_type: &str,
    ) -> Result<i64, RegexpError> {
        if return_option != 0 && return_option != 1 {
            return Err(RegexpError::InvalidReturnOption);
        }
        let trimmed = trim_bytes_for_instr(expression, position)?;
        let regexp = self.base.build_regexp(pattern, match_type)?;
        let Some(matched) = regexp
            .bytes
            .find_iter(trimmed)
            .nth(occurrence.max(1) as usize - 1)
        else {
            return Ok(0);
        };
        let offset = if return_option == 0 {
            matched.start()
        } else {
            matched.end()
        };
        Ok(offset as i64 + position)
    }

    /// REGEXP_REPLACE：按指令串替换第 occurrence 次（0 表示全部）匹配。
    pub fn regexp_replace(
        &self,
        expression: &str,
        pattern: &str,
        replacement: &str,
        position: i64,
        occurrence: i64,
        match_type: &str,
    ) -> Result<String, RegexpError> {
        if self.binary_collation {
            return Err(RegexpError::BinaryCollationUnsupported);
        }
        let (prefix_len, trimmed) = trim_utf8_at_position(expression, position, true)?;
        let occurrence = if occurrence < 0 { 1 } else { occurrence };
        let regexp = self.base.build_regexp(pattern, match_type)?;
        let instructions = get_instructions(replacement.as_bytes());
        let replaced = replace_text_matches(&regexp.text, trimmed, &instructions, occurrence)?;
        Ok(format!("{}{}", &expression[..prefix_len], replaced))
    }

    /// 向量化 REGEXP_REPLACE。
    pub fn regexp_replace_vec(
        &self,
        rows: &[Option<RegexpReplaceArgs<'_>>],
    ) -> Result<Vec<Option<String>>, RegexpError> {
        rows.iter()
            .map(|row| match row {
                Some(row) => self
                    .regexp_replace(
                        row.expression,
                        row.pattern,
                        row.replacement,
                        row.position,
                        row.occurrence,
                        row.match_type,
                    )
                    .map(Some),
                None => Ok(None),
            })
            .collect()
    }

    /// binary 路径 REPLACE：结果以 `0x` 十六进制字符串返回。
    pub fn regexp_replace_binary(
        &self,
        expression: &[u8],
        pattern: &str,
        replacement: &[u8],
        position: i64,
        occurrence: i64,
        match_type: &str,
    ) -> Result<String, RegexpError> {
        let trimmed = trim_bytes_at_position(expression, position, true)?;
        let prefix_len = expression.len() - trimmed.len();
        let occurrence = if occurrence < 0 { 1 } else { occurrence };
        let regexp = self.base.build_regexp(pattern, match_type)?;
        let instructions = get_instructions(replacement);
        let replaced = replace_byte_matches(&regexp.bytes, trimmed, &instructions, occurrence)?;
        let mut result = expression[..prefix_len].to_vec();
        result.extend_from_slice(&replaced);
        Ok(format!("0x{}", upper_hex(&result)))
    }
}

#[derive(Clone, Copy, Debug)]
/// REGEXP_SUBSTR 单行参数视图。
pub struct RegexpSubstrArgs<'a> {
    pub expression: &'a str,
    pub pattern: &'a str,
    pub position: i64,
    pub occurrence: i64,
    pub match_type: &'a str,
}

#[derive(Clone, Copy, Debug)]
/// REGEXP_INSTR 单行参数视图。
pub struct RegexpInstrArgs<'a> {
    pub expression: &'a str,
    pub pattern: &'a str,
    pub position: i64,
    pub occurrence: i64,
    pub return_option: i64,
    pub match_type: &'a str,
}

#[derive(Clone, Copy, Debug)]
/// REGEXP_REPLACE 单行参数视图。
pub struct RegexpReplaceArgs<'a> {
    pub expression: &'a str,
    pub pattern: &'a str,
    pub replacement: &'a str,
    pub position: i64,
    pub occurrence: i64,
    pub match_type: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 替换串解析后的指令：字面量或 `\\n` 捕获组替换。
pub struct Instruction {
    pub substitution_num: Option<usize>,
    pub literal: Vec<u8>,
}

impl Instruction {
    /// 构造捕获组替换指令。
    pub fn substitution(number: usize) -> Self {
        Self {
            substitution_num: Some(number),
            literal: Vec::new(),
        }
    }

    /// 构造字面量输出指令。
    pub fn literal(literal: &[u8]) -> Self {
        Self {
            substitution_num: None,
            literal: literal.to_vec(),
        }
    }
}

/// 解析 TiDB 替换串：`\\0`..`\\9` 为捕获组，其它反斜杠转义单字节，尾部反斜杠忽略。
/// Parses TiDB replacement text. `\\0` through `\\9` are capture
/// substitutions, a backslash before any other byte escapes that byte, and a
/// trailing backslash is ignored.
pub fn get_instructions(replacement: &[u8]) -> Vec<Instruction> {
    let mut instructions = Vec::new();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < replacement.len() {
        if replacement[index] == b'\\' {
            if index + 1 >= replacement.len() {
                break;
            }
            let escaped = replacement[index + 1];
            if escaped.is_ascii_digit() {
                if !literals.is_empty() {
                    instructions.push(Instruction::literal(&literals));
                    literals.clear();
                }
                instructions.push(Instruction::substitution((escaped - b'0') as usize));
            } else {
                literals.push(escaped);
            }
            index += 2;
        } else {
            literals.push(replacement[index]);
            index += 1;
        }
    }
    if !literals.is_empty() {
        instructions.push(Instruction::literal(&literals));
    }
    instructions
}

/// 按字符 position（1-based）截取 UTF-8 后缀；越界由 check_out_range_pos 判定。
fn trim_utf8_at_position(
    expression: &str,
    position: i64,
    allow_empty_position_one: bool,
) -> Result<(usize, &str), RegexpError> {
    let char_count = expression.chars().count();
    if (position < 1 || position > char_count as i64)
        && (!allow_empty_position_one || check_out_range_pos(expression.len(), position))
    {
        return Err(RegexpError::InvalidIndex);
    }
    let chars_to_skip = usize::try_from(position - 1).map_err(|_| RegexpError::InvalidIndex)?;
    let byte_index = expression
        .char_indices()
        .nth(chars_to_skip)
        .map(|(index, _)| index)
        .unwrap_or(expression.len());
    Ok((byte_index, &expression[byte_index..]))
}

/// INSTR 用 UTF-8 截取：返回 (前缀字节长度, 后缀)。
fn trim_utf8_for_instr(expression: &str, position: i64) -> Result<(usize, &str), RegexpError> {
    let char_count = expression.chars().count() as i64;
    if position < 1 || (position > char_count && !expression.is_empty()) {
        return Err(RegexpError::InvalidIndex);
    }
    if expression.is_empty() {
        return Ok((0, expression));
    }
    trim_utf8_at_position(expression, position, false)
}

/// 按字节 position 截取 binary 后缀。
fn trim_bytes_at_position(
    expression: &[u8],
    position: i64,
    allow_empty_position_one: bool,
) -> Result<&[u8], RegexpError> {
    if (position < 1 || position > expression.len() as i64)
        && (!allow_empty_position_one || check_out_range_pos(expression.len(), position))
    {
        return Err(RegexpError::InvalidIndex);
    }
    let index = usize::try_from(position - 1).map_err(|_| RegexpError::InvalidIndex)?;
    Ok(&expression[index.min(expression.len())..])
}

/// INSTR 用字节截取。
fn trim_bytes_for_instr(expression: &[u8], position: i64) -> Result<&[u8], RegexpError> {
    if position < 1 || (position > expression.len() as i64 && !expression.is_empty()) {
        return Err(RegexpError::InvalidIndex);
    }
    if expression.is_empty() {
        return Ok(expression);
    }
    trim_bytes_at_position(expression, position, false)
}

/// 按指令把捕获组/字面量渲染为文本替换结果。
fn render_text_replacement(
    captures: &regex::Captures<'_>,
    instructions: &[Instruction],
) -> Result<String, RegexpError> {
    let mut result = String::new();
    for instruction in instructions {
        if let Some(number) = instruction.substitution_num {
            if number >= captures.len() {
                return Err(RegexpError::InvalidSubstitution);
            }
            if let Some(captured) = captures.get(number) {
                result.push_str(captured.as_str());
            }
        } else {
            result.push_str(
                std::str::from_utf8(&instruction.literal).expect("replacement came from UTF-8"),
            );
        }
    }
    Ok(result)
}

/// 对 UTF-8 文本执行第 occurrence 次（或全部）匹配替换。
fn replace_text_matches(
    regexp: &Regex,
    expression: &str,
    instructions: &[Instruction],
    occurrence: i64,
) -> Result<String, RegexpError> {
    let mut output = String::with_capacity(expression.len());
    let mut first_not_copied = 0;
    let mut matched_count = 0_i64;
    let mut did_replace = false;
    for captures in regexp.captures_iter(expression) {
        matched_count += 1;
        if occurrence != 0 && matched_count != occurrence {
            continue;
        }
        let whole = captures.get(0).expect("capture zero always exists");
        output.push_str(&expression[first_not_copied..whole.start()]);
        output.push_str(&render_text_replacement(&captures, instructions)?);
        first_not_copied = whole.end();
        did_replace = true;
        if occurrence != 0 {
            break;
        }
    }
    if !did_replace {
        return Ok(expression.to_owned());
    }
    output.push_str(&expression[first_not_copied..]);
    Ok(output)
}

/// 按指令渲染 binary 替换结果。
fn render_byte_replacement(
    captures: &regex::bytes::Captures<'_>,
    instructions: &[Instruction],
) -> Result<Vec<u8>, RegexpError> {
    let mut result = Vec::new();
    for instruction in instructions {
        if let Some(number) = instruction.substitution_num {
            if number >= captures.len() {
                return Err(RegexpError::InvalidSubstitution);
            }
            if let Some(captured) = captures.get(number) {
                result.extend_from_slice(captured.as_bytes());
            }
        } else {
            result.extend_from_slice(&instruction.literal);
        }
    }
    Ok(result)
}

/// 对字节序列执行匹配替换。
fn replace_byte_matches(
    regexp: &BytesRegex,
    expression: &[u8],
    instructions: &[Instruction],
    occurrence: i64,
) -> Result<Vec<u8>, RegexpError> {
    let mut output = Vec::with_capacity(expression.len());
    let mut first_not_copied = 0;
    let mut matched_count = 0_i64;
    let mut did_replace = false;
    for captures in regexp.captures_iter(expression) {
        matched_count += 1;
        if occurrence != 0 && matched_count != occurrence {
            continue;
        }
        let whole = captures.get(0).expect("capture zero always exists");
        output.extend_from_slice(&expression[first_not_copied..whole.start()]);
        output.extend_from_slice(&render_byte_replacement(&captures, instructions)?);
        first_not_copied = whole.end();
        did_replace = true;
        if occurrence != 0 {
            break;
        }
    }
    if !did_replace {
        return Ok(expression.to_vec());
    }
    output.extend_from_slice(&expression[first_not_copied..]);
    Ok(output)
}

/// 将字节转为大写十六进制字符串（无分隔符）。
fn upper_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub const fn regexp_like_vectorized() -> bool {
    true
}

pub const fn regexp_substr_vectorized() -> bool {
    true
}

pub const fn regexp_instr_vectorized() -> bool {
    true
}

pub const fn regexp_replace_vectorized() -> bool {
    true
}
