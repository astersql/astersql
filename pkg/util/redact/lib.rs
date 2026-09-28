// Copyright 2026 AsterSQL.

// 日志与配置脱敏（redact）工具库入口。
//
// 重新导出 `redact` 模块中的脱敏 API；测试下提供全局互斥锁，避免并行用例
// 同时改写全局脱敏开关互相干扰。

pub mod redact;
pub use redact::*;

/// 测试用互斥锁：串行化依赖全局 `InitRedact` 状态的用例。
#[cfg(test)]
pub(crate) static REDACT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "redact_test.rs"]
mod redact_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
