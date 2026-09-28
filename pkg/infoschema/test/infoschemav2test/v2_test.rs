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

// InfoSchema V2 行为测试：直接驱动生产级 `infoschema_v2` / `cache` API。
//
// 对应 Go `v2_test.go`。Go 侧经 `testkit` 跑完整 SQL；本 crate 无 SQL/会话/DDL
// 执行器，故用同等数据形态与版本变迁复现断言链。无法在精简 Rust 端口中触及的
// 断言（failpoint 存储重载、`SchemaSimpleTableInfos` 等）
// 在替换点单独注明，避免静默丢弃。
//
// InfoSchema：库表元数据内存视图；schema 版本：DDL 变更后递增的元数据版本；
// 快照：按版本或时间戳固定的历史元数据视图；分区：一张逻辑表拆成的物理分片。

// Rust counterpart of `pkg/infoschema/test/infoschemav2test/v2_test.go`.
//
// The Go file drives `infoschema_v2` indirectly through full SQL statements
// executed via `testkit`. This crate has no SQL layer, session, or DDL
// executor, so every test below drives the real, production
// `astersql_infoschema::infoschema_v2` (and `cache`) APIs directly with the
// same data-shape and version transitions the SQL statements would have
// produced, then reproduces the same `require.*` assertion chain.
//
// A few Go assertions have no reachable production counterpart in this
// compact Rust port (e.g. failpoint-gated storage reloads, or
// `SchemaSimpleTableInfos`, which only survives as
// mechanical-draft comments in `infoschema_v2.rs`). Those are called out
// with a comment at the point where the substitute assertion is made instead
// of being silently dropped.

use std::collections::HashMap;
use std::sync::Arc;

use astersql_infoschema::infoschema::{
    CiString, ColumnInfo, DBInfo, ForeignKeyInfo, InfoSchema, PartitionDefinition, PartitionInfo,
    Table, TableInfo,
};
use astersql_infoschema::infoschema_v2::{
    Data as V2Data, IsSpecialDB, IsV2, NewData, NewInfoSchemaV2, infoschemaV2,
};
use astersql_infoschema::{NewCache, SchemaRef};

/// 默认 schema 缓存容量（1GiB），对应开启 schema cache 时的常见配置量级。
const DEFAULT_CACHE_CAPACITY: u64 = 1024 * 1024 * 1024;

/// 构造大小写不敏感的标识符（CiString）。
fn ci(name: &str) -> CiString {
    CiString::new(name)
}

/// 构造仅含 id/name 的空库元数据。
fn new_db(id: i64, name: &str) -> DBInfo {
    DBInfo {
        id,
        name: ci(name),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    }
}

/// 构造最小表元数据（其余字段取 Default）。
fn new_table(id: i64, db_id: i64, name: &str) -> TableInfo {
    TableInfo {
        id,
        db_id,
        name: ci(name),
        ..Default::default()
    }
}

/// Stands in for the DDL-layer guard that rejects `DROP DATABASE`/`DROP TABLE`
/// against a memory-only ("special") schema. There is no DDL executor in this
/// crate to exercise directly, so the guard itself is reproduced here using
/// the real `IsSpecialDB` predicate that production DDL code would consult.
/// 模拟 DDL 层对仅内存（special）库的 DROP/ALTER/写保护；生产侧同样查询 `IsSpecialDB`。
fn reject_if_special_db(schema: &str) -> Result<(), &'static str> {
    if IsSpecialDB(schema) {
        Err("Access denied; you cannot drop, alter or write to a system database")
    } else {
        Ok(())
    }
}

/// Corresponds to Go `TestSpecialSchemas`: mem-only ("special") databases are
/// listed alongside regular ones, cannot be dropped, and infoschema reports
/// itself as v2 once the schema cache is configured.
/// 对应 Go `TestSpecialSchemas`：special 库与普通库并列列出、不可删除，启用缓存后为 V2。
#[test]
fn test_special_schemas() {
    // 校验 IsSpecialDB 对 information_schema / metrics / performance 等系统库的判定。
    assert!(IsSpecialDB("INFORMATION_SCHEMA"));
    assert!(IsSpecialDB("information_schema"));
    assert!(IsSpecialDB("METRICS_SCHEMA"));
    assert!(IsSpecialDB("PERFORMANCE_SCHEMA"));
    assert!(!IsSpecialDB("test"));
    assert!(!IsSpecialDB("mysql"));
    assert!(!IsSpecialDB("sys"));

    let data = NewData();
    // `set @@global.tidb_schema_cache_size = 1073741824;` followed by
    // `select @@global.tidb_schema_cache_size;` -- exercised directly against
    // the real cache-capacity API instead of through a session variable.
    data.SetCacheCapacity(1073741824);
    assert_eq!(data.CacheCapacity(), 1073741824);

    let is_db = new_db(1, "INFORMATION_SCHEMA");
    let is_table_names = [
        "ANALYZE_STATUS",
        "ATTRIBUTES",
        "CHARACTER_SETS",
        "COLLATIONS",
        "COLUMNS",
        "COLUMN_PRIVILEGES",
        "COLUMN_STATISTICS",
        "VIEWS",
    ];
    let mut is_tables: Vec<Table> = is_table_names
        .iter()
        .enumerate()
        .map(|(i, name)| Table::new(new_table(1000 + i as i64, is_db.id, name)))
        .collect();
    let tables_table = Table::new(TableInfo {
        columns: vec![
            ColumnInfo {
                id: 1,
                name: ci("TABLE_CATALOG"),
                auto_increment: false,
            },
            ColumnInfo {
                id: 2,
                name: ci("TABLE_SCHEMA"),
                auto_increment: false,
            },
            ColumnInfo {
                id: 3,
                name: ci("TABLE_NAME"),
                auto_increment: false,
            },
            ColumnInfo {
                id: 4,
                name: ci("TABLE_TYPE"),
                auto_increment: false,
            },
        ],
        ..new_table(100, is_db.id, "TABLES")
    });
    is_tables.push(tables_table);
    data.addSpecialDB(is_db.clone(), is_tables);

    let metrics_db = new_db(2, "METRICS_SCHEMA");
    let uptime = Table::new(TableInfo {
        columns: vec![ColumnInfo {
            id: 1,
            name: ci("TIME"),
            auto_increment: false,
        }],
        ..new_table(2000, metrics_db.id, "UPTIME")
    });
    data.addSpecialDB(metrics_db.clone(), vec![uptime]);

    let performance_db = new_db(3, "PERFORMANCE_SCHEMA");
    data.addSpecialDB(performance_db, Vec::new());

    data.addDB(1, new_db(4, "mysql"));
    data.addDB(1, new_db(5, "sys"));
    data.addDB(1, new_db(6, "test"));

    let is = NewInfoSchemaV2(data.clone(), 1, 1);
    assert!(IsV2(&is));
    assert!(is.IsV2());

    // `show databases;` -- special and regular schemas are listed together.
    let mut names: Vec<String> = is
        .AllSchemas()
        .iter()
        .map(|db| db.name.original.clone())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "INFORMATION_SCHEMA",
            "METRICS_SCHEMA",
            "PERFORMANCE_SCHEMA",
            "mysql",
            "sys",
            "test",
        ]
    );

    // `use information_schema; show tables;` contains at least these names.
    for name in is_table_names {
        assert!(
            is.TableByName(&is_db.name, &ci(name)).is_ok(),
            "information_schema.{name} should exist"
        );
    }
    // `show create table tables;` -- column shape carries the expected fields.
    let tables_meta = is
        .TableByName(&is_db.name, &ci("TABLES"))
        .expect("TABLES exists");
    for column in ["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "TABLE_TYPE"] {
        assert!(
            tables_meta
                .Meta()
                .columns
                .iter()
                .any(|c| c.name.original == column),
            "TABLES should have a {column} column"
        );
    }

    // `use metrics_schema; show tables;` contains `uptime`, whose definition
    // has a `time` column (`show create table uptime;` contains "time").
    let uptime_meta = is
        .TableByName(&metrics_db.name, &ci("UPTIME"))
        .expect("uptime exists");
    assert!(
        uptime_meta
            .Meta()
            .columns
            .iter()
            .any(|c| c.name.lower == "time")
    );

    // `drop database information_schema;` / `drop table views;` must be
    // rejected; regular schemas remain droppable.
    assert!(reject_if_special_db(&is_db.name.original).is_err());
    assert!(reject_if_special_db("test").is_ok());

    data.SetCacheCapacity(DEFAULT_CACHE_CAPACITY);
}

