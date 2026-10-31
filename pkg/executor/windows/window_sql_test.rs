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

//! 窗口执行器的 SQL 语义回归测试。
//!
//! 这些用例直接调用 crate 的执行器 API，复现 `window_sql_test.go` 中与执行阶段相关的
//! 场景，覆盖分区、`ROWS`/`RANGE` 帧、流水线模式、跨数据块引用及空帧返回值等约束。

use super::window_executor_test::{build_plan, run_executor};
use super::{BoundType, Chunk, ExecContext, FrameBound, FrameType, OrderBy, Value, WindowFrame};

/// 构造单列排序规则，供窗口帧按升序或降序解释边界。
fn order(column: usize, descending: bool) -> Vec<OrderBy> {
    vec![OrderBy { column, descending }]
}

/// 构造 `CURRENT ROW` 边界，避免各用例重复填写其余默认字段。
fn current_row() -> FrameBound {
    let mut bound = FrameBound::default();
    bound.bound_type = BoundType::CurrentRow;
    bound
}

/// 从窗口计划构建并完整执行测试数据，返回原始列与窗口函数结果列。
///
/// 输入列数必须从首行推导；空输入则按零列计划处理。`pipelined` 用于让同一语义用例
/// 同时约束传统执行器和流水线执行器。
fn execute(
    rows: Vec<Vec<Value>>,
    partition_by: Vec<usize>,
    order_by: Vec<OrderBy>,
    frame: Option<WindowFrame>,
    functions: Vec<Box<dyn super::WindowFunction>>,
    pipelined: bool,
) -> Vec<Vec<Value>> {
    let input_columns = rows.first().map_or(0, Vec::len);
    let executor = super::build(
        build_plan(
            input_columns,
            partition_by,
            order_by,
            frame,
            functions,
            pipelined,
        ),
        Box::new(super::VecChunkExecutor::new(vec![Chunk::new(rows)])),
        false,
    )
    .unwrap();
    run_executor(executor, &ExecContext::default()).unwrap()
}

/// 将整数序列转换为单列输入，便于明确表达按值排序的窗口帧用例。
fn sorted_rows(values: &[i64]) -> Vec<Vec<Value>> {
    values
        .iter()
        .map(|value| vec![Value::Int(*value)])
        .collect()
}

/// 执行单个窗口函数并只保留结果列，便于逐项复现 Go 滑动窗口用例。
fn window_values(
    values: &[i64],
    frame_type: FrameType,
    start: FrameBound,
    end: FrameBound,
    function: Box<dyn super::WindowFunction>,
    pipelined: bool,
) -> Vec<Value> {
    execute(
        sorted_rows(values),
        Vec::new(),
        order(0, false),
        Some(WindowFrame {
            frame_type,
            start,
            end,
        }),
        vec![function],
        pipelined,
    )
    .into_iter()
    .map(|row| row[1].clone())
    .collect()
}

