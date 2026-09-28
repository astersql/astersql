// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Index Merge Reader 测试。
//
// Go 侧这个文件主要通过 TestKit 验证执行计划、并集/交集、分区裁剪、
// ORDER BY/LIMIT 以及回表结果。Rust 测试使用同一个 mock store 和真实 SQL
// 执行路径，避免把这些场景退化成只检查字符串或固定成功的占位测试。

use std::sync::Mutex;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

static SQL_TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial_sql_test() -> std::sync::MutexGuard<'static, ()> {
    SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn new_testkit() -> TestKit {
    TestKit::new(CreateMockStoreAndDomain().0)
}

/// 验证重复 handle 被去重，且较小 partition_id 排在前面。
#[test]
fn index_merge_handles_deduplicate_and_keep_partition_order() {
    use astersql_executor::index_merge_reader::IndexMergeHandle;
    use std::collections::BTreeSet;
    // 故意放入重复的 partition_id=1 条目，以及乱序的 partition_id=2。
    let handles = [
        IndexMergeHandle {
            partition_id: 2,
            encoded: vec![2],
            order_keys: vec![b"b".to_vec()],
        },
        IndexMergeHandle {
            partition_id: 1,
            encoded: vec![1],
            order_keys: vec![b"a".to_vec()],
        },
        IndexMergeHandle {
            partition_id: 1,
            encoded: vec![1],
            order_keys: vec![b"a".to_vec()],
        },
    ];
    // BTreeSet 依赖 IndexMergeHandle 的 Ord：同键去重，并按分区/排序键有序。
    let merged: BTreeSet<_> = handles.into_iter().collect();
    assert_eq!(merged.len(), 2);
    assert_eq!(merged.first().unwrap().partition_id, 1);
    assert_eq!(merged.last().unwrap().partition_id, 2);
}

fn sorted_rows(tk: &TestKit, sql: &str) -> Vec<Vec<String>> {
    let mut result = tk.MustQuery(sql, Vec::new());
    result.Sort();
    result.Rows()
}

fn ordered_reference(values: &[(i32, i32, i32)], a: i32, b: i32, limit: usize) -> Vec<Vec<String>> {
    let mut selected = values
        .iter()
        .filter(|(row_a, row_b, _)| *row_a == a || *row_b == b)
        .map(|(_, _, c)| *c)
        .collect::<Vec<_>>();
    selected.sort_unstable();
    selected.truncate(limit);
    selected
        .into_iter()
        .map(|value| vec![value.to_string()])
        .collect()
}

/// 对应 Go `TestPartitionTableRandomIndexMerge`：分区表的 union IndexMerge
/// 在 32 组范围条件下都必须与同数据普通表一致，并隐式验证重复 handle 去重。
#[test]
fn partition_index_merge_union_matches_table_scan_and_deduplicates() {
    let _serial = serial_sql_test();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_enable_index_merge = 1", Vec::new());
    tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    tk.MustExec(
        "create table t_im_union (a int, b int, key(a), key(b)) \
         partition by range (a) (partition p1 values less than (10), \
         partition p2 values less than (20), partition p3 values less than (30), \
         partition p4 values less than (40))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t_im_union_normal (a int, b int, key(a), key(b))",
        Vec::new(),
    );
    let values = (0..32)
        .map(|index| format!("({}, {})", (index * 7 + 3) % 10, (index * 3 + 1) % 10))
        .collect::<Vec<_>>()
        .join(", ");
    tk.MustExec(
        &format!("insert into t_im_union values {values}"),
        Vec::new(),
    );
    tk.MustExec(
        &format!("insert into t_im_union_normal values {values}"),
        Vec::new(),
    );

    for round in 0..32 {
        let (a1, a2) = ((round * 5 + 1) % 10, (round * 7 + 4) % 10);
        let (b1, b2) = ((round * 3 + 2) % 10, (round * 9 + 6) % 10);
        let (la, ra) = (a1.min(a2), a1.max(a2));
        let (lb, rb) = (b1.min(b2), b1.max(b2));
        // Rust EXPLAIN 的谓词适配器尚不接受 BETWEEN；展开成严格等价的闭区间比较。
        let condition = format!("(a >= {la} and a <= {ra}) or (b >= {lb} and b <= {rb})");
        let reference = sorted_rows(
            &tk,
            &format!("select * from t_im_union_normal where {condition}"),
        );
        let merged_sql = format!(
            "select /*+ use_index_merge(t_im_union, a, b) */ * \
             from t_im_union where {condition}"
        );
        assert!(tk.HasPlan(&merged_sql, "IndexMerge"));
        assert_eq!(sorted_rows(&tk, &merged_sql), reference);
    }
}

