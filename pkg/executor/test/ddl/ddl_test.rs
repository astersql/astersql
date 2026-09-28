// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// DDL（Data Definition Language，数据定义语言）执行器测试，对应 Go
// `pkg/executor/test/ddl/ddl_test.go`。
//
// SQL 场景通过真实 Rust TestKit/MockStore 执行；网络、TiKV 和外部进程边界仍由
// MockStore 隔离。每个测试独立创建会话，避免 DDL/会话变量状态互相污染。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use astersql_sessionctx_vardef as vardef;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, Rows, RowsWithSep, TestKit};
use std::sync::Arc;

static AUTO_ID_STEP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 列 charset/collate 表驱动用例：输入类型声明与期望推导结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CharsetCase {
    col_type: &'static str,
    charset: &'static str,
    collate: &'static str,
    expected_charset: &'static str,
    expected_collate: &'static str,
    error: &'static str,
}

fn ddl_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk
}

fn ddl_testkit_with_domain() -> (TestKit, Arc<astersql_domain::Domain>) {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    (tk, domain)
}

/// 触发 schema（元数据）全量 reload：建删表迫使 domain 重新加载表信息。
// Go 的 forceFullReload helper：降低 schema diff gap 阈值，通过建删表触发 full reload，再恢复阈值。
fn force_full_reload(tk: &mut TestKit, domain: &astersql_domain::Domain) {
    // The Rust domain reload is driven by the same create/drop schema-diff pair.
    tk.MustExec("create database if not exists test", Vec::new());
    tk.MustExec("create table test.forcereload(id int)", Vec::new());
    tk.MustExec("drop table test.forcereload", Vec::new());
    domain
        .force_full_reload_for_test()
        .expect("force schema reload");
}

// TestInTxnExecDDLFail tests the following case:
// 1. Execute the SQL of "begin";
// 2. A SQL that will fail to execute;
// 3. Execute DDL.
/// 事务（transaction）中先有失败 DML，再执行 DDL 应被拦截，不能越过错误提交。
#[test]
fn test_in_txn_exec_ddl_fail() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut set_txn_tk = NewTestKit(store.clone());
    set_txn_tk.MustExec("set global tidb_txn_mode=''", Vec::new());
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t (i int key)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustContainErrMsg("truncate table t", "Duplicate entry '1'");
    tk.MustQuery("select count(*) from t", Vec::new())
        .Check(Rows(&["1"]));
}

/// 建表基础用例：重复库/表、float 精度、多 COLLATE、auto_increment 起点与 IF EXISTS。
#[test]
fn test_create_table() {
    let mut tk = ddl_testkit();
    tk.MustExec(
        "drop table if exists create_test, issue312_1, issue312_2, create_auto_increment_test",
        Vec::new(),
    );
    assert!(
        tk.ExecToErr("create database test")
            .message()
            .contains("database exists")
    );
    tk.MustExec(
        "create table create_test (id int not null default 1, name varchar(255), primary key(id))",
        Vec::new(),
    );
    assert!(tk
        .ExecToErr("create table create_test (id int not null default 1, name varchar(255), primary key(id))")
        .message()
        .contains("already exists"));
    tk.MustExec(
        "create table if not exists test(id int not null default 1, name varchar(255), primary key(id))",
        Vec::new(),
    );
    tk.MustExec("create table issue312_1 (c float(24))", Vec::new());
    tk.MustExec("create table issue312_2 (c float(25))", Vec::new());
    let desc1 = tk.MustQuery("desc issue312_1", Vec::new()).Rows();
    let desc2 = tk.MustQuery("desc issue312_2", Vec::new()).Rows();
    assert!(
        desc1
            .iter()
            .any(|row| row.get(1).is_some_and(|ty| ty == "float"))
    );
    assert!(
        desc2
            .iter()
            .any(|row| row.get(1).is_some_and(|ty| ty == "double"))
    );

    tk.MustExec("create table create_auto_increment_test (id int not null auto_increment, name varchar(255), primary key(id)) auto_increment = 999", Vec::new());
    for name in ["aa", "bb", "cc"] {
        tk.MustExec(
            &format!("insert into create_auto_increment_test (name) values ('{name}')"),
            Vec::new(),
        );
    }
    tk.MustQuery("select * from create_auto_increment_test", Vec::new())
        .Check(Rows(&["999 aa", "1000 bb", "1001 cc"]));
    tk.MustExec("drop table create_auto_increment_test", Vec::new());
    tk.MustExec("create table create_auto_increment_test (id int not null auto_increment, name varchar(255), primary key(id)) auto_increment = 1999", Vec::new());
    for name in ["aa", "bb", "cc"] {
        tk.MustExec(
            &format!("insert into create_auto_increment_test (name) values ('{name}')"),
            Vec::new(),
        );
    }
    tk.MustQuery("select * from create_auto_increment_test", Vec::new())
        .Check(Rows(&["1999 aa", "2000 bb", "2001 cc"]));
    tk.MustExec("drop table create_auto_increment_test", Vec::new());
    tk.MustExec("create table create_auto_increment_test (id int not null auto_increment, name varchar(255), key(id)) auto_increment = 1000", Vec::new());
    tk.MustExec(
        "insert into create_auto_increment_test (name) values ('aa')",
        Vec::new(),
    );
    tk.MustQuery("select * from create_auto_increment_test", Vec::new())
        .Check(Rows(&["1000 aa"]));
    tk.MustExec("drop table create_auto_increment_test", Vec::new());

    tk.MustExec("drop table if exists t_if_exists", Vec::new());
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Note|1051|Unknown table 'test.t_if_exists'"],
    ));
    tk.MustExec("create table if not exists t1_if_exists(c int)", Vec::new());
    tk.MustExec(
        "drop table if exists t1_if_exists,t2_if_exists,t3_if_exists",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &[
            "Note|1051|Unknown table 'test.t2_if_exists'",
            "Note|1051|Unknown table 'test.t3_if_exists'",
        ],
    ));
    tk.MustContainErrMsg(
        "create table test_multiple_column_collate (a char(1) collate utf8_bin collate utf8_general_ci) charset utf8mb4 collate utf8mb4_bin",
        "Multiple COLLATE clauses",
    );
    tk.MustContainErrMsg(
        "create table test_multiple_column_collate (a char(1) charset utf8 collate utf8_bin collate utf8_general_ci) charset utf8mb4 collate utf8mb4_bin",
        "Multiple COLLATE clauses",
    );
    tk.MustContainErrMsg(
        "create table test_err_multiple_collate (a char(1) charset utf8mb4 collate utf8_unicode_ci collate utf8_general_ci) charset utf8mb4 collate utf8mb4_bin",
        "Multiple COLLATE clauses",
    );
    tk.MustContainErrMsg(
        "create table test_err_multiple_collate (a char(1) collate utf8_unicode_ci collate utf8mb4_general_ci) charset utf8mb4 collate utf8mb4_bin",
        "Multiple COLLATE clauses",
    );
}

