// Copyright 2026 AsterSQL.

// 仓库内解析表消费路径契约测试。

use std::{fs, path::Path};

#[test]
fn parser_uses_checked_in_generated_tables() {
    let parser_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let library = fs::read_to_string(parser_root.join("lib.rs"))
        .expect("parser library source should be readable");
    let manifest = fs::read_to_string(parser_root.join("Cargo.toml"))
        .expect("parser manifest should be readable");

    for output in ["main_tables.rs", "hint_tables.rs", "lexer_tokens.rs"] {
        let include = format!("include!(\"generated/{output}\")");
        assert!(
            library.contains(&include),
            "parser library must consume checked-in {output}"
        );
        assert!(
            parser_root.join("generated").join(output).is_file(),
            "checked-in generated output {output} must exist"
        );
    }

    let cargo_output_directory = ["OUT", "DIR"].join("_");
    assert!(
        !library.contains(&cargo_output_directory),
        "parser library must not consume build-script outputs"
    );
    assert!(
        !parser_root.join("build.rs").exists(),
        "parser build script must be removed"
    );
    assert!(
        !manifest.contains("[build-dependencies]"),
        "parser manifest must not declare build dependencies"
    );
}
