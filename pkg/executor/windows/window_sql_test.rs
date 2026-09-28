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
    run_executor(executor, &ExecContext).unwrap()
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
        run_executor(executor, &ExecContext).unwrap(),
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
