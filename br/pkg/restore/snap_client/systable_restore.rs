// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! System table restore helpers matching `systable_restore.go`.
//! 系统表恢复：用户/权限/统计等特殊表的过滤、重命名与兼容性检查。
//! 区分不可恢复 schema/表、可重命名系统表与 stats 临时表。
//! 校对权限表 collation 兼容 SQL，避免恢复后权限语义错乱。
//! SchemaVersionPair 承载上下游版本对，驱动后续 schema 更新。
//! 对齐 Go `systable_restore.go` 的集合与映射定义。
//! sysUserTableName 与权限表映射定义哪些 mysql 表可特殊处理。
//! unrecoverable_table/schema 集合阻止危险系统对象被覆盖。
//! renameable_sys_tables 与 stats_tables 决定临时表命名策略。
//! plan_replayer_tables 等集合随版本演进，变更需同步 Go。
//! collateCompatibilityTables 与 SQLPair 生成校对修复语句。
//! SchemaVersionPairT 连接上下游版本，驱动 schema update。
//! nested/set_of 辅助构造集合，保持插入顺序或去重语义清晰。
//! 特权表行校对兼容失败应阻断恢复，而不是警告后继续。
//! 临时表检查器区分 stats 与 renameable 前缀，避免误判。
//! 与 Go systable_restore.go 的集合字面量应定期 diff。
//! 补充要点1：sysUserTableName 与权限表映射定义哪些 mysql 表可特殊处理。
//! 补充要点2：unrecoverable_table/schema 集合阻止危险系统对象被覆盖。
//! 补充要点3：renameable_sys_tables 与 stats_tables 决定临时表命名策略。
//! 补充要点4：plan_replayer_tables 等集合随版本演进，变更需同步 Go。
//! 补充要点5：collateCompatibilityTables 与 SQLPair 生成校对修复语句。
//! 补充要点6：SchemaVersionPairT 连接上下游版本，驱动 schema update。

use std::collections::{HashMap, HashSet};

use crate::stubs::{
    Error, Result, StripTempDBPrefix, SystemDB, TemporaryDBName, WorkloadSchema, berrors, log,
    model,
};
use crate::systable_schema_update::update_stats_meta_schema_function_map;

/// `sysUserTableName`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const sysUserTableName: &str = "user";

/// `set_of`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn set_of(names: &[&str]) -> HashMap<String, ()> {
    names.iter().map(|n| ((*n).to_string(), ())).collect()
}

/// `nested`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn nested(entries: &[(&str, &[&str])]) -> HashMap<String, HashMap<String, ()>> {
    entries
        .iter()
        .map(|(db, tables)| ((*db).to_string(), set_of(tables)))
        .collect()
}

/// `plan_replayer_tables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn plan_replayer_tables() -> HashMap<String, HashMap<String, ()>> {
    nested(&[("mysql", &["plan_replayer_status", "plan_replayer_task"])])
}

/// `stats_tables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn stats_tables() -> HashMap<String, HashMap<String, ()>> {
    nested(&[(
        "mysql",
        &[
            "stats_buckets",
            "stats_extended",
            "stats_feedback",
            "stats_fm_sketch",
            "stats_histograms",
            "stats_history",
            "stats_meta",
            "stats_meta_history",
            "stats_table_locked",
            "stats_top_n",
            "column_stats_usage",
        ],
    )])
}

/// `renameable_sys_tables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn renameable_sys_tables() -> HashMap<String, HashMap<String, ()>> {
    nested(&[(
        "mysql",
        &[
            "bind_info",
            "user",
            "db",
            "tables_priv",
            "columns_priv",
            "default_roles",
            "role_edges",
            "global_priv",
            "global_grants",
        ],
    )])
}

