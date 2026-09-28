// Copyright 2026 AsterSQL.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn parser_no_legacy_source_dependency() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("parser crate lives below the workspace root");
    let mut failures = BTreeSet::new();

    check_legacy_wrapper_files(workspace, &mut failures);
    check_parser_action_dispatcher(workspace, &mut failures);
    check_workspace_metadata(workspace, &mut failures);
    check_parser_sources(workspace, &mut failures);
    check_static_generated_sources(workspace, &mut failures);
    check_parser_readme(workspace, &mut failures);

    assert!(
        failures.is_empty(),
        "parser still has legacy source dependencies:\n{}",
        failures.into_iter().collect::<Vec<_>>().join("\n")
    );
}

fn check_parser_action_dispatcher(workspace: &Path, failures: &mut BTreeSet<String>) {
    let actions = workspace.join("pkg/parser/parser_actions");
    let removed_fallback = actions.join("legacy.rs");
    if removed_fallback.exists() {
        failures.insert(format!(
            "legacy action fallback remains: {}",
            display_relative(workspace, &removed_fallback)
        ));
    }

    let dispatcher = actions.join("mod.rs");
    let source = std::fs::read_to_string(&dispatcher)
        .unwrap_or_else(|error| panic!("read {}: {error}", dispatcher.display()));
    let forbidden = [
        (
            ["mod ", "legacy", ";"].concat(),
            "declares a legacy action module",
        ),
        (
            ["legacy", "::"].concat(),
            "dispatches to a legacy action module",
        ),
        (
            ["apply_", "numeric"].concat(),
            "contains a numeric action adapter",
        ),
        (
            ["legacy_rule", "_number"].concat(),
            "maps RuleId back to a number",
        ),
    ];
    for (fragment, reason) in forbidden {
        if source.contains(&fragment) {
            failures.insert(format!(
                "{}: {reason}",
                display_relative(workspace, &dispatcher)
            ));
        }
    }
}

fn check_parser_readme(workspace: &Path, failures: &mut BTreeSet<String>) {
    let readme = workspace.join("pkg/parser/readme-rust.md");
    let source = std::fs::read_to_string(&readme)
        .unwrap_or_else(|error| panic!("read {}: {error}", readme.display()));
    let forbidden = [
        ([".", "go"].concat(), "references a generated Go source"),
        (["go", "yacc"].concat(), "documents a Go generator"),
        (
            ["parser", ".", "y"].concat(),
            "references the old main grammar",
        ),
        (
            ["hintparser", ".", "y"].concat(),
            "references the old Hint grammar",
        ),
        (
            "编译时动态生成".to_owned(),
            "documents build-time parser generation",
        ),
        (
            "自动重新生成".to_owned(),
            "documents automatic parser regeneration",
        ),
        (
            ["OUT", "DIR"].join("_"),
            "documents Cargo output-directory generation",
        ),
        (["build", ".rs"].concat(), "documents a parser build script"),
    ];
    for (fragment, reason) in forbidden {
        if source.contains(&fragment) {
            failures.insert(format!(
                "{}: {reason}",
                display_relative(workspace, &readme)
            ));
        }
    }

    for required in [
        ".astergram",
        "astersql-parsergen",
        "RuleId",
        "cargo test",
        "cargo run -p astersql-parsergen --bin astersql-parsergen -- generate",
        "cargo run -p astersql-parsergen --bin astersql-parsergen -- check",
        "generated/main_tables.rs",
        "generated/hint_tables.rs",
        "generated/lexer_tokens.rs",
        "普通编译",
        "数据库启动",
        "SQL",
    ] {
        if !source.contains(required) {
            failures.insert(format!(
                "{}: missing Rust maintenance guidance for {required}",
                display_relative(workspace, &readme)
            ));
        }
    }
}

fn check_static_generated_sources(workspace: &Path, failures: &mut BTreeSet<String>) {
    let parser = workspace.join("pkg/parser");
    let library = parser.join("lib.rs");
    let library_source = std::fs::read_to_string(&library)
        .unwrap_or_else(|error| panic!("read {}: {error}", library.display()));
    let manifest = parser.join("Cargo.toml");
    let manifest_source = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("read {}: {error}", manifest.display()));

    for output in ["main_tables.rs", "hint_tables.rs", "lexer_tokens.rs"] {
        let generated = parser.join("generated").join(output);
        if !generated.is_file() {
            failures.insert(format!(
                "committed parser output is missing: {}",
                display_relative(workspace, &generated)
            ));
        }

        let include = format!("include!(\"generated/{output}\")");
        if !library_source.contains(&include) {
            failures.insert(format!(
                "{}: does not statically include generated/{output}",
                display_relative(workspace, &library)
            ));
        }
    }

    let build_script = parser.join(["build", ".rs"].concat());
    if build_script.exists() {
        failures.insert(format!(
            "parser build script remains: {}",
            display_relative(workspace, &build_script)
        ));
    }
    if manifest_source.contains("[build-dependencies]") {
        failures.insert(format!(
            "{}: declares parser build dependencies",
            display_relative(workspace, &manifest)
        ));
    }
    if manifest_source
        .lines()
        .map(str::trim_start)
        .any(|line| line.starts_with("build ="))
    {
        failures.insert(format!(
            "{}: declares a parser build script",
            display_relative(workspace, &manifest)
        ));
    }

    let cargo_output_directory = ["OUT", "DIR"].join("_");
    if library_source.contains(&cargo_output_directory) {
        failures.insert(format!(
            "{}: consumes dynamically generated Cargo outputs",
            display_relative(workspace, &library)
        ));
    }
}

