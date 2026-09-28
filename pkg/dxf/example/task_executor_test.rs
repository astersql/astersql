// Copyright 2026 AsterSQL.

use crate::stepExecutor;
use astersql_dxf_framework_taskexecutor::{Context, StepExecutor, Subtask, SubtaskBase};
use astersql_util_logutil::log::{BgLogger, LogField};

#[test]
fn run_subtask_logs_id_and_decoded_message_like_go() {
    let mut subtask = Subtask {
        SubtaskBase: SubtaskBase {
            ID: 2994,
            ..SubtaskBase::default()
        },
        Meta: br#"{"message":"hello from task executor"}"#.to_vec(),
    };

    StepExecutor::RunSubtask(
        &stepExecutor::default(),
        &Context::Background(),
        &mut subtask,
    )
    .expect("valid Go-compatible JSON must run successfully");

    let entries = BgLogger().entries();
    let entry = entries
        .iter()
        .rev()
        .find(|entry| {
            entry.message == "RunSubtask"
                && entry
                    .fields
                    .contains(&LogField::I64("subtaskID".into(), 2994))
        })
        .expect("Go RunSubtask emits an info log containing subtaskID");
    assert!(entry.fields.contains(&LogField::String(
        "message".into(),
        "hello from task executor".into(),
    )));
}

#[test]
fn run_subtask_propagates_json_errors_without_logging_success() {
    let mut subtask = Subtask {
        SubtaskBase: SubtaskBase {
            ID: 2995,
            ..SubtaskBase::default()
        },
        Meta: br#"{"message":}"#.to_vec(),
    };

    assert!(
        StepExecutor::RunSubtask(
            &stepExecutor::default(),
            &Context::Background(),
            &mut subtask
        )
        .is_err()
    );
    assert!(!BgLogger().entries().iter().any(|entry| {
        entry.message == "RunSubtask"
            && entry
                .fields
                .contains(&LogField::I64("subtaskID".into(), 2995))
    }));
}
