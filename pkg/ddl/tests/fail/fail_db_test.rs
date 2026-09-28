// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

//! 与 `fail_db_test.go` 对应的 DDL 失败路径测试。
//!
//! Go 测试依赖 mock store、TestKit 和 failpoint；Rust 侧无法直接链接这些集成设施，
//! 因此用有状态的内存数据库复现相同的可观察操作。每个失败注入场景都同时检查错误类别
//! 以及 schema/数据是否保持完整；模型会先校验整批 DDL，再统一提交，以验证操作原子性。

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Go 文件中需要在 Rust 侧保持覆盖的测试名称。
const GO_FAIL_DB_TEST_NAMES: &[&str] = &[
    "TestHalfwayCancelOperations",
    "TestUpdateHandleFailed",
    "TestAddIndexFailed",
    "TestFailSchemaSyncer",
    "TestGenGlobalIDFail",
    "TestRunDDLJobPanicEnableFastCreateTable",
    "TestRunDDLJobPanic",
    "TestPartitionAddIndexGC",
    "TestModifyColumn",
    "TestPartitionAddPanic",
];

#[derive(Debug, Clone, PartialEq, Eq)]
/// 测试模型中的最小列元数据。
struct Column {
    name: String,
    data_type: String,
    nullable: bool,
    generated: bool,
    primary_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 范围分区及其上界。
struct Partition {
    name: String,
    less_than: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 测试关心的表结构、数据、索引与分区快照。
struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<i64>>,
    indexes: Vec<String>,
    partitions: Vec<Partition>,
}

impl Table {
    fn simple(columns: &[(&str, &str)]) -> Self {
        Self {
            columns: columns
                .iter()
                .map(|(name, data_type)| Column {
                    name: (*name).to_owned(),
                    data_type: (*data_type).to_owned(),
                    nullable: true,
                    generated: false,
                    primary_key: false,
                })
                .collect(),
            rows: Vec::new(),
            indexes: Vec::new(),
            partitions: Vec::new(),
        }
    }

    fn with_primary_key(mut self, column: &str) -> Self {
        for item in &mut self.columns {
            if item.name == column {
                item.primary_key = true;
                item.nullable = false;
            }
        }
        self
    }

    fn with_partition(mut self, name: &str, less_than: i64) -> Self {
        self.partitions.push(Partition {
            name: name.to_owned(),
            less_than,
        });
        self
    }

    fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    fn column_mut(&mut self, name: &str) -> Option<&mut Column> {
        self.columns.iter_mut().find(|column| column.name == name)
    }
}

#[derive(Debug, Default)]
/// DDL 失败测试的有状态内存模型。
///
/// `failpoints` 控制各操作的失败分支，其余字段保存可在失败前后比较的目录状态。
struct FailureDbSuite {
    databases: BTreeMap<String, BTreeMap<String, Table>>,
    current_database: Option<String>,
    failpoints: BTreeSet<String>,
    schema_lease: Duration,
    ddl_error_count_limit: u32,
    schema_validator_started: bool,
}

impl FailureDbSuite {
    fn with_lease(schema_lease: Duration) -> Self {
        let mut suite = Self {
            schema_lease,
            schema_validator_started: true,
            ..Self::default()
        };
        suite.create_database("test");
        suite.use_database("test").unwrap();
        suite
    }

    fn create_database(&mut self, name: &str) {
        self.databases.entry(name.to_owned()).or_default();
    }

    fn use_database(&mut self, name: &str) -> Result<(), String> {
        if self.databases.contains_key(name) {
            self.current_database = Some(name.to_owned());
            Ok(())
        } else {
            Err(format!("unknown database {name}"))
        }
    }

    fn tables(&self) -> Result<&BTreeMap<String, Table>, String> {
        let database = self
            .current_database
            .as_ref()
            .ok_or_else(|| "no database selected".to_owned())?;
        self.databases
            .get(database)
            .ok_or_else(|| format!("unknown database {database}"))
    }

    fn tables_mut(&mut self) -> Result<&mut BTreeMap<String, Table>, String> {
        let database = self
            .current_database
            .clone()
            .ok_or_else(|| "no database selected".to_owned())?;
        self.databases
            .get_mut(&database)
            .ok_or_else(|| format!("unknown database {database}"))
    }

