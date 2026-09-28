// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 比较表达式单元测试入口。
//
// 经 `#[path]` 挂载 `builtin_compare_7_aster_unit_test.rs`，
// 覆盖 coalesce / greatest / least / INTERVAL / 比较类型解析与常量精化。

/// 与 Go `builtin_compare_test.go` 对齐的比较语义用例。
#[path = "builtin_compare_7_aster_unit_test.rs"]
mod go_parity;

use crate::builtin_compare::{
    Collation, Datum, EvalType, ExpressionMeta, FieldType, MysqlType, TemporalMode,
    get_accurate_cmp_type, greatest, least,
};

#[test]
fn greatest_and_least_preserve_go_evaluation_order_before_null() {
    for result in [
        greatest(
            &[Datum::Error("first".into()), Datum::Null],
            TemporalMode::Direct,
            Collation::Binary,
        ),
        least(
            &[Datum::Error("first".into()), Datum::Null],
            TemporalMode::Direct,
            Collation::Binary,
        ),
    ] {
        assert_eq!(result.unwrap_err().to_string(), "first");
    }

    assert_eq!(
        greatest(
            &[Datum::Null, Datum::Error("not evaluated".into())],
            TemporalMode::Direct,
            Collation::Binary,
        )
        .unwrap(),
        Datum::Null
    );
}

#[test]
fn correlated_duration_is_not_treated_as_go_temporal_column() {
    let correlated = ExpressionMeta::correlated_column(FieldType::new(MysqlType::Duration));
    let constant = ExpressionMeta::constant(FieldType::new(MysqlType::Varchar));

    assert_eq!(
        get_accurate_cmp_type(&correlated, &constant),
        EvalType::String
    );
}
