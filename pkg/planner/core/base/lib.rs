// Copyright 2026 AsterSQL.

// 规划器 base 抽象层入口。
//
// 汇总逻辑/物理计划、优化规则、任务（Task）与杂项访问接口，并向外再导出。
// 本层只定义与 Go `planner/core/base` 对齐的对象安全边界，不绑定具体算子实现，
// 以便 operator、cascades 等包共享同一套执行计划抽象。

#![allow(non_snake_case)]

/// 表达式求值错误类型别名，与 expression 包对齐。
pub type Error = expression::Error;

/// 字段名与 NameSlice 等元数据类型再导出。
pub mod types {
    pub use types_dependency::metadata::{FieldName, NameSlice};
}

/// 扩展 base 接口时的约束说明（文档模块）。
mod doc;
/// 计划、规划上下文与 JoinType 等核心抽象。
mod plan_base;
/// 再导出 plan_base 中的公共类型与 trait。
pub use plan_base::*;

/// 访问对象、谓词提取器与数据访问者等杂项接口。
mod misc_base;
/// 再导出 misc_base 中的公共 trait。
pub use misc_base::*;

/// 逻辑优化规则（LogicalOptRule）抽象。
mod rule_base;
/// 再导出规则接口。
pub use rule_base::*;

/// 物理任务（Task / MPPSink）抽象。
mod task_base;
/// 再导出任务相关接口与无效任务槽位。
pub use task_base::*;

/// base 包契约测试：内置函数计数、JoinType 判别值与 PossibleProperties 哈希。
#[cfg(test)]
#[path = "base_test.rs"]
mod base_test;

/// `plan_base` 的 Go/Rust 哈希编码一致性回归测试。
#[cfg(test)]
#[path = "plan_base_test.rs"]
mod plan_base_test;

/// 杂项接口契约测试：内存表谓词提取器保留 NameSlice 可空字段语义。
#[cfg(test)]
#[path = "misc_base_test.rs"]
mod misc_base_test;
