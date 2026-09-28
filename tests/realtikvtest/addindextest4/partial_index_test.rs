// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `prepare` 负责 prepare。
// 中文总览：函数 `table` 负责 表。
// 中文总览：函数 `index` 负责 索引。
// 中文总览：函数 `assert_partial_index` 负责 断言 部分索引。
// 中文总览：函数 `test_create_partial_index` 负责 创建 部分索引。

//! Partial-index DDL coverage ported from `partial_index_test.go`.
//!
//! Every DDL is parsed and executed through the canonical TestKit session.
//! Metadata assertions read the Domain published by that same store.

use std::sync::Arc;

use astersql_kv as kv;
use astersql_meta_model::{IndexInfo, TableInfo};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, TestKit};
use astersql_tests_realtikvtest_addindextest4::serial_guard;

// 该辅助函数负责 prepare。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn prepare(
    database: &str,
) -> (
    std::sync::MutexGuard<'static, ()>,
    Arc<AnalyzeStatsStore>,
    TestKit,
) {
    let serial = serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists {database}"), Vec::new());
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());
    (serial, store, tk)
}

// 该辅助函数负责 表。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn table(store: &AnalyzeStatsStore, database: &str, table_name: &str) -> TableInfo {
    (*store
        .domain()
        .table_by_name(database, table_name)
        .unwrap_or_else(|error| panic!("load {database}.{table_name}: {error}")))
    .clone()
}

// 该辅助函数负责 索引。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn index(
    store: &AnalyzeStatsStore,
    database: &str,
    table_name: &str,
    index_name: &str,
) -> Option<IndexInfo> {
    table(store, database, table_name)
        .Indices
        .into_iter()
        .find(|index| index.Name.L == index_name)
}

// 该辅助函数负责 断言 部分索引。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn assert_partial_index(
    store: &AnalyzeStatsStore,
    database: &str,
    table_name: &str,
    index_name: &str,
) -> IndexInfo {
    let index = index(store, database, table_name, index_name)
        .unwrap_or_else(|| panic!("missing index {database}.{table_name}.{index_name}"));
    assert!(
        !index.ConditionExprString.is_empty(),
        "{database}.{table_name}.{index_name} lost its partial predicate"
    );
    index
}

fn assert_index_kv_count(
    store: &AnalyzeStatsStore,
    database: &str,
    table_name: &str,
    index_name: &str,
    expected: usize,
) {
    let table = table(store, database, table_name);
    let index = table
        .Indices
        .iter()
        .find(|index| index.Name.L == index_name)
        .unwrap_or_else(|| panic!("missing index {database}.{table_name}.{index_name}"));
    let encode_int = |value: i64| ((value as u64) ^ (1_u64 << 63)).to_be_bytes();
    let mut start = b"t".to_vec();
    start.extend_from_slice(&encode_int(table.ID));
    start.extend_from_slice(b"_i");
    start.extend_from_slice(&encode_int(index.ID));
    let mut end = start.clone();
    end.push(255);
    let actual = store.domain().storage().with_storage(|storage| {
        let version = storage.CurrentVersion("global").expect("current version");
        let snapshot = storage.GetSnapshot(version);
        let mut iterator = snapshot
            .Iter(kv::Key(start), Some(kv::Key(end)))
            .expect("scan index key range");
        let mut count = 0;
        while iterator.Valid() {
            count += 1;
            iterator.Next().expect("advance index iterator");
        }
        iterator.Close();
        count
    });
    assert_eq!(
        actual, expected,
        "unexpected {table_name}.{index_name} KV count"
    );
}

fn assert_ddl_row_count(tk: &mut TestKit, job_offset: usize, expected: &str) {
    let jobs = tk
        .MustQuery(
            &format!("admin show ddl jobs {}", job_offset + 1),
            Vec::new(),
        )
        .Rows();
    assert!(
        jobs.len() > job_offset,
        "expected DDL job at offset {job_offset}"
    );
    assert_eq!(jobs[job_offset][7], expected);
}

/// Go `TestPartialIndexDDL/TestCreatePartialIndex`.
// 该用例覆盖 创建 部分索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_create_partial_index() {
    let (_serial, store, mut tk) = prepare("partial_create");

    tk.MustExec(
        "create table t1 (col1 int primary key, col2 int, key idx(col2) where col1 > 100)",
        Vec::new(),
    );
    assert_partial_index(&store, "partial_create", "t1", "idx");

    tk.MustExec(
        "create table t2 (col1 int primary key, col2 int)",
        Vec::new(),
    );
    tk.MustExec("create index idx on t2(col2) where col1 > 100", Vec::new());
    assert_partial_index(&store, "partial_create", "t2", "idx");

    tk.MustExec(
        "create table t3 (col1 int primary key, col2 int)",
        Vec::new(),
    );
    tk.MustExec(
        "alter table t3 add index idx(col2) where col1 > 100",
        Vec::new(),
    );
    assert_partial_index(&store, "partial_create", "t3", "idx");

    tk.MustExec("create table t4 like t1", Vec::new());
    let copied = assert_partial_index(&store, "partial_create", "t4", "idx");
    let original = assert_partial_index(&store, "partial_create", "t1", "idx");
    assert_eq!(copied.ConditionExprString, original.ConditionExprString);

    tk.MustExec("drop table t1, t2, t3, t4", Vec::new());
}

