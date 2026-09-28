// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// Evaluation-independent implementation of the string operations in
// `builtin_string.go`.  The package integration task can wrap these functions
// in TiDB's expression traits without duplicating the boundary-sensitive
// byte, character, packet-size, and NULL behavior implemented here.

// 字符串内建函数的求值无关实现，对应 `builtin_string.go`。
//
// 包级集成可在此之上包装 TiDB 表达式 trait，而无需重复实现
// 字节/字符边界、`max_allowed_packet` 与 SQL NULL 等敏感语义。
// binary 签名按字节操作；UTF-8 签名按 Unicode 字符（rune）操作。

use std::cmp::Ordering;
use std::collections::HashMap;

use base64::Engine as _;

/// FORMAT 小数位上限，与 MySQL/TiDB 一致。
pub const FORMAT_MAX_DECIMALS: i64 = 30;
/// 翻译映射中标记「删除该字节」的哨兵值（超出 u8 范围）。
pub const INVALID_BYTE: u16 = 256;
const MAX_BLOB_WIDTH: i64 = 16_777_216;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 字符串类内建函数的返回值族：整数或字符串。
pub enum StringReturnKind {
    Integer,
    String,
}

/// Construction metadata retained from the Go function classes.  It keeps
/// argument validation and the byte/UTF-8 signature decision available before
/// the package-level expression traits are connected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 构造期元数据：参数个数、返回类型、是否受 max_allowed_packet 约束，
/// 以及是否同时存在 binary/UTF-8 两套签名。
pub struct StringFunctionSpec {
    pub name: &'static str,
    pub min_args: usize,
    pub max_args: usize,
    pub return_kind: StringReturnKind,
    pub packet_sensitive: bool,
    pub has_binary_and_utf8_signatures: bool,
}

macro_rules! string_specs {
    ($(($name:literal, $min:expr, $max:expr, $ret:ident, $packet:literal, $split:literal)),+ $(,)?) => {
        pub const STRING_FUNCTION_SPECS: &[StringFunctionSpec] = &[
            $(StringFunctionSpec {
                name: $name,
                min_args: $min,
                max_args: $max,
                return_kind: StringReturnKind::$ret,
                packet_sensitive: $packet,
                has_binary_and_utf8_signatures: $split,
            }),+
        ];
    };
}

string_specs!(
    ("length", 1, 1, Integer, false, false),
    ("ascii", 1, 1, Integer, false, false),
    ("concat", 1, usize::MAX, String, true, false),
    ("concat_ws", 2, usize::MAX, String, true, false),
    ("left", 2, 2, String, false, true),
    ("right", 2, 2, String, false, true),
    ("repeat", 2, 2, String, true, false),
    ("lower", 1, 1, String, false, true),
    ("reverse", 1, 1, String, false, true),
    ("space", 1, 1, String, true, false),
    ("upper", 1, 1, String, false, true),
    ("strcmp", 2, 2, Integer, false, false),
    ("replace", 3, 3, String, true, false),
    ("convert", 2, 2, String, false, false),
    ("substring", 2, 3, String, false, true),
    ("substring_index", 3, 3, String, false, false),
    ("locate", 2, 3, Integer, false, true),
    ("hex", 1, 1, String, false, false),
    ("unhex", 1, 1, String, false, false),
    ("trim", 1, 3, String, false, false),
    ("ltrim", 1, 1, String, false, false),
    ("rtrim", 1, 1, String, false, false),
    ("lpad", 3, 3, String, true, true),
    ("rpad", 3, 3, String, true, true),
    ("bit_length", 1, 1, Integer, false, false),
    ("char", 2, usize::MAX, String, false, false),
    ("char_length", 1, 1, Integer, false, true),
    ("find_in_set", 2, 2, Integer, false, false),
    ("field", 2, usize::MAX, Integer, false, false),
    ("make_set", 2, usize::MAX, String, true, false),
    ("oct", 1, 1, String, false, false),
    ("ord", 1, 1, Integer, false, false),
    ("quote", 1, 1, String, false, false),
    ("bin", 1, 1, String, false, false),
    ("elt", 2, usize::MAX, String, false, false),
    ("export_set", 3, 5, String, false, false),
    ("format", 2, 3, String, false, false),
    ("from_base64", 1, 1, String, true, false),
    ("to_base64", 1, 1, String, true, false),
    ("insert", 4, 4, String, true, true),
    ("instr", 2, 2, Integer, false, true),
    ("load_file", 1, 1, String, false, false),
    ("weight_string", 1, 3, String, true, false),
    ("translate", 3, 3, String, false, true),
);

/// 按名字（忽略大小写）查找字符串函数规格。
pub fn stringFunctionSpec(name: &str) -> Option<&'static StringFunctionSpec> {
    STRING_FUNCTION_SPECS
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(name))
}

