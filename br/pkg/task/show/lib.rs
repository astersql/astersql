// Copyright 2026 AsterSQL.

//! BR `show` 子命令 crate 入口：装配 stubs、cmd 与测试模块。
//! 对齐 Go `br/pkg/task/show`；CLI 查询备份元数据时经此路径导出类型。
//! `stubs` 提供 MetaReader 等桩，`cmd` 承载实际 show 流程；测试经 path 挂载。
//! 对外 `pub use` 把 cmd API 与 stubs 公共类型一并再导出，供 task 层直接引用。
//! 初始化顺序：先声明 path 模块，再统一 re-export，避免调用方感知内部文件拆分。

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
pub mod stubs;

#[path = "cmd.rs"]
pub mod cmd;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "cmd_test.rs"]
mod cmd_test;

pub use cmd::*;
pub use stubs::{
    CIStr, Context, DBInfo, Error, HexBytes, MemStorage, MetaReader, MetaTable, NewMetaReader,
    Result, TableInfo, backuppb, berrors, encryptionpb, objstore, set_read_backup_meta_hook, task,
};
