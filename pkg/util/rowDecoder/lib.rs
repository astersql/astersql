// Copyright 2026 AsterSQL.

// 行解码器（rowDecoder）crate 入口。
//
// 对应 Go `pkg/util/rowDecoder`：将编码行字节解码为 Datum 映射，并填充默认值、
// 评估生成列（generated column）。对外重导出 `decoder` 模块的全部公共 API。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

mod decoder;

pub use decoder::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "decoder_test.rs"]
mod decoder_test;
