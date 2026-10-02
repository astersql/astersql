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

use crate::builder::*;
use crate::func_max_min_count::*;
use crate::{AggFunc, SerializeHelper, Serializer};
use astersql_expression::{Column, Expression};
use astersql_expression_exprstatic::NewEvalContext;
use astersql_types::{datum::*, field::FieldType as SqlType};
use astersql_util_serialization::{chunk, types};

fn sql_type(tp: u8) -> SqlType {
    *types::NewFieldType(tp)
}

fn rows(values: Vec<Datum>) -> Vec<chunk::Row> {
    values
        .into_iter()
        .map(|v| {
            chunk::mutrow::MutRowFromDatums(vec![v])
                .ToRow()
                .CopyConstruct()
        })
        .collect()
}
fn descriptor(name: FunctionName, kind: FieldKind, eval: EvalType, mode: AggMode) -> AggFuncDesc {
    AggFuncDesc {
        name,
        mode,
        has_distinct: false,
        args: vec![ArgDesc::typed(crate::builder::FieldType::new(kind, eval))],
        return_type: crate::builder::FieldType::new(FieldKind::LongLong, EvalType::Int),
        order_by_items: vec![],
    }
}
fn build_function(desc: &AggFuncDesc, ft: SqlType, window: bool) -> Box<dyn CountExtremaAgg> {
    let metadata = if window {
        build_window_function(AggFuncBuildContext::default(), desc, 0)
    } else {
        build(AggFuncBuildContext::default(), desc, 0)
    }
    .unwrap();
    let collation = ft.GetCollate().to_owned();
    metadata
        .instantiate_count_extrema(vec![Box::new(Column::new(ft, 1, 1, 0))], &collation)
        .unwrap()
}
fn output(f: &dyn CountExtremaAgg, state: &crate::PartialResult) -> i64 {
    let ctx = NewEvalContext(vec![]);
    let mut chk = chunk::NewChunkWithCapacity(vec![sql_type(8)], 1);
    f.append_final_result_to_chunk(&ctx, state, &mut chk)
        .unwrap();
    chk.GetRow(0).GetInt64(0)
}
#[test]
fn count_extrema_real_rows_all_types_merge_reset_and_spill() {
    let samples = vec![
        (
            FieldKind::LongLong,
            EvalType::Int,
            8,
            vec![NewIntDatum(0), NewIntDatum(4)],
        ),
        (
            FieldKind::Float,
            EvalType::Real,
            4,
            vec![NewFloat32Datum(0.), NewFloat32Datum(4.)],
        ),
        (
            FieldKind::Double,
            EvalType::Real,
            5,
            vec![NewFloat64Datum(0.), NewFloat64Datum(4.)],
        ),
        (
            FieldKind::NewDecimal,
            EvalType::Decimal,
            246,
            vec![
                NewDecimalDatum(astersql_types::decimal::mydecimal::NewDecFromInt(0)),
                NewDecimalDatum(astersql_types::decimal::mydecimal::NewDecFromInt(4)),
            ],
        ),
        (
            FieldKind::String,
            EvalType::String,
            254,
            vec![NewStringDatum("a".into()), NewStringDatum("z".into())],
        ),
        (
            FieldKind::Date,
            EvalType::Datetime,
            10,
            vec![
                NewTimeDatum(astersql_types::time::NewTime(
                    astersql_types::time::FromDate(2026, 1, 1, 0, 0, 0, 0),
                    10,
                    0,
                )),
                NewTimeDatum(astersql_types::time::NewTime(
                    astersql_types::time::FromDate(2026, 1, 2, 0, 0, 0, 0),
                    10,
                    0,
                )),
            ],
        ),
        (
            FieldKind::Duration,
            EvalType::Duration,
            11,
            vec![
                NewDurationDatum(types::Duration {
                    Duration: 0,
                    Fsp: 0,
                }),
                NewDurationDatum(types::Duration {
                    Duration: 4,
                    Fsp: 6,
                }),
            ],
        ),
        (
            FieldKind::Json,
            EvalType::Json,
            245,
            vec![
                NewJSONDatum(types::CreateBinaryJSON(0_i64)),
                NewJSONDatum(types::CreateBinaryJSON(4_i64)),
            ],
        ),
        (
            FieldKind::Enum,
            EvalType::String,
            247,
            vec![
                NewMysqlEnumDatum(types::Enum {
                    Name: "a".into(),
                    Value: 9,
                }),
                NewMysqlEnumDatum(types::Enum {
                    Name: "z".into(),
                    Value: 1,
                }),
            ],
        ),
        (
            FieldKind::Set,
            EvalType::String,
            248,
            vec![
                NewMysqlSetDatum(
                    types::Set {
                        Name: "a".into(),
                        Value: 9,
                    },
                    "binary".into(),
                ),
                NewMysqlSetDatum(
                    types::Set {
                        Name: "z".into(),
                        Value: 1,
                    },
                    "binary".into(),
                ),
            ],
        ),
        (
            FieldKind::Bit,
            EvalType::Int,
            16,
            vec![
                NewBinaryLiteralDatum(types::BinaryLiteral(vec![0])),
                NewBinaryLiteralDatum(types::BinaryLiteral(vec![4])),
            ],
        ),
        (
            FieldKind::VectorFloat32,
            EvalType::VectorFloat32,
            225,
            vec![
                NewVectorFloat32Datum(astersql_types::vector::MustCreateVectorFloat32(&[0., 0.])),
                NewVectorFloat32Datum(astersql_types::vector::MustCreateVectorFloat32(&[4., 4.])),
            ],
        ),
    ];
    let ctx = NewEvalContext(vec![]);
    for (kind, eval, tp, data) in samples {
        for name in [FunctionName::MaxCount, FunctionName::MinCount] {
            let f = build_function(
                &descriptor(name, kind, eval, AggMode::Complete),
                sql_type(tp),
                false,
            );
            let (mut state, allocated) = f.alloc_partial_result();
            assert!(allocated > 0);
            assert_eq!(output(f.as_ref(), &state), 0);
            let input = rows(vec![
                data[0].clone(),
                data[0].clone(),
                data[1].clone(),
                data[1].clone(),
                data[1].clone(),
                Datum::default(),
            ]);
            let delta = f.update_partial_result(&ctx, &input, &mut state).unwrap();
            let selected = if name == FunctionName::MaxCount {
                &data[1]
            } else {
                &data[0]
            };
            let retained = match kind {
                FieldKind::String => selected.GetString().len() as i64,
                FieldKind::Json => selected.GetMysqlJSON().Value.len() as i64,
                FieldKind::Enum => selected.GetMysqlEnum().Name.len() as i64,
                FieldKind::Set => selected.GetMysqlSet().Name.len() as i64,
                FieldKind::VectorFloat32 => {
                    selected.GetVectorFloat32().SerializedSize() as i64
                        - types::VectorFloat32::default().SerializedSize() as i64
                }
                FieldKind::Bit => delta,
                _ => 0,
            };
            assert_eq!(delta, retained, "memory delta for {kind:?} {name:?}");
            macro_rules! check_layout {
                ($ty:ty) => {
                    assert_eq!(allocated, std::mem::size_of::<CountPartial<$ty>>() as i64)
                };
            }
            match kind {
                FieldKind::LongLong => check_layout!(i64),
                FieldKind::Float => check_layout!(f32),
                FieldKind::Double => check_layout!(f64),
                FieldKind::NewDecimal => check_layout!(types::MyDecimal),
                FieldKind::Date => check_layout!(types::Time),
                FieldKind::Duration => check_layout!(types::Duration),
                FieldKind::String | FieldKind::Bit => check_layout!(String),
                FieldKind::Json => check_layout!(types::BinaryJSON),
                FieldKind::Enum => check_layout!(types::Enum),
                FieldKind::Set => check_layout!(types::Set),
                FieldKind::VectorFloat32 => check_layout!(types::VectorFloat32),
                _ => unreachable!(),
            }

            let expected = if name == FunctionName::MaxCount { 3 } else { 2 };
            assert_eq!(output(f.as_ref(), &state), expected, "{kind:?} {name:?}");
            let (mut merged, _) = f.alloc_partial_result();
            f.merge_partial_result(&ctx, &state, &mut merged).unwrap();
            f.merge_partial_result(&ctx, &state, &mut merged).unwrap();
            assert_eq!(output(f.as_ref(), &merged), expected * 2);
            let mut spill = chunk::NewChunkWithCapacity(vec![sql_type(252)], 1);
            f.serialize_partial_result(&merged, &mut spill, &mut SerializeHelper::new());
            let (restored, memory) = f.deserialize_partial_result(&spill);
            assert_eq!(restored.len(), 1);
            assert!(memory >= allocated);
            assert_eq!(output(f.as_ref(), &restored[0]), expected * 2);
            f.reset_partial_result(&mut state);
            assert_eq!(output(f.as_ref(), &state), 0);
            if !matches!(
                kind,
                FieldKind::Enum | FieldKind::Set | FieldKind::Json | FieldKind::VectorFloat32
            ) {
                let mut window = build_function(
                    &descriptor(name, kind, eval, AggMode::Complete),
                    sql_type(tp),
                    true,
                );
                window.set_window_start(10);
                let (mut window_state, _) = window.alloc_partial_result();
                window
                    .update_partial_result(&ctx, &input[..2], &mut window_state)
                    .unwrap();
                assert_eq!(output(window.as_ref(), &window_state), 2);
                window
                    .slide(
                        &ctx,
                        &mut |index| input[(index - 10) as usize].clone(),
                        10,
                        12,
                        2,
                        3,
                        &mut window_state,
                    )
                    .unwrap();
                assert_eq!(output(window.as_ref(), &window_state), 3);
                window
                    .slide(
                        &ctx,
                        &mut |index| input[(index - 10) as usize].clone(),
                        12,
                        15,
                        3,
                        1,
                        &mut window_state,
                    )
                    .unwrap();
                assert_eq!(output(window.as_ref(), &window_state), 0);
            }
        }
    }
}
#[test]
fn count_extrema_sliding_duplicate_indices_expire_individually() {
    let ctx = NewEvalContext(vec![]);
    let input = rows(
        [Some(1), Some(1), Some(2), Some(2), None, Some(2), Some(1)]
            .into_iter()
            .map(|v| v.map_or_else(Datum::default, NewIntDatum))
            .collect(),
    );
    for name in [FunctionName::MaxCount, FunctionName::MinCount] {
        let mut f = build_function(
            &descriptor(name, FieldKind::LongLong, EvalType::Int, AggMode::Complete),
            sql_type(8),
            true,
        );
        f.set_window_start(0);
        let (mut state, _) = f.alloc_partial_result();
        f.update_partial_result(&ctx, &input[..1], &mut state)
            .unwrap();
        let mut values = vec![output(f.as_ref(), &state)];
        for end in 1..input.len() {
            let start = end.saturating_sub(2);
            let shift = if end > 1 { 1 } else { 0 };
            f.slide(
                &ctx,
                &mut |i| input[i as usize].clone(),
                start as u64,
                end as u64,
                shift,
                1,
                &mut state,
            )
            .unwrap();
            values.push(output(f.as_ref(), &state));
        }
        assert_eq!(values, vec![1, 2, 1, 2, 1, 1, 1]);
    }
}
#[test]
fn count_extrema_final_rejects_rows_but_merges_states() {
    let ctx = NewEvalContext(vec![]);
    for mode in [AggMode::Final, AggMode::Partial2] {
        for name in [FunctionName::MaxCount, FunctionName::MinCount] {
            let mut d = descriptor(name, FieldKind::LongLong, EvalType::Int, mode);
            d.args.push(d.args[0].clone());
            let f = build_function(&d, sql_type(8), false);
            let (mut state, _) = f.alloc_partial_result();
            let error = f
                .update_partial_result(&ctx, &rows(vec![NewIntDatum(10)]), &mut state)
                .unwrap_err();
            assert!(error.0.contains("row-based final aggregation for"));
            assert_eq!(output(f.as_ref(), &state), 0);
            let partial = build_function(
                &descriptor(name, FieldKind::LongLong, EvalType::Int, AggMode::Partial1),
                sql_type(8),
                false,
            );
            let (mut source, _) = partial.alloc_partial_result();
            partial
                .update_partial_result(
                    &ctx,
                    &rows(vec![NewIntDatum(2), NewIntDatum(2)]),
                    &mut source,
                )
                .unwrap();
            f.merge_partial_result(&ctx, &source, &mut state).unwrap();
            assert_eq!(output(f.as_ref(), &state), 2);
        }
    }
}

