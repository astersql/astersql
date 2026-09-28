// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn analyzer_wiring_matches_go_dependencies_and_rust_api_shape() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"recover\"",
        "doc: \"Check Recover() is directly called by defer\"",
        "requires: &[&inspect::Analyzer]",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn filtered_call_and_required_selector_are_explicitly_unwrapped() {
    assert!(RUST_SOURCE.contains("expect(\"inspector must yield a call expression\")"));
    assert!(RUST_SOURCE.contains("expect(\"selector expression must have an identifier\")"));
    assert!(RUST_SOURCE.contains("if selected.Name != funcName"));
    assert!(!RUST_SOURCE.contains("if sel.Sel.Name != funcName"));
}

#[test]
fn import_alias_and_direct_defer_contract_match_go() {
    for required in [
        "util::GetPackageName(&file.Imports, packagePath, packageName)",
        "if packageName.is_empty()",
        "if !push",
        "if usedPackage.Name != packageName",
        "let parentStmt = &stack[stack.len() - 2]",
        "if !parentStmt.is_defer_stmt()",
        "Recover() should be directly called by defer",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
