// Copyright 2026 AsterSQL.

// 正式挂载的 security 迁移单元测试入口。
//
// 使用正式 crate 名 `astersql_util`，
// 再通过 `include!` 引入 `security_2_aster_unit_test.rs` 中的全部用例。

include!("security_2_aster_unit_test.rs");