/// Go `TestPartialIndexDDL/TestValidationInPartialIndex`.
// 该用例覆盖 validation in 部分索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_validation_in_partial_index() {
    let (_serial, store, mut tk) = prepare("partial_validation");
    tk.MustExec(
        "create table t(col1 int primary key, col2 int, col3 varchar(255), \
         col4 int as (col2 + 1))",
        Vec::new(),
    );

    let cases = [
        ("t(col2) where col2 = 1", true),
        ("t(col2) where col1 != 1", true),
        ("t(col2) where col1 > 1", true),
        ("t(col2) where col1 IS NULL", true),
        ("t(col2) where col1 IS NOT NULL", true),
        ("t(col2) where col1 IN (1,2,3,4,5)", false),
        ("t(col2) where col1 LIKE '1%'", false),
        ("t(col2) where col1 > col2", false),
        ("t(col2) where col1 = NOW()", false),
        ("t(col2) where col1 = (select 1)", false),
        ("t(col2) where col4 = 4", false),
    ];
    for (definition, succeeds) in cases {
        let sql = format!("create index idx on {definition}");
        if succeeds {
            tk.MustExec(&sql, Vec::new());
            assert_partial_index(&store, "partial_validation", "t", "idx");
            tk.MustExec("drop index idx on t", Vec::new());
        } else {
            tk.MustContainErrMsg(&sql, "[ddl:8200]");
            assert!(index(&store, "partial_validation", "t", "idx").is_none());
        }
    }

    tk.MustExec("drop table t", Vec::new());
}

/// Go `TestPartialIndexDDL/TestIndexManagementForPartialIndex`.
// 该用例覆盖 索引 management for 部分索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_index_management_for_partial_index() {
    let (_serial, store, mut tk) = prepare("partial_management");
    tk.MustExec(
        "create table t(col1 int primary key, col2 int, col3 int)",
        Vec::new(),
    );
    tk.MustExec("create index idx on t(col3) where col2 = 1", Vec::new());
    assert_partial_index(&store, "partial_management", "t", "idx");

    tk.MustExec("alter table t rename index idx to idx2", Vec::new());
    assert!(index(&store, "partial_management", "t", "idx").is_none());
    assert_partial_index(&store, "partial_management", "t", "idx2");

    tk.MustExec("alter table t change column col3 col4 int", Vec::new());
    let renamed = assert_partial_index(&store, "partial_management", "t", "idx2");
    assert_eq!(renamed.Columns[0].Name.L, "col4");
    tk.MustExec("alter table t modify column col4 int unsigned", Vec::new());
    assert_eq!(
        assert_partial_index(&store, "partial_management", "t", "idx2").Columns[0]
            .Name
            .L,
        "col4"
    );
    tk.MustExec("alter table t drop column col4", Vec::new());
    assert!(index(&store, "partial_management", "t", "idx2").is_none());

    tk.MustExec("create index idx on t(col2) where col2 = 1", Vec::new());
    assert_partial_index(&store, "partial_management", "t", "idx");
    tk.MustExec("drop index idx on t", Vec::new());
    assert!(index(&store, "partial_management", "t", "idx").is_none());
    tk.MustExec("drop table t", Vec::new());
}

/// Go `TestPartialIndexDDL/TestManipulateColumnReferencedByPartialIndex`.
// 该用例覆盖 manipulate 列 referenced by 部分索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_manipulate_column_referenced_by_partial_index() {
    let (_serial, _store, mut tk) = prepare("partial_referenced_column");
    for inline in [false, true] {
        tk.MustExec("drop table if exists t", Vec::new());
        if inline {
            tk.MustExec(
                "create table t(col1 int primary key, col2 int, col3 int, \
                 key t(col3) where col2 = 1)",
                Vec::new(),
            );
        } else {
            tk.MustExec(
                "create table t(col1 int primary key, col2 int, col3 int)",
                Vec::new(),
            );
            tk.MustExec("create index idx on t(col3) where col2 = 1", Vec::new());
        }
        for sql in [
            "alter table t drop column col2",
            "alter table t change column col2 col4 int",
            "alter table t modify column col2 int unsigned",
        ] {
            tk.MustContainErrMsg(sql, "partial index");
        }
    }
    tk.MustExec("drop table t", Vec::new());
}

