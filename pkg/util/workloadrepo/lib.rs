// Copyright 2026 AsterSQL.

// workloadrepo crate 根：工作负载仓库（采样、快照、分区管家）。
//
// 汇总常量、housekeeper、snapshot/table/utils 与 worker；对外再导出后端接口、
// 建库启停及默认表定义。用于将运行时负载指标持久化到 WORKLOAD_SCHEMA。

#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]

/// 仓库相关常量（etcd 键、默认间隔、系统变量名等）。
#[path = "const.rs"]
mod consts;
/// 按日分区创建与过期清理。
mod housekeeper;
#[cfg(test)]
#[path = "housekeeper_test.rs"]
mod housekeeper_test;
/// 主动采样逻辑。
mod sampling;
#[cfg(test)]
#[path = "sampling_test.rs"]
mod sampling_test;
/// 全量快照逻辑。
mod snapshot;
#[cfg(test)]
#[path = "snapshot_test.rs"]
mod snapshot_test;
/// 仓库表结构与分区范围生成。
mod table;
#[cfg(test)]
#[path = "table_test.rs"]
mod table_test;
/// 通用工具函数。
mod utils;
#[cfg(test)]
mod utils_test;
/// 仓库 worker：后端抽象、启停与查询执行。
pub mod worker;

pub use consts::*;
pub use housekeeper::*;
pub use snapshot::*;
pub use table::*;
pub use utils::*;
/// 再导出 worker 侧核心类型与入口函数。
pub use worker::{
    ColumnDefinition, RepositoryBackend, Row, SetupRepository, StopRepository, Value,
    WorkloadRepoWorker, defaultWorkloadTables, execRetry, init, initializeWorker, metadataTable,
    runQuery, samplingTable, snapshotTable, takeSnapshot,
};

/// Worker 相关单元测试。
#[cfg(test)]
#[path = "worker_test.rs"]
mod worker_test;
