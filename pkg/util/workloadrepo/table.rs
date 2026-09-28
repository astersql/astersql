// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载仓库目标表的 DDL/DML 语句构建与建表检查。
//
// 对应 Go `pkg/util/workloadrepo/table.go`。根据源表列定义生成 `CREATE TABLE`
// 与 `INSERT…SELECT`，并在 owner 节点创建带 RANGE 分区的历史表；通过 information_schema
// 侧的分区信息判断表是否已就绪。

use crate::worker::{repositoryTable, worker as Worker};
use crate::*;
use chrono::{DateTime, Days, Local};

/// 将标识符用反引号包裹，并对内部反引号做转义。
fn identifier(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

/// 根据源表列定义拼出目标历史表的 `CREATE TABLE IF NOT EXISTS` 语句。
///
/// 快照表额外包含 `SNAP_ID`；元数据表不应走此路径。
pub fn buildCreateQuery(
    backend: &dyn RepositoryBackend,
    table: &repositoryTable,
) -> Result<String, String> {
    // Go resolves the source table before rejecting metadata tables, so preserve
    // that lookup/error ordering as part of the observable contract.
    let columns = backend.source_columns(&table.schema, &table.table)?;
    if table.tableType == metadataTable {
        return Err("buildCreateQuery invoked on metadataTable".into());
    }
    let mut output = format!(
        "CREATE TABLE IF NOT EXISTS `{workloadSchema}`.{} (",
        identifier(&table.destTable)
    );
    // 快照表用 SNAP_ID 关联一次全局快照批次。
    if table.tableType == snapshotTable {
        output.push_str("`SNAP_ID` INT UNSIGNED NOT NULL, ");
    }
    output.push_str("`TS` DATETIME NOT NULL, `INSTANCE_ID` VARCHAR(64) DEFAULT NULL");
    for column in columns {
        output.push_str(&format!(
            ", {} {} COMMENT '{}'",
            identifier(&column.name),
            column.type_description,
            column.comment.replace('\'', "''")
        ));
    }
    output.push_str(") DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin");
    Ok(output)
}

/// 构建并缓存 `INSERT…SELECT` 语句到 `table.insertStmt`。
///
/// 快照表绑定两个占位符（SNAP_ID、INSTANCE_ID）；采样表仅绑定 INSTANCE_ID。
pub fn buildInsertQuery(
    backend: &dyn RepositoryBackend,
    table: &mut repositoryTable,
) -> Result<(), String> {
    // Match Go's source-table lookup and error propagation order.
    let columns = backend.source_columns(&table.schema, &table.table)?;
    if table.tableType == metadataTable {
        return Err("buildInsertQuery invoked on metadataTable".into());
    }
    let mut names = Vec::new();
    if table.tableType == snapshotTable {
        names.push("`SNAP_ID`".into());
    }
    names.extend(["`TS`".into(), "`INSTANCE_ID`".into()]);
    names.extend(columns.iter().map(|column| identifier(&column.name)));
    let mut select = if table.tableType == snapshotTable {
        "%?, now(), %?".to_string()
    } else {
        "now(), %?".to_string()
    };
    for column in &columns {
        select.push_str(&format!(", {}", identifier(&column.name)));
    }
    table.insertStmt = format!(
        "INSERT `{workloadSchema}`.{} ({}) SELECT {select} FROM {}.{}{}",
        identifier(&table.destTable),
        names.join(", "),
        identifier(&table.schema),
        identifier(&table.table),
        if table.whereClause.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", table.whereClause)
        }
    );
    Ok(())
}

impl Worker {
    /// Owner 节点创建全部缺失的目标表，并追加近期分区定义。
    pub fn createAllTables(&self, now: DateTime<Local>) -> Result<(), String> {
        for table in self.workloadTables.lock().unwrap().clone() {
            if self.backend.table_exists(&table.destTable) {
                continue;
            }
            let mut statement = if table.createStmt.is_empty() {
                buildCreateQuery(self.backend.as_ref(), &table)?
            } else {
                table.createStmt.clone()
            };
            // 元数据表按 BEGIN_TIME 分区，其余按 TS 分区。
            generatePartitionDef(
                &mut statement,
                if table.tableType == metadataTable {
                    "BEGIN_TIME"
                } else {
                    "TS"
                },
                now,
            )?;
            execRetry(self.backend.as_ref(), &statement, &[])?;
        }
        self.createAllPartitions(now)
    }

    /// 检查所有目标表是否存在且分区覆盖到足够未来日期。
    pub fn checkTablesExists(&self, now: DateTime<Local>) -> bool {
        self.workloadTables
            .lock()
            .unwrap()
            .iter()
            .all(|table| checkTableExistsByIS(self.backend.as_ref(), &table.destTable, Some(now)))
    }
}

/// 通过后端查询判断表是否存在；若提供 `now`，还要求最晚分区日期晚于 `now+1` 天。
pub fn checkTableExistsByIS(
    backend: &dyn RepositoryBackend,
    tableName: &str,
    now: Option<DateTime<Local>>,
) -> bool {
    if !backend.table_exists(tableName) {
        return false;
    }
    let Some(now) = now else {
        return true;
    };
    let Ok(partitions) = backend.partitions(tableName) else {
        return false;
    };
    let Some(last) = partitions.last() else {
        return false;
    };
    // 要求至少预留到明天之后的分区，避免写入时分区缺失。
    let Some(tomorrow) = now.checked_add_days(Days::new(1)) else {
        return false;
    };
    parsePartitionName(last).is_ok_and(|date| date > tomorrow)
}
