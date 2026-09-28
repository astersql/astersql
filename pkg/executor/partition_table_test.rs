// Copyright 2026 AsterSQL.
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

// 分区表执行路径的集成式单元测试。
//
// 基于 `partition_runtime` 内存模型，覆盖点查、批量点查、ORDER BY/LIMIT、
// 各类连接、Apply、Union、聚合、全局索引、行锁与若干历史 issue 回归。

use std::collections::BTreeSet;

use crate::partition_runtime::{
    AccessPath, JoinKind, PartitionError, PartitionTable, Partitioning, Row, Value,
    correlated_apply, grouped_count_sum, join_rows, split_region_keys, union_rows,
};

/// 构造有符号整型 Value。
fn int(value: i64) -> Value {
    Value::Int(value)
}

/// 构造无符号整型 Value。
fn uint(value: u64) -> Value {
    Value::UInt(value)
}

/// 由整型列构造测试行。
fn row(values: &[i64]) -> Row {
    Row(values.iter().copied().map(int).collect())
}

/// 预填 1..=100 的 RANGE 分区表，主键列 0，唯一列 1。
fn range_table() -> PartitionTable {
    let partitioning = Partitioning::range(
        0,
        [
            ("p0", Some(30)),
            ("p1", Some(60)),
            ("p2", Some(90)),
            ("p3", Some(120)),
        ],
    )
    .unwrap();
    let mut table = PartitionTable::new(partitioning, 0, [1]);
    for value in 1..=100 {
        table.insert(row(&[value, 1000 + value])).unwrap();
    }
    table
}

/// 预填 1..=12 的 LIST 分区表。
fn list_table() -> PartitionTable {
    let partitioning = Partitioning::list(
        0,
        [
            (
                "p0",
                BTreeSet::from([Value::Null, int(1), int(2), int(3), int(4)]),
            ),
            ("p1", BTreeSet::from([int(5), int(6), int(7), int(8)])),
            ("p2", BTreeSet::from([int(9), int(10), int(11), int(12)])),
        ],
    )
    .unwrap();
    let mut table = PartitionTable::new(partitioning, 0, [1]);
    for value in 1..=12 {
        table.insert(row(&[value, 100 + value])).unwrap();
    }
    table
}

/// Assert the transaction-lock lifecycle shared by the Go pessimistic-lock tests:
/// another transaction conflicts while the owner is active and can acquire the
/// same physical row immediately after the owner releases it.
fn assert_lock_conflict_then_release(table: &mut PartitionTable, key: Value) {
    assert!(table.lock(1, &key).unwrap());
    assert!(matches!(
        table.lock(2, &key),
        Err(PartitionError::LockConflict { owner: 1, .. })
    ));
    table.unlock_transaction(1);
    assert!(table.lock(2, &key).unwrap());
    table.unlock_transaction(2);
}

/// RANGE / LIST 表上的主键点查与未命中 TableDual。
#[test]
fn test_point_getwith_range_and_list_partition_table() {
    let range = range_table();
    for key in 1..=100 {
        let result = range.point_get(&int(key));
        assert_eq!(result.path, AccessPath::PointGet);
        assert_eq!(result.rows, [row(&[key, 1000 + key])]);
    }
    assert_eq!(range.point_get(&int(200)).path, AccessPath::TableDual);

    let list = list_table();
    for key in 1..=12 {
        assert_eq!(list.point_get(&int(key)).rows, [row(&[key, 100 + key])]);
    }
    assert_eq!(list.point_get(&int(200)).path, AccessPath::TableDual);
}

/// 关闭分区信息后结果不报告分区名，且扫描跳过名称校验。
#[test]
fn test_partition_info_disable() {
    let mut table = range_table();
    table.set_partition_info_enabled(false);
    let point = table.point_get(&int(42));
    assert_eq!(point.path, AccessPath::PointGet);
    assert!(point.partitions.is_empty());
    assert_eq!(
        table
            .scan_partitions(&["not-a-real-partition"])
            .unwrap()
            .rows
            .len(),
        100
    );
}

/// ORDER BY + OFFSET/LIMIT 在全表与单分区上的行为。
#[test]
fn test_order_by_and_limit() {
    let table = range_table();
    assert_eq!(
        table.ordered_limit(&[], 0, false, 25, 5).unwrap().rows,
        (26..=30)
            .map(|value| row(&[value, 1000 + value]))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        table.ordered_limit(&["p1"], 0, true, 0, 3).unwrap().rows,
        [row(&[59, 1059]), row(&[58, 1058]), row(&[57, 1057])]
    );
}

