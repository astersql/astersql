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

// metadef 迁移对齐单元测试。
//
// 校验库名大小写敏感分类、保留全局 ID 边界与分配顺序，以及关键系统表建表 SQL
// 与 Go `system_tables_def.go` 中原始字符串常量一致（空白规范化后比较）。

use super::*;

/// 从 Go 源文件中按 `` name = `...` `` 形式截取原始常量正文。
fn go_raw_constant<'a>(source: &'a str, name: &str) -> &'a str {
    let assignment = format!("\t{name} = `");
    let start = source
        .find(&assignment)
        .unwrap_or_else(|| panic!("missing Go constant {}", name))
        + assignment.len();
    let end = source[start..]
        .find('`')
        .unwrap_or_else(|| panic!("unterminated Go constant {}", name));
    &source[start..start + end]
}

/// 将 SQL 按空白折叠为单空格，便于忽略 Go/Rust 字面量排版差异。
fn normalized_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 库名常量大小写字段与 Mem/System/BR 分类判定应对齐 Go。
#[test]
fn database_names_match_go_case_sensitive_classification() {
    assert_eq!(InformationSchemaName.O, "INFORMATION_SCHEMA");
    assert_eq!(InformationSchemaName.L, "information_schema");
    assert!(IsMemDB("information_schema"));
    assert!(IsMemDB("performance_schema"));
    assert!(IsMemDB("metrics_schema"));
    assert!(!IsMemDB("INFORMATION_SCHEMA"));

    assert!(IsSystemDB("mysql"));
    assert!(!IsSystemDB("sys"));
    assert!(IsSystemRelatedDB("mysql"));
    assert!(IsSystemRelatedDB("sys"));
    assert!(IsSystemRelatedDB("workload_schema"));
    assert!(IsMemOrSysDB("information_schema"));
    assert!(IsMemOrSysDB("mysql"));

    assert!(IsBRRelatedDB("__TiDB_BR_Temporary_orders"));
    assert!(!IsBRRelatedDB("__tidb_br_temporary_orders"));
}

/// 保留 ID 上/下界、用户最大 ID 与若干系统表 ID 的减法分配应对齐 Go。
#[test]
fn reserved_id_bounds_and_allocations_match_go() {
    assert_eq!(ReservedGlobalIDUpperBound, 0x0000_FFFF_FFFF_FFFF);
    assert_eq!(
        ReservedGlobalIDLowerBound,
        ReservedGlobalIDUpperBound - 1000
    );
    assert_eq!(MaxUserGlobalID, ReservedGlobalIDLowerBound);
    assert!(!IsReservedID(ReservedGlobalIDLowerBound));
    assert!(IsReservedID(ReservedGlobalIDLowerBound + 1));
    assert!(IsReservedID(ReservedGlobalIDUpperBound));
    assert!(!IsReservedID(ReservedGlobalIDUpperBound + 1));

    assert_eq!(SystemDatabaseID, ReservedGlobalIDUpperBound);
    assert_eq!(TiDBDDLJobTableID, SystemDatabaseID - 1);
    assert_eq!(TiDBMaskingPolicyTableID, SystemDatabaseID - 62);
}

/// 抽检关键建表 SQL 与 Go 常量 token 顺序一致；通知表名常量保持不变。
#[test]
fn system_table_sql_preserves_go_tokens_and_order() {
    let go = include_str!("system_tables_def.go");
    for (name, rust) in [
        ("CreateUserTable", CreateUserTable),
        ("CreateStatsHistoryTable", CreateStatsHistoryTable),
        ("CreateStatsMetaHistoryTable", CreateStatsMetaHistoryTable),
        (
            "CreateTiDBTTLTableStatusTable",
            CreateTiDBTTLTableStatusTable,
        ),
        ("CreateTiDBTTLJobHistoryTable", CreateTiDBTTLJobHistoryTable),
        ("CreateTiDBGlobalTaskTable", CreateTiDBGlobalTaskTable),
        (
            "CreateTiDBGlobalTaskHistoryTable",
            CreateTiDBGlobalTaskHistoryTable,
        ),
        (
            "CreateTiDBBackgroundSubtaskHistoryTable",
            CreateTiDBBackgroundSubtaskHistoryTable,
        ),
        ("CreateTiDBDDLNotifierTable", CreateTiDBDDLNotifierTable),
    ] {
        assert_eq!(
            normalized_sql(rust),
            normalized_sql(go_raw_constant(go, name)),
            "constant {name}"
        );
    }
    assert_eq!(NotifierTableName, "tidb_ddl_notifier");
}
