// Copyright 2026 AsterSQL.

const PARSER_TEST_INPUTS: &[(&str, &str)] = &[
    ("parser_test.rs", include_str!("parser_test.rs")),
    ("consistent_test.rs", include_str!("consistent_test.rs")),
    (
        "reserved_words_test.rs",
        include_str!("reserved_words_test.rs"),
    ),
    ("keywords_test.rs", include_str!("keywords_test.rs")),
];

#[test]
fn parser_tests_use_only_rust_owned_inputs() {
    let forbidden_suffixes = [concat!(".", "go"), concat!(".", "y")];
    let mut dependencies = Vec::new();

    for (path, source) in PARSER_TEST_INPUTS {
        for (line_index, line) in source.lines().enumerate() {
            if forbidden_suffixes
                .iter()
                .any(|suffix| line.contains(suffix))
            {
                dependencies.push(format!("{path}:{}: {}", line_index + 1, line.trim()));
            }
        }
    }

    assert!(
        dependencies.is_empty(),
        "parser tests still depend on non-Rust inputs:\n{}",
        dependencies.join("\n")
    );
}
