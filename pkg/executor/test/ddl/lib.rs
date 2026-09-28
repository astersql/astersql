// Copyright 2026 AsterSQL.

// `pkg/executor/test/ddl` crate 根：挂接 DDL（数据定义语言）执行器测试模块。
//
// 对应 Go 包 `executor/test/ddl`；仅在 `#[cfg(test)]` 下编译 `ddl_test` /
// `main_test`，不导出生产 API。

#![allow(dead_code)]

#[cfg(test)]
mod ddl_test;
#[cfg(test)]
mod main_test;
