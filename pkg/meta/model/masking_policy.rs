// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 列级数据脱敏（masking）策略的元数据模型。
//
// 脱敏策略决定查询结果中敏感列如何被遮蔽或改写（如全掩码、部分掩码、置空）。
// 本模块只描述策略状态、类型与绑定信息，不执行实际脱敏表达式求值。

use serde::{Deserialize, Serialize};

use group_1::time::Time;

// MaskingPolicyStatus 对应 Go 的 byte 状态值，显式判别值保持历史 JSON/元数据兼容。
/// 脱敏策略启用状态；判别值须与已持久化 JSON/元数据一致。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MaskingPolicyStatus(pub u8);

impl MaskingPolicyStatus {
    /// 策略已禁用，查询不应用脱敏。
    pub const MaskingPolicyStatusDisable: Self = Self(0);
    /// 策略已启用。
    pub const MaskingPolicyStatusEnable: Self = Self(1);
}

// 旧名称在增量迁移期间继续作为别名导出，调用方无需同时适配命名变更。
/// 旧名别名：Disabled，等同于 MaskingPolicyStatusDisable。
pub const MaskingPolicyStatusDisabled: MaskingPolicyStatus =
    MaskingPolicyStatus::MaskingPolicyStatusDisable;
/// 旧名别名：Enabled，等同于 MaskingPolicyStatusEnable。
pub const MaskingPolicyStatusEnabled: MaskingPolicyStatus =
    MaskingPolicyStatus::MaskingPolicyStatusEnable;

impl MaskingPolicyStatus {
    // String 对应 Go fmt.Stringer；未知值与 Go 一样返回空串。
    /// 返回 SQL/展示用状态字符串（DISABLED / ENABLED）。
    pub fn String(self) -> &'static str {
        match self.0 {
            0 => "DISABLED",
            1 => "ENABLED",
            _ => "",
        }
    }
}

// MaskingPolicyType 对应 Go 的字符串类型，常量值直接参与持久化与 SQL 展示。
/// 脱敏类型字符串别名；取值写入元数据并参与 SHOW 展示。
pub type MaskingPolicyType = &'static str;
/// 全列掩码。
pub const MaskingPolicyTypeFull: MaskingPolicyType = "MASK_FULL";
/// 部分掩码（保留部分可见字符）。
pub const MaskingPolicyTypePartial: MaskingPolicyType = "MASK_PARTIAL";
/// 结果置为 NULL。
pub const MaskingPolicyTypeNull: MaskingPolicyType = "MASK_NULL";
/// 日期类脱敏。
pub const MaskingPolicyTypeDate: MaskingPolicyType = "MASK_DATE";
/// 自定义表达式脱敏。
pub const MaskingPolicyTypeCustom: MaskingPolicyType = "CUSTOM";

// 保留 Go 的旧常量别名，避免增量阶段出现两套字符串值。
/// 旧名别名：MaskFull。
pub const MaskingPolicyTypeMaskFull: MaskingPolicyType = MaskingPolicyTypeFull;
/// 旧名别名：MaskPartial。
pub const MaskingPolicyTypeMaskPartial: MaskingPolicyType = MaskingPolicyTypePartial;
/// 旧名别名：MaskNull。
pub const MaskingPolicyTypeMaskNull: MaskingPolicyType = MaskingPolicyTypeNull;
/// 旧名别名：MaskDate。
pub const MaskingPolicyTypeMaskDate: MaskingPolicyType = MaskingPolicyTypeDate;

// MaskingPolicyInfo 一比一保存策略目标、表达式、限制操作、审计字段及 schema 状态。
/// 单条脱敏策略的完整元数据：绑定目标、表达式、限制操作、审计与 schema 状态。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MaskingPolicyInfo {
    /// 策略稳定 ID。
    #[serde(rename = "id")]
    pub ID: i64,
    /// 策略名称（大小写不敏感）。
    #[serde(rename = "name")]
    pub Name: ast::CIStr,
    /// 目标库名。
    #[serde(rename = "db_name")]
    pub DBName: ast::CIStr,
    /// 目标表名。
    #[serde(rename = "table_name")]
    pub TableName: ast::CIStr,
    /// 目标表 ID。
    #[serde(rename = "table_id")]
    pub TableID: i64,
    /// 目标列名。
    #[serde(rename = "column_name")]
    pub ColumnName: ast::CIStr,
    /// 目标列 ID。
    #[serde(rename = "column_id")]
    pub ColumnID: i64,
    /// 脱敏表达式文本。
    #[serde(rename = "expression")]
    pub Expression: String,
    /// 启用/禁用状态。
    #[serde(rename = "status")]
    pub Status: MaskingPolicyStatus,
    /// 脱敏类型字符串（见 MaskingPolicyType 常量）。
    #[serde(rename = "masking_type", skip_serializing_if = "String::is_empty")]
    pub MaskingType: String,
    /// 限制可触发脱敏的操作集合。
    #[serde(rename = "restrict_ops", skip_serializing_if = "is_zero")]
    pub RestrictOps: ast::MaskingPolicyRestrictOps,
    /// 创建时间。
    #[serde(rename = "created_at")]
    pub CreatedAt: Time,
    /// 最近更新时间。
    #[serde(rename = "updated_at")]
    pub UpdatedAt: Time,
    /// 创建者。
    #[serde(rename = "created_by", skip_serializing_if = "String::is_empty")]
    pub CreatedBy: String,
    /// 最近更新者。
    #[serde(rename = "updated_by", skip_serializing_if = "String::is_empty")]
    pub UpdatedBy: String,
    /// schema 对象状态（如 Public / WriteOnly 等 DDL 中间态）。
    #[serde(rename = "state")]
    pub State: SchemaState,
}

fn is_zero(value: &ast::MaskingPolicyRestrictOps) -> bool {
    *value == 0
}

fn default_go_time() -> Time {
    Time::from_timestamp(-62_135_596_800, 0).expect("Go zero time is a valid chrono timestamp")
}

impl Default for MaskingPolicyInfo {
    /// Go 零值默认：时间戳取公元一年，其余字段为空或默认状态。
    fn default() -> Self {
        Self {
            ID: 0,
            Name: Default::default(),
            DBName: Default::default(),
            TableName: Default::default(),
            TableID: 0,
            ColumnName: Default::default(),
            ColumnID: 0,
            Expression: String::new(),
            Status: Default::default(),
            MaskingType: String::new(),
            RestrictOps: Default::default(),
            CreatedAt: default_go_time(),
            UpdatedAt: default_go_time(),
            CreatedBy: String::new(),
            UpdatedBy: String::new(),
            State: Default::default(),
        }
    }
}

impl MaskingPolicyInfo {
    // Clone 对应 Go 的值拷贝；所有字段均为拥有型值，因此派生 Clone 即是原浅拷贝语义。
    /// 值拷贝副本；字段均为拥有型，语义等同 Go 浅拷贝。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
}
