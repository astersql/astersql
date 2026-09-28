// Copyright 2026 AsterSQL.

// server/internal crate 根模块：MySQL 线协议包读写。
//
// 再导出 `packetio`（PacketIO、压缩读写等），为上层连接层提供
// MySQL 协议包的序列号管理、分包与可选 zlib/zstd 压缩。

#[path = "packetio.rs"]
/// MySQL PacketIO 与压缩协议实现。
pub mod packetio;

pub use packetio::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
