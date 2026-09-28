// Copyright 2026 AsterSQL.

// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

const RUST_SOURCE: &str = include_str!("exclude.rs");

mod implementation {
    include!("exclude.rs");
}

#[test]
fn test_should_run_matches_go_cases() {
    assert!(implementation::shouldRun("gofmt", "some.go"));
    assert!(!implementation::shouldRun("gofmt", "uca_generated.go"));
    assert!(implementation::shouldRun(
        "revive",
        "/pkg/meta/distributed_lock.go"
    ));
}

#[test]
fn only_files_precedes_exclude_files_and_bad_patterns_panic() {
    let mut config = crate::build::AnalysisConfig::default();
    config.OnlyFiles = Some(std::collections::HashMap::from([(
        "allowed/".to_string(),
        String::new(),
    )]));
    config.ExcludeFiles = Some(std::collections::HashMap::from([(
        "allowed/".to_string(),
        String::new(),
    )]));
    assert!(implementation::shouldRunConfig(&config, "allowed/file.go"));
    assert!(!implementation::shouldRunConfig(&config, "other/file.go"));

    config.OnlyFiles = Some(std::collections::HashMap::new());
    config.ExcludeFiles = None;
    assert!(!implementation::shouldRunConfig(&config, "any/file.go"));

    config.OnlyFiles = None;
    config.ExcludeFiles = Some(std::collections::HashMap::new());
    assert!(implementation::shouldRunConfig(&config, "any/file.go"));

    config.OnlyFiles = None;
    config.ExcludeFiles = Some(std::collections::HashMap::from([(
        "[".to_string(),
        String::new(),
    )]));
    assert!(
        std::panic::catch_unwind(|| { implementation::shouldRunConfig(&config, "file.go") })
            .is_err()
    );
}

#[test]
fn repository_regex_subset_matches_go_examples() {
    for (pattern, text, expected) in [
        (".*_generated\\.go$", "uca_generated.go", true),
        (".*_generated\\.go$", "uca_generated.go.tmp", false),
        ("pkg/parser/", "/src/pkg/parser/parser.go", true),
        ("^pkg/parser/", "/src/pkg/parser/parser.go", false),
        ("^pkg/parser/", "pkg/parser/parser.go", true),
        (".*.cgo1.go", "x.cgo1.go", true),
    ] {
        assert_eq!(
            implementation::regexMatch(pattern, text).expect("valid Go regex"),
            expected,
            "pattern {pattern:?}, text {text:?}"
        );
    }
    assert!(implementation::regexMatch("trailing\\", "file.go").is_err());
    assert!(implementation::regexMatch("*bad", "file.go").is_err());
}

#[test]
fn valid_go_regex_operators_are_not_rejected() {
    for (pattern, text, expected) in [
        (r"^[a-z]+\.go$", "parser.go", true),
        (r"^(foo|bar)\.go$", "bar.go", true),
        (r"^file[0-9]{2}\.go$", "file42.go", true),
        (r"^colou?r\.go$", "color.go", true),
    ] {
        assert_eq!(
            implementation::regexMatch(pattern, text).expect("valid Go regex"),
            expected,
            "pattern {pattern:?}, text {text:?}"
        );
    }
}

#[test]
fn every_checked_in_nogo_pattern_is_supported() {
    for (analyzer, config) in crate::build::NogoConfig.iter() {
        for pattern in config
            .OnlyFiles
            .iter()
            .chain(config.ExcludeFiles.iter())
            .flat_map(|patterns| patterns.keys())
        {
            assert!(
                implementation::regexMatch(pattern, "representative/file.go").is_ok(),
                "unsupported {analyzer} pattern: {pattern}"
            );
        }
    }
}

#[test]
fn should_run_port_is_executable_against_the_rust_config_shape() {
    for required in [
        "// Copyright 2026 AsterSQL.",
        "if let Some(onlyFiles) = &config.OnlyFiles",
        "if let Some(excludeFiles) = &config.ExcludeFiles",
        "regexMatch(f, fileName)",
    ] {
        assert!(RUST_SOURCE.contains(required), "missing: {required}");
    }
    assert!(!RUST_SOURCE.contains("当前不保证可编译"));
    assert!(!RUST_SOURCE.contains("regexp::MatchString"));
}

#[test]
fn should_run_keeps_go_precedence_and_invalid_regex_contract() {
    let only = RUST_SOURCE
        .find("if let Some(onlyFiles) = &config.OnlyFiles")
        .unwrap();
    let exclude = RUST_SOURCE
        .find("if let Some(excludeFiles) = &config.ExcludeFiles")
        .unwrap();
    assert!(only < exclude, "only_files must take precedence");
    assert!(RUST_SOURCE.contains("panic!(\"regex is wrong: {}\", f)"));
}
