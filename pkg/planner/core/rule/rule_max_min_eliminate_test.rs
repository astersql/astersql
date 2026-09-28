// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// `MaxMinEliminator` 规则的单元测试。
//
// 验证：可走索引的 MAX/MIN 标量聚合会改写为 Join 下的多路 Limit；
// 空聚合列表则保持原计划不变。

use std::collections::BTreeMap;

use super::rule_init::{AggKind, AggregateExpr, Expr, FieldType, LogicalRule, Plan, PlanKind};
use super::rule_max_min_eliminate::MaxMinEliminator;

/// 构造带单列索引的 DataSource 测试桩。
fn data_source() -> Plan {
    Plan {
        kind: PlanKind::DataSource {
            table_id: 1,
            indexes: BTreeMap::from([(8, vec![7])]),
            partition: None,
            selected_partitions: None,
        },
        schema: vec![7],
        children: Vec::new(),
        predicates: Vec::new(),
        keys: vec![vec![7]],
        estimated_rows: 100.0,
        used_stats: BTreeMap::new(),
    }
}

/// 构造无 GROUP BY 的标量聚合，子节点为 `data_source()`。
fn scalar_agg(aggregates: Vec<AggregateExpr>) -> Plan {
    Plan {
        kind: PlanKind::Aggregation {
            aggregates,
            group_by: Vec::new(),
        },
        schema: vec![7],
        children: vec![data_source()],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    }
}

/// MAX 与 MIN 同时出现时应拆成两路 Sort+Limit 再 Inner Join。
#[test]
fn max_min_elimination_uses_index_order_and_limit() {
    let column = Expr::Column {
        id: 7,
        field_type: FieldType::SignedInt,
    };
    let aggregates = [AggKind::Max, AggKind::Min]
        .into_iter()
        .map(|kind| AggregateExpr {
            kind,
            args: vec![column.clone()],
            distinct: false,
        })
        .collect();
    let (optimized, changed) = MaxMinEliminator.optimize(scalar_agg(aggregates)).unwrap();
    assert!(!changed, "Go's Optimize always reports planChanged=false");
    let join = &optimized;
    assert!(matches!(join.kind, PlanKind::Join { .. }));
    assert_eq!(join.children.len(), 2);
    assert!(
        join.children
            .iter()
            .all(|branch| matches!(branch.kind, PlanKind::Aggregation { .. })
                && matches!(branch.children[0].kind, PlanKind::Limit { count: 1 }))
    );
}

#[test]
fn single_max_sorts_without_an_index_and_keeps_changed_false() {
    let mut source = data_source();
    if let PlanKind::DataSource { indexes, .. } = &mut source.kind {
        indexes.clear();
    }
    let column = Expr::Column {
        id: 7,
        field_type: FieldType::SignedInt,
    };
    let mut aggregate = scalar_agg(vec![AggregateExpr {
        kind: AggKind::Max,
        args: vec![column],
        distinct: true,
    }]);
    aggregate.children = vec![source];

    let (optimized, changed) = MaxMinEliminator.optimize(aggregate).unwrap();

    assert!(!changed);
    assert!(matches!(optimized.kind, PlanKind::Aggregation { .. }));
    let limit = &optimized.children[0];
    assert!(matches!(limit.kind, PlanKind::Limit { count: 1 }));
    assert!(matches!(limit.children[0].kind, PlanKind::Sort { .. }));
}

/// 空聚合函数列表不应触发改写。
#[test]
fn empty_scalar_aggregate_is_not_rewritten() {
    let original = scalar_agg(Vec::new());
    let (optimized, changed) = MaxMinEliminator.optimize(original.clone()).unwrap();
    assert!(!changed);
    assert_eq!(optimized, original);
}
