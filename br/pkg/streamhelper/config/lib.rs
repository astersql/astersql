// Copyright 2026 AsterSQL.

//! streamhelper 配置子 crate 入口：汇总命令行/TiDB 配置与 `Config` 抽象。
//!
//! 模块划分对齐 Go `br/pkg/streamhelper/config`：`command_conf` / `tidb_conf` /
//! `types`；测试模块仅在 `cfg(test)` 下挂载。
//! 对外 `pub use` 扁平导出常用符号，供 advancer 与测试直接引用。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]
#[path = "command_conf.rs"]
pub mod command_conf;
#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;
#[path = "tidb_conf.rs"]
pub mod tidb_conf;
#[cfg(test)]
#[path = "tidb_conf_test.rs"]
mod tidb_conf_test;
#[path = "types.rs"]
pub mod types;
/// 扁平再导出，供 advancer / 测试直接 `use` 配置类型与默认值。
pub use command_conf::*;
pub use tidb_conf::*;
pub use types::*;