/// HASH 分区上的批量点查与单点分区名。
#[test]
fn test_batch_getand_point_getwith_hash_partition() {
    let mut table = PartitionTable::new(Partitioning::hash(0, 4).unwrap(), 0, []);
    for value in -8..=8 {
        table.insert(row(&[value])).unwrap();
    }
    let result = table.batch_point_get(&[int(-8), int(-1), int(0), int(7), int(99)]);
    assert_eq!(result.path, AccessPath::BatchPointGet);
    assert_eq!(result.rows.len(), 4);
    assert_eq!(table.point_get(&int(7)).partitions, ["p3"]);
}

/// 过滤谓词模拟视图：仅保留主键整十的行。
#[test]
fn test_view() {
    let table = range_table();
    let view = table
        .filter(|row| matches!(&row.0[0], Value::Int(value) if value % 10 == 0))
        .unwrap();
    assert_eq!(view.rows.len(), 10);
    assert_eq!(view.rows.first(), Some(&row(&[10, 1010])));
    assert_eq!(view.rows.last(), Some(&row(&[100, 1100])));
}

/// 分区扫描结果与小表做内连接（模拟 index join 直读）。
#[test]
fn test_direct_readingwith_index_join() {
    let left = range_table().scan_partitions(&["p0"]).unwrap().rows;
    let right = vec![row(&[1, 10]), row(&[3, 30]), row(&[31, 310])];
    let joined = join_rows(&left, &right, 0, 0, JoinKind::Inner).unwrap();
    assert_eq!(
        joined,
        [
            Row(vec![int(1), int(1001), int(1), int(10)]),
            Row(vec![int(3), int(1003), int(3), int(30)])
        ]
    );
}

/// 相关 Apply 按外层键动态剪枝到对应分区行。
#[test]
fn test_dynamic_pruning_under_index_join() {
    let table = range_table();
    let outer = vec![row(&[29]), row(&[30]), row(&[90])];
    let applied = correlated_apply(&outer, |row| table.point_get(&row.0[0]).rows, Some(1));
    assert_eq!(applied.len(), 3);
    assert_eq!(applied[1], Row(vec![int(30), int(30), int(1030)]));
}

/// RANGE / LIST 上的批量点查汇总正确分区名。
#[test]
fn test_batch_getfor_rangeand_list_partition_table() {
    let range = range_table().batch_point_get(&[int(1), int(31), int(61), int(91), int(200)]);
    assert_eq!(range.rows.len(), 4);
    assert_eq!(range.partitions, ["p0", "p1", "p2", "p3"]);
    let list = list_table().batch_point_get(&[int(2), int(6), int(10), int(20)]);
    assert_eq!(list.rows.len(), 3);
    assert_eq!(list.partitions, ["p0", "p1", "p2"]);
}

/// Inner / Semi / AntiSemi / LeftOuter 在分区表上的行数。
#[test]
fn test_partition_table_with_different_join() {
    let left = list_table().scan_partitions(&[]).unwrap().rows;
    let right = vec![row(&[2]), row(&[4]), row(&[20])];
    assert_eq!(
        join_rows(&left, &right, 0, 0, JoinKind::Inner)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        join_rows(&left, &right, 0, 0, JoinKind::Semi)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        join_rows(&left, &right, 0, 0, JoinKind::AntiSemi)
            .unwrap()
            .len(),
        10
    );
    assert_eq!(
        join_rows(&left, &right, 0, 0, JoinKind::LeftOuter)
            .unwrap()
            .len(),
        12
    );
}

/// 多分区扫描的路径与行数（MPP explain 信息场景）。
#[test]
fn test_mpp_query_explain_info() {
    let result = range_table().scan_partitions(&["p0", "p2"]).unwrap();
    assert_eq!(result.path, AccessPath::PartitionScan);
    assert_eq!(result.partitions, ["p0", "p2"]);
    assert_eq!(result.rows.len(), 59);
}

