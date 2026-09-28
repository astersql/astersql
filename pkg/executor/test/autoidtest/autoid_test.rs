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

// 自增 ID（auto_increment / auto_random）分配与过滤行为测试。
//
// 对应 `pkg/executor/test/autoidtest/autoid_test.go`。
//
// 完整 auto_random / AUTO_ID_CACHE / failpoint / TestKit SQL 路径尚未端到端接线，
// 因此本文件对生产 `Allocators::filter`、`InMemoryAllocator`、`ShardIdFormat`、
// `MaskSortHandles` 做真实断言，并保留 Go SQL fixture 文本。
//
// AutoIncrement：按步长分配递增主键；AutoRandom：在高位嵌入分片信息以打散写入热点；
// AUTO_ID_CACHE：缓存一批 ID 以减少对元数据服务的往返。

#![allow(non_snake_case)]

use std::sync::Arc;

use astersql_meta_autoid::{
    Allocator, AllocatorType, Allocators, Context, InMemoryAllocator, ShardIdFormat,
};
use astersql_parser_mysql::r#type::TypeLonglong;
use astersql_testkit::{NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};
use astersql_testkit_testutil::MaskSortHandles;

/// AutoidCase 对应 Go helper testInsertWithAutoidSchema 中的表驱动测试项。
struct AutoidCase {
    /// 待执行的 INSERT（`;` 表示沿用上一语句的查询侧断言）。
    insert: &'static str,
    /// 用于核对结果的 SELECT / SET / last_insert_id 语句。
    query: &'static str,
    /// 期望结果行；`None` 表示仅执行不核对行集。
    result: Option<&'static [&'static str]>,
    /// 用例说明（英文，与 Go 注释意图对应）。
    note: &'static str,
}

/// QueryCheck 对应 Go 中 tk.MustQuery(...).Check(...) 或 CheckContain(...)。
struct QueryCheck {
    /// 查询 SQL。
    sql: &'static str,
    /// 期望完整行集（空切片时可能改用 contains）。
    rows: &'static [&'static str],
    /// 若设置，则断言结果字符串包含该片段（如 SHOW CREATE 中的 next id）。
    contains: Option<&'static str>,
}

/// 返回无缓存与 `AUTO_ID_CACHE 1` 两套建表后缀，覆盖默认与逐次取号路径。
fn filter_allocator_suffixes() -> Vec<&'static str> {
    vec!["", " AUTO_ID_CACHE 1"]
}

/// 生成 t1..t8 建表 SQL；多数表追加 `cache_suffix`，t8 刻意不加（对齐 Go）。
fn create_autoid_schema_sqls(cache_suffix: &str) -> Vec<String> {
    vec![
        format!("create table t1(id int primary key auto_increment, n int){cache_suffix};"),
        format!(
            "create table t2(id int unsigned primary key auto_increment, n int){cache_suffix};"
        ),
        format!("create table t3(id tinyint primary key auto_increment, n int){cache_suffix};"),
        format!(
            "create table t4(id int primary key, n float auto_increment, key I_n(n)){cache_suffix};"
        ),
        format!(
            "create table t5(id int primary key, n float unsigned auto_increment, key I_n(n)){cache_suffix};"
        ),
        format!(
            "create table t6(id int primary key, n double auto_increment, key I_n(n)){cache_suffix};"
        ),
        format!(
            "create table t7(id int primary key, n double unsigned auto_increment, key I_n(n)){cache_suffix};"
        ),
        // Go 原文件里 t8 的第二轮建表没有追加 cache_suffix。
        "create table t8(id int primary key auto_increment, n int);".to_owned(),
    ]
}