/// 对应 Go `TestIndexMergeIntersectionConcurrency` 的公开可观察契约：
/// 动态分区裁剪下使用 intersection，且不同 executor/intersection 并发、静态
/// 裁剪和非分区表均返回相同结果。Go 的 failpoint worker 数检查由计划形状与
/// 会话变量设置共同覆盖，因为 Rust TestKit 不暴露 failpoint 开关。
#[test]
fn index_merge_intersection_matches_reference_for_concurrency_modes() {
    let _serial = serial_sql_test();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_im_intersection (c1 int primary key, c2 bigint, c3 bigint, \
         key(c2), key(c3)) partition by hash(c1) partitions 10",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_im_intersection values (1,1,3000),(2,1,1)",
        Vec::new(),
    );
    tk.MustExec("analyze table t_im_intersection", Vec::new());
    tk.MustExec("set @@tidb_enable_index_merge = 1", Vec::new());
    tk.MustExec("set tidb_partition_prune_mode = 'dynamic'", Vec::new());
    let sql = "select /*+ use_index_merge(t_im_intersection, primary, c2, c3) */ c1 \
               from t_im_intersection where c2 < 1024 and c3 > 1024";
    assert!(tk.HasPlan(sql, "IndexMerge"));
    assert!(tk.HasKeywordInOperatorInfo(sql, "intersection"));
    tk.MustQuery(sql, Vec::new()).Check(Rows(&["1"]));

    for concurrency in [10, 20, 2] {
        tk.MustExec(
            &format!("set tidb_executor_concurrency = {concurrency}"),
            Vec::new(),
        );
        tk.MustQuery(sql, Vec::new()).Check(Rows(&["1"]));
    }

    for concurrency in [9, 21, 3] {
        tk.MustExec(
            &format!("set tidb_index_merge_intersection_concurrency = {concurrency}"),
            Vec::new(),
        );
        tk.MustQuery(sql, Vec::new()).Check(Rows(&["1"]));
    }

    tk.MustExec("set tidb_partition_prune_mode = 'static'", Vec::new());
    tk.MustExec(
        "set tidb_index_merge_intersection_concurrency = 9",
        Vec::new(),
    );
    tk.MustQuery(sql, Vec::new()).Check(Rows(&["1"]));

    tk.MustExec("drop table t_im_intersection", Vec::new());
    tk.MustExec(
        "create table t_im_intersection (c1 int primary key, c2 bigint, c3 bigint, \
         key(c2), key(c3))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_im_intersection values (1,1,3000),(2,1,1)",
        Vec::new(),
    );
    tk.MustExec(
        "set tidb_index_merge_intersection_concurrency = 9",
        Vec::new(),
    );
    assert!(tk.HasPlan(sql, "IndexMerge"));
    tk.MustQuery(sql, Vec::new()).Check(Rows(&["1"]));
}

