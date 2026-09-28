// Copyright 2026 AsterSQL.

use std::sync::atomic::Ordering;
use std::time::SystemTime;

use crate::execute::{Progress, SubtaskSummary};

#[test]
fn subtask_summary_preserves_the_go_execute_contract() {
    let update_time = SystemTime::UNIX_EPOCH;
    let summary = SubtaskSummary {
        Progresses: vec![Progress {
            RowCnt: 3,
            Processed: 5,
            UpdateTime: update_time,
        }],
        ..Default::default()
    };

    summary.RowCnt.store(7, Ordering::SeqCst);
    summary.Processed.store(11, Ordering::SeqCst);
    summary.ReadBytes.store(13, Ordering::SeqCst);
    summary.GetReqCnt.store(17, Ordering::SeqCst);
    summary.PutReqCnt.store(19, Ordering::SeqCst);

    assert_eq!(summary.RowCnt.load(Ordering::SeqCst), 7);
    assert_eq!(summary.Processed.load(Ordering::SeqCst), 11);
    assert_eq!(summary.ReadBytes.load(Ordering::SeqCst), 13);
    assert_eq!(summary.GetReqCnt.load(Ordering::SeqCst), 17);
    assert_eq!(summary.PutReqCnt.load(Ordering::SeqCst), 19);
    assert_eq!(summary.Progresses.len(), 1);
    assert_eq!(summary.Progresses[0].RowCnt, 3);
    assert_eq!(summary.Progresses[0].Processed, 5);
    assert_eq!(summary.Progresses[0].UpdateTime, update_time);
}
