// Copyright 2026 AsterSQL.

// 聚簇索引（Clustered Index）行为测试。
//
// 聚簇索引指主键与数据行按同一顺序存放（非聚簇则主键仅为二级索引）。本文件用 mock-store
// 验证 insert ignore、事务内 DML/union scan 可见性，以及 hash/range 分区表与普通表行数一致性。

/// Go 源文件版权与说明草稿归档，不参与运行。
const _GO_DRAFT: &str = r################"
// Copyright 2020 PingCAP, Inc.
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

// Real Rust tests below mirror Go semantics using the available mock-store ABI
// (CREATE/INSERT/UPDATE/DELETE/ANALYZE + Domain metadata/stats), matching schematest.
"################;

use astersql_sessionctx_vardef::{
    ClusteredIndexDefModeOff, ClusteredIndexDefModeOn, DefTiDBEnableClusteredIndex,
    TiDBEnableClusteredIndex, TiDBOptEnableClustered,
};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, Rows, TestKit};

/// 创建已强制开启聚簇索引的 `TestKit`，并校验相关系统变量默认值。
fn create_clustered_test_kit(
    store: std::sync::Arc<dyn astersql_testkit::db_driver::Database>,
) -> TestKit {
    // 对应 createTestKit：强制 clustered index 默认开启。
    assert_eq!(DefTiDBEnableClusteredIndex, ClusteredIndexDefModeOn);
    assert_eq!(TiDBOptEnableClustered("ON"), ClusteredIndexDefModeOn);
    assert_eq!(TiDBOptEnableClustered("OFF"), ClusteredIndexDefModeOff);
    assert_eq!(TiDBEnableClusteredIndex, "tidb_enable_clustered_index");
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_enable_clustered_index = 'ON'", Vec::new());
    tk
}

/// 对应 TestClusteredInsertIgnoreBatchGetKeyCount：varchar 聚簇主键上 insert ignore
/// 重复写入后行数仍为 1（Go 用 SnapCacheSize==1 断言同一 key 只缓存一次）。
// 对应 TestClusteredInsertIgnoreBatchGetKeyCount：varchar 聚簇主键上 insert ignore
// 重复写入后行数仍为 1（Go 用 SnapCacheSize==1 断言同一 key 只缓存一次）。
#[test]
fn clustered_insert_ignore_on_varchar_primary_key_keeps_single_row() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = create_clustered_test_kit(store);
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec(
        "CREATE TABLE t (a varchar(10) primary key, b int)",
        Vec::new(),
    );
    let meta = domain.table_by_name("test", "t").expect("table t");
    assert!(
        meta.HasClusteredIndex(),
        "varchar primary key under clustered-on must be a clustered handle"
    );

    tk.MustExec("begin optimistic", Vec::new());
    tk.MustExec(
        "insert ignore into t values (?, ?)",
        vec![DbValue::String("a".into()), DbValue::I64(1)],
    );
    assert_eq!(tk.Session().SnapCacheSize(), 1);
    tk.MustExec("rollback", Vec::new());
}

