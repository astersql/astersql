// Copyright 2026 AsterSQL.

// mockcopr：mock TiKV 协处理器（Coprocessor）子系统的 crate 入口。
//
// 协处理器把部分 SQL 算子（扫描、过滤、聚合、TopN 等）下推到存储侧执行。
// 本 crate 组装 DAG 处理、RPC 入口、执行器与分析/校验相关子模块，供 mockstore 测试使用。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_mut,
    unused_variables
)]

mod aggregate;
mod analyze;
mod checksum;
mod cop_handler_dag;
mod copr_handler;
mod executor;
mod rpc_copr;
mod topn;

pub use cop_handler_dag::*;
pub use copr_handler::*;
pub use rpc_copr::*;

#[cfg(test)]
#[path = "aggregate_test.rs"]
mod aggregate_test;
#[cfg(test)]
#[path = "analyze_test.rs"]
mod analyze_test;
#[cfg(test)]
#[path = "checksum_test.rs"]
mod checksum_test;
#[cfg(test)]
#[path = "cop_handler_dag_test.rs"]
mod cop_handler_dag_test;
#[cfg(test)]
#[path = "copr_handler_test.rs"]
mod copr_handler_test;
#[cfg(test)]
#[path = "executor_test.rs"]
mod executor_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "rpc_copr_test.rs"]
mod rpc_copr_test;
