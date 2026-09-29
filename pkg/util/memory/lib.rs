// Copyright 2026 AsterSQL.

// `pkg/util/memory` crate 根：聚合内存追踪、资源池、仲裁器与系统内存探针子模块。
//
// 对应 Go `pkg/util/memory`；
// `utils` 以内联 `include!` 并导出哈希/配额常量；测试用 `#[path]` 挂接各 `*_test.rs`。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 日志工具再导出，供本包内调用方使用统一 log 门面。
pub mod logutil {
    pub use logutil_crate::log;
}
/// SQLKiller 再导出：内存超限时可向会话注入 kill 信号。
pub mod sqlkiller {
    pub use sqlkiller_crate::sqlkiller::*;
}
/// 超限动作（OOM action）实现。
pub mod action;
/// 内存仲裁器（arbitrator）核心。
pub mod arbitrator;
pub use arbitrator::*;
/// 全局内存仲裁器与软限制解析。
pub mod global_arbitrator;
/// Threshold-triggered heap profile capture and retention.
pub mod heap_profile;
/// 主机/cgroup 内存总量与用量探针。
pub mod meminfo;
/// 进程堆内存统计缓存。
pub mod memstats;
/// 资源池与 Budget 配额树。
pub mod pool;
/// 查询执行期 Tracker 树与配额动作。
pub mod tracker;
pub use astersql_errno::errcode as errno;
/// 工具函数与配额哈希常量；`include!` 注入 `utils.rs` 正文。
pub mod utils {
    include!("utils.rs");
    const prime64: u64 = 1_099_511_628_211;
    const initHashKey: u64 = 14_695_981_039_346_656_037;
    pub(crate) const baseQuotaUnit: i64 = 4 * byteSizeKB;
}
pub(crate) use utils::*;

// 以下为 #[path] 挂接的单元测试模块，文件名保留 Aster 任务编号后缀。
#[cfg(test)]
#[path = "action_1_aster_unit_test.rs"]
mod action_aster_unit_test;
#[cfg(test)]
#[path = "arbitrator_2_aster_unit_test.rs"]
mod arbitrator_aster_unit_test;
#[cfg(test)]
#[path = "arbitrator_test.rs"]
mod arbitrator_test;
#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "global_arbitrator_3_aster_unit_test.rs"]
mod global_arbitrator_aster_unit_test;
#[cfg(test)]
#[path = "global_arbitrator_test.rs"]
mod global_arbitrator_test;
#[cfg(test)]
#[path = "heap_profile_test.rs"]
mod heap_profile_test;
#[cfg(test)]
#[path = "meminfo_test.rs"]
mod meminfo_test;
#[cfg(test)]
#[path = "memstats_test.rs"]
mod memstats_test;
#[cfg(test)]
#[path = "pool_test.rs"]
mod pool_test;
#[cfg(test)]
#[path = "tracker_4_aster_unit_test.rs"]
mod tracker_aster_unit_test;
#[cfg(test)]
#[path = "tracker_test.rs"]
mod tracker_test;
#[cfg(test)]
#[path = "utils_5_aster_unit_test.rs"]
mod utils_aster_unit_test;
#[cfg(test)]
#[path = "utils_test.rs"]
mod utils_test;

#[cfg(test)]
#[path = "go_merge_30_test.rs"]
mod go_merge_30_test;
