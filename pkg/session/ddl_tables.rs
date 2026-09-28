// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! DDL system-table bootstrap, matching Go `session.InitDDLTables`.

#![allow(non_snake_case, non_upper_case_globals)]

use astersql_meta::{DDLTableVersion, Mutator};
use astersql_parser::Parser;
use astersql_parser_ast as ast;

pub use crate::bootstrap::TableBasicInfo;

pub const DDLJobTables: [TableBasicInfo; 3] = [
    TableBasicInfo {
        id: astersql_meta_metadef::TiDBDDLJobTableID,
        name: "tidb_ddl_job",
        create_sql: astersql_meta_metadef::CreateTiDBDDLJobTable,
    },
    TableBasicInfo {
        id: astersql_meta_metadef::TiDBDDLReorgTableID,
        name: "tidb_ddl_reorg",
        create_sql: astersql_meta_metadef::CreateTiDBReorgTable,
    },
    TableBasicInfo {
        id: astersql_meta_metadef::TiDBDDLHistoryTableID,
        name: "tidb_ddl_history",
        create_sql: astersql_meta_metadef::CreateTiDBDDLHistoryTable,
    },
];

pub const MDLTables: [TableBasicInfo; 1] = [TableBasicInfo {
    id: astersql_meta_metadef::TiDBMDLInfoTableID,
    name: "tidb_mdl_info",
    create_sql: astersql_meta_metadef::CreateTiDBMDLTable,
}];

pub const BackfillTables: [TableBasicInfo; 2] = [
    TableBasicInfo {
        id: astersql_meta_metadef::TiDBBackgroundSubtaskTableID,
        name: "tidb_background_subtask",
        create_sql: astersql_meta_metadef::CreateTiDBBackgroundSubtaskTable,
    },
    TableBasicInfo {
        id: astersql_meta_metadef::TiDBBackgroundSubtaskHistoryTableID,
        name: "tidb_background_subtask_history",
        create_sql: astersql_meta_metadef::CreateTiDBBackgroundSubtaskHistoryTable,
    },
];

pub const DDLNotifierTables: [TableBasicInfo; 1] = [TableBasicInfo {
    id: astersql_meta_metadef::TiDBDDLNotifierTableID,
    name: "tidb_ddl_notifier",
    create_sql: astersql_meta_metadef::CreateTiDBDDLNotifierTable,
}];

fn create_tables(
    mutator: &mut Mutator,
    database_id: i64,
    tables: &[TableBasicInfo],
) -> Result<(), String> {
    for table in tables {
        let statement = Parser::default()
            .ParseOneStmt(table.create_sql, "", "")
            .map_err(|error| format!("parse {} DDL: {error}", table.name))?;
        let create = statement
            .as_any()
            .downcast_ref::<ast::CreateTableStmt>()
            .ok_or_else(|| format!("{} DDL is not CREATE TABLE", table.name))?;
        let context =
            astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(Vec::new());
        let built = astersql_ddl::BuildTableInfoFromAST(&context, create)
            .map_err(|error| format!("build {} metadata: {error}", table.name))?;
        if built.Name.L != table.name {
            return Err(format!(
                "{} DDL built unexpected table name {}",
                table.name, built.Name.L
            ));
        }
        mutator
            .create_table_or_view(
                database_id,
                &astersql_meta::model::TableInfo {
                    id: table.id,
                    name: astersql_meta::ast::CiString::new(table.name),
                    ..astersql_meta::model::TableInfo::default()
                },
            )
            .map_err(|error| format!("create {} metadata: {error}", table.name))?;
    }
    Ok(())
}

/// Create every DDL system table newer than the stored version and advance the
/// version only after all required tables have been written.
pub fn InitDDLTables(mutator: &mut Mutator) -> Result<(), String> {
    let current_version = mutator
        .get_ddl_table_version()
        .map_err(|error| format!("read DDL table version: {error}"))?;
    let database_id = mutator
        .create_mysql_database_if_not_exists()
        .map_err(|error| format!("create mysql database: {error}"))?;

    let versioned_tables: [(i32, &[TableBasicInfo]); 4] = [
        (DDLTableVersion::Base as i32, &DDLJobTables),
        (DDLTableVersion::Mdl as i32, &MDLTables),
        (DDLTableVersion::Backfill as i32, &BackfillTables),
        (DDLTableVersion::DdlNotifier as i32, &DDLNotifierTables),
    ];
    let mut largest_version = current_version;
    for (target_version, tables) in versioned_tables {
        if current_version >= target_version {
            continue;
        }
        create_tables(mutator, database_id, tables)?;
        largest_version = largest_version.max(target_version);
    }

    if largest_version > current_version {
        let final_version = match largest_version {
            value if value == DDLTableVersion::Base as i32 => DDLTableVersion::Base,
            value if value == DDLTableVersion::Mdl as i32 => DDLTableVersion::Mdl,
            value if value == DDLTableVersion::Backfill as i32 => DDLTableVersion::Backfill,
            _ => DDLTableVersion::DdlNotifier,
        };
        mutator
            .set_ddl_table_version(final_version)
            .map_err(|error| format!("write DDL table version: {error}"))?;
    }
    Ok(())
}