/// Tables restored when fullClusterRestore=true.
/// `sysPrivilegeTableMap`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn sysPrivilegeTableMap() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("user", "(user = '%s' and host = '%%')"),
        ("db", "(user = '%s' and host = '%%')"),
        ("tables_priv", "(user = '%s' and host = '%%')"),
        ("columns_priv", "(user = '%s' and host = '%%')"),
        ("default_roles", "(user = '%s' and host = '%%')"),
        ("role_edges", "(to_user = '%s' and to_host = '%%')"),
        ("global_priv", "(user = '%s' and host = '%%')"),
        ("global_grants", "(user = '%s' and host = '%%')"),
    ])
}

/// `unrecoverable_table`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn unrecoverable_table() -> HashMap<String, HashMap<String, ()>> {
    nested(&[
        (
            "mysql",
            &[
                "tidb",
                "global_variables",
                "capture_plan_baselines_blacklist",
                "advisory_locks",
                "analyze_jobs",
                "analyze_options",
                "dist_framework_meta",
                "tidb_global_task",
                "tidb_global_task_history",
                "tidb_background_subtask",
                "tidb_background_subtask_history",
                "tidb_ddl_history",
                "tidb_ddl_job",
                "tidb_ddl_reorg",
                "tidb_ddl_notifier",
                "tidb_import_jobs",
                "help_topic",
                "request_unit_by_group",
                "table_cache_meta",
                "tidb_runaway_queries",
                "tidb_runaway_watch",
                "tidb_runaway_watch_done",
                "tidb_ttl_job_history",
                "tidb_ttl_table_status",
                "tidb_ttl_task",
                "tidb_timers",
                "gc_delete_range",
                "gc_delete_range_done",
                "index_advisor_results",
                "tidb_mdl_info",
                "tidb_mdl_view",
                "tidb_pitr_id_map",
                "tidb_restore_registry",
                "tidb_masking_policy",
            ],
        ),
        ("sys", &["schema_unused_indexes"]),
    ])
}

#[derive(Clone, Debug)]
/// `checkPrivilegeTableRowsCollateCompatibilitySQLPair`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct checkPrivilegeTableRowsCollateCompatibilitySQLPair {
    pub upstreamCollateSQL: &'static str,
    pub downstreamCollateSQL: &'static str,
    pub columns: HashSet<&'static str>,
}

/// `collateCompatibilityTables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn collateCompatibilityTables()
-> HashMap<&'static str, HashMap<&'static str, checkPrivilegeTableRowsCollateCompatibilitySQLPair>>
{
    HashMap::from([(
        "mysql",
        HashMap::from([
            (
                "db",
                checkPrivilegeTableRowsCollateCompatibilitySQLPair {
                    upstreamCollateSQL: "SELECT COUNT(1) FROM __TiDB_BR_Temporary_mysql.db",
                    downstreamCollateSQL: "SELECT COUNT(1) FROM (SELECT Host, DB COLLATE utf8mb4_general_ci, User FROM __TiDB_BR_Temporary_mysql.db GROUP BY Host, DB COLLATE utf8mb4_general_ci, User) as a",
                    columns: HashSet::from(["db"]),
                },
            ),
            (
                "tables_priv",
                checkPrivilegeTableRowsCollateCompatibilitySQLPair {
                    upstreamCollateSQL: "SELECT COUNT(1) FROM __TiDB_BR_Temporary_mysql.tables_priv",
                    downstreamCollateSQL: "SELECT COUNT(1) FROM (SELECT Host, DB COLLATE utf8mb4_general_ci, User, Table_name COLLATE utf8mb4_general_ci FROM __TiDB_BR_Temporary_mysql.tables_priv GROUP BY Host, DB COLLATE utf8mb4_general_ci, User, Table_name COLLATE utf8mb4_general_ci) as a",
                    columns: HashSet::from(["db", "table_name"]),
                },
            ),
            (
                "columns_priv",
                checkPrivilegeTableRowsCollateCompatibilitySQLPair {
                    upstreamCollateSQL: "SELECT COUNT(1) FROM __TiDB_BR_Temporary_mysql.columns_priv",
                    downstreamCollateSQL: "SELECT COUNT(1) FROM (SELECT Host, DB COLLATE utf8mb4_general_ci, User, Table_name COLLATE utf8mb4_general_ci, Column_name COLLATE utf8mb4_general_ci FROM __TiDB_BR_Temporary_mysql.columns_priv GROUP BY Host, DB COLLATE utf8mb4_general_ci, User, Table_name COLLATE utf8mb4_general_ci, Column_name COLLATE utf8mb4_general_ci) as a",
                    columns: HashSet::from(["db", "table_name", "column_name"]),
                },
            ),
        ]),
    )])
}

