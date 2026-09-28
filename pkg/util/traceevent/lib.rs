// Copyright 2026 AsterSQL.

// `traceevent` crate 入口：结构化追踪事件、飞行记录器与 client-go 适配。
//
// 对应 Go `pkg/util/traceevent`。对外重导出 `adapter` / `flightrecorder` /
// `traceevent` 子模块；测试通过 `#[path]` 挂到本 crate，与 Go 同目录测试对齐。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

extern crate self as astersql_util_traceevent;

/// client-go / TiKV 追踪控制适配层。
pub mod adapter;
/// 飞行记录器（Flight Recorder）：按触发条件决定是否保留并导出事件。
pub mod flightrecorder;
/// 追踪事件核心：类别位图、事件结构、模式开关与环形缓冲。
pub mod traceevent;

pub use adapter::*;
pub use flightrecorder::*;
pub use traceevent::*;

/// 测试互斥：飞行记录器/全局 sink 等有共享状态，用例串行执行。
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 获取全局测试锁；若锁被毒化则接管内容继续。
    pub fn test_guard() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 迁移对齐：adapter 相关 Aster 单元测试。
#[cfg(test)]
#[path = "adapter_1_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// adapter 单元测试。
#[cfg(test)]
#[path = "adapter_test.rs"]
mod adapter_test;

/// flightrecorder 配置编译与真值表单元测试。
#[cfg(test)]
#[path = "flightrecorder_test.rs"]
mod flightrecorder_test;

/// traceevent 模式、环形缓冲与事件记录单元测试。
#[cfg(test)]
#[path = "traceevent_test.rs"]
mod traceevent_test;
