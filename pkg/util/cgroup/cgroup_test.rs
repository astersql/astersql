// Copyright 2026 AsterSQL.

use super::{detectControlPath, readInt64Value};
use std::fs;

#[test]
fn v2_control_path_matches_go_third_field_behavior() {
    let dir = tempfile::tempdir().expect("create fixture directory");
    let path = dir.path().join("cgroup");
    fs::write(&path, "0::/machine.slice:name:with:colons\n").expect("write cgroup fixture");

    assert_eq!(
        detectControlPath(&path, "cpu,cpuacct").expect("detect v2 path"),
        "/machine.slice"
    );
}

#[test]
fn empty_single_value_file_matches_go_zero_value() {
    let dir = tempfile::tempdir().expect("create fixture directory");
    fs::write(dir.path().join("memory.max"), "").expect("write empty value fixture");

    assert_eq!(
        readInt64Value(dir.path(), "memory.max", 2).expect("read empty value"),
        0
    );
}
