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

// JSON Path 表达式解析、缓存与路径 leg 操作。
//
// 对齐 Go `json_path_expr.go`：支持 `$`、键、数组下标/`last`/`to` 范围、
// `*` / `**` 通配；解析结果经 LRU 缓存，返回时拷贝以防调用方改动污染缓存。

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// `last` 关键字字符序列。
const LAST_STR: &[char] = &['l', 'a', 's', 't'];
/// `to` 关键字字符序列（数组范围）。
const TO_STR: &[char] = &['t', 'o'];
/// 路径表达式 LRU 缓存容量，与 Go SimpleLRUCache 一致。
const PATH_CACHE_CAPACITY: usize = 1_000;

// jsonPathArrayIndex keeps the Go representation: non-negative indexes count
// from the start, while -1 is `last` and -n-1 is `last-n`.
// 非负为从头计数；-1 表示 last；-n-1 表示 last-n。
type jsonPathArrayIndex = isize;

/// 数组下标：相对起点解析与字符串化。
trait JsonPathArrayIndexExt {
    fn getIndexFromStart(&self, elem_count: isize) -> isize;
    fn String(&self) -> String;
}

impl JsonPathArrayIndexExt for jsonPathArrayIndex {
    // 负下标相对数组长度换算为从头计数
    fn getIndexFromStart(&self, elem_count: isize) -> isize {
        if *self < 0 { elem_count + *self } else { *self }
    }

    fn String(&self) -> String {
        if *self < 0 {
            format!("last-{}", (*self + 1).unsigned_abs())
        } else {
            self.to_string()
        }
    }
}

// validateIndexRange returns whether a could be less than or equal to b. As in
// Go, indexes with different signs cannot be ordered until the array length is
// known, so they are accepted here.
/// 校验范围起止：异号在未知长度时可接受，同号则要求 a <= b。
fn validateIndexRange(a: jsonPathArrayIndex, b: jsonPathArrayIndex) -> bool {
    if (a >= 0 && b >= 0) || (a < 0 && b < 0) {
        return a <= b;
    }
    true
}

/// 构造从头计数的下标。
fn jsonPathArrayIndexFromStart(index: isize) -> jsonPathArrayIndex {
    index
}

