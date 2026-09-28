// Copyright 2026 AsterSQL.

// server/internal/resultset crate 根模块：查询结果集与游标封装。
//
// 再导出 `resultset`（TidbResultSet、RU v2 游标追踪）与 `cursor`
// （行迭代器 / lazy cursor），供 MySQL 协议层按 chunk 或逐行推送结果。

#![allow(dead_code, non_snake_case)]

/// 游标结果集与行迭代器（RowContainer / Lazy）。
mod cursor;
pub use cursor::*;
/// 核心 ResultSet 实现与 CursorRUV2Tracker。
mod resultset;
pub use resultset::*;

#[cfg(test)]
#[path = "resultset_aster_unit_test.rs"]
mod resultset_aster_unit_test;
