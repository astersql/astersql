// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/checkpoints`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/checkpoints`).
//!
//! 中文概览：这个入口把 Lightning 检查点子系统拆成“真实实现 + 迁移边界桩 + 测试”三层。
//! `checkpoints.rs` 承载真正的断点模型、增量合并和持久化协议，是上层恢复逻辑依赖的核心。
//! `stubs.rs` 则补齐当前 crate 在迁移过程中仍需引用的外部边界符号，避免尚未接线的依赖阻断编译。
//! crate 根在这里统一 `pub use`，让调用方继续沿用 Go 包级导出面的使用习惯。
//! 这样做的目的不是隐藏文件拆分，而是把“对外契约”固定下来，降低后续逐文件实装时的改动半径。
//! 测试模块按职责拆开：`parity_test` 看公共契约，`main_test` 看套件引导，其他测试覆盖文件与 SQL 后端。
//! 阅读顺序建议先看这里的导出关系，再进入 `checkpoints.rs`，最后回到各测试文件核对 Go 对齐点。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

#[path = "stubs.rs"]
mod stubs;
// 先导出迁移边界桩，保证主实现里引用到的外部能力有稳定占位接口。
pub use stubs::*;

#[path = "checkpoints.rs"]
mod checkpoints;
// 对外公开真实检查点实现；调用方应优先把这里视为包级主入口。
pub use checkpoints::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "checkpoints_test.rs"]
mod checkpoints_test;

#[cfg(test)]
#[path = "checkpoints_file_test.rs"]
mod checkpoints_file_test;

#[cfg(test)]
#[path = "checkpoints_sql_test.rs"]
mod checkpoints_sql_test;
