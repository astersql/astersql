// Copyright 2026 AsterSQL.

// Cascades `base` crate 入口。
//
// 聚合哈希相等、任务栈、任务调度等基础抽象，并通过 `include!` 嵌入各实现文件。
// `util` 子模块提供字符串缓冲写入器；测试配置下挂载 base/hash_equaler 与迁移回归测试。

#![allow(non_snake_case)]

/// 字符串缓冲写入工具（对应 Go cascades/util 中的 StrBufferWriter 等）。
pub mod util {
    include!("../util/string_writer.rs");
}

/// Cascades 基础类型与接口：Hash64/Equals、Hasher、Stack/Task、Scheduler。
pub mod base {
    use crate::util;

    include!("base.rs");
    include!("hash_equaler.rs");
    include!("task_stack_base.rs");
    include!("task_scheduler_base.rs");

    #[cfg(test)]
    mod base_test {
        use super::*;
        include!("base_test.rs");
    }

    #[cfg(test)]
    mod hash_equaler_test {
        use super::*;
        include!("hash_equaler_test.rs");
    }

    #[cfg(test)]
    mod task_stack_base_test {
        use super::*;
        include!("task_stack_base_test.rs");
    }
}

pub use base::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
