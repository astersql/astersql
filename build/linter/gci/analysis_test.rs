// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analysis.rs");

#[test]
fn analyzer_literal_and_run_signature_use_rust_api_shape() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"gci\"",
        "doc: \"Gci controls golang package import order and makes it always deterministic.\"",
        "requires: &[]",
        "anyhow::Result<Option<Box<dyn std::any::Any>>>",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("Lazy<analysis::Analyzer>"));
}

#[test]
fn rust_config_fills_every_go_zero_value_field() {
    for required in [
        "NoInlineComments: false",
        "NoPrefixComments: false",
        "Debug: false",
        "SkipGenerated: true",
        "SkipVendor: false",
        "CustomOrder: false",
        "NoLexOrder: false",
        "..Default::default()",
        "expect(\"default gci config must parse\")",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("rawCfg.Parse().ok()"));
}

#[test]
fn filenames_diff_error_and_diagnostics_match_go() {
    for required in [
        "Vec::with_capacity(pass.Files.len())",
        "pass.Fset.PositionFor(f.Pos(), false)",
        "fileNames.push(pos.Filename)",
        "let mut diffs: Vec<String> = Vec::new()",
        "let lock = std::sync::Mutex::new(())",
        "gci::DiffFormattedFilesToArray(fileNames, cfg, &mut diffs, &lock)?",
        "if diff.is_empty()",
        "Pos: 1",
        "Message: format!(\"\\n{}\", diff)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