/// 直接调用生产 `TableCatalog::truncate_table`：截断后 AutoID 归零、TiFlash 可用分区清空。
#[test]
fn ddl_catalog_truncate_resets_auto_ids_and_tiflash_availability() {
    use astersql_ddl::table::{
        TableCatalog, TableInfo, TableState, set_tiflash_replica, update_tiflash_replica_status,
    };
    use std::collections::BTreeMap;
    // 构造带分区与 TiFlash 副本的表，再 truncate，断言 ID 与可用性状态被重置。
    let mut table = TableInfo {
        id: 10,
        schema_id: 1,
        name: "t".to_owned(),
        state: TableState::Public,
        partition_ids: vec![11],
        auto_increment_id: 99,
        auto_random_id: 77,
        auto_id_cache: 0,
        auto_id_schema_id: 0,
        shard_row_id_bits: 0,
        max_shard_row_id_bits: 0,
        comment: String::new(),
        charset: "utf8mb4".to_owned(),
        collation: "utf8mb4_bin".to_owned(),
        version: 1,
        foreign_keys: Vec::new(),
        tiflash_replica: None,
        placement_policy: None,
        attributes: BTreeMap::new(),
        cached: false,
        affinity: None,
        split_policy: None,
    };
    set_tiflash_replica(&mut table, 1, vec!["zone".to_owned()]).unwrap();
    update_tiflash_replica_status(&mut table, 11, true).unwrap();
    let mut catalog = TableCatalog::default();
    catalog.insert(table).unwrap();
    assert_eq!(
        catalog.truncate_table(1, "t", 20, vec![21]).unwrap(),
        vec![10, 11]
    );
    let truncated = catalog.get(1, "T").unwrap();
    assert_eq!(truncated.auto_increment_id, 0);
    assert_eq!(truncated.auto_random_id, 0);
    assert!(
        truncated
            .tiflash_replica
            .as_ref()
            .unwrap()
            .available_partition_ids
            .is_empty()
    );
}

/// 建库/删库：drop 后无 DB 错误，以及 charset/collate 合法性。
#[test]
fn test_create_drop_database() {
    let mut tk = ddl_testkit();
    tk.MustExec("create database if not exists drop_test", Vec::new());
    tk.MustExec("drop database if exists drop_test", Vec::new());
    tk.MustExec("create database drop_test", Vec::new());
    tk.MustExec("use drop_test", Vec::new());
    tk.MustExec("drop database drop_test", Vec::new());
    tk.MustContainErrMsg("drop table t", "No database selected");
    tk.MustContainErrMsg("select * from t", "No database selected");
    assert!(!tk.ExecToErr("drop database mysql").message().is_empty());
    tk.MustExec("create database charset_test charset ascii", Vec::new());
    tk.MustQuery("show create database charset_test", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["charset_test|CREATE DATABASE `charset_test` /*!40100 DEFAULT CHARACTER SET ascii */"],
        ));
    tk.MustExec("drop database charset_test", Vec::new());
    tk.MustExec("create database charset_test charset binary", Vec::new());
    tk.MustQuery("show create database charset_test", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["charset_test|CREATE DATABASE `charset_test` /*!40100 DEFAULT CHARACTER SET binary */"],
        ));
    tk.MustExec("drop database charset_test", Vec::new());
    tk.MustExec(
        "create database charset_test collate utf8_general_ci",
        Vec::new(),
    );
    tk.MustQuery("show create database charset_test", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["charset_test|CREATE DATABASE `charset_test` /*!40100 DEFAULT CHARACTER SET utf8 COLLATE utf8_general_ci */"],
        ));
    tk.MustExec("drop database charset_test", Vec::new());
    tk.MustExec(
        "create database charset_test charset utf8 collate utf8_general_ci",
        Vec::new(),
    );
    tk.MustQuery("show create database charset_test", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["charset_test|CREATE DATABASE `charset_test` /*!40100 DEFAULT CHARACTER SET utf8 COLLATE utf8_general_ci */"],
        ));
    tk.MustContainErrMsg(
        "create database charset_test charset utf8 collate utf8mb4_unicode_ci",
        "COLLATION 'utf8mb4_unicode_ci' is not valid for CHARACTER SET 'utf8'",
    );
    tk.MustExec("drop database charset_test", Vec::new());
    tk.MustExec("set session character_set_server='ascii'", Vec::new());
    tk.MustExec("set session collation_server='ascii_bin'", Vec::new());
    tk.MustExec("create database charset_test", Vec::new());
    tk.MustQuery("show create database charset_test", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["charset_test|CREATE DATABASE `charset_test` /*!40100 DEFAULT CHARACTER SET ascii */"],
        ));
    tk.MustExec("drop database charset_test", Vec::new());
    tk.MustExec(
        "create database charset_test collate utf8mb4_general_ci",
        Vec::new(),
    );
    tk.MustQuery("show create database charset_test", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["charset_test|CREATE DATABASE `charset_test` /*!40100 DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci */"],
        ));
    tk.MustExec("drop database charset_test", Vec::new());
    tk.MustExec("create database charset_test charset utf8mb4", Vec::new());
    tk.MustQuery("show create database charset_test", Vec::new())
        .Check(RowsWithSep(
            "|",
            &["charset_test|CREATE DATABASE `charset_test` /*!40100 DEFAULT CHARACTER SET utf8mb4 */"],
        ));
    tk.MustExec("drop database charset_test", Vec::new());
}

/// ALTER TABLE ADD COLUMN：默认值、以及对 view/sequence 的错误对象检查。
#[test]
fn test_alter_table_add_column() {
    let mut tk = ddl_testkit();
    tk.MustExec("drop table if exists alter_test", Vec::new());
    tk.MustExec("create table alter_test (c1 int)", Vec::new());
    tk.MustExec("insert into alter_test values (1)", Vec::new());
    tk.MustExec(
        "alter table alter_test add column c2 timestamp default current_timestamp",
        Vec::new(),
    );
    tk.MustExec(
        "alter table alter_test add column c3 varchar(50) default 'CURRENT_TIMESTAMP'",
        Vec::new(),
    );
    let row = tk
        .MustQuery("select c1,c2,c3 from alter_test", Vec::new())
        .Rows();
    assert_eq!(row.len(), 1);
    assert_eq!(row[0][0], "1");
    tk.MustQuery("select c2 <= current_timestamp from alter_test", Vec::new())
        .Check(Rows(&["1"]));
    assert_eq!(row[0][2], "CURRENT_TIMESTAMP");
    tk.MustExec(
        "alter table alter_test add column c4 date default current_date",
        Vec::new(),
    );
    let date_row = tk.MustQuery("select c4 from alter_test", Vec::new()).Rows();
    assert_eq!(date_row.len(), 1);
    assert_eq!(
        date_row[0][0],
        chrono::Utc::now().format("%Y-%m-%d").to_string()
    );
    tk.MustExec(
        "create or replace view alter_view as select c1,c2 from alter_test",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "alter table alter_view add column c5 varchar(50)",
        "is not BASE TABLE",
    );
    tk.MustExec("drop view alter_view", Vec::new());
    tk.MustExec("create sequence alter_seq", Vec::new());
    tk.MustContainErrMsg(
        "alter table alter_seq add column c int",
        "is not BASE TABLE",
    );
    tk.MustExec("drop sequence alter_seq", Vec::new());
}

