// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn version_extrema_include_all_four_go_sources() {
    let sources = "[maxVerVariable, maxVerFunc, maxVerFuncUsed, curVerVariable]";
    assert_eq!(RUST_SOURCE.matches(sources).count(), 2);
    assert!(RUST_SOURCE.contains(".min()"));
    assert!(RUST_SOURCE.contains(".max()"));
    assert_eq!(
        RUST_SOURCE
            .matches("expect(\"version source list is non-empty\")")
            .count(),
        2
    );
    assert!(!RUST_SOURCE.contains("min(maxVerVariable,"));
    assert!(!RUST_SOURCE.contains("max(maxVerVariable,"));
}

#[test]
fn required_ast_shapes_preserve_go_type_assertion_panics() {
    for required in [
        "system table entry must be a composite literal",
        "system table field must be a key-value expression",
        "table ID must be a selector expression",
        "table name must be a basic literal",
        "versioned schema entry must be a composite literal",
        "databases field must be a key-value expression",
        "upgrade function list must be a composite literal",
        "currentBootstrapVersion must reference an identifier",
        "version value must be a basic literal",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn schema_set_comparison_borrows_maps_needed_for_diagnostics() {
    assert!(
        RUST_SOURCE
            .contains("maps::Equal(&schemaDefVarNames, &nameUsedInVersionedBootstrapSchema)")
    );
}

#[test]
fn bootstrap_and_upgrade_branch_contracts_match_go() {
    for required in [
        "strings::HasSuffix(&name, bootstrapCodeFile)",
        "strings::HasSuffix(&name, upgradeCodeFile)",
        "eleTpName == \"TableBasicInfo\"",
        "eleTpName == \"versionedBootstrapSchema\"",
        "schemaDefNodeCount < 1",
        "versionedBootstrapSchemaDefCount != 1",
        "v.Tok == token::CONST && v.Specs.len() > 1",
        "if valInName < maxVerVariable",
        "if val != valInName",
        "if minv == maxv && minv != 0",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
