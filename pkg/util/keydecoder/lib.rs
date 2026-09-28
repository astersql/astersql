// Copyright 2026 AsterSQL.

// Key 解码器包入口：将 TiKV/TiDB 编码的行键、索引键解析为可读结构。
//
// 对应 Go `pkg/util/keydecoder`。对外再导出 `keydecoder` 子模块中的解码类型与函数；
// 键（key）在此指存储层编码后的字节序列，解码后可得到库表名、分区、句柄（handle）等。

#![allow(non_snake_case, non_upper_case_globals)]

/// 核心解码实现（键类型判定、表/索引键解析等）。
mod keydecoder;

pub use keydecoder::*;

#[cfg(test)]
mod keydecoder_test;

#[cfg(test)]
mod main_test;
