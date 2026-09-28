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

// 数据库（schema）元数据模型。
//
// 定义 `DBInfo` 及其已废弃的表列表字段，并提供深拷贝、浅拷贝与按库名比较。

use super::{PolicyRefInfo, SchemaState, TableInfo, ast};
use std::collections::HashMap;
use std::sync::Arc;

/// 历史兼容字段：曾内嵌在 `DBInfo` 中的表列表（现通过 infoschema 等路径维护）。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DeprecatedDBInfo {
    /// 该库下表元数据的共享引用列表。
    #[serde(skip)]
    pub Tables: Vec<Arc<TableInfo>>,
}

/// 数据库（schema）级元数据：ID、名称、字符集、状态、放置策略引用等。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DBInfo {
    /// 数据库唯一 ID。
    #[serde(rename = "id")]
    pub ID: i64,
    /// 库名（大小写敏感原串与小写形式，见 `CIStr`）。
    #[serde(rename = "db_name")]
    pub Name: ast::CIStr,
    /// 默认字符集。
    #[serde(rename = "charset")]
    pub Charset: String,
    /// 默认校对规则（collation）。
    #[serde(rename = "collate")]
    pub Collate: String,
    /// 已废弃的内嵌表列表容器。
    pub Deprecated: DeprecatedDBInfo,
    /// Schema 状态机当前态（如 Public、DeleteOnly 等）。
    #[serde(rename = "state")]
    pub State: SchemaState,
    /// 可选的放置策略（placement policy）引用。
    #[serde(rename = "policy_ref_info")]
    pub PlacementPolicyRef: Option<PolicyRefInfo>,
    /// 表名（小写）到表 ID 的快速查找表。
    #[serde(skip)]
    pub TableName2ID: HashMap<String, i64>,
}

impl DBInfo {
    /// 深拷贝：对 `Deprecated.Tables` 中每张表再执行 `TableInfo::Clone`，断开 `Arc` 共享。
    pub fn Clone(&self) -> Self {
        let mut result = self.clone();
        // 对每张表做独立 Clone，避免与源库共享同一 `Arc<TableInfo>`。
        result.Deprecated.Tables = self
            .Deprecated
            .Tables
            .iter()
            .map(|table| Arc::new(table.Clone()))
            .collect();
        result
    }
    /// 浅拷贝：直接 `clone`，表列表仍共享同一批 `Arc`。
    pub fn Copy(&self) -> Self {
        self.clone()
    }
}

/// 按库名小写形式比较两个 `DBInfo`：小于返回 -1，相等 0，大于 1。
pub fn LessDBInfo(a: &DBInfo, b: &DBInfo) -> i32 {
    match a.Name.L.cmp(&b.Name.L) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}