/// 校验实参个数是否落在该函数规格的 [min, max] 区间。
pub fn verifyStringFunctionArgs(name: &str, count: usize) -> bool {
    stringFunctionSpec(name).is_some_and(|spec| (spec.min_args..=spec.max_args).contains(&count))
}

/// Reverse a byte string in place, returning the same owned buffer as Go does.
/// 原地反转字节串，返回与 Go 相同的拥有缓冲。
/// 按字节反转，用于 binary 签名路径。
pub fn reverseBytes(mut origin: Vec<u8>) -> Vec<u8> {
    origin.reverse();
    origin
}

/// Reverse Unicode scalar values, matching Go's `[]rune` helper.
/// 反转 Unicode 标量值，对应 Go 的 `[]rune` 辅助。
pub fn reverseRunes(mut origin: Vec<char>) -> Vec<char> {
    origin.reverse();
    origin
}

/// LENGTH/OCTET_LENGTH：返回字节长度。
pub fn length(value: &str) -> usize {
    value.len()
}

/// ASCII：返回首字节数值；空串为 0。
pub fn ascii(value: &str) -> i64 {
    value.as_bytes().first().copied().unwrap_or(0) as i64
}

/// CONCAT：任一参数为 SQL NULL（None）则整体为 NULL。
pub fn concat(values: &[Option<&str>]) -> Option<String> {
    let capacity = values.iter().map(|value| value.map_or(0, str::len)).sum();
    let mut result = String::with_capacity(capacity);
    for value in values {
        result.push_str(value.as_ref().copied()?);
    }
    Some(result)
}

/// CONCAT_WS：分隔符为 NULL 则结果 NULL；其余 NULL 参数跳过。
pub fn concatWS(separator: Option<&str>, values: &[Option<&str>]) -> Option<String> {
    let separator = separator?;
    Some(
        values
            .iter()
            .filter_map(|value| *value)
            .collect::<Vec<_>>()
            .join(separator),
    )
}

/// LEFT 二进制路径：按字节取左侧前缀。
pub fn leftBytes(value: &[u8], count: i64) -> Vec<u8> {
    if count <= 0 {
        return Vec::new();
    }
    value[..value.len().min(count as usize)].to_vec()
}

/// RIGHT 二进制路径：按字节取右侧后缀。
pub fn rightBytes(value: &[u8], count: i64) -> Vec<u8> {
    if count <= 0 {
        return Vec::new();
    }
    let count = value.len().min(count as usize);
    value[value.len() - count..].to_vec()
}

/// LEFT UTF-8 路径：按字符（rune）取左侧。
pub fn leftUtf8(value: &str, count: i64) -> String {
    if count <= 0 {
        return String::new();
    }
    value.chars().take(count as usize).collect()
}

/// RIGHT UTF-8 路径：按字符取右侧。
pub fn rightUtf8(value: &str, count: i64) -> String {
    if count <= 0 {
        return String::new();
    }
    let chars: Vec<_> = value.chars().collect();
    chars[chars.len().saturating_sub(count as usize)..]
        .iter()
        .collect()
}

/// `None` is the Go NULL result used when `max_allowed_packet` is exceeded.
/// REPEAT：超过 `max_allowed_packet` 时返回 NULL（对应 Go）。
pub fn repeat(value: &str, count: i64, max_allowed_packet: usize) -> Option<String> {
    if count < 1 {
        return Some(String::new());
    }
    let count = usize::try_from(count).ok()?;
    let length = value.len().checked_mul(count)?;
    (length <= max_allowed_packet).then(|| value.repeat(count))
}

/// LOWER UTF-8：Unicode 小写转换。
pub fn lowerUtf8(value: &str) -> String {
    value.to_lowercase()
}

/// UPPER UTF-8：Unicode 大写转换。
pub fn upperUtf8(value: &str) -> String {
    value.to_uppercase()
}

/// STRCMP：返回 -1/0/1；可按 collation 选择大小写不敏感比较。
pub fn strcmp(left: &str, right: &str, case_insensitive: bool) -> i64 {
    let ordering = if case_insensitive {
        left.to_lowercase().cmp(&right.to_lowercase())
    } else {
        left.cmp(right)
    };
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// REPLACE：替换所有非重叠出现的子串。
pub fn replace(value: &str, old: &str, new: &str) -> String {
    value.replace(old, new)
}

/// 将 MySQL 字符集名映射到 encoding_rs；未知则尝试 label 查找。
fn findEncoding(charset: &str) -> Option<&'static encoding_rs::Encoding> {
    match charset.to_ascii_lowercase().as_str() {
        "utf8" | "utf8mb4" => Some(encoding_rs::UTF_8),
        "ascii" => Some(encoding_rs::WINDOWS_1252),
        "gbk" | "gb18030" => Some(encoding_rs::GBK),
        "latin1" => Some(encoding_rs::WINDOWS_1252),
        _ => encoding_rs::Encoding::for_label(charset.as_bytes()),
    }
}

