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

use crate::column_value::{parse_decimal_to_scaled_integer, parse_raw_column_value};
use crate::{Column, ColumnInfo, ColumnType, LogicalType, PhysicalType, TimeUnit};

fn timestamp_column(nullable: bool) -> Column {
    Column {
        info: ColumnInfo {
            name: "ts".into(),
            database_type_name: "DATETIME".into(),
            nullable,
            precision: 0,
            scale: 0,
        },
        column_type: ColumnType {
            physical: PhysicalType::Int64,
            logical: LogicalType::Timestamp {
                adjusted_to_utc: false,
                unit: TimeUnit::Micros,
            },
            type_length: -1,
            precision: 0,
            scale: 0,
        },
        allows_null_encoding: nullable,
        timestamp_unit: TimeUnit::Micros,
    }
}

#[test]
fn decimal_parser_rejects_whitespace_like_go_big_rat() {
    assert!(parse_decimal_to_scaled_integer(" 1.25", 2).is_err());
    assert!(parse_decimal_to_scaled_integer("1.25 ", 2).is_err());
}

#[test]
fn timestamp_parser_rejects_an_empty_fraction_like_go_time_parse() {
    let required = timestamp_column(false);
    assert!(parse_raw_column_value(b"2024-01-02 03:04:05.", &required).is_err());

    let nullable = timestamp_column(true);
    assert_eq!(
        parse_raw_column_value(b"2024-01-02 03:04:05.", &nullable).unwrap(),
        (None, true)
    );
}
