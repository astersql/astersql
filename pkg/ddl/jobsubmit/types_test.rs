// Copyright 2026 AsterSQL.

use crate::{Job, JobArgs, JobState, JobType};

#[test]
fn encode_uses_go_job_json_and_versioned_raw_args() {
    let job = Job {
        id: 42,
        version: 2,
        schema_id: 7,
        table_id: 9,
        schema_name: "test".into(),
        table_name: "t".into(),
        job_type: JobType::TruncateTable,
        query: "truncate table test.t".into(),
        start_ts: 123,
        bdr_role: "primary".into(),
        state: JobState::Queueing,
        ..Job::default()
    };
    let encoded = job.encode(&JobArgs::TruncateTable {
        old_partition_ids: vec![1, 2],
        new_table_id: 10,
        new_partition_ids: vec![11, 12],
    });
    let wire: serde_json::Value = serde_json::from_slice(&encoded).expect("Go Job JSON");

    assert_eq!(wire["id"], 42);
    assert_eq!(wire["type"], 11);
    assert_eq!(wire["state"], 8);
    assert_eq!(wire["schema_id"], 7);
    assert_eq!(wire["table_id"], 9);
    assert_eq!(wire["version"], 2);
    assert_eq!(wire["start_ts"], 123);
    assert_eq!(wire["bdr_role"], "primary");
    assert_eq!(
        wire["raw_args"],
        serde_json::json!({
            "new_table_id": 10,
            "new_partition_ids": [11, 12],
            "old_partition_ids": [1, 2]
        })
    );
}