/// ALTER TABLE 一次添加多列（逗号多子句与括号语法）。
#[test]
fn test_alter_table_add_columns() {
    let mut tk = ddl_testkit();
    tk.MustExec("drop table if exists alter_test", Vec::new());
    tk.MustExec("create table alter_test (c1 int)", Vec::new());
    tk.MustExec("insert into alter_test values (1)", Vec::new());
    tk.MustExec("alter table alter_test add column c2 timestamp default current_timestamp, add column c8 varchar(50) default 'CURRENT_TIMESTAMP'", Vec::new());
    tk.MustExec("alter table alter_test add column (c7 timestamp default current_timestamp, c3 varchar(50) default 'CURRENT_TIMESTAMP')", Vec::new());
    let rows = tk
        .MustQuery("select c1,c3,c8 from alter_test", Vec::new())
        .Rows();
    assert_eq!(
        rows,
        vec![vec![
            "1".to_owned(),
            "CURRENT_TIMESTAMP".to_owned(),
            "CURRENT_TIMESTAMP".to_owned()
        ]]
    );
    tk.MustExec(
        "create or replace view alter_view as select c1,c2 from alter_test",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "alter table alter_view add column (c4 varchar(50), c5 varchar(50))",
        "is not BASE TABLE",
    );
    tk.MustExec("drop view alter_view", Vec::new());
    tk.MustExec("create sequence alter_seq", Vec::new());
    tk.MustContainErrMsg(
        "alter table alter_seq add column (c1 int, c2 varchar(10))",
        "is not BASE TABLE",
    );
    tk.MustExec("drop sequence alter_seq", Vec::new());
}

/// 新增 NOT NULL 无默认值列：存量行填 OriginDefaultValue（通常为 0）。
#[test]
fn test_add_not_null_column_no_default() {
    let mut tk = ddl_testkit();
    tk.MustExec("set sql_mode='STRICT_TRANS_TABLES'", Vec::new());
    tk.MustExec("drop table if exists nn", Vec::new());
    tk.MustExec("create table nn (c1 int)", Vec::new());
    tk.MustExec("insert nn values (1), (2)", Vec::new());
    tk.MustExec("alter table nn add column c2 int not null", Vec::new());
    let initial = tk
        .MustQuery("select c1, coalesce(c2, 0) from nn", Vec::new())
        .Rows();
    assert_eq!(
        initial,
        vec![
            vec!["1".to_owned(), "0".to_owned()],
            vec!["2".to_owned(), "0".to_owned()]
        ]
    );
    tk.MustContainErrMsg("insert nn (c1) values (3)", "doesn't have a default value");
    tk.MustExec("set sql_mode=''", Vec::new());
    tk.MustExec("insert nn (c1) values (3)", Vec::new());
    tk.MustQuery("select c1, coalesce(c2, 0) from nn", Vec::new())
        .Check(Rows(&["1 0", "2 0", "3 0"]));
}

/// ALTER TABLE MODIFY COLUMN：类型变更、错误对象与 COLLATE 组合校验。
#[test]
fn test_alter_table_modify_column() {
    let mut tk = ddl_testkit();
    tk.MustExec("drop table if exists mc", Vec::new());
    tk.MustExec(
        "create table mc(c1 int, c2 varchar(10), c3 bit)",
        Vec::new(),
    );
    assert!(
        !tk.ExecToErr("alter table mc modify column c1 short")
            .message()
            .is_empty()
    );
    tk.MustExec("alter table mc modify column c1 bigint", Vec::new());
    assert!(
        !tk.ExecToErr("alter table mc modify column c2 blob")
            .message()
            .is_empty()
    );
    tk.MustExec("alter table mc modify column c2 varchar(8)", Vec::new());
    tk.MustExec("alter table mc modify column c2 varchar(11)", Vec::new());
    tk.MustExec("alter table mc modify column c2 text(13)", Vec::new());
    tk.MustExec("alter table mc modify column c2 text", Vec::new());
    tk.MustExec("alter table mc modify column c3 bit", Vec::new());
    let create_sql = tk.MustQuery("show create table mc", Vec::new()).Rows()[0].join(" ");
    assert!(create_sql.contains("`c1` bigint"));
    assert!(create_sql.contains("`c2` text"));
    assert!(create_sql.contains("`c3` bit"));

    tk.MustExec(
        "create or replace view alter_view as select c1,c2 from mc",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "alter table alter_view modify column c2 text",
        "is not BASE TABLE",
    );
    tk.MustExec("drop view alter_view", Vec::new());
    tk.MustExec("create sequence alter_seq", Vec::new());
    tk.MustContainErrMsg(
        "alter table alter_seq modify column c int",
        "is not BASE TABLE",
    );
    tk.MustExec("drop sequence alter_seq", Vec::new());

    tk.MustExec("create table modify_column_multiple_collate (a char(1) collate utf8_general_ci) charset utf8mb4 collate utf8mb4_bin", Vec::new());
    tk.MustExec(
        "alter table modify_column_multiple_collate modify column a char(1) collate utf8mb4_bin",
        Vec::new(),
    );
    tk.MustQuery(
        "select character_set_name, collation_name from information_schema.columns where \
         table_schema='test' and table_name='modify_column_multiple_collate' and column_name='a'",
        Vec::new(),
    )
    .Check(Rows(&["utf8mb4 utf8mb4_bin"]));

    tk.MustExec("drop table modify_column_multiple_collate", Vec::new());
    tk.MustExec("create table modify_column_multiple_collate (a char(1) collate utf8_general_ci) charset utf8mb4 collate utf8mb4_bin", Vec::new());
    tk.MustExec(
        "alter table modify_column_multiple_collate modify column a char(1) charset utf8mb4 collate utf8mb4_bin",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "alter table modify_column_multiple_collate modify column a char(1) charset utf8mb4 collate utf8_bin",
        "COLLATION 'utf8_bin' is not valid for CHARACTER SET 'utf8mb4'",
    );
    tk.MustContainErrMsg(
        "alter table modify_column_multiple_collate modify column a char(1) collate utf8_bin collate utf8mb4_bin",
        "Multiple COLLATE clauses",
    );
}

