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

use std::collections::{BTreeMap, BTreeSet};

use super::rule_init::{
    Expr, FieldType, LogicalRule, PartitionDefinition, PartitionInfo, PartitionKind, Plan,
    PlanKind, Value,
};
use super::rule_partition_processor::PartitionProcessor;

#[test]
fn incomparable_range_constant_aborts_pruning_like_go() {
    let plan = Plan {
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
        predicates: vec![Expr::Scalar {
            function: "eq".into(),
            args: vec![
                Expr::Column {
                    id: 1,
                    field_type: FieldType::SignedInt,
                },
                Expr::Constant(Value::Text("not-an-integer".into())),
            ],
            field_type: FieldType::Bool,
        }],
        keys: Vec::new(),
        estimated_rows: 30.0,
        used_stats: BTreeMap::new(),
    };

    let (plan, changed) = PartitionProcessor.optimize(plan).unwrap();
    assert!(changed);
    let PlanKind::DataSource {
        selected_partitions,
        ..
    } = plan.kind
    else {
        panic!("an incomparable constant must not eliminate the data source")
    };
    assert_eq!(selected_partitions, Some(BTreeSet::from([0, 1, 2])));
}
