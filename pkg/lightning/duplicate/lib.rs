// Copyright 2026 AsterSQL.

// Lightning 重复键检测（duplicate detection）子 crate 入口。
//
// 在物理导入路径上，通过外排（external sort）对编码后的键排序，
// 再并行扫描相邻相同用户键以发现唯一索引/主键冲突。本 crate 聚合
// `Detector`、内部键编解码与工作线程队列。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_mut,
    unused_variables
)]

/// 桩：再导出 Lightning 日志依赖，供本包内 detector/worker 使用。
pub mod lightning {
    pub mod log {
        pub use lightning_log_dependency::*;
    }
}

/// 桩：再导出编解码与外排依赖。
pub mod util {
    /// memcomparable 等字节编解码。
    pub mod codec {
        pub use codec_dependency::*;
    }
    /// 外部排序（external sorter）实现。
    pub mod extsort {
        pub use extsort_dependency::*;
    }
}

mod detector;
pub use detector::*;

mod internal;
pub use internal::*;

mod worker;
pub use worker::*;

#[cfg(test)]
#[path = "detector_test.rs"]
mod detector_test;

#[cfg(test)]
#[path = "internal_test.rs"]
mod internal_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "worker_test.rs"]
mod worker_test;