/// 构造 `last-index` 形式下标（编码为 -1-index）。
fn jsonPathArrayIndexFromLast(index: isize) -> jsonPathArrayIndex {
    -1 - index
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 数组通配 `[*]` 选择。
struct jsonPathArraySelectionAsterisk;

impl jsonPathArraySelectionAsterisk {
    fn getIndexRange(&self, elem_count: isize) -> (isize, isize) {
        (0, elem_count - 1)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 单一下标选择。
struct jsonPathArraySelectionIndex {
    index: jsonPathArrayIndex,
}

impl jsonPathArraySelectionIndex {
    fn getIndexRange(&self, elem_count: isize) -> (isize, isize) {
        let start = self.index.getIndexFromStart(elem_count);
        let mut end = start;
        if end >= elem_count {
            end = elem_count - 1;
        }
        (start, end)
    }
}

// jsonPathArraySelectionRange represents a closed interval.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 闭区间范围选择 `[start to end]`。
struct jsonPathArraySelectionRange {
    start: jsonPathArrayIndex,
    end: jsonPathArrayIndex,
}

impl jsonPathArraySelectionRange {
    fn getIndexRange(&self, elem_count: isize) -> (isize, isize) {
        let start = self.start.getIndexFromStart(elem_count);
        let mut end = self.end.getIndexFromStart(elem_count);
        if end >= elem_count {
            end = elem_count - 1;
        }
        (start, end)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 数组选择：通配 / 单下标 / 范围。
enum jsonPathArraySelection {
    Asterisk(jsonPathArraySelectionAsterisk),
    Index(jsonPathArraySelectionIndex),
    Range(jsonPathArraySelectionRange),
}

impl jsonPathArraySelection {
    fn getIndexRange(&self, elem_count: isize) -> (isize, isize) {
        match self {
            Self::Asterisk(selection) => selection.getIndexRange(elem_count),
            Self::Index(selection) => selection.getIndexRange(elem_count),
            Self::Range(selection) => selection.getIndexRange(elem_count),
        }
    }
}

impl From<jsonPathArraySelectionAsterisk> for jsonPathArraySelection {
    fn from(value: jsonPathArraySelectionAsterisk) -> Self {
        Self::Asterisk(value)
    }
}

impl From<jsonPathArraySelectionIndex> for jsonPathArraySelection {
    fn from(value: jsonPathArraySelectionIndex) -> Self {
        Self::Index(value)
    }
}

impl From<jsonPathArraySelectionRange> for jsonPathArraySelection {
    fn from(value: jsonPathArraySelectionRange) -> Self {
        Self::Range(value)
    }
}

/// 路径 leg 类型码。
type jsonPathLegType = u8;

/// 对象键 leg。
const jsonPathLegKey: jsonPathLegType = 0x01;
/// 数组选择 leg。
const jsonPathLegArraySelection: jsonPathLegType = 0x02;
/// 递归下降 `**` leg。
const jsonPathLegDoubleAsterisk: jsonPathLegType = 0x03;

// jsonPathLeg is only used by JSONPathExpression.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 路径中的一步：键、数组选择或 `**`。
struct jsonPathLeg {
    typ: jsonPathLegType,
    arraySelection: Option<jsonPathArraySelection>,
    dotKey: String,
}

/// 路径表达式标志位集合。
type jsonPathExpressionFlag = u8;

/// 含 `*` 通配。
const jsonPathExpressionContainsAsterisk: jsonPathExpressionFlag = 0x01;
/// 含 `**` 递归下降。
const jsonPathExpressionContainsDoubleAsterisk: jsonPathExpressionFlag = 0x02;
/// 含数组范围选择。
const jsonPathExpressionContainsRange: jsonPathExpressionFlag = 0x04;

/// 标志位查询扩展。
trait JsonPathExpressionFlagExt {
    fn containsAnyAsterisk(&self) -> bool;
    fn containsAnyRange(&self) -> bool;
}

impl JsonPathExpressionFlagExt for jsonPathExpressionFlag {
    fn containsAnyAsterisk(&self) -> bool {
        self & (jsonPathExpressionContainsAsterisk | jsonPathExpressionContainsDoubleAsterisk) != 0
    }

    fn containsAnyRange(&self) -> bool {
        self & jsonPathExpressionContainsRange != 0
    }
}

// JSONPathExpression is an immutable parsed JSON path. Cloning it copies its
// legs, matching the Go cache's defensive-copy behavior.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
// JSONPathExpression 是不可变的已解析路径；clone 会拷贝 legs，对齐 Go 缓存防御性拷贝。
/// 已解析的 JSON Path：legs 序列与通配/范围标志。
pub struct JSONPathExpression {
    legs: Vec<jsonPathLeg>,
    flags: jsonPathExpressionFlag,
}

impl JSONPathExpression {
    /// 深拷贝 legs 与 flags。
    fn clone(&self) -> JSONPathExpression {
        JSONPathExpression {
            legs: self.legs.clone(),
            flags: self.flags,
        }
    }

    // popOneLeg returns the first leg and a child expression without that leg.
    // 弹出首 leg，子表达式重算 flags。
    fn popOneLeg(&self) -> (jsonPathLeg, JSONPathExpression) {
        let mut child = JSONPathExpression {
            legs: self.legs[1..].to_vec(),
            flags: 0,
        };
        child.recompute_flags();
        (self.legs[0].clone(), child)
    }

    // popOneLastLeg is used by modification paths already validated to contain
    // no wildcard, so Go intentionally leaves the parent flags at zero.
    // 弹出末 leg；修改路径已保证无通配，父 flags 刻意置 0。
    fn popOneLastLeg(&self) -> (JSONPathExpression, jsonPathLeg) {
        let last = self.legs.len() - 1;
        (
            JSONPathExpression {
                legs: self.legs[..last].to_vec(),
                flags: 0,
            },
            self.legs[last].clone(),
        )
    }

    /// 追加数组选择 leg，并按需置通配/范围标志。
    fn pushBackOneArraySelectionLeg<S>(&self, arraySelection: S) -> JSONPathExpression
    where
        S: Into<jsonPathArraySelection>,
    {
        let arraySelection = arraySelection.into();
        let mut result = self.clone();
        match arraySelection {
            jsonPathArraySelection::Asterisk(_) => {
                result.flags |= jsonPathExpressionContainsAsterisk;
            }
            jsonPathArraySelection::Range(_) => {
                result.flags |= jsonPathExpressionContainsRange;
            }
            jsonPathArraySelection::Index(_) => {}
        }
        result.legs.push(jsonPathLeg {
            typ: jsonPathLegArraySelection,
            arraySelection: Some(arraySelection),
            dotKey: String::new(),
        });
        result
    }

    /// 追加对象键 leg；键为 `*` 时置通配标志。
    fn pushBackOneKeyLeg(&self, key: String) -> JSONPathExpression {
        let mut result = self.clone();
        if key == "*" {
            result.flags |= jsonPathExpressionContainsAsterisk;
        }
        result.legs.push(jsonPathLeg {
            typ: jsonPathLegKey,
            arraySelection: None,
            dotKey: key,
        });
        result
    }

    // CouldMatchMultipleValues reports wildcard, double-wildcard, or range
    // selections exactly as the Go flag combination does.
    // 通配、双通配或范围均可匹配多值。
    /// 路径是否可能匹配多个值。
    pub fn CouldMatchMultipleValues(&self) -> bool {
        self.flags.containsAnyAsterisk() || self.flags.containsAnyRange()
    }

    /// 将路径格式化为以 `$` 开头的规范字符串。
    pub fn String(&self) -> String {
        let mut output = String::from("$");
        for leg in &self.legs {
            match leg.typ {
                jsonPathLegArraySelection => match leg.arraySelection.as_ref() {
                    Some(jsonPathArraySelection::Asterisk(_)) => output.push_str("[*]"),
                    Some(jsonPathArraySelection::Index(selection)) => {
                        output.push('[');
                        output.push_str(&selection.index.String());
                        output.push(']');
                    }
                    Some(jsonPathArraySelection::Range(selection)) => {
                        output.push('[');
                        output.push_str(&selection.start.String());
                        output.push_str(" to ");
                        output.push_str(&selection.end.String());
                        output.push(']');
                    }
                    None => unreachable!("array leg must carry a selection"),
                },
                jsonPathLegKey => {
                    output.push('.');
                    if leg.dotKey == "*" {
                        output.push('*');
                    } else {
                        output.push_str(&quoteJSONString(&leg.dotKey));
                    }
                }
                jsonPathLegDoubleAsterisk => output.push_str("**"),
                _ => unreachable!("unknown JSON path leg type"),
            }
        }
        output
    }

    /// 根据 legs 重新计算通配/范围标志。
    fn recompute_flags(&mut self) {
        self.flags = 0;
        for leg in &self.legs {
            match (leg.typ, leg.arraySelection.as_ref(), leg.dotKey.as_str()) {
                (jsonPathLegArraySelection, Some(jsonPathArraySelection::Asterisk(_)), _) => {
                    self.flags |= jsonPathExpressionContainsAsterisk;
                }
                (jsonPathLegArraySelection, Some(jsonPathArraySelection::Range(_)), _) => {
                    self.flags |= jsonPathExpressionContainsRange;
                }
                (jsonPathLegKey, _, "*") => {
                    self.flags |= jsonPathExpressionContainsAsterisk;
                }
                (jsonPathLegDoubleAsterisk, _, _) => {
                    self.flags |= jsonPathExpressionContainsDoubleAsterisk;
                }
                _ => {}
            }
        }
    }
}

impl fmt::Display for JSONPathExpression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// JSON Path 解析错误，携带出错字符位置。
pub struct JSONPathError {
    position: usize,
}

impl JSONPathError {
    /// 在指定字符位置构造错误。
    fn at(position: usize) -> Self {
        Self { position }
    }

    /// 返回出错位置（与 Go 文案一致）。
    pub fn position(&self) -> usize {
        self.position
    }
}

impl fmt::Display for JSONPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid JSON path expression. The error is around character position {}.",
            self.position
        )
    }
}

impl std::error::Error for JSONPathError {}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 缓存键包装。
struct jsonPathExpressionKey(String);

impl jsonPathExpressionKey {
    pub fn Hash(&self) -> Vec<u8> {
        self.0.as_bytes().to_vec()
    }
}

#[derive(Debug, Default)]
/// LRU 缓存内部状态：条目表与访问顺序。
struct JSONPathExpressionCacheState {
    entries: HashMap<String, JSONPathExpression>,
    order: VecDeque<String>,
}

// JSONPathExpressionCache is a mutex-protected LRU cache with the same 1,000
// entry capacity as kvcache.SimpleLRUCache in the Go implementation.
#[derive(Debug)]
// 互斥保护的 LRU，容量 1000，对齐 Go kvcache.SimpleLRUCache。
/// JSON Path 表达式 LRU 缓存。
pub struct JSONPathExpressionCache {
    state: Mutex<JSONPathExpressionCacheState>,
}

impl Default for JSONPathExpressionCache {
    fn default() -> Self {
        Self {
            state: Mutex::new(JSONPathExpressionCacheState::default()),
        }
    }
}

impl JSONPathExpressionCache {
    /// 获取缓存锁；中毒时仍取出内层状态。
    fn lock(&self) -> MutexGuard<'_, JSONPathExpressionCacheState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 查询并刷新 LRU 顺序。
    fn get(&self, key: &str) -> Option<JSONPathExpression> {
        let mut state = self.lock();
        let value = state.entries.get(key)?.clone();
        if let Some(index) = state.order.iter().position(|entry| entry == key) {
            state.order.remove(index);
        }
        state.order.push_back(key.to_owned());
        Some(value)
    }

    /// 写入缓存，超出容量时淘汰最旧条目。
    fn put(&self, key: String, value: JSONPathExpression) {
        let mut state = self.lock();
        if let Some(index) = state.order.iter().position(|entry| entry == &key) {
            state.order.remove(index);
        }
        state.entries.insert(key.clone(), value);
        state.order.push_back(key);

        while state.entries.len() > PATH_CACHE_CAPACITY {
            if let Some(oldest) = state.order.pop_front() {
                state.entries.remove(&oldest);
            }
        }
    }
}

static PE_CACHE: OnceLock<JSONPathExpressionCache> = OnceLock::new();

/// 进程级路径缓存单例。
fn path_cache() -> &'static JSONPathExpressionCache {
    PE_CACHE.get_or_init(JSONPathExpressionCache::default)
}

#[derive(Clone, Debug)]
/// 路径解析字符流：持有 char 序列与当前游标。
struct jsonPathStream {
    pathExpr: Vec<char>,
    pos: usize,
}

impl jsonPathStream {
    /// 按 Unicode 标量拆分路径文本。
    fn new(path_expr: &str) -> Self {
        Self {
            pathExpr: path_expr.chars().collect(),
            pos: 0,
        }
    }

    /// 跳过空白。
    fn skipWhiteSpace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }

    /// 读取并前进一个字符。
    fn read(&mut self) -> char {
        let value = self.pathExpr[self.pos];
        self.pos += 1;
        value
    }

    /// 窥视当前字符。
    fn peek(&self) -> Option<char> {
        self.pathExpr.get(self.pos).copied()
    }

    /// 前进 count 个字符。
    fn skip(&mut self, count: usize) {
        self.pos += count;
    }

    /// 是否已到末尾。
    fn exhausted(&self) -> bool {
        self.pos >= self.pathExpr.len()
    }

    /// 按谓词连续读取；第二返回值表示是否读到流末尾。
    fn readWhile<F>(&mut self, mut predicate: F) -> (Vec<char>, bool)
    where
        F: FnMut(char) -> bool,
    {
        let start = self.pos;
        while let Some(value) = self.peek() {
            if !predicate(value) {
                return (self.pathExpr[start..self.pos].to_vec(), false);
            }
            self.pos += 1;
        }
        (self.pathExpr[start..].to_vec(), true)
    }

    // Go 的 meetEnd 要求期望文本后仍有分隔符。
    /// 尝试匹配固定关键字。
    fn tryReadString(&mut self, expected: &[char]) -> bool {
        let record_pos = self.pos;
        // Go's readWhile reports meetEnd when the expected text consumes the
        // stream exactly, so a following delimiter is required.
        if self.pos + expected.len() >= self.pathExpr.len()
            || self.pathExpr[self.pos..self.pos + expected.len()] != *expected
        {
            return false;
        }
        self.pos += expected.len();
        if self.pos == record_pos {
            return false;
        }
        true
    }

    /// 尝试读取十进制下标数字（上限 u32::MAX）。
    fn tryReadIndexNumber(&mut self) -> (isize, bool) {
        let record_pos = self.pos;
        let (digits, met_end) = self.readWhile(|value| value.is_ascii_digit());
        if digits.is_empty() || met_end {
            self.pos = record_pos;
            return (0, false);
        }

        let value = digits.iter().collect::<String>().parse::<u64>();
        match value {
            Ok(value) if value <= u32::MAX as u64 => (value as isize, true),
            _ => {
                self.pos = record_pos;
                (0, false)
            }
        }
    }

    // tryParseArrayIndex reads number, last, or last - number. On failure the
    // cursor is restored to its original position.
    // 读取 number / last / last - number；失败时恢复游标。
    /// 解析数组下标表达式。
    fn tryParseArrayIndex(&mut self) -> (jsonPathArrayIndex, bool) {
        let record_pos = self.pos;
        self.skipWhiteSpace();
        let Some(current) = self.peek() else {
            return (0, false);
        };

        if current.is_ascii_digit() {
            let (index, ok) = self.tryReadIndexNumber();
            if !ok {
                self.pos = record_pos;
                return (0, false);
            }
            return (jsonPathArrayIndexFromStart(index), true);
        }

        if current == 'l' {
            if !self.tryReadString(LAST_STR) {
                self.pos = record_pos;
                return (0, false);
            }
            self.skipWhiteSpace();
            if self.exhausted() || self.peek() != Some('-') {
                return (jsonPathArrayIndexFromLast(0), true);
            }
            self.skip(1);
            self.skipWhiteSpace();
            let (index, ok) = self.tryReadIndexNumber();
            if !ok {
                self.pos = record_pos;
                return (0, false);
            }
            return (jsonPathArrayIndexFromLast(index), true);
        }

        (0, false)
    }
}

