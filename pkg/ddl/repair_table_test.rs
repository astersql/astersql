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

// Copyright 2026 AsterSQL.
// Repair Table（修复表）相关单元测试。
//
// Repair Mode（修复模式）用于在元信息损坏或与期望结构不一致时，
// 通过 `ADMIN REPAIR TABLE` 用新的 CREATE TABLE 定义覆盖表元数据，
// 同时必须保留原有物理 ID（表/列/索引/分区 ID），以避免 KV 层数据失联。
// 本文件既保留与 Go 集成测试对应的注释块，也覆盖可独立运行的纯逻辑单测。

/*
//

// repairTableLease 对应 Go 常量，用于 mock store/domain 的 schema lease。
pub const repairTableLease: time::Duration = time::Duration::from_millis(600);

// test_repair_table 对应 Go 的 TestRepairTable。
// 它按原顺序覆盖非 repair mode、repair 列表校验、结构兼容校验、schema 拉取和 repair 详情。
#[test]
fn test_repair_table() {
    require::NoError(failpoint::Enable(
        "github.com/pingcap/tidb/pkg/infoschema/repairFetchCreateTable",
        "return(true)",
    ));
    defer!(|| {
        // Go defer 在测试结束时关闭 failpoint，避免影响后续测试。
        require::NoError(failpoint::Disable("github.com/pingcap/tidb/pkg/infoschema/repairFetchCreateTable"));
    });

    let (store, domain) = testkit::CreateMockStoreAndDomainWithSchemaLease(repairTableLease);
    let tk = testkit::NewTestKit(store.clone());
    tk.MustExec("use test");

    // Test repair table when TiDB is not in repair mode.
    tk.MustExec("CREATE TABLE t (a int primary key nonclustered, b varchar(10));");
    tk.MustGetErrMsg(
        "admin repair table t CREATE TABLE t (a float primary key, b varchar(5));",
        "[ddl:8215]Failed to repair table: TiDB is not in REPAIR MODE",
    );

    // Test repair table when the repaired list is empty.
    domainutil::RepairInfo.SetRepairMode(true);
    tk.MustGetErrMsg(
        "admin repair table t CREATE TABLE t (a float primary key, b varchar(5));",
        "[ddl:8215]Failed to repair table: repair list is empty",
    );

    // Test repair table when it's database isn't in repairInfo.
    domainutil::RepairInfo.SetRepairTableList(vec!["test.other_table"]);
    tk.MustGetErrMsg(
        "admin repair table t CREATE TABLE t (a float primary key, b varchar(5));",
        "[ddl:8215]Failed to repair table: database test is not in repair",
    );

    // Test repair table when the table isn't in repairInfo.
    tk.MustExec("CREATE TABLE other_table (a int, b varchar(1), key using hash(b));");
    tk.MustGetErrMsg(
        "admin repair table t CREATE TABLE t (a float primary key, b varchar(5));",
        "[ddl:8215]Failed to repair table: table t is not in repair",
    );

    // Test user can't access to the repaired table.
    tk.MustGetErrMsg("select * from other_table", "[schema:1146]Table 'test.other_table' doesn't exist");

    // Test create statement use the same name with what is in repaired.
    tk.MustGetErrMsg(
        "CREATE TABLE other_table (a int);",
        "[ddl:1103]Incorrect table name 'other_table'%!(EXTRA string=this table is in repair)",
    );

    // 下面几组断言逐项校验 repair CREATE TABLE 与原表的列、索引、类型兼容性。
    tk.MustGetErrMsg(
        "admin repair table other_table CREATE TABLE other_table (a int, c char(1));",
        "[ddl:8215]Failed to repair table: Column c has lost",
    );
    tk.MustGetErrMsg(
        "admin repair table other_table CREATE TABLE other_table (a bigint, b varchar(1), key using hash(b));",
        "[ddl:8215]Failed to repair table: Column a type should be the same",
    );
    tk.MustGetErrMsg(
        "admin repair table other_table CREATE TABLE other_table (a int unique);",
        "[ddl:8215]Failed to repair table: Index a has lost",
    );
    tk.MustGetErrMsg(
        "admin repair table other_table CREATE TABLE other_table (a int, b varchar(2) unique)",
        "[ddl:8215]Failed to repair table: Index b type should be the same",
    );

    // Test sub create statement in repair statement with the same name.
    tk.MustExec("admin repair table other_table CREATE TABLE other_table (a int);");

    // Test whether repair table name is case-sensitive.
    domainutil::RepairInfo.SetRepairMode(true);
    domainutil::RepairInfo.SetRepairTableList(vec!["test.other_table2"]);
    tk.MustExec("CREATE TABLE otHer_tAblE2 (a int, b varchar(1));");
    tk.MustExec("admin repair table otHer_tAblE2 CREATE TABLE otHeR_tAbLe (a int, b varchar(2));");
    let mut repairTable = external::GetTableByName(tk.clone(), "test", "otHeR_tAbLe");
    require::Equal("otHeR_tAbLe", repairTable.Meta().Name.O);

    // Test cannot repair table before fetch all schemas.
    tk.MustExec("CREATE TABLE otHer_tAblE2 (a int, b varchar(1));");
    domainutil::RepairInfo.SetRepairMode(true);
    domainutil::RepairInfo.SetRepairTableList(vec!["test.other_table2"]);
    tk.MustGetErrMsg(
        "admin repair table otHer_tAblE2 CREATE TABLE otHeR_tAbLe (a int, b varchar(2))",
        "[ddl:8215]Failed to repair table: database test is not in repair",
    );

    // Test can repair table after fetch all schemas.
    domainutil::RepairInfo.SetRepairMode(true);
    domainutil::RepairInfo.SetRepairTableList(vec!["test.other_table2"]);
    let snapshot = store.GetSnapshot(kv::NewVersion(mathutil::MaxUint));
    let m = meta::NewReader(snapshot);
    let (dbs, err) = domain.FetchAllSchemasWithTables(m);
    require::NoError(err);
    require::Equal(3, dbs.len());
    tk.MustExec("admin repair table otHer_tAblE2 CREATE TABLE otHeR_tAbLe (a int, b varchar(2));");

    // Test memory and system database is not for repair.
    domainutil::RepairInfo.SetRepairMode(true);
    domainutil::RepairInfo.SetRepairTableList(vec!["test.xxx"]);
    tk.MustGetErrMsg(
        "admin repair table performance_schema.xxx CREATE TABLE yyy (a int);",
        "[ddl:8215]Failed to repair table: memory or system database is not for repair",
    );

    // Test the repair detail.
    turnRepairModeAndInit(true);
    defer!(|| turnRepairModeAndInit(false));
    // Domain reload the tableInfo and add it into repairInfo.
    tk.MustExec("CREATE TABLE origin (a int primary key nonclustered auto_increment, b varchar(10), c int);");
    // Repaired tableInfo has been filtered by domain.InfoSchema(), so get it in repairInfo.
    let (originTableInfo, _) = domainutil::RepairInfo.GetRepairedTableInfoByTableName("test", "origin");

    let mut repairErr: Option<errors::Error> = None;
    testfailpoint::EnableCall(
        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
        |job: &model::Job| {
            if job.Type != model::ActionRepairTable {
                return;
            }
            if job.TableID != originTableInfo.ID {
                repairErr = Some(errors::New("table id should be the same"));
                return;
            }
            if job.SchemaState != model::StateNone {
                repairErr = Some(errors::New("repair job state should be the none"));
                return;
            }
            // 在 repair 表仍为 StateNone 时尝试读取；Go 期望用户侧看不到该表。
            let tk = testkit::NewTestKit(store.clone());
            tk.MustExec("use test");
            let (_, err) = tk.Exec("select * from origin");
            repairErr = err;
            if repairErr.is_some() && terror::ErrorEqual(repairErr.clone().unwrap(), infoschema::ErrTableNotExists) {
                repairErr = None;
            }
        },
    );

    // Exec the repair statement to override the tableInfo.
    tk.MustExec("admin repair table origin CREATE TABLE origin (a int primary key nonclustered auto_increment, b varchar(5), c int);");
    require::NoError(repairErr);

    // Check the repaired tableInfo is exactly the same with old one in tableID, indexID, colID.
    repairTable = external::GetTableByName(tk.clone(), "test", "origin");
    require::Equal(originTableInfo.ID, repairTable.Meta().ID);
    require::Equal(3, repairTable.Meta().Columns.len());
    require::Equal(originTableInfo.Columns[0].ID, repairTable.Meta().Columns[0].ID);
    require::Equal(originTableInfo.Columns[1].ID, repairTable.Meta().Columns[1].ID);
    require::Equal(originTableInfo.Columns[2].ID, repairTable.Meta().Columns[2].ID);
    require::Equal(1, repairTable.Meta().Indices.len());
    require::Equal(originTableInfo.Columns[0].ID, repairTable.Meta().Indices[0].ID);
    require::Equal(originTableInfo.AutoIncID, repairTable.Meta().AutoIncID);

    require::Equal(mysql::TypeLong, repairTable.Meta().Columns[0].GetType());
    require::Equal(mysql::TypeVarchar, repairTable.Meta().Columns[1].GetType());
    require::Equal(5, repairTable.Meta().Columns[1].GetFlen());
    require::Equal(mysql::TypeLong, repairTable.Meta().Columns[2].GetType());

    // Exec the show create table statement to make sure new tableInfo has been set.
    let result = tk.MustQuery("show create table origin");
    require::Equal(
        "CREATE TABLE `origin` (\n  `a` int(11) NOT NULL AUTO_INCREMENT,\n  `b` varchar(5) DEFAULT NULL,\n  `c` int(11) DEFAULT NULL,\n  PRIMARY KEY (`a`) /*T![clustered_index] NONCLUSTERED */
