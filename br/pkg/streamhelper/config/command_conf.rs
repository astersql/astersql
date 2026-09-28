// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! BR 命令行路径的检查点推进器配置，实现 `Config` trait。
//!
//! 对齐 Go `command_conf.go`：用 `FlagSet`（HashMap 桩）代替 pflag，
//! 提供默认间隔与从 flags 填充字段；订阅错误时的轮询阈值按 0.45× 折算。

use std::collections::HashMap;
use std::time::Duration;

use crate::types::Config;

/// CLI flag 名：两次重试之间的退避间隔。
pub const flagBackoffTime: &str = "backoff-time";
/// CLI flag 名：推进 tick 周期。
pub const flagTickInterval: &str = "tick-interval";
/// CLI flag 名：落后超过该阈值才主动向 TiKV 轮询检查点。
pub const flagTryAdvanceThreshold: &str = "try-advance-threshold";
/// CLI flag 名：可容忍的最大检查点滞后。
pub const flagCheckPointLagLimit: &str = "check-point-lag-limit";
/// CLI flag 名：owner 主动轮转周期（混沌测试用，默认禁用）。
pub const flagOwnershipCycleInterval: &str = "ownership-cycle-interval";

/// 默认：落后 ≥4 分钟才开始 poll TiKV。
pub const DefaultTryAdvanceThreshold: Duration = Duration::from_secs(4 * 60);
/// 默认：检查点滞后上限 48 小时。
pub const DefaultCheckPointLagLimit: Duration = Duration::from_secs(48 * 3600);
/// 默认：重试退避 5 秒。
pub const DefaultBackOffTime: Duration = Duration::from_secs(5);
/// 默认：每 12 秒触发一次 tick。
pub const DefaultTickInterval: Duration = Duration::from_secs(12);
/// 默认：不轮转 ownership（0 表示关闭）。
pub const DefaultOwnershipCycleInterval: Duration = Duration::from_secs(0);

/// FlagSet stand-in for pflag.FlagSet duration lookups.
/// 轻量桩：仅保存 duration 类 flag，供 `GetFromFlags` 读取。
pub type FlagSet = HashMap<&'static str, Duration>;

/// 向 FlagSet 注册推进器相关 duration 默认值（对齐 Go `DefineFlags...`）。
pub fn DefineFlagsForCheckpointAdvancerConfig(f: &mut FlagSet) {
    f.insert(flagBackoffTime, DefaultBackOffTime);
    f.insert(flagTickInterval, DefaultTickInterval);
    f.insert(flagTryAdvanceThreshold, DefaultTryAdvanceThreshold);
    f.insert(flagCheckPointLagLimit, DefaultCheckPointLagLimit);
    f.insert(flagOwnershipCycleInterval, DefaultOwnershipCycleInterval);
}

/// 命令行驱动的推进器配置；`OwnershipCycleInterval` 仅建议混沌测试启用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandConfig {
    pub BackoffTime: Duration,
    pub TickDuration: Duration,
    pub TryAdvanceThreshold: Duration,
    pub CheckPointLagLimit: Duration,
    pub OwnershipCycleInterval: Duration,
}

/// 内部默认构造，字段均取本文件常量。
pub fn defaultCommandConfig() -> CommandConfig {
    CommandConfig {
        BackoffTime: DefaultBackOffTime,
        TickDuration: DefaultTickInterval,
        TryAdvanceThreshold: DefaultTryAdvanceThreshold,
        CheckPointLagLimit: DefaultCheckPointLagLimit,
        OwnershipCycleInterval: DefaultOwnershipCycleInterval,
    }
}

/// 公开入口：返回默认命令行配置实例。
pub fn DefaultCommandConfig() -> CommandConfig {
    defaultCommandConfig()
}

impl CommandConfig {
    /// 从 FlagSet 覆盖全部 duration 字段；缺 flag 则返回错误字符串。
    pub fn GetFromFlags(&mut self, f: &FlagSet) -> Result<(), String> {
        self.BackoffTime = *f.get(flagBackoffTime).ok_or("missing backoff-time")?;
        self.TickDuration = *f.get(flagTickInterval).ok_or("missing tick-interval")?;
        self.TryAdvanceThreshold = *f
            .get(flagTryAdvanceThreshold)
            .ok_or("missing try-advance-threshold")?;
        self.CheckPointLagLimit = *f
            .get(flagCheckPointLagLimit)
            .ok_or("missing check-point-lag-limit")?;
        self.OwnershipCycleInterval = *f
            .get(flagOwnershipCycleInterval)
            .ok_or("missing ownership-cycle-interval")?;
        Ok(())
    }
}

impl Config for CommandConfig {
    /// 订阅正常时开始 poll 的阈值，即 `TryAdvanceThreshold`。
    fn GetDefaultStartPollThreshold(&self) -> Duration {
        self.TryAdvanceThreshold
    }
    fn GetCheckPointLagLimit(&self) -> Duration {
        self.CheckPointLagLimit
    }
    /// 订阅出错时的 poll 阈值：约为原阈值的 0.45×（9/20），更积极轮询。
    fn GetSubscriberErrorStartPollThreshold(&self) -> Duration {
        self.TryAdvanceThreshold * 9 / 20
    }
    /// 检查点长时间不变时，每隔约两倍 tick 尝试 resolve lock。
    fn GetResolveLockInterval(&self) -> Duration {
        self.TickDuration * 2
    }
    fn TickTimeout(&self) -> Duration {
        self.TickDuration
    }
    fn GetBackoffTime(&self) -> Duration {
        self.BackoffTime
    }
}
