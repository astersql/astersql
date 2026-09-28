// Copyright 2026 AsterSQL.

// Import Into task-key 子 crate 入口。
//
// 聚合内核类型、keyspace 配置、任务类型常量与 `taskkey` 实现，
// 供构造 DXF（分布式执行框架）中 Import Into 作业的唯一路径键。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_dxf_importinto_taskkey;

/// 再导出内核类型判定（如 `IsNextGen`）。
pub mod kerneltype {
    pub use ::kerneltype::*;
}

/// 测试/本地用的全局 keyspace 名称配置。
pub mod config {
    use std::sync::{OnceLock, RwLock};
    /// 惰性初始化的全局 keyspace 名称存储。
    fn name() -> &'static RwLock<String> {
        static NAME: OnceLock<RwLock<String>> = OnceLock::new();
        NAME.get_or_init(|| RwLock::new(String::new()))
    }
    /// 读取当前配置的全局 keyspace 名称。
    pub fn get_global_keyspace_name() -> String {
        name().read().unwrap().clone()
    }
    /// 写入全局 keyspace 名称（测试中用于模拟 NextGen 配置）。
    pub fn set_global_keyspace_name(value: &str) {
        *name().write().unwrap() = value.to_owned();
    }
}
/// 从 settings 读取 keyspace 名称的薄封装。
pub mod keyspace {
    /// 对应 Go 的 `GetKeyspaceNameBySettings`，此处转发到本地 config。
    pub fn GetKeyspaceNameBySettings() -> String {
        crate::config::get_global_keyspace_name()
    }
}
/// 协议侧任务类型名常量。
pub mod proto {
    /// Import Into 任务类型字符串，出现在 task key 路径中。
    pub const ImportInto: &str = "ImportInto";
}
/// 任务类型别名占位，与框架侧 `TaskType` 对齐。
pub mod task {
    /// 静态任务类型字符串别名。
    pub type TaskType = &'static str;
}
/// task key 构造实现（见 `task_key.rs`）。
#[path = "task_key.rs"]
pub mod taskkey;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