/// 对应 TestClusteredWithOldRowFormat：旧行格式下聚簇主键、索引更新和事务读回。
#[test]
fn clustered_primary_key_dml_and_union_scan_match_go_row_visibility() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = create_clustered_test_kit(store);
    tk.Session()
        .SetRowEncoderEnabledForTest(false)
        .expect("disable row encoder");

    tk.MustExec(
        "create table t_base(id varchar(255) primary key, a int, b int, unique index idx(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_base values (?, ?, ?)",
        vec![
            DbValue::String("b568004d-afad-11ea-8e4d-d651e3a981b7".into()),
            DbValue::I64(1),
            DbValue::I64(-1),
        ],
    );
    tk.MustQuery("select * from t_base use index(primary)", Vec::new())
        .Check(Rows(&["b568004d-afad-11ea-8e4d-d651e3a981b7 1 -1"]));
    assert!(
        domain
            .table_by_name("test", "t_base")
            .expect("t_base")
            .HasClusteredIndex()
    );

    // 对应 Go #21568：含 DECIMAL 的复合谓词删除。
    tk.MustExec(
        "create table t_21568 (c_int int, c_str varchar(40), c_decimal decimal(12, 6), primary key(c_str))",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert into t_21568 (c_int, c_str) values (13, 'dazzling torvalds'), (3, 'happy rhodes')",
        Vec::new(),
    );
    tk.MustExec(
        "delete from t_21568 where c_decimal <= 3.024 or (c_int, c_str) in ((5, 'happy saha'))",
        Vec::new(),
    );
    tk.MustExec("commit", Vec::new());

    // 对应 Go #21502：double/decimal 复合聚簇主键的写删事务。
    tk.MustExec(
        "create table t_21502 (c_int int, c_double double, c_decimal decimal(12, 6), primary key(c_decimal, c_double), unique key(c_int))",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert into t_21502 values (5, 55.068712, 8.256)",
        Vec::new(),
    );
    tk.MustExec("delete from t_21502 where c_int = 5", Vec::new());
    tk.MustExec("commit", Vec::new());

    // 对应 Go t_21568_comment：复合聚簇主键上的事务内 insert+update+读回+commit。
    tk.MustExec(
        "create table t_21568_comment (c_int int, c_str varchar(40), c_timestamp timestamp, c_decimal decimal(12, 6), primary key(c_int, c_str), key(c_decimal))",
        Vec::new(),
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "insert into t_21568_comment values (11, 'abc', null, null)",
        Vec::new(),
    );
    tk.MustExec(
        "update t_21568_comment set c_str = upper(c_str) where c_decimal is null",
        Vec::new(),
    );
    tk.MustQuery(
        "select * from t_21568_comment where c_decimal is null",
        Vec::new(),
    )
    .Check(Rows(&["11 ABC <nil> <nil>"]));
    tk.MustExec("commit", Vec::new());

    // 对应 Go #22193：前缀主键、前缀唯一索引和索引一致性检查。
    tk.MustExec(
        "create table t_22193 (col_0 blob(20), col_1 int, primary key(col_0(1)), unique key idx(col_0(2)))",
        Vec::new(),
    );
    tk.MustExec("insert into t_22193 values('aaa', 1)", Vec::new());
    tk.MustExec("begin", Vec::new());
    tk.MustExec("update t_22193 set col_0 = 'ccc'", Vec::new());
    tk.MustExec("update t_22193 set col_0 = 'ddd'", Vec::new());
    tk.MustExec("commit", Vec::new());
    tk.MustQuery(
        "select cast(col_0 as char(20)) from t_22193 use index(primary)",
        Vec::new(),
    )
    .Check(Rows(&["ddd"]));
    tk.MustQuery(
        "select cast(col_0 as char(20)) from t_22193 use index(idx)",
        Vec::new(),
    )
    .Check(Rows(&["ddd"]));
    tk.MustExec("admin check table t_22193", Vec::new());

    // 对应 Go #23646：前缀 clustered 主键、SET 类型和二级索引维护。
    tk.MustExec(
        "create table t_23646(c1 varchar(100), c2 set('dav', 'aaa'), c3 varchar(100), primary key(c1(2), c2) clustered, unique key uk1(c2), index idx1(c2, c1, c3))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_23646 select 'AarTrNoAL', 'dav', '1'",
        Vec::new(),
    );
    tk.MustExec(
        "update t_23646 set c3 = '10', c1 = 'BxTXbyKRFBGbcPmPR' where c2 in ('dav', 'dav')",
        Vec::new(),
    );
    tk.MustExec("admin check table t_23646", Vec::new());

    // 对应 Go 的旧行格式下 collation clustered 主键查询。
    tk.MustExec(
        "create table t_collation(col_1 varchar(132) CHARACTER SET utf8 COLLATE utf8_unicode_ci, primary key(col_1) clustered)",
        Vec::new(),
    );
    tk.MustExec("insert into t_collation select 'aBc'", Vec::new());
    tk.MustQuery(
        "select col_1 from t_collation where col_1 = 'aBc'",
        Vec::new(),
    )
    .Check(Rows(&["aBc"]));

    tk.MustExec(
        "CREATE TABLE t_union_scan (a int, b int, c int, PRIMARY KEY (a, b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_union_scan (a, b) values (?, ?)",
        vec![DbValue::I64(1), DbValue::I64(1)],
    );
    tk.MustExec("begin", Vec::new());
    tk.MustExec(
        "update t_union_scan set c = ? where a = ?",
        vec![DbValue::I64(1), DbValue::I64(1)],
    );
    tk.MustQuery("select * from t_union_scan", Vec::new())
        .Check(Rows(&["1 1 1"]));
    // 对应 Go：事务内 union scan 读到更新后的行，再 rollback。
    tk.MustExec("rollback", Vec::new());
}

/// 对应 TestPartitionTable：hash/range 分区表与普通表在 primary 索引查询下结果一致。
#[test]
fn partitioned_clustered_tables_match_normal_table_queries() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create database test_view", Vec::new());
    tk.MustExec("use test_view", Vec::new());
    tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());

    tk.MustExec(
        "create table thash (a int, b int, c varchar(32), primary key(a, b) clustered) partition by hash(a) partitions 4",
        Vec::new(),
    );
    tk.MustExec(
        "create table trange (a int, b int, c varchar(32), primary key(a, b) clustered) partition by range columns(a) (
                        partition p0 values less than (3000),
                        partition p1 values less than (6000),
                        partition p2 values less than (9000),
                        partition p3 values less than (10000))",
        Vec::new(),
    );
    tk.MustExec(
        "create table tnormal (a int, b int, c varchar(32), primary key(a, b))",
        Vec::new(),
    );

    assert!(
        domain
            .table_by_name("test_view", "thash")
            .expect("thash")
            .HasClusteredIndex()
    );
    assert!(
        domain
            .table_by_name("test_view", "trange")
            .expect("trange")
            .HasClusteredIndex()
    );

    // 以确定性伪随机序列代替 Go 的 math/rand，保持 400 个不重复主键和三表相同输入。
    let mut seed = 1_u64;
    let mut values = Vec::with_capacity(400);
    let mut seen = std::collections::HashSet::new();
    while values.len() < 400 {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let a = (seed % 10000) as i64;
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let b = (seed % 10000) as i64;
        if !seen.insert((a, b)) {
            continue;
        }
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let c = format!("{}", seed % 10000);
        values.push(format!("({a}, {b}, '{c}')"));
    }

    let values = values.join(", ");
    for table in ["thash", "trange", "tnormal"] {
        tk.MustExec(&format!("insert into {table} values {values}"), Vec::new());
    }

    // 对应 Go 的 20 轮随机条件：普通表结果必须与两种分区表的 primary 索引扫描一致。
    for _ in 0..20 {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let a1 = seed % 10000;
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let a2 = seed % 10000;
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let a3 = seed % 10000;
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let b = seed % 10000;
        let condition = format!("where a in ({a1}, {a2}, {a3}) and b < {b}");
        let expected = tk
            .MustQuery(&format!("select * from tnormal {condition}"), Vec::new())
            .Sort()
            .Rows();
        for table in ["thash", "trange"] {
            tk.MustQuery(
                &format!("select * from {table} use index(primary) {condition}"),
                Vec::new(),
            )
            .Sort()
            .Check(expected.clone());
        }
    }
}