/// `unrecoverable_schema`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn unrecoverable_schema() -> HashSet<&'static str> {
    HashSet::from([WorkloadSchema])
}

#[derive(Clone, Debug, Default)]
/// `SchemaVersionPairT`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct SchemaVersionPairT {
    pub UpstreamVersionMajor: i64,
    pub UpstreamVersionMinor: i64,
    pub DownstreamVersionMajor: i64,
    pub DownstreamVersionMinor: i64,
}

impl SchemaVersionPairT {
    /// `UpstreamVersion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn UpstreamVersion(&self) -> String {
        format!(
            "{}.{}",
            self.UpstreamVersionMajor, self.UpstreamVersionMinor
        )
    }

    /// `DownstreamVersion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn DownstreamVersion(&self) -> String {
        format!(
            "{}.{}",
            self.DownstreamVersionMajor, self.DownstreamVersionMinor
        )
    }
}

/// `InfoSchema`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait InfoSchema: Send + Sync {
    /// `TableInfoByName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn TableInfoByName(&self, schema: &str, table: &str) -> Result<model::TableInfo>;
}

/// `updateStatsTableSchema`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn updateStatsTableSchema(
    renamed_tables: &HashMap<String, HashMap<String, ()>>,
    info_schema: &dyn InfoSchema,
    mut execution: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    for (schema_name, table_names) in renamed_tables {
        for table_name in table_names.keys() {
            let Some(update_fn) = update_stats_meta_schema_function_map(schema_name, table_name)
            else {
                continue;
            };
            let downstream = info_schema
                .TableInfoByName(schema_name, table_name)
                .map_err(|e| {
                    Error::Annotatef(
                        e,
                        format!(
                            "failed to get downstream table info, schema: {schema_name}, table: {table_name}"
                        ),
                    )
                })?;
            let upstream_schema = TemporaryDBName(schema_name);
            let upstream = info_schema
                .TableInfoByName(&upstream_schema, table_name)
                .map_err(|e| {
                    Error::Annotatef(
                        e,
                        format!(
                            "failed to get upstream table info, schema: {upstream_schema}, table: {table_name}"
                        ),
                    )
                })?;
            update_fn(&downstream, &upstream, &mut execution).map_err(|e| {
                Error::Annotatef(
                    e,
                    format!(
                        "failed to update stats table schema, schema: {schema_name}, table: {table_name}"
                    ),
                )
            })?;
        }
    }
    Ok(())
}

/// `notifyUpdateAllUsersPrivilege`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn notifyUpdateAllUsersPrivilege(
    renamed_tables: &HashMap<String, HashMap<String, ()>>,
    notifier: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let priv_map = sysPrivilegeTableMap();
    for (db_name, renamed_table) in renamed_tables {
        if db_name != SystemDB {
            continue;
        }
        for table_name in renamed_table.keys() {
            if priv_map.contains_key(table_name.as_str()) {
                if let Err(err) = notifier() {
                    log::Warn(
                        "failed to flush privileges, please manually execute `FLUSH PRIVILEGES`",
                    );
                    return Err(berrors::ErrUnknown(format!(
                        "failed to flush privileges: {err}"
                    )));
                }
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Exported for tests / export_test.
/// `NotifyUpdateAllUsersPrivilege`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn NotifyUpdateAllUsersPrivilege(
    renamed_tables: HashMap<String, HashMap<String, ()>>,
    notifier: impl FnOnce() -> Result<()>,
) -> Result<()> {
    notifyUpdateAllUsersPrivilege(&renamed_tables, notifier)
}

/// `isUnrecoverableTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn isUnrecoverableTable(schema_name: &str, table_name: &str) -> bool {
    if unrecoverable_schema().contains(schema_name) {
        return true;
    }
    unrecoverable_table()
        .get(schema_name)
        .map(|m| m.contains_key(table_name))
        .unwrap_or(false)
}

/// `IsStatsTemporaryTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn IsStatsTemporaryTable(temp_schema_name: &str, table_name: &str) -> bool {
    GetDBNameIfStatsTemporaryTable(temp_schema_name, table_name).1
}

/// `GetDBNameIfStatsTemporaryTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn GetDBNameIfStatsTemporaryTable(temp_schema_name: &str, table_name: &str) -> (String, bool) {
    let (db_name, ok) = StripTempDBPrefix(temp_schema_name);
    if ok && isStatsTable(&db_name, table_name) {
        (db_name, true)
    } else {
        (String::new(), false)
    }
}

/// `IsRenameableSysTemporaryTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn IsRenameableSysTemporaryTable(temp_schema_name: &str, table_name: &str) -> bool {
    GetDBNameIfRenameableSysTemporaryTable(temp_schema_name, table_name).1
}

/// `GetDBNameIfRenameableSysTemporaryTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn GetDBNameIfRenameableSysTemporaryTable(
    temp_schema_name: &str,
    table_name: &str,
) -> (String, bool) {
    let (db_name, ok) = StripTempDBPrefix(temp_schema_name);
    if ok && isRenameableSysTable(&db_name, table_name) {
        (db_name, true)
    } else {
        (String::new(), false)
    }
}

#[derive(Clone, Debug, Default)]
/// `TemporaryTableChecker`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct TemporaryTableChecker {
    pub loadStatsPhysical: bool,
    pub loadSysTablePhysical: bool,
}

impl TemporaryTableChecker {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn new(load_stats_physical: bool, load_sys_table_physical: bool) -> Self {
        Self {
            loadStatsPhysical: load_stats_physical,
            loadSysTablePhysical: load_sys_table_physical,
        }
    }

    /// `CheckTemporaryTables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn CheckTemporaryTables(&self, temp_schema_name: &str, table_name: &str) -> (String, bool) {
        if self.loadStatsPhysical {
            let (db, ok) = GetDBNameIfStatsTemporaryTable(temp_schema_name, table_name);
            if ok {
                return (db, true);
            }
        }
        if self.loadSysTablePhysical {
            let (db, ok) = GetDBNameIfRenameableSysTemporaryTable(temp_schema_name, table_name);
            if ok {
                return (db, true);
            }
        }
        (String::new(), false)
    }
}

/// `NewTemporaryTableChecker`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn NewTemporaryTableChecker(
    load_stats_physical: bool,
    load_sys_table_physical: bool,
) -> TemporaryTableChecker {
    TemporaryTableChecker::new(load_stats_physical, load_sys_table_physical)
}

/// `GenerateMoveRenamedTableSQLPair`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn GenerateMoveRenamedTableSQLPair(
    restore_ts: u64,
    statistic_tables: &HashMap<String, HashMap<String, ()>>,
) -> String {
    let mut rename_buffer = Vec::with_capacity(32);
    for (db_name, table_names) in statistic_tables {
        for table_name in table_names.keys() {
            let temp = TemporaryDBName(db_name);
            rename_buffer.push(format!(
                "{db_name}.{table_name} TO {temp}.{table_name}_deleted_{restore_ts}"
            ));
            rename_buffer.push(format!("{temp}.{table_name} TO {db_name}.{table_name}"));
        }
    }
    format!("RENAME TABLE {}", rename_buffer.join(","))
}

/// `isStatsTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn isStatsTable(schema_name: &str, table_name: &str) -> bool {
    stats_tables()
        .get(schema_name)
        .map(|m| m.contains_key(table_name))
        .unwrap_or(false)
}

