// Copyright 2026 AsterSQL.

// unistore 内嵌 mock TiKV 子系统入口。
//
// 聚合死锁检测、Region（键空间分片）管理、MVCC（多版本并发控制）、
// 服务端与写入路径等模块，供 TiDB 单测在进程内模拟 TiKV。

#![allow(dead_code)]

/// 死锁检测客户端/服务端封装。
pub mod deadlock;
/// 等待图死锁检测核心算法。
pub mod detector;
/// 内嵌存储服务生命周期（启动/停止/Raft 占位）。
pub mod inner_server;
/// Mock Region 元数据与路由。
pub mod mock_region;
/// MVCC 存储与事务读写逻辑。
pub mod mvcc;
/// Region 管理与键范围校验。
pub mod region;
/// KV RPC 服务端实现。
pub mod server;
/// 批处理 Raft/请求入口。
pub mod server_batch;
/// 通用工具函数。
pub mod util;
/// 写入与提交相关辅助。
pub mod write;

#[cfg(test)]
#[path = "deadlock_test.rs"]
mod deadlock_test;
/// 测试模块：死锁检测、TestMain、Mock PD、MVCC、工具函数。
#[cfg(test)]
#[path = "detector_test.rs"]
mod detector_test;
#[cfg(test)]
#[path = "inner_server_test.rs"]
mod inner_server_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "mock_pd_test.rs"]
mod mock_pd_test;
#[cfg(test)]
#[path = "mock_region_test.rs"]
mod mock_region_test;
#[cfg(test)]
#[path = "mvcc_test.rs"]
mod mvcc_test;
#[cfg(test)]
#[path = "region_test.rs"]
mod region_test;
#[cfg(test)]
#[path = "server_batch_test.rs"]
mod server_batch_test;
#[cfg(test)]
#[path = "server_test.rs"]
mod server_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
#[cfg(test)]
#[path = "write_test.rs"]
mod write_test;
