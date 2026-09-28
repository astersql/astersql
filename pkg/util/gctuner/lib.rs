// Copyright 2026 AsterSQL.

// GC（Garbage Collection，垃圾回收）调谐器包入口。
//
// 对应 Go `util/gctuner`：根据堆占用动态调节 GOGC（Go 运行时 GC 触发比例），
// 以及按服务器内存上限调节 runtime memory limit，避免 OOM 同时控制 GC 频率。
//
// 子模块：
// - `finalizer`：运行时周期回调并请求系统分配器回收，驱动调谐循环
// - `mem`：读取系统分配器堆占用（不支持时回退 RSS）
// - `memory_limit_tuner`：按 `ServerMemoryLimit * percentage` 设置内存上限
// - `tuner`：按阈值与占用比例计算并设置 GOGC

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

/// Finalizer 周期回调实现。
pub mod finalizer;
/// 进程内存占用探测。
pub mod mem;
/// 运行时 memory limit 调谐。
pub mod memory_limit_tuner;
/// GOGC 百分比调谐。
pub mod tuner;

/// 迁移期聚合回归测试（嵌入式 include）。
#[cfg(test)]
mod migration_aster_unit_test {
    include!("migration_aster_unit_test.rs");
}

#[cfg(test)]
mod finalizer_test;
#[cfg(test)]
mod mem_test;
#[cfg(test)]
mod memory_limit_tuner_test;
#[cfg(test)]
mod tuner_test;