/// 完整表驱动用例：显式 id、类型强制转换、rebase、`NO_AUTO_VALUE_ON_ZERO`、retryInfo 等。
fn autoid_cases() -> Vec<AutoidCase> {
    vec![
        AutoidCase {
            insert: "insert into t1(id, n) values(1, 1)",
            query: "select * from t1 where id = 1",
            result: Some(&["1 1"]),
            note: "explicit int auto_increment id",
        },
        AutoidCase {
            insert: "insert into t1(n) values(2)",
            query: "select * from t1 where id = 2",
            result: Some(&["2 2"]),
            note: "allocated next id",
        },
        AutoidCase {
            insert: "insert into t1(n) values(3)",
            query: "select * from t1 where id = 3",
            result: Some(&["3 3"]),
            note: "allocated next id again",
        },
        AutoidCase {
            insert: "insert into t1(id, n) values(-1, 4)",
            query: "select * from t1 where id = -1",
            result: Some(&["-1 4"]),
            note: "negative explicit id does not advance positive allocator",
        },
        AutoidCase {
            insert: "insert into t1(n) values(5)",
            query: "select * from t1 where id = 4",
            result: Some(&["4 5"]),
            note: "allocator continues at 4",
        },
        AutoidCase {
            insert: "insert into t1(id, n) values('5', 6)",
            query: "select * from t1 where id = 5",
            result: Some(&["5 6"]),
            note: "string literal is coerced to id",
        },
        AutoidCase {
            insert: "insert into t1(n) values(7)",
            query: "select * from t1 where id = 6",
            result: Some(&["6 7"]),
            note: "next id after explicit 5",
        },
        AutoidCase {
            insert: "insert into t1(id, n) values(7.4, 8)",
            query: "select * from t1 where id = 7",
            result: Some(&["7 8"]),
            note: "float rounds/coerces down",
        },
        AutoidCase {
            insert: "insert into t1(id, n) values(7.5, 9)",
            query: "select * from t1 where id = 8",
            result: Some(&["8 9"]),
            note: "float rounds/coerces to next integer",
        },
        AutoidCase {
            insert: "insert into t1(n) values(9)",
            query: "select * from t1 where id = 9",
            result: Some(&["9 9"]),
            note: "allocator continues after coercions",
        },
        AutoidCase {
            insert: "insert into t1 values(3000, -1), (null, -2)",
            query: "select * from t1 where id = 3000",
            result: Some(&["3000 -1"]),
            note: "multi-row explicit and null id",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t1 where id = 3001",
            result: Some(&["3001 -2"]),
            note: "second row allocated after explicit 3000",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id()",
            result: Some(&["3001"]),
            note: "last_insert_id is allocated null id",
        },
        AutoidCase {
            insert: "insert into t2(id, n) values(1, 1)",
            query: "select * from t2 where id = 1",
            result: Some(&["1 1"]),
            note: "unsigned int explicit id",
        },
        AutoidCase {
            insert: "insert into t2(n) values(2)",
            query: "select * from t2 where id = 2",
            result: Some(&["2 2"]),
            note: "unsigned int allocated id",
        },
        AutoidCase {
            insert: "insert into t2(n) values(3)",
            query: "select * from t2 where id = 3",
            result: Some(&["3 3"]),
            note: "unsigned int next id",
        },
        AutoidCase {
            insert: "insert into t3(id, n) values(1, 1)",
            query: "select * from t3 where id = 1",
            result: Some(&["1 1"]),
            note: "tinyint explicit id",
        },
        AutoidCase {
            insert: "insert into t3(n) values(2)",
            query: "select * from t3 where id = 2",
            result: Some(&["2 2"]),
            note: "tinyint allocated id",
        },
        AutoidCase {
            insert: "insert into t3(n) values(3)",
            query: "select * from t3 where id = 3",
            result: Some(&["3 3"]),
            note: "tinyint next id",
        },
        AutoidCase {
            insert: "insert into t3(id, n) values(-1, 4)",
            query: "select * from t3 where id = -1",
            result: Some(&["-1 4"]),
            note: "tinyint negative explicit id",
        },
        AutoidCase {
            insert: "insert into t3(n) values(5)",
            query: "select * from t3 where id = 4",
            result: Some(&["4 5"]),
            note: "tinyint allocator continues",
        },
        AutoidCase {
            insert: "insert into t4(id, n) values(1, 1)",
            query: "select * from t4 where id = 1",
            result: Some(&["1 1"]),
            note: "float auto_increment explicit value",
        },
        AutoidCase {
            insert: "insert into t4(id) values(2)",
            query: "select * from t4 where id = 2",
            result: Some(&["2 2"]),
            note: "float auto_increment allocates 2",
        },
        AutoidCase {
            insert: "insert into t4(id, n) values(3, -1)",
            query: "select * from t4 where id = 3",
            result: Some(&["3 -1"]),
            note: "negative float value does not rebase allocator as positive",
        },
        AutoidCase {
            insert: "insert into t4(id) values(4)",
            query: "select * from t4 where id = 4",
            result: Some(&["4 3"]),
            note: "float allocator resumes at 3",
        },
        AutoidCase {
            insert: "insert into t4(id, n) values(5, 5.5)",
            query: "select * from t4 where id = 5",
            result: Some(&["5 5.5"]),
            note: "float explicit decimal",
        },
        AutoidCase {
            insert: "insert into t4(id) values(6)",
            query: "select * from t4 where id = 6",
            result: Some(&["6 7"]),
            note: "float allocator after 5.5",
        },
        AutoidCase {
            insert: "insert into t4(id, n) values(7, '7.7')",
            query: "select * from t4 where id = 7",
            result: Some(&["7 7.7"]),
            note: "string decimal coerces to float",
        },
        AutoidCase {
            insert: "insert into t4(id) values(8)",
            query: "select * from t4 where id = 8",
            result: Some(&["8 9"]),
            note: "float allocator rounds next value",
        },
        AutoidCase {
            insert: "insert into t4(id, n) values(9, 10.4)",
            query: "select * from t4 where id = 9",
            result: Some(&["9 10.4"]),
            note: "explicit decimal above next",
        },
        AutoidCase {
            insert: "insert into t4(id) values(10)",
            query: "select * from t4 where id = 10",
            result: Some(&["10 11"]),
            note: "float allocator follows explicit 10.4",
        },
        AutoidCase {
            insert: "insert into t5(id, n) values(1, 1)",
            query: "select * from t5 where id = 1",
            result: Some(&["1 1"]),
            note: "float unsigned explicit",
        },
        AutoidCase {
            insert: "insert into t5(id) values(2)",
            query: "select * from t5 where id = 2",
            result: Some(&["2 2"]),
            note: "float unsigned allocated 2",
        },
        AutoidCase {
            insert: "insert into t5(id) values(3)",
            query: "select * from t5 where id = 3",
            result: Some(&["3 3"]),
            note: "float unsigned allocated 3",
        },
        AutoidCase {
            insert: "insert into t6(id, n) values(1, 1)",
            query: "select * from t6 where id = 1",
            result: Some(&["1 1"]),
            note: "double explicit value",
        },
        AutoidCase {
            insert: "insert into t6(id) values(2)",
            query: "select * from t6 where id = 2",
            result: Some(&["2 2"]),
            note: "double allocated 2",
        },
        AutoidCase {
            insert: "insert into t6(id, n) values(3, -1)",
            query: "select * from t6 where id = 3",
            result: Some(&["3 -1"]),
            note: "double negative explicit value",
        },
        AutoidCase {
            insert: "insert into t6(id) values(4)",
            query: "select * from t6 where id = 4",
            result: Some(&["4 3"]),
            note: "double allocator resumes at 3",
        },
        AutoidCase {
            insert: "insert into t6(id, n) values(5, 5.5)",
            query: "select * from t6 where id = 5",
            result: Some(&["5 5.5"]),
            note: "double explicit decimal",
        },
        AutoidCase {
            insert: "insert into t6(id) values(6)",
            query: "select * from t6 where id = 6",
            result: Some(&["6 7"]),
            note: "double allocator after 5.5",
        },
        AutoidCase {
            insert: "insert into t6(id, n) values(7, '7.7')",
            query: "select * from t4 where id = 7",
            result: Some(&["7 7.7"]),
            note: "Go source intentionally queries t4 here",
        },
        AutoidCase {
            insert: "insert into t6(id) values(8)",
            query: "select * from t4 where id = 8",
            result: Some(&["8 9"]),
            note: "Go source intentionally queries t4 here",
        },
        AutoidCase {
            insert: "insert into t6(id, n) values(9, 10.4)",
            query: "select * from t6 where id = 9",
            result: Some(&["9 10.4"]),
            note: "double explicit 10.4",
        },
        AutoidCase {
            insert: "insert into t6(id) values(10)",
            query: "select * from t6 where id = 10",
            result: Some(&["10 11"]),
            note: "double allocator next",
        },
        AutoidCase {
            insert: "insert into t7(id, n) values(1, 1)",
            query: "select * from t7 where id = 1",
            result: Some(&["1 1"]),
            note: "double unsigned explicit",
        },
        AutoidCase {
            insert: "insert into t7(id) values(2)",
            query: "select * from t7 where id = 2",
            result: Some(&["2 2"]),
            note: "double unsigned allocated 2",
        },
        AutoidCase {
            insert: "insert into t7(id) values(3)",
            query: "select * from t7 where id = 3",
            result: Some(&["3 3"]),
            note: "double unsigned allocated 3",
        },
        AutoidCase {
            insert: "insert into t8(n) values(1),(2)",
            query: "select * from t8 where id = 1",
            result: Some(&["1 1"]),
            note: "multi-value insert first row",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 2",
            result: Some(&["2 2"]),
            note: "multi-value insert second row",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id();",
            result: Some(&["1"]),
            note: "last_insert_id is first allocated id in multi-value insert",
        },
        AutoidCase {
            insert: "insert into t8 values(null, 3),(-1, -1),(null,4),(null, 5)",
            query: "select * from t8 where id = 3",
            result: Some(&["3 3"]),
            note: "mixed user rebase and allocated ids",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = -1",
            result: Some(&["-1 -1"]),
            note: "-1 does not rebase allocator because it is below base",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 4",
            result: Some(&["4 4"]),
            note: "allocated row after negative explicit id",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 5",
            result: Some(&["5 5"]),
            note: "next allocated row",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id();",
            result: Some(&["3"]),
            note: "last_insert_id remains first allocated id",
        },
        AutoidCase {
            insert: "insert into t8 values(null, 6),(10, 7),(null, 8)",
            query: "select * from t8 where id = 6",
            result: Some(&["6 6"]),
            note: "allocated id before explicit rebase",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 10",
            result: Some(&["10 7"]),
            note: "explicit 10 rebases allocator",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 11",
            result: Some(&["11 8"]),
            note: "allocation after explicit rebase",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id()",
            result: Some(&["6"]),
            note: "last_insert_id skips rebase id and stays first allocated id",
        },
        AutoidCase {
            insert: "insert into t8 values(100, 9),(null,10),(null,11)",
            query: "select * from t8 where id = 100",
            result: Some(&["100 9"]),
            note: "explicit 100 rebases before allocated rows",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 101",
            result: Some(&["101 10"]),
            note: "allocated after explicit 100",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 102",
            result: Some(&["102 11"]),
            note: "second allocated after explicit 100",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id()",
            result: Some(&["101"]),
            note: "bug fix: last_insert_id is first allocated id, not user rebase id",
        },
        AutoidCase {
            insert: ";",
            query: "select @@sql_mode",
            result: Some(&[
                "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION",
            ]),
            note: "baseline sql_mode before NO_AUTO_VALUE_ON_ZERO",
        },
        AutoidCase {
            insert: ";",
            query: "set session sql_mode = `ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION,NO_AUTO_VALUE_ON_ZERO`",
            result: None,
            note: "enable NO_AUTO_VALUE_ON_ZERO",
        },
        AutoidCase {
            insert: "insert into t8 values (0, 12), (null, 13)",
            query: "select * from t8 where id = 0",
            result: Some(&["0 12"]),
            note: "zero remains explicit under NO_AUTO_VALUE_ON_ZERO",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 103",
            result: Some(&["103 13"]),
            note: "null allocates after previous base",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id()",
            result: Some(&["103"]),
            note: "last_insert_id is allocated null row",
        },
        AutoidCase {
            insert: ";",
            query: "set session sql_mode = `ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION`",
            result: None,
            note: "disable NO_AUTO_VALUE_ON_ZERO",
        },
        AutoidCase {
            insert: "insert into t8 values (0, 14), (null, 15)",
            query: "select * from t8 where id = 104",
            result: Some(&["104 14"]),
            note: "zero is substituted by autoid without NO_AUTO_VALUE_ON_ZERO",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 105",
            result: Some(&["105 15"]),
            note: "second allocated row after zero substitution",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id()",
            result: Some(&["104"]),
            note: "last_insert_id is first allocated id after zero substitution",
        },
        AutoidCase {
            insert: "retry : insert into t8 values (null, 16), (null, 17)",
            query: "select * from t8 where id = 1000",
            result: Some(&["1000 16"]),
            note: "retryInfo supplies first auto increment id",
        },
        AutoidCase {
            insert: ";",
            query: "select * from t8 where id = 1001",
            result: Some(&["1001 17"]),
            note: "retryInfo supplies second auto increment id",
        },
        AutoidCase {
            insert: ";",
            query: "select last_insert_id()",
            result: Some(&["104"]),
            note: "retry insert does not change last_insert_id",
        },
    ]
}

