// Copyright 2026 AsterSQL.

// ingestcli mock 子 crate：手写 GoMock 风格的 Client / WriteClient 替身。
//
// 供上层测试排队期望（EXPECT）、记录调用并在结束时 verify，避免依赖真实 HTTP/TiKV。

#![allow(non_snake_case)]

/// MockClient / MockWriteClient 及其 recorder。
mod client_mock;
pub use client_mock::*;

#[cfg(test)]
#[path = "client_mock_test.rs"]
mod client_mock_test;
