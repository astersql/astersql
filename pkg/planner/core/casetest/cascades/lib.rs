// Copyright 2026 AsterSQL.

// `cascades` casetest crate 入口。
//
// Cascades 是基于 memo（等价类组）的优化器框架：逻辑计划被装入 Group/GroupExpression，
// 再在组内做变换与代价选择。本 crate 挂载 memo 相关用例与 TestMain 初始化语义。

#![allow(dead_code)]

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

/// Memo 初始化、逻辑属性派生与 cascades SQL 语法面测试。
#[cfg(test)]
#[path = "memo_test.rs"]
mod memo_test;