/// Issue #52622：带/不带 AUTO_ID_CACHE 时，按 increment/offset 分配后的查询核对项。
fn issue52622_queries(cache_one: bool) -> Vec<QueryCheck> {
    let mut checks = vec![QueryCheck {
        sql: "select * from issue52622",
        rows: &["1 1", "67 2", "133 3"],
        contains: None,
    }];
    if cache_one {
        // AUTO_ID_CACHE 1 时 SHOW CREATE 会暴露下一可用 id（fixture 期望含 "134"）。
        checks.push(QueryCheck {
            sql: "show create table issue52622",
            rows: &[],
            contains: Some("134"),
        });
    }
    checks.push(QueryCheck {
        sql: "select * from issue52622",
        rows: &["1 1", "67 2", "133 3", "199 4"],
        contains: None,
    });
    checks
}

/// 构造同时持有 AutoIncrement 与 AutoRandom 的 Allocators，供 filter 用例使用。
fn dual_allocators() -> Allocators {
    Allocators::new(
        true,
        vec![
            Arc::new(InMemoryAllocator::new(false, AllocatorType::AutoIncrement))
                as Arc<dyn Allocator>,
            Arc::new(InMemoryAllocator::new(false, AllocatorType::AutoRandom))
                as Arc<dyn Allocator>,
        ],
    )
}

