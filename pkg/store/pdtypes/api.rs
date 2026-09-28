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

// PD HTTP/API 兼容的 Store / Region 信息类型。
//
// PD（Placement Driver）负责集群元数据与调度；本模块描述对外 API 使用的
// Store（存储节点）与 Region（键空间分片）详情结构，字段命名与 Go pdtypes 对齐。

use crate::configtypes::{ByteSize, Duration};
use astersql_config_configtypes::{
    ByteSize_MarshalJSON, ByteSize_UnmarshalJSON, Duration_MarshalJSON, Duration_UnmarshalJSON,
};
use chrono::{DateTime, SecondsFormat, Utc};
use kvproto::{metapb, pdpb};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// StoresInfo records stores' info.
/// 集群中全部 Store 的列表与数量。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoresInfo {
    /// Store 总数。
    #[serde(rename = "count")]
    pub Count: i32,
    /// 各 Store 详情。
    #[serde(rename = "stores", with = "nil_if_empty_vec")]
    pub Stores: Vec<StoreInfo>,
}

/// StoreInfo contains information about a store.
/// 单个 Store：元数据与运行状态的组合。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreInfo {
    /// Store 元信息（含 metapb.Store 与状态名）。
    #[serde(rename = "store")]
    pub Store: Option<Box<MetaStore>>,
    /// Store 容量、Region/Leader 统计与心跳等状态。
    #[serde(rename = "status")]
    pub Status: Option<Box<StoreStatus>>,
}

/// MetaStore contains meta information about a store.
/// Store 元信息包装：底层 metapb.Store 加可读状态名。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaStore {
    /// protobuf 定义的 Store 元数据。
    pub Store: Option<Box<metapb::Store>>,
    /// 状态可读名称（如 Up / Offline）。
    pub StateName: String,
}

/// StoreStatus contains status about a store.
/// Store 运行时状态：容量、Leader/Region 负载、快照与心跳时间等。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreStatus {
    /// 总容量。
    #[serde(rename = "capacity", with = "byte_size_json")]
    pub Capacity: ByteSize,
    /// 可用容量。
    #[serde(rename = "available", with = "byte_size_json")]
    pub Available: ByteSize,
    /// 已用大小。
    #[serde(rename = "used_size", with = "byte_size_json")]
    pub UsedSize: ByteSize,
    /// Leader Peer 数量。
    #[serde(rename = "leader_count")]
    pub LeaderCount: i32,
    /// Leader 调度权重。
    #[serde(rename = "leader_weight")]
    pub LeaderWeight: f64,
    /// Leader 得分（调度用）。
    #[serde(rename = "leader_score")]
    pub LeaderScore: f64,
    /// Leader 数据量规模。
    #[serde(rename = "leader_size")]
    pub LeaderSize: i64,
    /// Region 数量。
    #[serde(rename = "region_count")]
    pub RegionCount: i32,
    /// Region 调度权重。
    #[serde(rename = "region_weight")]
    pub RegionWeight: f64,
    /// Region 得分（调度用）。
    #[serde(rename = "region_score")]
    pub RegionScore: f64,
    /// Region 数据量规模。
    #[serde(rename = "region_size")]
    pub RegionSize: i64,
    /// 慢节点评分。
    #[serde(rename = "slow_score")]
    pub SlowScore: u64,
    /// 正在发送的快照数。
    #[serde(
        rename = "sending_snap_count",
        default,
        skip_serializing_if = "is_zero_u32"
    )]
    pub SendingSnapCount: u32,
    /// 正在接收的快照数。
    #[serde(
        rename = "receiving_snap_count",
        default,
        skip_serializing_if = "is_zero_u32"
    )]
    pub ReceivingSnapCount: u32,
    /// 是否忙（负载过高）。
    #[serde(rename = "is_busy", default, skip_serializing_if = "is_false")]
    pub IsBusy: bool,
    /// 启动时间。
    #[serde(
        rename = "start_ts",
        default,
        skip_serializing_if = "Option::is_none",
        with = "datetime_option_json"
    )]
    pub StartTS: Option<DateTime<Utc>>,
    /// 最近心跳时间。
    #[serde(
        rename = "last_heartbeat_ts",
        default,
        skip_serializing_if = "Option::is_none",
        with = "datetime_option_json"
    )]
    pub LastHeartbeatTS: Option<DateTime<Utc>>,
    /// 已运行时长。
    #[serde(
        rename = "uptime",
        default,
        skip_serializing_if = "Option::is_none",
        with = "duration_option_json"
    )]
    pub Uptime: Option<Box<Duration>>,
}