/// Test-local DDL-version harness. Every lookup is delegated to the production
/// `Data`/`infoschemaV2` implementation; the harness only publishes the table
/// metadata versions that the Go test creates through SQL DDL.
/// 分区变更版本模拟器：仅发布 DDL 产生的表元数据版本，查找全部走真实 V2 API。
struct PartitionSim {
    data: Arc<V2Data>,
    db: DBInfo,
    version: i64,
}

impl PartitionSim {
    /// 创建仅含给定库的模拟器，初始 schema 版本为 1。
    fn new(db: DBInfo) -> Self {
        let data = NewData();
        data.addDB(1, db.clone());
        Self {
            data,
            db,
            version: 1,
        }
    }

    /// 按当前版本构造 `infoschemaV2` 快照。
    fn schema(&self) -> infoschemaV2 {
        NewInfoSchemaV2(self.data.clone(), self.version, self.version as u64)
    }

    /// Publishes a new version of `table` (`pt`). Removed partition IDs are
    /// retained in the call sites as parity documentation; production lookup
    /// decides visibility from the current table metadata.
    /// 发布分区表新版本；失效 ID 参数保留 Go DDL 映射，实际可见性由生产查找决定。
    fn publish(&mut self, table: TableInfo, _removed_partition_ids: &[i64]) {
        self.version += 1;
        self.data.add(&self.db, Table::new(table), self.version);
    }

    /// Publishes an unrelated table (`nt`) at a new version.
    /// 在新版本发布一张无关表（如 EXCHANGE 前的普通表）。
    fn add_table(&mut self, table: TableInfo) {
        self.version += 1;
        self.data.add(&self.db, Table::new(table), self.version);
    }

    /// 按分区 ID 通过真实 `infoschemaV2` 查找所属表。
    fn find_by_partition_id(&self, pid: i64) -> Option<(Table, Arc<DBInfo>, PartitionDefinition)> {
        self.schema().FindTableByPartitionID(pid)
    }
}

/// Corresponds to Go `TestFindTableByPartitionID`.
/// 对应 Go `TestFindTableByPartitionID`：分区增删截断重组交换后按分区 ID 定位表。
#[test]
fn test_find_table_by_partition_id() {
    let db = new_db(1, "test");
    let mut sim = PartitionSim::new(db.clone());
    let pt_id = 10;

    let p0 = PartitionDefinition {
        id: 100,
        name: ci("p0"),
    };
    let p1 = PartitionDefinition {
        id: 101,
        name: ci("p1"),
    };
    let p2 = PartitionDefinition {
        id: 102,
        name: ci("p2"),
    };
    let p3 = PartitionDefinition {
        id: 103,
        name: ci("p3"),
    };
    sim.publish(
        TableInfo {
            partition: Some(PartitionInfo {
                definitions: vec![p0.clone(), p1.clone(), p2.clone(), p3.clone()],
            }),
            ..new_table(pt_id, db.id, "pt")
        },
        &[],
    );

    let (tbl, dbinfo, pdef) = sim.find_by_partition_id(p3.id).expect("p3 resolves");
    assert_eq!(tbl.Meta().id, pt_id);
    assert_eq!(dbinfo.name.lower, "test");
    assert_eq!(pdef.id, p3.id);

    // Test FindTableByPartitionID after dropping an unrelated partition (p2).
    sim.publish(
        TableInfo {
            partition: Some(PartitionInfo {
                definitions: vec![p0.clone(), p1.clone(), p3.clone()],
            }),
            ..new_table(pt_id, db.id, "pt")
        },
        &[p2.id],
    );
    let (tbl, dbinfo, pdef) = sim
        .find_by_partition_id(p3.id)
        .expect("p3 still resolves after unrelated drop");
    assert_eq!(tbl.Meta().id, pt_id);
    assert_eq!(dbinfo.name.lower, "test");
    assert_eq!(pdef.id, p3.id);

    // Test FindTableByPartitionID after dropping that partition (p3 itself).
    sim.publish(
        TableInfo {
            partition: Some(PartitionInfo {
                definitions: vec![p0.clone(), p1.clone()],
            }),
            ..new_table(pt_id, db.id, "pt")
        },
        &[p3.id],
    );
    assert!(sim.find_by_partition_id(p3.id).is_none());

    // Test FindTableByPartitionID after adding back the partition (fresh id).
    let new_p3 = PartitionDefinition {
        id: 104,
        name: ci("p3"),
    };
    sim.publish(
        TableInfo {
            partition: Some(PartitionInfo {
                definitions: vec![p0.clone(), p1.clone(), new_p3.clone()],
            }),
            ..new_table(pt_id, db.id, "pt")
        },
        &[],
    );
    assert!(sim.find_by_partition_id(p3.id).is_none());
    let (_, _, pdef) = sim
        .find_by_partition_id(new_p3.id)
        .expect("new p3 resolves");
    assert_eq!(pdef.id, new_p3.id);

    // Test FindTableByPartitionID after truncate partition.
    let truncated_p3 = PartitionDefinition {
        id: 105,
        name: ci("p3"),
    };
    sim.publish(
        TableInfo {
            partition: Some(PartitionInfo {
                definitions: vec![p0.clone(), p1.clone(), truncated_p3.clone()],
            }),
            ..new_table(pt_id, db.id, "pt")
        },
        &[new_p3.id],
    );
    assert!(sim.find_by_partition_id(new_p3.id).is_none());
    sim.find_by_partition_id(truncated_p3.id)
        .expect("truncated p3 resolves");

    // Test FindTableByPartitionID after reorganize partition (p1,p3 -> p3,p5).
    let reorg_p3 = PartitionDefinition {
        id: 106,
        name: ci("p3"),
    };
    let reorg_p5 = PartitionDefinition {
        id: 107,
        name: ci("p5"),
    };
    sim.publish(
        TableInfo {
            partition: Some(PartitionInfo {
                definitions: vec![p0.clone(), reorg_p3.clone(), reorg_p5.clone()],
            }),
            ..new_table(pt_id, db.id, "pt")
        },
        &[p1.id, truncated_p3.id],
    );
    assert!(sim.find_by_partition_id(truncated_p3.id).is_none());
    sim.find_by_partition_id(reorg_p3.id)
        .expect("reorganized p3 resolves");

    // Test FindTableByPartitionID after exchange partition: the post-exchange
    // partition id equals `nt`'s pre-exchange table id.
    let nt_id = 200;
    sim.add_table(new_table(nt_id, db.id, "nt"));

    let exchanged_p3 = PartitionDefinition {
        id: nt_id,
        name: ci("p3"),
    };
    sim.publish(
        TableInfo {
            partition: Some(PartitionInfo {
                definitions: vec![p0, exchanged_p3, reorg_p5],
            }),
            ..new_table(pt_id, db.id, "pt")
        },
        &[reorg_p3.id],
    );
    let (_, _, pdef) = sim
        .find_by_partition_id(nt_id)
        .expect("exchanged p3 resolves to nt's old id");
    assert_eq!(pdef.id, nt_id);
}

