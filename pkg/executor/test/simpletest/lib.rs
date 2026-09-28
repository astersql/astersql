// Copyright 2026 AsterSQL.

// 简单执行器（simple executor）测试 crate 入口。
//
// 对应 Go `pkg/executor/test/simpletest` 包。覆盖会话启动顺序、事务提交/
// 回滚等基础执行路径的冒烟与语义用例。

#![allow(dead_code)]

/// 事务状态机：成功提交与失败回滚路径的最小可执行用例。
#[cfg(test)]
mod simple_test;
