// Copyright 2026 AsterSQL.

// 统计 handle 工具子模块入口。
//
// 汇总自动分析进程 ID、租约、工作池、表信息查找及通用 util，并在测试配置下挂载 util 测试。

#![allow(dead_code)]

pub mod auto_analyze_proc_id_generator;
pub mod lease_getter;
pub mod pool;
pub mod table_info;
pub mod util;

pub use auto_analyze_proc_id_generator::*;
pub use lease_getter::*;
pub use pool::*;
pub use table_info::*;
pub use util::*;

#[cfg(test)]
#[path = "pool_test.rs"]
mod pool_test;

#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