/// Per-table special-attribute flags this harness tracks alongside the real
/// `Data`/`infoschemaV2` catalog. Production's compact `TableInfo` (unlike
/// Go's `model.TableInfo`) does not carry `TTLInfo`/`TiFlashReplica`/
/// `PlacementPolicyRef` fields directly (only through an optional, unused-here
/// `model_meta`), and `InfoSchema::ListTablesWithSpecialAttribute`'s default
/// implementation cannot see anything for `infoschemaV2` because
/// `Data::addDB` always clears `DBInfo::tables` (mirroring Go's
/// `dbInfo.Deprecated.Tables = nil`). So this is exactly the "production has
/// no working `ListTablesWithSpecialAttribute` for v2; build the equivalent
/// filter in the test instead" case called out by the task.
/// 表级特殊属性标志（TTL / TiFlash 副本 / Placement Policy），由测试旁路跟踪，
/// 因精简 `TableInfo` 与 V2 的 `ListTablesWithSpecialAttribute` 尚不可用。
#[derive(Clone, Copy, Default)]
struct TableAttrs {
    /// 是否配置了 TTL（生存时间）策略。
    ttl: bool,
    /// 是否配置了 TiFlash 副本。
    tiflash_replica: bool,
    /// 是否绑定了 Placement Policy（副本放置策略）。
    placement_policy: bool,
}

impl TableAttrs {
    /// 任一特殊属性为真即视为“特殊表”。
    fn is_special(&self) -> bool {
        self.ttl || self.tiflash_replica || self.placement_policy
    }
}

/// 驱动“列出带特殊属性的表”断言链的本地模拟器。
struct SpecialAttrSim {
    data: Arc<V2Data>,
    version: i64,
    attrs: HashMap<i64, TableAttrs>,
}

impl SpecialAttrSim {
    fn new() -> Self {
        Self {
            data: NewData(),
            version: 1,
            attrs: HashMap::new(),
        }
    }

    /// 递增并返回新的 schema 版本号。
    fn bump(&mut self) -> i64 {
        self.version += 1;
        self.version
    }

    fn schema(&self) -> infoschemaV2 {
        NewInfoSchemaV2(self.data.clone(), self.version, self.version as u64)
    }

    fn create_db(&mut self, db: &DBInfo) {
        let v = self.bump();
        self.data.addDB(v, db.clone());
    }

    /// 先移除库内表再 deleteDB，同步清理本地属性映射。
    fn drop_db(&mut self, db: &DBInfo, tables: &[(&str, i64)]) {
        let v = self.bump();
        for (name, id) in tables {
            self.data.remove(db.name.clone(), db.id, ci(name), *id, v);
            self.attrs.remove(id);
        }
        self.data.deleteDB(db.clone(), v);
    }

    fn create_table(&mut self, db: &DBInfo, table: TableInfo, attrs: TableAttrs) {
        let v = self.bump();
        self.attrs.insert(table.id, attrs);
        self.data.add(db, Table::new(table), v);
    }

    /// 以同名同 ID 再 add 一次，表示 ALTER 后属性变化。
    fn alter_attrs(&mut self, db: &DBInfo, table: TableInfo, attrs: TableAttrs) {
        let v = self.bump();
        self.attrs.insert(table.id, attrs);
        self.data.add(db, Table::new(table), v);
    }

    fn drop_table(&mut self, db: &DBInfo, name: &str, id: i64) {
        let v = self.bump();
        self.data.remove(db.name.clone(), db.id, ci(name), id, v);
        self.attrs.remove(&id);
    }

    /// Mirrors Go's `checkResult` (built on
    /// `is.ListTablesWithSpecialAttribute(infoschemacontext.AllSpecialAttribute)`):
    /// lists "<db> <table>" pairs for every currently-visible table this
    /// harness has tagged with a special attribute, sorted for comparison.
    /// 镜像 Go `checkResult`：列出当前可见且带特殊属性的 "<库> <表>"，排序后比较。
    fn special_tables(&self, schemas: &[&DBInfo]) -> Vec<String> {
        let is = self.schema();
        let mut rows = Vec::new();
        for db in schemas {
            if let Ok(tables) = is.SchemaTableInfos(&db.name) {
                for table in tables {
                    if self
                        .attrs
                        .get(&table.id)
                        .is_some_and(TableAttrs::is_special)
                    {
                        rows.push(format!("{} {}", db.name.lower, table.name.lower));
                    }
                }
            }
        }
        rows.sort();
        rows
    }
}

