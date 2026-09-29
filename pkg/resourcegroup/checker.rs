// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 资源组（Resource Group） runaway 检查与消费上报接口。
//
// 资源组用于隔离与配额管理：按组限制 CPU/IO 等折算后的 RU（Request Unit，
// 请求单元）。Runaway 机制检测并处置超出阈值/规则的查询（如标记、降速、杀查询）。
// 本文件定义与 Go 接口对齐的 trait，具体实现由各 TiKV 客户端集成提供。

/// 默认资源组名称（未指定时回退到该组）。
/// The default resource group name.
pub const DEFAULT_RESOURCE_GROUP_NAME: &str = "default";

/// 判断查询是否 runaway（失控/超限）。
///
/// Checks whether a query is runaway.
///
/// The associated types preserve the external protocol types used by the Go
/// interface while allowing each TiKV client integration to select its native
/// request, RU-detail, action, and error representations.
/// 关联类型保留 Go 侧外部协议类型，便于各 TiKV 客户端选用原生的
/// Request、RU 明细、动作与错误表示。
pub trait RunawayChecker: Send + Sync {
    /// 对 runaway 查询采取的动作类型（如 Kill）。
    type Action: Copy + Send + Sync + 'static;
    /// 检查失败时返回的错误类型。
    type Error: std::error::Error + Send + Sync + 'static;
    /// Coprocessor（协处理器）请求类型。
    type Request;
    /// RU 消费明细类型。
    type RuDetails;

    /// 执行前/编译后检查 watch list（监视名单）。
    /// Checks the watch list before execution and after compilation.
    fn before_executor(&self) -> Result<String, Self::Error>;
    /// 发送前检查 runaway 状态，并可修改 coprocessor 请求。
    /// Checks runaway state and may modify a coprocessor request before send.
    fn before_cop_request(&self, req: &mut Self::Request) -> Result<(), Self::Error>;
    /// 检查 TiKV 结果是否超过配置阈值。
    /// Checks whether TiKV results exceed a configured threshold.
    fn check_thresholds(
        &self,
        detail: &Self::RuDetails,
        process_keys: i64,
        err: Option<Self::Error>,
    ) -> Option<Self::Error>;
    /// 重置累计已处理 key 计数。
    /// Resets the accumulated processed-key count.
    fn reset_total_processed_keys(&self);
    /// 返回当前动作；实现须支持并发调用。
    /// Returns the current action. Implementations must support concurrent calls.
    fn check_action(&self) -> Self::Action;
    /// 报告资源组规则是否要求杀掉查询；返回 (规则名, 是否 kill)。
    /// Reports whether the resource-group rule requires killing the query.
    fn check_rule_kill_action(&self) -> (String, bool);
}

/// 上报原始资源消耗。
/// Reports raw resource consumption.
pub trait ConsumptionReporter: Send + Sync {
    /// 一次消费采样的数据结构。
    type Consumption;

    /// 在常规 KV 拦截器不可用时上报消费。
    /// Reports consumption when the normal KV interceptor is unavailable.
    fn report_consumption(&self, resource_group_name: &str, consumption: &Self::Consumption);
    /// 通过旧版引擎槽位 API 上报 RU 消费。
    /// Reports RU consumption through the legacy engine-slot API.
    fn report_ruv2_consumption(
        &self,
        resource_group_name: &str,
        tikv_ruv2: f64,
        tidb_ruv2: f64,
        tiflash_ruv2: f64,
    );
}