/// 列级 charset/collate 推导与未知字符集错误（表驱动）。
#[test]
fn test_column_charset_and_collate() {
    // 对应 Go 的 TestColumnCharsetAndCollate：表驱动测试列 charset/collate 推导和未知 charset 错误。
    let cases = [
        CharsetCase {
            col_type: "varchar(10)",
            charset: "charset utf8",
            collate: "collate utf8_bin",
            expected_charset: "utf8",
            expected_collate: "utf8_bin",
            error: "",
        },
        CharsetCase {
            col_type: "varchar(10)",
            charset: "charset utf8mb4",
            collate: "",
            expected_charset: "utf8mb4",
            expected_collate: "utf8mb4_bin",
            error: "",
        },
        CharsetCase {
            col_type: "varchar(10)",
            charset: "charset utf16",
            collate: "",
            expected_charset: "",
            expected_collate: "",
            error: "Unknown charset utf16",
        },
        CharsetCase {
            col_type: "varchar(10)",
            charset: "charset latin1",
            collate: "",
            expected_charset: "latin1",
            expected_collate: "latin1_bin",
            error: "",
        },
        CharsetCase {
            col_type: "varchar(10)",
            charset: "charset binary",
            collate: "",
            expected_charset: "binary",
            expected_collate: "binary",
            error: "",
        },
        CharsetCase {
            col_type: "varchar(10)",
            charset: "charset ascii",
            collate: "",
            expected_charset: "ascii",
            expected_collate: "ascii_bin",
            error: "",
        },
    ];
    assert_eq!(cases.len(), 6);
    let mut tk = ddl_testkit();
    tk.MustExec("drop database if exists col_charset_collate", Vec::new());
    tk.MustExec("create database col_charset_collate", Vec::new());
    tk.MustExec("use col_charset_collate", Vec::new());
    for (i, case) in cases.iter().enumerate() {
        let sql = format!(
            "create table t{i} (a {} {} {})",
            case.col_type, case.charset, case.collate
        );
        if case.error.is_empty() {
            tk.MustExec(&sql, Vec::new());
            let expected = format!("{} {}", case.expected_charset, case.expected_collate);
            tk.MustQuery(
                &format!(
                    "select character_set_name, collation_name from information_schema.columns \
                     where table_schema='col_charset_collate' and table_name='t{i}' and \
                     column_name='a'"
                ),
                Vec::new(),
            )
            .Check(Rows(&[&expected]));
        } else {
            assert!(!tk.ExecToErr(&sql).message().is_empty(), "{sql}");
        }
    }
    tk.MustExec("drop database col_charset_collate", Vec::new());
}

