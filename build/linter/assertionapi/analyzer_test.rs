// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn selector_identifier_is_unwrapped_before_name_and_position_access() {
    assert!(RUST_SOURCE.contains("let Some(ident) = sel.Sel.as_ref() else"));
    assert!(RUST_SOURCE.contains("ident.Name != \"UpdateAssertionFlags\""));
    assert!(RUST_SOURCE.contains("pass.Reportf("));
    assert!(RUST_SOURCE.contains("ident.Pos(),"));
    assert!(!RUST_SOURCE.contains("sel.Sel.Name"));
    assert!(!RUST_SOURCE.contains("sel.Sel.Pos()"));
}

#[test]
fn named_type_objects_and_packages_are_unwrapped_before_matching() {
    assert_eq!(
        RUST_SOURCE.matches("let Some(obj) = n.Obj() else").count(),
        2
    );
    assert_eq!(
        RUST_SOURCE
            .matches("let Some(pkg) = obj.Pkg() else")
            .count(),
        2
    );
    assert!(RUST_SOURCE.contains("obj.Name() == \"Key\" && pkg.Path() == kvPkgPath"));
    assert!(RUST_SOURCE.contains("obj.Name() == \"AssertionOp\" && pkg.Path() == kvPkgPath"));
}

#[test]
fn analyzer_keeps_go_path_signature_and_skip_contracts() {
    for required in [
        "github.com/pingcap/tidb/pkg/kv",
        "strings::Contains(&f, \"pkg/table/tables/\")",
        "2 => (0, 1)",
        "3 => (1, 2)",
        "if !isKVKey(params.At(keyIdx).Type())",
        "if !isKVAssertionOp(params.At(opIdx).Type())",
        "util::SkipAnalyzerByConfig(&Analyzer)",
        "util::SkipAnalyzer(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
