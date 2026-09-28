// Copyright 2026 AsterSQL.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::stream_status::{
    Checkpoint, EstimateQPSFromCounts, ParseLogBackupHandleKvBatchSum, PauseV2, Severity,
    TaskPrinter, TaskStatus, TeeTaskPrinter,
};
use crate::stubs::backuppb::{StreamBackupError, StreamBackupTaskInfo};

fn task_status() -> TaskStatus {
    TaskStatus {
        Info: StreamBackupTaskInfo::default(),
        paused: false,
        globalCheckpoint: 0,
        Checkpoints: Vec::new(),
        QPS: 0.0,
        LastErrors: HashMap::new(),
        PauseV2: None,
    }
}

struct CountingPrinter(Rc<RefCell<usize>>);

impl TaskPrinter for CountingPrinter {
    fn AddTask(&mut self, _task: TaskStatus) {
        *self.0.borrow_mut() += 1;
    }

    fn PrintTasks(&mut self) {}
}

#[test]
fn tee_printer_appends_to_callers_output_and_forwards() {
    let forwarded = Rc::new(RefCell::new(0));
    let mut output = Vec::new();
    {
        let inner = Box::new(CountingPrinter(Rc::clone(&forwarded)));
        let mut tee = TeeTaskPrinter(inner, &mut output);
        tee.AddTask(task_status());
    }

    assert_eq!(1, output.len());
    assert_eq!(1, *forwarded.borrow());
}

#[test]
fn metrics_parser_matches_go_regex_inside_prometheus_text() {
    assert_eq!(
        Some(123),
        ParseLogBackupHandleKvBatchSum(
            "# HELP metric\nprefix tikv_stream_handle_kv_batch_sum 123 trailing\n",
        )
    );
    assert_eq!(
        Some(456),
        ParseLogBackupHandleKvBatchSum("tikv_log_backup_handle_kv_batch_sum 456\n")
    );
    assert_eq!(None, ParseLogBackupHandleKvBatchSum("unrelated 99"));
}

#[test]
fn status_and_checkpoint_branches_match_go() {
    let mut task = task_status();
    assert_eq!("NORMAL", task.StatusString());
    task.paused = true;
    assert_eq!("PAUSE", task.StatusString());
    task.LastErrors.insert(7, StreamBackupError::default());
    assert_eq!("ERROR", task.StatusString());
    task.LastErrors.clear();
    task.PauseV2 = Some(PauseV2 {
        Severity: Severity::Error,
    });
    assert_eq!("ERROR", task.StatusString());
    task.paused = false;
    assert_eq!("NORMAL", task.StatusString());

    task.Checkpoints = vec![Checkpoint::Store(1, 30), Checkpoint::Store(2, 20)];
    assert_eq!(Checkpoint::Store(2, 20), task.GetMinStoreCheckpoint());
    task.Checkpoints.push(Checkpoint::Global(25));
    assert_eq!(Checkpoint::Global(25), task.GetMinStoreCheckpoint());
}

#[test]
fn qps_delta_uses_go_uint64_wraparound() {
    assert_eq!(2.0, EstimateQPSFromCounts(10, 14, 2.0));
    assert_eq!(u64::MAX as f64, EstimateQPSFromCounts(1, 0, 1.0));
    assert_eq!(0.0, EstimateQPSFromCounts(1, 2, 0.0));
}
