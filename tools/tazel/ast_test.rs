// Copyright 2026 AsterSQL.

use std::fs;

use crate::ast::{initCount, scan};

#[test]
fn scan_rejects_go_syntax_errors() {
    initCount();
    let dir = std::env::temp_dir().join(format!("tazel-ast-invalid-go-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("invalid_test.go");
    fs::write(&source, b"package invalid\nfunc TestInvalid() { ??? }\n").unwrap();

    let result = scan(source.to_str().unwrap());
    let _ = fs::remove_dir_all(&dir);

    assert!(result.is_err(), "Go parser rejects invalid function bodies");
}
