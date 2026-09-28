// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Schema（数据库）级 DDL 的内存目录实现。
//
// 提供创建/修改字符集与排序规则、修改 Placement Policy（放置策略）、
// 分阶段删除（Public → WriteOnly → DeleteOnly → None）以及从 dropped
// 缓存恢复库的能力，由 `SchemaCatalog` 提供。

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Schema（库）在 DDL 状态机中的可见性阶段。
///
/// 在线删除库时按 Public → WriteOnly → DeleteOnly → None 推进：
/// WriteOnly 禁止新写入，DeleteOnly 仅允许删除，None 表示元数据已移除。
pub enum SchemaState {
    Public,
    WriteOnly,
    DeleteOnly,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 内存中的数据库元信息。
pub struct DatabaseInfo {
    /// 库的全局唯一 ID。
    pub id: i64,
    /// 库名（展示用原始大小写）。
    pub name: String,
    /// 默认字符集。
    pub charset: String,
    /// 默认排序规则（collation）。
    pub collation: String,
    /// 默认 Placement Policy 名称；None 表示未绑定放置策略。
    pub placement_policy: Option<String>,
    /// 当前 schema 状态。
    pub state: SchemaState,
    /// 库内表摘要列表（含分区物理 ID）。
    pub tables: Vec<SchemaTable>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 库目录中登记的表摘要，用于收集物理 ID（表 ID 与分区 ID）。
pub struct SchemaTable {
    /// 逻辑表 ID。
    pub id: i64,
    /// 分区表各分区的物理 ID；非分区表为空。
    pub partition_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// SchemaCatalog 操作失败原因。
pub enum SchemaError {
    /// 库已存在且未指定 IF NOT EXISTS。
    AlreadyExists,
    /// 目标库不存在。
    NotFound,
    /// 字符集与排序规则不匹配或不受支持。
    InvalidCharsetCollation,
    /// Placement Policy 名称为空串等非法值。
    InvalidPlacementPolicy,
    /// 恢复时发现同名库已存在。
    RecoveryConflict,
}

/// 以 Debug 形式展示错误，便于测试断言。
impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SchemaError {}

#[derive(Default)]
/// 内存 schema 目录：维护存活库与已删除库缓存。
pub struct SchemaCatalog {
    /// 下一个可分配的库 ID。
    next_id: i64,
    /// 小写库名 → 存活库信息。
    schemas: BTreeMap<String, DatabaseInfo>,
    /// 已删除库按 ID 缓存，供 recover 使用。
    dropped: BTreeMap<i64, DatabaseInfo>,
}

impl SchemaCatalog {
    /// 创建库；`if_not_exists` 为 true 时已存在则返回 Ok(None)。
    pub fn create_schema(
        &mut self,
        name: &str,
        charset: &str,
        collation: &str,
        placement_policy: Option<String>,
        if_not_exists: bool,
    ) -> Result<Option<i64>, SchemaError> {
        // 库名按 ASCII 小写做唯一键，兼容大小写不敏感匹配。
        let key = name.to_ascii_lowercase();
        if self.schemas.contains_key(&key) {
            return if if_not_exists {
                Ok(None)
            } else {
                Err(SchemaError::AlreadyExists)
            };
        }
        validate_charset_and_collation(charset, collation)?;
        validate_placement_policy(placement_policy.as_deref())?;
        // 分配从 1 起的单调库 ID。
        self.next_id = self.next_id.saturating_add(1).max(1);
        let id = self.next_id;
        self.schemas.insert(
            key,
            DatabaseInfo {
                id,
                name: name.to_string(),
                charset: charset.to_ascii_lowercase(),
                collation: collation.to_ascii_lowercase(),
                placement_policy,
                state: SchemaState::Public,
                tables: Vec::new(),
            },
        );
        Ok(Some(id))
    }

    /// 修改库的默认字符集与排序规则；返回是否实际发生变更。
    pub fn modify_charset_and_collation(
        &mut self,
        name: &str,
        charset: &str,
        collation: &str,
    ) -> Result<bool, SchemaError> {
        let info = self
            .schemas
            .get_mut(&name.to_ascii_lowercase())
            .ok_or(SchemaError::NotFound)?;
        // Go onModifySchemaCharsetAndCollate resolves the database before it
        // examines or applies the requested values, so preserve that error order.
        validate_charset_and_collation(charset, collation)?;
        let changed = !info.charset.eq_ignore_ascii_case(charset)
            || !info.collation.eq_ignore_ascii_case(collation);
        info.charset = charset.to_ascii_lowercase();
        info.collation = collation.to_ascii_lowercase();
        Ok(changed)
    }