\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
        result.Rows()[0][1],
    );
}

// turnRepairModeAndInit 对应 Go helper：根据开关初始化 RepairInfo 的模式和表列表。
pub fn turnRepairModeAndInit(on: bool) {
    let mut list: Vec<&str> = Vec::new();
    if on {
        list.push("test.origin");
    }
    domainutil::RepairInfo.SetRepairMode(on);
    domainutil::RepairInfo.SetRepairTableList(list);
}

// test_repair_table_with_partition 对应 Go 的 TestRepairTableWithPartition。
// 它验证 range/hash 分区 repair 时必须保留旧分区语义，并继承原分区 ID。
#[test]
fn test_repair_table_with_partition() {
    require::NoError(failpoint::Enable(
        "github.com/pingcap/tidb/pkg/infoschema/repairFetchCreateTable",
        "return(true)",
    ));
    defer!(|| {
        require::NoError(failpoint::Disable("github.com/pingcap/tidb/pkg/infoschema/repairFetchCreateTable"));
    });
    let store = testkit::CreateMockStoreWithSchemaLease(repairTableLease);
    let tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists origin");

    turnRepairModeAndInit(true);
    defer!(|| turnRepairModeAndInit(false));
    // Domain reload the tableInfo and add it into repairInfo.
    tk.MustExec(
        "create table origin (a int not null) partition by RANGE(a) (partition p10 values less than (10),partition p30 values less than (30),partition p50 values less than (50),partition p70 values less than (70),partition p90 values less than (90));",
    );
    // Test for some old partition has lost.
    tk.MustGetErrMsg(
        "admin repair table origin create table origin (a int not null) partition by RANGE(a) (partition p10 values less than (10),partition p30 values less than (30),partition p50 values less than (50),partition p90 values less than (90),partition p100 values less than (100));",
        "[ddl:8215]Failed to repair table: Partition p100 has lost",
    );

    // Test for some partition changed the condition.
    tk.MustGetErrMsg(
        "admin repair table origin create table origin (a int not null) partition by RANGE(a) (partition p10 values less than (10),partition p20 values less than (25),partition p50 values less than (50),partition p90 values less than (90));",
        "[ddl:8215]Failed to repair table: Partition p20 has lost",
    );

    // Test for some partition changed the partition name.
    tk.MustGetErrMsg(
        "admin repair table origin create table origin (a int not null) partition by RANGE(a) (partition p10 values less than (10),partition p30 values less than (30),partition pNew values less than (50),partition p90 values less than (90));",
        "[ddl:8215]Failed to repair table: Partition pnew has lost",
    );

    let (originTableInfo, _) = domainutil::RepairInfo.GetRepairedTableInfoByTableName("test", "origin");
    tk.MustExec(
        "admin repair table origin create table origin_rename (a int not null) partition by RANGE(a) (partition p10 values less than (10),partition p30 values less than (30),partition p50 values less than (50),partition p90 values less than (90));",
    );
    let mut repairTable = external::GetTableByName(tk.clone(), "test", "origin_rename");
    require::Equal(originTableInfo.ID, repairTable.Meta().ID);
    require::Equal(1, repairTable.Meta().Columns.len());
    require::Equal(originTableInfo.Columns[0].ID, repairTable.Meta().Columns[0].ID);
    require::Equal(4, repairTable.Meta().Partition.Definitions.len());
    require::Equal(originTableInfo.Partition.Definitions[0].ID, repairTable.Meta().Partition.Definitions[0].ID);
    require::Equal(originTableInfo.Partition.Definitions[1].ID, repairTable.Meta().Partition.Definitions[1].ID);
    require::Equal(originTableInfo.Partition.Definitions[2].ID, repairTable.Meta().Partition.Definitions[2].ID);
    require::Equal(originTableInfo.Partition.Definitions[4].ID, repairTable.Meta().Partition.Definitions[3].ID);

    // Test hash partition.
    tk.MustExec("drop table if exists origin");
    domainutil::RepairInfo.SetRepairMode(true);
    domainutil::RepairInfo.SetRepairTableList(vec!["test.origin"]);
    tk.MustExec("create table origin (a varchar(1), b int not null, c int, key idx(c)) partition by hash(b) partitions 30");

    // Test partition num in repair should be exactly same with old one, otherwise will cause partition semantic problem.
    tk.MustGetErrMsg(
        "admin repair table origin create table origin (a varchar(2), b int not null, c int, key idx(c)) partition by hash(b) partitions 20",
        "[ddl:8215]Failed to repair table: Hash partition num should be the same",
    );

    let (originTableInfo, _) = domainutil::RepairInfo.GetRepairedTableInfoByTableName("test", "origin");
    tk.MustExec("admin repair table origin create table origin (a varchar(3), b int not null, c int, key idx(c)) partition by hash(b) partitions 30");
    repairTable = external::GetTableByName(tk, "test", "origin");
    require::Equal(originTableInfo.ID, repairTable.Meta().ID);
    require::Equal(30, repairTable.Meta().Partition.Definitions.len());
    require::Equal(originTableInfo.Partition.Definitions[0].ID, repairTable.Meta().Partition.Definitions[0].ID);
    require::Equal(originTableInfo.Partition.Definitions[1].ID, repairTable.Meta().Partition.Definitions[1].ID);
    require::Equal(originTableInfo.Partition.Definitions[29].ID, repairTable.Meta().Partition.Definitions[29].ID);
}
*/

