// Copyright 2026 AsterSQL.

// TopSQL reporter 的 build 脚本：适配源文件后写入 OUT_DIR。
//
// 去掉 `ru_datamodel.rs` 中重复的 allow 属性；将 `pubsub.rs` 中
// `matches!(collector, …)` 改为解引用形式 `matches!(*collector, …)`，
// 以匹配当前类型布局。源文件变更时通过 `cargo:rerun-if-changed` 触发重建。

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("out dir"));

    // 去掉 crate 级 allow，避免被 include! 进子模块时属性位置非法。
    let source = manifest_dir.join("ru_datamodel.rs");
    let contents = fs::read_to_string(&source).expect("read ru_datamodel.rs");
    let contents = contents.replace(
        "#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]\n",
        "",
    );
    fs::write(out_dir.join("ru_datamodel.rs"), contents).expect("write adapted ru_datamodel.rs");
    println!("cargo:rerun-if-changed={}", source.display());

    // pubsub 中 collector 现为引用，matches! 需解引用一次以保持模式匹配成立。
    let source = manifest_dir.join("pubsub.rs");
    let contents = fs::read_to_string(&source).expect("read pubsub.rs");
    let adapted = contents.replacen(
        "matches!(\n                collector,",
        "matches!(\n                *collector,",
        1,
    );
    assert_ne!(contents, adapted, "pubsub collector match shape changed");
    fs::write(out_dir.join("pubsub.rs"), adapted).expect("write adapted pubsub.rs");
    println!("cargo:rerun-if-changed={}", source.display());
}