/// 对应 Go `TestFilterDifferentAllocators`：rename 放弃全部 allocator；
/// rebase auto_increment 丢掉 AutoIncrement；rebase auto_random 丢掉 AutoRandom。
#[test]
fn TestFilterDifferentAllocators() {
    let _guard = crate::AUTOID_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());

    for suffix in filter_allocator_suffixes() {
        tk.MustExec(
            &format!(
                "create table t(a bigint auto_random(5) key, b int auto_increment unique){suffix}"
            ),
            Vec::new(),
        );
        tk.MustExec("insert into t values()", Vec::new());
        let rows = tk.MustQuery("select a, b from t", Vec::new()).Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "1");
        let handle = rows[0][0].parse::<i64>().expect("auto_random handle");
        assert_eq!(MaskSortHandles(vec![handle], 5, TypeLonglong), vec![1]);
        tk.MustExec("delete from t", Vec::new());

        tk.MustExec("alter table t auto_increment 3000000", Vec::new());
        tk.MustExec("insert into t values()", Vec::new());
        let rows = tk.MustQuery("select a, b from t", Vec::new()).Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "3000000");
        let handle = rows[0][0].parse::<i64>().expect("auto_random handle");
        assert_eq!(MaskSortHandles(vec![handle], 5, TypeLonglong), vec![2]);
        tk.MustExec("delete from t", Vec::new());

        tk.MustExec("alter table t auto_random_base 3000000", Vec::new());
        tk.MustExec("insert into t values()", Vec::new());
        let rows = tk.MustQuery("select a, b from t", Vec::new()).Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "3000001");
        let handle = rows[0][0].parse::<i64>().expect("auto_random handle");
        assert_eq!(
            MaskSortHandles(vec![handle], 5, TypeLonglong),
            vec![3_000_000]
        );
        tk.MustExec("delete from t", Vec::new());

        tk.MustExec("rename table t to t1", Vec::new());
        tk.MustExec("insert into t1 values()", Vec::new());
        let rows = tk.MustQuery("select a, b from t1", Vec::new()).Rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0][1].parse::<i64>().expect("auto_increment") >= 3_000_002);
        let handle = rows[0][0].parse::<i64>().expect("auto_random handle");
        assert!(MaskSortHandles(vec![handle], 5, TypeLonglong)[0] >= 3_000_001);
        tk.MustExec("drop table t1", Vec::new());
    }
}