/// Execute `CONVERT(expr USING charset)` at the byte/charset boundary.  A
/// `None` result is the Go error/NULL path for an unknown charset or malformed
/// binary input.
/// 执行 `CONVERT(expr USING charset)` 的字节/字符集边界转换。
/// `None` 对应 Go 对未知字符集或非法二进制输入的错误/NULL 路径。
pub fn convertCharset(value: &[u8], source_charset: &str, target_charset: &str) -> Option<Vec<u8>> {
    if target_charset.eq_ignore_ascii_case("binary") {
        if source_charset.eq_ignore_ascii_case("binary") {
            return Some(value.to_vec());
        }
        let source = findEncoding(source_charset)?;
        let text = std::str::from_utf8(value).ok()?;
        let (encoded, _, had_errors) = source.encode(text);
        return (!had_errors).then(|| encoded.into_owned());
    }

    let target = findEncoding(target_charset)?;
    if source_charset.eq_ignore_ascii_case("binary") {
        let (decoded, _, had_errors) = target.decode(value);
        return (!had_errors).then(|| decoded.into_owned().into_bytes());
    }

    if target == encoding_rs::UTF_8 {
        return Some(String::from_utf8_lossy(value).into_owned().into_bytes());
    }
    let text = std::str::from_utf8(value).ok()?;
    let (encoded, _, _) = target.encode(text);
    let (decoded, _, _) = target.decode(&encoded);
    Some(decoded.into_owned().into_bytes())
}

/// 将 MySQL 一基 `pos`/`len` 转为半开区间；pos=0 或越界返回 None。
fn substringBounds(length: usize, position: i64, requested: Option<i64>) -> Option<(usize, usize)> {
    if position == 0 {
        return None;
    }
    // MySQL 一基下标：正数从左，负数从右回退。
    let start = if position > 0 {
        usize::try_from(position - 1).ok()?
    } else {
        length.checked_sub(position.unsigned_abs() as usize)?
    };
    if start >= length {
        return None;
    }
    let take = match requested {
        Some(value) if value <= 0 => return None,
        Some(value) => usize::try_from(value).unwrap_or(usize::MAX),
        None => usize::MAX,
    };
    Some((start, length.min(start.saturating_add(take))))
}

/// SUBSTRING 二进制路径：按字节切片。
pub fn substringBytes(value: &[u8], position: i64, length: Option<i64>) -> Vec<u8> {
    substringBounds(value.len(), position, length)
        .map(|(start, end)| value[start..end].to_vec())
        .unwrap_or_default()
}

/// SUBSTRING UTF-8 路径：按字符切片。
pub fn substringUtf8(value: &str, position: i64, length: Option<i64>) -> String {
    let chars: Vec<_> = value.chars().collect();
    substringBounds(chars.len(), position, length)
        .map(|(start, end)| chars[start..end].iter().collect())
        .unwrap_or_default()
}

/// SUBSTRING_INDEX：按分隔符出现次数截取；负 count 从右侧计数。
pub fn substringIndex(value: &str, delimiter: &str, count: i64) -> String {
    if count == 0 || delimiter.is_empty() {
        return String::new();
    }
    // 正 count：取第 count 次分隔符之前的前缀。
    if count > 0 {
        let mut seen = 0i64;
        for (index, _) in value.match_indices(delimiter) {
            seen += 1;
            if seen == count {
                return value[..index].to_owned();
            }
        }
    } else {
        let wanted = count.unsigned_abs();
        let matches: Vec<_> = value
            .match_indices(delimiter)
            .map(|(index, _)| index)
            .collect();
        if wanted <= matches.len() as u64 {
            let index = matches[matches.len() - wanted as usize] + delimiter.len();
            return value[index..].to_owned();
        }
    }
    value.to_owned()
}

