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

// PD 复制（replication）相关配置类型。
//
// 控制 Region 副本数、机房/机架等 location label、是否启用 placement rule，
// JSON 字段名与布尔字符串序列化方式与 Go PD 配置保持一致。

use crate::typeutil::StringSlice;
use serde::{Deserialize, Serialize};

/// 将 bool 序列化为 `"true"` / `"false"` 字符串，匹配 PD JSON 约定。
mod bool_string {
    use serde::{Deserialize, Deserializer, Serializer};

    /// 序列化为字符串形式的布尔值。
    pub fn serialize<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(if *value { "true" } else { "false" })
    }

    /// 从 `"true"` / `"false"` 字符串反序列化；`null` 对齐 Go 零值语义。
    pub fn deserialize<'de, D>(deserializer: D) -> Result<bool, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<String>::deserialize(deserializer)?.as_deref() {
            Some("true") => Ok(true),
            Some("false") | Some("null") | None => Ok(false),
            Some(value) => Err(serde::de::Error::custom(format!(
                "invalid boolean string {value:?}"
            ))),
        }
    }
}

/// ReplicationConfig is the replication configuration.
/// Region 副本与放置相关的复制配置。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplicationConfig {
    /// 最大副本数（如 3）。
    #[serde(rename = "max-replicas")]
    pub MaxReplicas: u64,
    /// 拓扑位置标签键列表（如 zone,rack），用于隔离与调度。
    #[serde(rename = "location-labels")]
    pub LocationLabels: StringSlice,
    /// 是否严格匹配 Store 标签。
    #[serde(rename = "strictly-match-label", with = "bool_string")]
    pub StrictlyMatchLabel: bool,
    /// 是否启用 placement rule（细粒度放置规则）。
    #[serde(rename = "enable-placement-rules", with = "bool_string")]
    pub EnablePlacementRules: bool,
    /// 是否启用 placement rule 缓存。
    #[serde(rename = "enable-placement-rules-cache", with = "bool_string")]
    pub EnablePlacementRulesCache: bool,
    /// 隔离级别（对应某个 location label，如 zone）。
    #[serde(rename = "isolation-level")]
    pub IsolationLevel: String,
}