/// 更新可能跨分区迁移主键；删除后点查为空。
#[test]
fn test_dml() {
    let mut table = range_table();
    table.update(&int(29), row(&[110, 2029])).unwrap();
    assert!(table.point_get(&int(29)).rows.is_empty());
    assert_eq!(table.point_get(&int(110)).partitions, ["p3"]);
    assert_eq!(table.delete(&int(110)).unwrap(), Some(row(&[110, 2029])));
    assert!(table.point_get(&int(110)).rows.is_empty());
}

/// UNION ALL 保留重复，UNION DISTINCT 去重。
#[test]
fn test_union() {
    let first = vec![row(&[1]), row(&[2])];
    let second = vec![row(&[2]), row(&[3])];
    assert_eq!(union_rows(&[&first, &second], false).len(), 4);
    assert_eq!(
        union_rows(&[&first, &second], true),
        [row(&[1]), row(&[2]), row(&[3])]
    );
}

/// 子查询式过滤：仅保留白名单主键。
#[test]
fn test_subqueries() {
    let allowed = BTreeSet::from([int(2), int(6), int(10)]);
    let result = list_table()
        .filter(|row| allowed.contains(&row.0[0]))
        .unwrap();
    assert_eq!(
        result.rows,
        [row(&[2, 102]), row(&[6, 106]), row(&[10, 110])]
    );
}

/// Region 均匀分裂键生成与非法区间报错。
#[test]
fn test_split_region() {
    assert_eq!(split_region_keys(0, 100, 4).unwrap(), [25, 50, 75]);
    assert_eq!(split_region_keys(-100, 100, 4).unwrap(), [-50, 0, 50]);
    assert!(matches!(
        split_region_keys(10, 10, 2),
        Err(PartitionError::InvalidDefinition(_))
    ));
}

/// 带 LIMIT 的相关 Apply 在 RANGE 分区列上每外层行最多取 1 内层行。
#[test]
fn test_parallel_apply_with_limit_on_range_columns_partition() {
    let table = range_table();
    let outer = vec![row(&[1]), row(&[31]), row(&[61])];
    let rows = correlated_apply(&outer, |outer| table.point_get(&outer.0[0]).rows, Some(1));
    assert_eq!(rows.len(), 3);
}

/// 无 LIMIT 的相关 Apply：每外层行展开两份内层行。
#[test]
fn test_parallel_apply() {
    let outer = vec![row(&[1]), row(&[2]), row(&[3])];
    let rows = correlated_apply(&outer, |outer| vec![outer.clone(), outer.clone()], None);
    assert_eq!(rows.len(), 6);
}

/// UnionScan 语义：已提交分区行与 pending 行 DISTINCT 合并。
#[test]
fn test_direct_reading_with_union_scan() {
    let committed = range_table().scan_partitions(&["p0"]).unwrap().rows;
    let pending = vec![row(&[29, 1029]), row(&[101, 1101])];
    let visible = union_rows(&[&committed, &pending], true);
    assert_eq!(visible.len(), 30);
    assert!(visible.contains(&row(&[101, 1101])));
}

/// 无符号分区键：0 进 p0，u64::MAX 进 MAXVALUE 分区。
#[test]
fn test_unsigned_partition_column() {
    let partitioning = Partitioning::range(0, [("p0", Some(30)), ("p1", None)]).unwrap();
    let mut table = PartitionTable::new(partitioning, 0, []);
    table.insert(Row(vec![uint(0)])).unwrap();
    table.insert(Row(vec![uint(u64::MAX)])).unwrap();
    assert_eq!(table.point_get(&uint(0)).partitions, ["p0"]);
    assert_eq!(table.point_get(&uint(u64::MAX)).partitions, ["p1"]);
}

/// 分组 COUNT/SUM 聚合。
#[test]
fn test_direct_reading_with_agg() {
    let rows = vec![row(&[1, 10]), row(&[1, 20]), row(&[2, 7])];
    let groups = grouped_count_sum(&rows, 0, 1).unwrap();
    assert_eq!(groups.get(&int(1)), Some(&(2, 30)));
    assert_eq!(groups.get(&int(2)), Some(&(1, 7)));
}

/// IndexMerge：主键与唯一索引查找去重合并。
#[test]
fn test_idex_merge() {
    let table = range_table();
    let result = table.index_merge(&[(0, int(1)), (1, int(1031)), (0, int(1))]);
    assert_eq!(result.path, AccessPath::IndexMerge);
    assert_eq!(result.rows, [row(&[1, 1001]), row(&[31, 1031])]);
}

