// Copyright 2026 AsterSQL.

use super::*;
use std::time::SystemTime;

struct ErrorTable;

impl TaskTable for ErrorTable {
    fn insert_subtask(&self, _: NewSubtask) -> Result<i64, DxfError> {
        unreachable!()
    }
    fn get_one_pending_task(&self) -> Result<Option<Task>, DxfError> {
        unreachable!()
    }
    fn subtask_history_count(&self, _: Option<i64>) -> Result<usize, DxfError> {
        unreachable!()
    }
    fn subtasks_by_task_id(&self, _: i64) -> Result<Vec<Subtask>, DxfError> {
        unreachable!()
    }
    fn task_history_count(&self) -> Result<usize, DxfError> {
        unreachable!()
    }
    fn task_end_time(&self, _: i64) -> Result<Option<SystemTime>, DxfError> {
        Err(DxfError("task query failed".into()))
    }
    fn subtask_end_time(&self, _: i64) -> Result<Option<SystemTime>, DxfError> {
        Err(DxfError("subtask query failed".into()))
    }
    fn subtask_nodes(&self, _: i64) -> Result<Vec<String>, DxfError> {
        unreachable!()
    }
    fn update_subtask_exec_id(&self, _: &str, _: i64) -> Result<(), DxfError> {
        unreachable!()
    }
    fn transfer_subtasks_to_history(&self, _: i64) -> Result<(), DxfError> {
        unreachable!()
    }
    fn history_tasks_in_states(&self, _: &[TaskState]) -> Result<Vec<Task>, DxfError> {
        unreachable!()
    }
    fn delete_subtasks(&self, _: i64) -> Result<(), DxfError> {
        unreachable!()
    }
    fn is_task_cancelling(&self, _: i64) -> Result<bool, DxfError> {
        unreachable!()
    }
    fn print_subtask_info(&self, _: i64) -> Result<(), DxfError> {
        Err(DxfError("diagnostic query failed".into()))
    }
}

#[test]
fn end_time_helpers_ignore_query_errors_like_go() {
    let table = ErrorTable;
    assert_eq!(GetTaskEndTime(&table, 1).unwrap(), None);
    assert_eq!(GetSubtaskEndTime(&table, 2).unwrap(), None);
}

#[test]
fn print_subtask_info_ignores_diagnostic_query_errors_like_go() {
    assert!(PrintSubtaskInfo(&ErrorTable, 1).is_ok());
}
