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

fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
        0,
        id,
        0,
    )
}

fn descriptor(name: &str, argument: Expression) -> AggFuncDesc {
    aggregation::NewAggFuncDesc(
        &exprstatic::NewExprContext(Vec::new()),
        name,
        vec![argument],
        false,
    )
    .expect("build aggregate descriptor")
}

#[test]
fn group_by_columns_match_go_direct_column_contract() {
    let first = column(2);
    let second = column(1);
    let aggregation = LogicalAggregation {
        GroupByItems: vec![
            Box::new(first.clone()),
            Box::new(second.clone()),
            Box::new(first),
        ],
        ..LogicalAggregation::default()
    };

    let ids = aggregation
        .GetGroupByCols()
        .into_iter()
        .map(|column| column.UniqueID)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![2, 1, 2]);
}

#[test]
fn constant_group_and_pull_up_empty_aggregation_match_go() {
    let empty = LogicalAggregation::default();
    assert!(empty.hasOnlyConstGroupByItems());
    assert!(empty.CanPullUp());

    let grouped = LogicalAggregation {
        GroupByItems: vec![Box::new(expression::NewOne())],
        ..LogicalAggregation::default()
    };
    assert!(grouped.hasOnlyConstGroupByItems());
    assert!(!grouped.CanPullUp());
}

#[test]
fn pull_up_evaluates_aggregate_arguments_with_child_columns_as_null() {
    let input = column(6);
    let mut nullable_argument = LogicalAggregation {
        AggFuncs: vec![descriptor(
            aggregation::ast::AggFuncMax,
            Box::new(input.clone()),
        )],
        ..LogicalAggregation::default()
    };
    let mut child = LogicalTableDual::default();
    child.SetSchema(expression::NewSchema(vec![input]));
    nullable_argument.SetChildren(vec![Box::new(child)]);
    assert!(nullable_argument.CanPullUp());

    nullable_argument.AggFuncs = vec![descriptor(
        aggregation::ast::AggFuncCount,
        Box::new(expression::NewOne()),
    )];
    assert!(!nullable_argument.CanPullUp());
}

#[test]
fn aggregate_argument_equivalence_matches_go_restrictions() {
    let constant = Box::new(expression::NewOne()) as Expression;
    let mut max = descriptor(aggregation::ast::AggFuncMax, constant.CloneExpr());
    let first_row = descriptor(aggregation::ast::AggFuncFirstRow, constant.CloneExpr());
    let non_constant = descriptor(aggregation::ast::AggFuncMin, Box::new(column(7)));
    max.HasDistinct = true;

    let aggregation = LogicalAggregation {
        AggFuncs: vec![max, first_row, non_constant],
        ..LogicalAggregation::default()
    };
    assert!(!aggregation.aggFuncResultMatchesArgForNonEmptyGroup(0));
    assert!(!aggregation.aggFuncResultMatchesArgForNonEmptyGroup(1));
    assert!(!aggregation.aggFuncResultMatchesArgForNonEmptyGroup(2));
}

#[test]
fn aggregate_output_helpers_return_only_matching_schema_columns() {
    let max_output = column(10);
    let first_row_output = column(11);
    let count_output = column(12);
    let aggregation = LogicalAggregation {
        AggFuncs: vec![
            descriptor(aggregation::ast::AggFuncMax, Box::new(expression::NewOne())),
            descriptor(aggregation::ast::AggFuncFirstRow, Box::new(column(3))),
            descriptor(aggregation::ast::AggFuncCount, Box::new(column(4))),
        ],
        GroupByItems: vec![Box::new(column(5))],
        ..LogicalAggregation::default()
    };
    let mut aggregation = aggregation;
    aggregation.SetSchema(expression::NewSchema(vec![
        max_output.clone(),
        first_row_output.clone(),
        count_output,
    ]));

    assert_eq!(
        aggregation
            .getAggFuncsColsForConstResult()
            .into_iter()
            .map(|column| column.UniqueID)
            .collect::<Vec<_>>(),
        vec![max_output.UniqueID]
    );
    assert_eq!(
        aggregation
            .getAggFuncsColsForFirstRow()
            .into_iter()
            .map(|column| column.UniqueID)
            .collect::<Vec<_>>(),
        vec![first_row_output.UniqueID]
    );
}
