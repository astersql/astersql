// Copyright 2026 AsterSQL.

//! Crate entry for `lightning/pkg/server`
//! (Go package `github.com/pingcap/tidb/lightning/pkg/server`).
//!
//!
//! 这个 crate 负责把 server 子系统拆成入口、控制面、运行参数和依赖替身几个部分。
//! 它不直接实现导入算法，而是把 HTTP API、checkpoint 控制和 importer 适配组织起来。
//! `stubs` 提供当前迁移阶段需要的边界替身，隔离 kv、PD、HTTP 和对象存储等重依赖。
//! `run_options` 复刻 Go 的 Option 模式，让单次任务入口仍能通过闭包注入可选资源。
//! `sigusr1` 按平台拆分实现，保证 Unix 与非 Unix 的差异不会泄漏到上层调用点。
//! `checkpoint_control` 统一旧 Lightning 与 Import-Into 两套 checkpoint 控制语义。
//! `lightning` 承担服务生命周期、HTTP 路由、数据源初始化和 importer 选择。
//! 测试模块只在 `cfg(test)` 下编译，保持源文件与测试职责分离。
//! 因此可以把本文件视为 server 子系统的装配清单，而不是业务逻辑所在地。

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
pub use stubs::*;

#[path = "run_options.rs"]
mod run_options;
pub use run_options::*;

#[cfg(unix)]
#[path = "sigusr1_unix.rs"]
mod sigusr1;
#[cfg(not(unix))]
#[path = "sigusr1_other.rs"]
mod sigusr1;
pub use sigusr1::*;

#[path = "checkpoint_control.rs"]
mod checkpoint_control;
pub use checkpoint_control::*;

#[path = "lightning.rs"]
mod lightning;
pub use lightning::*;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "checkpoint_control_test.rs"]
mod checkpoint_control_test;

#[cfg(test)]
#[path = "lightning_serial_test.rs"]
mod lightning_serial_test;

#[cfg(test)]
#[path = "lightning_test.rs"]
mod lightning_test;

#[cfg(test)]
#[path = "lightning_server_serial_test.rs"]
mod lightning_server_serial_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "sigusr1_other_test.rs"]
mod sigusr1_other_test;

#[cfg(all(test, unix))]
#[path = "sigusr1_unix_test.rs"]
mod sigusr1_unix_test;
