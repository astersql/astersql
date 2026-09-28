// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Placement-rule helpers matching `br/pkg/pdutil/utils.go`.
//!
//! 本文件对应 Go `utils.go`：为 BR 提供 PD placement rule 查询与表级规则匹配。
//! 核心数据流是 HTTP 拉取规则 → JSON 反序列化 → 按 tableID/角色搜索。
//! 键空间编解码对齐 TiDB memcomparable / tablecodec，以便与 PD 存的 StartKeyHex 一致。
//! UndoFunc/Nop 则服务于调度器暂停后的可回滚闭包契约（与 pd.rs 协作）。
//! 与 Go 差异点：HTTP 客户端通过 trait 注入，便于单测；编解码在本文件内联实现。

use std::sync::Arc;

use astersql_br_pkg_errors::ErrPDInvalidResponse;
use astersql_errors::{Annotate, SharedError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::pd::Context;

/// UndoFunc is a 'undo' operation of some undoable command.
/// Mirrors Go `type UndoFunc func(context.Context) error`.
/// 可撤销操作的回滚闭包：接收 Context，失败时返回 SharedError。
pub type UndoFunc = Arc<dyn Fn(Context) -> Result<(), SharedError> + Send + Sync>;

/// Nop is the 'zero value' of undo func.
/// 空回滚：无副作用，用作“无需恢复”时的零值。
pub fn Nop(_ctx: Context) -> Result<(), SharedError> {
    Ok(())
}

/// Returns an owned Nop undo function.
/// 包装为 Arc，便于与需要拥有 UndoFunc 的 API 组合。
pub fn nop_undo() -> UndoFunc {
    Arc::new(Nop)
}

/// PeerRoleType mirrors `pdtypes.PeerRoleType`.
/// 副本角色枚举；serde rename 必须与 PD JSON 字段字面量一致。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerRoleType {
    Voter,
    Leader,
    Follower,
    Learner,
    /// Go uses a string alias and therefore preserves roles added by PD later.
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
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PeerRoleType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "voter" => Self::Voter,
            "leader" => Self::Leader,
            "follower" => Self::Follower,
            "learner" => Self::Learner,
            value => Self::Unknown(value.to_owned()),
        })
    }
}

// 与 Go 导出常量同名，降低迁移时符号替换成本。
pub const Voter: PeerRoleType = PeerRoleType::Voter;
pub const Leader: PeerRoleType = PeerRoleType::Leader;
pub const Follower: PeerRoleType = PeerRoleType::Follower;
pub const Learner: PeerRoleType = PeerRoleType::Learner;

/// LabelConstraintOp mirrors the string alias in Go's `pdtypes` package.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum LabelConstraintOp {
    #[default]
    Empty,
    In,
    NotIn,
    Exists,
    NotExists,
    Unknown(String),
}