    /// 修改或清除库的默认 Placement Policy；返回是否实际发生变更。
    pub fn modify_placement(
        &mut self,
        name: &str,
        placement: Option<String>,
    ) -> Result<bool, SchemaError> {
        let info = self
            .schemas
            .get_mut(&name.to_ascii_lowercase())
            .ok_or(SchemaError::NotFound)?;
        // Go onModifySchemaDefaultPlacement checks schema existence before
        // validating the referenced placement policy.
        validate_placement_policy(placement.as_deref())?;
        let changed = info.placement_policy != placement;
        info.placement_policy = placement;
        Ok(changed)
    }

    /// 推进删除库的一个状态步；到达 None 时移入 dropped 缓存。
    pub fn drop_schema_step(&mut self, name: &str) -> Result<SchemaState, SchemaError> {
        let key = name.to_ascii_lowercase();
        let state = self.schemas.get(&key).ok_or(SchemaError::NotFound)?.state;
        // 状态机：Public → WriteOnly → DeleteOnly → None。
        let next = match state {
            SchemaState::Public => SchemaState::WriteOnly,
            SchemaState::WriteOnly => SchemaState::DeleteOnly,
            SchemaState::DeleteOnly => SchemaState::None,
            SchemaState::None => SchemaState::None,
        };
        // 最终步：从存活目录移除并记入 dropped，便于后续 recover。
        if next == SchemaState::None {
            let mut info = self.schemas.remove(&key).ok_or(SchemaError::NotFound)?;
            info.state = SchemaState::None;
            self.dropped.insert(info.id, info);
        } else if let Some(info) = self.schemas.get_mut(&key) {
            info.state = next;
        }
        Ok(next)
    }

    /// 按 ID 从 dropped 缓存恢复库；若同名库已存在则冲突。
    pub fn recover_schema(&mut self, schema_id: i64) -> Result<(), SchemaError> {
        let mut info = self
            .dropped
            .remove(&schema_id)
            .ok_or(SchemaError::NotFound)?;
        let key = info.name.to_ascii_lowercase();
        // 同名库已存在：把记录放回 dropped，避免丢弃可恢复数据。
        if self.schemas.contains_key(&key) {
            self.dropped.insert(schema_id, info);
            return Err(SchemaError::RecoveryConflict);
        }
        info.state = SchemaState::Public;
        self.schemas.insert(key, info);
        Ok(())
    }

    /// 按名查找存活库（大小写不敏感）。
    pub fn schema(&self, name: &str) -> Option<&DatabaseInfo> {
        self.schemas.get(&name.to_ascii_lowercase())
    }
}

/// 校验字符集与排序规则前缀是否匹配（如 utf8mb4 对应 utf8mb4_*）。
pub fn validate_charset_and_collation(charset: &str, collation: &str) -> Result<(), SchemaError> {
    let charset = charset.to_ascii_lowercase();
    let collation = collation.to_ascii_lowercase();
    let valid = match charset.as_str() {
        "utf8mb4" => collation.starts_with("utf8mb4_"),
        "utf8" => collation.starts_with("utf8_") && !collation.starts_with("utf8mb4_"),
        "latin1" => collation.starts_with("latin1_"),
        "ascii" => collation.starts_with("ascii_"),
        "binary" => collation == "binary",
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(SchemaError::InvalidCharsetCollation)
    }
}

/// 校验 Placement Policy：允许 None，但不允许空白字符串。
pub fn validate_placement_policy(policy: Option<&str>) -> Result<(), SchemaError> {
    if policy.is_some_and(|value| value.trim().is_empty()) {
        Err(SchemaError::InvalidPlacementPolicy)
    } else {
        Ok(())
    }
}

/// 收集表 ID 及其分区物理 ID，供 DropSchema 等清理路径使用。
pub fn schema_physical_ids(tables: &[SchemaTable]) -> Vec<i64> {
    let mut ids = Vec::new();
    for table in tables {
        ids.push(table.id);
        ids.extend(table.partition_ids.iter().copied());
    }
    ids
}