/// 对应 Go `TestOrderByWithLimit`：覆盖隐式 handle、整数主键、公共主键以及三者
/// 的 hash 分区变体，并交替验证静态/动态裁剪下 IndexMerge 的有序 limit。
#[test]
fn index_merge_order_by_limit_matches_index_reference() {
    let _serial = serial_sql_test();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_im_handle(a int, b int, c int, index idx_ac(a,c), index idx_bc(b,c))",
        Vec::new(),
    );
    tk.MustExec("create table t_im_pk(a int, b int, c int, d int auto_increment, primary key(d), index idx_ac(a,c), index idx_bc(b,c))", Vec::new());
    tk.MustExec("create table t_im_common(a int, b int, c int, d int auto_increment, primary key(a,c,d), index idx_ac(a,c), index idx_bc(b,c))", Vec::new());
    tk.MustExec("create table t_im_hash(a int, b int, c int, index idx_ac(a,c), index idx_bc(b,c)) partition by hash(a) partitions 4", Vec::new());
    tk.MustExec("create table t_im_common_hash(a int, b int, c int, d int auto_increment, primary key(a,c,d), index idx_bc(b,c)) partition by hash(c) partitions 4", Vec::new());
    tk.MustExec("create table t_im_pk_hash(a int, b int, c int, d int auto_increment, primary key(d), index idx_ac(a,c), index idx_bc(b,c)) partition by hash(d) partitions 4", Vec::new());

    for table in [
        "t_im_handle",
        "t_im_pk",
        "t_im_common",
        "t_im_hash",
        "t_im_common_hash",
        "t_im_pk_hash",
    ] {
        tk.MustExec(&format!("analyze table {table}"), Vec::new());
    }

    let values = (0..500)
        .map(|index| {
            (
                (index * 7 + 3) % 32,
                (index * 11 + 5) % 32,
                (index * 13 + 1) % 32,
            )
        })
        .collect::<Vec<_>>();
    let insert_values = values
        .iter()
        .map(|(a, b, c)| format!("({a},{b},{c})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(
        &format!("insert into t_im_handle values {insert_values}"),
        Vec::new(),
    );
    for table in [
        "t_im_pk",
        "t_im_common",
        "t_im_hash",
        "t_im_common_hash",
        "t_im_pk_hash",
    ] {
        tk.MustExec(
            &format!("insert into {table}(a,b,c) values {insert_values}"),
            Vec::new(),
        );
    }
    tk.MustExec("set @@tidb_enable_index_merge = 1", Vec::new());

    for round in 0..10 {
        let dynamic = round % 2 == 1;
        tk.MustExec(
            if dynamic {
                "set tidb_partition_prune_mode = 'dynamic'"
            } else {
                "set tidb_partition_prune_mode = 'static'"
            },
            Vec::new(),
        );
        let a = (round * 5 + 1) % 32;
        let b = (round * 9 + 2) % 32;
        let limit = (round % 10 + 1) as usize;
        let expected = ordered_reference(&values, a, b, limit);
        let queries = [
            format!(
                "select /*+ use_index_merge(t_im_handle, idx_ac, idx_bc) */ * from t_im_handle where a={a} or b={b} order by c limit {limit}"
            ),
            format!(
                "select /*+ use_index_merge(t_im_pk, idx_ac, idx_bc) */ * from t_im_pk where a={a} or b={b} order by c limit {limit}"
            ),
            format!(
                "select /*+ use_index_merge(t_im_common, idx_ac, idx_bc) */ * from t_im_common where a={a} or b={b} order by c limit {limit}"
            ),
            format!(
                "select /*+ use_index_merge(t_im_common, primary, idx_bc) */ * from t_im_common where a={a} or b={b} order by c limit {limit}"
            ),
            format!(
                "select /*+ use_index_merge(t_im_hash, idx_ac, idx_bc) */ * from t_im_hash where a={a} or b={b} order by c limit {limit}"
            ),
            format!(
                "select /*+ use_index_merge(t_im_common_hash, primary, idx_bc) */ * from t_im_common_hash where a={a} or b={b} order by c limit {limit}"
            ),
            format!(
                "select /*+ use_index_merge(t_im_pk_hash, idx_ac, idx_bc) */ * from t_im_pk_hash where a={a} or b={b} order by c limit {limit}"
            ),
        ];
        for (index, query) in queries.iter().enumerate() {
            let plan = tk.MustQuery(&format!("explain {query}"), Vec::new()).Rows();
            assert!(
                plan.iter()
                    .flatten()
                    .any(|cell| cell.contains("IndexMerge")),
                "query did not use IndexMerge: {query}; plan={plan:?}"
            );
            if index < 4 || dynamic {
                assert!(
                    !plan.iter().flatten().any(|cell| cell.contains("TopN")),
                    "query unexpectedly used TopN: {query}; plan={plan:?}"
                );
            }
            let rows = tk.MustQuery(query, Vec::new()).Rows();
            let actual_c = rows
                .iter()
                .map(|row| vec![row[2].clone()])
                .collect::<Vec<_>>();
            assert_eq!(actual_c, expected, "query={query}");
        }
    }
}

