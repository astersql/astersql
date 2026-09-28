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

// `parsedef` 迁移单元测试：校验 `Row` 零值/克隆与 `MarshalLogArray` 行为对齐 Go。

use super::{ArrayEncoder, Row};
use types_crate::datum::Datum;

/// 记录 AppendString 调用顺序的测试用编码器。
#[derive(Default)]
struct RecordingEncoder {
    /// 已追加的字符串列表。
    values: Vec<String>,
}

/// 把字符串推入 `values`。
impl ArrayEncoder for RecordingEncoder {
    fn AppendString(&mut self, value: &str) {
        self.values.push(value.to_owned());
    }
}

/// 构造带 Int64 的 Datum，便于断言 String() 输出。
fn int_datum(value: i64) -> Datum {
    let mut datum = Datum::default();
    datum.SetInt64(value);
    datum
}

/// 构造 utf8mb4_bin 排序规则下的字符串 Datum。
fn string_datum(value: &str) -> Datum {
    let mut datum = Datum::default();
    datum.SetString(value.to_owned(), "utf8mb4_bin".to_owned());
    datum
}

/// 零值与 clone 后字段应与 Go 结构体语义一致。
#[test]
fn row_zero_value_and_clone_match_go_struct_behavior() {
    let zero = Row::default();
    assert_eq!(zero.RowID, 0);
    assert!(zero.Row.is_empty());
    assert_eq!(zero.Length, 0);

    let original = Row {
        RowID: 17,
        Row: vec![int_datum(23), string_datum("tidb")],
        Length: 31,
    };
    let cloned = original.clone();

    assert_eq!(cloned.RowID, original.RowID);
    assert_eq!(cloned.Length, original.Length);
    assert_eq!(cloned.Row.len(), original.Row.len());
    assert_eq!(cloned.Row[0].String(), original.Row[0].String());
    assert_eq!(cloned.Row[1].String(), original.Row[1].String());
}

/// MarshalLogArray 应按列顺序追加每个 Datum 的字符串形式。
#[test]
fn marshal_log_array_appends_each_datum_string_in_order() {
    let row = Row {
        RowID: 9,
        Row: vec![int_datum(-12), string_datum("a\nquoted")],
        Length: 19,
    };
    let expected = row.Row.iter().map(Datum::String).collect::<Vec<_>>();
    let mut encoder = RecordingEncoder::default();

    assert_eq!(row.MarshalLogArray(&mut encoder), Ok(()));
    assert_eq!(encoder.values, expected);
    assert_eq!(row.RowID, 9);
    assert_eq!(row.Length, 19);
}

/// 空行序列化不应向编码器写入任何字符串。
#[test]
fn marshal_log_array_accepts_an_empty_row() {
    let row = Row::default();
    let mut encoder = RecordingEncoder::default();

    assert_eq!(row.MarshalLogArray(&mut encoder), Ok(()));
    assert!(encoder.values.is_empty());
}

/// Go `int` 的行长度在 64 位目标上可超过 i32。
#[test]
fn row_length_uses_platform_int_width() {
    let length: isize = i32::MAX as isize + 1;
    let row = Row {
        Length: length,
        ..Row::default()
    };
    assert_eq!(row.Length, length);
}
