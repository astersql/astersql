// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analysis.rs");

#[test]
fn upstream_analyzer_is_reexported_by_reference_without_clone() {
    assert!(
        RUST_SOURCE
            .contains("pub static Analyzer: &analysis::Analyzer = forcetypeassert::Analyzer;")
    );
    assert!(!RUST_SOURCE.contains("forcetypeassert::Analyzer.clone()"));
    assert!(!RUST_SOURCE.contains("Lazy<analysis::Analyzer>"));
}

#[test]
fn configured_and_global_skip_keep_go_order_on_same_analyzer() {
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
