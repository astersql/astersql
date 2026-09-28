// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// `SELECT INTO OUTFILE` 浮点格式化单元测试。
//
// 验证 `DumpRealOutfile` 在普通量级使用十进制输出，在极大/极小量级切换为科学计数法。

use crate::select_into::DumpRealOutfile;
use astersql_types::datum::FieldType;

/// 与 Go `TestDumpReal` 的精度、阈值和科学计数法矩阵保持一致。
#[test]
fn select_into_real_formatter_matches_go_precision_and_thresholds() {
    let cases = [
        (1.2, 1, "1.2"),
        (1.2, 2, "1.20"),
        (2.0, 2, "2.00"),
        (2.333, -1, "2.333"),
        (1e14, -1, "100000000000000"),
        (1e15, -1, "1e15"),
        (1e-15, -1, "0.000000000000001"),
        (1e-16, -1, "1e-16"),
    ];

    for (value, decimal, expected) in cases {
        let mut field = FieldType::default();
        field.SetDecimal(decimal);
        let (_, actual) = DumpRealOutfile(Vec::new(), Vec::new(), value, &field);
        assert_eq!(String::from_utf8(actual).unwrap(), expected);
    }
}

/// Go `strconv.AppendFloat` preserves the SQL-facing spellings of non-finite values.
#[test]
fn select_into_real_formatter_matches_go_non_finite_spellings() {
    let field = FieldType::default();
    for (value, expected) in [
        (f64::INFINITY, "Inf"),
        (f64::NEG_INFINITY, "-Inf"),
        (f64::NAN, "NaN"),
    ] {
        let (_, actual) = DumpRealOutfile(Vec::new(), Vec::new(), value, &field);
        assert_eq!(String::from_utf8(actual).unwrap(), expected);
    }

    let mut fixed = FieldType::default();
    fixed.SetDecimal(2);
    let (_, actual) = DumpRealOutfile(Vec::new(), Vec::new(), f64::INFINITY, &fixed);
    assert_eq!(actual, b"+Inf");
}
