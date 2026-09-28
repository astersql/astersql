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

//! 多值索引在真实 TestKit/MockStore 上的回归测试，对应
//! `multi_valued_index_test.go`。
//!
//! 测试先通过常规 SQL DDL/DML 驱动生产写入路径，再从真实 KV 快照扫描物理索引键区间，
//! 以覆盖数组展开、唯一性、分区路由、更新/删除维护及键编码，而不在测试中另造简化存储。

use astersql_tablecodec::{
    DecodeKeyHead, EncodeTableIndexPrefix, codec, idLen, kv, prefixLen, types,
};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

use crate::main_test::{ALLOWS_EXPRESSION_INDEX, ensure_test_env};

fn null_datum() -> types::Datum {
    types::Datum::default()
}

fn int_datum(value: i64) -> types::Datum {
    types::NewIntDatum(value)
}

fn uint_datum(value: u64) -> types::Datum {
    types::NewUintDatum(value)
}

fn bytes_datum(value: &[u8]) -> types::Datum {
    types::NewBytesDatum(value.to_vec())
}

/// 去掉表与索引前缀，按编码顺序还原物理索引键中的所有 Datum。
///
/// 非唯一索引的键尾还包含行句柄；本文件唯一索引用例的非 NULL 项则只在键内编码索引值，
/// 因此调用方会按索引类型提供不同长度的期望值。
fn decode_index_key(key: kv::Key) -> Vec<types::Datum> {
    let (_, _, is_record) = DecodeKeyHead(key.clone()).expect("decode index key head");
    assert!(
        !is_record,
        "multi-valued index key must not be a record key"
    );
    let mut encoded = key.0[prefixLen + idLen..].to_vec();
    let mut values = Vec::new();
    while !encoded.is_empty() {
        let (remaining, value) = codec::DecodeOne(&encoded).expect("decode index datum");
        values.push(value);
        encoded = remaining.to_vec();
    }
    values
}

/// 在保留 Datum 类型语义的前提下比较解码结果。
///
/// 有符号数、无符号数及字节串必须分别比较，避免仅靠字符串展示掩盖编码类型差异。
fn datum_equal(left: &types::Datum, right: &types::Datum) -> bool {
    if left.IsNull() || right.IsNull() {
        return left.IsNull() && right.IsNull();
    }
    if left.Kind() != right.Kind() {
        return false;
    }
    match left.Kind() {
        types::KindInt64 => left.GetInt64() == right.GetInt64(),
        types::KindUint64 => left.GetUint64() == right.GetUint64(),
        types::KindBytes | types::KindString => left.GetBytes() == right.GetBytes(),
        _ => left.ToString().unwrap_or_default() == right.ToString().unwrap_or_default(),
    }
}

/// 按物理键迭代顺序逐项校验索引 Datum；顺序本身也是键编码正确性的一部分。
fn assert_index_keys(actual: &[Vec<types::Datum>], expected: &[Vec<types::Datum>]) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "multi-valued index entry count"
    );
    for (position, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            actual.len(),
            expected.len(),
            "datum count at index entry {position}"
        );
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                datum_equal(actual, expected),
                "index entry {position}: actual={} ({:?}), expected={} ({:?})",
                actual.ToString().unwrap_or_default(),
                actual.Kind(),
                expected.ToString().unwrap_or_default(),
                expected.Kind()
            );
        }
    }
}

// 用表或分区的物理 ID 限定目标索引前缀，并在同一 KV 快照中解码完整键区间。
macro_rules! scan_multi_valued_index {
    ($domain:expr, $table:expr, $physical_id:expr) => {{
        let index = $table
            .Indices
            .iter()
            .find(|index| index.MVIndex)
            .expect("multi-valued index metadata");
        let start = EncodeTableIndexPrefix($physical_id, index.ID);
        let end = start.PrefixNext();
        $domain.storage().with_storage(|storage| {
            let version = storage.CurrentVersion("global").expect("current version");
            let snapshot = storage.GetSnapshot(version);
            let mut iterator = snapshot
                .Iter(start, Some(end))
                .expect("scan multi-valued index");
            let mut keys = Vec::new();
            while iterator.Valid() {
                keys.push(decode_index_key(iterator.Key()));
                iterator
                    .Next()
                    .expect("advance multi-valued index iterator");
            }
            iterator.Close();
            keys
        })
    }};
}

// 每个用例使用独立 MockStore，并统一切换到 test 数据库。
macro_rules! new_testkit {
    () => {{
        let (store, domain) = CreateMockStoreAndDomain();
        let mut testkit = TestKit::new(store);
        testkit.MustExec("use test", Vec::new());
        (testkit, domain)
    }};
}