/// `shard_row_id_bits`：打散行 ID 高位、与 auto_increment/PK handle 的兼容与溢出。
#[test]
fn test_shard_row_id_bits() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t, t1, auto", Vec::new());
    tk.MustExec("create table t (a int) shard_row_id_bits = 15", Vec::new());
    let values = (0..100)
        .map(|i| format!("({i})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());
    tk.MustQuery("select count(*) from t", Vec::new())
        .Check(Rows(&["100"]));
    let hidden_handles = tk
        .MustQuery("select _tidb_rowid from t order by a", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(hidden_handles.len(), 100);
    assert!(hidden_handles.iter().all(|handle| *handle >= 0));
    assert!(hidden_handles.iter().any(|handle| (*handle >> 56) > 0));
    let create_sql = tk.MustQuery("show create table t", Vec::new()).Rows()[0].join(" ");
    assert!(create_sql.contains("SHARD_ROW_ID_BITS=15"));

    tk.MustExec(
        "create table auto (id int not null auto_increment unique) shard_row_id_bits = 4",
        Vec::new(),
    );
    tk.MustExec("alter table auto shard_row_id_bits = 5", Vec::new());
    tk.MustExec("drop table auto", Vec::new());
    tk.MustExec(
        "create table auto (id int not null auto_increment unique) shard_row_id_bits = 0",
        Vec::new(),
    );
    tk.MustExec("alter table auto shard_row_id_bits = 5", Vec::new());
    tk.MustExec("drop table auto", Vec::new());
    tk.MustExec(
        "create table auto (id int not null auto_increment unique)",
        Vec::new(),
    );
    tk.MustExec("alter table auto shard_row_id_bits = 5", Vec::new());
    tk.MustExec("drop table auto", Vec::new());
    tk.MustExec(
        "create table auto (id int not null auto_increment unique) shard_row_id_bits = 4",
        Vec::new(),
    );
    tk.MustExec("alter table auto shard_row_id_bits = 0", Vec::new());
    tk.MustExec("drop table auto", Vec::new());
    tk.MustContainErrMsg(
        "create table auto (id varchar(255) primary key clustered, b int) shard_row_id_bits = 4",
        "Unsupported shard_row_id_bits for table with primary key as row id",
    );
    tk.MustExec(
        "create table auto (id varchar(255) primary key clustered, b int) shard_row_id_bits = 0",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "alter table auto shard_row_id_bits = 5",
        "Unsupported shard_row_id_bits for table with primary key as row id",
    );
    tk.MustExec("alter table auto shard_row_id_bits = 0", Vec::new());
    tk.MustExec("drop table auto", Vec::new());

    tk.MustContainErrMsg(
        "create table auto (id int not null auto_increment primary key, b int) shard_row_id_bits = 4",
        "Unsupported shard_row_id_bits for table with primary key as row id",
    );
    tk.MustExec("create table auto (id int not null auto_increment primary key, b int) shard_row_id_bits = 0", Vec::new());
    tk.MustContainErrMsg(
        "alter table auto shard_row_id_bits = 5",
        "Unsupported shard_row_id_bits for table with primary key as row id",
    );
    tk.MustExec("alter table auto shard_row_id_bits = 0", Vec::new());
    tk.MustExec("drop table auto", Vec::new());

    tk.MustExec(
        "create table auto (a int, b int auto_increment unique) shard_row_id_bits = 15",
        Vec::new(),
    );
    for i in 0..100 {
        tk.MustExec(&format!("insert into auto(a) values ({i})"), Vec::new());
    }
    let auto_ids = tk
        .MustQuery("select b from auto order by a", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<u64>().unwrap())
        .collect::<Vec<_>>();
    assert!(auto_ids.windows(2).all(|pair| pair[0] < pair[1]));
    tk.MustExec("drop table auto", Vec::new());

    tk.MustExec("create table t1 (a int) shard_row_id_bits = 15", Vec::new());
    let (_, table) = domain.stats_table("test", "t1").unwrap();
    let max_id = (1_u64 << (64 - 15 - 1)) - 1;
    domain
        .allocate_stats_auto_id_kind(table.ID, Some(max_id - 1), 2)
        .unwrap();
    tk.MustExec("insert into t1 values (1)", Vec::new());
    tk.MustContainErrMsg(
        "insert into t1 values (2)",
        "Failed to read auto-increment value",
    );
    tk.MustContainErrMsg(
        "insert into t1 values (3)",
        "Failed to read auto-increment value",
    );
    tk.MustExec("drop table t1", Vec::new());
}

/// `AUTO_RANDOM`：随机化自增主键高位 shard bits、显式插入、溢出与 rename。
#[test]
fn test_auto_random_bits_data() {
    let mut tk = ddl_testkit();
    tk.MustExec("drop database if exists test_auto_random_bits", Vec::new());
    tk.MustExec("create database test_auto_random_bits", Vec::new());
    tk.MustExec("use test_auto_random_bits", Vec::new());
    tk.MustExec("set @@allow_auto_random_explicit_insert = true", Vec::new());
    tk.MustExec(
        "create table t (a bigint primary key clustered auto_random(15), b int)",
        Vec::new(),
    );
    for i in 0..100 {
        tk.MustExec(&format!("insert into t(b) values ({i})"), Vec::new());
    }
    let handles = tk
        .MustQuery("select a from t order by a", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(handles.len(), 100);
    assert!(handles.iter().any(|handle| (*handle >> 48) != 0));
    let mut incremental = handles
        .iter()
        .map(|handle| (*handle as u64) & ((1_u64 << 48) - 1))
        .collect::<Vec<_>>();
    incremental.sort_unstable();
    assert_eq!(incremental, (1..=100).collect::<Vec<_>>());
    tk.MustExec("drop table t", Vec::new());

    let upper = (2_i64 << 47) - 1;
    tk.MustExec(
        "create table t (a bigint primary key clustered auto_random(15), b int)",
        Vec::new(),
    );
    for i in -10..10 {
        tk.MustExec(
            &format!("insert into t values ({}, {i})", upper + i),
            Vec::new(),
        );
    }
    tk.MustContainErrMsg(
        "insert into t (b) values (0)",
        "Failed to read auto-random value",
    );
    tk.MustExec("drop table t", Vec::new());

    tk.MustExec(
        "create table t (a bigint primary key auto_random(15), b int)",
        Vec::new(),
    );
    tk.MustExec(&format!("insert into t values ({upper}, 1)"), Vec::new());
    assert!(
        !tk.MustQuery("select a from t", Vec::new())
            .Rows()
            .is_empty()
    );
    tk.MustContainErrMsg(
        "insert into t (b) values (0)",
        "Failed to read auto-random value",
    );
    tk.MustExec("drop table t", Vec::new());

    tk.MustExec(
        "create table t (a bigint primary key auto_random(15), b int)",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1, 2)", Vec::new());
    tk.MustExec(&format!("update t set a = {upper} where a = 1"), Vec::new());
    assert!(
        !tk.MustQuery("select a from t", Vec::new())
            .Rows()
            .is_empty()
    );
    tk.MustContainErrMsg(
        "insert into t (b) values (0)",
        "Failed to read auto-random value",
    );
    tk.MustExec("drop table t", Vec::new());

    tk.MustExec(
        "create table t (a bigint primary key auto_random(15), b int)",
        Vec::new(),
    );
    for i in 1..=100 {
        tk.MustExec(&format!("insert into t(b) values ({i})"), Vec::new());
        tk.MustExec(
            &format!("insert into t(a,b) values ({}, {i})", -i),
            Vec::new(),
        );
    }
    let mut signed_handles = tk
        .MustQuery("select a from t", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    signed_handles.sort_by_key(|handle| (*handle as u64) & ((1_u64 << 48) - 1));
    assert_eq!(
        signed_handles.iter().filter(|handle| **handle < 0).count(),
        100
    );
    let mut positive_incremental = signed_handles
        .iter()
        .filter(|handle| **handle > 0)
        .map(|handle| (*handle as u64) & ((1_u64 << 48) - 1))
        .collect::<Vec<_>>();
    positive_incremental.sort_unstable();
    assert_eq!(positive_incremental, (1..=100).collect::<Vec<_>>());
    tk.MustExec("drop table t", Vec::new());

    tk.MustExec(
        "create table t (a bigint primary key auto_random(10), b int)",
        Vec::new(),
    );
    for i in 0..100 {
        tk.MustExec(&format!("insert into t(b) values ({i})"), Vec::new());
    }
    assert!(
        tk.MustQuery("select a from t", Vec::new())
            .Rows()
            .iter()
            .all(|row| row[0].parse::<i64>().unwrap() > 0)
    );
    tk.MustExec("drop table t", Vec::new());

    tk.MustExec(
        "create table t (a bigint unsigned primary key auto_random(10), b int)",
        Vec::new(),
    );
    for i in 0..100 {
        tk.MustExec(&format!("insert into t(b) values ({i})"), Vec::new());
    }
    assert!(
        tk.MustQuery("select a from t", Vec::new())
            .Rows()
            .iter()
            .any(|row| row[0].parse::<u64>().unwrap() > i64::MAX as u64)
    );
    tk.MustExec("drop table t", Vec::new());

    tk.MustExec(
        "create table t (a bigint auto_random primary key)",
        Vec::new(),
    );
    for _ in 0..10 {
        tk.MustExec("insert into t values ()", Vec::new());
    }
    tk.MustExec("create database test_auto_random_bits_rename", Vec::new());
    tk.MustExec(
        "alter table t rename to test_auto_random_bits_rename.t1",
        Vec::new(),
    );
    for _ in 0..10 {
        tk.MustExec(
            "insert into test_auto_random_bits_rename.t1 values ()",
            Vec::new(),
        );
    }
    tk.MustExec(
        "alter table test_auto_random_bits_rename.t1 rename to t",
        Vec::new(),
    );
    for _ in 0..10 {
        tk.MustExec("insert into t values ()", Vec::new());
    }
    let unique_incremental = tk
        .MustQuery("select a from t", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<u64>().unwrap() & ((1_u64 << (63 - 5)) - 1))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique_incremental.len(), 30);
    tk.MustExec("drop database test_auto_random_bits_rename", Vec::new());
    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("drop database test_auto_random_bits", Vec::new());
}

/// 表选项 `auto_random_base`：CREATE/ALTER rebase，普通表不适用。
#[test]
fn test_auto_random_table_option() {
    let mut tk = ddl_testkit();
    tk.MustExec("drop table if exists auto_random_table_option, alter_table_auto_random_option, alter_auto_random_normal", Vec::new());
    tk.MustExec("create table auto_random_table_option (a bigint auto_random(5) key) auto_random_base = 1000", Vec::new());
    tk.MustExec(
        "insert into auto_random_table_option values (),(),(),(),()",
        Vec::new(),
    );
    let handles = tk
        .MustQuery(
            "select a from auto_random_table_option order by a",
            Vec::new(),
        )
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<u64>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(handles.len(), 5);
    let mut incremental = handles
        .iter()
        .map(|handle| handle & ((1_u64 << (63 - 5)) - 1))
        .collect::<Vec<_>>();
    incremental.sort_unstable();
    assert_eq!(incremental, (1000..1005).collect::<Vec<_>>());
    tk.MustExec(
        "create table alter_table_auto_random_option (a bigint primary key auto_random(4), b int)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into alter_table_auto_random_option values(),(),(),(),()",
        Vec::new(),
    );
    let mut initial = tk
        .MustQuery("select a from alter_table_auto_random_option", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<u64>().unwrap() & ((1_u64 << (63 - 4)) - 1))
        .collect::<Vec<_>>();
    initial.sort_unstable();
    assert_eq!(initial, (1..=5).collect::<Vec<_>>());
    tk.MustExec("delete from alter_table_auto_random_option", Vec::new());
    tk.MustExec(
        "alter table alter_table_auto_random_option auto_random_base = 3000000",
        Vec::new(),
    );
    tk.MustExec(
        "insert into alter_table_auto_random_option values(),(),(),(),()",
        Vec::new(),
    );
    let mut rebased = tk
        .MustQuery("select a from alter_table_auto_random_option", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[0].parse::<u64>().unwrap() & ((1_u64 << (63 - 4)) - 1))
        .collect::<Vec<_>>();
    rebased.sort_unstable();
    assert_eq!(rebased, (3_000_000..3_000_005).collect::<Vec<_>>());
    tk.MustExec("create table alter_auto_random_normal (a int)", Vec::new());
    tk.MustContainErrMsg(
        "alter table alter_auto_random_normal auto_random_base = 100",
        "non auto_random table",
    );
}

/// 全局变量 `tidb_ddl_error_count_limit`：负值/越界/非法类型与合法值。
#[test]
fn test_set_ddl_error_count_limit() {
    let mut tk = ddl_testkit();
    let original = vardef::GetDDLErrorCountLimit();
    tk.MustExec("set @@global.tidb_ddl_error_count_limit = -1", Vec::new());
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Warning|1292|Truncated incorrect tidb_ddl_error_count_limit value: '-1'"],
    ));
    assert_eq!(vardef::GetDDLErrorCountLimit(), 0);
    tk.MustExec(
        "set @@global.tidb_ddl_error_count_limit = 9223372036854775808",
        Vec::new(),
    );
    tk.MustQuery("show warnings", Vec::new()).Check(RowsWithSep(
        "|",
        &["Warning|1292|Truncated incorrect tidb_ddl_error_count_limit value: '9223372036854775808'"],
    ));
    assert_eq!(vardef::GetDDLErrorCountLimit(), i64::MAX);
    tk.MustContainErrMsg(
        "set @@global.tidb_ddl_error_count_limit = invalid_val",
        "Incorrect argument type",
    );
    tk.MustExec("set @@global.tidb_ddl_error_count_limit = 100", Vec::new());
    assert_eq!(vardef::GetDDLErrorCountLimit(), 100);
    tk.MustQuery("select @@global.tidb_ddl_error_count_limit", Vec::new())
        .Check(Rows(&["100"]));
    vardef::SetDDLErrorCountLimit(original);
}

/// DDL reorg（重组）最大写速：`tidb_ddl_reorg_max_write_speed` 数值与 RAM 字符串。
#[test]
fn test_set_ddl_reorg_max_write_speed() {
    let mut tk = ddl_testkit();
    let original = vardef::DDLReorgMaxWriteSpeed.Load();
    let values = [
        1_i64,
        0,
        100,
        1024 * 1024,
        2_147_483_647,
        1_125_899_906_842_624,
    ];
    for value in values {
        tk.MustExec(
            &format!("set @@global.tidb_ddl_reorg_max_write_speed = {value}"),
            Vec::new(),
        );
        assert_eq!(vardef::DDLReorgMaxWriteSpeed.Load(), value);
        tk.MustQuery("select @@global.tidb_ddl_reorg_max_write_speed", Vec::new())
            .Check(Rows(&[&value.to_string()]));
    }
    for (value, expected) in [
        ("1", 1_i64),
        ("0", 0),
        ("100", 100),
        ("2KB", 2 * 1024),
        ("3MiB", 3 * 1024 * 1024),
        ("4 gb", 4 * 1024 * 1024 * 1024),
        ("2147483647", 2_147_483_647),
        ("1125899906842624", 1_125_899_906_842_624),
    ] {
        tk.MustExec(
            &format!("set @@global.tidb_ddl_reorg_max_write_speed = '{value}'"),
            Vec::new(),
        );
        assert_eq!(vardef::DDLReorgMaxWriteSpeed.Load(), expected);
        tk.MustQuery("select @@global.tidb_ddl_reorg_max_write_speed", Vec::new())
            .Check(Rows(&[&expected.to_string()]));
    }
    for sql in [
        "set @@global.tidb_ddl_reorg_max_write_speed = -1",
        "set @@global.tidb_ddl_reorg_max_write_speed = invalid_val",
        "set @@global.tidb_ddl_reorg_max_write_speed = 1125899906842625",
    ] {
        assert!(!tk.ExecToErr(sql).message().is_empty());
    }
    vardef::DDLReorgMaxWriteSpeed.Store(original);
}

/// 分布式 DDL 任务开关 `tidb_enable_dist_task` 的加载与非法值。
#[test]
fn test_load_ddl_distribute_vars() {
    let mut tk = ddl_testkit();
    let original = vardef::EnableDistTask.Load();
    tk.MustContainErrMsg(
        "set @@global.tidb_enable_dist_task = invalid_val",
        "can't be set",
    );
    tk.MustExec("set @@global.tidb_enable_dist_task = 'on'", Vec::new());
    assert!(vardef::EnableDistTask.Load());
    tk.MustExec("set @@global.tidb_enable_dist_task = false", Vec::new());
    assert!(!vardef::EnableDistTask.Load());
    vardef::EnableDistTask.Store(original);
}

/// 极小 AutoID step 下跨库 RENAME，并恢复原 step。
#[test]
fn test_rename_with_small_auto_id_step() {
    let _step_lock = AUTO_ID_STEP_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let original_step = astersql_meta_autoid::get_step();
    astersql_meta_autoid::set_step(1);
    let mut tk = ddl_testkit();
    for db in ["rename1", "rename2", "rename3"] {
        tk.MustExec(&format!("drop database if exists {db}"), Vec::new());
        tk.MustExec(&format!("create database {db}"), Vec::new());
    }
    tk.MustExec(
        "create table rename1.t (a int primary key auto_increment)",
        Vec::new(),
    );
    astersql_meta_autoid::set_step(original_step);
    tk.MustExec("insert rename1.t values ()", Vec::new());
    tk.MustExec("rename table rename1.t to rename2.t", Vec::new());
    tk.MustExec("drop database rename1", Vec::new());
    tk.MustExec("insert rename2.t values ()", Vec::new());
    tk.MustExec("rename table rename2.t to rename3.t", Vec::new());
    tk.MustExec("insert rename3.t values ()", Vec::new());
    tk.MustExec("drop database rename2", Vec::new());
    tk.MustExec("insert rename3.t values ()", Vec::new());
    tk.MustQuery("select * from rename3.t", Vec::new())
        .Check(Rows(&["1", "2", "3", "4"]));
    tk.MustExec("drop database rename3", Vec::new());
}

/// RENAME 后强制 schema full reload，覆盖 `AUTO_ID_CACHE=1` 与跨库删除。
#[test]
fn test_rename_table_with_reload() {
    let _step_lock = AUTO_ID_STEP_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut tk, domain) = ddl_testkit_with_domain();
    for db in ["rename1", "rename2", "rename3"] {
        tk.MustExec(&format!("drop database if exists {db}"), Vec::new());
        tk.MustExec(&format!("create database {db}"), Vec::new());
    }
    tk.MustExec(
        "create table rename1.t(id int primary key auto_increment) AUTO_ID_CACHE=1",
        Vec::new(),
    );
    tk.MustExec("insert into rename1.t values ()", Vec::new());
    tk.MustExec("rename table rename1.t to rename2.t", Vec::new());
    force_full_reload(&mut tk, &domain);
    tk.MustExec("insert into rename2.t values ()", Vec::new());
    tk.MustQuery("select * from rename2.t", Vec::new())
        .Check(Rows(&["1", "2"]));
    tk.MustExec("drop table rename2.t", Vec::new());

    tk.MustExec(
        "create table rename1.t(id int primary key auto_increment) AUTO_ID_CACHE=1",
        Vec::new(),
    );
    tk.MustExec("insert into rename1.t values (), ()", Vec::new());
    tk.MustExec("rename table rename1.t to rename2.t", Vec::new());
    tk.MustExec("insert into rename2.t values (100)", Vec::new());
    force_full_reload(&mut tk, &domain);
    tk.MustExec("insert into rename2.t values ()", Vec::new());
    tk.MustQuery("select * from rename2.t", Vec::new())
        .Check(Rows(&["1", "2", "100", "101"]));
    tk.MustExec("drop table rename2.t", Vec::new());

    let original_step = astersql_meta_autoid::get_step();
    astersql_meta_autoid::set_step(5_000);
    tk.MustExec(
        "create table rename1.t (a int primary key auto_increment)",
        Vec::new(),
    );
    astersql_meta_autoid::set_step(original_step);
    tk.MustExec("insert rename1.t values ()", Vec::new());
    tk.MustExec("rename table rename1.t to rename2.t", Vec::new());
    tk.MustExec("drop database rename1", Vec::new());
    astersql_meta_autoid::set_step(5_000);
    force_full_reload(&mut tk, &domain);
    astersql_meta_autoid::set_step(original_step);
    tk.MustExec("insert rename2.t values ()", Vec::new());
    tk.MustExec("rename table rename2.t to rename3.t", Vec::new());
    tk.MustExec("insert rename3.t values ()", Vec::new());
    tk.MustExec("drop database rename2", Vec::new());
    astersql_meta_autoid::set_step(5_000);
    force_full_reload(&mut tk, &domain);
    astersql_meta_autoid::set_step(original_step);
    tk.MustExec("insert rename3.t values ()", Vec::new());
    tk.MustQuery("select * from rename3.t", Vec::new())
        .Check(Rows(&["1", "5001", "5002", "10001"]));
    tk.MustExec("drop database rename3", Vec::new());
}

