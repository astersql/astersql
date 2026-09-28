// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 为表数据文件生成唯一通配路径（wildcard path）。
//
// 导入时需用一条模式匹配「本表全部且仅本表」的数据文件。优先按 Mydumper
// （`db.table.chunk.ext[.compression]`）命名约定生成；失败则退回公共前后缀
// `prefix*suffix`，并保证 `*` 不跨越路径分隔符 `/`。

use crate::{ErrNoTableDataFiles, ErrWildcardNotSpecific};
use astersql_errors as errors;
use astersql_lightning_mydump as mydump;
use std::collections::{HashMap, HashSet};

/// 根据本表文件列表与全局文件集合，生成唯一匹配本表文件的通配路径。
pub(crate) fn generateWildcardPath(
    files: &[mydump::FileInfo],
    all_files: &HashMap<String, mydump::FileInfo>,
    database: &str,
    table: &str,
) -> Result<String, errors::SharedError> {
    // 本表路径集合，用于后续校验模式是否「只匹配本表」。
    let table_files: HashSet<String> = files
        .iter()
        .map(|file| file.file_meta.path.clone())
        .collect();

    if files.is_empty() {
        return Err(errors::Annotate(
            Some((*ErrNoTableDataFiles).clone()),
            "cannot generate wildcard pattern because the table has no data files",
        )
        .expect("annotating an existing error cannot return None"));
    }

    // 单文件无需引入通配符，直接返回源路径可以保证特异性。
    if files.len() == 1 {
        return Ok(files[0].file_meta.path.clone());
    }

    // 优先利用 Mydumper 的 db.table.chunk.ext 命名约定，得到可读且稳定的模式。
    let mut pattern = generateMydumperPattern(&files[0], database, table);
    if !pattern.is_empty() && isValidPattern(&pattern, &table_files, all_files) {
        return Ok(pattern);
    }

    // Mydumper 模式不适用时，再从所有实际路径提取公共前后缀。
    let paths: Vec<String> = files
        .iter()
        .map(|file| file.file_meta.path.clone())
        .collect();
    pattern = generatePrefixSuffixPattern(&paths);
    if !pattern.is_empty() && isValidPattern(&pattern, &table_files, all_files) {
        return Ok(pattern);
    }

    Err(errors::Annotate(
        Some((*ErrWildcardNotSpecific).clone()),
        "failed to find a wildcard that matches all and only the table's files",
    )
    .expect("annotating an existing error cannot return None"))
}

/// 校验模式是否恰好匹配 table_files 中的全部路径，且不匹配 all_files 中的其他路径。
pub(crate) fn isValidPattern(
    pattern: &str,
    table_files: &HashSet<String>,
    all_files: &HashMap<String, mydump::FileInfo>,
) -> bool {
    if pattern.is_empty() {
        return false;
    }

    for path in all_files.keys() {
        let is_match = pathPatternMatches(pattern, path);
        let is_table_file = table_files.contains(path);
        // 匹配结果与「是否属于本表」不一致则模式无效。
        if (is_match && !is_table_file) || (!is_match && is_table_file) {
            return false;
        }
    }
    true
}

/// 按 Mydumper 命名约定生成 `dir/db.table.*ext[.compression]` 模式。
pub(crate) fn generateMydumperPattern(
    file: &mydump::FileInfo,
    database: &str,
    table: &str,
) -> String {
    let full = &file.file_meta.path;
    // rfind('/') 保留 Go LastIndex 的行为，目录前缀包含末尾斜杠。
    let (dir_prefix, name) = match full.rfind('/') {
        Some(index) => (&full[..=index], &full[index + 1..]),
        None => ("", full.as_str()),
    };
    if database.is_empty() || table.is_empty() {
        return String::new();
    }

    // 有压缩时先剥离最后一个扩展名（例如 .gz/.zst），再获取数据格式扩展名。
    let compression_ext = if file.file_meta.compression != mydump::Compression::None {
        pathExtension(name)
    } else {
        ""
    };
    let base = name.strip_suffix(compression_ext).unwrap_or(name);
    let data_ext = pathExtension(base);
    format!("{dir_prefix}{database}.{table}.*{data_ext}{compression_ext}")
}

