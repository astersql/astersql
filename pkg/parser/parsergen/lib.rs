// Copyright 2026 AsterSQL.

//! 解析器生成器的统一入口。
//!
//! 该模块串联 `.astergram` 文法解析、LALR(1) 自动机构造、action/goto 分析表生成，
//! 以及自包含 Rust 解析器数据的确定性渲染。语义动作由生成数据的使用方实现，不在此处生成。

/// `.astergram` 文法的 LALR(1) 自动机及 FIRST/nullable 集合计算。
pub mod automaton;
/// 兼容解析表产物的确定性生成、写入与防漂移检查。
pub mod generate;
/// `.astergram` 文法模型、词法解析与合法性校验。
pub mod grammar;
/// 解析器数据的 Rust 源码渲染与表驱动行为跟踪。
pub mod render;
/// action/goto 分析表的冲突处理和稀疏整数编码。
pub mod table;

// 统一重导出各阶段的公开 API，使调用方可以从 parsergen 门面完成整条生成流水线。
pub use automaton::*;
pub use generate::*;
pub use grammar::*;
pub use render::*;
pub use table::*;

#[cfg(test)]
#[path = "grammar_aster_unit_test.rs"]
mod grammar_aster_unit_test;

#[cfg(test)]
mod grammar_test;

#[cfg(test)]
#[path = "automaton_aster_unit_test.rs"]
mod automaton_aster_unit_test;

#[cfg(test)]
#[path = "table_aster_unit_test.rs"]
mod table_aster_unit_test;

#[cfg(test)]
#[path = "render_aster_unit_test.rs"]
mod render_aster_unit_test;

#[cfg(test)]
mod render_test;

#[cfg(test)]
#[path = "generate_aster_unit_test.rs"]
mod generate_aster_unit_test;
