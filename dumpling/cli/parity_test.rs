// Copyright 2026 AsterSQL.

//! Parity tests for `dumpling/cli` public contracts vs Go `versions.go`.
//!
//! 这些测试锁定 `versions.rs` 对外可见的字符串格式与日志字段顺序，
//! 防止 Rust 版本在重构时偏离 Go `versions.go` 已公开的 CLI 契约。
//! 由于版本信息通常通过 ldflags 在构建阶段注入，这里同时覆盖默认值、
//! 自定义值、空字符串边界以及日志等级过滤等行为。

use astersql_dumpling_log::{Level, Logger, ZapLogger};

use crate::{
    BuildTimestamp, GitBranch, GitHash, LogLongVersion, LongVersion, ReleaseVersion, RustVersion,
    reset_version_vars,
};

#[test]
fn go_rust_public_contract_matches() {
    // 聚合执行四类契约，确保一次失败即可指向具体语义回归。
    contract_normal_defaults_and_log();
    contract_boundary_custom_values();
    contract_error_logger_level_filter();
    contract_resource_cleanup_resets_vars();
}

/// Normal: default `"Unknown"` LongVersion format and LogLongVersion fields.
fn contract_normal_defaults_and_log() {
    // 先恢复共享全局状态，避免其他测试写入的版本信息污染当前断言。
    reset_version_vars();
    let text = LongVersion();
    let rust_version = rustc_version_runtime::version().to_string();
    // Rust 编译器版本应来自当前构建工具链。
    assert_eq!(
        text,
        format!(
            "Release version: Unknown\n\
Git commit hash: Unknown\n\
Git branch:      Unknown\n\
Build timestamp: UnknownZ\n\
Rust version:    {rust_version}\n"
        )
    );

    let logger = Logger {
        Logger: ZapLogger::capture(Level::Info),
    };
    // Go 版本会在 Info 级别输出欢迎语与键值字段，Rust 端必须保持一致。
    LogLongVersion(&logger);
    let entries = logger.entries();
    assert_eq!(entries.len(), 1);
    // 这里不比较整条日志字面量，而是逐段检查关键字段，降低日志框架细节噪声。
    assert!(entries[0].contains("[INFO] Welcome to dumpling"));
    assert!(entries[0].contains("Release Version=Unknown"));
    assert!(entries[0].contains("Git Commit Hash=Unknown"));
    assert!(entries[0].contains("Git Branch=Unknown"));
    assert!(entries[0].contains("Build timestamp=Unknown"));
    assert!(entries[0].contains(&format!("Rust Version={rust_version}")));
}

/// Boundary: ldflags-style overrides and empty strings still format correctly.
fn contract_boundary_custom_values() {
    // 这里模拟构建脚本通过 ldflags 注入版本元数据后的展示结果。
    reset_version_vars();
    *ReleaseVersion.write().unwrap() = "v7.5.0";
    *GitHash.write().unwrap() = "abc123def";
    *GitBranch.write().unwrap() = "master";
    *BuildTimestamp.write().unwrap() = "2026-07-27 03:00:00";
    *RustVersion.write().unwrap() = Some("rustc 1.85.0");

    let text = LongVersion();
    // 自定义值分支验证的是“值替换后仍保持原布局”，不是只看是否能读到变量。
    assert_eq!(
        text,
        "Release version: v7.5.0\n\
Git commit hash: abc123def\n\
Git branch:      master\n\
Build timestamp: 2026-07-27 03:00:00Z\n\
Rust version:    rustc 1.85.0\n"
    );

    // Empty values: Go still prints labels and trailing Z on timestamp.
    // 空字符串同样属于有效输入，重点验证格式骨架而不是字段内容本身。
    *ReleaseVersion.write().unwrap() = "";
    *GitHash.write().unwrap() = "";
    *GitBranch.write().unwrap() = "";
    *BuildTimestamp.write().unwrap() = "";
    *RustVersion.write().unwrap() = Some("");
    assert_eq!(
        LongVersion(),
        "Release version: \n\
Git commit hash: \n\
Git branch:      \n\
Build timestamp: Z\n\
Rust version:    \n"
    );
}

/// Error/filter: below-Info logger drops the welcome line (zap level gate).
fn contract_error_logger_level_filter() {
    reset_version_vars();
    let logger = Logger {
        Logger: ZapLogger::capture(Level::Error),
    };
    // 当 logger 的门限高于 Info 时，欢迎日志必须被完全抑制。
    LogLongVersion(&logger);
    // 若这里仍有输出，说明 Rust 封装的等级判断与 Go zap 语义出现偏差。
    assert!(
        logger.entries().is_empty(),
        "Info must not emit when logger level is Error"
    );

    // Nop sink: call must not panic and must leave no capture entries.
    // 无输出 sink 对应 Go 中常见的空 logger，用来约束调用方可安全忽略版本日志。
    let nop = Logger::nop();
    LogLongVersion(&nop);
    assert!(nop.entries().is_empty());
}

/// Resource: mutating helpers restore `"Unknown"` defaults after use.
fn contract_resource_cleanup_resets_vars() {
    // 直接把全局变量写脏，验证 reset 帮助函数能恢复 Go 约定的默认值。
    *ReleaseVersion.write().unwrap() = "dirty";
    *BuildTimestamp.write().unwrap() = "dirty";
    *GitHash.write().unwrap() = "dirty";
    *GitBranch.write().unwrap() = "dirty";
    *RustVersion.write().unwrap() = Some("dirty");
    reset_version_vars();
    // 逐项断言而不是只比字符串，便于定位到底是哪一个共享状态没有被清理。
    assert_eq!(*ReleaseVersion.read().unwrap(), "Unknown");
    assert_eq!(*BuildTimestamp.read().unwrap(), "Unknown");
    assert_eq!(*GitHash.read().unwrap(), "Unknown");
    assert_eq!(*GitBranch.read().unwrap(), "Unknown");
    assert_eq!(*RustVersion.read().unwrap(), None);
    // 最后一条字符串断言再次覆盖 LongVersion 的默认拼接路径，防止只清理存储未更新展示层。
    assert!(LongVersion().contains("Release version: Unknown\n"));
}
