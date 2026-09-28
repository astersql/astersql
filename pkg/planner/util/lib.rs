// Copyright 2026 AsterSQL.

// 规划器（planner）通用工具 crate 根模块。
//
// 汇总执行计划构建与优化阶段常用的辅助类型：
// - `byitem` / `column` / `expression`：ORDER BY、列与表达式相关工具；
// - `handle_cols`：行句柄（Handle）列抽象，用于定位表行；
// - `path`：访问路径（AccessPath），描述索引/表扫描候选；
// - `null_misc`：外连接等场景的 NULL 拒绝证明；
// - `misc` / `funcdep_misc` / `explain_misc`：杂项克隆、函数依赖与 explain 辅助。
//
// 本 crate 不直接产出最终物理计划，而是供 `planner/core` 等上层模块复用。

#![allow(non_snake_case, non_upper_case_globals)]

/// ORDER BY / GROUP BY 单项包装。
mod byitem;
/// 列相关规划辅助。
mod column;
/// Explain 展示相关杂项。
mod explain_misc;
/// 带 PlanContext 的 AST 表达式求值/改写回调。
mod expression;
/// 函数依赖（functional dependency）杂项。
mod funcdep_misc;
/// 行句柄列（HandleCols）抽象与整型/公共句柄实现。
mod handle_cols;
/// 递归切片扁平化、克隆与隔离读路径过滤等杂项。
mod misc;
/// NULL 拒绝（null-reject）证明：判断谓词在内表列上是否恒非真。
mod null_misc;
/// NULL 拒绝证明所用 builtin 函数属性表。
mod null_misc_builtins;
/// 访问路径 AccessPath 与列前缀长度比较。
mod path;

/// 再导出 ORDER BY/GROUP BY 项。
pub use byitem::*;
/// 再导出列辅助 API。
pub use column::*;
/// 再导出 explain 杂项。
pub use explain_misc::*;
/// 再导出表达式回调 API。
pub use expression::*;
/// 再导出函数依赖杂项。
pub use funcdep_misc::*;
/// 再导出 HandleCols API。
pub use handle_cols::*;
/// 再导出 misc 杂项 API。
pub use misc::*;
/// 再导出 NULL 拒绝证明 API。
pub use null_misc::*;
/// 再导出 AccessPath 等路径 API。
pub use path::*;

#[cfg(test)]
#[path = "column_test.rs"]
/// 列相关单元测试。
mod column_test;
#[cfg(test)]
#[path = "funcdep_misc_test.rs"]
/// 函数依赖杂项单元测试。
mod funcdep_misc_test;
#[cfg(test)]
#[path = "handle_cols_test.rs"]
/// 行句柄列 Go/Rust 一致性测试。
mod handle_cols_test;
#[cfg(test)]
#[path = "main_test.rs"]
/// 包级测试入口与公共初始化。
mod main_test;
#[cfg(test)]
#[path = "misc_test.rs"]
/// 杂项工具 Go/Rust 一致性测试。
mod misc_test;
#[cfg(test)]
#[path = "null_misc_test.rs"]
/// NULL 拒绝证明单元测试。
mod null_misc_test;
#[cfg(test)]
#[path = "path_test.rs"]
/// AccessPath 相关单元测试。
mod path_test;
#[cfg(test)]
#[path = "slice_recursive_flatten_iter_test.rs"]
/// 递归切片扁平化迭代器单元测试。
mod slice_recursive_flatten_iter_test;
