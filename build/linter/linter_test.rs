// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("linter.rs");
const GO_MOD: &str = include_str!("../../go.mod");

#[test]
fn go_only_blank_import_has_no_invented_rust_runtime_api() {
    assert!(!RUST_SOURCE.contains("pub fn ensure_skywalking_eye_config_dependency"));
    assert!(!RUST_SOURCE.contains("pub fn "));
    assert!(RUST_SOURCE.contains("Go 专属依赖保留"));
    assert!(RUST_SOURCE.contains("Rust 无运行时对应动作"));
}

#[test]
fn dependency_retained_by_the_go_blank_import_remains_pinned() {
    assert!(GO_MOD.contains("github.com/apache/skywalking-eyes v0.4.0"));
    assert!(RUST_SOURCE.contains("github.com/apache/skywalking-eyes/pkg/config"));
}

#[test]
fn audited_marker_and_original_license_are_preserved() {
    assert!(RUST_SOURCE.starts_with("// Copyright 2026 AsterSQL."));
    assert!(RUST_SOURCE.contains("// Copyright 2023 PingCAP, Inc."));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("文档化占位点"));
}
