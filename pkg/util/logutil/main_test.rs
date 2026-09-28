// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//
// 对应 Go `TestMain` / 日志格式断言用到的常量；Rust 测试框架自行驱动进程，
// test_main 适配器在套件前初始化，成功结束后检查原生线程泄漏。

/// 匹配不含额外上下文键的 zap 文本日志行（时间、级别、文件:行、消息与字段）。
pub const zapLogWithoutCheckKeyPattern: &str = r#"\[\d\d\d\d/\d\d/\d\d \d\d:\d\d:\d\d.\d\d\d\ (\+|-)\d\d:\d\d\] \[(FATAL|ERROR|WARN|INFO|DEBUG)\] \[([\w_%!$@.,+~-]+|\\.)+:\d+\] \[.*\] (\[.*=.*\]).*\n"#;
/// 匹配带 `conn=`（连接 ID）字段的 zap 日志行。
pub const zapLogWithConnIDPattern: &str = r#"\[\d\d\d\d/\d\d/\d\d \d\d:\d\d:\d\d.\d\d\d\ (\+|-)\d\d:\d\d\] \[(FATAL|ERROR|WARN|INFO|DEBUG)\] \[([\w_%!$@.,+~-]+|\\.)+:\d+\] \[.*\] \[conn=.*\] (\[.*=.*\]).*\n"#;
/// 匹配带连接 ID 与 `session_alias=`（会话别名）的 zap 日志行。
pub const zapLogWithTraceInfoPattern: &str = r#"\[\d\d\d\d/\d\d/\d\d \d\d:\d\d:\d\d.\d\d\d\ (\+|-)\d\d:\d\d\] \[(FATAL|ERROR|WARN|INFO|DEBUG)\] \[([\w_%!$@.,+~-]+|\\.)+:\d+\] \[.*\] \[conn=.*\] \[session_alias=.*\] (\[.*=.*\]).*\n"#;
/// 匹配通过 context 注入 `ctxKey=` 键值对的 zap 日志行。
pub const zapLogWithKeyValPatternByCtx: &str = r#"\[\d\d\d\d/\d\d/\d\d \d\d:\d\d:\d\d.\d\d\d\ (\+|-)\d\d:\d\d\] \[(FATAL|ERROR|WARN|INFO|DEBUG)\] \[([\w_%!$@.,+~-]+|\\.)+:\d+\] \[.*\] \[ctxKey=.*\] (\[.*=.*\]).*\n"#;
/// 匹配通过 logger core 注入 `coreKey=` 键值对的 zap 日志行。
pub const zapLogWithKeyValPatternByCore: &str = r#"\[\d\d\d\d/\d\d/\d\d \d\d:\d\d:\d\d.\d\d\d\ (\+|-)\d\d:\d\d\] \[(FATAL|ERROR|WARN|INFO|DEBUG)\] \[([\w_%!$@.,+~-]+|\\.)+:\d+\] \[.*\] \[coreKey=.*\] (\[.*=.*\]).*\n"#;

/// 导出 hex 模块的 `prettyPrint`，保持与 Go 侧 `PrettyPrint` 命名一致。
pub use super::hex::prettyPrint as PrettyPrint;

// These Go-only workers have no Rust equivalent in this crate. The custom
// TestMain checks all native workers; these names never exempt a Rust thread.

fn child(case: &str, level: Option<&str>) -> std::process::Output {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "main_test::testmain_child", "--nocapture"])
        .env("ASTERSQL_LOGUTIL_TESTMAIN_CASE", case)
        .env_remove("log_level");
    if let Some(level) = level {
        command.env("log_level", level);
    }
    command.output().unwrap()
}

#[test]
fn testmain_rejects_invalid_level_before_running_tests() {
    let output = child("panic", Some("definitely-invalid"));
    assert_eq!(
        output.status.code(),
        Some(if cfg!(windows) { -1 } else { 255 })
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("applyOSLogLevel failed:"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture panic"));
}

#[test]
fn testmain_detects_detached_threads() {
    let output = child("leak", None);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("thread leak:"));
}

#[test]
fn testmain_accepts_joined_and_finishing_threads() {
    for case in ["joined", "finishing", "none"] {
        let output = child(case, None);
        assert!(output.status.success(), "{case}: {output:?}");
    }
}

#[test]
fn testmain_preserves_test_failure() {
    let output = child("panic", None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture panic"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("thread leak:"));
}

#[test]
fn testmain_child() {
    let Ok(case) = std::env::var("ASTERSQL_LOGUTIL_TESTMAIN_CASE") else {
        return;
    };
    match case.as_str() {
        "leak" | "panic" => {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                tx.send(()).unwrap();
                loop {
                    std::thread::park();
                }
            });
            rx.recv().unwrap();
            if case == "panic" {
                panic!("fixture panic");
            }
        }
        "joined" => std::thread::spawn(|| {}).join().unwrap(),
        "finishing" => {
            std::thread::spawn(|| std::thread::sleep(std::time::Duration::from_millis(20)));
        }
        "none" => {}
        case if case.starts_with("level:") => {
            assert_eq!(
                format!("{:?}", testsetup::configured_log_level()),
                &case[6..]
            );
        }
        _ => panic!("unknown fixture {case}"),
    }
}

#[test]
fn testmain_initializes_all_go_log_levels() {
    for (level, expected) in [
        (None, "Info"),
        (Some(""), "Info"),
        (Some("debug"), "Debug"),
        (Some("INFO"), "Info"),
        (Some("warn"), "Warn"),
        (Some("warning"), "Warn"),
        (Some("error"), "Error"),
        (Some("dpanic"), "Error"),
        (Some("panic"), "Error"),
        (Some("fatal"), "Error"),
    ] {
        let output = child(&format!("level:{expected}"), level);
        assert!(output.status.success(), "{level:?}: {output:?}");
    }
}
