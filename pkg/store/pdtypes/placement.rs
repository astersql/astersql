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

// PD Placement Rule：控制 Region Peer 在哪些 Store 上如何放置。
//
// Placement rule 按组（RuleGroup）组织，每条 Rule 指定角色（voter/leader 等）、
// 副本数、键范围与标签约束（LabelConstraint），供 PD 调度器匹配 Store。

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Rule is a PD placement rule checked against a region.
/// 针对 Region 校验的一条放置规则。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default = "Default::default")]
pub struct PlacementRule<const HTTP: bool> {
    /// 所属规则组 ID。
    #[serde(rename = "group_id")]
    pub GroupID: String,
    /// 规则自身 ID。
    #[serde(rename = "id")]
    pub ID: String,
    /// 组内排序索引；0 时序列化省略。
    #[serde(rename = "index", default, skip_serializing_if = "is_zero_i32")]
    pub Index: i32,
    /// 是否覆盖同组较低优先级规则。
    #[serde(rename = "override", default, skip_serializing_if = "is_false")]
    pub Override: bool,
    /// 规则适用键范围起点（原始字节，不序列化）。
    #[serde(skip)]
    pub StartKey: Vec<u8>,
    /// 起点的十六进制字符串（JSON 字段 start_key）。
    #[serde(rename = "start_key")]
    pub StartKeyHex: String,
    /// 规则适用键范围终点（原始字节，不序列化）。
    #[serde(skip)]
    pub EndKey: Vec<u8>,
    /// 终点的十六进制字符串（JSON 字段 end_key）。
    #[serde(rename = "end_key")]
    pub EndKeyHex: String,
    /// 期望的 Peer 角色。
    #[serde(rename = "role")]
    pub Role: PeerRoleType,
    /// Only pd/client/http.Rule includes witness metadata; local Go Rule ignores it.
    #[serde(
        rename = "is_witness",
        default,
        skip_serializing_if = "skip_witness::<HTTP>",
        deserialize_with = "deserialize_witness::<_, HTTP>"
    )]
    pub IsWitness: bool,
    /// 该角色需要的 Peer 数量。
    #[serde(rename = "count")]
    pub Count: i32,
    /// Store 标签约束列表。
    #[serde(
        rename = "label_constraints",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub LabelConstraints: Vec<LabelConstraint>,
    /// 用于隔离的位置标签键。
    #[serde(
        rename = "location_labels",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub LocationLabels: Vec<String>,
    /// 隔离级别（某个 location label）。
    #[serde(
        rename = "isolation_level",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub IsolationLevel: String,
    /// 规则版本号。
    #[serde(rename = "version", default, skip_serializing_if = "is_zero_u64")]
    pub Version: u64,
    /// 创建时间戳。
    #[serde(
        rename = "create_timestamp",
        default,
        skip_serializing_if = "is_zero_u64"
    )]
    pub CreateTimestamp: u64,
}

/// Matches pkg/store/pdtypes.Rule, which has no witness JSON field.
pub type Rule = PlacementRule<false>;
/// Witness-bearing Rule used by callers of Go's pd/client/http package.
pub type HttpRule = PlacementRule<true>;

fn skip_witness<const HTTP: bool>(_: &bool) -> bool {
    !HTTP
}

fn deserialize_witness<'de, D: Deserializer<'de>, const HTTP: bool>(
    deserializer: D,
) -> Result<bool, D::Error> {
    if HTTP {
        bool::deserialize(deserializer)
    } else {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| false)
    }
}

/// serde：`false` 时跳过序列化。
fn is_false(value: &bool) -> bool {
    !*value
}
/// serde：`0` 时跳过序列化。
fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}
/// serde：`0` 时跳过序列化。
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// PeerRoleType is the expected peer type of a placement rule.
/// 放置规则期望的 Peer 角色类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerRoleType {
    /// 投票副本（参与 Raft 多数派）。
    Voter,
    /// Leader 角色约束。
    Leader,
    /// Follower 角色约束。
    Follower,
    /// Learner：只同步日志，不投票。
    Learner,
    /// Forward-compatible role value accepted by Go's string alias.
    Unknown(String),
}

impl Default for PeerRoleType {
    fn default() -> Self {
        Self::Unknown(String::new())
    }
}

