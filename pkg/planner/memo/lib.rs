// Copyright 2026 AsterSQL.

// Memo（记忆化搜索空间）crate 根：Cascades 风格优化器的等价类与物理实现抽象。
//
// Memo 把逻辑上等价的表达式收进 Group，用 GroupExpr 连接算子与子 Group，
// 再用 Implementation 挂接带代价的物理计划；ExprIter 按 Pattern 枚举匹配表达式。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 按 Pattern 深度优先枚举 Group 内匹配的等价表达式。
mod expr_iterator;
/// Group：逻辑等价表达式集合及其属性、物理实现缓存。
mod group;
/// GroupExpr：子节点为 Group 的逻辑算子节点。
mod group_expr;
/// Implementation：带代价的物理实现 trait。
mod implementation;

/// 再导出表达式迭代器 API。
pub use expr_iterator::*;
/// 再导出 Group 与探索标记等 API。
pub use group::*;
/// 再导出 GroupExpr 构造与指纹 API。
pub use group_expr::*;
/// 再导出物理 Implementation trait。
pub use implementation::*;

#[cfg(test)]
/// ExprIter 与引擎类型过滤的单元测试。
mod expr_iterator_test;
#[cfg(test)]
/// GroupExpr 构造与指纹编码的单元测试。
mod group_expr_test;
#[cfg(test)]
/// Group 插入/删除/指纹/BuildKeyInfo 的单元测试。
mod group_test;
#[cfg(test)]
/// 测试入口（main/test harness）。
mod main_test;
#[cfg(test)]
/// AsterSQL 侧 memo 额外单元测试。
mod memo_aster_unit_test;