/// Corresponds to Go `TestListTablesWithSpecialAttribute`.
/// 对应 Go `TestListTablesWithSpecialAttribute`：TTL/TiFlash/Placement 表的增删改列表示。
#[test]
fn test_list_tables_with_special_attribute() {
    // First exercise the production API directly. `SpecialAttrSim` below keeps
    // the full Go DDL/assertion inventory, but it must not hide a regression in
    // `infoschemaV2::ListTablesWithSpecialAttribute` itself. A filter that
    // accepts every complete table metadata object avoids coupling this probe
    // to any one attribute representation.
    let production_data = NewData();
    let production_db_a = new_db(9000, "production_probe_a");
    let production_db_b = new_db(9001, "production_probe_b");
    production_data.addDB(1, production_db_a.clone());
    production_data.addDB(1, production_db_b.clone());
    let complete_meta = Table::from_model(Default::default())
        .Meta()
        .model_meta
        .clone();
    let complete_table = |id, db_id, name| {
        let mut table = new_table(id, db_id, name);
        table.model_meta = complete_meta.clone();
        Table::new(table)
    };
    production_data.add(
        &production_db_a,
        complete_table(9100, production_db_a.id, "a1"),
        1,
    );
    production_data.add(
        &production_db_a,
        complete_table(9101, production_db_a.id, "a2"),
        1,
    );
    production_data.add(
        &production_db_b,
        complete_table(9200, production_db_b.id, "b1"),
        1,
    );
    production_data.remove(
        production_db_a.name.clone(),
        production_db_a.id,
        ci("a2"),
        9101,
        2,
    );

    let historical_schema = NewInfoSchemaV2(production_data.clone(), 1, 1);
    let historical_rows = historical_schema.ListTablesWithSpecialAttribute(|_| true);
    assert_eq!(
        historical_rows
            .iter()
            .map(|result| result.TableInfos.len())
            .sum::<usize>(),
        3,
        "a historical V2 snapshot must retain every table visible at its version",
    );

    let production_schema = NewInfoSchemaV2(production_data, 2, 2);
    let production_rows = production_schema.ListTablesWithSpecialAttribute(|_| true);
    assert_eq!(
        production_rows
            .iter()
            .map(|result| result.TableInfos.len())
            .sum::<usize>(),
        2,
        "V2 must list every current normal-schema table accepted by the filter",
    );
    assert_eq!(
        production_rows
            .iter()
            .map(|result| result.DBName.L.as_str())
            .collect::<Vec<_>>(),
        vec!["production_probe_b", "production_probe_a"],
        "V2 must preserve Go's descending database grouping order",
    );
    assert!(
        production_schema
            .ListTablesWithSpecialAttribute(|_| false)
            .is_empty(),
        "databases without a matching table must not produce empty result groups",
    );

    let mut sim = SpecialAttrSim::new();
    let mut next_id = 1000i64;
    let mut alloc_id = move || {
        next_id += 1;
        next_id
    };

    // 分别在开启缓存与关闭缓存（capacity=0）下跑同一套 DDL 模拟序列。
    for cache_capacity in [1024000u64, 0u64] {
        sim.data.SetCacheCapacity(cache_capacity);

        let db1 = new_db(alloc_id(), "test_db1");
        sim.create_db(&db1);

        let t_ttl1 = alloc_id();
        sim.create_table(
            &db1,
            new_table(t_ttl1, db1.id, "t_ttl"),
            TableAttrs {
                ttl: true,
                ..Default::default()
            },
        );
        assert_eq!(
            sim.special_tables(&[&db1]),
            vec!["test_db1 t_ttl".to_string()]
        );

        // alter table t_ttl remove ttl
        sim.alter_attrs(
            &db1,
            new_table(t_ttl1, db1.id, "t_ttl"),
            TableAttrs::default(),
        );
        assert_eq!(sim.special_tables(&[&db1]), Vec::<String>::new());

        // drop table t_ttl
        sim.drop_table(&db1, "t_ttl", t_ttl1);
        assert_eq!(sim.special_tables(&[&db1]), Vec::<String>::new());

        // create table t_ttl (created_at1 datetime) ttl = ...
        let t_ttl1b = alloc_id();
        sim.create_table(
            &db1,
            new_table(t_ttl1b, db1.id, "t_ttl"),
            TableAttrs {
                ttl: true,
                ..Default::default()
            },
        );
        assert_eq!(
            sim.special_tables(&[&db1]),
            vec!["test_db1 t_ttl".to_string()]
        );

        let db2 = new_db(alloc_id(), "test_db2");
        sim.create_db(&db2);
        assert_eq!(
            sim.special_tables(&[&db1, &db2]),
            vec!["test_db1 t_ttl".to_string()]
        );

        let t_ttl2 = alloc_id();
        sim.create_table(
            &db2,
            new_table(t_ttl2, db2.id, "t_ttl"),
            TableAttrs {
                ttl: true,
                ..Default::default()
            },
        );
        assert_eq!(
            sim.special_tables(&[&db1, &db2]),
            vec!["test_db1 t_ttl".to_string(), "test_db2 t_ttl".to_string()]
        );

        let t_tiflash = alloc_id();
        sim.create_table(
            &db2,
            new_table(t_tiflash, db2.id, "t_tiflash"),
            TableAttrs::default(),
        );
        assert_eq!(
            sim.special_tables(&[&db1, &db2]),
            vec!["test_db1 t_ttl".to_string(), "test_db2 t_ttl".to_string()]
        );

        // alter table t_tiflash set tiflash replica 1
        sim.alter_attrs(
            &db2,
            new_table(t_tiflash, db2.id, "t_tiflash"),
            TableAttrs {
                tiflash_replica: true,
                ..Default::default()
            },
        );
        assert_eq!(
            sim.special_tables(&[&db1, &db2]),
            vec![
                "test_db1 t_ttl".to_string(),
                "test_db2 t_tiflash".to_string(),
                "test_db2 t_ttl".to_string(),
            ]
        );

        // alter table t_tiflash set tiflash replica 0
        sim.alter_attrs(
            &db2,
            new_table(t_tiflash, db2.id, "t_tiflash"),
            TableAttrs::default(),
        );
        assert_eq!(
            sim.special_tables(&[&db1, &db2]),
            vec!["test_db1 t_ttl".to_string(), "test_db2 t_ttl".to_string()]
        );

        // drop table t_tiflash
        sim.drop_table(&db2, "t_tiflash", t_tiflash);
        assert_eq!(
            sim.special_tables(&[&db1, &db2]),
            vec!["test_db1 t_ttl".to_string(), "test_db2 t_ttl".to_string()]
        );

        // create table t_tiflash (id int); alter table t_tiflash set tiflash replica 1
        let t_tiflash2 = alloc_id();
        sim.create_table(
            &db2,
            new_table(t_tiflash2, db2.id, "t_tiflash"),
            TableAttrs::default(),
        );
        sim.alter_attrs(
            &db2,
            new_table(t_tiflash2, db2.id, "t_tiflash"),
            TableAttrs {
                tiflash_replica: true,
                ..Default::default()
            },
        );
        assert_eq!(
            sim.special_tables(&[&db1, &db2]),
            vec![
                "test_db1 t_ttl".to_string(),
                "test_db2 t_tiflash".to_string(),
                "test_db2 t_ttl".to_string(),
            ]
        );

        // drop database test_db1
        sim.drop_db(&db1, &[("t_ttl", t_ttl1b)]);
        assert_eq!(
            sim.special_tables(&[&db2]),
            vec![
                "test_db2 t_tiflash".to_string(),
                "test_db2 t_ttl".to_string()
            ]
        );

        // create or replace placement policy x ...; create table t_placement (...) placement policy="x"
        let t_placement = alloc_id();
        sim.create_table(
            &db2,
            new_table(t_placement, db2.id, "t_placement"),
            TableAttrs {
                placement_policy: true,
                ..Default::default()
            },
        );
        assert_eq!(
            sim.special_tables(&[&db2]),
            vec![
                "test_db2 t_placement".to_string(),
                "test_db2 t_tiflash".to_string(),
                "test_db2 t_ttl".to_string(),
            ]
        );

        // create table pt_placement (...) placement policy x partition by HASH(id) PARTITIONS 4
        let pt_placement = alloc_id();
        let partitions = vec![
            PartitionDefinition {
                id: alloc_id(),
                name: ci("p0"),
            },
            PartitionDefinition {
                id: alloc_id(),
                name: ci("p1"),
            },
            PartitionDefinition {
                id: alloc_id(),
                name: ci("p2"),
            },
            PartitionDefinition {
                id: alloc_id(),
                name: ci("p3"),
            },
        ];
        sim.create_table(
            &db2,
            TableInfo {
                partition: Some(PartitionInfo {
                    definitions: partitions,
                }),
                ..new_table(pt_placement, db2.id, "pt_placement")
            },
            TableAttrs {
                placement_policy: true,
                ..Default::default()
            },
        );
        assert_eq!(
            sim.special_tables(&[&db2]),
            vec![
                "test_db2 pt_placement".to_string(),
                "test_db2 t_placement".to_string(),
                "test_db2 t_tiflash".to_string(),
                "test_db2 t_ttl".to_string(),
            ]
        );

        // drop database test_db2
        sim.drop_db(
            &db2,
            &[
                ("t_ttl", t_ttl2),
                ("t_placement", t_placement),
                ("t_tiflash", t_tiflash2),
                ("pt_placement", pt_placement),
            ],
        );
        assert_eq!(sim.special_tables(&[&db2]), Vec::<String>::new());
    }
}