/// RegionsInfo contains regions with detailed region info.
/// Region 列表与数量。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RegionsInfo {
    /// Region 总数。
    #[serde(rename = "count")]
    pub Count: i32,
    /// 各 Region 详情。
    #[serde(rename = "regions", with = "nil_if_empty_vec")]
    pub Regions: Vec<RegionInfo>,
}

/// RegionInfo records detailed region info for API usage.
/// API 用 Region 详情：键范围、Peer、流量与近似大小等。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RegionInfo {
    /// Region ID。
    #[serde(rename = "id")]
    pub ID: u64,
    /// 起始键（通常为编码后字符串）。
    #[serde(rename = "start_key")]
    pub StartKey: String,
    /// 结束键（左闭右开区间的上界）。
    #[serde(rename = "end_key")]
    pub EndKey: String,
    /// Region 纪元（conf_ver / version），用于检测元数据过期。
    #[serde(
        rename = "epoch",
        default,
        skip_serializing_if = "Option::is_none",
        with = "region_epoch_option_json"
    )]
    pub RegionEpoch: Option<Box<metapb::RegionEpoch>>,
    /// 全部 Peer（副本）。
    #[serde(rename = "peers", default, skip_serializing_if = "Vec::is_empty")]
    pub Peers: Vec<MetaPeer>,
    /// 当前 Leader Peer。
    #[serde(rename = "leader")]
    pub Leader: MetaPeer,
    /// 失联/落后的 Down Peer。
    #[serde(rename = "down_peers", default, skip_serializing_if = "Vec::is_empty")]
    pub DownPeers: Vec<PDPeerStats>,
    /// 尚未完成的 Pending Peer。
    #[serde(
        rename = "pending_peers",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub PendingPeers: Vec<MetaPeer>,
    /// 写入字节数。
    #[serde(rename = "written_bytes")]
    pub WrittenBytes: u64,
    /// 读取字节数。
    #[serde(rename = "read_bytes")]
    pub ReadBytes: u64,
    /// 写入键数。
    #[serde(rename = "written_keys")]
    pub WrittenKeys: u64,
    /// 读取键数。
    #[serde(rename = "read_keys")]
    pub ReadKeys: u64,
    /// 近似数据大小。
    #[serde(rename = "approximate_size")]
    pub ApproximateSize: i64,
    /// 近似键数量。
    #[serde(rename = "approximate_keys")]
    pub ApproximateKeys: i64,
    /// 副本复制模式状态。
    #[serde(
        rename = "replication_status",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ReplicationStatus: Option<Box<ReplicationStatus>>,
}

/// MetaPeer is API compatible with metapb.Peer.
/// 与 metapb.Peer API 兼容的 Peer 包装，附带角色名与是否 learner。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetaPeer {
    /// protobuf Peer。
    pub Peer: Option<Box<metapb::Peer>>,
    /// 角色可读名称（voter/leader/follower/learner 等）。
    pub RoleName: String,
    /// 是否为 learner（只同步、不参与投票）。
    pub IsLearner: bool,
}

/// PDPeerStats is API compatible with pdpb.PeerStats.
/// 与 pdpb.PeerStats API 兼容：Peer 统计 + MetaPeer。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PDPeerStats {
    /// protobuf PeerStats。
    pub PeerStats: Option<Box<pdpb::PeerStats>>,
    /// 对应 Peer 的 API 视图。
    pub Peer: MetaPeer,
}

/// ReplicationStatus represents the region replication mode status.
/// Region 复制模式状态（如 DR Auto-Sync 的状态名与状态 ID）。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplicationStatus {
    /// 状态名称。
    #[serde(rename = "state")]
    pub State: String,
    /// 状态 ID。
    #[serde(rename = "state_id")]
    pub StateID: u64,
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Go 的 nil slice 编码为 null；Rust 以空 Vec 表示同一零值。
mod nil_if_empty_vec {
    use super::*;

    pub fn serialize<S, T>(value: &[T], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        if value.is_empty() {
            serializer.serialize_none()
        } else {
            serializer.serialize_some(value)
        }
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
    }
}

mod byte_size_json {
    use super::*;

    pub fn serialize<S>(value: &ByteSize, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let json = ByteSize_MarshalJSON(*value).map_err(serde::ser::Error::custom)?;
        let text: String = serde_json::from_slice(&json).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&text)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ByteSize, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        let json = serde_json::to_vec(&text).map_err(D::Error::custom)?;
        let mut value = 0;
        ByteSize_UnmarshalJSON(&mut value, &json).map_err(D::Error::custom)?;
        Ok(value)
    }
}

mod duration_option_json {
    use super::*;