/// `isRenameableSysTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn isRenameableSysTable(schema_name: &str, table_name: &str) -> bool {
    renameable_sys_tables()
        .get(schema_name)
        .map(|m| m.contains_key(table_name))
        .unwrap_or(false)
}

/// `isPlanReplayerTables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn isPlanReplayerTables(schema_name: &str, table_name: &str) -> bool {
    plan_replayer_tables()
        .get(schema_name)
        .map(|m| m.contains_key(table_name))
        .unwrap_or(false)
}

/// `removeUserResourceGroup`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn removeUserResourceGroup(
    db_name: &str,
    mut exec_sql: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let sql = format!(
        "UPDATE {} SET User_attributes = JSON_REMOVE(User_attributes, '$.resource_group');",
        crate::stubs::EncloseDBAndTable(db_name, sysUserTableName)
    );
    if let Err(err) = exec_sql(&sql) {
        if !err
            .msg
            .contains("Unknown column 'User_attributes' in 'field list'")
        {
            return Err(err);
        }
        log::Warn(
            "remove resource group meta failed, please ensure target cluster is newer than v6.6.0",
        );
    }
    Ok(())
}

/// `checkSysTableColumnCollateCompatibility`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn checkSysTableColumnCollateCompatibility(
    db_name_l: &str,
    table_name_l: &str,
    column_name_l: &str,
    upstream_collate: &str,
    downstream_collate: &str,
) -> bool {
    if upstream_collate != "utf8mb4_bin" || downstream_collate != "utf8mb4_general_ci" {
        return false;
    }
    collateCompatibilityTables()
        .get(db_name_l)
        .and_then(|tables| tables.get(table_name_l))
        .map(|pair| pair.columns.contains(column_name_l))
        .unwrap_or(false)
}

/// `CheckSysTableCompatibility`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn CheckSysTableCompatibility(
    downstream_tables: &[model::TableInfo],
    upstream_tables: &[model::TableInfo],
    collation_check: bool,
) -> Result<bool> {
    let mut can_load = true;
    for up in upstream_tables {
        let down = downstream_tables
            .iter()
            .find(|table| table.Name.L == up.Name.L)
            .ok_or_else(|| Error::new(format!("missed system table: {}", up.Name.O)))?;

        if down.Columns.len() != up.Columns.len() && up.Name.L != sysUserTableName {
            return Err(Error::new(format!(
                "column count mismatch, table: {}, col in cluster: {}, col in backup: {}",
                up.Name.O,
                down.Columns.len(),
                up.Columns.len()
            )));
        }

        for down_col in &down.Columns {
            let Some(up_col) = up
                .Columns
                .iter()
                .find(|column| column.Name.L == down_col.Name.L)
            else {
                if up.Name.L == sysUserTableName {
                    can_load = false;
                    continue;
                }
                return Err(Error::new(format!(
                    "missing column in backup data, table: {}, col: {}",
                    up.Name.O, down_col.Name.O
                )));
            };

            let collate_eq = up_col.Collate == down_col.Collate;
            can_load = can_load && collate_eq;
            let collate_compatible = collate_eq
                || (collation_check
                    && checkSysTableColumnCollateCompatibility(
                        "mysql",
                        &down.Name.L,
                        &down_col.Name.L,
                        &up_col.Collate,
                        &down_col.Collate,
                    ));
            if !collate_compatible {
                return Err(Error::new(format!(
                    "incompatible column, table: {}, col in cluster: {}, col in backup: {}",
                    up.Name.O, down_col.Name.O, up_col.Name.O
                )));
            }
        }

        if up.Name.L == sysUserTableName {
            for up_col in &up.Columns {
                if !down
                    .Columns
                    .iter()
                    .any(|column| column.Name.L == up_col.Name.L)
                {
                    return Err(Error::new(format!(
                        "missing column in cluster data, table: {}, col: {}",
                        up.Name.O, up_col.Name.O
                    )));
                }
            }
        }
    }
    Ok(can_load)
}
