// Copyright 2026 AsterSQL.

// 异步统计加载（asyncload）crate 入口。
//
// 将待加载的直方图/列/索引统计项登记到进程级队列，供后台按需从存储拉取，
// 避免查询路径同步阻塞在完整统计加载上。对应 Go 包 `pkg/statistics/asyncload`。

mod async_load;

pub use async_load::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
