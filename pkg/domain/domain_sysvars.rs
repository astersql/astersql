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
// 文件前半为 Go 迁移草稿（已注释），后半为可编译的 DomainSysVars 实现。

// Domain 初始化和更新依赖当前 Domain 的特殊系统变量回调。
//
// impl Domain {
// initDomainSysVars 对应 Go 的同名方法：Domain 初始化时把需要 Domain 上下文的系统变量回调接到 variable 包。
// Go 中这些 Store/赋值会改全局函数指针；这里保留注册顺序，但不真正修改全局状态。
//     pub fn initDomainSysVars(&mut self) {
//         let setStatsCacheCapacityFunc = self.setStatsCacheCapacity;
//         variable::SetStatsCacheCapacity.Store(&setStatsCacheCapacityFunc);
//         let pdClientDynamicOptionFunc = self.setPDClientDynamicOption;
//         variable::SetPDClientDynamicOption.Store(&pdClientDynamicOptionFunc);
//
//         variable::SetExternalTimestamp = self.setExternalTimestamp;
//         variable::GetExternalTimestamp = self.getExternalTimestamp;
//
//         let setGlobalResourceControlFunc = self.setGlobalResourceControl;
//         variable::SetGlobalResourceControl.Store(&setGlobalResourceControlFunc);
//         variable::SetLowResolutionTSOUpdateInterval = self.setLowResolutionTSOUpdateInterval;
//
// Go 这里把 schema cache 调整入口转接到 info syncer；仅保留字段访问形状。
//         variable::ChangeSchemaCacheSize = self.isSyncer.ChangeSchemaCacheSize;
//
//         variable::ChangePDMetadataCircuitBreakerErrorRateThresholdRatio =
//             changePDMetadataCircuitBreakerErrorRateThresholdRatio;
//     }
//
// setStatsCacheCapacity 对应 Go 的 stats cache 容量设置回调。
//     pub fn setStatsCacheCapacity(&mut self, c: i64) {
//         let statsHandle = self.StatsHandle();
//         if statsHandle.is_none() {
// Go 注释说明该分支来自测试场景；没有 stats handle 时保持 no-op。
//             return;
//         }
//         self.StatsHandle().SetStatsCacheCapacity(c);
//     }
//
// setPDClientDynamicOption 对应 Go 中按系统变量名动态更新 PD client option 的逻辑。
//     pub fn setPDClientDynamicOption(
//         &mut self,
//         name: &str,
//         sVal: &str,
//     ) -> Result<(), errors::Error> {
//         match name {
//             vardef::TiDBTSOClientBatchMaxWaitTime => {
// Go 使用 strconv.ParseFloat 解析毫秒数，再转成 time.Duration。
//                 let val = sVal.parse::<f64>()?;
//                 self.updatePDClient(
//                     opt::MaxTSOBatchWaitInterval,
//                     time::Duration::from_millis(val as u64),
//                 )?;
//                 vardef::MaxTSOBatchWaitInterval.Store(val);
//             }
//             vardef::TiDBEnableTSOFollowerProxy => {
//                 let val = variable::TiDBOptOn(sVal);
//                 self.updatePDClient(opt::EnableTSOFollowerProxy, val)?;
//                 vardef::EnableTSOFollowerProxy.Store(val);
//             }
//             vardef::PDEnableFollowerHandleRegion => {
//                 let val = variable::TiDBOptOn(sVal);
// Go 说明 EnableFollowerHandle 当前只服务 region API；保留同一 PD option。
//                 self.updatePDClient(opt::EnableFollowerHandle, val)?;
//                 vardef::EnablePDFollowerHandleRegion.Store(val);
//             }
//             vardef::TiDBTSOClientRPCMode => {
//                 let concurrency = match sVal {
//                     vardef::TSOClientRPCModeDefault => 1,
//                     vardef::TSOClientRPCModeParallel => 2,
//                     vardef::TSOClientRPCModeParallelFast => 4,
//                     _ => {
// Go 返回 ErrWrongValueForVar.GenWithStackByArgs，表示变量值非法。
//                         return Err(variable::ErrWrongValueForVar.GenWithStackByArgs(name, sVal));
//                     }
//                 };
//
//                 self.updatePDClient(opt::TSOClientRPCConcurrency, concurrency)?;
//             }
//             vardef::TiDBEnableBatchQueryRegion => {
//                 let val = variable::TiDBOptOn(sVal);
//                 self.updatePDClient(opt::EnableRouterClient, val)?;
//                 vardef::EnableBatchQueryRegion.Store(val);
//             }
//             _ => {}
//         }
//         Ok(())
//     }
//
// setGlobalResourceControl 对应 Go 的全局资源管控开关。
//     pub fn setGlobalResourceControl(&self, enable: bool) {
//         if enable {
//             variable::EnableGlobalResourceControlFunc();
//         } else {
//             variable::DisableGlobalResourceControlFunc();
//         }
//     }
//
// setLowResolutionTSOUpdateInterval 对应 Go 中通过 store oracle 调整低精度 TSO 刷新间隔。
//     pub fn setLowResolutionTSOUpdateInterval(
//         &mut self,
//         interval: time::Duration,
//     ) -> Result<(), errors::Error> {
//         self.store
//             .GetOracle()
//             .SetLowResolutionTimestampUpdateInterval(interval)
//     }
//
// updatePDClient 对应 Go 的动态 PD client option 更新辅助函数。
//     pub fn updatePDClient(
//         &mut self,
//         option: opt::DynamicOption,
//         val: impl std::any::Any,
//     ) -> Result<(), errors::Error> {
//         let store = match self.store.as_pd_client_provider() {
//             Some(store) => store,
//             None => {
// Go 类型断言失败时返回 nil；表示非 PD store 不需要更新。
//                 return Ok(());
//             }
//         };
//         let pdClient = store.GetPDClient();
//         if pdClient.is_none() {
//             return Ok(());
//         }
//         pdClient.UpdateOption(option, val)
//     }
//
// setExternalTimestamp 对应 Go 中把外部时间戳写入 store oracle。
//     pub fn setExternalTimestamp(
//         &mut self,
//         ctx: context::Context,
//         ts: u64,
//     ) -> Result<(), errors::Error> {
//         self.store.GetOracle().SetExternalTimestamp(ctx, ts)
//     }
//
// getExternalTimestamp 对应 Go 中读取 store oracle 的外部时间戳。
//     pub fn getExternalTimestamp(&self, ctx: context::Context) -> Result<u64, errors::Error> {
//         self.store.GetOracle().GetExternalTimestamp(ctx)
//     }
// }
//
// changePDMetadataCircuitBreakerErrorRateThresholdRatio 对应 Go 的 PD region meta 熔断阈值更新函数。
// pub fn changePDMetadataCircuitBreakerErrorRateThresholdRatio(errorRateRatio: u32) {
//     tikv::ChangePDRegionMetaCircuitBreakerSettings(|config: &mut circuitbreaker::Settings| {
//         config.ErrorRateThresholdPct = errorRateRatio;
//     });
// }
// */
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
