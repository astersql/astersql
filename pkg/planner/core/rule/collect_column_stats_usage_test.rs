// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `collect_column_stats_usage` 的单元测试。
//
// 构造含分区 DataSource 与等值 Join 的逻辑计划，校验谓词列、
// 访问表、选中分区、interesting columns 与算子计数是否正确汇总。

use std::collections::{BTreeMap, BTreeSet};

use super::collect_column_stats_usage::collect_column_stats_usage;
use super::rule_init::{
    AggKind, AggregateExpr, Expr, FieldType, JoinType, PartitionDefinition, PartitionInfo,
    PartitionKind, Plan, PlanKind,
};

/// 构造指定列 ID 的列表达式。
fn column(id: i64) -> Expr {
    Expr::Column {
        id,
        field_type: FieldType::SignedInt,
    }
}

/// 构造带 Hash 分区与下推谓词的 DataSource 叶子计划。
fn source(table_id: i64, column_id: i64) -> Plan {
    Plan {
        kind: PlanKind::DataSource {
            table_id,
            indexes: BTreeMap::from([(1, vec![column_id])]),
            partition: Some(PartitionInfo {
                kind: PartitionKind::Hash,
                columns: vec![column_id],
                definitions: (0..4)
                    .map(|id| PartitionDefinition {
                        id,
                        name: format!("p{id}"),
                        less_than: Vec::new(),
                        in_values: Vec::new(),
                    })
                    .collect(),
            }),
            // 静态选中分区下标 1 与 3，对应物理分区 ID 1、3。
            selected_partitions: Some(BTreeSet::from([1, 3])),
        },
        schema: vec![column_id],
        children: Vec::new(),
        predicates: vec![column(column_id)],
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    }
}

/// 真实 Join 计划应汇总两侧表、分区、谓词列与 join interesting columns。
#[test]
fn collects_predicate_partition_and_join_columns_from_real_plan() {
    let plan = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: vec![Expr::Scalar {
                function: "eq".into(),
                args: vec![column(10), column(20)],
                field_type: FieldType::Bool,
            }],
            other_conditions: Vec::new(),
        },
        schema: vec![10, 20],
        children: vec![source(1, 10), source(2, 20)],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 4.0,
        used_stats: BTreeMap::new(),
    };

    let usage = collect_column_stats_usage(&plan, true);
    assert_eq!(usage.visited_tables, BTreeSet::from([1, 2]));
    assert_eq!(usage.table_partitions[&1], BTreeSet::from([1, 3]));
    assert_eq!(
        usage.predicate_columns,
        BTreeMap::from([(10, true), (20, true)])
    );
    assert_eq!(usage.interesting_columns[&1], BTreeSet::from([10]));
    assert_eq!(usage.interesting_columns[&2], BTreeSet::from([20]));
    assert_eq!(usage.operator_count, 3);
}

/// Go 只把 GROUP BY 登记为谓词列；普通聚合参数仅参与输出列血缘传播。
#[test]
fn aggregate_arguments_are_not_predicate_columns_without_a_parent_predicate() {
    let mut child = source(1, 10);
    child.predicates.clear();
    let plan = Plan {
        kind: PlanKind::Aggregation {
            aggregates: vec![AggregateExpr {
                kind: AggKind::Sum,
                args: vec![column(10)],
                distinct: false,
            }],
            group_by: Vec::new(),
        },
        schema: vec![100],
        children: vec![child],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };

    let usage = collect_column_stats_usage(&plan, false);
    assert!(usage.predicate_columns.is_empty());
}

/// 开启索引裁剪列收集不应把索引首列本身误当成谓词列。
#[test]
fn index_pruning_collection_does_not_create_predicate_columns() {
    let mut plan = source(1, 10);
    plan.predicates.clear();

    let usage = collect_column_stats_usage(&plan, true);
    assert!(usage.predicate_columns.is_empty());
}
