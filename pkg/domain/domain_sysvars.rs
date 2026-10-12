// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Domain 系统变量回调与动态 PD Client 选项存储。
// 将依赖 Domain 上下文的 sysvar（系统变量）变更落到进程内状态；
// TSO（Timestamp Oracle，时间戳分配服务）相关选项影响取时间戳批处理与 follower 代理行为。
// 外部 PD/资源组客户端的真实副作用由边界适配层负责；本模块保留可观测状态与回调顺序。

use crate::domain::Domain;
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

/// 系统变量名：TSO 客户端批处理最大等待时间。
pub const TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME: &str = "tidb_tso_client_batch_max_wait_time";
/// 系统变量名：是否启用 TSO Follower Proxy。
pub const TIDB_ENABLE_TSO_FOLLOWER_PROXY: &str = "tidb_enable_tso_follower_proxy";
/// 系统变量名：是否允许 PD Follower 处理 Region 请求。
pub const PD_ENABLE_FOLLOWER_HANDLE_REGION: &str = "pd_enable_follower_handle_region";
/// 系统变量名：TSO 客户端 RPC 并发模式。
pub const TIDB_TSO_CLIENT_RPC_MODE: &str = "tidb_tso_client_rpc_mode";
/// 系统变量名：是否启用批量查询 Region（Router Client）。
pub const TIDB_ENABLE_BATCH_QUERY_REGION: &str = "tidb_enable_batch_query_region";

#[derive(Clone, Debug, PartialEq)]
/// PD Client 动态选项的类型化取值。
pub enum DynamicOption {
    /// Go `time.Duration` 对应的有符号纳秒数（如批等待间隔）。
    DurationNanos(i64),
    /// 布尔开关类选项。
    Bool(bool),
    /// 整数类选项（如 RPC 并发度）。
    Integer(usize),
}

#[derive(Default)]
/// Domain 侧系统变量状态容器（全局单例）。
pub struct DomainSysVars {
    options: RwLock<BTreeMap<String, DynamicOption>>,
    stats_cache_capacity: RwLock<i64>,
    external_timestamp: RwLock<u64>,
    resource_control: RwLock<bool>,
    circuit_breaker_ratio: RwLock<u32>,
}

impl DomainSysVars {
    /// 返回进程级全局 DomainSysVars。
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<DomainSysVars> = OnceLock::new();
        INSTANCE.get_or_init(Self::default)
    }

    /// 设置统计缓存容量。
    pub fn set_stats_cache_capacity(&self, capacity: i64) {
        *self
            .stats_cache_capacity
            .write()
            .expect("sysvar lock poisoned") = capacity;
    }

    /// 读取统计缓存容量。
    pub fn stats_cache_capacity(&self) -> i64 {
        *self
            .stats_cache_capacity
            .read()
            .expect("sysvar lock poisoned")
    }

    /// 按系统变量名解析并写入对应 DynamicOption；未知名忽略。
    pub fn set_pd_client_dynamic_option(&self, name: &str, value: &str) -> Result<(), String> {
        // 按变量名解析字符串值为类型化选项。
        let parsed = match name {
            TIDB_TSO_CLIENT_BATCH_MAX_WAIT_TIME => {
                let millis = value.parse::<f64>().map_err(|error| error.to_string())?;
                DynamicOption::DurationNanos((1_000_000.0 * millis) as i64)
            }
            TIDB_ENABLE_TSO_FOLLOWER_PROXY
            | PD_ENABLE_FOLLOWER_HANDLE_REGION
            | TIDB_ENABLE_BATCH_QUERY_REGION => DynamicOption::Bool(tidb_opt_on(value)),
            TIDB_TSO_CLIENT_RPC_MODE => DynamicOption::Integer(match value {
                "DEFAULT" => 1,
                "PARALLEL" => 2,
                "PARALLEL-FAST" => 4,
                _ => return Err(format!("wrong value for {name}: {value}")),
            }),
            _ => return Ok(()),
        };
        self.options
            .write()
            .expect("sysvar lock poisoned")
            .insert(name.to_owned(), parsed);
        Ok(())
    }

    /// 读取已设置的动态选项。
    pub fn option(&self, name: &str) -> Option<DynamicOption> {
        self.options
            .read()
            .expect("sysvar lock poisoned")
            .get(name)
            .cloned()
    }

    /// 写入外部时间戳（供 oracle/外部时钟对齐）。
    pub fn set_external_timestamp(&self, timestamp: u64) {
        *self
            .external_timestamp
            .write()
            .expect("sysvar lock poisoned") = timestamp;
    }

    /// 读取外部时间戳。
    pub fn external_timestamp(&self) -> u64 {
        *self
            .external_timestamp
            .read()
            .expect("sysvar lock poisoned")
    }

    /// 设置全局资源管控（Resource Control）开关。
    pub fn set_global_resource_control(&self, enabled: bool) {
        *self.resource_control.write().expect("sysvar lock poisoned") = enabled;
    }

    /// 设置 PD metadata 熔断器错误率阈值。
    pub fn set_circuit_breaker_error_rate_ratio(&self, ratio: u32) {
        *self
            .circuit_breaker_ratio
            .write()
            .expect("sysvar lock poisoned") = ratio;
    }

    /// 读取 PD metadata 熔断器错误率阈值。
    pub fn circuit_breaker_error_rate_ratio(&self) -> u32 {
        *self
            .circuit_breaker_ratio
            .read()
            .expect("sysvar lock poisoned")
    }
}

/// 对齐 `variable.TiDBOptOn`：仅 ON（忽略 ASCII 大小写）和 1 为真。
fn tidb_opt_on(value: &str) -> bool {
    value.eq_ignore_ascii_case("ON") || value == "1"
}

impl Domain {
    /// Domain 初始化时接入系统变量回调入口；返回全局 DomainSysVars。
    pub fn init_domain_sys_vars(&self) -> &'static DomainSysVars {
        DomainSysVars::global()
    }
}