/// 对应 Go `TestInsertWithAutoidSchema`：通过真实 TestKit/session 执行完整表驱动场景。
#[test]
fn TestInsertWithAutoidSchema() {
    let _guard = crate::AUTOID_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set session sql_mode = `ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION`",
        Vec::new(),
    );
    for suffix in filter_allocator_suffixes() {
        if !suffix.is_empty() {
            tk.MustExec(
                "drop table if exists t1, t2, t3, t4, t5, t6, t7, t8",
                Vec::new(),
            );
        }
        let sqls = create_autoid_schema_sqls(suffix);
        assert_eq!(sqls.len(), 8);
        for sql in &sqls {
            tk.MustExec(sql, Vec::new());
        }

        let cases = autoid_cases();
        assert_eq!(cases.len(), 75);
        for case in cases {
            let insert = if let Some(insert) = case.insert.strip_prefix("retry : ") {
                tk.Session()
                    .SetRetryAutoIncrementIDsForTest(vec![1000, 1001])
                    .expect("install retry auto IDs");
                insert
            } else {
                case.insert
            };
            tk.MustExec(insert, Vec::new());
            if let Some(expected) = case.result {
                tk.MustQuery(case.query, Vec::new()).Check(Rows(expected));
            } else {
                tk.MustExec(case.query, Vec::new());
            }
        }
    }
}

