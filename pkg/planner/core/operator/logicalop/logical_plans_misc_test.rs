// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use crate::*;
use std::any::Any;

#[derive(Default)]
struct TestPlan {
    base: BaseLogicalPlan,
}

impl LogicalPlan for TestPlan {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.base
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.base
    }
}

fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        0,
        id,
        0,
    )
}

fn descriptor(name: &str, argument: Expression) -> aggregation::AggFuncDesc {
    aggregation::NewAggFuncDesc(
        &exprstatic::NewExprContext(Vec::new()),
        name,
        vec![argument],
        false,
    )
    .expect("build aggregate descriptor")
}

#[test]
fn get_has_tiflash_reads_the_prepared_cache_only() {
    let mut parent = TestPlan::default();
    let mut child = TestPlan::default();
    child.base_mut().PreparePossibleProperties(&[true]);
    parent.SetChildren(vec![Box::new(child)]);

    assert!(!GetHasTiFlash(Some(&parent)));
    parent.base_mut().PreparePossibleProperties(&[true]);
    assert!(GetHasTiFlash(Some(&parent)));
    assert!(!GetHasTiFlash(None));
}

#[test]
fn duplicate_agnostic_columns_require_every_aggregate_to_qualify() {
    let first = column(11);
    let second = column(22);
    let mut distinct_sum = descriptor(aggregation::ast::AggFuncSum, Box::new(first.clone()));
    distinct_sum.HasDistinct = true;
    let aggregate = LogicalAggregation {
        AggFuncs: vec![
            distinct_sum,
            descriptor(aggregation::ast::AggFuncMax, Box::new(second.clone())),
        ],
        ..LogicalAggregation::default()
    };
    let (is_aggregate, columns) = GetDupAgnosticAggCols(&aggregate, vec![column(99)]);
    assert!(is_aggregate);
    assert_eq!(
        columns
            .iter()
            .map(|column| column.UniqueID)
            .collect::<Vec<_>>(),
        vec![11, 22]
    );

    let ordinary_sum = LogicalAggregation {
        AggFuncs: vec![descriptor(aggregation::ast::AggFuncSum, Box::new(first))],
        ..LogicalAggregation::default()
    };
    let (is_aggregate, columns) = GetDupAgnosticAggCols(&ordinary_sum, vec![second]);
    assert!(is_aggregate);
    assert!(columns.is_empty());

    let non_aggregate = TestPlan::default();
    let (is_aggregate, columns) = GetDupAgnosticAggCols(&non_aggregate, Vec::new());
    assert!(!is_aggregate);
    assert!(columns.is_empty());
}