/// 无缓存地解析路径：必须以 `$` 开头，禁止以 `**` 结尾。
fn parseJSONPathExpr(pathExpr: &str) -> Result<JSONPathExpression, JSONPathError> {
    let mut stream = jsonPathStream::new(pathExpr);
    stream.skipWhiteSpace();
    if stream.exhausted() || stream.read() != '$' {
        return Err(JSONPathError::at(1));
    }
    stream.skipWhiteSpace();

    let mut expression = JSONPathExpression {
        legs: Vec::with_capacity(16),
        flags: 0,
    };

    while !stream.exhausted() {
        let ok = match stream.peek() {
            Some('.') => parseJSONPathMember(&mut stream, &mut expression),
            Some('[') => parseJSONPathArray(&mut stream, &mut expression),
            Some('*') => parseJSONPathWildcard(&mut stream, &mut expression),
            _ => false,
        };
        if !ok {
            return Err(JSONPathError::at(stream.pos));
        }
        stream.skipWhiteSpace();
    }

    if expression
        .legs
        .last()
        .is_some_and(|leg| leg.typ == jsonPathLegDoubleAsterisk)
    {
        return Err(JSONPathError::at(stream.pos));
    }
    Ok(expression)
}

/// 解析 `**` 递归下降通配。
fn parseJSONPathWildcard(stream: &mut jsonPathStream, expression: &mut JSONPathExpression) -> bool {
    stream.skip(1);
    if stream.exhausted() || stream.read() != '*' {
        return false;
    }
    if stream.exhausted() || stream.peek() == Some('*') {
        return false;
    }

    expression.flags |= jsonPathExpressionContainsDoubleAsterisk;
    expression.legs.push(jsonPathLeg {
        typ: jsonPathLegDoubleAsterisk,
        arraySelection: None,
        dotKey: String::new(),
    });
    true
}