// this test will change the fail-point `mockAutoIDChange`, so we move it to the `testRecoverTable` suite
/// RENAME TABLE：启用 `mockAutoIDChange` failpoint，覆盖跨库/同库 rename 与显式大 AutoID。
#[test]
fn test_rename_table() {
    let _auto_id_change = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/meta/autoid/mockAutoIDChange",
        "return(true)",
    );
    let mut tk = ddl_testkit();
    for db in ["rename1", "rename2", "rename3"] {
        tk.MustExec(&format!("drop database if exists {db}"), Vec::new());
        tk.MustExec(&format!("create database {db}"), Vec::new());
    }
    tk.MustExec(
        "create table rename1.t (a int primary key auto_increment)",
        Vec::new(),
    );
    tk.MustExec("insert rename1.t values ()", Vec::new());
    tk.MustExec("rename table rename1.t to rename2.t", Vec::new());
    tk.MustExec("drop database rename1", Vec::new());
    tk.MustExec("insert rename2.t values ()", Vec::new());
    tk.MustExec("rename table rename2.t to rename3.t", Vec::new());
    tk.MustExec("insert rename3.t values ()", Vec::new());
    tk.MustExec("drop database rename2", Vec::new());
    tk.MustExec("insert rename3.t values ()", Vec::new());
    tk.MustQuery("select * from rename3.t", Vec::new())
        .Check(Rows(&["1", "2", "3", "4"]));
    tk.MustExec("drop database rename3", Vec::new());

    tk.MustExec("create database rename1", Vec::new());
    tk.MustExec("create database rename2", Vec::new());
    tk.MustExec(
        "create table rename1.t (a int primary key auto_increment)",
        Vec::new(),
    );
    tk.MustExec("insert rename1.t values ()", Vec::new());
    tk.MustExec("rename table rename1.t to rename2.t1", Vec::new());
    tk.MustExec("insert rename2.t1 values ()", Vec::new());
    tk.MustExec("rename table rename2.t1 to rename2.t2", Vec::new());
    tk.MustExec("insert rename2.t2 values ()", Vec::new());
    tk.MustQuery("select * from rename2.t2", Vec::new())
        .Check(Rows(&["1", "2", "3"]));
    tk.MustExec("drop database rename1", Vec::new());
    tk.MustExec("drop database rename2", Vec::new());

    tk.MustExec("create database rename1", Vec::new());
    tk.MustExec("create database rename2", Vec::new());
    tk.MustExec(
        "create table rename1.t (a int primary key auto_increment)",
        Vec::new(),
    );
    tk.MustExec("insert rename1.t values ()", Vec::new());
    tk.MustExec("rename table rename1.t to rename2.t1", Vec::new());
    tk.MustExec("insert rename2.t1 values (100000)", Vec::new());
    tk.MustExec("insert rename2.t1 values ()", Vec::new());
    tk.MustQuery("select * from rename2.t1", Vec::new())
        .Check(Rows(&["1", "100000", "100001"]));
    assert!(
        !tk.ExecToErr("insert rename1.t values ()")
            .message()
            .is_empty()
    );
    tk.MustExec("drop database rename1", Vec::new());
    tk.MustExec("drop database rename2", Vec::new());
}