fn check_workspace_metadata(workspace: &Path, failures: &mut BTreeSet<String>) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(workspace)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .expect("read workspace metadata");
    if !output.status.success() {
        failures.insert(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        return;
    }

    let metadata = String::from_utf8(output.stdout).expect("workspace metadata is UTF-8 JSON");
    let legacy_packages = [
        ["astersql-parser-", "go", "yacc"].concat(),
        ["astersql-parser-", "generate", "_keyword"].concat(),
    ];
    for package in legacy_packages {
        if metadata.contains(&format!("\"name\":\"{package}\"")) {
            failures.insert(format!(
                "workspace metadata contains legacy package {package}"
            ));
        }
    }
}

fn check_legacy_wrapper_files(workspace: &Path, failures: &mut BTreeSet<String>) {
    let parser = workspace.join("pkg/parser");
    let wrappers = [["go", "yacc"].concat(), ["generate", "_keyword"].concat()];
    for wrapper in wrappers {
        let directory = parser.join(wrapper);
        let contains_rust_wrapper = directory.read_dir().is_ok_and(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                let path = entry.path();
                path.file_name().is_some_and(|name| name == "Cargo.toml")
                    || path.extension().is_some_and(|extension| extension == "rs")
            })
        });
        if contains_rust_wrapper {
            failures.insert(format!(
                "legacy wrapper remains: {}",
                display_relative(workspace, &directory)
            ));
        }
    }
}

fn check_parser_sources(workspace: &Path, failures: &mut BTreeSet<String>) {
    let sources = versioned_parser_sources(workspace);
    let process_call = ["Command::new(\"", "go", "\")"].concat();
    let legacy_grammar_paths = [
        ["\"parser", ".", "y\""].concat(),
        ["\"hintparser", ".", "y\""].concat(),
    ];
    let generated_source_path_end = [".", "go", "\""].concat();
    let legacy_names = [
        ["parser-", "go", "yacc"].concat(),
        ["parser-", "generate", "_keyword"].concat(),
        ["facade_parser_", "go", "yacc"].concat(),
        ["facade_parser_", "generate", "_keyword"].concat(),
        ["pkg/parser/", "go", "yacc"].concat(),
        ["pkg/parser/", "generate", "_keyword"].concat(),
    ];

    for path in sources {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let relative = display_relative(workspace, &path);
        for (line_index, code) in code_lines(&source).into_iter().enumerate() {
            let reason = if code.contains(&process_call) {
                Some("starts a Go process")
            } else if legacy_grammar_paths
                .iter()
                .any(|grammar| code.contains(grammar))
            {
                Some("references a legacy grammar path")
            } else if code.contains(&generated_source_path_end) {
                Some("references a generated Go source path")
            } else if legacy_names.iter().any(|name| code.contains(name)) {
                Some("references a legacy parser wrapper")
            } else {
                None
            };

            if let Some(reason) = reason {
                failures.insert(format!("{relative}:{}: {reason}", line_index + 1));
            }
        }
    }
}

fn versioned_parser_sources(workspace: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()
        .expect("list versioned and pending workspace files");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .expect("workspace paths are UTF-8")
        .lines()
        .filter(|relative| {
            (*relative == "Cargo.toml"
                || *relative == "pkg/lib.rs"
                || relative.starts_with("pkg/parser/"))
                && (relative.ends_with(".rs") || relative.ends_with("Cargo.toml"))
        })
        .filter(|relative| workspace.join(relative).is_file())
        .map(|relative| workspace.join(relative))
        .collect()
}

fn code_lines(source: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut in_block_comment = false;
    for line in source.lines() {
        let mut remaining = line;
        loop {
            if in_block_comment {
                if let Some((_, after)) = remaining.split_once("*/") {
                    in_block_comment = false;
                    remaining = after;
                    continue;
                }
                remaining = "";
                break;
            }
            if let Some((before, after)) = remaining.split_once("/*") {
                lines.push(before.split_once("//").map_or(before, |(code, _)| code));
                remaining = after;
                in_block_comment = true;
                continue;
            }
            break;
        }
        if !remaining.is_empty() {
            lines.push(
                remaining
                    .split_once("//")
                    .map_or(remaining, |(code, _)| code),
            );
        } else if !in_block_comment {
            lines.push("");
        }
    }
    lines
}

fn display_relative(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .display()
        .to_string()
}