#[test]
/// 覆盖分区、`ROWS` 与双向 `RANGE` 边界，并校验两种执行模式的结果一致性。
fn go_test_window_functions_covers_partition_rows_and_range_frames() {
    let rows = vec![
        vec![Value::Int(1), Value::Int(2)],
        vec![Value::Int(4), Value::Int(3)],
        vec![Value::Int(2), Value::Int(3)],
    ];
    // 相同输入在传统与流水线执行路径上必须保持完全一致的分区和帧语义。
    for pipelined in [false, true] {
        let result = execute(
            rows.clone(),
            vec![0],
            vec![],
            None,
            vec![
                Box::new(super::Sum::new(0)),
                Box::new(super::RowNumber::default()),
            ],
            pipelined,
        );
        assert_eq!(
            result,
            vec![
                vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Real(1.0),
                    Value::UInt(1)
                ],
                vec![
                    Value::Int(4),
                    Value::Int(3),
                    Value::Real(4.0),
                    Value::UInt(1)
                ],
                vec![
                    Value::Int(2),
                    Value::Int(3),
                    Value::Real(2.0),
                    Value::UInt(1)
                ],
            ]
        );

        let rows_frame = WindowFrame {
            frame_type: FrameType::Rows,
            start: FrameBound::preceding(1),
            end: current_row(),
        };
        let result = execute(
            rows.clone(),
            Vec::new(),
            vec![],
            Some(rows_frame),
            vec![Box::new(super::Sum::new(0))],
            pipelined,
        );
        assert_eq!(
            result,
            vec![
                vec![Value::Int(1), Value::Int(2), Value::Real(1.0)],
                vec![Value::Int(4), Value::Int(3), Value::Real(5.0)],
                vec![Value::Int(2), Value::Int(3), Value::Real(6.0)],
            ]
        );
    }

    // RANGE 的 preceding/following 基于排序值而非物理行位置，且降序时方向也要反转。
    let range_frame = WindowFrame {
        frame_type: FrameType::Range,
        start: FrameBound::preceding(1),
        end: FrameBound::following(2),
    };
    assert_eq!(
        execute(
            sorted_rows(&[1, 2, 3, 5]),
            Vec::new(),
            order(0, false),
            Some(range_frame.clone()),
            vec![Box::new(super::Sum::new(0))],
            false,
        )
        .into_iter()
        .map(|row| row[1].clone())
        .collect::<Vec<_>>(),
        vec![
            Value::Real(6.0),
            Value::Real(6.0),
            Value::Real(10.0),
            Value::Real(5.0)
        ]
    );
    assert_eq!(
        execute(
            sorted_rows(&[5, 3, 2, 1]),
            Vec::new(),
            order(0, true),
            Some(range_frame),
            vec![Box::new(super::Sum::new(0))],
            true,
        )
        .into_iter()
        .map(|row| row[1].clone())
        .collect::<Vec<_>>(),
        vec![
            Value::Real(8.0),
            Value::Real(6.0),
            Value::Real(6.0),
            Value::Real(3.0)
        ]
    );
}

#[test]
/// 验证流水线跨 `Chunk` 取前一行时不会丢失 `LAG` 的数据引用。
fn go_test_window_functions_data_reference_preserves_lag_across_chunks() {
    let executor = super::build(
        build_plan(
            1,
            Vec::new(),
            order(0, false),
            None,
            vec![Box::new(super::Lag::new(0, 1, Value::Null))],
            true,
        ),
        Box::new(super::VecChunkExecutor::new(vec![
            Chunk::new(sorted_rows(&[1, 2])),
            Chunk::new(sorted_rows(&[3])),
        ])),
        false,
    )
    .unwrap();
    assert_eq!(
        run_executor(executor, &ExecContext::default()).unwrap(),
        vec![
            vec![Value::Int(1), Value::Null],
            vec![Value::Int(2), Value::Int(1)],
            vec![Value::Int(3), Value::Int(2)],
        ]
    );
}

#[test]
/// 验证滑动 `ROWS` 帧内各聚合函数在传统与流水线模式下同步增删帧成员。
fn go_test_sliding_window_functions_matches_rows_and_pipeline_modes() {
    let frame = WindowFrame {
        frame_type: FrameType::Rows,
        start: FrameBound::preceding(1),
        end: FrameBound::following(1),
    };
    for pipelined in [false, true] {
        let result = execute(
            sorted_rows(&[1, 2, 3, 4, 5]),
            Vec::new(),
            order(0, false),
            Some(frame.clone()),
            vec![
                Box::new(super::Sum::new(0)),
                Box::new(super::Average::new(0)),
                Box::new(super::MinValue::new(0)),
                Box::new(super::MaxValue::new(0)),
                Box::new(super::BitXor::new(0)),
            ],
            pipelined,
        );
        assert_eq!(
            result,
            vec![
                vec![
                    Value::Int(1),
                    Value::Real(3.0),
                    Value::Real(1.5),
                    Value::Int(1),
                    Value::Int(2),
                    Value::UInt(3)
                ],
                vec![
                    Value::Int(2),
                    Value::Real(6.0),
                    Value::Real(2.0),
                    Value::Int(1),
                    Value::Int(3),
                    Value::UInt(0)
                ],
                vec![
                    Value::Int(3),
                    Value::Real(9.0),
                    Value::Real(3.0),
                    Value::Int(2),
                    Value::Int(4),
                    Value::UInt(5)
                ],
                vec![
                    Value::Int(4),
                    Value::Real(12.0),
                    Value::Real(4.0),
                    Value::Int(3),
                    Value::Int(5),
                    Value::UInt(2)
                ],
                vec![
                    Value::Int(5),
                    Value::Real(9.0),
                    Value::Real(4.5),
                    Value::Int(4),
                    Value::Int(5),
                    Value::UInt(1)
                ],
            ]
        );
    }
}