/// Go `TestPartialIndexDDL/TestPartialIndexCanOnlyBeCreatedWithFastReorg`.
// 该用例覆盖 部分索引 requires fast reorg。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_partial_index_requires_fast_reorg() {
    let (_serial, store, mut tk) = prepare("partial_fast_reorg");
    tk.MustExec(
        "create table t (a int, b int, c int, primary key (a))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t(a, b, c) values (1, 2, 3), (2, 3, 4), (3, 4, 5)",
        Vec::new(),
    );

    tk.MustExec("set global tidb_ddl_enable_fast_reorg = 0", Vec::new());
    tk.MustContainErrMsg("alter table t add index idx1(a) where c > 3", "[ddl:8200]");
    assert!(index(&store, "partial_fast_reorg", "t", "idx1").is_none());
    tk.MustExec("set global tidb_ddl_enable_fast_reorg = 1", Vec::new());

    tk.MustExec("alter table t add index idx0(a)", Vec::new());
    assert_index_kv_count(&store, "partial_fast_reorg", "t", "idx0", 3);
    assert_ddl_row_count(&mut tk, 0, "3");
    tk.MustExec("drop table t", Vec::new());
}

/// Go `TestPartialIndexDDL/TestAddPartialIndex`.
// 该用例覆盖 添加 部分索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_partial_index() {
    let (_serial, store, mut tk) = prepare("partial_add");
    tk.MustExec(
        "create table t (a int, b int, c int, primary key (a))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t(a, b, c) values (1, 2, 3), (2, 3, 4), (3, 4, 5)",
        Vec::new(),
    );

    tk.MustExec("alter table t add index idx1(a) where c >= 5", Vec::new());
    assert_index_kv_count(&store, "partial_add", "t", "idx1", 1);
    assert_ddl_row_count(&mut tk, 0, "3");

    tk.MustExec("alter table t add index idx2(a) where c >= 6", Vec::new());
    assert_index_kv_count(&store, "partial_add", "t", "idx2", 0);
    assert_ddl_row_count(&mut tk, 0, "3");

    tk.MustExec(
        "alter table t add index idx5(a) where c >= 4, \
         add index idx6(a) where c >= 5",
        Vec::new(),
    );
    assert_index_kv_count(&store, "partial_add", "t", "idx5", 2);
    assert_index_kv_count(&store, "partial_add", "t", "idx6", 1);
    assert_ddl_row_count(&mut tk, 1, "3");
    assert_partial_index(&store, "partial_add", "t", "idx5");
    assert_partial_index(&store, "partial_add", "t", "idx6");

    tk.MustExec(
        "INSERT INTO mysql.expr_pushdown_blacklist VALUES('not','tikv','')",
        Vec::new(),
    );
    tk.MustExec("ADMIN reload expr_pushdown_blacklist", Vec::new());
    tk.MustExec(
        "alter table t add index idx7(a) where b is not null",
        Vec::new(),
    );
    assert_index_kv_count(&store, "partial_add", "t", "idx7", 3);
    tk.MustExec(
        "DELETE FROM mysql.expr_pushdown_blacklist WHERE name='not' AND store_type='tikv'",
        Vec::new(),
    );

    tk.MustExec(
        "alter table t add index idx8(a), add index idx9(a) where c >= 5",
        Vec::new(),
    );
    assert_index_kv_count(&store, "partial_add", "t", "idx8", 3);
    assert_index_kv_count(&store, "partial_add", "t", "idx9", 1);
    assert_ddl_row_count(&mut tk, 1, "3");
    assert!(index(&store, "partial_add", "t", "idx8").is_some());
    assert_partial_index(&store, "partial_add", "t", "idx9");

    tk.MustExec("create table t1 (a int, b int, c int)", Vec::new());
    tk.MustExec(
        "insert into t1(a, b, c) values (1, 2, 3), (2, 3, 4), (3, 4, 5)",
        Vec::new(),
    );
    tk.MustExec("alter table t1 add index idx1(a) where a > 1", Vec::new());
    assert_index_kv_count(&store, "partial_add", "t1", "idx1", 2);
    tk.MustExec("alter table t1 add index idx2(a)", Vec::new());
    assert_index_kv_count(&store, "partial_add", "t1", "idx2", 3);
    assert_ddl_row_count(&mut tk, 0, "3");

    tk.MustExec("drop table t, t1", Vec::new());
}

/// Go `TestPartialIndexDDL/TestValidateColumnExistsInAddIndex`.
// 该用例覆盖 validate 列 exists in 添加 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_validate_column_exists_in_add_index() {
    let (_serial, _store, mut tk) = prepare("partial_unknown_column");
    tk.MustExec("create table t (a int, b int)", Vec::new());
    tk.MustExec("alter table t add index idx_b(b) where a = 1", Vec::new());
    tk.MustContainErrMsg(
        "alter table t add index idx_b_2(b) where c = 1",
        "[ddl:8200]",
    );
    tk.MustExec("drop table t", Vec::new());
}
