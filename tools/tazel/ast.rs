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

//! 按目录统计 Go 测试文件中的 `Test*` 顶层函数数量。
//!
//! 该模块对应 Go 版 `tools/tazel/ast.go`，服务 `tazel` 工具对测试目录热度的粗粒度聚合。
//! Rust 迁移版通过 Go 词法规则识别函数声明，避免把注释或字符串中的文本当成声明。
//! 统计口径与 Go 保持一致：只计算名字以 `Test` 开头、没有接收者、且不是 `TestMain`
//! 的顶层测试函数；命中后按测试文件所在目录累加计数。
//! Count `Test*` functions per directory (Go `ast.go`).

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Go 包级 `testMap` 的 Rust 对应物。
///
/// 这里用 `OnceLock<Mutex<_>>` 延迟初始化全局计数表，既保留单例共享语义，
/// 也让 `initCount` 能像 Go 版重新开始统计那样清空已有结果。
pub static testMap: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

/// 提供内部访问入口，隐藏 `OnceLock` 的初始化细节。
fn test_map() -> &'static Mutex<HashMap<String, u32>> {
    testMap.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Go `initCount`。
///
/// 每轮遍历前清空累积结果，避免同一进程内重复调用时把旧目录计数带入新一轮扫描。
pub fn initCount() {
    test_map().lock().expect("testMap lock").clear();
}

/// Go `addTestMap`。
///
/// 目录首次出现时从 0 开始，之后每命中一个符合条件的测试函数就递增一次。
pub fn addTestMap(path: &str) {
    let mut map = test_map().lock().expect("testMap lock");
    *map.entry(path.to_string()).or_insert(0) += 1;
}

/// 为调用方返回当前目录计数快照。
///
/// 只暴露只读查询结果，避免外部绕过本模块约束直接改写全局表。
pub fn test_count_for(dir: &str) -> Option<u32> {
    test_map().lock().ok().and_then(|m| m.get(dir).copied())
}

/// 从指定根目录开始递归扫描。
///
/// Go 版固定从 `"."` 开始；这里拆出带参入口，便于测试或上层调用定向限制扫描范围。
pub fn walk_from(root: &Path) {
    if let Err(err) = walk_dir(root) {
        panic!("fail to walk: {err}");
    }
}

/// Go `walk` 的默认入口，保持“从当前目录出发”的调用习惯。
pub fn walk() {
    walk_from(Path::new("."));
}

/// 递归枚举目录，只把 `_test.go` 文件交给 `scan`。
///
/// 目录会继续下钻，非测试文件则直接跳过，等价于 Go `filepath.Walk` 回调里的筛选逻辑。
fn walk_dir(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.metadata()?.is_dir() {
            walk_dir(&path)?;
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with("_test.go") {
            scan(&path.to_string_lossy())?;
        }
    }
    Ok(())
}

/// Go `scan`。
///
/// 词法扫描会跳过 Go 注释、解释字符串、原始字符串和字符字面量，再按 token
/// 判断 `func <identifier>`；`func (<receiver>)` 方法不会被计数。
pub fn scan(path: &str) -> io::Result<()> {
    let abs_path: PathBuf = fs::canonicalize(path)?;
    let source = fs::read_to_string(&abs_path)?;

    validate_go_syntax(&abs_path)?;
    let tokens = go_tokens(&source)?;
    for pair in tokens.windows(2) {
        if pair[0] == "func"
            && pair[1] != "("
            && pair[1].starts_with("Test")
            && pair[1] != "TestMain"
        {
            if let Some(parent) = abs_path.parent() {
                addTestMap(&parent.to_string_lossy());
            }
        }
    }
    Ok(())
}

/// Validate with Go's parser before extracting declarations.
///
/// `ast.go` uses `parser.ParseFile(..., parser.AllErrors)`, so accepting a
/// balanced but otherwise invalid source file would change its error contract.
/// `gofmt -e` uses the same Go parser and reports all syntax errors without
/// modifying the input file.
fn validate_go_syntax(path: &Path) -> io::Result<()> {
    let output = Command::new("gofmt").arg("-e").arg(path).output()?;
    if output.status.success() {
        return Ok(());
    }

    let message = String::from_utf8_lossy(&output.stderr);
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        message.trim().to_string(),
    ))
}

fn go_tokens(source: &str) -> io::Result<Vec<String>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut delimiters = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 >= bytes.len() {
                    return invalid_go(start, "unterminated block comment");
                }
                i += 2;
            }
            b'"' | b'\'' => {
                let quote = bytes[i];
                let start = i;
                i += 1;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        closed = true;
                        break;
                    } else {
                        i += 1;
                    }
                }
                if !closed {
                    return invalid_go(start, "unterminated quoted literal");
                }
            }
            b'`' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    i += 1;
                }
                if i == bytes.len() {
                    return invalid_go(start, "unterminated raw string");
                }
                i += 1;
            }
            c if is_ident_start(c) => {
                let start = i;
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
                tokens.push(source[start..i].to_string());
            }
            b'(' | b'[' | b'{' => {
                delimiters.push(bytes[i]);
                tokens.push((bytes[i] as char).to_string());
                i += 1;
            }
            b')' | b']' | b'}' => {
                let expected = match bytes[i] {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                if delimiters.pop() != Some(expected) {
                    return invalid_go(i, "unbalanced delimiter");
                }
                tokens.push((bytes[i] as char).to_string());
                i += 1;
            }
            _ => i += 1,
        }
    }
    if !delimiters.is_empty() {
        return invalid_go(bytes.len(), "unclosed delimiter");
    }
    Ok(tokens)
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic() || byte >= 0x80
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

fn invalid_go<T>(offset: usize, message: &str) -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Go parse error at byte {offset}: {message}"),
    ))
}
