// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn analyzer_factory_is_lazily_initialized_and_skipped_by_reference() {
    assert!(RUST_SOURCE.contains("pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer>"));
    assert!(RUST_SOURCE.contains("once_cell::sync::Lazy::new(|| copyloopvar::NewAnalyzer())"));
    assert!(RUST_SOURCE.contains("util::SkipAnalyzerByConfig(&Analyzer)"));
    assert!(
        !RUST_SOURCE
            .contains("pub static Analyzer: analysis::Analyzer = copyloopvar::NewAnalyzer()")
    );
}