/// 解析 `[...]` 数组选择：`*`、下标或 `start to end`。
fn parseJSONPathArray(stream: &mut jsonPathStream, expression: &mut JSONPathExpression) -> bool {
    stream.skip(1);
    stream.skipWhiteSpace();
    if stream.exhausted() {
        return false;
    }

    let selection = if stream.peek() == Some('*') {
        stream.skip(1);
        expression.flags |= jsonPathExpressionContainsAsterisk;
        jsonPathArraySelectionAsterisk.into()
    } else {
        let (start, ok) = stream.tryParseArrayIndex();
        if !ok {
            return false;
        }

        let mut selection = jsonPathArraySelectionIndex { index: start }.into();
        if stream.peek().is_some_and(char::is_whitespace) {
            stream.skipWhiteSpace();
            if stream.tryReadString(TO_STR) && stream.peek().is_some_and(char::is_whitespace) {
                stream.skipWhiteSpace();
                if stream.exhausted() {
                    return false;
                }
                let (end, ok) = stream.tryParseArrayIndex();
                if !ok || !validateIndexRange(start, end) {
                    return false;
                }
                expression.flags |= jsonPathExpressionContainsRange;
                selection = jsonPathArraySelectionRange { start, end }.into();
            }
        }
        selection
    };

    expression.legs.push(jsonPathLeg {
        typ: jsonPathLegArraySelection,
        arraySelection: Some(selection),
        dotKey: String::new(),
    });

    stream.skipWhiteSpace();
    !stream.exhausted() && stream.read() == ']'
}

