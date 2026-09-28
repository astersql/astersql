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

// Executor 内部测试与调试辅助工具。
//
// 提供随机字符串生成、调用栈函数名解析，以及临时存储目录的泄漏文件检查，
// 供 spill（磁盘溢出）等路径的单元测试断言“无残留文件”。

use std::path::{Path, PathBuf};

use rand::Rng;

/// 生成随机 ASCII 字符串时使用的字母表（大小写字母与数字）。
pub const LETTER_BYTES: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Returns a pseudorandom ASCII string with the requested byte length.
/// 返回指定字节长度的伪随机 ASCII 字符串（字符取自 `LETTER_BYTES`）。
pub fn GenerateRandomString(length: usize) -> String {
    let letters = LETTER_BYTES.as_bytes();
    let mut rng = rand::thread_rng();
    // 逐字节从字母表中均匀抽样并拼成字符串。
    (0..length)
        .map(|_| letters[rng.gen_range(0..letters.len())] as char)
        .collect()
}

/// Returns the demangled name of the calling Rust function.
/// 返回调用方 Rust 函数的 demangle 后名称（用于测试定位调用栈帧）。
#[inline(never)]
pub fn GetFunctionName() -> String {
    let trace = backtrace::Backtrace::new();
    // 展开所有栈帧符号名，便于在后续过滤中定位调用者。
    let names: Vec<String> = trace
        .frames()
        .iter()
        .flat_map(|frame| frame.symbols())
        .filter_map(|symbol| symbol.name().map(|name| name.to_string()))
        .collect();

    // 跳过本函数自身与 backtrace 库帧，取最近的真实调用者。
    let own_frame = names
        .iter()
        .position(|name| name.contains("GetFunctionName"));
    own_frame
        .and_then(|index| {
            names[index + 1..]
                .iter()
                .find(|name| !name.contains("GetFunctionName") && !name.starts_with("backtrace::"))
        })
        .cloned()
        .unwrap_or_default()
}

#[derive(Debug, thiserror::Error)]
/// 临时目录泄漏检查过程中的错误。
pub enum LeakCheckError {
    /// 遍历临时存储目录失败。
    #[error("failed to walk temporary storage: {0}")]
    Walk(#[from] walkdir::Error),
    /// 发现仍存在的泄漏文件（basename 匹配测试前缀）。
    #[error("leaked file: {path}")]
    LeakedFile { path: PathBuf },
}

impl LeakCheckError {
    /// 返回与错误相关的路径（Walk 失败时尽量取 walkdir 提供的路径）。
    pub fn path(&self) -> &Path {
        match self {
            Self::Walk(error) => error.path().unwrap_or_else(|| Path::new("")),
            Self::LeakedFile { path } => path,
        }
    }
}

/// Recursively verifies that temporary storage contains no file whose basename
/// begins with `file_name_prefix_for_test`.
/// 递归检查临时存储目录中是否存在 basename 以给定前缀开头的文件；
/// 若有则视为 spill/临时文件泄漏并返回错误。
pub fn CheckNoLeakFiles(
    temp_storage_path: impl AsRef<Path>,
    file_name_prefix_for_test: &str,
) -> Result<(), LeakCheckError> {
    for entry in walkdir::WalkDir::new(temp_storage_path) {
        let entry = entry?;
        // 与 Go WalkDir 的 `!d.IsDir()` 一致：目录名不参与，符号链接等非目录项参与。
        if !entry.file_type().is_dir()
            && entry
                .file_name()
                .to_string_lossy()
                .starts_with(file_name_prefix_for_test)
        {
            return Err(LeakCheckError::LeakedFile {
                path: entry.into_path(),
            });
        }
    }
    Ok(())
}
