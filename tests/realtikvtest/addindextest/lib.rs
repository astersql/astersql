// Copyright 2026 AsterSQL.

// 本文件主要负责模块接线和导出关系说明。
// 阅读时关注哪些模块只在测试条件下启用。
// 中文注释只帮助快速判断依赖方向。
#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

extern crate self as astersql_tests_realtikvtest_addindextest;

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard};

// `TEST_SERIAL` 记录跨函数共享的固定约束、错误文本或全局状态。
static TEST_SERIAL: Mutex<()> = Mutex::new(());

/// Mutable package flag corresponding to Go's
/// `flag.Bool("full-mode", false, ...)`.
// `FULL_MODE` 记录跨函数共享的固定约束、错误文本或全局状态。
pub static FULL_MODE: AtomicBool = AtomicBool::new(false);

/// Serialize cases that mutate process-wide RealTiKV, config, or failpoint state.
// `serial_guard` 承担当前文件中的一段辅助职责或状态转换。
pub fn serial_guard() -> MutexGuard<'static, ()> {
    TEST_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Extract the Go boolean flag before the Rust harness parses its own options.
/// Accept exactly strconv.ParseBool's spellings; repeated flags use the last value.
pub fn parse_full_mode(
    args: impl IntoIterator<Item = String>,
) -> Result<(bool, Vec<String>), String> {
    let mut full_mode = false;
    let mut remaining = Vec::new();
    let mut terminated = false;
    for arg in args {
        if terminated {
            remaining.push(arg);
            continue;
        }
        if arg == "--" {
            terminated = true;
            remaining.push(arg);
        } else if arg == "-full-mode" || arg == "--full-mode" {
            full_mode = true;
        } else if let Some(value) = arg
            .strip_prefix("-full-mode=")
            .or_else(|| arg.strip_prefix("--full-mode="))
        {
            full_mode = match value {
                "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
                "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
                _ => return Err(format!("invalid boolean value {value:?} for -full-mode")),
            };
        } else {
            remaining.push(arg);
        }
    }
    Ok((full_mode, remaining))
}
