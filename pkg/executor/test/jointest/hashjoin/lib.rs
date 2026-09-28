// Copyright 2026 AsterSQL.

// Hash Join 执行器专项测试 crate 入口。
//
// 对应 Go `pkg/executor/test/jointest/hashjoin`：覆盖 IndexHashJoin、
// HashJoin V1/V2、failpoint 注入、OOM/kill 与 explain analyze 统计。
// Hash Join 通过构建侧哈希表与探测侧探测完成等值连接。
// 本文件仅在 `#[cfg(test)]` 下挂接测试模块。

#![allow(dead_code)]

#[cfg(test)]
/// Hash Join 回归用例：Go 草稿归档与可执行 V1/V2 契约冒烟。
mod hash_join_test;
#[cfg(test)]
/// 包级 TestMain：AutoID、全局配置与 failpoint 策略。
mod main_test;