impl PeerRoleType {
    fn as_str(&self) -> &str {
        match self {
            Self::Voter => "voter",
            Self::Leader => "leader",
            Self::Follower => "follower",
            Self::Learner => "learner",
            Self::Unknown(value) => value,
        }
    }
}

impl Serialize for PeerRoleType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PeerRoleType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            match Option::<String>::deserialize(deserializer)?
                .unwrap_or_default()
                .as_str()
            {
                "voter" => Self::Voter,
                "leader" => Self::Leader,
                "follower" => Self::Follower,
                "learner" => Self::Learner,
                value => Self::Unknown(value.to_owned()),
            },
        )
    }
}

/// `PeerRoleType::Voter` 常量别名（对齐 Go）。
pub const Voter: PeerRoleType = PeerRoleType::Voter;
/// `PeerRoleType::Leader` 常量别名。
pub const Leader: PeerRoleType = PeerRoleType::Leader;
/// `PeerRoleType::Follower` 常量别名。
pub const Follower: PeerRoleType = PeerRoleType::Follower;
/// `PeerRoleType::Learner` 常量别名。
pub const Learner: PeerRoleType = PeerRoleType::Learner;

/// LabelConstraint filters stores when placing a peer.
/// 放置 Peer 时按 Store 标签过滤的约束。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelConstraint {
    /// 标签键。
    #[serde(rename = "key", default, skip_serializing_if = "String::is_empty")]
    pub Key: String,
    /// 匹配操作（in / notIn / exists / notExists）。
    #[serde(
        rename = "op",
        default,
        skip_serializing_if = "LabelConstraintOp::is_empty"
    )]
    pub Op: LabelConstraintOp,
    /// 操作数取值列表（in/notIn 使用）。
    #[serde(rename = "values", default, skip_serializing_if = "Vec::is_empty")]
    pub Values: Vec<String>,
}

/// RuleGroup defines properties of a rule group.
/// 规则组属性：组 ID、排序与是否覆盖。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleGroup {
    /// 规则组 ID。
    #[serde(rename = "id", default, skip_serializing_if = "String::is_empty")]
    pub ID: String,
    /// 组间排序索引。
    #[serde(rename = "index", default, skip_serializing_if = "is_zero_i32")]
    pub Index: i32,
    /// 是否覆盖其他组。
    #[serde(rename = "override", default, skip_serializing_if = "is_false")]
    pub Override: bool,
}

/// LabelConstraintOp defines how a label constraint matches a store.
/// 标签约束的匹配操作符。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LabelConstraintOp {
    /// 空操作（未设置）。
    #[default]
    Empty,
    /// 标签值落在 Values 集合内。
    In,
    /// 标签值不在 Values 集合内。
    NotIn,
    /// 标签键存在。
    Exists,
    /// 标签键不存在。
    NotExists,
    /// Forward-compatible operator value accepted by Go's string alias.
    Unknown(String),
}

impl LabelConstraintOp {
    /// 是否为空操作（用于 serde skip）。
    fn is_empty(&self) -> bool {
        self == &Self::Empty
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Empty => "",
            Self::In => "in",
            Self::NotIn => "notIn",
            Self::Exists => "exists",
            Self::NotExists => "notExists",
            Self::Unknown(value) => value,
        }
    }
}

impl Serialize for LabelConstraintOp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LabelConstraintOp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(
            match Option::<String>::deserialize(deserializer)?
                .unwrap_or_default()
                .as_str()
            {
                "" => Self::Empty,
                "in" => Self::In,
                "notIn" => Self::NotIn,
                "exists" => Self::Exists,
                "notExists" => Self::NotExists,
                value => Self::Unknown(value.to_owned()),
            },
        )
    }
}

/// `LabelConstraintOp::In` 常量别名。
pub const In: LabelConstraintOp = LabelConstraintOp::In;
/// `LabelConstraintOp::NotIn` 常量别名。
pub const NotIn: LabelConstraintOp = LabelConstraintOp::NotIn;
/// `LabelConstraintOp::Exists` 常量别名。
pub const Exists: LabelConstraintOp = LabelConstraintOp::Exists;
/// `LabelConstraintOp::NotExists` 常量别名。
pub const NotExists: LabelConstraintOp = LabelConstraintOp::NotExists;