#[test]
/// 复现 Go 参数化滑动用例中的非对称与反向边界，约束空帧返回值和两种执行路径。
fn go_test_sliding_window_functions_covers_asymmetric_and_empty_frames() {
    let values = [1, 2, 3, 4, 5, 10, 11];
    for pipelined in [false, true] {
        assert_eq!(
            window_values(
                &values,
                FrameType::Rows,
                FrameBound::following(1),
                FrameBound::following(2),
                Box::new(super::CountRows::default()),
                pipelined,
            ),
            vec![2, 2, 2, 2, 2, 1, 0]
                .into_iter()
                .map(Value::UInt)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            window_values(
                &values,
                FrameType::Rows,
                FrameBound::preceding(2),
                FrameBound::preceding(1),
                Box::new(super::Sum::new(0)),
                pipelined,
            ),
            vec![
                Value::Null,
                Value::Real(1.0),
                Value::Real(3.0),
                Value::Real(5.0),
                Value::Real(7.0),
                Value::Real(9.0),
                Value::Real(15.0),
            ]
        );
        assert_eq!(
            window_values(
                &values,
                FrameType::Rows,
                FrameBound::following(3),
                FrameBound::following(1),
                Box::new(super::Sum::new(0)),
                pipelined,
            ),
            vec![Value::Null; values.len()]
        );
        assert_eq!(
            window_values(
                &values,
                FrameType::Range,
                FrameBound::following(1),
                FrameBound::following(2),
                Box::new(super::Sum::new(0)),
                pipelined,
            ),
            vec![
                Value::Real(5.0),
                Value::Real(7.0),
                Value::Real(9.0),
                Value::Real(5.0),
                Value::Null,
                Value::Real(11.0),
                Value::Null,
            ]
        );
    }
}

#[test]
/// 回归空帧的可空性：`SUM` 返回 NULL、`COUNT` 返回零，行号仍按分区位置递增。
fn go_test_issue_45964_and_46050_preserves_empty_frame_nullability() {
    let result = execute(
        vec![
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(1), Value::Int(20)],
            vec![Value::Int(1), Value::Int(30)],
        ],
        vec![0],
        order(1, false),
        Some(WindowFrame {
            frame_type: FrameType::Rows,
            start: FrameBound::following(1),
            end: FrameBound::following(1),
        }),
        vec![
            Box::new(super::Sum::new(1)),
            Box::new(super::CountRows::default()),
            Box::new(super::RowNumber::default()),
        ],
        false,
    );
    assert_eq!(
        result[0][2..],
        [Value::Real(20.0), Value::UInt(1), Value::UInt(1)]
    );
    assert_eq!(
        result[1][2..],
        [Value::Real(30.0), Value::UInt(1), Value::UInt(2)]
    );
    assert_eq!(
        result[2][2..],
        [Value::Null, Value::UInt(0), Value::UInt(3)]
    );
}

#[test]
/// 校验 `VAR_SAMP` 使用整个分区，并在样本数不足两个时返回 NULL。
fn go_test_var_samp_as_a_window_function_returns_null_for_small_samples() {
    let result = execute(
        sorted_rows(&[1, 2, 4]),
        Vec::new(),
        order(0, false),
        None,
        vec![Box::new(super::VarSamp::new(0))],
        false,
    );
    for row in &result {
        let Value::Real(value) = row[1] else {
            panic!("VAR_SAMP should return a numeric value");
        };
        assert!((value - 7.0 / 3.0).abs() < 1e-12);
    }
    let single = execute(
        sorted_rows(&[1]),
        Vec::new(),
        order(0, false),
        None,
        vec![Box::new(super::VarSamp::new(0))],
        false,
    );
    assert_eq!(single[0][1], Value::Null);
}

#[test]
/// 确认规范值模型不会混淆 SQL NULL、整数零与不同标度表示的等值十进制数。
fn canonical_window_values_keep_sql_null_and_decimal_types_distinct() {
    assert_ne!(Value::Null, Value::Int(0));
    assert_eq!(super::Decimal::new(120, 2), super::Decimal::new(12, 1));
    assert_eq!(
        execute(
            vec![vec![Value::Decimal(super::Decimal::new(120, 2))]],
            Vec::new(),
            vec![],
            None,
            vec![Box::new(super::DecimalSum::new(0))],
            false,
        )[0][1],
        Value::Decimal(super::Decimal::new(120, 2))
    );
}