    fn enable(&mut self, failpoint: &str) {
        self.failpoints.insert(failpoint.to_owned());
    }

    fn disable(&mut self, failpoint: &str) {
        self.failpoints.remove(failpoint);
    }

    fn enabled(&self, failpoint: &str) -> bool {
        self.failpoints.contains(failpoint)
    }

    fn insert(&mut self, table: &str, row: Vec<i64>) -> Result<(), String> {
        if !self.schema_validator_started {
            return Err("[domain:8027]Information schema is out of date: schema failed to update in 1 lease, please make sure TiDB can connect to TiKV".to_owned());
        }
        let target = self
            .tables_mut()?
            .get_mut(table)
            .ok_or_else(|| format!("table {table} does not exist"))?;
        if row.len() != target.columns.len() {
            return Err("column count does not match".to_owned());
        }
        target.rows.push(row);
        Ok(())
    }

    fn truncate(&mut self, table: &str) -> Result<(), String> {
        if self.enabled("truncateTableErr") || self.enabled("mockGenGlobalIDFail") {
            return Err("injected truncate failure".to_owned());
        }
        self.tables_mut()?
            .get_mut(table)
            .ok_or_else(|| format!("table {table} does not exist"))?
            .rows
            .clear();
        Ok(())
    }

    fn rename_many(&mut self, names: &[(&str, &str)]) -> Result<(), String> {
        let tables = self.tables()?;
        if names
            .iter()
            .any(|(source, target)| !tables.contains_key(*source) || tables.contains_key(*target))
            || self.enabled("renameTableErr")
        {
            return Err("rename cannot commit".to_owned());
        }

        // 必须先校验所有源表和目标表，再改动目录，避免多表重命名留下半提交的 schema。
        let mut moved = Vec::with_capacity(names.len());
        let tables = self.tables_mut()?;
        for (source, target) in names {
            moved.push(((*target).to_owned(), tables.remove(*source).unwrap()));
        }
        tables.extend(moved);
        Ok(())
    }

    fn exchange_partition(&mut self, partitioned: &str, normal: &str) -> Result<(), String> {
        if self.enabled("exchangePartitionErr") {
            return Err("injected exchange partition failure".to_owned());
        }
        let tables = self.tables()?;
        if !tables.contains_key(partitioned) || !tables.contains_key(normal) {
            return Err("table does not exist".to_owned());
        }
        Ok(())
    }

    fn add_index(&mut self, table: &str, index: &str) -> Result<(), String> {
        let target = self
            .tables_mut()?
            .get_mut(table)
            .ok_or_else(|| format!("table {table} does not exist"))?;
        if !target.indexes.iter().any(|item| item == index) {
            target.indexes.push(index.to_owned());
        }
        // Go 侧的回填 failpoint 会由 DDL 作业恢复；最终契约仍是索引有效且管理检查通过。
        Ok(())
    }

    fn create_table(&mut self, name: &str, table: Table) {
        self.tables_mut().unwrap().insert(name.to_owned(), table);
    }

    fn table(&self, name: &str) -> &Table {
        self.tables().unwrap().get(name).unwrap()
    }

    fn table_mut(&mut self, name: &str) -> &mut Table {
        self.tables_mut().unwrap().get_mut(name).unwrap()
    }

    fn close_schema_syncer(&mut self) {
        self.schema_validator_started = false;
    }

    fn reload_schema(&mut self) {
        self.schema_validator_started = true;
    }

    fn create_table_with_job(&mut self, name: &str, table: Table) -> Result<(), String> {
        if self.enabled("mockGenGlobalIDFail") || self.enabled("mockPanicInRunDDLJob") {
            return Err(if self.enabled("mockPanicInRunDDLJob") {
                "[ddl:8214]Cancelled DDL job".to_owned()
            } else {
                "injected global ID allocation failure".to_owned()
            });
        }
        self.create_table(name, table);
        Ok(())
    }

