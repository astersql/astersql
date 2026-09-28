// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Schema/name helpers ported from `br/pkg/utils/schema.go`.
//! 库表名引号、临时系统库前缀与 AutoID 判定；与 Go `schema.go` 语义对齐。

use astersql_meta_model::TableInfo;
use astersql_parser_ast::{CIStr, NewCIStr};
use astersql_parser_mysql::r#const::{SysDB, SystemDB, WorkloadSchema};

/// Prefix applied to temporary system databases during restore.
/// 恢复时为系统库加此前缀，避免与在线库名冲突。
pub const temporaryDBNamePrefix: &str = "__TiDB_BR_Temporary_";

/// Returns whether the table needs backing up with an autoid.
/// 非聚簇主键（隐式 `_tidb_rowid`）或存在自增列时需要备份 AutoID。
pub fn NeedAutoID(tbl_info: &TableInfo) -> bool {
    let has_row_id = !tbl_info.PKIsHandle && !tbl_info.IsCommonHandle;
    let has_auto_inc_id = tbl_info.GetAutoIncrementColInfo().is_some();
    has_row_id || has_auto_inc_id
}

/// Formats a SQL identifier with backticks.
/// 反引号内的 `` ` `` 需加倍转义，与 MySQL 标识符规则一致。
pub fn EncloseName(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// Removes surrounding backticks from a SQL identifier.
/// 仅剥最外层一对反引号，再把内部 `` `` `` 还原为单个 `` ` ``。
pub fn UnquoteName(name: &str) -> String {
    let mut name = name.to_string();
    if name.len() >= 2 && name.as_bytes()[0] == b'`' && name.as_bytes()[name.len() - 1] == b'`' {
        name = name[1..name.len() - 1].to_string();
    }
    name.replace("``", "`")
}

/// Formats database and table names for SQL.
/// 库名与表名分别加引号后以 `.` 连接，供 DDL/DML 拼接。
pub fn EncloseDBAndTable(database: &str, table: &str) -> String {
    format!("{}.{}", EncloseName(database), EncloseName(table))
}

/// Returns whether the database name is a temporary system database.
/// 只认 `mysql`/`sys` 的临时形态，其它库即使带前缀也不算模板系统库。
pub fn IsTemplateSysDB(dbname: &CIStr) -> bool {
    dbname.O == format!("{temporaryDBNamePrefix}{SystemDB}")
        || dbname.O == format!("{temporaryDBNamePrefix}{SysDB}")
}

/// Returns whether the database is a system database.
/// 含 `mysql`、`sys`、`workload_schema`，入参应为小写库名。
pub fn IsSysDB(db_lower_name: &str) -> bool {
    db_lower_name == SystemDB || db_lower_name == SysDB || db_lower_name == WorkloadSchema
}

/// Builds a temporary system database name.
/// 生成带 BR 临时前缀的 `CIStr`，供恢复写元数据时使用。
pub fn TemporaryDBName(db: &str) -> CIStr {
    NewCIStr(&format!("{temporaryDBNamePrefix}{db}"))
}

/// Strips the temporary database prefix when present.
/// 无前缀则原样返回；有前缀则剥掉，不报告是否发生剥离。
pub fn StripTempDBPrefixIfNeeded(temp_db: &str) -> String {
    if !temp_db.starts_with(temporaryDBNamePrefix) {
        return temp_db.to_string();
    }
    temp_db[temporaryDBNamePrefix.len()..].to_string()
}

/// Strips the temporary database prefix and reports whether stripping happened.
/// 与 `StripTempDBPrefixIfNeeded` 相同剥离逻辑，额外返回是否真正去掉了前缀。
pub fn StripTempDBPrefix(temp_db: &str) -> (String, bool) {
    if !temp_db.starts_with(temporaryDBNamePrefix) {
        return (temp_db.to_string(), false);
    }
    (temp_db[temporaryDBNamePrefix.len()..].to_string(), true)
}

/// Returns whether the database is a system DB, including temporary prefixes.
/// 先剥离临时前缀再做系统库判定，覆盖恢复中的临时系统库名。
pub fn IsSysOrTempSysDB(db: &str) -> bool {
    IsSysDB(&StripTempDBPrefixIfNeeded(db))
}

/// Returns the original system database CIStr after stripping a temporary prefix.
/// 同时改写 `O`/`L`，保证大小写不敏感比较仍指向真实系统库名。
pub fn GetSysDBCIStrName(mut temp_db: CIStr) -> (CIStr, bool) {
    if !temp_db.O.starts_with(temporaryDBNamePrefix) {
        return (temp_db, false);
    }
    temp_db.O = temp_db.O[temporaryDBNamePrefix.len()..].to_string();
    temp_db.L = temp_db.L[temporaryDBNamePrefix.len()..].to_string();
    (temp_db, true)
}