use crate::executor::{
    ColumnInfo, ColumnKind, ExecutorError, Ident, IndexInfo, PartitionDefinition,
    RepairTableRegistry, TableInfo, repair_table_definition,
};

/// 构造一张“损坏待修”的样例表：含主键、多列与 RANGE 分区定义及固定物理 ID。
fn damaged_table() -> TableInfo {
    let mut a = ColumnInfo::integer("a");
    a.id = 11;
    a.nullable = false;
    let mut b = ColumnInfo::integer("b");
    b.id = 12;
    b.kind = ColumnKind::String;
    let mut c = ColumnInfo::integer("c");
    c.id = 13;

    let mut primary = IndexInfo::new("PRIMARY", vec!["a".into()]);
    primary.id = 21;
    primary.primary = true;
    primary.unique = true;

    let mut table = TableInfo::new("origin", vec![a, b, c]);
    table.id = 101;
    table.schema_id = 9;
    table.auto_increment = 43;
    table.indexes.push(primary);
    // 五个 RANGE 分区边界 10/30/50/70/90，对应物理分区 ID 301..305。
    table.partitions = ["10", "30", "50", "70", "90"]
        .into_iter()
        .enumerate()
        .map(|(index, bound)| {
            let mut partition = PartitionDefinition::new(format!("p{bound}"), vec![bound.into()]);
            partition.id = 301 + index as i64;
            partition
        })
        .collect();
    table
}