    pub fn serialize<S>(value: &Option<Box<Duration>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => {
                let json =
                    Duration_MarshalJSON(value.as_ref()).map_err(serde::ser::Error::custom)?;
                let text: String =
                    serde_json::from_slice(&json).map_err(serde::ser::Error::custom)?;
                serializer.serialize_some(&text)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Box<Duration>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = Option::<String>::deserialize(deserializer)?;
        text.map(|text| {
            let json = serde_json::to_vec(&text).map_err(D::Error::custom)?;
            let mut value = Duration::default();
            Duration_UnmarshalJSON(&mut value, &json).map_err(D::Error::custom)?;
            Ok(Box::new(value))
        })
        .transpose()
    }
}

mod datetime_option_json {
    use super::*;

    fn format_go_time(value: &DateTime<Utc>) -> String {
        let text = value.to_rfc3339_opts(SecondsFormat::Nanos, true);
        let Some((prefix, suffix)) = text.rsplit_once('Z') else {
            return text;
        };
        let prefix = prefix
            .strip_suffix(".000000000")
            .unwrap_or_else(|| prefix.trim_end_matches('0').trim_end_matches('.'));
        format!("{prefix}{suffix}Z")
    }

    pub fn serialize<S>(value: &Option<DateTime<Utc>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_some(&format_go_time(value)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                DateTime::parse_from_rfc3339(&text)
                    .map(|time| time.with_timezone(&Utc))
                    .map_err(D::Error::custom)
            })
            .transpose()
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct RegionEpochJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conf_ver: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u64>,
}

impl From<&metapb::RegionEpoch> for RegionEpochJson {
    fn from(value: &metapb::RegionEpoch) -> Self {
        Self {
            conf_ver: (value.conf_ver != 0).then_some(value.conf_ver),
            version: (value.version != 0).then_some(value.version),
        }
    }
}

impl From<RegionEpochJson> for metapb::RegionEpoch {
    fn from(value: RegionEpochJson) -> Self {
        Self {
            conf_ver: value.conf_ver.unwrap_or_default(),
            version: value.version.unwrap_or_default(),
            ..Default::default()
        }
    }
}

mod region_epoch_option_json {
    use super::*;