impl LabelConstraintOp {
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
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LabelConstraintOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match String::deserialize(deserializer)?.as_str() {
            "" => Self::Empty,
            "in" => Self::In,
            "notIn" => Self::NotIn,
            "exists" => Self::Exists,
            "notExists" => Self::NotExists,
            value => Self::Unknown(value.to_owned()),
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LabelConstraint {
    #[serde(rename = "key", default)]
    pub Key: String,
    #[serde(rename = "op", default)]
    pub Op: LabelConstraintOp,
    #[serde(rename = "values", default)]
    pub Values: Vec<String>,
}

/// Rule mirrors Go `pdtypes.Rule` without dropping response fields.
/// JSON 名与 PD `/config/rules` 响应对齐，原始键字段与 Go 一样不参与 JSON。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    // 规则所属 placement group，PD 侧用于分组管理。
    #[serde(rename = "group_id", default)]
    pub GroupID: String,
    // 规则唯一 id，搜索命中后常用于断言或删除。
    #[serde(rename = "id", default)]
    pub ID: String,
    #[serde(rename = "index", default)]
    pub Index: i64,
    #[serde(rename = "override", default)]
    pub Override: bool,
    #[serde(skip)]
    pub StartKey: Vec<u8>,
    // PD 以十六进制字符串存储 memcomparable 起止键。
    #[serde(rename = "start_key", default)]
    pub StartKeyHex: String,
    #[serde(skip)]
    pub EndKey: Vec<u8>,
    #[serde(rename = "end_key", default)]
    pub EndKeyHex: String,
    // 该规则约束的 peer 角色（voter/leader/follower/learner）。
    #[serde(rename = "role", default)]
    pub Role: PeerRoleType,
    // 期望副本数；BR 匹配逻辑当前主要依赖 StartKey 与 Role。
    #[serde(rename = "count", default)]
    pub Count: i64,
    #[serde(rename = "label_constraints", default)]
    pub LabelConstraints: Vec<LabelConstraint>,
    #[serde(rename = "location_labels", default)]
    pub LocationLabels: Vec<String>,
    #[serde(rename = "isolation_level", default)]
    pub IsolationLevel: String,
    #[serde(rename = "version", default)]
    pub Version: u64,
    #[serde(rename = "create_timestamp", default)]
    pub CreateTimestamp: u64,
}

/// HTTP GET abstraction for GetPlacementRules (PD network boundary).
/// 注入 HTTP 边界，便于单测 mock，避免真实拨号 PD。
pub trait PlacementHttpClient: Send + Sync {
    fn get_placement_rules(&self, ctx: &Context, url: &str) -> Result<(u16, Vec<u8>), SharedError>;
}

/// PD placement rules HTTP path (`pd.PlacementRules`).
/// 与 PD API v1 路径常量保持一致。
pub const PLACEMENT_RULES_PATH: &str = "/pd/api/v1/config/rules";

/// GetPlacementRules return the current placement rules.
///
/// `use_tls` mirrors Go's `tlsConf != nil` scheme choice. The HTTP client is
/// injected so tests can mock the PD network boundary without dialing.
/// 拉取当前 placement rules：按 TLS 选择 scheme，412 视为规则未启用并返回空列表。
pub fn GetPlacementRules(
    ctx: &Context,
    pdAddr: &str,
    use_tls: bool,
    cli: &dyn PlacementHttpClient,
) -> Result<Vec<Rule>, SharedError> {
    // Go: tlsConf != nil → https，否则 http。
    let prefix = if use_tls { "https://" } else { "http://" };
    let reqURL = format!("{prefix}{pdAddr}{PLACEMENT_RULES_PATH}");
    let (status, body) = cli.get_placement_rules(ctx, &reqURL)?;
    if status == 412 {
        // http.StatusPreconditionFailed — placement rules disabled.
        // PD 未开启 placement rule 时返回 412；BR 按空规则集继续。
        return Ok(Vec::new());
    }
    if status != 200 {
        // 非 200 包装为 ErrPDInvalidResponse，文案对齐 Go Annotate 格式。
        let msg = String::from_utf8_lossy(&body);
        return Err(Annotate(
            Some(SharedError::new((*ErrPDInvalidResponse).clone())),
            format!("get placement rules failed: resp={msg}, err=, code={status}"),
        )
        .expect("annotate"));
    }
    // 响应体必须是 Rule 数组 JSON；反序列化失败直接上抛。
    let rules: Vec<Rule> = serde_json::from_slice(&body)
        .map_err(|e| astersql_errors::New(format!("unmarshal placement rules: {e}")))?;
    Ok(rules)
}

/// SearchPlacementRule returns the placement rule matched to the table or None.
/// 按 tableID 与角色在规则列表中查找；键解码失败的规则被跳过而非报错。
pub fn SearchPlacementRule<'a>(
    tableID: i64,
    placementRules: &'a [Rule],
    role: PeerRoleType,
) -> Option<&'a Rule> {
    for rule in placementRules {
        // StartKeyHex → 原始字节 → memcomparable 解码 → 提取 table id。
        let key = match hex::decode(&rule.StartKeyHex) {
            Ok(key) => key,
            Err(_) => continue,
        };
        let decoded = match decode_bytes(&key) {
            Ok(decoded) => decoded,
            Err(_) => continue,
        };
        if rule.Role == role && tableID == decode_table_id(&decoded) {
            return Some(rule);
        }
    }
    None
}

// memcomparable 分组长度与填充标记，对齐 TiDB codec。
const ENC_GROUP_SIZE: usize = 8;
const ENC_MARKER: u8 = 0xFF;
const TABLE_PREFIX: &[u8] = b"t";

