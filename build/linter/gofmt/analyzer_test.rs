// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn analyzer_literal_and_initialization_match_go() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"gofmt\"",
        "doc: concat!(",
        "requires: &[]",
        "run",
        "static mut needSimplify: bool = false",
        "pub fn init()",
        "Analyzer.Flags.BoolVar(",
        "\"need-simplify\"",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("pub fn init_flags()"));
    assert!(!RUST_SOURCE.contains("pub fn init_skip_by_config()"));
}

#[test]
fn filename_filter_rewrite_and_diagnostics_match_go() {
    for required in [
        "Vec::with_capacity(10)",
        "pass.Fset.PositionFor(f.Pos(), false)",
        "!pos.Filename.ends_with(\"failpoint_binding__.go\")",
        "Pattern: \"interface{}\"",
        "Replacement: \"any\"",
        "gofmt::RunRewrite(&f, unsafe { needSimplify }, &rules)",
        "if let Some(diff) = diff",
        "Pos: 1",
        "Message: format!(\"\\n{}\", diff)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn run_preserves_dynamic_nil_and_rewrite_error_chain() {
    for required in [
        "anyhow::Result<Option<Box<dyn std::any::Any>>>",
        "anyhow::Error::new(err).context(format!(\"could not run gofmt ({f})\"))",
        "Ok(None)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("fmt::Errorf(format!"));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("Rust 草稿不执行 gofmt"));
}
