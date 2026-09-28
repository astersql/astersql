// Copyright 2026 AsterSQL.

// Lightning TikV 交互子 crate：本地 SST 写入、属性收集与集群管控客户端。
//
// - `prop_collector`：为 SST 生成 MVCC（多版本并发控制）与 Range 属性；
// - `local_sst_writer`：按 write CF 编码写出本地 SST 文件；
// - `tikv`：PD/TiKV 客户端抽象、导入模式切换、压缩与版本检查。
//
// 测试通过 `#[path]` 挂载，与实现同 crate 编译。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

/// SST 表属性（MVCC / Range）收集器。
mod prop_collector;
pub use prop_collector::*;
/// 本地 write-CF SST 编解码与写入。
mod local_sst_writer;
pub use local_sst_writer::*;
/// PD/TiKV 远程操作与版本校验。
mod tikv;
pub use tikv::*;

#[cfg(test)]
#[path = "local_sst_writer_test.rs"]
mod local_sst_writer_test;
#[cfg(test)]
#[path = "prop_collector_test.rs"]
mod prop_collector_test;
#[cfg(test)]
#[path = "tikv_test.rs"]
mod tikv_test;
