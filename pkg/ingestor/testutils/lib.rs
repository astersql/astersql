// Copyright 2026 AsterSQL.

// ingestor 测试工具库入口。
//
// 导出可追踪打开次数的内存对象存储包装（`TrackOpenMemStorage` 等），
// 供 simplesst / 外部存储相关单测断言 reader 生命周期与打开计数。
// 对象存储：云上键值文件抽象（如 S3），此处用内存实现避免真实网络。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 可追踪 Open/Close 计数的内存存储与 reader 包装。
mod util;
pub use util::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
