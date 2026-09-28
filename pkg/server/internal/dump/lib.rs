// Copyright 2026 AsterSQL.

// server/internal/dump crate 入口。
//
// 以 `extern crate self as dump` 模拟 Go 包内自引用，导出协议 dump 原语，
// 并在测试配置下挂接 Go 对照与 Aster 迁移单测。

extern crate self as dump;
#[path = "dump.rs"]
mod dump_impl;
pub use dump_impl::*;

#[cfg(test)]
#[path = "dump_test.rs"]
mod dump_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
