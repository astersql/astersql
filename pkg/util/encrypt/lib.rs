// Copyright 2026 AsterSQL.

// 加密工具 crate 入口：AES、CTR 流式层与 MySQL SQL 编解码。
//
// 对应 Go `pkg/util/encrypt`。子模块分别实现分组密码模式、按块 CTR 读写层，
// 以及 `ENCODE`/`DECODE` 兼容算法；测试通过 `#[path]` 挂载与 Go 同目录用例。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

extern crate self as util_encrypt;

/// AES 分组模式与 PKCS7 / MySQL 密钥派生。
pub mod aes;
/// AES-CTR 缓冲写入与按偏移解密读取层。
pub mod aes_layer;
/// MySQL 风格 SQLEncode/SQLDecode。
pub mod crypt;
/// 再导出 `aes` 公开 API。
pub use aes::*;
/// 再导出 `aes_layer` 公开 API。
pub use aes_layer::*;
/// 再导出 `crypt` 公开 API。
pub use crypt::*;

/// 对应 Go `aes_layer_test.go` 的 CTR 层管线测试。
#[cfg(test)]
#[path = "aes_layer_test.rs"]
mod aes_layer_test;
/// 对应 Go `aes_test.go` 的 AES 模式与填充测试。
#[cfg(test)]
#[path = "aes_test.rs"]
mod aes_test;
/// 对应 Go `crypt_test.go` 的 SQL 编解码测试。
#[cfg(test)]
#[path = "crypt_test.rs"]
mod crypt_test;
/// AsterSQL 迁移补充回归测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
