// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn upstream_analyzer_is_reexported_by_reference() {
    assert!(RUST_SOURCE.contains("pub static Analyzer: &analysis::Analyzer = intrange::Analyzer;"));
    assert!(!RUST_SOURCE.contains("pub static Analyzer: analysis::Analyzer = intrange::Analyzer;"));
}

#[test]
fn configured_and_global_skip_keep_go_order_and_upstream_identity() {
    let configured = RUST_SOURCE
        .find("util::SkipAnalyzerByConfig(Analyzer);")
        .expect("missing configured skip");
    let unconditional = RUST_SOURCE
        .find("util::SkipAnalyzer(Analyzer);")
        .expect("missing unconditional skip");
    assert!(configured < unconditional);
    assert!(!RUST_SOURCE.contains("SkipAnalyzerByConfig(&Analyzer)"));
    assert!(!RUST_SOURCE.contains("SkipAnalyzer(&Analyzer)"));
}

#[test]
fn completed_port_preserves_pingcap_license_and_has_no_placeholder_claims() {
    assert!(RUST_SOURCE.starts_with("// Copyright 2026 AsterSQL."));
    assert!(RUST_SOURCE.contains("// Copyright 2024 PingCAP, Inc."));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("不会真正运行 intrange"));
}
