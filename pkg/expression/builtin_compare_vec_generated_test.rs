// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 生成式向量比较测试入口：通过 `#[path]` 引入 Go 对齐实现文件。

#[path = "builtin_compare_vec_generated_5_aster_unit_test.rs"]
mod go_parity;

use crate::expression_compare_vec_generated::{
    BinaryJSON, CompareOp, MyDecimal, Time, vec_compare_decimal, vec_compare_duration,
    vec_compare_json, vec_compare_real, vec_compare_string, vec_compare_time,
};
use types_decimal::mydecimal::NewDecFromStringForTest;
use types_json_functions::CreateBinaryJSON;
use types_time::{FromDate, NewTime, mysql};

fn expected(operator: CompareOp) -> Vec<Option<i64>> {
    let values = match operator {
        CompareOp::Lt => [1, 0, 0],
        CompareOp::Le => [1, 1, 0],
        CompareOp::Gt => [0, 0, 1],
        CompareOp::Ge => [0, 1, 1],
        CompareOp::Eq | CompareOp::NullEq => [0, 1, 0],
        CompareOp::Ne => [1, 0, 1],
    };
    let mut result = values.into_iter().map(Some).collect::<Vec<_>>();
    result.push(if operator == CompareOp::NullEq {
        Some(1)
    } else {
        None
    });
    result
}

#[test]
fn every_go_comparison_signature_covers_all_types_and_operators() {
    let decimals: Vec<Option<MyDecimal>> = ["1", "2", "3"]
        .into_iter()
        .map(|value| Some(NewDecFromStringForTest(value)))
        .chain([None])
        .collect();
    let decimal_rhs: Vec<Option<MyDecimal>> = ["2", "2", "2"]
        .into_iter()
        .map(|value| Some(NewDecFromStringForTest(value)))
        .chain([None])
        .collect();
    let strings = [
        Some("a".to_owned()),
        Some("b".to_owned()),
        Some("c".to_owned()),
        None,
    ];
    let string_rhs = [
        Some("b".to_owned()),
        Some("b".to_owned()),
        Some("b".to_owned()),
        None,
    ];
    let times: Vec<Option<Time>> = [1, 2, 3]
        .into_iter()
        .map(|day| {
            Some(NewTime(
                FromDate(2024, 1, day, 0, 0, 0, 0),
                mysql::TypeDatetime,
                0,
            ))
        })
        .chain([None])
        .collect();
    let time_rhs = vec![times[1], times[1], times[1], None];
    let json: Vec<Option<BinaryJSON>> = [1_i64, 2, 3]
        .into_iter()
        .map(|value| Some(CreateBinaryJSON(value.into()).unwrap()))
        .chain([None])
        .collect();
    let json_rhs = vec![json[1].clone(), json[1].clone(), json[1].clone(), None];
    let real = [Some(1.0), Some(2.0), Some(3.0), None];
    let real_rhs = [Some(2.0), Some(2.0), Some(2.0), None];
    let duration = [Some(1_i64), Some(2), Some(3), None];
    let duration_rhs = [Some(2_i64), Some(2), Some(2), None];

    for operator in [
        CompareOp::Lt,
        CompareOp::Le,
        CompareOp::Gt,
        CompareOp::Ge,
        CompareOp::Eq,
        CompareOp::Ne,
        CompareOp::NullEq,
    ] {
        let expected = expected(operator);
        assert_eq!(
            vec_compare_real(operator, &real, &real_rhs).unwrap(),
            expected
        );
        assert_eq!(
            vec_compare_decimal(operator, &decimals, &decimal_rhs).unwrap(),
            expected
        );
        assert_eq!(
            vec_compare_string(operator, &strings, &string_rhs, "utf8mb4_bin").unwrap(),
            expected
        );
        assert_eq!(
            vec_compare_time(operator, &times, &time_rhs).unwrap(),
            expected
        );
        assert_eq!(
            vec_compare_duration(operator, &duration, &duration_rhs).unwrap(),
            expected
        );
        assert_eq!(
            vec_compare_json(operator, &json, &json_rhs).unwrap(),
            expected
        );
    }
}
