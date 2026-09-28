// Copyright 2026 AsterSQL.

// infoschemav2test 测试 crate 的库入口。
//
// 对应 Go 的 `pkg/infoschema/test/infoschemav2test` 包。实际用例在 `v2_test.rs`
// 与 `main_test.rs` 中；本文件仅作为 crate 根，并放宽未使用项的编译告警。
//
// InfoSchema：会话可见的库表等元数据的内存视图；V2 表示按 schema 版本组织的新实现。

#![allow(dead_code)]