/// 验证 repair 会继承旧表的物理 ID / schema_id / auto_inc，并允许删分区与改名。
#[test]
fn repair_preserves_physical_ids_and_allows_metadata_changes() {
    let old = damaged_table();
    let mut replacement = old.clone();
    // 故意清零新定义中的物理 ID，确认 repair 会从旧表回填。
    replacement.id = 0;
    replacement.name = "origin_rename".into();
    replacement.auto_increment = 0;
    replacement.columns[0].id = 0;
    replacement.columns[1].id = 0;
    replacement.columns[2].id = 0;
    replacement.indexes[0].id = 0;
    // 移除 p70（原 index 3），剩余分区仍按名称匹配继承旧 ID。
    replacement.partitions.remove(3);
    for partition in &mut replacement.partitions {
        partition.id = 0;
    }

    let repaired = repair_table_definition(&old, replacement, false).unwrap();
    assert_eq!(101, repaired.id);
    assert_eq!(9, repaired.schema_id);
    assert_eq!(43, repaired.auto_increment);
    assert_eq!(
        vec![11, 12, 13],
        repaired.columns.iter().map(|c| c.id).collect::<Vec<_>>()
    );
    assert_eq!(21, repaired.indexes[0].id);
    assert_eq!(
        vec![301, 302, 303, 305],
        repaired.partitions.iter().map(|p| p.id).collect::<Vec<_>>()
    );
}