#[test]
fn pg09_12_execution() {
    for pipelined in [false, true] {
        assert!(
            execute(
                vec![],
                vec![],
                vec![],
                None,
                vec![Box::new(super::Rank::default())],
                pipelined
            )
            .is_empty()
        );
        let nulls = execute(
            vec![vec![Value::Null], vec![Value::Null]],
            vec![],
            order(0, false),
            None,
            vec![Box::new(super::Rank::default())],
            pipelined,
        );
        assert_eq!(
            nulls.iter().map(|r| r[1].clone()).collect::<Vec<_>>(),
            vec![Value::Int(1), Value::Int(1)]
        );
        let result = execute(
            vec![
                vec![Value::Int(1), Value::Int(10)],
                vec![Value::Int(1), Value::Int(10)],
                vec![Value::Int(1), Value::Int(20)],
                vec![Value::Int(2), Value::Null],
            ],
            vec![0],
            order(1, false),
            None,
            vec![Box::new(super::RowNumber::default())],
            pipelined,
        );
        assert_eq!(
            result.iter().map(|r| r[2].clone()).collect::<Vec<_>>(),
            vec![
                Value::UInt(1),
                Value::UInt(2),
                Value::UInt(3),
                Value::UInt(1)
            ]
        );
        let result = execute(
            vec![
                vec![Value::Int(10)],
                vec![Value::Int(10)],
                vec![Value::Int(20)],
            ],
            vec![],
            order(0, false),
            None,
            vec![Box::new(super::Rank::default())],
            pipelined,
        );
        assert_eq!(
            result.iter().map(|r| r[1].clone()).collect::<Vec<_>>(),
            vec![Value::Int(1), Value::Int(1), Value::Int(3)]
        );
    }
}

#[test]
fn pg09_13_execution() {
    for pipelined in [false, true] {
        for (rows, frame, expected) in [
            (
                sorted_rows(&[10, 10, 20]),
                Some(WindowFrame {
                    frame_type: FrameType::Range,
                    start: FrameBound::unbounded(BoundType::Preceding),
                    end: current_row(),
                }),
                vec![Value::Int(10), Value::Int(10), Value::Int(20)],
            ),
            (sorted_rows(&[10, 10, 20]), None, vec![Value::Int(20); 3]),
            (
                sorted_rows(&[10, 10, 20]),
                Some(WindowFrame {
                    frame_type: FrameType::Rows,
                    start: current_row(),
                    end: current_row(),
                }),
                vec![Value::Int(10), Value::Int(10), Value::Int(20)],
            ),
            (
                sorted_rows(&[10]),
                Some(WindowFrame {
                    frame_type: FrameType::Rows,
                    start: FrameBound::following(1),
                    end: FrameBound::following(1),
                }),
                vec![Value::Null],
            ),
            (
                vec![vec![Value::Int(10)], vec![Value::Null]],
                None,
                vec![Value::Null; 2],
            ),
            (vec![vec![Value::Null]; 2], None, vec![Value::Null; 2]),
            (vec![], None, vec![]),
        ] {
            let result = execute(
                rows,
                vec![],
                order(0, false),
                frame,
                vec![Box::new(super::LastValue::new(0))],
                pipelined,
            );
            assert_eq!(
                result.into_iter().map(|r| r[1].clone()).collect::<Vec<_>>(),
                expected,
                "pipelined={pipelined}"
            );
        }
    }
}

#[test]
fn pg09_13_execution_chunks_and_cleanup() {
    for pipelined in [false, true] {
        let plan = build_plan(
            2,
            vec![],
            order(0, false),
            Some(WindowFrame {
                frame_type: FrameType::Range,
                start: FrameBound::unbounded(BoundType::Preceding),
                end: current_row(),
            }),
            vec![Box::new(super::LastValue::new(1))],
            pipelined,
        );
        let chunks = vec![
            Chunk::new(vec![vec![Value::Int(10), Value::Int(1)]]),
            Chunk::new(vec![
                vec![Value::Int(10), Value::Int(2)],
                vec![Value::Int(20), Value::Null],
            ]),
        ];
        let mut executor =
            super::build(plan, Box::new(super::VecChunkExecutor::new(chunks)), false).unwrap();
        let context = ExecContext::default();
        executor.open(&context).unwrap();
        let mut values = vec![];
        let mut output = Chunk::default();
        loop {
            executor.next(&context, &mut output).unwrap();
            if output.rows.is_empty() {
                break;
            }
            values.extend(output.rows.iter().map(|r| r[2].clone()));
        }
        assert_eq!(values, vec![Value::Int(2), Value::Int(2), Value::Null]);
        executor.close().unwrap();
        assert_eq!(executor.memory_bytes(), 0);
    }
}

