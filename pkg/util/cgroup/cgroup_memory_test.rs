// Copyright 2026 AsterSQL.

// Exercise the Linux implementation on every host, including macOS.
use super::*;
#[path = "cgroup_memory.rs"]
mod memory;

#[test]
fn stat_byte_and_scanner_parity() {
    let dir = tempfile::tempdir().unwrap();
    let cases: Vec<(Vec<u8>, Option<u64>)> = vec![
        (b"\xff 1\ninactive_file 42\n".to_vec(), Some(42)),
        (b"inactive_file +42\n".to_vec(), None),
        (
            b"inactive_file 18446744073709551615\n".to_vec(),
            Some(u64::MAX),
        ),
        (b"inactive_file 18446744073709551616\n".to_vec(), None),
        (
            [vec![b'x'; 65536], b"\ninactive_file 42\n".to_vec()].concat(),
            None,
        ),
        (b"inactive_file 42\r\n inactive_file 99".to_vec(), Some(42)),
    ];
    for (contents, expected) in cases {
        std::fs::write(dir.path().join("memory.stat"), &contents).unwrap();
        let result = memory::detectMemInactiveFileUsageInV2(dir.path());
        assert_eq!(
            result.ok(),
            expected,
            "input prefix {:?}",
            &contents[..contents.len().min(60)]
        );
    }
}

#[test]
fn stat_read_error_matches_go_missing_key_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("memory.stat")).unwrap();
    assert_eq!(
        memory::detectMemInactiveFileUsageInV2(dir.path())
            .unwrap_err()
            .to_string(),
        "failed to find expected memory stat \"inactive_file\" for cgroup v2 in memory.stat"
    );
}
