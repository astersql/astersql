// Copyright 2026 AsterSQL.

const GO_SOURCE: &str = include_str!("analyzer.go");
const RUST_SOURCE: &str = include_str!("analyzer.rs");

fn go_active_rules() -> Vec<&'static str> {
    GO_SOURCE
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("&rule.")
                .and_then(|rule| rule.split_once('{').map(|(name, _)| name))
        })
        .collect()
}

fn rust_active_rules() -> Vec<&'static str> {
    RUST_SOURCE
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("Box::new(rule::")
                .and_then(|rule| rule.split_once(' ').map(|(name, _)| name))
        })
        .collect()
}

#[test]
fn active_revive_rules_match_go_order_and_duplicates() {
    assert_eq!(rust_active_rules(), go_active_rules());
    assert!(RUST_SOURCE.contains("rules.extend(defaultRules());"));
}

#[test]
fn formatter_pipeline_has_distinct_endpoints_and_returns_output() {
    assert!(RUST_SOURCE.contains("let (format_tx, format_rx)"));
    assert!(RUST_SOURCE.contains("let (output_tx, output_rx)"));
    assert!(RUST_SOURCE.contains("formatter.Format(format_rx, formatter_conf)"));
    assert!(RUST_SOURCE.contains("let output = output_rx"));
    assert!(RUST_SOURCE.contains(".recv()"));
    assert!(!RUST_SOURCE.contains("chan::Sender"));
}

#[test]
fn flattened_failure_fields_are_accessed_through_the_rust_field() {
    assert!(RUST_SOURCE.contains("res.Failure.Position.Start.Filename"));
    assert!(RUST_SOURCE.contains("res.Failure.RuleName"));
    assert!(!RUST_SOURCE.contains("res.Position.Start.Filename"));
}

#[test]
fn run_keeps_go_configuration_filtering_and_error_paths() {
    for required in [
        "goversion::NewVersion(\"1.21\")",
        "Confidence: 0.8",
        "\"loop\", \"method-call\", \"immediate-recover\", \"return\"",
        "if f.Confidence < conf.Confidence",
        "log::Error(\"Format error\", zap::Error(err))",
        "json::Unmarshal(output.as_bytes())?",
        "util::FindOffset(",
        "pass.Reportf(",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
}
