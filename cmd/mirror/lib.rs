// Copyright 2026 AsterSQL.

// 本文件承担共享库入口的装配职责，独立包装层提供二进制入口。
// `#![allow(...)]` 集中放在这里，避免迁移兼容配置分散到子模块。
// 测试通过 `#[path]` 直接纳入 parity 模块，确保二进制与测试共用同一实现。
// 真正业务逻辑仍在 `mirror.rs`，这里保持薄转发以降低入口层复杂度。
// 因此本文件的关键契约只有模块装配顺序与 `main` 的单跳调用。
// 补充注释的目的也是让读者快速理解它为何几乎没有业务代码。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

#[path = "stubs.rs"]
/// `stubs` 收口与 Go 版本对齐所需的外部依赖抽象，
/// 让入口层只负责装配，不直接持有文件系统或命令执行细节。
pub mod stubs;

#[path = "mirror.rs"]
/// `mirror` 模块承载真正的命令流程，
/// `lib.rs` 对它的暴露方式应保持稳定，避免入口与实现边界混淆。
pub mod mirror;

#[cfg(test)]
#[path = "parity_test.rs"]
// parity 测试从这里挂接，确保入口装配变化也会被同一组对齐用例覆盖。
mod parity_test;

#[cfg(test)]
#[path = "mirror_test.rs"]
mod mirror_test;

/// Shared process entrypoint called by the binary wrapper.
/// `main` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn main() {
    mirror::main();
}
