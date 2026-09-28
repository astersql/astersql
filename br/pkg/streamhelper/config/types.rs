// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! 流式辅助（streamhelper）推进器配置抽象；与 Go 侧配置接口对齐。

use std::sync::atomic::AtomicI32;
use std::time::Duration;

/// 默认最大并发 advance 上限，防止打爆 PD/TiKV。
/// Go 将它声明为包级变量；使用原子整数保留运行时可调整语义并避免数据竞争。
pub static DefaultMaxConcurrencyAdvance: AtomicI32 = AtomicI32::new(8);

/// 推进循环所需的超时与阈值配置；实现可来自命令行或默认值。
pub trait Config {
    fn GetBackoffTime(&self) -> Duration;
    fn TickTimeout(&self) -> Duration;
    fn GetDefaultStartPollThreshold(&self) -> Duration;
    fn GetSubscriberErrorStartPollThreshold(&self) -> Duration;
    fn GetResolveLockInterval(&self) -> Duration;
    fn GetCheckPointLagLimit(&self) -> Duration;
}