/// 对应 Go `TestMockAutoIDServiceError`：保留 failpoint 名称与建表 SQL。
/// 模拟 autoid 元数据服务返回错误时，AUTO_ID_CACHE 1 表的插入路径。
#[test]
fn TestMockAutoIDServiceError() {
    let _guard = crate::AUTOID_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    const FP: &str = "github.com/pingcap/tidb/pkg/autoid_service/mockErr";
    let (store, _) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_mock_err (id int key auto_increment) auto_id_cache 1",
        Vec::new(),
    );
    let _guard = astersql_testkit_testfailpoint::enable(FP, "return(true)");
    let error = tk.ExecToErr("insert into t_mock_err values (),()");
    assert!(error.message().contains("auto increment action failed"));
}

/// 对应 Go `TestIssue39528`：AUTO_ID_CACHE=1 + shard_row_id_bits 分离路径的 fixture。
/// shard_row_id_bits：隐藏行 ID 高位分片位数；nonclustered 主键与 RowId 分离分配。
#[test]
fn TestIssue39528() {
    let _guard = crate::AUTOID_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table issue39528 (id int unsigned key nonclustered auto_increment) \
         shard_row_id_bits=4 auto_id_cache 1",
        Vec::new(),
    );
    tk.MustExec("insert into issue39528 values ()", Vec::new());
    tk.MustExec("insert into issue39528 values ()", Vec::new());

    // Go 在第三次插入的 allocator context 中安装探针，断言隐藏 RowID 的本地缓存
    // 仍然充足，因而不会访问 TiKV。Rust 没有透传该 context；检查持久化高水位
    // 同样能证明 RowID 使用默认预留步长，而没有错误继承 AUTO_ID_CACHE 1。
    let table_id = domain
        .stats_table("test", "issue39528")
        .expect("issue39528 table metadata")
        .1
        .ID;
    assert!(
        domain
            .stats_auto_id_base(table_id, 2)
            .expect("hidden RowID allocator")
            > 3,
        "AUTO_ID_CACHE 1 must not shrink the separate hidden RowID reservation"
    );

    tk.MustExec("insert into issue39528 values ()", Vec::new());
    tk.MustQuery("select id from issue39528 order by id", Vec::new())
        .Check(Rows(&["1", "2", "3"]));
}

