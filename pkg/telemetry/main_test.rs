// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// telemetry 包测试入口与导出辅助绑定。
//
// 提供与 Go 测试同名的 `GetTxnUsageInfo` / `GetFeatureUsage` 包装。

use crate::{
    SessionContext, TelemetryError, TxnUsage, featureUsage, getFeatureUsage, getTxnUsageInfo,
};

/// GetTxnUsageInfo mirrors Go's exported test helper binding.
/// 导出测试辅助：封装 [`getTxnUsageInfo`]，对应 Go 侧测试绑定。
#[allow(non_snake_case)]
pub fn GetTxnUsageInfo(ctx: &SessionContext) -> TxnUsage {
    getTxnUsageInfo(ctx)
}

/// GetFeatureUsage mirrors Go's exported wrapper with InternalTxnTelemetry source.
/// 导出包装：以 InternalTxnTelemetry 来源调用 [`getFeatureUsage`]。
#[allow(non_snake_case)]
pub fn GetFeatureUsage(ctx: &SessionContext) -> Result<featureUsage, TelemetryError> {
    getFeatureUsage(ctx)
}
