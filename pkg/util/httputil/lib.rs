// Copyright 2026 AsterSQL.

// HTTP 工具包入口：提供带超时的客户端与 JSON/文本 GET 辅助函数。
//
// 对应 Go `pkg/util/httputil`。再导出 `http` 模块中的 `NewClient`、`GetJSON`、
// `GetText` 等 API，供运维接口、Lightning 控制面等拉取远程配置或状态。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// HTTP 客户端与请求辅助实现。
pub mod http;
pub use http::*;

#[cfg(test)]
#[path = "http_test.rs"]
mod http_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