/// Corresponds to Go `TestTiDBSchemaCacheSizeVariable`.
/// 对应 Go `TestTiDBSchemaCacheSizeVariable`：缓存容量变更通过共享 `Arc<Data>` 可见。
#[test]
fn test_tidb_schema_cache_size_variable() {
    let data = NewData();
    assert_eq!(data.CacheCapacity(), DEFAULT_CACHE_CAPACITY);

    let is = NewInfoSchemaV2(data.clone(), 1, 1);
    assert!(IsV2(&is));
    assert_eq!(is.Data.CacheCapacity(), DEFAULT_CACHE_CAPACITY);

    data.SetCacheCapacity(1024 * 1024 * 1024);
    assert_eq!(data.CacheCapacity(), 1073741824);
    // `is.Data` shares the same underlying `Arc<Data>`, matching Go's
    // `raw.Data.CacheCapacity()` check against the same infoschema instance.
    assert_eq!(is.Data.CacheCapacity(), 1073741824);
}

/// Corresponds to Go `TestUnrelatedDDLTriggerReload`. The Go test uses a
/// failpoint to prove that unrelated DDL does not force a storage reload for
/// an evicted, unrelated table; this compact Rust port has no storage-backed
/// `loadTableInfo` path to fail (`TableByName`/`TableByID` always retain the
/// original `Table` alongside their metadata record), so the closest real
/// assertion is: eviction clears the cache slot, unrelated table creation
/// does not disturb it, and accessing the evicted table afterwards still
/// resolves correctly and refills the cache.
/// 对应 Go `TestUnrelatedDDLTriggerReload`：无关 DDL 不应强制重载已驱逐表的缓存槽。
#[test]
fn test_unrelated_ddl_trigger_reload() {
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());

    let t1_id = 10;
    data.add(&db, Table::new(new_table(t1_id, db.id, "t1")), 1);
    let is = NewInfoSchemaV2(data.clone(), 1, 1);
    is.TableByName(&db.name, &ci("t1")).expect("t1 exists");
    assert!(is.TableIsCached(t1_id));

    // Mock t1 schema cache being evicted.
    is.EvictTable(&db.name, &ci("t1"));
    assert!(!is.TableIsCached(t1_id));

    // DDL on t2 should not cause reload or cache miss on t1.
    data.add(&db, Table::new(new_table(11, db.id, "t2")), 2);
    assert!(
        !is.TableIsCached(t1_id),
        "unrelated DDL must not refill t1's cache slot"
    );

    // Refill the cache, and do more unrelated DDL.
    let reloaded = is
        .TableByName(&db.name, &ci("t1"))
        .expect("t1 still resolves after eviction");
    assert_eq!(reloaded.Meta().id, t1_id);
    assert!(is.TableIsCached(t1_id));

    data.add(&db, Table::new(new_table(12, db.id, "t3")), 3);
    assert!(
        is.TableIsCached(t1_id),
        "unrelated DDL must not evict t1's cache slot"
    );
    is.TableByName(&db.name, &ci("t1"))
        .expect("t1 still resolves");
}