/// LOCATE 二进制路径：返回一基字节位置；未找到为 0。
pub fn locateBinary(needle: &[u8], value: &[u8], position: i64) -> i64 {
    if position < 1 || position as usize > value.len().saturating_add(1) {
        return 0;
    }
    let start = position as usize - 1;
    if needle.is_empty() {
        return position;
    }
    value[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map_or(0, |index| (start + index + 1) as i64)
}

/// LOCATE UTF-8 路径：按字符定位；可大小写不敏感。
pub fn locateUtf8(needle: &str, value: &str, position: i64, case_insensitive: bool) -> i64 {
    let value_chars: Vec<_> = value.chars().collect();
    if position < 1 || position as usize > value_chars.len().saturating_add(1) {
        return 0;
    }
    if needle.is_empty() {
        return position;
    }
    let start_byte = value
        .char_indices()
        .nth(position as usize - 1)
        .map_or(value.len(), |(index, _)| index);
    let suffix = &value[start_byte..];
    let (haystack, needle) = if case_insensitive {
        (suffix.to_lowercase(), needle.to_lowercase())
    } else {
        (suffix.to_owned(), needle.to_owned())
    };
    haystack.find(&needle).map_or(0, |offset| {
        position + haystack[..offset].chars().count() as i64
    })
}

/// HEX(字符串)：大写十六进制编码。
pub fn hexString(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

/// HEX(整数)：按无符号 64 位十六进制输出。
pub fn hexInt(value: i64) -> String {
    format!("{:X}", value as u64)
}

/// UNHEX：十六进制解码；非法输入返回 None。
pub fn unhex(value: &str) -> Option<Vec<u8>> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = value.as_bytes();
    let mut result = Vec::with_capacity(bytes.len().div_ceil(2));
    let mut index = 0;
    if bytes.len() % 2 == 1 {
        result.push(digit(bytes[0])?);
        index = 1;
    }
    while index < bytes.len() {
        result.push(digit(bytes[index])? * 16 + digit(bytes[index + 1])?);
        index += 2;
    }
    Some(result)
}

/// 从左侧反复剥离 `remove` 前缀。
pub fn trimLeft<'a>(mut value: &'a str, remove: &str) -> &'a str {
    if remove.is_empty() {
        return value;
    }
    while let Some(rest) = value.strip_prefix(remove) {
        value = rest;
    }
    value
}

/// 从右侧反复剥离 `remove` 后缀。
pub fn trimRight<'a>(mut value: &'a str, remove: &str) -> &'a str {
    if remove.is_empty() {
        return value;
    }
    while let Some(rest) = value.strip_suffix(remove) {
        value = rest;
    }
    value
}

/// 双侧 TRIM（指定剥离串）。
pub fn trimBoth<'a>(value: &'a str, remove: &str) -> &'a str {
    trimRight(trimLeft(value, remove), remove)
}

/// LTRIM：剥离左侧空白。
pub fn ltrim(value: &str) -> &str {
    value.trim_start_matches(' ')
}

/// RTRIM：剥离右侧空白。
pub fn rtrim(value: &str) -> &str {
    value.trim_end_matches(' ')
}

/// LPAD 二进制：左侧填充至目标字节长度；pad 为空且需填充则 NULL。
pub fn lpadBytes(value: &[u8], target: i64, pad: &[u8]) -> Option<Vec<u8>> {
    if target < 0 || target > MAX_BLOB_WIDTH {
        return None;
    }
    if target == 0 {
        return Some(Vec::new());
    }
    let target = target as usize;
    if value.len() >= target {
        return Some(value[..target].to_vec());
    }
    if pad.is_empty() {
        return Some(Vec::new());
    }
    let needed = target - value.len();
    let mut result = Vec::with_capacity(target);
    for index in 0..needed {
        result.push(pad[index % pad.len()]);
    }
    result.extend_from_slice(value);
    Some(result)
}

/// RPAD 二进制：右侧填充至目标字节长度。
pub fn rpadBytes(value: &[u8], target: i64, pad: &[u8]) -> Option<Vec<u8>> {
    if target < 0 || target > MAX_BLOB_WIDTH {
        return None;
    }
    if target == 0 {
        return Some(Vec::new());
    }
    let target = target as usize;
    if value.len() >= target {
        return Some(value[..target].to_vec());
    }
    if pad.is_empty() {
        return Some(Vec::new());
    }
    let mut result = Vec::with_capacity(target);
    result.extend_from_slice(value);
    for index in 0..target - value.len() {
        result.push(pad[index % pad.len()]);
    }
    Some(result)
}

/// LPAD UTF-8：按字符数左侧填充。
pub fn lpadUtf8(value: &str, target: i64, pad: &str) -> Option<String> {
    if target < 0 || target > MAX_BLOB_WIDTH {
        return None;
    }
    if target == 0 {
        return Some(String::new());
    }
    let target = target as usize;
    let value: Vec<_> = value.chars().collect();
    if value.len() >= target {
        return Some(value[..target].iter().collect());
    }
    let pad: Vec<_> = pad.chars().collect();
    if pad.is_empty() {
        return Some(String::new());
    }
    let needed = target - value.len();
    Some(
        (0..needed)
            .map(|index| pad[index % pad.len()])
            .chain(value)
            .collect(),
    )
}

