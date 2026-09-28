// Copyright 2026 AsterSQL.

//! Go/Rust 公开配置契约一致性检查（非 Go 同名测试文件的机械移植）。
//!
//! 覆盖默认 CommandConfig 各 `Config` 方法、FlagSet 回填、TiDB 全局滞后原子量，
//! 以及缺 flag 时 `GetFromFlags` 的错误路径。

use std::time::Duration;

use crate::{
    AdvancerCheckPointLagLimitNanos, Config, DefaultBackOffTime, DefaultCheckPointLagLimit,
    DefaultCommandConfig, DefaultMaxConcurrencyAdvance, DefaultTiDBConfig,
    DefaultTryAdvanceThreshold, DefineFlagsForCheckpointAdvancerConfig, flagBackoffTime,
    flagCheckPointLagLimit, flagOwnershipCycleInterval, flagTickInterval, flagTryAdvanceThreshold,
    tidb_conf::ADVANCER_LAG_LIMIT_TEST_LOCK,
};
use std::sync::atomic::Ordering;

/// 核对默认值、0.45× 订阅错误阈值、Flag 填充与 TiDB 全局覆盖语义。
#[test]
fn go_rust_public_contract_matches() {
    let _guard = ADVANCER_LAG_LIMIT_TEST_LOCK.lock().unwrap();
    let conf = DefaultCommandConfig();
    // 与 Go CommandConfig 默认实现一一对应。
    assert_eq!(conf.GetBackoffTime(), DefaultBackOffTime);
    assert_eq!(conf.TickTimeout(), conf.TickDuration);
    assert_eq!(
        conf.GetDefaultStartPollThreshold(),
        DefaultTryAdvanceThreshold
    );
    // 订阅出错时阈值为 TryAdvanceThreshold * 9/20。
    assert_eq!(
        conf.GetSubscriberErrorStartPollThreshold(),
        DefaultTryAdvanceThreshold * 9 / 20
    );
    assert_eq!(conf.GetResolveLockInterval(), conf.TickDuration * 2);
    assert_eq!(conf.GetCheckPointLagLimit(), DefaultCheckPointLagLimit);

    let mut flags: crate::FlagSet = Default::default();
    DefineFlagsForCheckpointAdvancerConfig(&mut flags);
    let mut c2 = DefaultCommandConfig();
    // 用默认 FlagSet 回填后字段应等于常量默认值。
    c2.GetFromFlags(&flags).unwrap();
    assert_eq!(c2.BackoffTime, DefaultBackOffTime);

    // TiDBConfig 从全局原子量读取滞后上限；测完恢复默认以免串扰。
    AdvancerCheckPointLagLimitNanos
        .store(Duration::from_secs(10).as_nanos() as u64, Ordering::SeqCst);
    let tidb = DefaultTiDBConfig();
    assert_eq!(tidb.GetCheckPointLagLimit(), Duration::from_secs(10));
    // restore default for other tests
    AdvancerCheckPointLagLimitNanos.store(
        DefaultCheckPointLagLimit.as_nanos() as u64,
        Ordering::SeqCst,
    );

    // error: missing flag — 空 FlagSet 必须报错。
    assert!(c2.GetFromFlags(&Default::default()).is_err());
}

/// Go exposes `DefaultMaxConcurrencyAdvance` as a mutable package variable and
/// `CommandConfig` is a value-comparable struct because all of its fields are comparable.
#[test]
fn go_mutable_concurrency_default_and_value_equality_match() {
    let original = DefaultMaxConcurrencyAdvance.load(Ordering::SeqCst);
    DefaultMaxConcurrencyAdvance.store(13, Ordering::SeqCst);
    assert_eq!(DefaultMaxConcurrencyAdvance.load(Ordering::SeqCst), 13);
    DefaultMaxConcurrencyAdvance.store(original, Ordering::SeqCst);

    assert_eq!(DefaultCommandConfig(), DefaultCommandConfig());
}

/// Go assigns flags in declaration order and returns immediately at the first lookup error.
#[test]
fn get_from_flags_preserves_values_and_first_error_side_effects() {
    let mut flags: crate::FlagSet = Default::default();
    flags.insert(flagBackoffTime, Duration::from_secs(1));
    flags.insert(flagTickInterval, Duration::from_secs(2));
    flags.insert(flagTryAdvanceThreshold, Duration::from_secs(3));
    flags.insert(flagCheckPointLagLimit, Duration::from_secs(4));
    flags.insert(flagOwnershipCycleInterval, Duration::from_secs(5));

    let mut conf = DefaultCommandConfig();
    conf.GetFromFlags(&flags).unwrap();
    assert_eq!(conf.BackoffTime, Duration::from_secs(1));
    assert_eq!(conf.TickDuration, Duration::from_secs(2));
    assert_eq!(conf.TryAdvanceThreshold, Duration::from_secs(3));
    assert_eq!(conf.CheckPointLagLimit, Duration::from_secs(4));
    assert_eq!(conf.OwnershipCycleInterval, Duration::from_secs(5));

    flags.remove(flagTryAdvanceThreshold);
    let mut partial = DefaultCommandConfig();
    assert_eq!(
        partial.GetFromFlags(&flags),
        Err("missing try-advance-threshold".to_string())
    );
    assert_eq!(partial.BackoffTime, Duration::from_secs(1));
    assert_eq!(partial.TickDuration, Duration::from_secs(2));
    assert_eq!(partial.TryAdvanceThreshold, DefaultTryAdvanceThreshold);
    assert_eq!(partial.CheckPointLagLimit, DefaultCheckPointLagLimit);
}

/// TiDBConfig embeds CommandConfig in Go, so every method except lag-limit delegates to it.
#[test]
fn tidb_config_delegates_embedded_command_config_methods() {
    let mut conf = DefaultTiDBConfig();
    conf.CommandConfig.BackoffTime = Duration::from_secs(1);
    conf.CommandConfig.TickDuration = Duration::from_secs(2);
    conf.CommandConfig.TryAdvanceThreshold = Duration::from_secs(20);

    assert_eq!(conf.GetBackoffTime(), Duration::from_secs(1));
    assert_eq!(conf.TickTimeout(), Duration::from_secs(2));
    assert_eq!(conf.GetResolveLockInterval(), Duration::from_secs(4));
    assert_eq!(conf.GetDefaultStartPollThreshold(), Duration::from_secs(20));
    assert_eq!(
        conf.GetSubscriberErrorStartPollThreshold(),
        Duration::from_secs(9)
    );
}
