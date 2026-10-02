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
    assert_eq!(task.Error.as_deref(), Some("[7]"));

    let task = Row2Task(task_row(Cell::Bytes(b"not-json".to_vec())));
    assert_eq!(task.Error.as_deref(), Some("not-json"));
}

#[test]
fn task_error_null_normalized_and_fallback() {
    for (cell, expected) in [
        (Cell::Null, None),
        (
            Cell::Bytes(br#"{"message":7}"#.to_vec()),
            Some(r#"{"message":7}"#),
        ),
        (Cell::Bytes(b"[]".to_vec()), Some("[]")),
        (
            Cell::Bytes(br#"{"message":"history task failed"}"#.to_vec()),
            Some("[0]history task failed"),
        ),
        (Cell::Bytes(b"not-json".to_vec()), Some("not-json")),
        (
            Cell::Bytes(br#"{"class":8,"code":1062,"message":"Duplicate entry"}"#.to_vec()),
            Some("[kv:1062]Duplicate entry"),
        ),
        (
            Cell::Bytes(
                br#"{"code":1062,"rfccode":"kv:1062","message":"Duplicate entry"}"#.to_vec(),
            ),
            Some("[kv:1062]Duplicate entry"),
        ),
        (Cell::Bytes(br#"{"code":7}"#.to_vec()), Some("[7]")),
    ] {
        assert_eq!(Row2Task(task_row(cell)).Error.as_deref(), expected);
    }
}
