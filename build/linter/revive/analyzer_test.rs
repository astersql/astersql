// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn analyzer_and_rule_order_match_go() {
    for required in [
        "name: \"revive\"",
        "requires: &[]",
        "run",
        "util::SkipAnalyzerByConfig(&Analyzer)",
        "util::SkipAnalyzer(&Analyzer)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }

    let mut last = RUST_SOURCE.find("pub fn allRules").expect("allRules");
    for rule in [
        "EmptyBlockRule",
        "ConfusingResultsRule",
        "UnusedParamRule",
        "UnnecessaryStmtRule",
        "CallToGCRule",
        "UnusedReceiverRule",
        "UnexportedNamingRule",
        "UselessBreak",
    ] {
        let offset = RUST_SOURCE[last..]
            .find(&format!("Box::new(rule::{rule} {{}})"))
            .map(|offset| last + offset)
            .unwrap_or_else(|| panic!("missing or out-of-order extra rule: {rule}"));
        last = offset + rule.len();
    }
    assert!(RUST_SOURCE[last..].contains("rules.extend(defaultRules())"));

    last = RUST_SOURCE
        .find("pub fn defaultRules")
        .expect("defaultRules");
    for rule in [
        "VarDeclarationsRule",
        "DotImportsRule",
        "ExportedRule",
        "IncrementDecrementRule",
        "ContextKeysType",
    ] {
        let offset = RUST_SOURCE[last..]
            .find(&format!("Box::new(rule::{rule} {{}})"))
            .map(|offset| last + offset)
            .unwrap_or_else(|| panic!("missing or out-of-order default rule: {rule}"));
        last = offset + rule.len();
    }
}

#[test]
fn formatter_thread_returns_owned_output_after_all_failures_are_sent() {
    for required in [
        "let confidence = conf.Confidence;",
        "let formatter_conf = conf.clone();",
        "let formatter_thread = std::thread::spawn(move ||",
        "formatter.Format(format_rx, formatter_conf)",
        "if f.Confidence < confidence",
        "drop(format_tx);",
        "let output = formatter_thread",
        ".join()",
        "json::Unmarshal(output.as_bytes())?",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("channel::<bool>()"));
    assert!(!RUST_SOURCE.contains("let mut output = String::new()"));
}

#[test]
fn file_offsets_keep_go_bytes_and_report_shape() {
    for required in [
        "util::ReadFile(&mut pass.Fset",
        "sanitizeForOffset(&fileContent)",
        "unsafe { (*tf).Base() }",
        "util::FindOffset(",
        "res.Failure.Position.Start.Line",
        "res.Failure.Position.Start.Column",
        "&format!(\"{}: {}\", res.Failure.RuleName, res.Failure.Failure)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("String::from_utf8_lossy"));
}

#[test]
fn configuration_and_error_paths_match_go() {
    for required in [
        "goversion::NewVersion(\"1.21\")",
        "lint::New(os::ReadFile, 1024)",
        "IgnoreGeneratedHeader: false",
        "Confidence: 0.8",
        "Severity: \"error\".to_string()",
        "ErrorCode: -1",
        "WarningCode: -1",
        "\"loop\", \"method-call\", \"immediate-recover\", \"return\"",
        "config::GetLintingRules(&conf, vec![])?",
        "revive.Lint(packages, lintingRules, conf.clone())?",
        "config::GetFormatter(\"json\")?",
        "log::Error(\"Format error\", zap::Error(err))",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}

#[test]
fn completed_port_preserves_license_without_placeholders() {
    assert!(RUST_SOURCE.starts_with("// Copyright 2026 AsterSQL."));
    assert!(RUST_SOURCE.contains("// Copyright 2022 PingCAP, Inc."));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("Rust 草稿"));
}