/// EncodeBytes matches TiDB memcomparable encoding used by PD keys.
/// 将原始键编码为 PD 使用的 memcomparable 形式（每 8 字节一组 + 填充标记）。
pub fn encode_bytes(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity((data.len() / ENC_GROUP_SIZE + 1) * (ENC_GROUP_SIZE + 1));
    let d_len = data.len();
    let mut idx = 0;
    // 含最后一组：即使数据耗尽也要输出全填充组以终止。
    while idx <= d_len {
        let remain = d_len - idx;
        let pad_count;
        if remain >= ENC_GROUP_SIZE {
            result.extend_from_slice(&data[idx..idx + ENC_GROUP_SIZE]);
            pad_count = 0;
        } else {
            pad_count = ENC_GROUP_SIZE - remain;
            result.extend_from_slice(&data[idx..]);
            result.extend(std::iter::repeat_n(0u8, pad_count));
        }
        // marker = 0xFF - pad_count，解码时据此还原真实长度。
        result.push(ENC_MARKER - pad_count as u8);
        idx += ENC_GROUP_SIZE;
    }
    result
}

// 逆变换 encode_bytes；遇非零填充或非法 marker 返回错误。
fn decode_bytes(b: &[u8]) -> Result<Vec<u8>, SharedError> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if i + ENC_GROUP_SIZE >= b.len() {
            return Err(astersql_errors::New("insufficient bytes to decode value"));
        }
        let group = &b[i..i + ENC_GROUP_SIZE];
        let marker = b[i + ENC_GROUP_SIZE];
        let pad_count = (ENC_MARKER - marker) as usize;
        if pad_count > ENC_GROUP_SIZE {
            return Err(astersql_errors::New("invalid marker"));
        }
        let real = ENC_GROUP_SIZE - pad_count;
        result.extend_from_slice(&group[..real]);
        for &p in &group[real..] {
            if p != 0 {
                return Err(astersql_errors::New("invalid padding"));
            }
        }
        i += ENC_GROUP_SIZE + 1;
        // 有填充表示这是最后一组，结束解码。
        if pad_count != 0 {
            break;
        }
    }
    Ok(result)
}

// 有符号整数的 memcomparable 编码：翻转最高位后按大端写出。
fn encode_int(v: i64) -> [u8; 8] {
    let u = (v as u64) ^ (1u64 << 63);
    u.to_be_bytes()
}

fn decode_int(b: &[u8]) -> Result<(usize, i64), SharedError> {
    if b.len() < 8 {
        return Err(astersql_errors::New("insufficient bytes to decode int"));
    }
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&b[..8]);
    // 与 encode_int 对称：再异或一次还原有符号值。
    let u = u64::from_be_bytes(raw) ^ (1u64 << 63);
    Ok((8, u as i64))
}

/// DecodeTableID mirrors tablecodec.DecodeTableID for raw (decoded) keys.
/// 从已解码键中提取 table id；支持可选的 keyspace 前缀 `x????`。
pub fn decode_table_id(key: &[u8]) -> i64 {
    let mut key = key.to_vec();
    if !key.starts_with(TABLE_PREFIX) {
        // API V2 键带 4 字节 keyspace 前缀（首字节 'r' 或 'x'），剥掉后再认 `t`。
        if key.len() >= 4 && matches!(key[0], b'r' | b'x') {
            key = key[4..].to_vec();
        } else {
            return 0;
        }
        if !key.starts_with(TABLE_PREFIX) {
            return 0;
        }
    }
    // 解码失败按 Go 习惯返回 0，表示“非表键/不可识别”。
    match decode_int(&key[TABLE_PREFIX.len()..]) {
        Ok((_, table_id)) => table_id,
        Err(_) => 0,
    }
}

/// Build a memcomparable StartKeyHex for table `table_id` (test / rule helper).
/// 构造某 table 的 StartKeyHex，供测试或规则生成使用。
pub fn table_start_key_hex(table_id: i64) -> String {
    let mut raw = Vec::with_capacity(1 + 8);
    raw.extend_from_slice(TABLE_PREFIX);
    raw.extend_from_slice(&encode_int(table_id));
    hex::encode(encode_bytes(&raw))
}
