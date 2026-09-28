// Copyright 2026 AsterSQL.

// TLS（传输层安全）相关 server 测试 crate 根模块。
//
// 在测试构建下挂载 `main_test` 与 `tls_test`，覆盖默认配置与
// TLS 配置热更新 / 禁用路径。

#![allow(dead_code)]

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod tls_test;
