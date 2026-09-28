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

// executor 包级回归测试：与 Go `pkg_test.go` 保持同一组 Apply
// 连接语义和 information_schema 排序契约。

use std::sync::Arc;

use crate::show::moveInfoSchemaToFront;
use astersql_executor_join::hash_join_v1::NestedLoopApplyExec;
use astersql_executor_join::joiner::{JoinType, Joiner, Row};
use astersql_executor_join::row_table_builder::Value;

fn row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

#[test]
fn nested_loop_apply_matches_equal_rows_after_outer_filter() {
    let equality = Arc::new(|joined: &[Value]| {
        Ok(Some(matches!(
            joined,
            [Value::Int(left), Value::Int(right)] if left == right
        )))
    });
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        vec![equality],
        None,
        false,
        32,
    )
    .unwrap();
    let outer_rows = (1..=6)
        .filter(|value| *value < 6)
        .map(|value| row(&[value]))
        .collect();
    let mut apply = NestedLoopApplyExec::new(
        outer_rows,
        Box::new(|_| Ok((1..=6).map(|value| row(&[value])).collect())),
        joiner,
        vec![0],
    );
    apply.open();

    let mut actual = Vec::new();
    loop {
        let batch = apply.next(2).unwrap();
        if batch.is_empty() {
            break;
        }
        actual.extend(batch);
    }

    assert_eq!(
        actual,
        (1..=5)
            .map(|value| row(&[value, value]))
            .collect::<Vec<_>>()
    );
}

#[test]
fn move_information_schema_to_front_matches_go_cases() {
    let cases = [
        (vec![], vec![]),
        (
            vec!["A", "B", "C", "a", "b", "c"],
            vec!["A", "B", "C", "a", "b", "c"],
        ),
        (
            vec!["A", "B", "C", "INFORMATION_SCHEMA"],
            vec!["INFORMATION_SCHEMA", "A", "B", "C"],
        ),
        (
            vec!["A", "B", "INFORMATION_SCHEMA", "a"],
            vec!["INFORMATION_SCHEMA", "A", "B", "a"],
        ),
        (vec!["INFORMATION_SCHEMA"], vec!["INFORMATION_SCHEMA"]),
        (
            vec!["A", "B", "C", "INFORMATION_SCHEMA", "a", "b"],
            vec!["INFORMATION_SCHEMA", "A", "B", "C", "a", "b"],
        ),
    ];

    for (input, expected) in cases {
        let mut databases = input.into_iter().map(str::to_owned).collect::<Vec<_>>();
        moveInfoSchemaToFront(&mut databases);
        assert_eq!(databases, expected);
    }
}
