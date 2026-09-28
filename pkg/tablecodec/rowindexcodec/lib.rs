// Copyright 2026 AsterSQL.

// `rowindexcodec`：行与索引键值的编码辅助子 crate（对应 Go `pkg/tablecodec/rowindexcodec`）。
//
// 对表记录键、索引键中的 handle（行标识）等进行编解码封装，供 tablecodec 与存储层复用。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 行/索引编解码实现模块。
pub mod rowindexcodec;
/// 再导出行索引编解码 API。
pub use rowindexcodec::*;

// 包级与迁移单元测试入口。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
