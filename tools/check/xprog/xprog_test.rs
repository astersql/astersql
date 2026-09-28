// Copyright 2026 AsterSQL.

use std::fs;

use crate::xprog::get_package_info;

#[test]
fn get_package_info_matches_go_read_line_prefix_limit() {
    let dir = std::env::temp_dir().join(format!(
        "xprog-long-importcfg-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();

    let line = format!("packagefile {}=/cache/x\n", "a".repeat(4096));
    fs::write(dir.join("importcfg.link"), line).unwrap();

    let result = std::panic::catch_unwind(|| get_package_info(&dir));
    assert!(
        result.is_err(),
        "Go ReadLine returns its 4096-byte prefix with isPrefix=true; the ignored flag leaves '=' absent and slicing panics"
    );

    let _ = fs::remove_dir_all(dir);
}
