// Copyright 2026 AsterSQL.
//
// 本模块是 `br/pkg/pdutil` 的 Rust 入口，对应 Go 包 `pdutil`。
// 职责是把 PD 客户端封装（`pd`）与时间/调度辅助（`utils`）组装成统一 crate。
// 生产路径只挂接 `pd`/`utils`；测试模块在 `#[cfg(test)]` 下按 path 接入，避免污染正常编译。
// `pub use` 平铺导出，使上层可像 Go 一样直接使用包级符号，而不必关心子文件边界。
// 对迁移工作而言，这里也是核对 pdutil 子模块是否已接入 Rust 的总索引。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// PD 控制面客户端与限流、暂停调度等核心能力。
#[path = "pd.rs"]
pub mod pd;

// 与 PD 协作时常用的时间转换、退避等辅助函数。
#[path = "utils.rs"]
pub mod utils;

// Go/Rust 行为对齐测试，仅在测试构建中编译。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

// 串行/集成向的 PD 交互测试，隔离并发干扰。
#[cfg(test)]
#[path = "pd_serial_test.rs"]
mod pd_serial_test;

#[cfg(test)]
#[path = "pd_test.rs"]
mod pd_test;

#[cfg(test)]
#[path = "utils_test.rs"]
mod utils_test;

// 平铺导出，对齐 Go `package pdutil` 的导入体验。
pub use pd::*;
pub use utils::*;