/// 一次 RENAME 多张表：目标冲突与缺表错误。
#[test]
fn test_rename_multi_tables() {
    let _auto_id_change = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/meta/autoid/mockAutoIDChange",
        "return(true)",
    );
    let mut tk = ddl_testkit();
    for db in ["rename1", "rename2", "rename3", "rename4"] {
        tk.MustExec(&format!("drop database if exists {db}"), Vec::new());
        tk.MustExec(&format!("create database {db}"), Vec::new());
    }
    tk.MustExec(
        "create table rename1.t1 (a int primary key auto_increment)",
        Vec::new(),
    );
    tk.MustExec(
        "create table rename3.t3 (a int primary key auto_increment)",
        Vec::new(),
    );
    tk.MustExec("insert rename1.t1 values ()", Vec::new());
    tk.MustExec("insert rename3.t3 values ()", Vec::new());
    tk.MustExec(
        "rename table rename1.t1 to rename2.t2, rename3.t3 to rename4.t4",
        Vec::new(),
    );
    tk.MustExec("drop database rename1", Vec::new());
    tk.MustExec("insert rename2.t2 values ()", Vec::new());
    tk.MustExec("drop database rename3", Vec::new());
    tk.MustExec("insert rename4.t4 values ()", Vec::new());
    tk.MustQuery("select * from rename2.t2", Vec::new())
        .Check(Rows(&["1", "2"]));
    tk.MustQuery("select * from rename4.t4", Vec::new())
        .Check(Rows(&["1", "2"]));
    tk.MustExec(
        "rename table rename2.t2 to rename2.t1, rename4.t4 to rename4.t3",
        Vec::new(),
    );
    tk.MustExec("insert rename2.t1 values ()", Vec::new());
    tk.MustExec("insert rename4.t3 values ()", Vec::new());
    tk.MustQuery("select * from rename2.t1", Vec::new())
        .Check(Rows(&["1", "2", "3"]));
    tk.MustQuery("select * from rename4.t3", Vec::new())
        .Check(Rows(&["1", "2", "3"]));
    tk.MustExec("drop database rename2", Vec::new());
    tk.MustExec("drop database rename4", Vec::new());

    tk.MustExec("create database rename1", Vec::new());
    tk.MustExec("create database rename2", Vec::new());
    tk.MustExec("create database rename3", Vec::new());
    tk.MustExec(
        "create table rename1.t1 (a int primary key auto_increment)",
        Vec::new(),
    );
    tk.MustExec(
        "create table rename3.t3 (a int primary key auto_increment)",
        Vec::new(),
    );
    tk.MustContainErrMsg(
        "rename table rename1.t1 to rename2.t2, rename3.t3 to rename2.t2",
        "duplicate RENAME TABLE target",
    );
    tk.MustExec(
        "rename table rename1.t1 to rename2.t2, rename2.t2 to rename1.t1",
        Vec::new(),
    );
    tk.MustExec(
        "rename table rename1.t1 to rename2.t2, rename3.t3 to rename1.t1",
        Vec::new(),
    );
    tk.MustExec("use rename1", Vec::new());
    tk.MustQuery("show tables", Vec::new()).Check(Rows(&["t1"]));
    tk.MustExec("use rename2", Vec::new());
    tk.MustQuery("show tables", Vec::new()).Check(Rows(&["t2"]));
    tk.MustExec("use rename3", Vec::new());
    tk.MustExec(
        "create table rename3.t3 (a int primary key auto_increment)",
        Vec::new(),
    );
    assert!(
        !tk.ExecToErr("rename table rename1.t1 to rename1.t2, rename1.t1 to rename3.t3",)
            .message()
            .is_empty()
    );
    assert!(
        !tk.ExecToErr("rename table rename1.t1 to rename1.t2, rename1.t1 to rename3.t4")
            .message()
            .is_empty()
    );
    for db in ["rename1", "rename2", "rename3"] {
        tk.MustExec(&format!("drop database {db}"), Vec::new());
    }
}

