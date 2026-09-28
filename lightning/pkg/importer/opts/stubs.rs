// Copyright 2026 AsterSQL.
//! Local stand-ins for `pkg/lightning/mydump` option types (arm64-safe).
//!
//! Go stores `mydump.MDLoaderSetupOption` func values and `slices.Clone`s them.
//! The real mydump port uses `Box<dyn FnOnce + Send>` which is not `Clone`, so
//! this crate keeps an `Arc`-based stand-in that preserves copy/clone semantics.
//! 也就是说，这里的核心目标不是复刻完整 mydump，
//! 而是保留“option 可复制、可重复应用”的行为契约，
//! 让 opts 包测试能继续验证与 Go 一致的切片复制和闭包叠加语义。

use std::sync::Arc;

/// Minimal config mutated by [`MDLoaderSetupOption`] (mirrors mydump shape).
/// 这里只放当前测试会观察到的字段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MDLoaderSetupConfig {
    pub max_scan_files: usize,
    pub scan_file_concurrency: usize,
    pub skip_real_size_estimation: bool,
    pub support_partial_result: bool,
}

/// Corresponds to Go `mydump.MDLoaderSetupOption`.
/// `Arc` 让这些 option 像 Go 函数值一样可以放进切片后再克隆。
pub type MDLoaderSetupOption = Arc<dyn Fn(&mut MDLoaderSetupConfig) + Send + Sync>;

/// Corresponds to Go `mydump.WithScanFileConcurrency`.
/// 非正值不覆盖已有配置；`isize` 对应 Go `int` 的平台字长与有符号边界。
pub fn WithScanFileConcurrency(concurrency: isize) -> MDLoaderSetupOption {
    Arc::new(move |c: &mut MDLoaderSetupConfig| {
        if concurrency > 0 {
            c.scan_file_concurrency = concurrency as usize;
        }
    })
}

/// Corresponds to Go `mydump.WithMaxScanFiles`.
/// 只有传入正数时才启用限制，并顺带打开 partial result 支持。
pub fn WithMaxScanFiles(max_scan_files: isize) -> MDLoaderSetupOption {
    Arc::new(move |c: &mut MDLoaderSetupConfig| {
        if max_scan_files > 0 {
            c.max_scan_files = max_scan_files as usize;
            c.support_partial_result = true;
        }
    })
}

/// Corresponds to Go `mydump.WithSkipRealSizeEstimation`.
/// 该开关只改一个布尔位，不影响其他 loader 参数。
pub fn WithSkipRealSizeEstimation(skip: bool) -> MDLoaderSetupOption {
    Arc::new(move |c: &mut MDLoaderSetupConfig| {
        c.skip_real_size_estimation = skip;
    })
}

/// Corresponds to Go `mydump.ReturnPartialResultOnError`.
/// 该 option 显式控制遇错时是否允许返回部分结果。
pub fn ReturnPartialResultOnError(support_partial_result: bool) -> MDLoaderSetupOption {
    Arc::new(move |c: &mut MDLoaderSetupConfig| {
        c.support_partial_result = support_partial_result;
    })
}

pub mod mydump {
    /// 重新导出这些符号，便于调用方以接近真实包路径的方式引用。
    pub use super::{
        MDLoaderSetupConfig, MDLoaderSetupOption, ReturnPartialResultOnError, WithMaxScanFiles,
        WithScanFileConcurrency, WithSkipRealSizeEstimation,
    };
}
