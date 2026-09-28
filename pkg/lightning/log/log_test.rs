// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `log` 模块单元测试。
//
// 覆盖 Config::Adjust、测试 Logger JSON 输出、stdout 初始化（子进程），
// 以及错误链上的取消判定。

use std::error::Error as StdError;
use std::fmt;
use std::process::Command;

use astersql_lightning_log::filter::{Field, Level};
use astersql_lightning_log::log::{self, CancellationError, Config};
use astersql_lightning_log::testlogger::MakeTestLogger;

/// 验证默认调整与“目录不能当日志文件”的校验。
#[test]
fn test_config_adjust() {
    let mut cfg = Config::default();
    cfg.Adjust();
    assert_eq!(cfg.Level, "info");

    cfg.File = ".".to_owned();
    assert_eq!(
        log::InitLogger(&cfg, "info").unwrap_err(),
        "can't use directory as log file name"
    );
}

/// 验证测试 Logger 写出的 JSON 行格式。
#[test]
fn test_test_logger() {
    let (logger, buffer) = MakeTestLogger([]);
    logger.Warn(
        "the message",
        [
            Field::int("number", 123456),
            Field::ints("array", [7, 8, 9]),
        ],
    );
    assert_eq!(
        buffer.stripped(),
        r#"{"$lvl":"WARN","$msg":"the message","number":123456,"array":[7,8,9]}"#
    );
}

/// 通过子进程验证 stdout 初始化及诊断模式下的 GRPC_DEBUG。
#[test]
fn test_init_stdout_logger() {
    const CHILD_MODE: &str = "LIGHTNING_LOG_TASK_263_CHILD";
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        // The child process has no concurrent environment readers or writers.
        // 子进程路径：初始化 Logger 并向 stdout 打一条 Info。
        unsafe { std::env::remove_var("GRPC_DEBUG") };
        let cfg = Config {
            File: "-".to_owned(),
            EnableDiagnoseLogs: mode == "diagnose",
            ..Config::default()
        };
        log::InitLogger(&cfg, "info").unwrap();
        log::L().Info(&format!("logger is initialized to stdout ({mode})"), []);
        if mode == "diagnose" {
            assert_eq!(std::env::var("GRPC_DEBUG").as_deref(), Ok("true"));
        } else {
            assert!(std::env::var_os("GRPC_DEBUG").is_none());
        }
        return;
    }

    // 父进程：分别以 filtered / diagnose 模式拉起自身子测试。
    for mode in ["filtered", "diagnose"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["test_init_stdout_logger", "--nocapture"])
            .env(CHILD_MODE, mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(&format!("logger is initialized to stdout ({mode})")));
    }
}

/// `SetLevel` must immediately update the filtering level of the initialized logger.
#[test]
fn test_set_level_updates_initialized_logger() {
    const CHILD_MODE: &str = "LIGHTNING_LOG_SET_LEVEL_CHILD";
    if let Ok(path) = std::env::var(CHILD_MODE) {
        let cfg = Config {
            Level: "info".to_owned(),
            File: path,
            EnableDiagnoseLogs: true,
            ..Config::default()
        };
        log::InitLogger(&cfg, "info").unwrap();
        log::L().Debug("before level change", []);
        assert_eq!(log::SetLevel(Level::Debug), Level::Info);
        log::L().Debug("after level change", []);
        return;
    }

    let path = std::env::temp_dir().join(format!(
        "astersql-lightning-log-set-level-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["test_set_level_updates_initialized_logger", "--nocapture"])
        .env(CHILD_MODE, &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let contents = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(path);
    assert!(!contents.contains("before level change"));
    assert!(contents.contains("after level change"));
}

/// 包装 `CancellationError` 的测试错误，用于验证错误链遍历。
#[derive(Debug)]
struct AnnotatedError {
    source: CancellationError,
}

impl fmt::Display for AnnotatedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "foo: {}", self.source)
    }
}

impl StdError for AnnotatedError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(&self.source)
    }
}

/// 各类取消错误（含包装与 Operation）均应被识别；None 则否。
#[test]
fn test_is_context_canceled_error() {
    let context = CancellationError::ContextCanceled;
    let grpc = CancellationError::GrpcCanceled;
    let smithy = CancellationError::SmithyCanceled;
    let annotated = AnnotatedError {
        source: CancellationError::ContextCanceled,
    };
    let operation = CancellationError::Operation(Box::new(CancellationError::ContextCanceled));

    for error in [
        &context as &(dyn StdError + 'static),
        &grpc,
        &annotated,
        &smithy,
        &operation,
    ] {
        assert!(log::IsContextCanceledError(Some(error)));
    }
    assert!(!log::IsContextCanceledError(None));
}
