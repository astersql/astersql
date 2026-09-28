// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Go-equivalent tests for `br/pkg/streamhelper/config/config_test.go`.
//!
//! 验证 TiDB/Command 两套配置对检查点滞后上限与 resolve-lock 间隔的语义。
//! Go `TestCheckPointLimit` 经 testkit 写 `tidb_advancer_check_point_lag_limit`，
//! 最终落到 `vardef.AdvancerCheckPointLagLimit`；本 crate 无 kv/domain，
//! 故用 `AdvancerCheckPointLagLimitNanos` 模拟同一全局副作用。

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::{
    AdvancerCheckPointLagLimitNanos, Config, DefaultCheckPointLagLimit, DefaultCommandConfig,
    DefaultTiDBConfig, DefaultTickInterval, tidb_conf::ADVANCER_LAG_LIMIT_TEST_LOCK,
};

/// Corresponds to Go `TestCheckPointLimit`.
/// 断言：默认 48h；改全局后仅 TiDBConfig 跟随，CommandConfig 保持独立。
#[test]
fn test_check_point_limit() {
    let _guard = ADVANCER_LAG_LIMIT_TEST_LOCK.lock().unwrap();
    // Go: require.Equal(t, time.Hour*48, config.DefaultTiDBConfig().GetCheckPointLagLimit())
    //     require.Equal(t, time.Hour*48, config.DefaultCommandConfig().GetCheckPointLagLimit())
    // 先复位全局原子量，避免其它测试残留影响默认值断言。
    AdvancerCheckPointLagLimitNanos.store(
        DefaultCheckPointLagLimit.as_nanos() as u64,
        Ordering::SeqCst,
    );
    assert_eq!(
        DefaultTiDBConfig().GetCheckPointLagLimit(),
        Duration::from_secs(48 * 3600)
    );
    assert_eq!(
        DefaultCommandConfig().GetCheckPointLagLimit(),
        Duration::from_secs(48 * 3600)
    );

    // Go: tk.MustExec("set @@global.tidb_advancer_check_point_lag_limit = '100h'")
    // sysvar setter stores into vardef.AdvancerCheckPointLagLimit; mirror that store.
    // 模拟 set global 后：TiDBConfig 读到 100h，CommandConfig 仍为默认 48h。
    AdvancerCheckPointLagLimitNanos.store(
        Duration::from_secs(100 * 3600).as_nanos() as u64,
        Ordering::SeqCst,
    );
    assert_eq!(
        DefaultTiDBConfig().GetCheckPointLagLimit(),
        Duration::from_secs(100 * 3600)
    );
    // CommandConfig is independent of the global vardef.
    assert_eq!(
        DefaultCommandConfig().GetCheckPointLagLimit(),
        Duration::from_secs(48 * 3600)
    );

    // Restore default so other tests in this crate see the Go DefTiDBAdvancerCheckPointLagLimit.
    // 恢复默认，避免污染同 crate 其它用例。
    AdvancerCheckPointLagLimitNanos.store(
        DefaultCheckPointLagLimit.as_nanos() as u64,
        Ordering::SeqCst,
    );
}

/// Corresponds to Go `TestResolveLockInterval`.
/// 断言 resolve-lock 间隔恒为 TickDuration×2（TiDB 默认与改写 Command 后均成立）。
#[test]
fn test_resolve_lock_interval() {
    // Go: require.Equal(t, config.DefaultTickInterval*2, config.DefaultTiDBConfig().GetResolveLockInterval())
    assert_eq!(
        DefaultTiDBConfig().GetResolveLockInterval(),
        DefaultTickInterval * 2
    );

    // Go: conf := config.DefaultCommandConfig().(*config.CommandConfig)
    // 改 TickDuration 后，GetResolveLockInterval 必须同步变为 2 倍。
    let mut conf = DefaultCommandConfig();
    assert_eq!(conf.GetResolveLockInterval(), DefaultTickInterval * 2);
    conf.TickDuration = Duration::from_secs(10);
    assert_eq!(conf.GetResolveLockInterval(), Duration::from_secs(20));
}