/// Corresponds to Go `TestTrace`.
/// 对应 Go `TestTrace`：驱逐后再读应回填缓存（替代 Go 对 loadTableInfo 路径的 trace）。
#[test]
fn test_trace() {
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());
    let t_trace_id = 10;
    data.add(
        &db,
        Table::new(TableInfo {
            columns: vec![ColumnInfo {
                id: 1,
                name: ci("id"),
                auto_increment: true,
            }],
            ..new_table(t_trace_id, db.id, "t_trace")
        }),
        1,
    );

    let is = NewInfoSchemaV2(data.clone(), 1, 1);
    assert!(IsV2(&is));

    // Evict the table cache and check that a subsequent read recovers it.
    // This stands in for Go's `trace select ...` assertion that the read path
    // visibly goes through `infoschema.loadTableInfo`; this compact port has
    // no storage-backed reload step to trace.
    is.EvictTable(&db.name, &ci("t_trace"));
    assert!(!is.TableIsCached(t_trace_id));
    let reloaded = is
        .TableByName(&db.name, &ci("t_trace"))
        .expect("t_trace resolves after eviction");
    assert_eq!(reloaded.Meta().id, t_trace_id);
    assert!(is.TableIsCached(t_trace_id));
}

/// Corresponds to Go `TestCachedTable`.
/// 对应 Go `TestCachedTable`：缓存表驱逐后再次加载，表元数据仍一致且不 panic。
#[test]
fn test_cached_table() {
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());
    let t_cache_id = 10;
    data.add(
        &db,
        Table::new(TableInfo {
            columns: vec![ColumnInfo {
                id: 1,
                name: ci("id"),
                auto_increment: true,
            }],
            ..new_table(t_cache_id, db.id, "t_cache")
        }),
        1,
    );

    let is = NewInfoSchemaV2(data.clone(), 1, 1);
    let before = is
        .TableByName(&db.name, &ci("t_cache"))
        .expect("t_cache exists");

    // Cover a case that after cached table evict and load, table.Table goes wrong.
    is.EvictTable(&db.name, &ci("t_cache"));
    let after = is
        .TableByName(&db.name, &ci("t_cache"))
        .expect("t_cache resolves after eviction, no panic here");
    assert_eq!(before.Meta().id, after.Meta().id);
    assert_eq!(before.Meta().name.lower, after.Meta().name.lower);
}

/// Corresponds to Go `TestFullLoadAndSnapshot`: renaming a table across
/// databases, dropping a database, and reading an older snapshot version
/// afterwards. The global-temporary-table assertion uses the production V2
/// temporary-table registry directly.
/// 对应 Go `TestFullLoadAndSnapshot`：跨库 rename、删库、临时表保留与历史快照读。
#[test]
fn test_full_load_and_snapshot() {
    let db1 = new_db(1, "db1");
    let db2 = new_db(2, "db2");
    let data = NewData();
    data.addDB(1, db1.clone());
    data.addDB(1, db2.clone());
    data.addTemporaryTable(99);

    let t_id = 10;
    data.add(&db1, Table::new(new_table(t_id, db1.id, "t")), 1);

    let is_before_rename = NewInfoSchemaV2(data.clone(), 1, 100);
    is_before_rename
        .TableByName(&db1.name, &ci("t"))
        .expect("t starts under db1");
    assert!(is_before_rename.TableByName(&db2.name, &ci("t")).is_err());

    // rename table db1.t to db2.t
    data.remove(db1.name.clone(), db1.id, ci("t"), t_id, 2);
    data.add(&db2, Table::new(new_table(t_id, db2.id, "t")), 2);

    let is_after_rename = NewInfoSchemaV2(data.clone(), 2, 200);
    is_after_rename
        .TableByName(&db2.name, &ci("t"))
        .expect("t now lives under db2");
    assert!(is_after_rename.TableByName(&db1.name, &ci("t")).is_err());
    assert!(is_after_rename.HasTemporaryTable());

    // A read pinned at the pre-rename version still sees the old layout.
    is_before_rename
        .TableByName(&db1.name, &ci("t"))
        .expect("snapshot still sees pre-rename layout");
    assert!(is_before_rename.TableByName(&db2.name, &ci("t")).is_err());

    // drop database db1
    data.deleteDB(db1.clone(), 3);
    let is_after_drop = NewInfoSchemaV2(data.clone(), 3, 300);
    assert!(is_after_drop.SchemaByName(&db1.name).is_none());
    assert!(is_after_drop.SchemaByName(&db2.name).is_some());
    assert!(is_after_drop.TableByName(&db2.name, &ci("t")).is_ok());
    assert!(is_after_drop.HasTemporaryTable());

    // The earlier snapshot is unaffected by the later drop.
    assert!(is_before_rename.SchemaByName(&db1.name).is_some());
}

/// Corresponds to Go `TestIssue54926`: a `cache::InfoCache` resolves the
/// correct historical schema for a snapshot timestamp, and `GetByVersion`
/// resolves a specific schema version directly -- the properties Go exercises
/// indirectly through `SET TRANSACTION READ ONLY AS OF TIMESTAMP` and the
/// `tidb_snapshot` session variable.
/// 对应 Go `TestIssue54926`：InfoCache 按快照 TS / 版本号解析历史 schema。
#[test]
fn test_issue54926() {
    let cache = NewCache(16);
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());

    let is_v1: SchemaRef = Arc::new(NewInfoSchemaV2(data.clone(), 1, 1));
    let schema_ver1 = is_v1.SchemaMetaVersion();
    cache.Insert(is_v1.clone(), 100);

    data.add(&db, Table::new(new_table(10, db.id, "t")), 2);
    let is_v2: SchemaRef = Arc::new(NewInfoSchemaV2(data.clone(), 2, 1));
    let schema_ver2 = is_v2.SchemaMetaVersion();
    cache.Insert(is_v2.clone(), 200);
    assert!(schema_ver1 < schema_ver2);

    let snap1 = cache
        .GetBySnapshotTS(100)
        .expect("snapshot at ts=100 resolves");
    assert_eq!(snap1.SchemaMetaVersion(), schema_ver1);
    assert!(snap1.TableByName(&db.name, &ci("t")).is_err());

    let snap2 = cache
        .GetBySnapshotTS(200)
        .expect("snapshot at ts=200 resolves");
    assert_eq!(snap2.SchemaMetaVersion(), schema_ver2);
    assert!(snap2.TableByName(&db.name, &ci("t")).is_ok());

    assert_eq!(
        cache
            .GetByVersion(schema_ver1)
            .expect("version 1 resolves")
            .SchemaMetaVersion(),
        schema_ver1
    );
    assert_eq!(
        cache
            .GetByVersion(schema_ver2)
            .expect("version 2 resolves")
            .SchemaMetaVersion(),
        schema_ver2
    );
}

