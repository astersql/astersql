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

// InfoSchema 构建开销相关基准/单元测试。
//
// 对应 Go `BenchmarkInfoschemaOverhead`：在无 TiKV/HTTP 依赖的前提下，
// 用相同的默认表数量验证 infoschema v2 `Data` 批量建表路径。

// Rust counterpart of pkg/infoschema/bench_test.go.
//
// Go BenchmarkInfoschemaOverhead needs TiKV + status HTTP server. This port
// keeps the same construction loop (create N tables into an infoschema v2
// Data) as a unit-style benchmark over the production APIs.

use crate::infoschema::{CiString, ColumnInfo, DBInfo, InfoSchema, Table, TableInfo};
use crate::infoschema_v2::{NewData, NewInfoSchemaV2};

/// 构造大小写不敏感字符串包装，便于测试中设置库表名。
fn ci(name: &str) -> CiString {
    CiString::new(name)
}

#[test]
/// 向 infoschema v2 Data 写入 Go 基准默认数量的表，并验证每张表均可查询。
fn test_infoschema_overhead_create_tables() {
    // Match BenchmarkInfoschemaOverhead's default -table-cnt value. The Rust
    // crate deliberately has no TiKV/testkit dev-dependencies, so this test
    // exercises the same table construction loop over the production v2 data
    // APIs without the Go benchmark's external store and status HTTP server.
    let table_cnt = 100_i32;
    let data = NewData();
    data.SetCacheCapacity(1_000_000);

    let db = DBInfo {
        id: 1,
        name: ci("test"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    data.addDB(1, db.clone());
    // 逐表构造最小 TableInfo（仅含 id 列）并写入 Data。
    for j in 0..table_cnt {
        let tbl = Table::new(TableInfo {
            id: (j as i64) + 1,
            db_id: 1,
            name: ci(&format!("test{j}")),
            columns: vec![ColumnInfo {
                id: 1,
                name: ci("id"),
                auto_increment: false,
            }],
            ..Default::default()
        });
        data.add(&db, tbl, (j as i64) + 1);
    }

    let info_schema = NewInfoSchemaV2(data.clone(), table_cnt as i64, u64::MAX);
    let tables = info_schema
        .SchemaTableInfos(&ci("test"))
        .expect("created schema must be queryable");
    assert_eq!(table_cnt as usize, tables.len());
    for j in 0..table_cnt {
        let table = info_schema
            .TableByName(&ci("test"), &ci(&format!("test{j}")))
            .expect("created table must be queryable");
        assert_eq!((j as i64) + 1, table.Meta().id);
    }
    assert_eq!(1_000_000, data.CacheCapacity());
}
