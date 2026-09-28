// Copyright 2026 AsterSQL.

// `pkg/executor/test/cte` crate 根：挂接 CTE（公用表表达式）相关测试模块。
//
// 对应 Go 包 `executor/test/cte`；本 crate 仅在 `#[cfg(test)]` 下编译测试源，
// 不导出生产 API。

#![allow(dead_code)]
#[cfg(test)]
mod cte_test;
#[cfg(test)]
mod main_test;