/// Corresponds to Go `TestSchemaSimpleTableInfos`. The compact `infoschemaV2`
/// port in this crate does not implement `SchemaSimpleTableInfos` (only a
/// mechanical-draft comment survives for it in `infoschema_v2.rs`);
/// `SchemaTableInfos` is the closest real, callable substitute and is used
/// here instead.
/// 对应 Go `TestSchemaSimpleTableInfos`：用 `SchemaTableInfos` 替代未实现的 Simple 接口。
#[test]
fn test_schema_simple_table_infos() {
    let data = NewData();

    // Cover the special schema: `SchemaTableInfos` only reads the by-name/
    // by-id maps, not `specials`, so the special catalog's tables are
    // additionally published there to keep this real, callable API
    // consistent for the fixture.
    let is_db = new_db(1, "INFORMATION_SCHEMA");
    let is_tables = vec![
        Table::new(new_table(900, is_db.id, "TABLES")),
        Table::new(new_table(901, is_db.id, "COLUMNS")),
    ];
    data.addSpecialDB(is_db.clone(), is_tables.clone());
    for table in &is_tables {
        data.add(&is_db, table.clone(), 1);
    }

    let simple_db = new_db(2, "simple");
    data.addDB(1, simple_db.clone());
    let t1_id = 10;
    let t2_id = 11;
    data.add(
        &simple_db,
        Table::new(new_table(t1_id, simple_db.id, "t1")),
        1,
    );
    data.add(
        &simple_db,
        Table::new(new_table(t2_id, simple_db.id, "t2")),
        1,
    );

    let is_before_rename = NewInfoSchemaV2(data.clone(), 1, 100);

    let mut is_names: Vec<String> = is_before_rename
        .SchemaTableInfos(&is_db.name)
        .expect("information_schema table list")
        .iter()
        .map(|t| t.name.lower.clone())
        .collect();
    is_names.sort();
    assert_eq!(is_names, vec!["columns".to_string(), "tables".to_string()]);

    // rename table simple.t2 to elsewhere (Go additionally routes it through
    // a throwaway `aaa` database that gets dropped right after; only the
    // resulting change to `simple` matters for this assertion chain).
    data.remove(simple_db.name.clone(), simple_db.id, ci("t2"), t2_id, 2);
    let is_after_rename = NewInfoSchemaV2(data.clone(), 2, 200);

    // Cover the current (post-rename) schema: only t1 remains.
    let current_names: Vec<String> = is_after_rename
        .SchemaTableInfos(&simple_db.name)
        .expect("simple table list")
        .iter()
        .map(|t| t.name.lower.clone())
        .collect();
    assert_eq!(current_names, vec!["t1".to_string()]);

    // Cover the snapshot infoschema pinned before the rename: both tables
    // are still visible.
    let mut snapshot_names: Vec<String> = is_before_rename
        .SchemaTableInfos(&simple_db.name)
        .expect("simple table list at snapshot")
        .iter()
        .map(|t| t.name.lower.clone())
        .collect();
    snapshot_names.sort();
    assert_eq!(snapshot_names, vec!["t1".to_string(), "t2".to_string()]);
}

/// Corresponds to Go `TestSnapshotInfoschemaReader` (issue 55827): a read "as
/// of" a timestamp before a table's creation must see zero rows, not error
/// out or see a partially-initialized table.
/// 对应 Go `TestSnapshotInfoschemaReader`：建表前快照应看到空表列表而非报错。
#[test]
fn test_snapshot_infoschema_reader() {
    let db = new_db(1, "issue55827");
    let data = NewData();
    data.addDB(1, db.clone());

    let is_before = NewInfoSchemaV2(data.clone(), 1, 100);
    assert_eq!(
        is_before
            .SchemaTableInfos(&db.name)
            .expect("schema exists")
            .len(),
        0
    );

    data.add(&db, Table::new(new_table(10, db.id, "t")), 2);
    let is_after = NewInfoSchemaV2(data.clone(), 2, 200);
    let tables = is_after.SchemaTableInfos(&db.name).expect("schema exists");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].name.lower, "t");

    // "as of timestamp" before the table existed still resolves to the
    // earlier, empty schema, via the version pinned at construction time.
    assert_eq!(
        is_before
            .SchemaTableInfos(&db.name)
            .expect("schema exists")
            .len(),
        0
    );
}

/// Corresponds to Go `TestInfoSchemaCachedAutoIncrement`: toggling the schema
/// cache capacity between 0 (effectively disabled) and a large value must not
/// change which column production reports as the auto-increment column.
/// 对应 Go `TestInfoSchemaCachedAutoIncrement`：开关缓存不影响 auto_increment 列判定。
#[test]
fn test_info_schema_cached_auto_increment() {
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());
    data.SetCacheCapacity(0);

    let t_id = 10;
    data.add(
        &db,
        Table::new(TableInfo {
            columns: vec![ColumnInfo {
                id: 1,
                name: ci("a"),
                auto_increment: true,
            }],
            ..new_table(t_id, db.id, "t")
        }),
        1,
    );
    let is = NewInfoSchemaV2(data.clone(), 1, 1);

    let fetched = is.TableByName(&db.name, &ci("t")).expect("t exists");
    let auto_inc_column = fetched
        .Meta()
        .columns
        .iter()
        .find(|c| c.auto_increment)
        .expect("t has an auto_increment column");
    assert_eq!(auto_inc_column.name.lower, "a");
    assert_eq!(data.CacheCapacity(), 0);

    // Raising the cache capacity back up allows the table object to stay
    // cached across repeated reads.
    data.SetCacheCapacity(DEFAULT_CACHE_CAPACITY);
    assert_eq!(data.CacheCapacity(), DEFAULT_CACHE_CAPACITY);
    let cached_again = is.TableByName(&db.name, &ci("t")).expect("t still exists");
    assert!(is.TableIsCached(t_id));
    assert_eq!(cached_again.Meta().id, t_id);
}

