// Copyright 2026 AsterSQL.
// configtypes crate 入口：定义配置系统使用的基础类型。
//
// 本 crate 从 Go(TiDB) 的 `pkg/config` 相关类型机械迁移而来，
// 主要提供配置项的通用类型封装（如带单位的字节大小 ByteSize、
// 支持人类可读格式解析的时长 Duration 等），
// 供上层配置解析与序列化模块复用。实际类型定义位于 `types` 子模块。

// 由于代码为 Go 到 Rust 的机械迁移，命名风格沿用 Go 习惯，
// 此处全局关闭相关命名与死代码告警，避免编译噪音。
#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 配置基础类型定义模块：包含 ByteSize（字节大小，支持 "1GB" 等单位写法）
/// 与 Duration（时长）及其 JSON/文本序列化、反序列化辅助函数。
pub mod types;
// 将 types 模块的公开项在 crate 根重导出，方便外部直接引用。
pub use types::*;

// 迁移期的单元测试模块：通过 #[path] 指向同目录下的测试文件，
// 仅在 cfg(test) 下编译，用于验证迁移后行为与原实现一致。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

// types 模块对应的单元测试，同样仅在测试构建时编译。
#[cfg(test)]
#[path = "types_test.rs"]
mod types_test;
