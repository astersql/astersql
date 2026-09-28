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

//! Schema version upgrade/downgrade for stats_meta matching
//! `systable_schema_update.go`.
//! 负责 mysql.stats_meta 临时表的 schema 升降级，对齐 Go 同名文件。
//! 下游/上游 TableInfo 版本不一致时，对 `__TiDB_BR_Temporary_mysql.stats_meta` 执行 ALTER。
//! 版本靠列 `last_stats_histograms_version` 是否存在区分；SQL 经回调注入以便测试捕获。
//! 未知 schema/table 不注册更新函数，避免误改其它系统表。

use crate::stubs::{Error, Result, model};

/// stats_meta schema 版本枚举：Invalid 为哨兵，V1/V2 对应列集差异。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(i32)]
pub enum SchemaVersionType {
    InvalidVersion = 0,
    Version1 = 1,
    Version2 = 2,
}

/// 按 (schema, table, ver) 返回升级 SQL；未知组合返回 None。
/// ver=0 空串表示无需变更的台阶，与 Go upgradeSQLs 表一致。
pub fn upgrade_sqls(schema: &str, table: &str, ver: usize) -> Option<&'static str> {
    match (schema, table, ver) {
        ("mysql", "stats_meta", 0) => Some(""),
        ("mysql", "stats_meta", 1) => Some(
            "ALTER TABLE __TiDB_BR_Temporary_mysql.stats_meta ADD COLUMN IF NOT EXISTS last_stats_histograms_version bigint unsigned DEFAULT NULL",
        ),
        _ => None,
    }
}

/// 降级 SQL：从高版本台阶回退时 DROP 新增列，保证临时表可与上游对齐。
pub fn downgrade_sqls(schema: &str, table: &str, ver: usize) -> Option<&'static str> {
    match (schema, table, ver) {
        ("mysql", "stats_meta", 0) => Some(""),
        ("mysql", "stats_meta", 1) => Some(
            "ALTER TABLE __TiDB_BR_Temporary_mysql.stats_meta DROP COLUMN IF EXISTS last_stats_histograms_version",
        ),
        _ => None,
    }
}

/// 扫描列名推断版本：存在 last_stats_histograms_version → V2，否则 V1。
pub fn getSchemaVersionFromStatsMeta(table_info: &model::TableInfo) -> SchemaVersionType {
    for column_info in &table_info.Columns {
        if column_info.Name.L == "last_stats_histograms_version" {
            return SchemaVersionType::Version2;
        }
    }
    SchemaVersionType::Version1
}

/// 比较上下游版本并按需执行升降级；任一为 Invalid 直接报错。
/// 下游版本更低时走 downgrade（从 upstream-1 递减到 downstream）；
/// 下游更高时走 upgrade（从 upstream 递增到 downstream-1）。
pub fn updateStatsMetaSchema(
    downstream_table_info: &model::TableInfo,
    upstream_table_info: &model::TableInfo,
    mut execution: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let downstream_version = getSchemaVersionFromStatsMeta(downstream_table_info);
    let upstream_version = getSchemaVersionFromStatsMeta(upstream_table_info);
    if downstream_version == SchemaVersionType::InvalidVersion
        || upstream_version == SchemaVersionType::InvalidVersion
    {
        // 非法版本不可继续，避免对临时表发出无定义 SQL。
        return Err(Error::Errorf("invalid stats meta schema"));
    }
    if downstream_version == upstream_version {
        // 版本一致无需 ALTER，保持幂等。
        return Ok(());
    }
    let table_name = downstream_table_info.Name.L.as_str();
    if (downstream_version as i32) < (upstream_version as i32) {
        // Downgrade: walk ver from upstream-1 down to downstream inclusive.
        // 下游旧、上游新：临时表需 DROP 列以匹配上游恢复路径。
        let mut ver = (upstream_version as i32) - 1;
        while ver >= downstream_version as i32 {
            let sql = downgrade_sqls("mysql", table_name, ver as usize).unwrap_or("");
            if !sql.is_empty() {
                execution(sql)?;
            }
            ver -= 1;
        }
    } else {
        // 下游新、上游旧：临时表需 ADD 列以承接下游 schema。
        let mut ver = upstream_version as i32;
        while ver < downstream_version as i32 {
            let sql = upgrade_sqls("mysql", table_name, ver as usize).unwrap_or("");
            if !sql.is_empty() {
                execution(sql)?;
            }
            ver += 1;
        }
    }
    Ok(())
}

/// Dispatch map entry matching Go `updateStatsMetaSchemaFunctionMap`.
/// 仅 mysql.stats_meta 注册更新函数，其它系统表返回 None 跳过。
pub fn update_stats_meta_schema_function_map(
    schema: &str,
    table: &str,
) -> Option<
    fn(&model::TableInfo, &model::TableInfo, &mut dyn FnMut(&str) -> Result<()>) -> Result<()>,
> {
    match (schema, table) {
        ("mysql", "stats_meta") => Some(|down, up, exec| updateStatsMetaSchema(down, up, exec)),
        _ => None,
    }
}
