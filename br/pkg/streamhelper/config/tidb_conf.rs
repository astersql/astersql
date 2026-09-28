// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! 嵌入 TiDB 进程时的推进器配置：多数字段委托 `CommandConfig`。
//!
//! 与 Go `tidb_conf.go` 对齐：唯一差异是 `GetCheckPointLagLimit` 读取
//! 全局原子量（Go 的 `vardef.AdvancerCheckPointLagLimit`），可由系统变量热更新。

use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_sessionctx_vardef::AdvancerCheckPointLagLimit;

#[cfg(test)]
use std::sync::Mutex;

use crate::command_conf::{CommandConfig, defaultCommandConfig};
use crate::types::Config;

/// Compatibility view over the process-wide vardef value, expressed as nanoseconds.
///
/// Existing streamhelper tests use Rust atomic-style `load`/`store` calls; both methods
/// deliberately delegate to the same vardef atomic updated by the system-variable path.
pub struct AdvancerCheckPointLagLimitNanosValue;

impl AdvancerCheckPointLagLimitNanosValue {
    pub fn load(&self, _ordering: Ordering) -> u64 {
        AdvancerCheckPointLagLimit.Load() as u64
    }

    pub fn store(&self, value: u64, _ordering: Ordering) {
        AdvancerCheckPointLagLimit.Store(value as i64);
    }
}

pub static AdvancerCheckPointLagLimitNanos: AdvancerCheckPointLagLimitNanosValue =
    AdvancerCheckPointLagLimitNanosValue;

/// Serializes tests that emulate Go's global sysvar setter.
#[cfg(test)]
pub(crate) static ADVANCER_LAG_LIMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// TiDB 内嵌配置：内含一份 CommandConfig，并覆盖滞后上限读取路径。
#[derive(Clone, Debug)]
pub struct TiDBConfig {
    pub CommandConfig: CommandConfig,
}

/// 构造默认 TiDB 配置（内嵌默认 CommandConfig）。
pub fn DefaultTiDBConfig() -> TiDBConfig {
    TiDBConfig {
        CommandConfig: defaultCommandConfig(),
    }
}

impl Config for TiDBConfig {
    fn GetBackoffTime(&self) -> Duration {
        self.CommandConfig.GetBackoffTime()
    }
    fn TickTimeout(&self) -> Duration {
        self.CommandConfig.TickTimeout()
    }
    fn GetDefaultStartPollThreshold(&self) -> Duration {
        self.CommandConfig.GetDefaultStartPollThreshold()
    }
    fn GetSubscriberErrorStartPollThreshold(&self) -> Duration {
        self.CommandConfig.GetSubscriberErrorStartPollThreshold()
    }
    fn GetResolveLockInterval(&self) -> Duration {
        self.CommandConfig.GetResolveLockInterval()
    }
    /// 从 vardef 的进程级全局原子量读取，不走内嵌 CommandConfig 的静态字段。
    fn GetCheckPointLagLimit(&self) -> Duration {
        Duration::from_nanos(AdvancerCheckPointLagLimit.Load() as u64)
    }
}