// 覆盖有符号整数、定长字符和无符号整数数组索引的写入、更新与删除维护。
// 同一 JSON 数组内的重复元素只生成一条索引项；NULL 生成 NULL 项，空数组不生成索引项，
// 标量则按单元素处理。
#[test]
fn test_write_multi_valued_index() {
    ensure_test_env();
    assert!(ALLOWS_EXPRESSION_INDEX);

    let (mut testkit, domain) = new_testkit!();
    testkit.MustExec(
        "create table t1(pk int primary key, a json, index idx((cast(a as signed array))))",
        Vec::new(),
    );
    for statement in [
        "insert into t1 values (1, '[1,2,2,3]')",
        "insert into t1 values (2, '[1,2,3]')",
        "insert into t1 values (3, '[]')",
        "insert into t1 values (4, '[2,3,4]')",
        "insert into t1 values (5, null)",
        "insert into t1 values (6, '1')",
    ] {
        testkit.MustExec(statement, Vec::new());
    }
    let table = domain.table_by_name("test", "t1").expect("t1 metadata");
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, table.ID),
        &[
            vec![null_datum(), int_datum(5)],
            vec![int_datum(1), int_datum(1)],
            vec![int_datum(1), int_datum(2)],
            vec![int_datum(1), int_datum(6)],
            vec![int_datum(2), int_datum(1)],
            vec![int_datum(2), int_datum(2)],
            vec![int_datum(2), int_datum(4)],
            vec![int_datum(3), int_datum(1)],
            vec![int_datum(3), int_datum(2)],
            vec![int_datum(3), int_datum(4)],
            vec![int_datum(4), int_datum(4)],
        ],
    );
    testkit.MustExec("delete from t1", Vec::new());
    assert_index_keys(&scan_multi_valued_index!(domain, table, table.ID), &[]);

    // 字符串数组的物理键保留尾随空格，更新后旧索引项必须被精确移除并写入新项。
    testkit.MustExec("drop table t1", Vec::new());
    testkit.MustExec(
        "create table t1(pk int primary key, a json, index idx((cast(a as char(5) array))))",
        Vec::new(),
    );
    for statement in [
        r#"insert into t1 values (1, '["abc", "abc "]')"#,
        r#"insert into t1 values (2, '["b"]')"#,
        r#"insert into t1 values (3, '["b   "]')"#,
    ] {
        testkit.MustExec(statement, Vec::new());
    }
    testkit
        .MustQuery("select pk from t1 where 'b   ' member of (a)", Vec::new())
        .Check(Rows(&["3"]));
    let table = domain.table_by_name("test", "t1").expect("recreated t1");
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, table.ID),
        &[
            vec![bytes_datum(b"abc"), int_datum(1)],
            vec![bytes_datum(b"abc "), int_datum(1)],
            vec![bytes_datum(b"b"), int_datum(2)],
            vec![bytes_datum(b"b   "), int_datum(3)],
        ],
    );
    for statement in [
        "update t1 set a = json_array_append(a, '$', 'bcd') where pk = 1",
        "update t1 set a = '[]' where pk = 2",
        r#"update t1 set a = '["abc"]' where pk = 3"#,
    ] {
        testkit.MustExec(statement, Vec::new());
    }
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, table.ID),
        &[
            vec![bytes_datum(b"abc"), int_datum(1)],
            vec![bytes_datum(b"abc"), int_datum(3)],
            vec![bytes_datum(b"abc "), int_datum(1)],
            vec![bytes_datum(b"bcd"), int_datum(1)],
        ],
    );
    testkit.MustExec("delete from t1", Vec::new());
    assert_index_keys(&scan_multi_valued_index!(domain, table, table.ID), &[]);

    // 无符号数组必须解码为 Uint Datum，不能退化为数值相同的有符号类型。
    testkit.MustExec("drop table t1", Vec::new());
    testkit.MustExec(
        "create table t1(pk int primary key, a json, index idx((cast(a as unsigned array))))",
        Vec::new(),
    );
    for statement in [
        "insert into t1 values (1, '[1,2,2,3]')",
        "insert into t1 values (2, '[1,2,3]')",
        "insert into t1 values (3, '[]')",
        "insert into t1 values (4, '[2,3,4]')",
        "insert into t1 values (5, null)",
        "insert into t1 values (6, '1')",
    ] {
        testkit.MustExec(statement, Vec::new());
    }
    let table = domain.table_by_name("test", "t1").expect("unsigned t1");
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, table.ID),
        &[
            vec![null_datum(), int_datum(5)],
            vec![uint_datum(1), int_datum(1)],
            vec![uint_datum(1), int_datum(2)],
            vec![uint_datum(1), int_datum(6)],
            vec![uint_datum(2), int_datum(1)],
            vec![uint_datum(2), int_datum(2)],
            vec![uint_datum(2), int_datum(4)],
            vec![uint_datum(3), int_datum(1)],
            vec![uint_datum(3), int_datum(2)],
            vec![uint_datum(3), int_datum(4)],
            vec![uint_datum(4), int_datum(4)],
        ],
    );
    testkit.MustExec("delete from t1", Vec::new());
    assert_index_keys(&scan_multi_valued_index!(domain, table, table.ID), &[]);
}