/// longestCommonPrefix 按 Go 字符串的字节索引语义求最长公共前缀。
pub(crate) fn longestCommonPrefix(values: &[String]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut length = first.len();
    for value in &values[1..] {
        length = first.as_bytes()[..length]
            .iter()
            .zip(value.as_bytes())
            .take_while(|(left, right)| left == right)
            .count();
        if length == 0 {
            break;
        }
    }
    // 路径通常是 UTF-8；若公共边界落在多字节字符内部，按损失转换明确保留 Go 的字节切片意图。
    String::from_utf8_lossy(&first.as_bytes()[..length]).into_owned()
}

/// longestCommonSuffix 从公共前缀之后开始求最长公共后缀，避免前缀与后缀重叠。
pub(crate) fn longestCommonSuffix(values: &[String], prefix_len: usize) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let first_remaining = &first.as_bytes()[prefix_len..];
    let mut length = first_remaining.len();
    for value in &values[1..] {
        let remaining = &value.as_bytes()[prefix_len..];
        length = first_remaining[first_remaining.len() - length..]
            .iter()
            .rev()
            .zip(remaining.iter().rev())
            .take_while(|(left, right)| left == right)
            .count();
        if length == 0 {
            break;
        }
    }
    String::from_utf8_lossy(&first_remaining[first_remaining.len() - length..]).into_owned()
}

/// generateFlatPrefixSuffixPattern 为不跨路径组件的字符串集合生成 prefix*suffix。
fn generateFlatPrefixSuffixPattern(paths: &[String]) -> String {
    let Some(first) = paths.first() else {
        return String::new();
    };
    // 全部相同则无需通配符。
    if paths.len() == 1 || paths[1..].iter().all(|path| path == first) {
        return first.clone();
    }

    let prefix = longestCommonPrefix(paths);
    let suffix = longestCommonSuffix(paths, prefix.len());
    format!("{prefix}*{suffix}")
}

/// generatePrefixSuffixPattern 在路径组件数一致时逐组件生成模式，保证每个 `*` 不跨越 `/`。
pub(crate) fn generatePrefixSuffixPattern(paths: &[String]) -> String {
    if paths.len() <= 1 {
        return generateFlatPrefixSuffixPattern(paths);
    }

    let path_components: Vec<Vec<&str>> =
        paths.iter().map(|path| path.split('/').collect()).collect();
    let component_count = path_components[0].len();
    if path_components
        .iter()
        .any(|parts| parts.len() != component_count)
        || component_count <= 1
    {
        // 组件数不一致时无法保证逐段对齐，退回 Go 的扁平前后缀算法。
        return generateFlatPrefixSuffixPattern(paths);
    }

    // 逐段生成 prefix*suffix，再用 `/` 拼回完整路径模式。
    let mut component_patterns = Vec::with_capacity(component_count);
    for component_index in 0..component_count {
        let component_values: Vec<String> = path_components
            .iter()
            .map(|parts| parts[component_index].to_owned())
            .collect();
        component_patterns.push(generateFlatPrefixSuffixPattern(&component_values));
    }
    component_patterns.join("/")
}

/// 返回路径最后一个扩展名（含点），若点号后含 `/` 则视为无扩展名。
fn pathExtension(path: &str) -> &str {
    path.rfind('.')
        .filter(|index| path[*index..].find('/').is_none())
        .map_or("", |index| &path[index..])
}

