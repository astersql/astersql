// Copyright 2026 AsterSQL.

// Timer 运行时 crate 入口。
//
// 组织定时器缓存、分组运行时与 Hook Worker，并在测试配置下挂载对应测试模块。

#![allow(non_snake_case, non_upper_case_globals)]
/// 测试用正式 crate 自引用别名。
extern crate self as astersql_timer_runtime;
/// 再导出 `timer_api`，供运行时内部统一使用定时器 API。
pub mod api {
    pub use timer_api::*;
}
/// 定时器内存缓存：维护触发顺序与处理状态。
pub mod cache;
/// 分组运行时主循环：刷新、监听、触发与收尾。
pub mod runtime;
/// Hook Worker：异步执行调度前后回调。
pub mod worker;

#[cfg(test)]
#[path = "cache_test.rs"]
/// 缓存单元测试。
mod cache_test;
#[cfg(test)]
/// 供其他测试模块引用缓存测试辅助。
pub(crate) use cache_test as runtime_cache_test;

#[cfg(test)]
#[path = "main_test.rs"]
/// 运行时测试公共夹具与 mock。
mod main_test;
#[cfg(test)]
/// 供其他测试模块引用公共测试工具。
pub(crate) use main_test as runtime_main_test;

#[cfg(test)]
#[path = "cache_1_aster_unit_test.rs"]
/// AsterSQL 补充的缓存单元测试。
mod cache_1_aster_unit_test;

#[cfg(test)]
#[path = "runtime_test.rs"]
/// 运行时主循环与触发逻辑测试。
mod runtime_test;

#[cfg(test)]
#[path = "worker_test.rs"]
/// Hook Worker 行为测试。
mod worker_test;
