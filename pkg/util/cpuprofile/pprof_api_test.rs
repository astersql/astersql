// Copyright 2026 AsterSQL.

use std::time::{Duration, Instant};

use serial_test::serial;

use super::*;

/// Go waits exactly two profile intervals and treats an empty collection as a
/// successful no-op.
#[test]
#[serial]
fn stop_without_profile_data_matches_go_timeout_and_result() {
    StopCPUProfiler();
    set_profile_duration(Duration::from_millis(10));

    let mut collector = NewCollector();
    collector
        .StartCPUProfile(shared_buffer_writer(Default::default()))
        .expect("collector starts");
    let started = Instant::now();
    let result = collector.StopCPUProfile();

    set_profile_duration(Duration::from_secs(1));
    assert!(
        result.is_ok(),
        "Go returns nil when no profile was received"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "Go only waits two configured profile intervals"
    );
}
