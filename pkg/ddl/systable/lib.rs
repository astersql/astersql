// Copyright 2026 AsterSQL.

// DDL 系统表访问子模块入口。
//
// 封装对 `mysql.tidb_ddl_job`、`mysql.tidb_mdl_info` 等 DDL 系统表的读写，
// 以及最小 job_id 的后台刷新（MinJobIdRefresher）。
// MDL（Metadata Lock，元数据锁）版本信息也经此路径查询。

#![allow(dead_code)]

/// 系统表 Manager：按 job_id 查询 job / MDL 版本、最小 job_id、flashback 作业存在性。
pub mod manager;
/// 最小 job_id 后台刷新器：周期性扫描系统表并单调推进缓存值。
pub mod min_job_id;
/// DDL 完成后触发 InfoSchema 重载的生产接口。
pub mod schema_loader;

pub use manager::*;
pub use min_job_id::*;
pub use schema_loader::*;

#[cfg(test)]
#[path = "manager_test.rs"]
mod manager_test;
#[cfg(test)]
#[path = "min_job_id_test.rs"]
mod min_job_id_test;
