// Copyright 2026 AsterSQL.

use super::window_executor_test::{build_plan, run_executor};
use super::{
    BoundType, Chunk, CountRows, ExecContext, FrameBound, FrameType, OrderBy, Value,
    VecChunkExecutor, WindowFrame,
};

#[test]
fn range_current_row_keeps_adjacent_large_integers_in_distinct_peer_groups() {
    let mut current_row = FrameBound::default();
    current_row.bound_type = BoundType::CurrentRow;
    let executor = super::build(
        build_plan(
            1,
            Vec::new(),
            vec![OrderBy {
                column: 0,
                descending: false,
            }],
            Some(WindowFrame {
                frame_type: FrameType::Range,
                start: current_row.clone(),
                end: current_row,
            }),
            vec![Box::new(CountRows::default())],
            false,
        ),
        Box::new(VecChunkExecutor::new(vec![Chunk::new(vec![
            vec![Value::Int(9_007_199_254_740_992)],
            vec![Value::Int(9_007_199_254_740_993)],
        ])])),
        false,
    )
    .unwrap();

    assert_eq!(
        run_executor(executor, &ExecContext).unwrap(),
        vec![
            vec![Value::Int(9_007_199_254_740_992), Value::UInt(1)],
            vec![Value::Int(9_007_199_254_740_993), Value::UInt(1)],
        ]
    );
}