/// RPAD UTF-8：按字符数右侧填充。
pub fn rpadUtf8(value: &str, target: i64, pad: &str) -> Option<String> {
    if target < 0 || target > MAX_BLOB_WIDTH {
        return None;
    }
    if target == 0 {
        return Some(String::new());
    }
    let target = target as usize;
    let mut value: Vec<_> = value.chars().collect();
    if value.len() >= target {
        return Some(value[..target].iter().collect());
    }
    let pad: Vec<_> = pad.chars().collect();
    if pad.is_empty() {
        return Some(String::new());
    }
    let original_length = value.len();
    value.extend((0..target - original_length).map(|index| pad[index % pad.len()]));
    Some(value.into_iter().collect())
}

/// BIT_LENGTH：字节长度 × 8。
pub fn bitLength(value: &str) -> i64 {
    (value.len() as i64).saturating_mul(8)
}

/// Convert each integer to its shortest big-endian byte sequence, as MySQL CHAR does.
/// CHAR(n,...)：将整数码点/字节拼成字节串（含负数按无符号字节展开）。
pub fn charFromInts(values: &[Option<i64>]) -> Vec<u8> {
    let mut result = Vec::new();
    for value in values.iter().flatten() {
        let mut value = *value;
        let mut bytes = Vec::with_capacity(4);
        for _ in 0..4 {
            bytes.push((value & 0xff) as u8);
            value >>= 8;
            if value == 0 {
                break;
            }
        }
        bytes.reverse();
        result.extend(bytes);
    }
    result
}

/// CHAR_LENGTH UTF-8：字符个数。
pub fn charLength(value: &str) -> i64 {
    value.chars().count() as i64
}

/// FIND_IN_SET：在逗号列表中找一基位置；含逗号的 needle 永不匹配。
pub fn findInSet(needle: &str, list: &str, case_insensitive: bool) -> i64 {
    if needle.contains(',') {
        return 0;
    }
    list.split(',')
        .position(|candidate| {
            if case_insensitive {
                candidate.to_lowercase() == needle.to_lowercase()
            } else {
                candidate == needle
            }
        })
        .map_or(0, |index| index as i64 + 1)
}

/// FIELD：返回 needle 在后续参数中首次匹配的一基下标。
pub fn fieldString(needle: Option<&str>, values: &[Option<&str>], case_insensitive: bool) -> i64 {
    let Some(needle) = needle else {
        return 0;
    };
    values
        .iter()
        .position(|value| {
            value.is_some_and(|value| {
                if case_insensitive {
                    value.to_lowercase() == needle.to_lowercase()
                } else {
                    value == needle
                }
            })
        })
        .map_or(0, |index| index as i64 + 1)
}

/// MAKE_SET：按 bits 位图选取非 NULL 字符串，逗号拼接。
pub fn makeSet(bits: i64, values: &[Option<&str>]) -> String {
    values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            (index < 64 && (bits as u64 & (1u64 << index)) != 0)
                .then_some(*value)
                .flatten()
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// OCT：无符号八进制字符串。
pub fn oct(value: i64) -> String {
    format!("{:o}", value as u64)
}

/// BIN：无符号二进制字符串。
pub fn bin(value: i64) -> String {
    format!("{:b}", value as u64)
}

/// Interpret the encoded bytes of the first character as one big-endian integer.
/// ORD：按首字符 UTF-8 多字节编码折算整数值。
pub fn calcOrd(left_most: &[u8]) -> i64 {
    left_most.iter().fold(0i64, |result, byte| {
        result.wrapping_mul(256).wrapping_add(*byte as i64)
    })
}

/// Produce a value escaped exactly like MySQL `QUOTE`.
/// QUOTE：SQL 字符串字面量转义（含 \0、\Z 等）。
pub fn Quote(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('\'');
    for character in value.chars() {
        match character {
            '\\' | '\'' => {
                result.push('\\');
                result.push(character);
            }
            '\0' => result.push_str("\\0"),
            '\u{1a}' => result.push_str("\\Z"),
            _ => result.push(character),
        }
    }
    result.push('\'');
    result
}

/// ELT：一基下标选取；越界或 NULL 槽返回 None。
pub fn elt<'a>(index: i64, values: &[Option<&'a str>]) -> Option<&'a str> {
    if index < 1 {
        return None;
    }
    values.get(index as usize - 1).copied().flatten()
}

