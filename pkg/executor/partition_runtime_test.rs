// Copyright 2026 AsterSQL.

// 分区路由与分区表运行时语义的单元测试。
//
// 覆盖 RANGE / LIST / HASH 分区键落点（含 NULL 与边界）、
// 点查 / 批量点查 / 全局索引、DML 跨分区更新、以及行锁冲突。

use std::collections::BTreeSet;

use crate::partition_runtime::{
    AccessPath, PartitionError, PartitionTable, Partitioning, Row, Value,
};

/// 由整型列构造测试行。
fn row(values: &[i64]) -> Row {
    Row(values.iter().copied().map(Value::Int).collect())
}

/// RANGE / LIST / HASH 路由应符合 TiDB 边界语义（上界开区间、NULL 进首分区等）。
#[test]
fn range_list_and_hash_route_with_tidb_boundary_semantics() {
    let range =
        Partitioning::range(0, [("p0", Some(30)), ("p1", Some(60)), ("pmax", None)]).unwrap();
    // RANGE 上界为开区间：29 → p0，30 → p1。
    assert_eq!(range.partition_for(&row(&[29])).unwrap(), 0);
    assert_eq!(range.partition_for(&row(&[30])).unwrap(), 1);
    assert_eq!(range.partition_for(&row(&[600])).unwrap(), 2);
    // NULL 落在第一个 RANGE 分区。
    assert_eq!(range.partition_for(&Row(vec![Value::Null])).unwrap(), 0);

    let list = Partitioning::list(
        0,
        [
            ("p0", BTreeSet::from([Value::Null, Value::Int(1)])),
            ("p1", BTreeSet::from([Value::Int(2), Value::Int(3)])),
        ],
    )
    .unwrap();
    assert_eq!(list.partition_for(&Row(vec![Value::Null])).unwrap(), 0);
    assert_eq!(list.partition_for(&row(&[3])).unwrap(), 1);
    assert_eq!(
        list.partition_for(&row(&[4])),
        Err(PartitionError::NoPartition)
    );

    let hash = Partitioning::hash(0, 4).unwrap();
    // HASH 使用绝对值取模。
    assert_eq!(hash.partition_for(&row(&[-5])).unwrap(), 1);
    assert_eq!(hash.partition_for(&row(&[6])).unwrap(), 2);
}

/// 点查、批量点查、全局索引与行锁均跟随物理分区。
#[test]
fn point_batch_dml_global_index_and_lock_follow_physical_partition() {
    let partitioning =
        Partitioning::range(0, [("p0", Some(10)), ("p1", Some(20)), ("pmax", None)]).unwrap();
    let mut table = PartitionTable::new(partitioning, 0, [1]);
    table.insert(row(&[1, 101])).unwrap();
    table.insert(row(&[11, 111])).unwrap();
    table.insert(row(&[21, 121])).unwrap();

    let point = table.point_get(&Value::Int(11));
    assert_eq!(point.path, AccessPath::PointGet);
    assert_eq!(point.partitions, ["p1"]);
    assert_eq!(point.rows, [row(&[11, 111])]);
    // 键不存在时退化为 TableDual（空结果）。
    assert_eq!(
        table.point_get(&Value::Int(999)).path,
        AccessPath::TableDual
    );

    let batch = table.batch_point_get(&[Value::Int(21), Value::Int(1)]);
    assert_eq!(batch.path, AccessPath::BatchPointGet);
    assert_eq!(batch.partitions, ["p0", "pmax"]);
    assert_eq!(batch.rows, [row(&[21, 121]), row(&[1, 101])]);

    let global = table.global_index_get(1, &Value::Int(111));
    assert_eq!(global.path, AccessPath::GlobalIndex);
    assert_eq!(global.partitions, ["p1"]);

    // 更新可能改变分区归属：11→12 从 p1 迁到仍属 p1 的边界内新键。
    table.update(&Value::Int(11), row(&[12, 112])).unwrap();
    assert!(table.point_get(&Value::Int(11)).rows.is_empty());
    assert_eq!(table.point_get(&Value::Int(12)).rows, [row(&[12, 112])]);
    assert_eq!(
        table.insert(row(&[13, 112])),
        Err(PartitionError::DuplicateKey(Value::Int(112)))
    );

    // 行锁：同一键被其他事务占用时返回 LockConflict。
    assert!(table.lock(7, &Value::Int(12)).unwrap());
    assert!(matches!(
        table.lock(8, &Value::Int(12)),
        Err(PartitionError::LockConflict { owner: 7, .. })
    ));
    table.unlock_transaction(7);
    assert!(table.lock(8, &Value::Int(12)).unwrap());

    assert_eq!(
        table.delete(&Value::Int(12)).unwrap(),
        Some(row(&[12, 112]))
    );
    assert!(table.global_index_get(1, &Value::Int(112)).rows.is_empty());
}

/// 非 NULL 的非数值 HASH 键必须报类型错误，不能按 NULL 路由到首分区。
#[test]
fn hash_rejects_text_keys_instead_of_treating_them_as_null() {
    let hash = Partitioning::hash(0, 4).unwrap();

    assert!(matches!(
        hash.partition_for(&Row(vec![Value::Text("not-a-number".to_owned())])),
        Err(PartitionError::TypeMismatch(_))
    ));
    assert_eq!(hash.partition_for(&Row(vec![Value::Null])).unwrap(), 0);
}

/// LIST 定义中的常量只能出现一次，包括同一分区内部的重复项。
#[test]
fn list_rejects_duplicate_values_within_one_partition() {
    assert!(matches!(
        Partitioning::list(0, [("p0", vec![Value::Int(1), Value::Int(1)])],),
        Err(PartitionError::InvalidDefinition(_))
    ));
}

/// UPDATE 失败必须完整恢复旧行及其既有锁。
#[test]
fn failed_update_restores_the_existing_row_lock() {
    let partitioning = Partitioning::range(0, [("p0", Some(10)), ("pmax", None)]).unwrap();
    let mut table = PartitionTable::new(partitioning, 0, [1]);
    table.insert(row(&[1, 101])).unwrap();
    table.insert(row(&[2, 102])).unwrap();
    assert!(table.lock(7, &Value::Int(1)).unwrap());

    assert_eq!(
        table.update(&Value::Int(1), row(&[3, 102])),
        Err(PartitionError::DuplicateKey(Value::Int(102)))
    );
    assert_eq!(table.point_get(&Value::Int(1)).rows, [row(&[1, 101])]);
    assert!(matches!(
        table.lock(8, &Value::Int(1)),
        Err(PartitionError::LockConflict { owner: 7, .. })
    ));
}
