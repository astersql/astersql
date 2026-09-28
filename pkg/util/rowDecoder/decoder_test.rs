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

// rowDecoder 解码与生成列评估的单测。
//
// 对照 Go `TestRowDecoder` / `TestClusterIndexRowDecoder`：验证默认值填充、
// 生成列（generated column）求值、整数/无符号 PK handle，以及聚集索引
// （clustered index / common handle）路径。

use std::collections::HashMap;
use std::sync::Arc;

use super::*;

/// 构造带 id、offset、类型与默认值的测试列。
fn column(id: i64, offset: usize, kind: FieldKind, default_value: Datum) -> Arc<TableColumn> {
    Arc::new(TableColumn {
        column_info: ColumnInfo {
            id,
            offset,
            field_type: FieldType { kind },
        },
        default_value,
        change_state_info: None,
    })
}

/// 将表列与可选生成表达式包装为解码用 Column。
fn decode_column(col: Arc<TableColumn>, gen_expr: Option<Arc<dyn Expression>>) -> Column {
    Column { col, gen_expr }
}

/// Mirrors Go's generated column `c4+c5` over the simplified Datum model: when
/// both inputs are integers, return their sum; otherwise preserve nullability.
/// 模拟 Go 生成列 `c4+c5`：两整数相加，遇 NULL 则传播 NULL。
struct AddColumns {
    left: usize,
    right: usize,
}

impl Expression for AddColumns {
    fn eval(&self, _context: &BuildContext, row: &[Datum]) -> Result<Datum, DecodeError> {
        let left = row.get(self.left).cloned().unwrap_or(Datum::Null);
        let right = row.get(self.right).cloned().unwrap_or(Datum::Null);
        match (left, right) {
            (Datum::Int(left), Datum::Int(right)) => Ok(Datum::Int(left + right)),
            (Datum::Null, _) | (_, Datum::Null) => Ok(Datum::Null),
            (left, right) => Err(DecodeError::Eval(format!(
                "unsupported generated add operands {left:?} + {right:?}"
            ))),
        }
    }
}

/// 构建带/不带生成列的两套 RowDecoder，以及共享的列与 BuildContext。
fn build_decoders(
    pk_kind: FieldKind,
) -> (RowDecoder, RowDecoder, Vec<Arc<TableColumn>>, BuildContext) {
    let c1 = column(1, 0, FieldKind::Int, Datum::Null);
    let c2 = column(2, 1, FieldKind::String, Datum::Null);
    let c3 = column(3, 2, FieldKind::Int, Datum::Null);
    let c4 = column(4, 3, FieldKind::Int, Datum::Null);
    // OriginDefaultValue "02:00:02" is represented as seconds for the simplified model.
    let c5 = column(5, 4, FieldKind::Int, Datum::Int(7202));
    let c6 = column(6, 5, FieldKind::Int, Datum::Null);
    let c7 = column(7, 6, pk_kind, Datum::Null);
    let columns = vec![
        Arc::clone(&c1),
        Arc::clone(&c2),
        Arc::clone(&c3),
        Arc::clone(&c4),
        Arc::clone(&c5),
        Arc::clone(&c6),
        Arc::clone(&c7),
    ];

    let mut decode_cols_map = HashMap::new();
    let mut decode_cols_map2 = HashMap::new();
    for col in &columns {
        decode_cols_map2.insert(col.column_info.id, decode_column(Arc::clone(col), None));
        let gen_expr = if col.column_info.id == 6 {
            Some(Arc::new(AddColumns { left: 3, right: 4 }) as Arc<dyn Expression>)
        } else {
            None
        };
        decode_cols_map.insert(col.column_info.id, decode_column(Arc::clone(col), gen_expr));
    }

    let table = Table {
        meta: TableMeta {
            pk_is_handle: true,
            pk_column_id: Some(7),
            ..TableMeta::default()
        },
    };
    let de = NewRowDecoder(table.clone(), columns.clone(), decode_cols_map);
    let de_with_no_gen_cols = NewRowDecoder(table, columns.clone(), decode_cols_map2);
    let context = BuildContext {
        time_zone: "UTC".to_owned(),
    };
    (de, de_with_no_gen_cols, columns, context)
}

