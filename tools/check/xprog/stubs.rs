// Copyright 2026 AsterSQL.
//! Local path helpers matching Go `path/filepath` on Unix
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! Production file I/O uses `std::fs` directly. These helpers mirror Go
//! `filepath.Join` / `filepath.Clean` (lexical concat + clean — unlike
//! Rust `Path::join`, an element starting with `/` does **not** discard
//! prior elements).
//! 这个模块只补齐 `xprog` 所需的最小路径语义，不承担真实文件系统探测职责。
//! 之所以单独实现，是为了在工具侧复刻 Go `path/filepath` 的 Unix 词法规则，
//! 同时避免把 `PathBuf` 的平台相关拼接行为误当成 Go 版本的兼容结果。
//! 因而它更像一个轻量适配边界：为迁移代码提供可预测的字符串规则，
//! 但不会声明自己支持符号链接解析、卷前缀处理或其它超出 Go 原实现的能力。

use std::path::{Component, Path, PathBuf};

/// Go `filepath.Join` on Unix: concatenate with `/`, ignore empty elements,
/// then `Clean`. Does **not** reset on absolute-looking elements.
/// 这里先按 Go 的方式把非空片段直接用 `/` 连接，
/// 再统一走 `filepath_clean`，以保证 Rust 端得到和 Go 命令行工具一致的路径文本。
/// 特别要注意，输入片段里即使出现看起来像绝对路径的元素，也不会像 `Path::join`
/// 那样丢弃前缀；这是 `xprog` 计算目标测试产物路径时依赖的兼容点。
pub fn filepath_join(parts: &[&str]) -> String {
    let mut buf = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !buf.is_empty() {
            buf.push('/');
        }
        buf.push_str(part);
    }
    if buf.is_empty() {
        // Go returns "" when all elements are empty.
        // 这里保留空字符串而不是返回 `.`，是为了和 Go `filepath.Join`
        // 在“全部输入都为空”时的特殊返回值完全一致。
        return String::new();
    }
    filepath_clean(&buf)
}

/// Go `filepath.Clean` on Unix (lexical only; no symlink resolution).
/// `Clean` 只做纯词法归一化，不访问磁盘，也不会确认路径是否存在。
/// 这让它适合在构造 `importcfg.link`、测试二进制搬运目标等中间路径时复用，
/// 并与 Go 版工具在 `.`、`..` 与根目录边界上的表现保持一致。
pub fn filepath_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }

    let rooted = path.starts_with('/');
    let mut out: Vec<String> = Vec::new();

    for c in Path::new(path).components() {
        match c {
            Component::RootDir => {
                // Root is tracked via `rooted`; skip pushing.
                // 根目录单独由 `rooted` 记录，避免输出向量里混入哨兵值，
                // 这样后续处理 `..` 时可以更直接地复刻 Go 的根路径语义。
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = out.last() {
                    if last != ".." {
                        out.pop();
                        continue;
                    }
                }
                if !rooted {
                    out.push("..".to_string());
                }
                // If rooted, ".." at root is a no-op (Go Clean).
                // 非根路径要保留无法继续折叠的 `..`，否则相对路径层级会被错误吃掉；
                // 但在根路径下继续回退没有意义，所以这里故意什么都不追加。
            }
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
            Component::Prefix(_) => {}
        }
    }

    if out.is_empty() {
        return if rooted {
            "/".to_string()
        } else {
            ".".to_string()
        };
    }

    let body = out.join("/");
    if rooted { format!("/{body}") } else { body }
}

/// Convenience: build a `PathBuf` from a Go-cleaned path string.
/// 这里不再重新清理路径，只负责把上游已经按 Go 规则处理过的字符串
/// 包装成 `PathBuf`，避免调用方误以为此处会再次改变语义。
pub fn path_buf(path: &str) -> PathBuf {
    PathBuf::from(path)
}
