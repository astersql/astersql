// Copyright 2026 AsterSQL.

// JSON Path 表达式解析门面。
//
// 通过 `include!` 引入 `json_path_expr.rs`，提供路径腿（key / 数组下标 /
// `**`）解析与匹配，供 `JSON_EXTRACT` 等函数定位文档节点。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

include!("../../json_path_expr.rs");
