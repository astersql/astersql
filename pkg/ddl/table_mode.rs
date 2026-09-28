// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 表模式（Table Mode）元数据与访问控制。
//
// 表模式用于在数据导入（Import）或备份恢复（Restore，如 BR/Lightning）期间
// 保护表结构：处于 Import/Restore 时禁止普通读写与 DDL，仅允许元数据查询
// 与 checksum 类操作，避免导入过程中被并发修改破坏一致性。
//
// 主要内容：
// - [`TableMode`]：Normal / Import / Restore 三种模式；
// - [`alter_table_mode`] / [`on_alter_table_mode`]：模式切换与版本递增；
// - [`table_mode_allows`]：按模式判定某类操作是否允许。

/// 表的运行模式，影响允许执行的操作集合。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableMode {
    /// 普通模式：读写与 DDL 均允许。
    #[default]
    Normal,
    /// 导入模式（如 IMPORT INTO / Lightning 导入中）：仅允许元数据与 checksum。
    Import,
    /// 恢复模式（如 BR restore 中）：权限与 Import 相同，且二者不可直接互转。
    Restore,
}
/// 表模式相关的精简表元信息（测试与状态机用）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    /// 所属 schema（数据库）ID。
    pub schema_id: i64,
    /// 表 ID；为 0 视为表不存在。
    pub table_id: i64,
    /// 当前表模式。
    pub mode: TableMode,
    /// 元数据版本；每次成功切换模式时递增。
    pub version: i64,
}
/// 切换表模式。
///
/// 返回 `Ok(true)` 表示实际发生了切换并递增了 version；
/// `Ok(false)` 表示目标模式与当前相同（幂等）；
/// Import ↔ Restore 直接互转非法。
pub fn alter_table_mode(table: &mut TableInfo, requested: TableMode) -> Result<bool, String> {
    if table.table_id == 0 {
        return Err("table not found".into());
    }
    // 目标与当前相同：幂等成功，不递增版本。
    if table.mode == requested {
        return Ok(false);
    }
    // Import 与 Restore 互转非法，必须先回到 Normal。
    if matches!(
        (table.mode, requested),
        (TableMode::Restore, TableMode::Import) | (TableMode::Import, TableMode::Restore)
    ) {
        return Err(format!(
            "invalid mode transition from {:?} to {:?}",
            table.mode, requested
        ));
    }
    table.mode = requested;
    table.version += 1;
    Ok(true)
}
/// DDL job 入口：校验 schema/table ID 后切换模式。
///
/// 实际发生切换时返回最新 version；同模式与 Go `onAlterTableMode`
/// 一致按 no-op 完成，不更新 schema 并返回零值 version。
pub fn on_alter_table_mode(
    table: &mut TableInfo,
    schema_id: i64,
    table_id: i64,
    mode: TableMode,
) -> Result<i64, String> {
    if table.schema_id != schema_id || table.table_id != table_id {
        return Err("schema or table ID mismatch".into());
    }
    if !alter_table_mode(table, mode)? {
        return Ok(0);
    }
    Ok(table.version)
}

/// 按表模式划分的访问操作类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableOperation {
    /// 元数据类（SHOW / DESCRIBE / CREATE LIKE 等）。
    Metadata,
    /// 校验和（ADMIN CHECKSUM TABLE）。
    Checksum,
    /// 普通读（SELECT 等）。
    Read,
    /// 普通写（INSERT/UPDATE/DELETE 等）。
    Write,
    /// 结构变更（ALTER TABLE 等）。
    Alter,
    /// 删除表。
    Drop,
}

/// 判断给定表模式下是否允许执行某类操作。
///
/// Normal 允许全部操作；Import/Restore 仅允许 Metadata 与 Checksum。
pub fn table_mode_allows(mode: TableMode, operation: TableOperation) -> bool {
    match mode {
        TableMode::Normal => true,
        TableMode::Import | TableMode::Restore => {
            matches!(
                operation,
                TableOperation::Metadata | TableOperation::Checksum
            )
        }
    }
}