/// 按路径组件对齐后，判断完整路径是否匹配通配模式（`*`/`?`）。
fn pathPatternMatches(pattern: &str, path: &str) -> bool {
    let pattern_parts = pattern.split('/').collect::<Vec<_>>();
    let path_parts = path.split('/').collect::<Vec<_>>();
    pattern_parts.len() == path_parts.len()
        && pattern_parts
            .iter()
            .zip(path_parts)
            .all(|(pattern, value)| componentPatternMatches(pattern.as_bytes(), value.as_bytes()))
}

/// 单路径组件上的通配匹配，对齐 Go `filepath.Match` 的 `*`、`?`、字符类和反斜杠转义。
/// 非法字符类与悬空转义和 Go 的 `ErrBadPattern` 一样按不匹配处理。
fn componentPatternMatches(pattern: &[u8], value: &[u8]) -> bool {
    let Ok(pattern) = std::str::from_utf8(pattern) else {
        return false;
    };
    let Ok(value) = std::str::from_utf8(value) else {
        return false;
    };
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut memo = HashMap::new();
    matchComponent(&pattern, &value, 0, 0, &mut memo).unwrap_or(false)
}

fn matchComponent(
    pattern: &[char],
    value: &[char],
    pattern_index: usize,
    value_index: usize,
    memo: &mut HashMap<(usize, usize), bool>,
) -> Option<bool> {
    if let Some(result) = memo.get(&(pattern_index, value_index)) {
        return Some(*result);
    }
    if pattern_index == pattern.len() {
        return Some(value_index == value.len());
    }

    let result = match pattern[pattern_index] {
        '*' => {
            let mut next = pattern_index + 1;
            while next < pattern.len() && pattern[next] == '*' {
                next += 1;
            }
            if next == pattern.len() {
                true
            } else {
                let mut matched = false;
                for candidate in value_index..=value.len() {
                    if matchComponent(pattern, value, next, candidate, memo)? {
                        matched = true;
                        break;
                    }
                }
                matched
            }
        }
        '?' => {
            value_index < value.len()
                && matchComponent(pattern, value, pattern_index + 1, value_index + 1, memo)?
        }
        '[' => {
            let (next, matches) =
                matchCharacterClass(pattern, pattern_index, value.get(value_index).copied())?;
            matches && matchComponent(pattern, value, next, value_index + 1, memo)?
        }
        '\\' => {
            let literal = *pattern.get(pattern_index + 1)?;
            value.get(value_index) == Some(&literal)
                && matchComponent(pattern, value, pattern_index + 2, value_index + 1, memo)?
        }
        literal => {
            value.get(value_index) == Some(&literal)
                && matchComponent(pattern, value, pattern_index + 1, value_index + 1, memo)?
        }
    };
    memo.insert((pattern_index, value_index), result);
    Some(result)
}

fn matchCharacterClass(
    pattern: &[char],
    start: usize,
    value: Option<char>,
) -> Option<(usize, bool)> {
    let mut index = start + 1;
    let negated = pattern.get(index) == Some(&'^');
    index += usize::from(negated);
    let mut has_range = false;
    let mut matched = false;

    while pattern.get(index) != Some(&']') {
        let (lower, next) = escapedClassCharacter(pattern, index)?;
        index = next;
        let upper = if pattern.get(index) == Some(&'-') {
            let (upper, next) = escapedClassCharacter(pattern, index + 1)?;
            index = next;
            upper
        } else {
            lower
        };
        has_range = true;
        if value.is_some_and(|character| lower <= character && character <= upper) {
            matched = true;
        }
    }
    if !has_range || value.is_none() {
        return has_range.then_some((index + 1, false));
    }
    Some((index + 1, matched != negated))
}

fn escapedClassCharacter(pattern: &[char], index: usize) -> Option<(char, usize)> {
    let character = *pattern.get(index)?;
    if character == '\\' {
        Some((*pattern.get(index + 1)?, index + 2))
    } else if character == '-' || character == ']' {
        None
    } else {
        Some((character, index + 1))
    }
}