#[test]
fn count_extrema_unsigned_collation_memory_and_parallel_merge() {
    let ctx = NewEvalContext(vec![]);
    let mut unsigned = descriptor(
        FunctionName::MaxCount,
        FieldKind::LongLong,
        EvalType::Int,
        AggMode::Complete,
    );
    unsigned.args[0].field_type.unsigned = true;
    let mut ft = sql_type(8);
    ft.SetFlag(astersql_expression::mysql::UnsignedFlag);
    let f = build_function(&unsigned, ft, false);
    let (mut state, _) = f.alloc_partial_result();
    f.update_partial_result(
        &ctx,
        &rows(vec![
            NewUintDatum(1),
            NewUintDatum(u64::MAX),
            NewUintDatum(u64::MAX),
        ]),
        &mut state,
    )
    .unwrap();
    assert_eq!(output(f.as_ref(), &state), 2);
    let mut ft = sql_type(254);
    ft.SetCharset("utf8mb4".into());
    ft.SetCollate("utf8mb4_general_ci".into());
    let f = build_function(
        &descriptor(
            FunctionName::MaxCount,
            FieldKind::String,
            EvalType::String,
            AggMode::Complete,
        ),
        ft,
        false,
    );
    let (mut state, _) = f.alloc_partial_result();
    assert_eq!(
        f.update_partial_result(
            &ctx,
            &rows(vec![
                NewStringDatum("a".into()),
                NewStringDatum("ZZ".into())
            ]),
            &mut state
        )
        .unwrap(),
        2
    );
    assert_eq!(
        f.update_partial_result(&ctx, &rows(vec![NewStringDatum("zz".into())]), &mut state)
            .unwrap(),
        0
    );
    assert_eq!(output(f.as_ref(), &state), 2);
    for name in [FunctionName::MaxCount, FunctionName::MinCount] {
        let mut workers = Vec::new();
        for _ in 0..4 {
            workers.push(std::thread::spawn(move || {
                let ctx = NewEvalContext(vec![]);
                let f = build_function(
                    &descriptor(name, FieldKind::LongLong, EvalType::Int, AggMode::Partial1),
                    sql_type(8),
                    false,
                );
                let (mut state, _) = f.alloc_partial_result();
                f.update_partial_result(
                    &ctx,
                    &rows(vec![
                        NewIntDatum(0),
                        NewIntDatum(0),
                        NewIntDatum(1),
                        NewIntDatum(1),
                        NewIntDatum(1),
                        Datum::default(),
                    ]),
                    &mut state,
                )
                .unwrap();
                state
            }));
        }
        let f = build_function(
            &descriptor(name, FieldKind::LongLong, EvalType::Int, AggMode::Final),
            sql_type(8),
            false,
        );
        let (mut state, _) = f.alloc_partial_result();
        for worker in workers {
            f.merge_partial_result(&ctx, &worker.join().unwrap(), &mut state)
                .unwrap();
        }
        assert_eq!(
            output(f.as_ref(), &state),
            if name == FunctionName::MaxCount {
                12
            } else {
                8
            }
        );
    }
}

