// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/schema_test.go`.
//! 覆盖系统库与 BR 临时系统库判定，确保过滤逻辑与 Go 表驱动用例一致。
//! 备份过滤依赖此判定，避免误导出或误跳过系统元数据。

use crate::schema::IsSysOrTempSysDB;

#[test]
fn test_is_sys_or_temp_sys_db() {
    // 表驱动：系统库/临时系统库为 true，普通库及其临时名为 false。
    // 名称列仅用于断言消息，不参与判定。
    let tests = [
        ("mysql system db", "mysql", true),
        ("sys system db", "sys", true),
        ("workload_schema system db", "workload_schema", true),
        ("temporary mysql db", "__TiDB_BR_Temporary_mysql", true),
        ("temporary sys db", "__TiDB_BR_Temporary_sys", true),
        (
            "temporary workload_schema db",
            "__TiDB_BR_Temporary_workload_schema",
            true,
        ),
        ("normal db", "test", false),
        // 临时前缀 alone 不够，目标库本身也须是系统库。
        ("temporary normal db", "__TiDB_BR_Temporary_test", false),
    ];
    for (name, db, expected) in tests {
        let result = IsSysOrTempSysDB(db);
        assert_eq!(result, expected, "IsSysOrTempSysDB({db:?}) via {name}");
    }
}