    pub fn serialize<S>(
        value: &Option<Box<metapb::RegionEpoch>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_some(&RegionEpochJson::from(value.as_ref())),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<Box<metapb::RegionEpoch>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Option::<RegionEpochJson>::deserialize(deserializer)?
            .map(metapb::RegionEpoch::from)
            .map(Box::new))
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct StoreLabelJson {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    value: String,
}

impl From<&metapb::StoreLabel> for StoreLabelJson {
    fn from(value: &metapb::StoreLabel) -> Self {
        Self {
            key: value.key.clone(),
            value: value.value.clone(),
        }
    }
}

impl From<StoreLabelJson> for metapb::StoreLabel {
    fn from(value: StoreLabelJson) -> Self {
        Self {
            key: value.key,
            value: value.value,
            ..Default::default()
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct StoreJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<StoreLabelJson>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peer_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    git_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_timestamp: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deploy_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_heartbeat: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    physically_destroyed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node_state: Option<i32>,
}

impl StoreJson {
    fn is_empty(&self) -> bool {
        self.id.is_none()
            && self.address.is_none()
            && self.state.is_none()
            && self.labels.is_none()
            && self.version.is_none()
            && self.peer_address.is_none()
            && self.status_address.is_none()
            && self.git_hash.is_none()
            && self.start_timestamp.is_none()
            && self.deploy_path.is_none()
            && self.last_heartbeat.is_none()
            && self.physically_destroyed.is_none()
            && self.node_state.is_none()
    }

    fn into_store(self) -> Result<Option<Box<metapb::Store>>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        let state = match self.state.unwrap_or_default() {
            0 => metapb::StoreState::Up,
            1 => metapb::StoreState::Offline,
            2 => metapb::StoreState::Tombstone,
            value => return Err(format!("invalid metapb.StoreState value {value}")),
        };
        let node_state = match self.node_state.unwrap_or_default() {
            0 => metapb::NodeState::Preparing,
            1 => metapb::NodeState::Serving,
            2 => metapb::NodeState::Removing,
            3 => metapb::NodeState::Removed,
            value => return Err(format!("invalid metapb.NodeState value {value}")),
        };
        let mut store = metapb::Store {
            id: self.id.unwrap_or_default(),
            address: self.address.unwrap_or_default(),
            state,
            version: self.version.unwrap_or_default(),
            peer_address: self.peer_address.unwrap_or_default(),
            status_address: self.status_address.unwrap_or_default(),
            git_hash: self.git_hash.unwrap_or_default(),
            start_timestamp: self.start_timestamp.unwrap_or_default(),
            deploy_path: self.deploy_path.unwrap_or_default(),
            last_heartbeat: self.last_heartbeat.unwrap_or_default(),
            physically_destroyed: self.physically_destroyed.unwrap_or_default(),
            node_state,
            ..Default::default()
        };
        if let Some(labels) = self.labels {
            for label in labels {
                store.mut_labels().push(metapb::StoreLabel::from(label));
            }
        }
        Ok(Some(Box::new(store)))
    }
}

impl From<Option<&metapb::Store>> for StoreJson {
    fn from(value: Option<&metapb::Store>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        Self {
            id: (value.id != 0).then_some(value.id),
            address: (!value.address.is_empty()).then(|| value.address.clone()),
            state: ((value.state as i32) != 0).then_some(value.state as i32),
            labels: (!value.labels.is_empty())
                .then(|| value.labels.iter().map(StoreLabelJson::from).collect()),
            version: (!value.version.is_empty()).then(|| value.version.clone()),
            peer_address: (!value.peer_address.is_empty()).then(|| value.peer_address.clone()),
            status_address: (!value.status_address.is_empty())
                .then(|| value.status_address.clone()),
            git_hash: (!value.git_hash.is_empty()).then(|| value.git_hash.clone()),
            start_timestamp: (value.start_timestamp != 0).then_some(value.start_timestamp),
            deploy_path: (!value.deploy_path.is_empty()).then(|| value.deploy_path.clone()),
            last_heartbeat: (value.last_heartbeat != 0).then_some(value.last_heartbeat),
            physically_destroyed: value.physically_destroyed.then_some(true),
            node_state: ((value.node_state as i32) != 0).then_some(value.node_state as i32),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct MetaStoreJson {
    #[serde(flatten)]
    store: StoreJson,
    state_name: String,
}

impl Serialize for MetaStore {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        MetaStoreJson {
            store: StoreJson::from(self.Store.as_deref()),
            state_name: self.StateName.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MetaStore {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = MetaStoreJson::deserialize(deserializer)?;
        Ok(Self {
            Store: value.store.into_store().map_err(D::Error::custom)?,
            StateName: value.state_name,
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct PeerJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    store_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    is_witness: Option<bool>,
}

impl PeerJson {
    fn is_empty(&self) -> bool {
        self.id.is_none()
            && self.store_id.is_none()
            && self.role.is_none()
            && self.is_witness.is_none()
    }

    fn into_peer(self) -> Result<Option<Box<metapb::Peer>>, String> {
        if self.is_empty() {
            return Ok(None);
        }
        let role = match self.role.unwrap_or_default() {
            0 => metapb::PeerRole::Voter,
            1 => metapb::PeerRole::Learner,
            2 => metapb::PeerRole::IncomingVoter,
            3 => metapb::PeerRole::DemotingVoter,
            value => return Err(format!("invalid metapb.PeerRole value {value}")),
        };
        Ok(Some(Box::new(metapb::Peer {
            id: self.id.unwrap_or_default(),
            store_id: self.store_id.unwrap_or_default(),
            role,
            is_witness: self.is_witness.unwrap_or_default(),
            ..Default::default()
        })))
    }
}

impl From<Option<&metapb::Peer>> for PeerJson {
    fn from(value: Option<&metapb::Peer>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        Self {
            id: (value.id != 0).then_some(value.id),
            store_id: (value.store_id != 0).then_some(value.store_id),
            role: ((value.role as i32) != 0).then_some(value.role as i32),
            is_witness: value.is_witness.then_some(true),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct MetaPeerJson {
    #[serde(flatten)]
    peer: PeerJson,
    role_name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    is_learner: bool,
}

impl Serialize for MetaPeer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        MetaPeerJson {
            peer: PeerJson::from(self.Peer.as_deref()),
            role_name: self.RoleName.clone(),
            is_learner: self.IsLearner,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MetaPeer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = MetaPeerJson::deserialize(deserializer)?;
        Ok(Self {
            Peer: value.peer.into_peer().map_err(D::Error::custom)?,
            RoleName: value.role_name,
            IsLearner: value.is_learner,
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct PDPeerStatsJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    down_seconds: Option<u64>,
    peer: MetaPeer,
}

impl Serialize for PDPeerStats {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PDPeerStatsJson {
            down_seconds: self
                .PeerStats
                .as_deref()
                .and_then(|stats| (stats.down_seconds != 0).then_some(stats.down_seconds)),
            peer: self.Peer.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PDPeerStats {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = PDPeerStatsJson::deserialize(deserializer)?;
        Ok(Self {
            PeerStats: value.down_seconds.map(|down_seconds| {
                Box::new(pdpb::PeerStats {
                    down_seconds,
                    ..Default::default()
                })
            }),
            Peer: value.peer,
        })
    }
}