#[test]
fn count_extrema_merge_replaces_worse_values_and_ignores_null_sources() {
    let ctx = NewEvalContext(vec![]);
    for name in [FunctionName::MaxCount, FunctionName::MinCount] {
        let f = build_function(
            &descriptor(name, FieldKind::LongLong, EvalType::Int, AggMode::Complete),
            sql_type(8),
            false,
        );
        let (mut destination, _) = f.alloc_partial_result();
        let best = if name == FunctionName::MaxCount {
            9
        } else {
            -9
        };
        f.update_partial_result(
            &ctx,
            &rows(vec![NewIntDatum(0), NewIntDatum(0)]),
            &mut destination,
        )
        .unwrap();
        let (mut source, _) = f.alloc_partial_result();
        f.merge_partial_result(&ctx, &source, &mut destination)
            .unwrap();
        assert_eq!(output(f.as_ref(), &destination), 2);
        f.update_partial_result(
            &ctx,
            &rows(vec![
                NewIntDatum(best),
                NewIntDatum(best),
                NewIntDatum(best),
            ]),
            &mut source,
        )
        .unwrap();
        f.merge_partial_result(&ctx, &source, &mut destination)
            .unwrap();
        assert_eq!(output(f.as_ref(), &destination), 3);
        f.reset_partial_result(&mut source);
        f.update_partial_result(&ctx, &rows(vec![NewIntDatum(0)]), &mut source)
            .unwrap();
        f.merge_partial_result(&ctx, &source, &mut destination)
            .unwrap();
        assert_eq!(output(f.as_ref(), &destination), 3);
        let (empty, _) = f.alloc_partial_result();
        let mut spill = chunk::NewChunkWithCapacity(vec![sql_type(252)], 1);
        let mut helper = SerializeHelper::new();
        f.serialize_partial_result(&destination, &mut spill, &mut helper);
        f.serialize_partial_result(&empty, &mut spill, &mut helper);
        let (restored, _) = f.deserialize_partial_result(&spill);
        assert_eq!(restored.len(), 2);
        assert_eq!(output(f.as_ref(), &restored[0]), 3);
        assert_eq!(output(f.as_ref(), &restored[1]), 0);
    }
}
