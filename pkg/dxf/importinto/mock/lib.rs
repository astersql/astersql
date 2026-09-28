// Copyright 2026 AsterSQL.

// IMPORT INTO mock crate 入口：再导出 GoMock 风格的 MiniTaskExecutor。

#![allow(dead_code)]

/// MiniTaskExecutor mock 实现与相关类型。
pub mod import_mock;

pub use import_mock::*;

#[cfg(test)]
mod import_mock_test;