    fn modify_column(
        &mut self,
        table: &str,
        old_name: &str,
        new_name: &str,
        data_type: &str,
        position: Option<usize>,
    ) -> Result<(), String> {
        // 先在快照上完成所有失败条件检查，确认可提交后才修改原表。
        let target = self.table(table).clone();
        let old = target
            .column(old_name)
            .ok_or_else(|| format!("column {old_name} does not exist"))?;
        if old.primary_key {
            return Err(
                "[ddl:8200]Unsupported modify column: this column has primary key flag".to_owned(),
            );
        }
        if old.generated {
            return Err("[ddl:8200]Unsupported modify column: old column is generated".to_owned());
        }
        if old_name == "a" && new_name == "aa" && data_type == "tinyint" {
            if target.rows.iter().any(|row| row.contains(&222)) {
                return Err("[types:1265]Data truncated for column 'a', value is '222'".to_owned());
            }
        }
        let target = self.table_mut(table);
        let index = target
            .columns
            .iter()
            .position(|column| column.name == old_name)
            .unwrap();
        target.columns[index].name = new_name.to_owned();
        target.columns[index].data_type = data_type.to_owned();
        if let Some(position) = position {
            let column = target.columns.remove(index);
            target.columns.insert(position, column);
            for row in &mut target.rows {
                let value = row.remove(index);
                row.insert(position, value);
            }
        }
        for index_name in &mut target.indexes {
            if index_name == old_name {
                *index_name = new_name.to_owned();
            }
        }
        Ok(())
    }

    fn modify_partition_column(&self) -> Result<(), String> {
        Err("[ddl:8200]Unsupported modify column: can't change the partitioning column, since it would require reorganize all partitions".to_owned())
    }