/// Corresponds to Go `TestGetAndResetRecentInfoSchemaTS`.
/// 对应 Go `TestGetAndResetRecentInfoSchemaTS`：keep_alive 的 min-ts 水位与 reset 语义。
#[test]
fn test_get_and_reset_recent_info_schema_ts() {
    let cache = NewCache(16);

    // Production wires `Data::keep_alive` automatically into every
    // `infoschemaV2::TableByName`/`TableByID` call, but `cache::InfoCache` in
    // this port owns a separate, lightweight `Data` (see cache.rs) that is
    // never connected to `infoschema_v2::Data` -- so there is no DDL/read
    // path that drives it implicitly here. We call the same public
    // `keep_alive`/`GetAndResetRecentInfoSchemaTS` API by hand, standing in
    // for "some DDL/read happened at ts=X", to exercise their real
    // swap/min-ts semantics.
    let schema_ts1 = cache.GetAndResetRecentInfoSchemaTS(u64::MAX);
    assert_eq!(schema_ts1, 0, "nothing recorded yet");

    // After some DDL changes.
    cache.Data.keep_alive(100);
    let schema_ts2 = cache.GetAndResetRecentInfoSchemaTS(u64::MAX);
    assert!(schema_ts1 <= schema_ts2);
    assert_eq!(schema_ts2, 100);

    cache.Data.keep_alive(80);
    cache.Data.keep_alive(50);
    let schema_ts3 = cache.GetAndResetRecentInfoSchemaTS(u64::MAX);
    // `keep_alive` only ever lowers the watermark (min-ts semantics), so the
    // reset reflects the smallest ts observed since the previous reset.
    assert_eq!(schema_ts3, 50);

    // After a reset with no further activity, the watermark stays at the
    // sentinel value passed in.
    let schema_ts4 = cache.GetAndResetRecentInfoSchemaTS(u64::MAX);
    assert_eq!(schema_ts4, u64::MAX);
}

/// Corresponds to Go `TestGCOldVersionPivotDeletedLeadsToTableNotExists`
/// (prepared-statement variant): GC of old btree versions must retain the
/// "pivot" record (latest version <= cutVer) for every table, so a snapshot
/// pinned at cutVer keeps resolving it.
/// 对应 Go 同名用例（预处理语句变体）：GC 必须保留 cutVer 处的 pivot 表记录。
/// GC：回收旧版本元数据；pivot：每个表在 cutVer 及之前的最新可见版本记录。
#[test]
fn test_gc_old_version_pivot_deleted_leads_to_table_not_exists() {
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());

    data.add(&db, Table::new(new_table(10, db.id, "t")), 1);
    // Bump schema version so cutVer sits strictly between t's creation and
    // its later ALTER.
    data.add(&db, Table::new(new_table(11, db.id, "bump")), 2);

    let cut_ver = 2;
    let is_txn = NewInfoSchemaV2(data.clone(), cut_ver, 1);
    is_txn
        .TableByName(&db.name, &ci("t"))
        .expect("t resolves before GC");

    // alter table t add column d int
    data.add(
        &db,
        Table::new(TableInfo {
            columns: vec![ColumnInfo {
                id: 1,
                name: ci("d"),
                auto_increment: false,
            }],
            ..new_table(10, db.id, "t")
        }),
        3,
    );

    data.GCOldVersion(cut_ver);

    // The transaction's pinned snapshot (schema version == cutVer) must
    // still resolve `t` via its retained pivot record.
    is_txn
        .TableByName(&db.name, &ci("t"))
        .expect("t must still resolve after GC at the pivot version");
}

/// Corresponds to Go `TestGCOldVersionPivotDeletedLeadsToTableNotExistsNormalSQL`
/// (plain-SQL variant of the test above; same underlying `Data` invariant).
/// 对应 Go 同名用例（普通 SQL 变体）：与上例相同的 pivot 保留不变量。
#[test]
fn test_gc_old_version_pivot_deleted_leads_to_table_not_exists_normal_sql() {
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());

    data.add(&db, Table::new(new_table(10, db.id, "t")), 1);
    data.add(&db, Table::new(new_table(11, db.id, "bump")), 2);

    let cut_ver = 2;
    let is_txn = NewInfoSchemaV2(data.clone(), cut_ver, 1);

    data.add(
        &db,
        Table::new(TableInfo {
            columns: vec![ColumnInfo {
                id: 1,
                name: ci("d"),
                auto_increment: false,
            }],
            ..new_table(10, db.id, "t")
        }),
        3,
    );

    data.GCOldVersion(cut_ver);

    is_txn
        .TableByName(&db.name, &ci("t"))
        .expect("select id from t must still resolve after GC at the pivot version");
}

/// Corresponds to Go `TestGCOldVersionPivotDeletedLeadsToReferredFKMissing`:
/// GC must retain the pivot `referredForeignKeys` record too, so a snapshot
/// pinned before a later foreign key was added still sees exactly the
/// foreign keys that existed as of that version.
/// 对应 Go 同名用例：GC 也须保留 referredForeignKeys 的 pivot，快照仍见当时 FK 集合。
/// 外键（FK）：子表引用父表的约束；referredForeignKeys：父表被哪些子表引用的反向索引。
#[test]
fn test_gc_old_version_pivot_deleted_leads_to_referred_fk_missing() {
    let db = new_db(1, "test");
    let data = NewData();
    data.addDB(1, db.clone());

    // create table parent (id int primary key)
    data.add(&db, Table::new(new_table(10, db.id, "parent")), 1);

    // create table child1 (pid int, foreign key fk1(pid) references parent(id))
    data.add(
        &db,
        Table::new(TableInfo {
            foreign_keys: vec![ForeignKeyInfo {
                name: ci("fk1"),
                ref_schema: db.name.clone(),
                ref_table: ci("parent"),
            }],
            ..new_table(11, db.id, "child1")
        }),
        2,
    );

    // create table bump (id int) -- bumps schema version so cutVer sits
    // between child1's fk and child2's fk below.
    data.add(&db, Table::new(new_table(12, db.id, "bump")), 3);
    let cut_ver = 3;

    // create table child2 (pid int, foreign key fk2(pid) references parent(id))
    data.add(
        &db,
        Table::new(TableInfo {
            foreign_keys: vec![ForeignKeyInfo {
                name: ci("fk2"),
                ref_schema: db.name.clone(),
                ref_table: ci("parent"),
            }],
            ..new_table(13, db.id, "child2")
        }),
        4,
    );

    let refs_before_gc = data.getTableReferredForeignKeys("test", "parent", 4);
    assert_eq!(refs_before_gc.len(), 2);

    data.GCOldVersion(cut_ver);

    // A read pinned at cutVer (before child2's fk was added) must still see
    // exactly child1's foreign key -- GC must not drop the pivot record.
    let refs_at_cut_ver = data.getTableReferredForeignKeys("test", "parent", cut_ver);
    assert_eq!(refs_at_cut_ver.len(), 1);
    assert_eq!(refs_at_cut_ver[0].child_table.lower, "child1");
}
