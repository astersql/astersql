// Copyright 2026 AsterSQL.

// 正式 crate 路径下的 column 单元测试入口。
//
// 使用正式 crate 名 `astersql_util_stmtsummary_v2`，
// 再 `include!` 共享的 `column_1_aster_unit_test.rs`，使同一套用例可在
// workspace 迁移别名与正式包名两套路径下运行。

include!("column_1_aster_unit_test.rs");
