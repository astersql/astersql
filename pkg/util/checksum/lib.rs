// Copyright 2026 AsterSQL.

// `util/checksum` crate 入口：带 CRC-32 块校验的读写封装。
//
// 对应 Go `util/checksum`。按固定块写入 IEEE CRC-32 头与用户载荷，
// 读取时校验每个编码块，用于 spill / 落盘等需要防损坏的字节流场景。

#![allow(non_snake_case, non_upper_case_globals)]

/// 校验和 Writer / Reader 核心实现。
pub mod checksum;
pub use checksum::*;

#[cfg(test)]
#[path = "checksum_test.rs"]
/// 对应 Go `checksum_test.go` 的读写与损坏检测测试。
mod checksum_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充：块编码、跨块读与 short write 语义回归。
mod migration_aster_unit_test;
