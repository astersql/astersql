// Copyright 2026 AsterSQL.

// Lightning 后端编码子 crate 入口。
//
// 再导出 `encode` 模块中的行编码接口、Datum（列值）、编码器与行缓冲等类型，
// 供 KV 编码与数据导入路径统一使用。

#![allow(non_snake_case)]

mod encode;
pub use encode::*;
