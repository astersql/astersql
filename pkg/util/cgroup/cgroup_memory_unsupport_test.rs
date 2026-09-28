// Copyright 2026 AsterSQL.

use super::*;

#[path = "cgroup_memory_unsupport.rs"]
mod memory_unsupport;

#[test]
fn unsupported_stat_scanner_stops_after_go_max_token_size() {
    let dir = tempfile::tempdir().expect("create temporary cgroup root");
    let contents = [vec![b'x'; 65536], b"\ninactive_file 42\n".to_vec()].concat();
    std::fs::write(dir.path().join("memory.stat"), contents).expect("write memory.stat");

    assert_eq!(
        memory_unsupport::detectMemInactiveFileInV2(dir.path())
            .unwrap_err()
            .to_string(),
        "failed to find expected memory stat \"inactive_file\" for cgroup v2 in memory.stat"
    );
}
