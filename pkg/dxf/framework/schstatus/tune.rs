// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 调度器资源调参因子（TuneFactors）。
//
// AmplifyFactor 用于放大任务估算所需的资源/节点数；TTLTuneFactors
// 将 TTL（Time To Live，存活时间）信息与调参因子一并持久化。

use super::status::TTLInfo;
use serde::{Deserialize, Serialize};

/// 判断 f64 是否为零，供 serde skip_serializing_if 使用。
fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

// MinAmplifyFactor is the minimum amplify factor.
/// 放大因子下限（与 Go 常量一致）。
pub const MinAmplifyFactor: f64 = 1.0;
// MaxAmplifyFactor is the maximum amplify factor.
/// 放大因子上限（与 Go 常量一致）。
pub const MaxAmplifyFactor: f64 = 10.0;
// defaultAmplifyFactor 对应 Go 的包内默认值，保持为 MinAmplifyFactor。
/// 包内默认放大因子，等于 MinAmplifyFactor。
pub const defaultAmplifyFactor: f64 = MinAmplifyFactor;

// TuneFactors represents the resource tuning factors.
// TuneFactors 对应 Go 的调参因子结构，目前只有 AmplifyFactor 一个 JSON 字段。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// 资源调参因子集合；当前仅暴露 amplify_factor。
pub struct TuneFactors {
    // AmplifyFactor is used to amplify the input data size and node count limit,
    // through this, we can amplify the calculated resource for new tasks.
    #[serde(rename = "amplify_factor", default, skip_serializing_if = "is_zero")]
    /// 放大输入数据规模与节点数上限，从而放大新任务的资源估算。
    pub AmplifyFactor: f64,
}

// TTLTuneFactors represents the TTL info and tuning factors.
// only used for storage.
// TTLTuneFactors 对应 Go 的结构体嵌入：TTLInfo + TuneFactors，主要用于持久化存储。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
/// 带 TTL 的调参因子，主要用于存储层持久化。
pub struct TTLTuneFactors {
    #[serde(flatten)]
    /// 嵌入的 TTL 信息（ttl / expire_time）。
    pub TTLInfo: TTLInfo,
    #[serde(flatten)]
    /// 嵌入的调参因子（amplify_factor）。
    pub TuneFactors: TuneFactors,
}

impl TTLTuneFactors {
    // String implements the fmt.Stringer interface.
    // String 对应 Go 的 json.Marshal(f)，忽略序列化错误并返回 JSON 字符串。
    /// 序列化为 JSON 字符串；忽略序列化错误并返回空串（对齐 Go）。
    pub fn String(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// GetDefaultTuneFactors get the default tuning factors.
// GetDefaultTuneFactors 对应 Go 的默认因子构造函数，返回新的 TuneFactors 指针语义。
/// 构造默认 TuneFactors（AmplifyFactor = defaultAmplifyFactor）。
pub fn GetDefaultTuneFactors() -> TuneFactors {
    TuneFactors {
        AmplifyFactor: defaultAmplifyFactor,
    }
}
