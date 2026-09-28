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

//! 窗口执行器的可执行回归测试，与 `window_executor_test.go` 的核心场景保持一致。
//!
//! 本模块直接构造物理窗口计划和分块输入，验证缓冲式、流水式及有序执行路径，
//! 同时覆盖窗口帧为空时不同窗口函数的返回值语义。

use super::{
    BoundType, Chunk, ExecContext, FrameBound, FrameType, OrderBy, Row, Value, WindowFrame,
};

/// 按执行器生命周期拉取所有输出行，供各测试统一比较最终结果。
pub(crate) fn run_executor(
    mut executor: super::WindowExecutor,
    context: &ExecContext,
) -> super::Result<Vec<Row>> {
    executor.open(context)?;
    let mut output = Chunk::default();
    let mut rows = Vec::new();
    loop {
        executor.next(context, &mut output)?;
        if output.num_rows() == 0 {
            break;
        }
        rows.extend(output.rows.iter().cloned());
    }
    executor.close()?;
    Ok(rows)
}

/// 构造测试所需的最小物理窗口计划。
///
/// 输出模式在输入列之后依次追加每个窗口函数的结果列。
pub(crate) fn build_plan(
    input_columns: usize,
    partition_by: Vec<usize>,
    order_by: Vec<OrderBy>,
    frame: Option<WindowFrame>,
    window_functions: Vec<Box<dyn super::WindowFunction>>,
    pipelined_enabled: bool,
) -> super::PhysicalWindowPlan {
    super::PhysicalWindowPlan {
        schema_columns: input_columns + window_functions.len(),
        partition_by,
        order_by,
        window_functions,
        frame,
        pipelined_enabled,
    }
}

fn rows() -> Vec<Row> {
    vec![
        vec![Value::Int(1), Value::Int(1)],
        vec![Value::Int(1), Value::Int(2)],
        vec![Value::Int(2), Value::Int(1)],
        vec![Value::Int(2), Value::Int(2)],
    ]
}

fn current_row() -> FrameBound {
    let mut bound = FrameBound::default();
    bound.bound_type = BoundType::CurrentRow;
    bound
}

fn order_by_value() -> Vec<OrderBy> {
    vec![OrderBy {
        column: 1,
        descending: false,
    }]
}

#[test]
fn go_test_window_executors_basic_matches_buffered_and_pipelined() {
    let context = ExecContext;
    let input = rows();

    // 同一组分区编号与滑动求和断言必须同时适用于缓冲式和流水式执行器。
    for pipelined_enabled in [false, true] {
        let row_number = super::build(
            build_plan(
                2,
                vec![0],
                order_by_value(),
                None,
                vec![Box::new(super::RowNumber::default())],
                pipelined_enabled,
            ),
            Box::new(super::VecChunkExecutor::new(vec![
                Chunk::new(input[..2].to_vec()),
                Chunk::new(input[2..].to_vec()),
            ])),
            false,
        )
        .unwrap();
        assert_eq!(
            run_executor(row_number, &context).unwrap(),
            vec![
                vec![Value::Int(1), Value::Int(1), Value::UInt(1)],
                vec![Value::Int(1), Value::Int(2), Value::UInt(2)],
                vec![Value::Int(2), Value::Int(1), Value::UInt(1)],
                vec![Value::Int(2), Value::Int(2), Value::UInt(2)],
            ]
        );

        let sum = super::build(
            build_plan(
                2,
                Vec::new(),
                order_by_value(),
                Some(WindowFrame {
                    frame_type: FrameType::Rows,
                    start: FrameBound::preceding(1),
                    end: current_row(),
                }),
                vec![Box::new(super::Sum::new(1))],
                pipelined_enabled,
            ),
            Box::new(super::VecChunkExecutor::new(vec![
                Chunk::new(input[..2].to_vec()),
                Chunk::new(input[2..].to_vec()),
            ])),
            false,
        )
        .unwrap();
        assert_eq!(
            run_executor(sum, &context).unwrap(),
            vec![
                vec![Value::Int(1), Value::Int(1), Value::Real(1.0)],
                vec![Value::Int(1), Value::Int(2), Value::Real(3.0)],
                vec![Value::Int(2), Value::Int(1), Value::Real(3.0)],
                vec![Value::Int(2), Value::Int(2), Value::Real(3.0)],
            ]
        );
    }
}

#[test]
fn go_test_build_ordered_window_exec_returns_ordered_pipeline() {
    let context = ExecContext;
    let executor = super::build_ordered(
        build_plan(
            2,
            vec![0],
            order_by_value(),
            None,
            vec![Box::new(super::RowNumber::default())],
            false,
        ),
        Box::new(super::VecChunkExecutor::new(vec![Chunk::new(rows())])),
    )
    .unwrap();
    // `build_ordered` 固定选择有序流水执行路径，输出仍须维持分区内的行号顺序。
    assert_eq!(
        run_executor(super::WindowExecutor::Pipelined(executor.inner), &context).unwrap(),
        vec![
            vec![Value::Int(1), Value::Int(1), Value::UInt(1)],
            vec![Value::Int(1), Value::Int(2), Value::UInt(2)],
            vec![Value::Int(2), Value::Int(1), Value::UInt(1)],
            vec![Value::Int(2), Value::Int(2), Value::UInt(2)],
        ]
    );
}

#[test]
fn go_test_window_return_column_nullable_attribute_matches_empty_frame_values() {
    let context = ExecContext;
    // “下一行到下一行”的帧会让末行得到空帧，用于核对各函数的空输入约定。
    let frame = WindowFrame {
        frame_type: FrameType::Rows,
        start: FrameBound::following(1),
        end: FrameBound::following(1),
    };
    let executor = super::build(
        build_plan(
            2,
            Vec::new(),
            order_by_value(),
            Some(frame),
            vec![
                Box::new(super::Sum::new(1)),
                Box::new(super::CountRows::default()),
                Box::new(super::RowNumber::default()),
            ],
            true,
        ),
        Box::new(super::VecChunkExecutor::new(vec![Chunk::new(vec![
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(1), Value::Int(20)],
            vec![Value::Int(1), Value::Int(30)],
        ])])),
        false,
    )
    .unwrap();
    // 空帧上的 SUM 返回 NULL、COUNT 返回 0；ROW_NUMBER 与帧内容无关，仍连续编号。
    assert_eq!(
        run_executor(executor, &context).unwrap(),
        vec![
            vec![
                Value::Int(1),
                Value::Int(10),
                Value::Real(20.0),
                Value::UInt(1),
                Value::UInt(1)
            ],
            vec![
                Value::Int(1),
                Value::Int(20),
                Value::Real(30.0),
                Value::UInt(1),
                Value::UInt(2)
            ],
            vec![
                Value::Int(1),
                Value::Int(30),
                Value::Null,
                Value::UInt(0),
                Value::UInt(3)
            ],
        ]
    );
}