/// Evaluate `EXPORT_SET`; invalid bit counts use Go's default of 64.
/// EXPORT_SET：按位输出 on/off 串，负 bit 数回退为 64。
pub fn exportSet(bits: i64, on: &str, off: &str, separator: &str, number_of_bits: i64) -> String {
    let count = if (0..=64).contains(&number_of_bits) {
        number_of_bits as usize
    } else {
        64
    };
    (0..count)
        .map(|index| {
            if (bits as u64 & (1u64 << index)) != 0 {
                on
            } else {
                off
            }
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// Preserve Go's rounding helper, including the untrimmed fractional tail.
/// FORMAT 舍入前处理：按小数位规则进位数字串。
pub fn roundFormatArgs(value: &str, max_num_decimals: usize) -> String {
    let Some((integer, decimal)) = value.trim_start_matches('-').split_once('.') else {
        return value.to_owned();
    };
    let negative = value.starts_with('-');
    let mut integer = integer.as_bytes().to_vec();
    let mut decimal = decimal.as_bytes().to_vec();

    if decimal.len() > max_num_decimals {
        let mut carry = decimal[max_num_decimals] >= b'5';
        for digit in decimal[..max_num_decimals].iter_mut().rev() {
            if !carry {
                break;
            }
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
            }
        }
        for digit in integer.iter_mut().rev() {
            if !carry {
                break;
            }
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
            }
        }
        if carry {
            integer.insert(0, b'1');
        }
    }

    let mut result = String::with_capacity(value.len() + 1);
    if negative {
        result.push('-');
    }
    result.push_str(std::str::from_utf8(&integer).expect("decimal integer is ASCII"));
    result.push('.');
    result.push_str(std::str::from_utf8(&decimal).expect("decimal fraction is ASCII"));
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// FORMAT 各 locale 的千分位/小数分隔风格。
enum LocaleStyle {
    CommaDot,
    DotComma,
    SpaceComma,
    NoneComma,
    ApostropheDot,
    ApostropheComma,
    NoneDot,
    Indian,
}

/// 解析 locale 名到分隔风格；未知返回 None。
fn localeStyle(locale: &str) -> Option<LocaleStyle> {
    let locale = locale.to_ascii_lowercase();
    let style = match locale.as_str() {
        "de_de" | "es_es" | "id_id" | "vi_vn" | "ro_ro" | "da_dk" | "tr_tr" | "nb_no" | "uk_ua"
        | "no_no" => LocaleStyle::DotComma,
        "ru_ru" | "sv_se" | "cs_cz" => LocaleStyle::SpaceComma,
        "el_gr" | "pt_pt" | "it_it" | "pt_br" | "fr_fr" | "pl_pl" | "fr_ch" | "de_at" | "bg_bg" => {
            LocaleStyle::NoneComma
        }
        "de_ch" => LocaleStyle::ApostropheDot,
        "it_ch" => LocaleStyle::ApostropheComma,
        "ar_sa" | "sr_rs" => LocaleStyle::NoneDot,
        "en_in" | "ta_in" | "te_in" => LocaleStyle::Indian,
        "en_us" | "zh_cn" | "ja_jp" | "en_gb" | "ko_kr" | "th_th" | "en_au" | "zh_tw" | "es_mx"
        | "ce_ru" | "ky_kg" | "aa_dj" | "ps_af" | "an_es" | "az_az" | "br_fr" | "kv_ru"
        | "su_id" => LocaleStyle::CommaDot,
        _ => return None,
    };
    Some(style)
}

/// 对小数位做进位，必要时向整数部分进一。
fn incrementDecimalDigits(integer: &mut Vec<u8>, fraction: &mut [u8]) {
    let mut carry = true;
    for digit in fraction.iter_mut().rev() {
        if *digit == b'9' {
            *digit = b'0';
        } else {
            *digit += 1;
            carry = false;
            break;
        }
    }
    for digit in integer.iter_mut().rev() {
        if !carry {
            break;
        }
        if *digit == b'9' {
            *digit = b'0';
        } else {
            *digit += 1;
            carry = false;
        }
    }
    if carry {
        integer.insert(0, b'1');
    }
}

/// 对整数部分按千分位或印度分组插入分隔符。
fn groupInteger(integer: &str, separator: &str, indian: bool) -> String {
    if separator.is_empty() {
        return integer.to_owned();
    }
    let first_group = if indian { 3 } else { 3 };
    if integer.len() <= first_group {
        return integer.to_owned();
    }
    let mut groups = Vec::new();
    let mut end = integer.len();
    groups.push(&integer[end - first_group..end]);
    end -= first_group;
    let group_size = if indian { 2 } else { 3 };
    while end > group_size {
        groups.push(&integer[end - group_size..end]);
        end -= group_size;
    }
    groups.push(&integer[..end]);
    groups.reverse();
    groups.join(separator)
}

/// MySQL locale formatter.  The boolean reports whether the locale was known;
/// unknown/NULL callers use the en_US result and append a warning.
/// FORMAT：按 locale 格式化数字；返回 (结果, locale 是否识别)。
pub fn formatByLocale(value: &str, decimals: i64, locale: Option<&str>) -> (String, bool) {
    let decimals = decimals.clamp(0, FORMAT_MAX_DECIMALS) as usize;
    let found = locale.and_then(localeStyle).is_some();
    let style = locale
        .and_then(localeStyle)
        .unwrap_or(LocaleStyle::CommaDot);

    let input = value.trim();
    let negative = input.starts_with('-');
    let input = input.trim_start_matches(['-', '+']);
    let mut parts = input.splitn(2, '.');
    let integer_input = parts.next().unwrap_or_default();
    let fraction_input = parts.next().unwrap_or_default();
    let mut integer: Vec<u8> = integer_input
        .bytes()
        .take_while(u8::is_ascii_digit)
        .collect();
    if integer.is_empty() {
        integer.push(b'0');
    }
    let mut fraction: Vec<u8> = fraction_input
        .bytes()
        .take_while(u8::is_ascii_digit)
        .collect();
    let round_up = fraction.get(decimals).is_some_and(|digit| *digit >= b'5');
    fraction.resize(decimals, b'0');
    fraction.truncate(decimals);
    if round_up {
        incrementDecimalDigits(&mut integer, &mut fraction);
    }

    let (group, decimal, indian) = match style {
        LocaleStyle::CommaDot => (",", ".", false),
        LocaleStyle::DotComma => (".", ",", false),
        LocaleStyle::SpaceComma => (" ", ",", false),
        LocaleStyle::NoneComma => ("", ",", false),
        LocaleStyle::ApostropheDot => ("'", ".", false),
        LocaleStyle::ApostropheComma => ("'", ",", false),
        LocaleStyle::NoneDot => ("", ".", false),
        LocaleStyle::Indian => (",", ".", true),
    };
    let integer = std::str::from_utf8(&integer).expect("decimal integer is ASCII");
    let mut result = String::new();
    if negative && (integer != "0" || fraction.iter().any(|digit| *digit != b'0')) {
        result.push('-');
    }
    result.push_str(&groupInteger(integer, group, indian));
    if decimals > 0 {
        result.push_str(decimal);
        result.push_str(std::str::from_utf8(&fraction).expect("decimal fraction is ASCII"));
    }
    (result, found)
}

/// 估算 Base64 解码后字节长度；溢出返回 None。
pub fn base64NeededDecodedLength(length: usize) -> Option<usize> {
    length.checked_mul(3).map(|value| value / 4)
}

/// 估算 Base64 编码后长度（含换行）；溢出返回 None。
pub fn base64NeededEncodedLength(length: usize) -> Option<usize> {
    let encoded = length.checked_add(2)?.checked_div(3)?.checked_mul(4)?;
    encoded.checked_add(encoded.saturating_sub(1) / 76)
}

/// FROM_BASE64：忽略空白解码；超包或非法返回 None。
pub fn fromBase64(value: &str, max_allowed_packet: usize) -> Option<Vec<u8>> {
    if base64NeededDecodedLength(value.len())? > max_allowed_packet {
        return None;
    }
    let compact: String = value
        .chars()
        .filter(|character| !matches!(character, ' ' | '\t' | '\r' | '\n'))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()
}

/// TO_BASE64：编码并按 76 字符换行；超包返回 None。
pub fn toBase64(value: &[u8], max_allowed_packet: usize) -> Option<String> {
    if base64NeededEncodedLength(value.len())? > max_allowed_packet {
        return None;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(value);
    Some(splitToSubN(&encoded, 76).join("\n"))
}

/// Split by bytes, as the Go helper does for ASCII Base64 output.
/// 将字符串按固定字符宽度切段（Base64 换行辅助）。
pub fn splitToSubN(value: &str, length: usize) -> Vec<&str> {
    if length == 0 || value.len() <= length {
        return vec![value];
    }
    let mut result = Vec::with_capacity(value.len() / length + 1);
    let mut rest = value;
    while rest.len() > length {
        result.push(&rest[..length]);
        rest = &rest[length..];
    }
    result.push(rest);
    result
}

/// INSERT 二进制：从一基位置替换指定字节长度。
pub fn insertBinary(
    value: &[u8],
    position: i64,
    length: i64,
    replacement: &[u8],
    max_allowed_packet: usize,
) -> Option<Vec<u8>> {
    if position < 1 || position as usize > value.len() {
        return Some(value.to_vec());
    }
    let start = position as usize - 1;
    let remove = if length < 0 {
        value.len() - start
    } else {
        (length as usize).min(value.len() - start)
    };
    let result_length = value.len() - remove + replacement.len();
    if result_length > max_allowed_packet {
        return None;
    }
    let mut result = Vec::with_capacity(result_length);
    result.extend_from_slice(&value[..start]);
    result.extend_from_slice(replacement);
    result.extend_from_slice(&value[start + remove..]);
    Some(result)
}

/// INSERT UTF-8：从一基字符位置替换；超包返回 None。
pub fn insertUtf8(
    value: &str,
    position: i64,
    length: i64,
    replacement: &str,
    max_allowed_packet: usize,
) -> Option<String> {
    let chars: Vec<_> = value.chars().collect();
    if position < 1 || position as usize > chars.len() {
        return Some(value.to_owned());
    }
    let start = position as usize - 1;
    let remove = if length < 0 {
        chars.len() - start
    } else {
        (length as usize).min(chars.len() - start)
    };
    let head: String = chars[..start].iter().collect();
    let tail: String = chars[start + remove..].iter().collect();
    let result_length = head.len() + replacement.len() + tail.len();
    if result_length > max_allowed_packet {
        return None;
    }
    Some(head + replacement + &tail)
}

/// INSTR 二进制：一基字节位置。
pub fn instrBinary(value: &[u8], needle: &[u8]) -> i64 {
    if needle.is_empty() {
        return 1;
    }
    value
        .windows(needle.len())
        .position(|window| window == needle)
        .map_or(0, |index| index as i64 + 1)
}

/// INSTR UTF-8：一基字符位置。
pub fn instrUtf8(value: &str, needle: &str, case_insensitive: bool) -> i64 {
    let (value, needle) = if case_insensitive {
        (value.to_lowercase(), needle.to_lowercase())
    } else {
        (value.to_owned(), needle.to_owned())
    };
    value
        .find(&needle)
        .map_or(0, |index| value[..index].chars().count() as i64 + 1)
}

/// Go deliberately returns SQL NULL for LOAD_FILE because TiDB servers do not
/// expose the local filesystem through this builtin.
/// LOAD_FILE：安全默认返回 None（无服务端读文件）。
pub fn loadFile(_path: &str) -> Option<Vec<u8>> {
    None
}

/// 构建 UTF-8 TRANSLATE 映射；to 较短时多余 from 字符映射为删除。
pub fn buildTranslateMap4UTF8(from: &[char], to: &[char]) -> HashMap<char, Option<char>> {
    let mut result = HashMap::new();
    for index in (to.len()..from.len()).rev() {
        result.insert(from[index], None);
    }
    for index in (0..from.len().min(to.len())).rev() {
        result.insert(from[index], Some(to[index]));
    }
    result
}

/// 构建二进制 TRANSLATE 映射；删除用 INVALID_BYTE 标记。
pub fn buildTranslateMap4Binary(from: &[u8], to: &[u8]) -> HashMap<u8, u16> {
    let mut result = HashMap::new();
    for index in (to.len()..from.len()).rev() {
        result.insert(from[index], INVALID_BYTE);
    }
    for index in (0..from.len().min(to.len())).rev() {
        result.insert(from[index], to[index] as u16);
    }
    result
}

/// TRANSLATE UTF-8：按字符映射/删除。
pub fn translateUtf8(value: &str, from: &str, to: &str) -> String {
    let map = buildTranslateMap4UTF8(
        &from.chars().collect::<Vec<_>>(),
        &to.chars().collect::<Vec<_>>(),
    );
    value
        .chars()
        .filter_map(|character| map.get(&character).copied().unwrap_or(Some(character)))
        .collect()
}

/// TRANSLATE 二进制：按字节映射/删除。
pub fn translateBinary(value: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let map = buildTranslateMap4Binary(from, to);
    value
        .iter()
        .filter_map(|byte| match map.get(byte).copied() {
            Some(INVALID_BYTE) => None,
            Some(mapped) => Some(mapped as u8),
            None => Some(*byte),
        })
        .collect()
}

/// Binary-collation WEIGHT_STRING is the byte string itself, with MySQL's
/// optional CHAR/BINARY length adjustment.
/// WEIGHT_STRING 二进制路径：截断/填充权重字节。
pub fn weightStringBinary(value: &[u8], length: Option<usize>, pad_with_space: bool) -> Vec<u8> {
    let Some(length) = length else {
        return value.strip_suffix(b" ").map_or_else(
            || value.to_vec(),
            |value| weightStringBinary(value, None, false),
        );
    };
    if value.len() >= length {
        return value[..length].to_vec();
    }
    let mut result = Vec::with_capacity(length);
    result.extend_from_slice(value);
    result.resize(length, if pad_with_space { b' ' } else { 0 });
    if pad_with_space {
        result.truncate(
            result
                .iter()
                .rposition(|byte| *byte != b' ')
                .map_or(0, |i| i + 1),
        );
    }
    result
}

/// SPACE：生成空格串；负数为空串，超包为 NULL。
pub fn space(count: i64, max_allowed_packet: usize) -> Option<String> {
    if count < 1 {
        return Some(String::new());
    }
    let count = usize::try_from(count).ok()?;
    (count <= max_allowed_packet).then(|| " ".repeat(count))
}
