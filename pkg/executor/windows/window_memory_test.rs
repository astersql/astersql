// Copyright 2026 AsterSQL.

use super::window_executor_test::build_plan;
use super::{Chunk, ExecContext, Value, WindowExecutor};

fn input() -> Vec<Chunk> {
    vec![Chunk::new(vec![
        vec![Value::Int(1), Value::Text("first".repeat(8))],
        vec![Value::Int(1), Value::Text("second".repeat(8))],
    ])]
}

#[test]
fn window_memory_is_charged_to_statement_and_released_on_close() {
    for pipelined in [false, true] {
        let context = ExecContext::default();
        let mut executor = super::build(
            build_plan(
                2,
                vec![0],
                Vec::new(),
                None,
                vec![Box::new(super::Lag::new(1, 1, Value::Null))],
                pipelined,
            ),
            Box::new(super::VecChunkExecutor::new(input())),
            false,
        )
        .unwrap();

        executor.open(&context).unwrap();
        assert!(executor.memory_bytes() > 0);
        assert_eq!(
            context.statement_memory_tracker.bytes_consumed(),
            executor.memory_bytes()
        );

        let mut output = Chunk::default();
        executor.next(&context, &mut output).unwrap();
        assert_eq!(output.num_rows(), 2);
        assert!(executor.memory_bytes() > 0);
        assert_eq!(
            context.statement_memory_tracker.bytes_consumed(),
            executor.memory_bytes()
        );

        executor.close().unwrap();
        assert_eq!(executor.memory_bytes(), 0);
        assert_eq!(context.statement_memory_tracker.bytes_consumed(), 0);
    }
}

#[test]
fn output_chunk_memory_is_transferred_out_of_the_window_tracker() {
    for pipelined in [false, true] {
        let context = ExecContext::default();
        let mut executor = super::build(
            build_plan(
                2,
                Vec::new(),
                Vec::new(),
                None,
                vec![Box::new(super::CountRows::default())],
                pipelined,
            ),
            Box::new(super::VecChunkExecutor::new(input())),
            false,
        )
        .unwrap();
        executor.open(&context).unwrap();

        let initial = executor.memory_bytes();
        let mut output = Chunk::default();
        executor.next(&context, &mut output).unwrap();
        assert_eq!(output.num_rows(), 2);
        assert_eq!(executor.memory_bytes(), initial);

        executor.close().unwrap();
        assert_eq!(context.statement_memory_tracker.bytes_consumed(), 0);
    }
}