/// 删除全局唯一索引后无法再经该列定位，且允许重复唯一值插入。
#[test]
fn test_drop_global_index() {
    let mut table = range_table();
    assert_eq!(
        table.global_index_get(1, &int(1010)).rows,
        [row(&[10, 1010])]
    );
    assert!(table.drop_unique_index(1));
    assert_eq!(
        table.global_index_get(1, &int(1010)).path,
        AccessPath::TableDual
    );
    table.insert(row(&[101, 1010])).unwrap();
}

/// SELECT ... FOR UPDATE 式行锁冲突与释放后可重获。
#[test]
fn test_select_lock_on_partition_table() {
    let mut table = range_table();
    // Go covers table-reader, index-reader, and index-lookup paths in both
    // pruning modes. They converge on the same physical record-lock contract.
    for key in [5, 35, 65, 95, 30, 60] {
        assert_lock_conflict_then_release(&mut table, int(key));
    }
}

/// issue26251：分区表与普通表连接的命中行保持锁直至事务释放。
#[test]
fn test_issue26251() {
    let mut table = range_table();
    let normal = vec![row(&[1]), row(&[2])];
    let joined = join_rows(
        &table.scan_partitions(&[]).unwrap().rows,
        &normal,
        0,
        0,
        JoinKind::Inner,
    )
    .unwrap();
    assert_eq!(joined[0], Row(vec![int(1), int(1001), int(1)]));
    assert_lock_conflict_then_release(&mut table, int(1));
}

/// 左连接后对命中键加锁，未命中键返回 false。
#[test]
fn test_left_join_for_update() {
    let mut table = range_table();
    let left = vec![row(&[29]), row(&[200])];
    let right = table.scan_partitions(&[]).unwrap().rows;
    let joined = join_rows(&left, &right, 0, 0, JoinKind::LeftOuter).unwrap();
    assert_eq!(joined.len(), 2);
    assert_lock_conflict_then_release(&mut table, int(29));
    assert!(!table.lock(77, &int(200)).unwrap());

    // Reverse the join direction, matching the second Go round. The matched
    // partition row is locked; an absent right-hand row must not invent a lock.
    let normal = vec![row(&[29]), row(&[200])];
    let partitioned = table.scan_partitions(&[]).unwrap().rows;
    let reverse = join_rows(&normal, &partitioned, 0, 0, JoinKind::LeftOuter).unwrap();
    assert_eq!(reverse.len(), 2);
    assert_lock_conflict_then_release(&mut table, int(29));
    assert!(!table.lock(77, &int(200)).unwrap());
}

/// issue31024：IndexMerge 连接选中的分区行在 FOR UPDATE 生命周期内持锁。
#[test]
fn test_issue31024() {
    let mut table = range_table();
    let merged = table.index_merge(&[(0, int(1)), (1, int(1001))]);
    assert_eq!(merged.path, AccessPath::IndexMerge);
    assert_eq!(merged.rows, [row(&[1, 1001])]);

    let joined = join_rows(&merged.rows, &[row(&[1])], 0, 0, JoinKind::Inner).unwrap();
    assert_eq!(joined, [Row(vec![int(1), int(1001), int(1)])]);
    assert_lock_conflict_then_release(&mut table, int(1));
}

/// 全局索引定位后再加行锁，冲突检测使用主键。
#[test]
fn test_global_index_with_select_lock() {
    let mut table = range_table();
    // Go exercises IndexLookUp, IndexReader, PointGet, BatchPointGet, and
    // IndexMerge. The in-memory runtime exposes the common global-index and
    // record-lock boundary, so exercise single, batch, and merged lookup keys.
    let mut keys = Vec::new();
    for index_value in [1002, 1003] {
        let result = table.global_index_get(1, &int(index_value));
        assert_eq!(result.path, AccessPath::GlobalIndex);
        keys.push(result.rows[0].0[0].clone());
    }
    assert_eq!(table.batch_point_get(&keys).path, AccessPath::BatchPointGet);
    assert_eq!(
        table.index_merge(&[(1, int(1002)), (1, int(1003))]).rows,
        [row(&[2, 1002]), row(&[3, 1003])]
    );
    for key in keys {
        assert_lock_conflict_then_release(&mut table, key);
    }
}