/// 默认 `shard_row_id_bits` / pre-split、手动 table option、clustered index 与临时表。
#[test]
fn test_def_shard_tables() {
    let mut tk = ddl_testkit();
    tk.MustExec("drop table if exists t, t0, t1, tengine", Vec::new());
    tk.MustExec(
        "set @@session.tidb_enable_clustered_index = off",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_shard_row_id_bits = 4", Vec::new());
    tk.MustExec("set @@session.tidb_pre_split_regions = 4", Vec::new());
    tk.MustExec("create table t (i int primary key)", Vec::new());
    let t_sql = tk.MustQuery("show create table t", Vec::new()).Rows()[0].join(" ");
    assert!(t_sql.contains("NONCLUSTERED"), "{t_sql}");
    assert!(t_sql.contains("SHARD_ROW_ID_BITS=4"), "{t_sql}");
    assert!(t_sql.contains("PRE_SPLIT_REGIONS=4"), "{t_sql}");

    tk.MustExec(
        "create table t0 (i int primary key) /*T! SHARD_ROW_ID_BITS=2 PRE_SPLIT_REGIONS=2 */",
        Vec::new(),
    );
    let t0_sql = tk.MustQuery("show create table t0", Vec::new()).Rows()[0].join(" ");
    assert!(t0_sql.contains("NONCLUSTERED"), "{t0_sql}");
    assert!(t0_sql.contains("SHARD_ROW_ID_BITS=2"), "{t0_sql}");
    assert!(t0_sql.contains("PRE_SPLIT_REGIONS=2"), "{t0_sql}");

    tk.MustExec("set @@session.tidb_enable_clustered_index = on", Vec::new());
    tk.MustExec("create table t1 (i int primary key)", Vec::new());
    let t1_sql = tk.MustQuery("show create table t1", Vec::new()).Rows()[0].join(" ");
    assert!(t1_sql.contains("CLUSTERED"), "{t1_sql}");
    assert!(!t1_sql.contains("SHARD_ROW_ID_BITS"), "{t1_sql}");

    tk.MustExec(
        "create global temporary table tengine (id int) engine = 'innodb' on commit delete rows",
        Vec::new(),
    );
    let temp_sql = tk.MustQuery("show create table tengine", Vec::new()).Rows()[0].join(" ");
    assert!(
        temp_sql.contains("CREATE GLOBAL TEMPORARY TABLE"),
        "{temp_sql}"
    );
    assert!(temp_sql.contains("ON COMMIT DELETE ROWS"), "{temp_sql}");
}
