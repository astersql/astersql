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

// Region 统计信息类型定义。
//
// 对应 PD API 返回的 Region 分布与容量统计：总数量、空 Region、存储体积，
// 以及按 Store（TiKV 节点）聚合的 Leader/Peer 数量与大小。用于展示集群
// 负载分布，支撑调度与运维观察。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Go 的 nil map 编码为 null；Rust 以空 HashMap 表示同一零值。
mod nil_if_empty_map {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;
    use std::hash::{BuildHasher, Hash};

    pub fn serialize<S, K, V, H>(value: &HashMap<K, V, H>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize + Eq + Hash,
        V: Serialize,
        H: BuildHasher,
    {
        if value.is_empty() {
            serializer.serialize_none()
        } else {
            serializer.serialize_some(value)
        }
    }

    pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
    {
        Ok(Option::<HashMap<K, V>>::deserialize(deserializer)?.unwrap_or_default())
    }
}

/// RegionStats records regions' statistics and distribution status.
/// Region 统计与分布状态：含全局计数及按 Store 聚合的 Leader/Peer 指标。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RegionStats {
    /// Region 总数。
    #[serde(rename = "count")]
    pub Count: i64,
    /// 空 Region（无用户数据）数量。
    #[serde(rename = "empty_count")]
    pub EmptyCount: i64,
    /// 全部 Region 占用的存储大小（字节量级聚合）。
    #[serde(rename = "storage_size")]
    pub StorageSize: i64,
    /// 全部 Region 中的 key 数量聚合。
    #[serde(rename = "storage_keys")]
    pub StorageKeys: i64,
    /// 各 Store 上担任 Leader 的 Region 数量；键为 store id。
    #[serde(rename = "store_leader_count", with = "nil_if_empty_map")]
    pub StoreLeaderCount: HashMap<u64, i64>,
    /// 各 Store 上 Peer（副本）数量；键为 store id。
    #[serde(rename = "store_peer_count", with = "nil_if_empty_map")]
    pub StorePeerCount: HashMap<u64, i64>,
    /// 各 Store 上 Leader Region 的数据大小。
    #[serde(rename = "store_leader_size", with = "nil_if_empty_map")]
    pub StoreLeaderSize: HashMap<u64, i64>,
    /// 各 Store 上 Leader Region 的 key 数量。
    #[serde(rename = "store_leader_keys", with = "nil_if_empty_map")]
    pub StoreLeaderKeys: HashMap<u64, i64>,
    /// 各 Store 上 Peer Region 的数据大小。
    #[serde(rename = "store_peer_size", with = "nil_if_empty_map")]
    pub StorePeerSize: HashMap<u64, i64>,
    /// 各 Store 上 Peer Region 的 key 数量。
    #[serde(rename = "store_peer_keys", with = "nil_if_empty_map")]
    pub StorePeerKeys: HashMap<u64, i64>,
}
