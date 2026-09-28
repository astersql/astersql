// Copyright 2026 AsterSQL.

// AutoID（自动标识）分配 crate 入口。
//
// 本包负责为表生成自增 ID、隐式 `_tidb_rowid`、`AUTO_RANDOM`（带分片位的伪随机主键）
// 以及 SEQUENCE（序列对象）所需的 ID。核心逻辑在 `autoid`；远程单点分配服务
// 在 `autoid_service`；内存分配器在 `memid`；错误与文案在 `errors`。
//
// 对外 re-export 各子模块的公开 API，测试文件通过 `#[path]` 挂到本 crate。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]
/// 本地缓存式分配器、批大小计算与 AUTO_RANDOM 位布局。
pub mod autoid;
/// 基于 etcd Leader 发现的远程 AutoID 服务客户端与单点分配器。
pub mod autoid_service;
/// AutoID 相关错误类型与 AUTO_RANDOM 校验文案。
pub mod errors;
/// 临时表等场景使用的纯内存分配器。
pub mod memid;
pub use autoid::*;
pub use autoid_service::*;
pub use errors::*;
pub use memid::*;

#[cfg(test)]
#[path = "autoid_service_1_aster_unit_test.rs"]
mod autoid_service_1_aster_unit_test;
#[cfg(test)]
#[path = "autoid_service_test.rs"]
mod autoid_service_test;
#[cfg(test)]
#[path = "autoid_test.rs"]
mod autoid_test;
#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "memid_test.rs"]
mod memid_test;
#[cfg(test)]
#[path = "seq_autoid_test.rs"]
mod seq_autoid_test;