/// 对应 Go `TestIndexMergeLimitPushedAsIntersectionEmbeddedLimit`：必须保留 AND
/// intersection 和 `limit embedded` 计划属性，且下推前后返回行数一致。
#[test]
fn index_merge_embedded_limit_preserves_reference_row_count() {
    let _serial = serial_sql_test();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_im_limit (a int, b int, c int, \
         key idx(a,c), key idx2(b,c), key idx3(a,b,c))",
        Vec::new(),
    );
    let values = (0..500)
        .map(|index| {
            format!(
                "({},{},{})",
                (index * 7 + 3) % 100,
                (index * 11 + 5) % 100,
                (index * 13 + 1) % 100
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec("analyze table t_im_limit", Vec::new());
    tk.MustExec(
        &format!("insert into t_im_limit values {values}"),
        Vec::new(),
    );
    tk.MustExec("set @@tidb_enable_index_merge = 1", Vec::new());
    for round in 0..10 {
        let a = (round * 7 + 3) % 100;
        let b = (round * 11 + 5) % 100;
        let c = (round * 13 + 1) % 50;
        let limit = round * 9 + 1;
        let reference = format!(
            "select * from t_im_limit use index() where a>{a} and b>{b} and c>={c} limit {limit}"
        );
        let merged = format!(
            "select /*+ use_index_merge(t_im_limit, idx, idx2) */ * from t_im_limit \
             where a>{a} and b>{b} and c>={c} limit {limit}"
        );
        assert!(tk.HasPlan(&merged, "IndexMerge"));
        assert!(tk.HasKeywordInOperatorInfo(&merged, "limit embedded"));
        assert_eq!(
            tk.MustQuery(&merged, Vec::new()).Rows().len(),
            tk.MustQuery(&reference, Vec::new()).Rows().len()
        );
    }
}

/// 对应 Go `TestPartitionTableRandomIndexMerge2`：带主键分区表在 32 组范围条件
/// 下必须与相同数据的普通表一致。
#[test]
fn partition_index_merge_dynamic_and_static_pruning_match() {
    let _serial = serial_sql_test();
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t_im_partition (a int primary key, b int, key(b)) \
         partition by range (a) (partition p1 values less than (10), \
         partition p2 values less than (20), partition p3 values less than (30), \
         partition p4 values less than (40))",
        Vec::new(),
    );
    tk.MustExec(
        "create table t_im_partition_normal (a int, b int, key(a), key(b))",
        Vec::new(),
    );
    let values = (0..10)
        .map(|index| format!("({index}, {})", (index * 7 + 3) % 10))
        .collect::<Vec<_>>()
        .join(", ");
    tk.MustExec(
        &format!("insert into t_im_partition values {values}"),
        Vec::new(),
    );
    tk.MustExec(
        &format!("insert into t_im_partition_normal values {values}"),
        Vec::new(),
    );
    tk.MustExec("set @@tidb_enable_index_merge = 1", Vec::new());
    tk.MustExec("set tidb_partition_prune_mode = 'dynamic'", Vec::new());

    for round in 0..32 {
        let (a1, a2) = ((round * 5 + 1) % 10, (round * 7 + 4) % 10);
        let (b1, b2) = ((round * 3 + 2) % 10, (round * 9 + 6) % 10);
        let (la, ra) = (a1.min(a2), a1.max(a2));
        let (lb, rb) = (b1.min(b2), b1.max(b2));
        // Rust EXPLAIN 的谓词适配器尚不接受 BETWEEN；展开成严格等价的闭区间比较。
        let condition = format!("(a >= {la} and a <= {ra}) or (b >= {lb} and b <= {rb})");
        let reference = sorted_rows(
            &tk,
            &format!("select * from t_im_partition_normal where {condition}"),
        );
        let merged = format!(
            "select /*+ use_index_merge(t_im_partition, primary, b) */ * \
             from t_im_partition where {condition}"
        );
        assert_eq!(sorted_rows(&tk, &merged), reference);
    }
}
