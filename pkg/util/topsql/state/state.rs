// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TopSQL / TopRU 全局开关与采样参数状态。
//
// TopSQL：按 SQL/计划摘要收集执行耗时等高开销语句画像。
// TopRU：按 Resource Unit（RU，资源计量单位）汇总资源消耗；消费者以引用计数管理，
// 最后一个消费者退出时重置 item interval。对应 Go `pkg/util/topsql/state`。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use thiserror::Error;

// Default Top-SQL state values.
/// TopSQL 默认关闭。
pub const DefTiDBTopSQLEnable: bool = false;
/// TopSQL 时间序列精度（秒）。
pub const DefTiDBTopSQLPrecisionSeconds: i64 = 1;
/// TopSQL 最大时间序列条数。
pub const DefTiDBTopSQLMaxTimeSeriesCount: i64 = 100;
/// TopSQL 最大元数据收集条数。
pub const DefTiDBTopSQLMaxMetaCount: i64 = 5000;
/// TopSQL 上报间隔（秒）。
pub const DefTiDBTopSQLReportIntervalSeconds: i64 = 60;

// Default Top-RU state values.
/// TopRU item 默认聚合窗口（秒）。
pub const DefTiDBTopRUItemIntervalSeconds: i64 = 60;

// The Rust protobuf enum cannot represent unknown numeric values, while the Go
// protobuf enum can. Keep the wire-level i32 at this boundary so invalid values
// such as 1 and 99 retain the Go validation behavior.
/// TopRU item interval 的线协议 i32；非法值保留 Go 校验语义。
pub type ItemInterval = i32;

/// 非法 TopRU item interval 的错误前缀文案。
pub const ErrInvalidTopRUItemInterval: &str = "invalid top ru item interval";

/// TopRU 状态相关错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TopRUStateError {
    /// item interval 不在允许集合内。
    #[error("invalid top ru item interval: {0}")]
    InvalidItemInterval(ItemInterval),
}

// State is the state for control top sql feature.
/// 控制 TopSQL/TopRU 的进程级原子状态。
pub struct State {
    /// TopSQL 总开关。
    enable: AtomicBool,
    /// 采样时间精度（秒）。
    pub PrecisionSeconds: AtomicI64,
    /// 最多保留的语句时间序列数。
    pub MaxStatementCount: AtomicI64,
    /// 最多收集的元数据条数。
    pub MaxCollect: AtomicI64,
    /// TopRU 消费者引用计数；>0 表示 TopRU 开启。
    ruConsumerCount: AtomicI64,
    /// TopRU item 聚合窗口（秒）。
    pub TopRUItemIntervalSeconds: AtomicI64,
}

// GlobalState is the global Top-SQL state.
/// 全局单例状态，供各模块原子读写。
pub static GlobalState: State = State {
    enable: AtomicBool::new(DefTiDBTopSQLEnable),
    PrecisionSeconds: AtomicI64::new(DefTiDBTopSQLPrecisionSeconds),
    MaxStatementCount: AtomicI64::new(DefTiDBTopSQLMaxTimeSeriesCount),
    MaxCollect: AtomicI64::new(DefTiDBTopSQLMaxMetaCount),
    ruConsumerCount: AtomicI64::new(0),
    TopRUItemIntervalSeconds: AtomicI64::new(DefTiDBTopRUItemIntervalSeconds),
};

/// 打开 TopSQL 开关。
pub fn EnableTopSQL() {
    GlobalState.enable.store(true, Ordering::SeqCst);
}

/// 关闭 TopSQL 开关。
pub fn DisableTopSQL() {
    GlobalState.enable.store(false, Ordering::SeqCst);
}

/// 查询 TopSQL 是否开启。
pub fn TopSQLEnabled() -> bool {
    GlobalState.enable.load(Ordering::SeqCst)
}

/// TopSQL 或 TopRU 任一开启时，画像/剖析相关路径可工作。
pub fn TopProfilingEnabled() -> bool {
    TopSQLEnabled() || TopRUEnabled()
}

/// 增加 TopRU 消费者引用计数。
pub fn EnableTopRU() {
    GlobalState.ruConsumerCount.fetch_add(1, Ordering::SeqCst);
}

/// 减少 TopRU 引用计数；归零时重置 item interval；禁止下溢。
pub fn DisableTopRU() {
    // CAS 循环避免并发减计数时出现负值或漏重置。
    loop {
        let previous = GlobalState.ruConsumerCount.load(Ordering::SeqCst);
        if previous <= 0 {
            return;
        }

        if GlobalState
            .ruConsumerCount
            .compare_exchange(previous, previous - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            // 最后一个消费者离开时恢复默认窗口。
            if previous == 1 {
                ResetTopRUItemInterval();
            }
            return;
        }
    }
}

/// TopRU 是否仍有活跃消费者。
pub fn TopRUEnabled() -> bool {
    GlobalState.ruConsumerCount.load(Ordering::SeqCst) > 0
}

/// 将线协议 interval 归一化为允许的秒数；0 表示默认 60。
fn normalizeTopRUItemIntervalSeconds(
    interval_seconds: ItemInterval,
) -> Result<i64, TopRUStateError> {
    match interval_seconds {
        0 => Ok(DefTiDBTopRUItemIntervalSeconds),
        15 | 30 | 60 => Ok(i64::from(interval_seconds)),
        value => Err(TopRUStateError::InvalidItemInterval(value)),
    }
}

/// 设置 TopRU item interval；非法值不覆盖当前合法值（后写覆盖）。
pub fn SetTopRUItemInterval(item_interval_seconds: ItemInterval) -> Result<(), TopRUStateError> {
    let normalized = normalizeTopRUItemIntervalSeconds(item_interval_seconds);
    let current = GetTopRUItemInterval();
    let active_subscribers = GlobalState.ruConsumerCount.load(Ordering::SeqCst);

    let interval_seconds = match normalized {
        Ok(interval_seconds) => interval_seconds,
        Err(error) => {
            log::warn!(
                "[top-sql] top ru item interval invalid; current_interval_seconds={current}, active_subscribers={active_subscribers}, error={error}"
            );
            return Err(error);
        }
    };

    log::info!(
        "[top-sql] top ru item interval overridden by later subscription; current_interval_seconds={current}, new_interval_seconds={interval_seconds}, active_subscribers={active_subscribers}"
    );
    GlobalState
        .TopRUItemIntervalSeconds
        .store(interval_seconds, Ordering::SeqCst);
    Ok(())
}

/// 读取当前 TopRU item interval（秒）。
pub fn GetTopRUItemInterval() -> i64 {
    GlobalState.TopRUItemIntervalSeconds.load(Ordering::SeqCst)
}

/// 将 TopRU item interval 重置为默认值。
pub fn ResetTopRUItemInterval() {
    GlobalState
        .TopRUItemIntervalSeconds
        .store(DefTiDBTopRUItemIntervalSeconds, Ordering::SeqCst);
}
