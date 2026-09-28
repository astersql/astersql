// Copyright 2026 AsterSQL.

// Multi-Valued Index（多值索引，MV Index）DDL 测试 crate 入口。
//
// 多值索引对 JSON/数组列展开后的每个元素建索引；对应 Go
// `pkg/ddl/tests/multivaluedindex`。聚合 `main_test` 与
// `multi_valued_index_test`。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "multi_valued_index_test.rs"]
mod multi_valued_index_test;