/// TestRowDecoder covers generated columns, defaults, PK handle and the
/// no-generated-column decoder path from Go's TestRowDecoder.
/// 覆盖生成列、默认值、PK handle 及无生成列解码路径。
#[test]
fn TestRowDecoder() {
    // c4=8*3600+60+1, c5=3601 => generated c6 = c4+c5.
    let t1 = Datum::Int(8 * 3600 + 60 + 1);
    let d1 = Datum::Int(3601);
    let t2 = Datum::Int(8 * 3600 + 60 + 1 + 3601);
    // Missing c5 uses OriginDefaultValue 7202 => c6 = c4 + 7202.
    let t3 = Datum::Int(8 * 3600 + 60 + 1 + 7202);

    struct TestRow {
        cols: Vec<(i64, Datum)>,
        // None means the column is filled only into mutRow (Go default path) and
        // is absent from the returned map unless it is a generated column.
        output: Vec<(i64, Option<Datum>)>,
    }

    let test_rows = [
        TestRow {
            cols: vec![
                (1, Datum::Int(100)),
                (2, Datum::String("abc".to_owned())),
                (3, Datum::Int(1)),
                (4, t1.clone()),
                (5, d1.clone()),
            ],
            output: vec![
                (1, Some(Datum::Int(100))),
                (2, Some(Datum::String("abc".to_owned()))),
                (3, Some(Datum::Int(1))),
                (4, Some(t1.clone())),
                (5, Some(d1)),
                (6, Some(t2)),
            ],
        },
        TestRow {
            cols: vec![
                (1, Datum::Int(100)),
                (2, Datum::String("abc".to_owned())),
                (3, Datum::Int(1)),
                (4, t1.clone()),
            ],
            output: vec![
                (1, Some(Datum::Int(100))),
                (2, Some(Datum::String("abc".to_owned()))),
                (3, Some(Datum::Int(1))),
                (4, Some(t1)),
                // Go keeps the default only in mutRow for generated-expr inputs;
                // the returned map has no c5 entry (DefaultValue stays empty).
                (5, None),
                (6, Some(t3)),
            ],
        },
        TestRow {
            cols: vec![
                (1, Datum::Null),
                (2, Datum::Null),
                (3, Datum::Null),
                (4, Datum::Null),
                (5, Datum::Null),
            ],
            output: vec![
                (1, Some(Datum::Null)),
                (2, Some(Datum::Null)),
                (3, Some(Datum::Null)),
                (4, Some(Datum::Null)),
                (5, Some(Datum::Null)),
                (6, Some(Datum::Null)),
            ],
        },
    ];

    for (i, row) in test_rows.iter().enumerate() {
        // Go flips the PK handle column to unsigned from the second case.
        let pk_kind = if i > 0 {
            FieldKind::UInt
        } else {
            FieldKind::Int
        };
        let (mut de, mut de_with_no_gen_cols, columns, context) = build_decoders(pk_kind);

        let bs = EncodeRow(&row.cols, true).unwrap();
        assert!(!bs.is_empty());

        let decoded = de
            .DecodeAndEvalRowWithMap(&context, &Handle::Int(i as i64), &bs, HashMap::new())
            .unwrap();

        // The final handle column lives in the key, not the payload; Go skips
        // comparing that column against the encoded output slice.
        for (col_id, expected) in &row.output {
            match expected {
                Some(value) => {
                    assert_eq!(decoded.get(col_id), Some(value), "col {col_id}");
                }
                None => {
                    assert!(
                        !decoded.contains_key(col_id),
                        "col {col_id} should come from default only"
                    );
                    let col = columns
                        .iter()
                        .find(|column| column.column_info.id == *col_id)
                        .unwrap();
                    assert_ne!(
                        col.default_value,
                        Datum::Null,
                        "missing map entry requires a column default"
                    );
                }
            }
        }

        let decoded_no_gen = de_with_no_gen_cols
            .DecodeAndEvalRowWithMap(&context, &Handle::Int(i as i64), &bs, HashMap::new())
            .unwrap();
        for (key, value) in &decoded_no_gen {
            let with_gen = decoded.get(key).expect("no-gen result must be a subset");
            assert_eq!(value, with_gen, "col {key}");
        }
    }
}

/// TestClusterIndexRowDecoder covers common-handle decoding from Go's
/// TestClusterIndexRowDecoder.
/// 覆盖聚集索引（common handle）行解码。
#[test]
fn TestClusterIndexRowDecoder() {
    let c1 = column(1, 0, FieldKind::Int, Datum::Null);
    let c2 = column(2, 1, FieldKind::String, Datum::Null);
    let c3 = column(3, 2, FieldKind::Int, Datum::Null);
    let columns = vec![Arc::clone(&c1), Arc::clone(&c2), Arc::clone(&c3)];

    let mut decode_cols_map = HashMap::new();
    for col in &columns {
        decode_cols_map.insert(col.column_info.id, decode_column(Arc::clone(col), None));
    }

    let table = Table {
        meta: TableMeta {
            is_common_handle: true,
            common_pk_column_ids: vec![1, 2],
            ..TableMeta::default()
        },
    };
    let mut de = NewRowDecoder(table, columns, decode_cols_map);
    let context = BuildContext {
        time_zone: "UTC".to_owned(),
    };

    let input = vec![
        (1, Datum::Int(100)),
        (2, Datum::String("abc".to_owned())),
        (3, Datum::Int(1)),
    ];
    let bs = EncodeRow(&input, true).unwrap();
    assert!(!bs.is_empty());

    let handle = Handle::Common(vec![Datum::Int(100), Datum::String("abc".to_owned())]);
    let decoded = de
        .DecodeAndEvalRowWithMap(&context, &handle, &bs, HashMap::new())
        .unwrap();

    assert_eq!(decoded.get(&1), Some(&Datum::Int(100)));
    assert_eq!(decoded.get(&2), Some(&Datum::String("abc".to_owned())));
    assert_eq!(decoded.get(&3), Some(&Datum::Int(1)));
}

/// Go indexes `schema.Columns[col.Offset]` directly and rejects a schema that
/// cannot cover every table-column offset instead of silently dropping the
/// generated expression.
#[test]
#[should_panic]
fn build_full_decode_col_map_rejects_short_schema_like_go() {
    let columns = vec![column(1, 1, FieldKind::Int, Datum::Null)];
    let schema = Schema {
        columns: vec![SchemaColumn::default()],
    };

    let _ = BuildFullDecodeColMap(&columns, &schema);
}
