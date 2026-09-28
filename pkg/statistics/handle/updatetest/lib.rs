// Copyright 2026 AsterSQL.

// 统计增量更新（stats update / delta）集成测试 crate 入口。
//
// 在 `#[cfg(test)]` 下挂载会话搭建（`main_test`）与 Go 同路径更新用例（`update_test`），
// 验证插入/删除/事务回滚等操作后，统计元数据（`stats_meta`）与实时行数是否正确刷盘。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
/// 测试公共夹具：MockStore、Domain 与 TestKit 会话生命周期。
mod main_test;

#[cfg(test)]
#[path = "update_test.rs"]
/// Go `update_test.go` 同路径用例：增量刷盘、自动分析、列使用追踪等。
mod update_test;
