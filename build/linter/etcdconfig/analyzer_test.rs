// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn analyzer_wiring_matches_go_dependencies_and_rust_api_shape() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"etcdconfig\"",
        "doc: \"Check necessary fields of etcd config\"",
        "requires: &[&inspect::Analyzer]",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn required_selector_identifier_is_unwrapped_before_name_access() {
    assert!(RUST_SOURCE.contains("expect(\"selector expression must have an identifier\")"));
    assert!(RUST_SOURCE.contains("if selected.Name != configStructName"));
    assert!(!RUST_SOURCE.contains("if tp.Sel.Name != configStructName"));
}

#[test]
fn import_filter_field_scan_and_diagnostic_match_go() {
    for required in [
        "go.etcd.io/etcd/client/v3",
        "util::GetPackageName(&file.Imports, configPackagePath, configPackageName)",
        "if packageName.is_empty()",
        "n.as_composite_lit()",
        "lit.Type.as_selector_expr()",
        "tp.X.as_ident()",
        "if litPackage.Name != packageName",
        "field.as_key_value_expr()",
        "kv.Key.as_ident()",
        "if key.Name == \"AutoSyncInterval\"",
        "pass.Reportf(lit.Pos(), \"missing field AutoSyncInterval\")",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
