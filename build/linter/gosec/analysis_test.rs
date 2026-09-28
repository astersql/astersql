// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analysis.rs");

#[test]
fn analyzer_rules_and_skip_wiring_match_go() {
    for required in [
        "// Copyright 2026 AsterSQL.",
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: Name",
        "doc: \"Inspects source code for security problems\"",
        "requires: &[]",
        "run",
        "Result<Option<Box<dyn std::any::Any>>, analysis::Error>",
        "matches!(id, \"G104\" | \"G103\" | \"G101\" | \"G201\")",
        "util::SkipAnalyzerByConfig(&Analyzer)",
        "util::SkipAnalyzer(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn loader_program_uses_shared_owned_package_info_without_invalid_raw_pointers() {
    for required in [
        "Vec<std::sync::Arc<loader::PackageInfo>>",
        "std::sync::Arc::new(",
        "util::MakeFakeLoaderPackageInfo(pass)",
        "HashMap<types::Package, std::sync::Arc<loader::PackageInfo>>",
        "allPkgs.insert(pkg.Pkg.clone(), std::sync::Arc::clone(pkg))",
        "Fset: &pass.Fset",
        "Created: createdPkgs",
        "AllPackages: allPkgs",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("Vec<*mut loader::PackageInfo>"));
}

#[test]
fn issue_lines_file_offsets_and_diagnostics_match_go() {
    for required in [
        "util::ReadFile(&mut pass.Fset, &i.File)",
        "parseIssueLine(&i.Line)",
        "split_once('-')",
        "let file_base = unsafe { (*tf).Base() }",
        "findLineOffset(&fileContent, line)",
        "format!(\"[{}] {}: {}\", Name, i.RuleID, i.What)",
        "pub fn findLineOffset(fileContent: &[u8], line: i32) -> i32",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("String::from_utf8_lossy"));
}

#[test]
fn severity_filter_preserves_all_qualified_issues_without_raw_dereference() {
    for required in [
        "issues: Vec<gosec::Issue>",
        ") -> Vec<gosec::Issue>",
        "if issue.Severity >= severity && issue.Confidence >= confidence",
        "res.push(issue)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("Vec<*mut gosec::Issue>"));
}