/// 解析 `.key` / `.*` / `."quoted"` 成员访问。
fn parseJSONPathMember(stream: &mut jsonPathStream, expression: &mut JSONPathExpression) -> bool {
    stream.skip(1);
    stream.skipWhiteSpace();
    if stream.exhausted() {
        return false;
    }

    let dot_key = if stream.peek() == Some('*') {
        stream.skip(1);
        expression.flags |= jsonPathExpressionContainsAsterisk;
        "*".to_owned()
    } else if stream.peek() == Some('"') {
        stream.skip(1);
        let start = stream.pos;
        while let Some(current) = stream.peek() {
            if current == '\\' {
                stream.skip(1);
                if stream.exhausted() {
                    // Go's readWhile advances once more after its escape
                    // callback, including when the backslash ends the input.
                    stream.skip(1);
                    return false;
                }
                stream.skip(1);
                continue;
            }
            if current == '"' {
                break;
            }
            stream.skip(1);
        }
        if stream.exhausted() {
            return false;
        }

        let raw = stream.pathExpr[start..stream.pos]
            .iter()
            .collect::<String>();
        stream.skip(1);
        let encoded = format!("\"{raw}\"");
        match decodePathMemberKey(&encoded) {
            Ok(value) => value,
            Err(_) => return false,
        }
    } else {
        let (key, _) =
            stream.readWhile(|value| !(value.is_whitespace() || matches!(value, '.' | '[' | '*')));
        let raw = key.iter().collect::<String>();
        // Go validates and unquotes JSON escapes before checking the identifier,
        // even when the member was not surrounded by quotation marks.
        let Ok(key) = decodePathMemberKey(&format!("\"{raw}\"")) else {
            return false;
        };
        if !isEcmascriptIdentifier(&key) {
            return false;
        }
        key
    };

    expression.legs.push(jsonPathLeg {
        typ: jsonPathLegKey,
        arraySelection: None,
        dotKey: dot_key,
    });
    true
}