#[test]
fn pg09_14_execution() {
    for pipelined in [false, true] {
        for descending in [false, true] {
            let mut values = vec![9007199254740994i64, 9007199254740995, 9007199254741001];
            if descending {
                values.reverse();
            }
            let result = execute(
                sorted_rows(&values),
                vec![],
                order(0, descending),
                Some(WindowFrame {
                    frame_type: FrameType::Range,
                    start: current_row(),
                    end: FrameBound::following(1),
                }),
                vec![Box::new(super::LastValue::new(0))],
                pipelined,
            );
            let expected = if descending {
                vec![9007199254741001, 9007199254740994, 9007199254740994]
            } else {
                vec![9007199254740995, 9007199254740995, 9007199254741001]
            };
            assert_eq!(
                result.iter().map(|r| r[1].clone()).collect::<Vec<_>>(),
                expected.into_iter().map(Value::Int).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn pg09_14_execution_chunks_and_cleanup() {
    for pipelined in [false, true] {
        let mut start = current_row();
        start.set_numeric_range("0".parse().unwrap(), false);
        let mut end = FrameBound::following(0);
        end.set_numeric_range("0.1".parse().unwrap(), false);
        let plan = build_plan(
            2,
            vec![],
            order(0, false),
            Some(WindowFrame {
                frame_type: FrameType::Range,
                start,
                end,
            }),
            vec![Box::new(super::LastValue::new(1))],
            pipelined,
        );
        let chunks = vec![
            Chunk::new(vec![vec![
                Value::Text("9007199254740992.1".into()),
                Value::Int(1),
            ]]),
            Chunk::new(vec![
                vec![Value::Text("9007199254740992.2".into()), Value::Int(2)],
                vec![Value::Text("9007199254740992.4".into()), Value::Int(3)],
                vec![Value::Null, Value::Int(4)],
            ]),
        ];
        let mut exec =
            super::build(plan, Box::new(super::VecChunkExecutor::new(chunks)), false).unwrap();
        let cx = ExecContext::default();
        exec.open(&cx).unwrap();
        let mut values = vec![];
        let mut output = Chunk::default();
        loop {
            exec.next(&cx, &mut output).unwrap();
            if output.rows.is_empty() {
                break;
            }
            values.extend(output.rows.iter().map(|r| r[2].clone()));
        }
        assert_eq!(
            values,
            vec![Value::Int(2), Value::Int(2), Value::Int(3), Value::Int(4)]
        );
        exec.close().unwrap();
        assert_eq!(exec.memory_bytes(), 0);
    }
}

#[test]
fn pg09_14_exact_key_domains() {
    for pipelined in [false, true] {
        for keys in [
            vec![
                Value::UInt(u64::MAX - 2),
                Value::UInt(u64::MAX - 1),
                Value::UInt(u64::MAX),
            ],
            vec![
                Value::Decimal(super::Decimal::new(90071992547409941, 1)),
                Value::Decimal(super::Decimal::new(90071992547409951, 1)),
                Value::Decimal(super::Decimal::new(90071992547409971, 1)),
            ],
        ] {
            let expected = if matches!(keys[0], Value::Decimal(_)) {
                vec![Value::Int(1), Value::Int(1), Value::Int(2)]
            } else {
                vec![Value::Int(1), Value::Int(2), Value::Int(2)]
            };
            let input = keys
                .into_iter()
                .enumerate()
                .map(|(i, k)| vec![k, Value::Int(i as i64)])
                .collect();
            let result = execute(
                input,
                vec![],
                order(0, false),
                Some(WindowFrame {
                    frame_type: FrameType::Range,
                    start: current_row(),
                    end: FrameBound::following(1),
                }),
                vec![Box::new(super::LastValue::new(1))],
                pipelined,
            );
            assert_eq!(
                result.iter().map(|r| r[2].clone()).collect::<Vec<_>>(),
                expected
            );
        }
    }
}

#[test]
fn pg09_15_execution() {
    for pipelined in [false, true] {
        let rows = vec![
            vec![Value::Int(10)],
            vec![Value::Null],
            vec![Value::Int(30)],
        ];
        let result = execute(
            rows,
            vec![],
            vec![],
            Some(WindowFrame {
                frame_type: FrameType::Rows,
                start: current_row(),
                end: current_row(),
            }),
            vec![Box::new(super::Lag::new(0, 1, Value::Int(99)))],
            pipelined,
        );
        assert_eq!(
            result.into_iter().map(|r| r[1].clone()).collect::<Vec<_>>(),
            vec![Value::Int(99), Value::Int(10), Value::Null]
        );
    }
}

#[test]
fn pg09_15_execution_row_arguments_chunks_cleanup() {
    for pipelined in [false, true] {
        for lead in [false, true] {
            let input = vec![
                vec![Value::Int(10), Value::Int(1), Value::Int(1)],
                vec![Value::Null, Value::Int(-1), Value::Int(2)],
                vec![Value::Int(30), Value::Int(0), Value::Int(3)],
                vec![Value::Int(40), Value::Null, Value::Int(4)],
            ];
            let plan = build_plan(
                3,
                vec![],
                vec![],
                Some(WindowFrame {
                    frame_type: FrameType::Rows,
                    start: current_row(),
                    end: current_row(),
                }),
                vec![Box::new(super::Lag::with_row_arguments(0, 1, 2, lead))],
                pipelined,
            );
            let mut executor = super::build(
                plan,
                Box::new(super::VecChunkExecutor::new(
                    input.into_iter().map(|r| Chunk::new(vec![r])).collect(),
                )),
                false,
            )
            .unwrap();
            let context = ExecContext::default();
            executor.open(&context).unwrap();
            let mut actual = vec![];
            let mut chunk = Chunk::default();
            loop {
                executor.next(&context, &mut chunk).unwrap();
                if chunk.rows.is_empty() {
                    break;
                }
                actual.extend(chunk.rows.iter().map(|r| r[3].clone()));
            }
            assert_eq!(
                actual,
                if lead {
                    vec![Value::Null, Value::Int(10), Value::Int(30), Value::Null]
                } else {
                    vec![Value::Int(1), Value::Int(30), Value::Int(30), Value::Null]
                }
            );
            executor.close().unwrap();
            assert_eq!(executor.memory_bytes(), 0);
        }
    }
}

#[test]
fn pg09_16_execution() {
    // Session supplies dense peer ordinals after sorting all ORDER BY keys.
    // Exercise that existing RANGE processor contract across chunks/partitions.
    for pipelined in [false, true] {
        for (start, end, expected) in [
            (
                current_row(),
                FrameBound::following(1),
                vec![20, 20, 30, 30, 90],
            ),
            (
                FrameBound::preceding(1),
                current_row(),
                vec![11, 11, 20, 30, 90],
            ),
        ] {
            let plan = build_plan(
                3,
                vec![0],
                order(1, false),
                Some(WindowFrame {
                    frame_type: FrameType::Range,
                    start,
                    end,
                }),
                vec![Box::new(super::LastValue::new(2))],
                pipelined,
            );
            let rows = [(1, 0, 10), (1, 0, 11), (1, 1, 20), (1, 2, 30), (2, 0, 90)]
                .into_iter()
                .map(|(p, k, v)| vec![Value::Int(p), Value::Int(k), Value::Int(v)])
                .collect::<Vec<_>>();
            let chunks = rows.chunks(1).map(|r| Chunk::new(r.to_vec())).collect();
            let mut exec =
                super::build(plan, Box::new(super::VecChunkExecutor::new(chunks)), false).unwrap();
            let cx = ExecContext::default();
            {
                exec.open(&cx).unwrap();
                let mut actual = vec![];
                let mut chunk = Chunk::default();
                loop {
                    exec.next(&cx, &mut chunk).unwrap();
                    if chunk.rows.is_empty() {
                        break;
                    }
                    actual.extend(chunk.rows.iter().map(|r| r[3].clone()));
                }
                assert_eq!(
                    actual,
                    expected.iter().copied().map(Value::Int).collect::<Vec<_>>()
                );
                exec.close().unwrap();
                assert_eq!(exec.memory_bytes(), 0);
                // VecChunkExecutor is exhausted on reopen: no prior frame leaks.
                exec.open(&cx).unwrap();
                exec.next(&cx, &mut chunk).unwrap();
                assert!(chunk.rows.is_empty());
                exec.close().unwrap();
                assert_eq!(exec.memory_bytes(), 0);
            }
        }
    }
}
