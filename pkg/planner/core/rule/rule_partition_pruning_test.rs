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

// 分区裁剪（Partition Pruning）相关单元测试。
//
// 覆盖 Range 分区等值谓词选中的规范分区，以及 `PartitionRangeOr` 的
// 交集、并集与区间压实行为是否与 Go 侧一致。

use std::collections::{BTreeMap, BTreeSet};

use super::rule_init::{
    Expr, FieldType, LogicalRule, PartitionDefinition, PartitionInfo, PartitionKind, Plan,
    PlanKind, Value,
};
use super::rule_partition_processor::{PartitionProcessor, PartitionRange, PartitionRangeOr};

/// 构造三区 Range 分区 DataSource，并挂上单个过滤谓词。
fn range_source(predicate: Expr) -> Plan {
    Plan {
        kind: PlanKind::DataSource {
            table_id: 42,
            indexes: BTreeMap::new(),
            partition: Some(PartitionInfo {
                kind: PartitionKind::Range,
                columns: vec![1],
                definitions: [10, 20, 30]
                    .into_iter()
                    .enumerate()
                    .map(|(id, bound)| PartitionDefinition {
                        id: id as i64,
                        name: format!("p{id}"),
                        less_than: vec![Value::Int(bound)],
                        in_values: Vec::new(),
                    })
                    .collect(),
            }),
            selected_partitions: None,
        },
        schema: vec![1],
        children: Vec::new(),
        predicates: vec![predicate],
        keys: Vec::new(),
        estimated_rows: 30.0,
        used_stats: BTreeMap::new(),
    }
}

/// `col = 15` 应只命中第二个分区（less_than 20）。
#[test]
fn range_partition_predicate_selects_canonical_partition() {
    let predicate = Expr::Scalar {
        function: "eq".into(),
        args: vec![
            Expr::Column {
                id: 1,
                field_type: FieldType::SignedInt,
            },
            Expr::Constant(Value::Int(15)),
        ],
        field_type: FieldType::Bool,
    };
    let (plan, changed) = PartitionProcessor
        .optimize(range_source(predicate))
        .unwrap();
    assert!(changed);
    let PlanKind::DataSource {
        selected_partitions,
        ..
    } = plan.kind
    else {
        panic!("partition pruning must keep a non-empty data source")
    };
    assert_eq!(selected_partitions, Some(BTreeSet::from([1])));
}

/// 验证区间并交差与压实结果与 Go 测试期望一致。
#[test]
fn partition_ranges_union_intersection_and_compaction_match_go() {
    let intersection_range_cases = [
        (vec![(0, 3), (6, 12)], (4, 7), vec![(6, 7)]),
        (vec![(0, 5)], (6, 7), vec![]),
        (
            vec![(0, 4), (6, 7), (8, 11)],
            (3, 9),
            vec![(3, 4), (6, 7), (8, 9)],
        ),
    ];
    for (left, right, expected) in intersection_range_cases {
        assert_eq!(
            ranges(&left).intersect(&ranges(&[right])),
            ranges(&expected)
        );
    }

    let intersection_cases = [
        (vec![(0, 3), (6, 12)], vec![(4, 7)], vec![(6, 7)]),
        (vec![(4, 7)], vec![(0, 3), (6, 12)], vec![(6, 7)]),
        (
            vec![(4, 7), (8, 10)],
            vec![(0, 5), (6, 12)],
            vec![(4, 5), (6, 7), (8, 10)],
        ),
    ];
    for (left, right, expected) in intersection_cases {
        assert_eq!(ranges(&left).intersect(&ranges(&right)), ranges(&expected));
    }

    let union_cases = [
        (vec![(0, 1), (2, 7)], vec![(3, 5)], vec![(0, 1), (2, 7)]),
        (vec![(2, 7)], vec![(0, 3), (4, 12)], vec![(0, 12)]),
        (vec![(4, 7), (8, 10)], vec![(0, 5)], vec![(0, 7), (8, 10)]),
    ];
    for (left, right, expected) in union_cases {
        assert_eq!(ranges(&left).union(&ranges(&right)), ranges(&expected));
    }
}

fn ranges(bounds: &[(usize, usize)]) -> PartitionRangeOr {
    PartitionRangeOr(
        bounds
            .iter()
            .map(|&(start, end)| PartitionRange { start, end })
            .collect(),
    )
}
