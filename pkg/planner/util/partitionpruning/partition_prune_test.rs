// Copyright 2026 AsterSQL.

// 分区剪枝入口的边界与 DDL 删除中分区回归测试。
//
// 这里用最小分区元数据直接调用 `partition_pruning`，重点固定返回值中的定义下标、
// 全分区哨兵约定，以及 List 分区删除期间的重叠替代规则。

use super::partition_prune::{FULL_RANGE, PartitionedTable, partition_pruning};
use astersql_planner_core_rule::rule_init::{
    Expr, FieldType, PartitionDefinition, PartitionInfo, PartitionKind, Value,
};
use std::collections::BTreeMap;

fn range_table(bounds: &[i64]) -> PartitionedTable {
    // 定义 ID 与数组下标保持一致，便于断言直接表达剪枝结果中的分区下标。
    let definitions = bounds
        .iter()
        .enumerate()
        .map(|(id, bound)| PartitionDefinition {
            id: id as i64,
            name: format!("p{id}"),
            less_than: vec![Value::Int(*bound)],
            in_values: Vec::new(),
        })
        .collect();
    PartitionedTable {
        partition: PartitionInfo {
            kind: PartitionKind::Range,
            columns: vec![1],
            definitions,
        },
        overlapping_dropping: BTreeMap::new(),
    }
}

fn comparison(function: &str, value: Value) -> Expr {
    // 所有用例都约束分区列 1；比较函数名沿用剪枝器识别的标量表达式协议。
    Expr::Scalar {
        function: function.to_owned(),
        args: vec![
            Expr::Column {
                id: 1,
                field_type: FieldType::SignedInt,
            },
            Expr::Constant(value),
        ],
        field_type: FieldType::SignedInt,
    }
}

#[test]
fn strict_range_upper_bound_excludes_boundary_partition() {
    let table = range_table(&[10, 20, 30]);

    // `col < 10` 仅触及首个 LESS THAN 10 分区，不应把边界归入后继分区。
    assert_eq!(
        partition_pruning(&table, &[comparison("lt", Value::Int(10))], &[]),
        Ok(vec![0])
    );
}

#[test]
fn one_partition_full_selection_returns_full_range() {
    let table = range_table(&[10]);

    // Go handleDroppingForRange 在选中数等于定义数时一律压缩为 FullRange，
    // 单分区也不例外。
    assert_eq!(partition_pruning(&table, &[], &[]), Ok(vec![FULL_RANGE]));
}

#[test]
fn values_outside_finite_range_do_not_select_last_partition() {
    let table = range_table(&[10, 20]);

    // 没有 MAXVALUE 定义时，有限末界 20 及其之上不属于任何分区。
    for function in ["eq", "ge", "gt"] {
        assert_eq!(
            partition_pruning(&table, &[comparison(function, Value::Int(20))], &[]),
            Ok(Vec::new()),
            "operator {function} must not fall back to the last finite partition"
        );
    }
}

#[test]
fn null_comparison_is_not_treated_as_is_null() {
    let table = range_table(&[10, 20]);

    assert_eq!(
        partition_pruning(&table, &[comparison("eq", Value::Null)], &[]),
        Ok(Vec::new())
    );
    assert_eq!(
        partition_pruning(
            &table,
            &[Expr::Scalar {
                function: "in".to_owned(),
                args: vec![
                    Expr::Column {
                        id: 1,
                        field_type: FieldType::SignedInt,
                    },
                    Expr::Constant(Value::Null),
                ],
                field_type: FieldType::SignedInt,
            }],
            &[]
        ),
        Ok(Vec::new())
    );
    assert_eq!(
        partition_pruning(
            &table,
            &[Expr::Scalar {
                function: "is_null".to_owned(),
                args: vec![Expr::Column {
                    id: 1,
                    field_type: FieldType::SignedInt,
                }],
                field_type: FieldType::SignedInt,
            }],
            &[]
        ),
        Ok(vec![0])
    );
}

#[test]
fn list_dropping_partition_uses_overlapping_replacement() {
    // p0、p2 都映射到仍有效的重叠分区 p2；p1 表示删除后没有替代分区。
    let table = PartitionedTable {
        partition: PartitionInfo {
            kind: PartitionKind::List,
            columns: vec![1],
            definitions: (0..3)
                .map(|id| PartitionDefinition {
                    id,
                    name: format!("p{id}"),
                    less_than: Vec::new(),
                    in_values: vec![vec![Value::Int(id + 1)]],
                })
                .collect(),
        },
        overlapping_dropping: BTreeMap::from([(0, Some(2)), (1, None), (2, Some(2))]),
    };

    assert_eq!(
        partition_pruning(&table, &[comparison("eq", Value::Int(1))], &[]),
        Ok(vec![2])
    );
    // 显式指定原分区 p0 时，替代后的 p2 不满足名称过滤，因此结果为空。
    assert_eq!(
        partition_pruning(
            &table,
            &[comparison("eq", Value::Int(1))],
            &["p0".to_owned()]
        ),
        Ok(Vec::new())
    );
}