/// 对应 Go `TestIssue52622`：increment/offset 下的分配序列与 fixture。
/// `auto_increment_increment` / `auto_increment_offset`：控制自增值的步长与起点偏移。
#[test]
fn TestIssue52622() {
    let _guard = crate::AUTOID_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@auto_increment_increment = 66", Vec::new());
    tk.MustExec("set @@auto_increment_offset = 9527", Vec::new());
    tk.MustQuery("select @@auto_increment_increment", Vec::new())
        .Check(Rows(&["66"]));
    tk.MustQuery("select @@auto_increment_offset", Vec::new())
        .Check(Rows(&["9527"]));

    for cache_one in [true, false] {
        let create = if cache_one {
            "create table issue52622 (id int primary key auto_increment, k int) AUTO_ID_CACHE 1"
        } else {
            "create table issue52622 (id int primary key auto_increment, k int)"
        };
        tk.MustExec(create, Vec::new());
        tk.MustExec("insert into issue52622 (k) values (1),(2),(3)", Vec::new());
        tk.MustQuery("select * from issue52622", Vec::new())
            .Check(Rows(&["1 1", "67 2", "133 3"]));
        if cache_one {
            tk.MustQuery("show create table issue52622", Vec::new())
                .CheckContain("134");
        }
        tk.MustExec("insert into issue52622 (k) values (4)", Vec::new());
        tk.MustQuery("select * from issue52622", Vec::new())
            .Check(Rows(&["1 1", "67 2", "133 3", "199 4"]));

        tk.MustExec("truncate table issue52622", Vec::new());
        tk.MustExec("insert into issue52622 (k) values (1)", Vec::new());
        tk.MustExec("insert into issue52622 (k) values (2)", Vec::new());
        tk.MustExec("insert into issue52622 (k) values (3)", Vec::new());
        if cache_one {
            tk.MustQuery("show create table issue52622", Vec::new())
                .CheckContain("134");
        }
        tk.MustExec("insert into issue52622 (k) values (4)", Vec::new());
        tk.MustQuery("select * from issue52622", Vec::new())
            .Check(Rows(&["1 1", "67 2", "133 3", "199 4"]));
        tk.MustExec("drop table issue52622", Vec::new());
    }
}

/// Go 接受负数显式 AUTO_INCREMENT 值，且该值不会推进正数分配器。
#[test]
fn TestAutoIncrementNegativeExplicitIdThroughSession() {
    let _guard = crate::AUTOID_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (store, _) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_negative(id int primary key auto_increment, n int)",
        Vec::new(),
    );
    tk.MustExec("insert into t_negative(id, n) values(-1, 1)", Vec::new());
    tk.MustQuery("select * from t_negative where id = -1", Vec::new())
        .Check(Rows(&["-1 1"]));
    tk.MustExec("insert into t_negative(n) values(2)", Vec::new());
    tk.MustQuery("select * from t_negative where id = 1", Vec::new())
        .Check(Rows(&["1 2"]));
}
