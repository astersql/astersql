// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// testdata 加载与录制兼容性测试。
//
// 验证 Rust 实现能够读取带 Go 风格行注释的 fixture，并严格区分普通输出与
// cascades 输出；录制模式只改写已加载的输出文件，不应额外生成 cascades 文件。

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use crate::testdata::{LoadTestSuiteDataWithCascades, SetRecord};

static RECORD_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 为每个用例创建进程内唯一的临时 fixture 目录，避免并发测试互相覆盖。
fn fixture_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "astersql-testkit-testdata-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).expect("create fixture directory");
    path
}

/// 覆盖 Go fixture 注释兼容、cascades 加载边界以及普通输出录制路径。
#[test]
fn loader_accepts_go_fixture_comments_and_requires_requested_cascades_output() {
    let directory = fixture_dir();
    fs::write(
        directory.join("suite_in.json"),
        "// generated fixture\n[{\"Name\":\"case\",\"Cases\":[1]}]\n",
    )
    .expect("write input fixture");
    fs::write(
        directory.join("suite_out.json"),
        serde_json::to_vec(&[json!({ "Name": "case", "Cases": [2] })]).unwrap(),
    )
    .expect("write output fixture");

    let mut data = LoadTestSuiteDataWithCascades(directory.to_str().unwrap(), "suite", false)
        .expect("load non-cascades fixture");
    assert_eq!(
        data.LoadTestCasesByName("case", false).unwrap().1,
        json!([2])
    );
    // 加载时未请求 cascades，后续读取必须报错，不能回退到普通输出。
    assert!(data.LoadTestCasesByName("case", true).is_err());

    // 全局录制开关开启后只更新普通输出，随后恢复开关以免影响其他测试。
    let _record_guard = RECORD_TEST_LOCK.lock().expect("lock record mode");
    SetRecord(true);
    data.RecordTestCasesByName("case", json!([3]), false)
        .expect("record output");
    data.flush().expect("flush output");
    SetRecord(false);

    let encoded = fs::read_to_string(directory.join("suite_out.json")).unwrap();
    assert!(encoded.contains("\"Name\""));
    assert!(!directory.join("suite_xut.json").exists());
    fs::remove_dir_all(directory).expect("remove fixture directory");
}

/// Go only rewrites the output kind that a test actually records. Loading the
/// cascades fixture must not make an ordinary recording rewrite `_xut.json`.
#[test]
fn recording_standard_output_preserves_unrelated_cascades_file() {
    let directory = fixture_dir();
    let input = serde_json::to_vec(&[json!({ "Name": "case", "Cases": [1] })]).unwrap();
    let output = serde_json::to_vec(&[json!({ "Name": "case", "Cases": [2] })]).unwrap();
    let cascades = serde_json::to_vec(&[json!({ "Name": "case", "Cases": [9] })]).unwrap();
    fs::write(directory.join("suite_in.json"), input).expect("write input fixture");
    fs::write(directory.join("suite_out.json"), output).expect("write output fixture");
    fs::write(directory.join("suite_xut.json"), &cascades).expect("write cascades fixture");

    let mut data = LoadTestSuiteDataWithCascades(directory.to_str().unwrap(), "suite", true)
        .expect("load cascades fixture");
    let _record_guard = RECORD_TEST_LOCK.lock().expect("lock record mode");
    SetRecord(true);
    data.RecordTestCasesByName("case", json!([3]), false)
        .expect("record standard output");
    data.flush().expect("flush standard output");
    SetRecord(false);

    assert_eq!(
        fs::read(directory.join("suite_xut.json")).expect("read cascades fixture"),
        cascades
    );
    fs::remove_dir_all(directory).expect("remove fixture directory");
}