/// 验证列丢失/类型变更、索引类型变更、分区丢失、Hash 分区数不一致时均被拒绝。
#[test]
fn repair_rejects_lost_or_incompatible_objects() {
    let old = damaged_table();

    let mut unknown_column = old.clone();
    unknown_column.columns[1].name = "lost".into();
    assert!(matches!(
        repair_table_definition(&old, unknown_column, false),
        Err(ExecutorError::InvalidTableDefinition(message)) if message == "Column lost has lost"
    ));

    let mut changed_type = old.clone();
    changed_type.columns[0].kind = ColumnKind::String;
    assert!(matches!(
        repair_table_definition(&old, changed_type, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Column a type should be the same"
    ));

    let mut changed_index = old.clone();
    changed_index.indexes[0].unique = false;
    assert!(matches!(
        repair_table_definition(&old, changed_index, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Index PRIMARY type should be the same"
    ));

    let mut unknown_index = old.clone();
    unknown_index.indexes[0].name = "lost".into();
    assert!(matches!(
        repair_table_definition(&old, unknown_index, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Index lost has lost"
    ));

    let mut unknown_partition = old.clone();
    unknown_partition.partitions[2].name = "pnew".into();
    assert!(matches!(
        repair_table_definition(&old, unknown_partition, false),
        Err(ExecutorError::InvalidPartition(message)) if message == "Partition pnew has lost"
    ));

    let mut changed_partition_bound = old.clone();
    changed_partition_bound.partitions[1].less_than = vec!["25".into()];
    assert!(matches!(
        repair_table_definition(&old, changed_partition_bound, false),
        Err(ExecutorError::InvalidPartition(message)) if message == "Partition p30 has lost"
    ));

    // Hash 分区语义依赖分区数量固定；减少分区数必须失败。
    let mut fewer_hash_partitions = old.clone();
    fewer_hash_partitions.partitions.pop();
    assert!(matches!(
        repair_table_definition(&old, fewer_hash_partitions, true),
        Err(ExecutorError::InvalidPartition(message))
            if message == "Hash partition num should be the same"
    ));
}

/// 验证修复注册表：需开启 Repair Mode、列表非空，修复成功后表才对外可见。
#[test]
fn repair_registry_enforces_mode_fetch_visibility_and_successful_removal() {
    let ident = Ident::new("test", "origin");
    let old = damaged_table();
    let mut registry = RepairTableRegistry::default();

    // 未开启 Repair Mode 时拒绝 repair。
    assert!(matches!(
        registry.repair(&ident, old.clone(), false),
        Err(ExecutorError::Unsupported(message)) if message == "TiDB is not in REPAIR MODE"
    ));
    registry.set_mode(true);
    assert!(matches!(
        registry.repair(&ident, old.clone(), false),
        Err(ExecutorError::Unsupported(message)) if message == "repair list is empty"
    ));

    // fetch 后表对 infoschema 不可见，直到 repair 成功移除。
    registry.fetch_tables([(ident.clone(), old.clone())]);
    assert!(!registry.is_visible(&ident));
    let repaired = registry.repair(&ident, old, false).unwrap();
    assert_eq!(101, repaired.id);
    assert!(registry.is_visible(&ident));
}

/// 对应 Go 中系统库拒绝、标识符大小写不敏感，以及失败 repair 不移除待修表。
#[test]
fn repair_registry_matches_go_error_and_retry_contracts() {
    let old = damaged_table();
    let mut registry = RepairTableRegistry::default();
    registry.set_mode(true);
    registry.fetch_tables([(Ident::new("test", "origin"), old.clone())]);

    let system_ident = Ident::new("performance_schema", "origin");
    assert!(matches!(
        registry.repair(&system_ident, old.clone(), false),
        Err(ExecutorError::Unsupported(message))
            if message == "memory or system database is not for repair"
    ));

    let mixed_case_ident = Ident::new("TeSt", "OrIgIn");
    assert!(!registry.is_visible(&mixed_case_ident));

    let mut incompatible = old.clone();
    incompatible.columns[0].kind = ColumnKind::String;
    assert!(matches!(
        registry.repair(&mixed_case_ident, incompatible, false),
        Err(ExecutorError::InvalidTableDefinition(message))
            if message == "Column a type should be the same"
    ));
    assert!(!registry.is_visible(&mixed_case_ident));

    let repaired = registry.repair(&mixed_case_ident, old, false).unwrap();
    assert_eq!(101, repaired.id);
    assert!(registry.is_visible(&mixed_case_ident));
}