// Go validates JSON syntax first, then its unquoteJSONString consumes two
// Unicode escapes whenever the first is a surrogate. utf16.DecodeRune emits
// U+FFFD for an invalid pair (but a lone surrogate remains an error).
fn decodePathMemberKey(encoded: &str) -> Result<String, serde_json::Error> {
    let mut normalized = Vec::with_capacity(encoded.len());
    let bytes = encoded.as_bytes();
    let mut pos = 0;
    let hex = |digits: &[u8]| {
        std::str::from_utf8(digits)
            .ok()
            .and_then(|text| u16::from_str_radix(text, 16).ok())
    };
    while pos < bytes.len() {
        if bytes[pos] == b'\\' {
            if bytes.get(pos + 1) == Some(&b'u')
                && let Some(first) = bytes.get(pos + 2..pos + 6).and_then(hex)
                && (0xd800..=0xdfff).contains(&first)
                && bytes.get(pos + 6..pos + 8) == Some(b"\\u")
                && let Some(second) = bytes.get(pos + 8..pos + 12).and_then(hex)
            {
                if !(0xd800..=0xdbff).contains(&first) || !(0xdc00..=0xdfff).contains(&second) {
                    normalized.extend_from_slice(b"\\ufffd");
                } else {
                    normalized.extend_from_slice(&bytes[pos..pos + 12]);
                }
                pos += 12;
                continue;
            }
            // Skip an escaped backslash as a pair so its following `u` is
            // never mistaken for a Unicode escape.
            if pos + 1 < bytes.len() {
                normalized.extend_from_slice(&bytes[pos..pos + 2]);
                pos += 2;
                continue;
            }
        }
        normalized.push(bytes[pos]);
        pos += 1;
    }
    serde_json::from_slice(&normalized)
}

