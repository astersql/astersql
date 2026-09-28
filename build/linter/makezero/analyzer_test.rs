// Copyright 2026 AsterSQL.

const RUST_SOURCE: &str = include_str!("analyzer.rs");

#[test]
fn runtime_factory_is_lazily_initialized_as_one_shared_analyzer() {
    assert!(RUST_SOURCE.contains("pub static Analyzer: once_cell::sync::Lazy<analysis::Analyzer>"));
    assert!(RUST_SOURCE.contains("once_cell::sync::Lazy::new(|| analyzer::NewAnalyzer())"));
    assert!(
        !RUST_SOURCE.contains("pub static Analyzer: analysis::Analyzer = analyzer::NewAnalyzer()")
    );
}

#[test]
fn configured_and_global_skip_keep_go_order_on_the_lazy_value() {
    let configured = RUST_SOURCE
        .find("util::SkipAnalyzerByConfig(&Analyzer);")
        .expect("missing configured skip");
    let unconditional = RUST_SOURCE
        .find("util::SkipAnalyzer(&Analyzer);")
        .expect("missing unconditional skip");
    assert!(configured < unconditional);
}

#[test]
fn completed_port_preserves_license_and_removes_placeholder_claims() {
    assert!(RUST_SOURCE.starts_with("// Copyright 2026 AsterSQL."));
    assert!(RUST_SOURCE.contains("// Copyright 2022 PingCAP, Inc."));
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("不会运行外部 makezero"));
}
