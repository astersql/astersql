// Copyright 2026 AsterSQL.

use super::global_arbitrator::{
    CleanupGlobalMemArbitratorForTest, RuntimeMemStateRecorder, SetupGlobalMemArbitratorForTest,
};

#[test]
fn setup_removes_stale_runtime_memory_state_like_go() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = RuntimeMemStateRecorder::new(dir.path());
    recorder
        .store(&serde_json::json!({"version": 1, "stale": true}))
        .unwrap();

    SetupGlobalMemArbitratorForTest(dir.path().display().to_string());

    assert_eq!(recorder.load().unwrap(), None);
    CleanupGlobalMemArbitratorForTest();
}