// Go intentionally classifies each UTF-8 byte after converting it to a rune.
// Keeping that byte-wise behavior preserves cases such as µ being accepted and
// unquoted Cyrillic identifiers being rejected.
// 按字节转 rune 分类，保留 µ 可接受、西里尔等需引号的行为。
/// 判断是否可作为未加引号的 ECMAScript 标识符。
fn isEcmascriptIdentifier(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }

    for (index, byte) in value.bytes().enumerate() {
        let character = char::from(byte);
        if character.is_alphabetic() || character == '$' || character == '_' {
            continue;
        }
        if index == 0 {
            return false;
        }
        if character.is_ascii_digit() {
            continue;
        }
        return false;
    }
    true
}

// quoteJSONString mirrors the package helper used by Go JSON path rendering:
// identifiers stay unquoted, while special bytes and non-identifiers are JSON
// quoted. Rust strings are always valid UTF-8, so Go's RuneError repair branch
// is not needed.
// 标识符保持未加引号；特殊字符与非标识符则 JSON 引号包裹。
/// 路径渲染时对键名按需加引号。
fn quoteJSONString(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    let mut has_escaped = false;
    for character in value.chars() {
        let escaped = match character {
            '\\' => Some("\\\\"),
            '"' => Some("\\\""),
            '\u{0008}' => Some("\\b"),
            '\u{000C}' => Some("\\f"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            _ => None,
        };
        if let Some(escaped) = escaped {
            has_escaped = true;
            output.push_str(escaped);
        } else {
            output.push(character);
        }
    }

    if has_escaped || !isEcmascriptIdentifier(value) {
        output.push('"');
        output
    } else {
        output.remove(0);
        output
    }
}

// ParseJSONPathExpr parses a JSON path expression for JSON_EXTRACT, JSON_SET,
// and related operations. Successful values are cached and always cloned on
// return so caller-side leg operations cannot mutate the cached expression.
// 成功结果写入缓存，返回前 clone，避免调用方改动污染缓存。
/// 解析 JSON Path（供 JSON_EXTRACT / JSON_SET 等使用）。
pub fn ParseJSONPathExpr(pathExpr: impl AsRef<str>) -> Result<JSONPathExpression, JSONPathError> {
    let pathExpr = pathExpr.as_ref();
    if let Some(expression) = path_cache().get(pathExpr) {
        return Ok(expression);
    }

    let expression = parseJSONPathExpr(pathExpr)?;
    path_cache().put(pathExpr.to_owned(), expression.clone());
    Ok(expression)
}
