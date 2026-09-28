// Copyright 2026 AsterSQL.
//
// 本模块是 `br/pkg/registry` 的 Rust 入口，对应 Go 包 `registry`。
// 负责组装 BR 任务在 PD/etcd 上的注册、心跳续约与桩实现，供备份恢复协调使用。
// `stubs` 提供尚未完整移植的依赖面；`heartbeat`/`registration` 承载主流程。
// 测试模块仅在 `#[cfg(test)]` 下挂接，避免生产编译引入对齐测试依赖。
// `pub use` 平铺导出，使上层可像 Go 一样直接使用包级符号。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    clippy::all
)]

// 迁移期桩与共享类型，填补尚未对齐的外部依赖。
#[path = "stubs.rs"]
pub mod stubs;

// 任务心跳上报与租约续期逻辑。
#[path = "heartbeat.rs"]
pub mod heartbeat;

// 任务在 registry 中的注册/注销与元数据读写。
#[path = "registration.rs"]
pub mod registration;

// Go/Rust 行为对齐测试，仅测试构建编译。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

// 平铺导出，对齐 Go `package registry` 的导入体验。
pub use heartbeat::*;
pub use registration::*;
pub use stubs::*;