// 分区索引以分区物理 ID 隔离，分别扫描 p0、p1 可验证路由结果和删除后的清理。
#[test]
fn test_write_multi_valued_index_partition_table() {
    ensure_test_env();
    let (mut testkit, domain) = new_testkit!();
    testkit.MustExec(
        "create table t1(pk int primary key, a json, index idx((cast(a as signed array)))) \
         partition by range columns (pk) \
         (partition p0 values less than (10), partition p1 values less than (20))",
        Vec::new(),
    );
    for statement in [
        "insert into t1 values (1, '[1,2,2,3]')",
        "insert into t1 values (11, '[1,2,3]')",
        "insert into t1 values (2, '[]')",
        "insert into t1 values (12, '[2,3,4]')",
        "insert into t1 values (3, null)",
        "insert into t1 values (13, null)",
    ] {
        testkit.MustExec(statement, Vec::new());
    }
    let table = domain.table_by_name("test", "t1").expect("partitioned t1");
    let partition = table.GetPartitionInfo().expect("partition metadata");
    let p0 = partition.Definitions[0].ID;
    let p1 = partition.Definitions[1].ID;
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, p0),
        &[
            vec![null_datum(), int_datum(3)],
            vec![int_datum(1), int_datum(1)],
            vec![int_datum(2), int_datum(1)],
            vec![int_datum(3), int_datum(1)],
        ],
    );
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, p1),
        &[
            vec![null_datum(), int_datum(13)],
            vec![int_datum(1), int_datum(11)],
            vec![int_datum(2), int_datum(11)],
            vec![int_datum(2), int_datum(12)],
            vec![int_datum(3), int_datum(11)],
            vec![int_datum(3), int_datum(12)],
            vec![int_datum(4), int_datum(12)],
        ],
    );
    testkit.MustExec("delete from t1", Vec::new());
    assert_index_keys(&scan_multi_valued_index!(domain, table, p0), &[]);
    assert_index_keys(&scan_multi_valued_index!(domain, table, p1), &[]);
}

// 唯一多值索引允许单行数组内元素重复，但不同记录展开出相同元素时必须报重复键。
#[test]
fn test_write_multi_valued_index_unique() {
    ensure_test_env();
    let (mut testkit, domain) = new_testkit!();
    testkit.MustExec(
        "create table t1(pk int primary key, a json, unique index idx((cast(a as signed array))))",
        Vec::new(),
    );
    testkit.MustExec("insert into t1 values (1, '[1,2,2]')", Vec::new());
    testkit.MustContainErrMsg("insert into t1 values (2, '[1]')", "[kv:1062]");
    testkit.MustExec("insert into t1 values (3, '[3,3,4]')", Vec::new());
    let table = domain.table_by_name("test", "t1").expect("unique t1");
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, table.ID),
        &[
            vec![int_datum(1)],
            vec![int_datum(2)],
            vec![int_datum(3)],
            vec![int_datum(4)],
        ],
    );
}

// 复合索引键保持“前置列、数组元素、后置列、行句柄”的顺序；NULL 参与编码，空数组
// 不产生任何组合项。
#[test]
fn test_write_multi_valued_index_composite() {
    ensure_test_env();
    let (mut testkit, domain) = new_testkit!();
    testkit.MustExec(
        "create table t1(pk int primary key, a json, c int, d int, \
         index idx(c, (cast(a as signed array)), d))",
        Vec::new(),
    );
    for statement in [
        "insert into t1 values (1, '[1,2,2]', 1, 1)",
        "insert into t1 values (2, '[2,2,2]', 2, 2)",
        "insert into t1 values (3, '[3,3,4]', 3, 3)",
        "insert into t1 values (4, null, 4, 4)",
        "insert into t1 values (5, '[]', 5, 5)",
    ] {
        testkit.MustExec(statement, Vec::new());
    }
    let table = domain.table_by_name("test", "t1").expect("composite t1");
    assert_index_keys(
        &scan_multi_valued_index!(domain, table, table.ID),
        &[
            vec![int_datum(1), int_datum(1), int_datum(1), int_datum(1)],
            vec![int_datum(1), int_datum(2), int_datum(1), int_datum(1)],
            vec![int_datum(2), int_datum(2), int_datum(2), int_datum(2)],
            vec![int_datum(3), int_datum(3), int_datum(3), int_datum(3)],
            vec![int_datum(3), int_datum(4), int_datum(3), int_datum(3)],
            vec![int_datum(4), null_datum(), int_datum(4), int_datum(4)],
        ],
    );
}