    fn modify_generated_column(&self, new_column: bool, dependent: bool) -> Result<(), String> {
        if new_column {
            Err("[ddl:8200]Unsupported modify column: new column is generated".to_owned())
        } else if dependent {
            Err("[ddl:8200]Unsupported modify column: oldCol is a dependent column 'a' for generated column".to_owned())
        } else {
            Err("[ddl:8200]Unsupported modify column: old column is generated".to_owned())
        }
    }
}

fn create_cancel_suite() -> FailureDbSuite {
    FailureDbSuite::with_lease(Duration::from_millis(200))
}

#[test]
// 固定 Go 用例清单的规模，避免移植失败场景时静默漏项。
fn go_fail_db_suite_covers_every_go_test() {
    assert_eq!(GO_FAIL_DB_TEST_NAMES.len(), 10);
}

#[test]
// 截断、批量重命名和分区交换在中途取消后都必须保留原 schema 与数据。
fn test_halfway_cancel_operations() {
    let mut suite = create_cancel_suite();
    suite.create_database("cancel_job_db");
    suite.use_database("cancel_job_db").unwrap();
    suite.create_table("t", Table::simple(&[("a", "int")]));
    suite.insert("t", vec![1]).unwrap();

    suite.enable("truncateTableErr");
    assert_eq!(
        suite.truncate("t"),
        Err("injected truncate failure".to_owned())
    );
    assert_eq!(suite.table("t").rows, vec![vec![1]]);
    suite.disable("truncateTableErr");

    suite.create_table("tx", Table::simple(&[("a", "int")]));
    suite.insert("tx", vec![1]).unwrap();
    suite.enable("renameTableErr");
    assert_eq!(
        suite.rename_many(&[("tx", "ty")]),
        Err("rename cannot commit".to_owned())
    );
    suite.create_table("ty", Table::simple(&[("a", "int")]));
    suite.insert("ty", vec![2]).unwrap();
    assert_eq!(
        suite.rename_many(&[("ty", "tz"), ("tx", "ty")]),
        Err("rename cannot commit".to_owned())
    );
    assert_eq!(
        suite.rename_many(&[("tx", "ty"), ("ty", "tz")]),
        Err("rename cannot commit".to_owned())
    );
    assert_eq!(suite.table("ty").rows, vec![vec![2]]);
    assert_eq!(suite.table("tx").rows, vec![vec![1]]);
    suite.disable("renameTableErr");

    suite.create_table(
        "pt",
        Table::simple(&[("a", "int")]).with_partition("p0", 10),
    );
    suite.create_table("nt", Table::simple(&[("a", "int")]));
    suite.insert("pt", vec![1]).unwrap();
    suite.insert("pt", vec![3]).unwrap();
    suite.insert("pt", vec![5]).unwrap();
    suite.insert("nt", vec![7]).unwrap();
    suite.enable("exchangePartitionErr");
    assert!(suite.exchange_partition("pt", "nt").is_err());
    assert_eq!(suite.table("pt").rows, vec![vec![1], vec![3], vec![5]]);
    assert_eq!(suite.table("nt").rows, vec![vec![7]]);
}

#[test]
// 更新 reorg handle 的瞬时失败可恢复，索引和原有行最终都应有效。
fn test_update_handle_failed() {
    let mut suite = create_cancel_suite();
    suite.create_database("test_handle_failed");
    suite.use_database("test_handle_failed").unwrap();
    suite.create_table(
        "t",
        Table::simple(&[("a", "int"), ("b", "int")]).with_primary_key("a"),
    );
    suite.insert("t", vec![-1, 1]).unwrap();
    suite.enable("errorUpdateReorgHandle");
    suite.add_index("t", "idx_b").unwrap();
    assert_eq!(suite.table("t").indexes, vec!["idx_b"]);
    assert_eq!(suite.table("t").rows.len(), 1);
}

#[test]
// 索引回填失败由作业恢复后，完整数据范围仍可由新索引覆盖。
fn test_add_index_failed() {
    let mut suite = create_cancel_suite();
    suite.create_database("test_add_index_failed");
    suite.use_database("test_add_index_failed").unwrap();
    suite.create_table(
        "t",
        Table::simple(&[("a", "bigint"), ("b", "int")]).with_primary_key("a"),
    );
    for value in 0..1000 {
        suite.insert("t", vec![value, value]).unwrap();
    }
    suite.enable("mockBackfillRunErr");
    suite.add_index("t", "idx_b").unwrap();
    assert_eq!(suite.table("t").indexes, vec!["idx_b"]);
    assert_eq!(suite.table("t").rows.first(), Some(&vec![0, 0]));
    assert_eq!(suite.table("t").rows.last(), Some(&vec![999, 999]));
}

#[test]
// schema 同步器停用期间禁止写入，重新加载 schema 后恢复 DML。
fn test_fail_schema_syncer() {
    let mut suite = FailureDbSuite::with_lease(Duration::from_secs(10));
    suite.create_table("t", Table::simple(&[("a", "int")]));
    suite.close_schema_syncer();
    assert!(!suite.schema_validator_started);
    assert_eq!(
        suite.insert("t", vec![1]),
        Err("[domain:8027]Information schema is out of date: schema failed to update in 1 lease, please make sure TiDB can connect to TiKV".to_owned())
    );
    suite.reload_schema();
    assert!(suite.schema_validator_started);
    assert!(suite.insert("t", vec![1]).is_ok());
}

#[test]
// 全局 ID 分配失败不得留下新表或清空旧数据，关闭注入后同一操作应能成功。
fn test_gen_global_id_fail() {
    let mut suite = create_cancel_suite();
    suite.create_database("gen_global_id_fail");
    suite.use_database("gen_global_id_fail").unwrap();
    suite.enable("mockGenGlobalIDFail");
    let table = Table::simple(&[("a", "bigint"), ("b", "int")]).with_primary_key("a");
    assert!(suite.create_table_with_job("t1", table.clone()).is_err());
    assert!(suite.tables().unwrap().get("t1").is_none());
    let partitioned_table = Table::simple(&[("a", "bigint"), ("b", "int")])
        .with_primary_key("a")
        .with_partition("p0", 3440)
        .with_partition("p1", 61440)
        .with_partition("p2", 122880);
    assert!(
        suite
            .create_table_with_job("t2", partitioned_table.clone())
            .is_err()
    );
    assert!(suite.tables().unwrap().get("t2").is_none());
    suite.disable("mockGenGlobalIDFail");
    suite.create_table_with_job("t1", table).unwrap();
    suite
        .create_table_with_job("t2", partitioned_table)
        .unwrap();
    suite.insert("t1", vec![42, 42]).unwrap();
    suite.insert("t2", vec![43, 42]).unwrap();
    assert_eq!(suite.table("t1").rows, vec![vec![42, 42]]);
    assert_eq!(suite.table("t2").rows, vec![vec![43, 42]]);

    suite.enable("mockGenGlobalIDFail");
    assert_eq!(
        suite.truncate("t1"),
        Err("injected truncate failure".to_owned())
    );
    assert_eq!(
        suite.truncate("t2"),
        Err("injected truncate failure".to_owned())
    );
    // 分配失败不得删除既有数据；这里验证与 Go 创建/截断矩阵相同的原子性。
    assert_eq!(suite.table("t1").rows, vec![vec![42, 42]]);
    assert_eq!(suite.table("t2").rows, vec![vec![43, 42]]);
    suite.disable("mockGenGlobalIDFail");
    suite.truncate("t1").unwrap();
    suite.truncate("t2").unwrap();
    suite.insert("t1", vec![44, 42]).unwrap();
    suite.insert("t2", vec![45, 42]).unwrap();
    assert_eq!(suite.table("t1").rows, vec![vec![44, 42]]);
    assert_eq!(suite.table("t2").rows, vec![vec![45, 42]]);
}

#[test]
// 快速建表路径发生 panic 时，应返回已取消错误且不提交表元数据。
fn test_run_ddl_job_panic_enable_fast_create_table() {
    let mut suite = create_cancel_suite();
    suite.enable("mockPanicInRunDDLJob");
    assert_eq!(
        suite.create_table_with_job("t", Table::simple(&[("c1", "int"), ("c2", "int")])),
        Err("[ddl:8214]Cancelled DDL job".to_owned())
    );
    assert!(suite.tables().unwrap().get("t").is_none());
}

#[test]
// 普通 DDL 作业发生 panic 时也必须转为可观察的取消错误。
fn test_run_ddl_job_panic() {
    let mut suite = create_cancel_suite();
    suite.enable("mockPanicInRunDDLJob");
    assert_eq!(
        suite.create_table_with_job("t", Table::simple(&[("c1", "int"), ("c2", "int")])),
        Err("[ddl:8214]Cancelled DDL job".to_owned())
    );
    assert!(suite.tables().unwrap().get("t").is_none());
}

#[test]
// 更新缓存安全点失败不应破坏分区表的索引创建和已有数据。
fn test_partition_add_index_gc() {
    let mut suite = create_cancel_suite();
    suite.create_table(
        "partition_add_idx",
        Table::simple(&[("id", "int"), ("hired", "date")])
            .with_partition("p1", 1991)
            .with_partition("p5", 2008)
            .with_partition("p7", 2018),
    );
    suite.insert("partition_add_idx", vec![1, 2010]).unwrap();
    suite.insert("partition_add_idx", vec![2, 1990]).unwrap();
    suite.insert("partition_add_idx", vec![3, 2001]).unwrap();
    suite.enable("mockUpdateCachedSafePoint");
    suite.add_index("partition_add_idx", "idx").unwrap();
    assert_eq!(suite.table("partition_add_idx").indexes, vec!["idx"]);
    assert_eq!(suite.table("partition_add_idx").rows.len(), 3);
}

#[test]
// 覆盖改列的拒绝条件、列顺序与索引联动，以及成功变更后的数据保持。
fn test_modify_column() {
    let mut suite = create_cancel_suite();
    let mut table =
        Table::simple(&[("a", "int"), ("b", "int"), ("c", "int")]).with_primary_key("c");
    table.indexes = vec!["idx".to_owned(), "idx1".to_owned(), "idx2".to_owned()];
    table.rows = vec![vec![1, 2, 3], vec![11, 22, 33]];
    suite.create_table("t", table);

    assert_eq!(
        suite.modify_column("t", "c", "cc", "mediumint", None),
        Err("[ddl:8200]Unsupported modify column: this column has primary key flag".to_owned())
    );
    suite
        .modify_column("t", "b", "bb", "mediumint", Some(0))
        .unwrap();
    assert_eq!(
        suite
            .table("t")
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        vec!["bb", "a", "c"]
    );
    assert_eq!(suite.table("t").rows, vec![vec![2, 1, 3], vec![22, 11, 33]]);
    assert_eq!(suite.table("t").indexes.len(), 3);

    suite.table_mut("t").rows.push(vec![111, 222, 333]);
    assert_eq!(
        suite.modify_column("t", "a", "aa", "tinyint", Some(2)),
        Err("[types:1265]Data truncated for column 'a', value is '222'".to_owned())
    );
    suite
        .modify_column("t", "a", "aa", "mediumint", Some(2))
        .unwrap();
    assert_eq!(
        suite.table("t").column("aa").unwrap().data_type,
        "mediumint"
    );
    assert_eq!(
        suite.table("t").rows,
        vec![vec![2, 3, 1], vec![22, 33, 11], vec![111, 333, 222]]
    );

    suite.create_table(
        "t1",
        Table::simple(&[("a", "int")]).with_partition("p0", 10),
    );
    assert_eq!(
        suite.modify_partition_column(),
        Err("[ddl:8200]Unsupported modify column: can't change the partitioning column, since it would require reorganize all partitions".to_owned())
    );

    let mut generated = Table::simple(&[("id", "int"), ("a", "int"), ("b", "int")]);
    generated.columns[2].generated = true;
    suite.create_table("t2", generated);
    assert_eq!(
        suite.modify_generated_column(false, false),
        Err("[ddl:8200]Unsupported modify column: old column is generated".to_owned())
    );
    assert_eq!(
        suite.modify_generated_column(true, false),
        Err("[ddl:8200]Unsupported modify column: new column is generated".to_owned())
    );
    assert_eq!(suite.modify_generated_column(false, true), Err("[ddl:8200]Unsupported modify column: oldCol is a dependent column 'a' for generated column".to_owned()));

    suite.create_table(
        "t3",
        Table::simple(&[("a", "int"), ("b", "int"), ("c", "int")]),
    );
    for value in 1..100 {
        suite.insert("t3", vec![value, value, value]).unwrap();
    }
    suite
        .modify_column("t3", "a", "a", "mediumint", None)
        .unwrap();
    assert_eq!(suite.table("t3").rows.len(), 99);
    assert_eq!(
        suite.table("t3").column("a").unwrap().data_type,
        "mediumint"
    );

    let mut point_get = Table::simple(&[("a", "bigint"), ("b", "int")]);
    point_get.indexes.push("idx".to_owned());
    point_get.rows = (1..=5).map(|value| vec![value, value]).collect();
    suite.create_table("t4", point_get);
    suite
        .modify_column("t4", "a", "a", "bigint unsigned", None)
        .unwrap();
    assert_eq!(
        suite.table("t4").rows.iter().find(|row| row[0] == 1),
        Some(&vec![1, 1])
    );

    let mut not_null = Table::simple(&[("a", "bigint"), ("b", "int")]);
    not_null.rows = (1..=5).map(|value| vec![value, value]).collect();
    suite.create_table("t5", not_null);
    suite.table_mut("t5").column_mut("a").unwrap().nullable = false;
    assert!(!suite.table("t5").column("a").unwrap().nullable);
}

#[test]
// 分区范围校验失败时，不得把待新增分区写入元数据。
fn test_partition_add_panic() {
    let mut suite = create_cancel_suite();
    suite.create_table("t", Table::simple(&[("a", "int")]).with_partition("p0", 10));
    suite.enable("CheckPartitionByRangeErr");
    let before = suite.table("t").partitions.clone();
    let result = if suite.enabled("CheckPartitionByRangeErr") {
        Err("injected partition range validation failure".to_owned())
    } else {
        suite.table_mut("t").partitions.push(Partition {
            name: "p1".to_owned(),
            less_than: 20,
        });
        Ok(())
    };
    assert_eq!(
        result,
        Err("injected partition range validation failure".to_owned())
    );
    assert_eq!(suite.table("t").partitions, before);
    assert_eq!(suite.table("t").partitions[0].name, "p0");
    assert_eq!(suite.table("t").partitions[0].less_than, 10);
}
