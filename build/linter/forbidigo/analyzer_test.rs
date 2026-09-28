// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn analyzer_literal_uses_rust_api_shape_without_unneeded_lazy_wrapper() {
    for required in [
        "pub static Analyzer: analysis::Analyzer = analysis::Analyzer {",
        "name: \"forbidigo\"",
        "doc: \"forbid identifiers\"",
        "requires: &[]",
        "run",
        "util::SkipAnalyzerByConfig(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("Lazy<analysis::Analyzer>"));
}

#[test]
fn linter_setup_preserves_go_pattern_options_and_error_source() {
    for required in [
        "Lazy<Vec<String>>",
        "sessionctx.Context.GetSessionVars",
        "forbidigo::OptionIgnorePermitDirectives(true)",
        "forbidigo::OptionExcludeGodocExamples(false)",
        "forbidigo::OptionAnalyzeTypes(true)",
        "anyhow::Error::new(err).context(\"failed to configure linter\")",
        "anyhow::Result<Option<Box<dyn std::any::Any>>>",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("anyhow!(\"failed to configure linter: {}\""));
}

#[test]
fn run_uses_original_ast_and_shared_type_context() {
    for required in [
        "nodes.push(f.as_node())",
        "Fset: &pass.Fset",
        "DebugLog: None",
        "TypesInfo: &pass.TypesInfo",
        "linter.RunWithConfig(config, nodes)?",
        "reportIssues(pass, &issues)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("f.clone().into()"));
    assert!(!RUST_SOURCE.contains("pass.Fset.clone()"));
    assert!(!RUST_SOURCE.contains("pass.TypesInfo.clone()"));
}

#[test]
fn whitelist_and_restriction_diagnostic_match_go() {
    for required in [
        "SQLMode",
        "CDCWriteSource",
        "StmtCtx",
        "TimeZone",
        "Location",
        "GetSplitRegionTimeout",
        "SetInTxn",
        "BuildParserConfig",
        "DiskFull",
        "DefaultCollationForUTF8MB4",
        "RowEncoder",
        "SetStatusFlag",
        "lc.GetLine(i.Position().Filename, i.Position().Line)",
        "if s.contains(whiteList)",
        "Pos: i.Pos()",
        "Message: i.Details()",
        "Category: \"restriction\"",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
