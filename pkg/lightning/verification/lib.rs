// Copyright 2026 AsterSQL.

// Lightning verification 子 crate：导入后 KV 校验和与分组聚合。
//
// 对外重导出 `checksum` 模块；测试通过 `#[path]` 挂载。

#![allow(non_snake_case, non_upper_case_globals)]

/// CRC64 XOR 校验和与 KV 分组统计。
mod checksum;
pub use checksum::*;

#[cfg(test)]
#[path = "checksum_test.rs"]
mod checksum_test;
