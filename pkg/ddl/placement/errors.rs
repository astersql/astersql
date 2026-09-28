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

// Placement（放置策略）模块的错误类型与哨兵错误消息。
//
// 解析 LabelConstraint（标签约束）、Constraints（约束集合）、
// Bundle（规则组）以及 Placement Options（放置选项）时，
// 用这些常量化错误消息标识失败原因，便于上层按字符串匹配或展示。

use std::fmt::{Display, Formatter};

/// Placement 解析/构造过程中的错误，内部保存可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(String);

impl Error {
    /// 由任意可转为字符串的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 将错误类别（哨兵常量）与细节拼成 `kind: detail` 形式的错误。
pub fn wrap(kind: &str, detail: impl Display) -> Error {
    Error::new(format!("{kind}: {detail}"))
}

/// 单条标签约束格式非法（期望 `{+|-}key=value`）。
pub const ErrInvalidConstraintFormat: &str =
    "label constraint should be in format '{+|-}key=value'";
/// 不支持的标签约束（例如强制 `+engine=tiflash`）。
pub const ErrUnsupportedConstraint: &str = "unsupported label constraint";
/// 多条约束互相冲突（同 key 的 In/NotIn 或取值矛盾）。
pub const ErrConflictingConstraints: &str = "conflicting label constraints";
/// map 语法约束中的副本数非法。
pub const ErrInvalidConstraintsMapcnt: &str =
    "label constraints in map syntax have invalid replicas";
/// 约束集合整体格式非法（如 YAML 解析失败）。
pub const ErrInvalidConstraintsFormat: &str = "invalid label constraints format";
/// 存活偏好（survival preference）格式非法。
pub const ErrInvalidSurvivalPreferenceFormat: &str =
    "survival preference format should be in format [xxx=yyy, ...]";
/// 约束携带的 REPLICAS（副本数）非法。
pub const ErrInvalidConstraintsReplicas: &str = "label constraints with invalid REPLICAS";
/// Bundle ID 内容非法（前缀正确但 ID 非正整数）。
pub const ErrInvalidBundleID: &str = "invalid bundle ID";
/// Bundle ID 格式非法（缺少 `TiDB_DDL_` 前缀等）。
pub const ErrInvalidBundleIDFormat: &str = "invalid bundle ID format";
/// ROLE=leader 时 REPLICAS 必须为 1。
pub const ErrLeaderReplicasMustOne: &str = "REPLICAS must be 1 if ROLE=leader";
/// 规则未指定 ROLE 字段。
pub const ErrMissingRoleField: &str = "the ROLE field is not specified";
/// 按角色删除规则时找不到对应规则。
pub const ErrNoRulesToDrop: &str = "no rule of such role to drop";
/// Placement 选项整体非法。
pub const ErrInvalidPlacementOptions: &str = "invalid placement option";
/// 约束映射分隔符错误（应为 `": "`）。
pub const ErrInvalidConstraintsMappingWrongSeparator: &str =
    "mappings use a colon and space (“: ”) to mark each key/value pair";
/// 约束映射中未找到冒号分隔符。
pub const ErrInvalidConstraintsMappingNoColonFound: &str = "no colon found";
