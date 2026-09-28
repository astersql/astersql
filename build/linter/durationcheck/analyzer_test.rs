// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn upstream_analyzer_is_reexported_by_reference() {
    assert!(
        RUST_SOURCE.contains("pub static Analyzer: &analysis::Analyzer = durationcheck::Analyzer;")
    );
    assert!(
        !RUST_SOURCE.contains("pub static Analyzer: analysis::Analyzer = durationcheck::Analyzer;")
    );
}

#[test]
fn configuration_and_global_skip_keep_go_order_without_double_reference() {
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
