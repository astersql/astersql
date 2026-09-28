// Copyright 2026 AsterSQL.

use super::*;
use std::time::UNIX_EPOCH;

fn task_row(error: Cell) -> chunk::Row {
    chunk::Row::new(vec![
        Cell::Int(1),
        Cell::String("task-key".into()),
        Cell::String("Example".into()),
        Cell::String("running".into()),
        Cell::Int(1),
        Cell::Int(512),
        Cell::Int(4),
        Cell::Time(UNIX_EPOCH),
        Cell::String("background".into()),
        Cell::Int(0),
        Cell::Json("{}".into()),
        Cell::String(String::new()),
        Cell::Null,
        Cell::Null,
        Cell::Bytes(Vec::new()),
        Cell::String(String::new()),
        error,
        Cell::Null,
    ])
}

#[test]
fn task_error_uses_normalized_error_json_semantics() {
    let task = Row2Task(task_row(Cell::Bytes(br#"{"code":7}"#.to_vec())));
    assert_eq!(task.Error.as_deref(), Some(""));

    let task = Row2Task(task_row(Cell::Bytes(b"not-json".to_vec())));
    assert_eq!(task.Error.as_deref(), Some("not-json"));
}
