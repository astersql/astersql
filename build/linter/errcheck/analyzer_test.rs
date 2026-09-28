// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn upstream_analyzer_is_reexported_by_reference() {
    assert!(RUST_SOURCE.contains("pub static Analyzer: &analysis::Analyzer = errcheck::Analyzer;"));
    assert!(!RUST_SOURCE.contains("pub static Analyzer: analysis::Analyzer = errcheck::Analyzer;"));
}

#[test]
fn embedded_excludes_are_applied_before_skip_registration() {
    for required in [
        "include_str!(\"errcheck_excludes.txt\")",
        "Analyzer.Flags.Set(\"excludes\", data.to_string())",
        "if err.is_err()",
        "log::Fatal(err.unwrap_err())",
        "util::SkipAnalyzerByConfig(Analyzer)",
        "util::SkipAnalyzer(Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }

    let set_flag = RUST_SOURCE
        .find("Analyzer.Flags.Set")
        .expect("missing excludes flag setup");
    let configured = RUST_SOURCE
        .find("util::SkipAnalyzerByConfig(Analyzer)")
        .expect("missing configured skip");
    let unconditional = RUST_SOURCE
        .find("util::SkipAnalyzer(Analyzer)")
        .expect("missing unconditional skip");
    assert!(set_flag < configured && configured < unconditional);
    assert!(!RUST_SOURCE.contains("SkipAnalyzerByConfig(&Analyzer)"));
    assert!(!RUST_SOURCE.contains("SkipAnalyzer(&Analyzer)"));
}
