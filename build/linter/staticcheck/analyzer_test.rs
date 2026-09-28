// Copyright 2026 AsterSQL.

const ANALYZER_SOURCE: &str = include_str!("analyzer.rs");
const UTIL_SOURCE: &str = include_str!("util.rs");

#[test]
fn stamped_name_and_selected_analyzer_use_thread_safe_lazy_identity() {
    for required in [
        "pub static name: &str = \"dummy value please replace using x_defs\";",
        "pub static Analyzer: once_cell::sync::Lazy<&'static analysis::Analyzer>",
        "once_cell::sync::Lazy::new(|| FindAnalyzerByName(name))",
        "util::SkipAnalyzerByConfig(*Analyzer);",
        "util::SkipAnalyzer(*Analyzer);",
    ] {
        assert!(ANALYZER_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!ANALYZER_SOURCE.contains("static mut"));
    assert!(!ANALYZER_SOURCE.contains("unsafe"));
    assert!(!ANALYZER_SOURCE.contains("Option<&'static analysis::Analyzer>"));
}

#[test]
fn analyzer_families_and_unused_singleton_match_go_order() {
    let mut last = 0;
    for family in [
        "quickfix::Analyzers.as_slice()",
        "simple::Analyzers.as_slice()",
        "staticcheck::Analyzers.as_slice()",
        "stylecheck::Analyzers.as_slice()",
        "std::slice::from_ref(&unused::Analyzer)",
    ] {
        let offset = UTIL_SOURCE[last..]
            .find(family)
            .map(|offset| last + offset)
            .unwrap_or_else(|| panic!("missing or out-of-order analyzer family: {family}"));
        last = offset + family.len();
    }
    assert!(UTIL_SOURCE.contains("resMap.insert(a.Analyzer.Name.clone(), a.Analyzer);"));
}

#[test]
fn lookup_returns_shared_analyzer_and_preserves_panic_text() {
    for required in [
        "once_cell::sync::Lazy<HashMap<String, &'static analysis::Analyzer>>",
        "if let Some(a) = Analyzers.get(name)",
        "return *a;",
        "panic!(\"not a valid staticcheck analyzer: {}\", name)",
    ] {
        assert!(UTIL_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn completed_ports_preserve_licenses_without_placeholders() {
    for (source, year) in [(ANALYZER_SOURCE, "2022"), (UTIL_SOURCE, "2021")] {
        assert!(source.starts_with("// Copyright 2026 AsterSQL."));
        assert!(source.contains(&format!("// Copyright {year} PingCAP, Inc.")));
        assert!(!source.contains("当前不保证可编译"));
        assert!(!source.contains("Rust 草稿"));
    }
}
