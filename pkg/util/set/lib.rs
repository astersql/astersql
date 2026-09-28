// Copyright 2026 AsterSQL.

// `set` crate 入口：导出各类集合实现并挂接单元测试模块。
//
// 对应 Go `pkg/util/set`。子模块覆盖 float64/int/string 集合、泛型 Set
// 代数运算，以及带内存用量追踪的变体；`hack`/`memory`/`types` 为依赖重导出。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 重导出 MemAwareMap，供带内存追踪的集合实现使用。
pub mod hack {
    pub use hack_crate::map_abi::{MemAwareMap, NewMemAwareMap};
}

/// 重导出内存 Tracker，用于统计集合占用字节。
pub mod memory {
    pub use memory_crate::tracker::{NewTracker, Tracker};
}

/// 重导出 MyDecimal，供 string→decimal 映射使用。
pub mod types {
    pub use types_crate::decimal::mydecimal::MyDecimal;
}

#[path = "float64_set.rs"]
pub mod float64_set;
#[path = "int_set.rs"]
pub mod int_set;
#[path = "set.rs"]
pub mod set;
#[path = "set_with_memory_usage.rs"]
pub mod set_with_memory_usage;
#[path = "string_set.rs"]
pub mod string_set;

pub use float64_set::*;
pub use int_set::*;
pub use set::*;
pub use set_with_memory_usage::*;
pub use string_set::*;

#[cfg(test)]
mod float64_set_test;
#[cfg(test)]
mod int_set_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod set_test;
#[cfg(test)]
mod set_with_memory_usage_test;
#[cfg(test)]
mod string_set_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
