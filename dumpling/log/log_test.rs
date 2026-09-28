// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// InitAppLogger 文件权限边界单测，对应 Go log_test.go TestInitLogNoPermission。
// root 用户跳过 chmod 场景；非 root 覆盖目录/只读目录/目录作文件名/只读文件等错误路径。

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Config, InitAppLogger, Level, ZapLogger};

// 临时目录 RAII：记录需恢复权限的路径，Drop 时 chmod 700 并 remove_dir_all。
struct TempDir {
    path: PathBuf,
    paths_requiring_restore: Vec<PathBuf>,
}

impl TempDir {
    // 在系统 temp 下创建 pid+纳秒唯一目录，避免并行测试冲突。
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "astersql-dumpling-log-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after Unix epoch")
                .as_nanos()
        ));
        fs::create_dir(&path).expect("create temporary directory");
        Self {
            path,
            // 初始无需恢复；仅 set_mode 收紧权限后才登记。
            paths_requiring_restore: Vec::new(),
        }
    }

    // 暴露临时根路径，供用例拼接日志文件名。
    fn path(&self) -> &Path {
        &self.path
    }

    // 设置 Unix 权限；非 0o700 时登记以便 Drop 恢复，避免污染后续用例。
    fn set_mode(&mut self, path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .unwrap_or_else(|err| panic!("chmod {} to {mode:o}: {err}", path.display()));
        if mode & 0o700 != 0o700 {
            self.paths_requiring_restore.push(path.to_owned());
        } else {
            self.paths_requiring_restore.retain(|entry| entry != path);
        }
    }
}

impl Drop for TempDir {
    // 逆序恢复 chmod 过的路径，再删整个临时树。
    fn drop(&mut self) {
        for path in self.paths_requiring_restore.iter().rev() {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        }
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&self.path);
    }
}

// 读取有效 UID；失败时返回 u32::MAX 使 root 检测为 false。
fn effective_uid() -> u32 {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|output| output.trim().parse().ok())
        .unwrap_or(u32::MAX)
}

// 断言 Result 错误信息包含期望子串，与 Go require.Contains 对齐。
fn assert_error_contains<T>(result: Result<T, String>, expected: &str) {
    let err = result
        .err()
        .unwrap_or_else(|| panic!("expected error containing {expected:?}"));
    assert!(
        err.contains(expected),
        "expected error containing {expected:?}, got {err:?}"
    );
}

#[test]
#[allow(non_snake_case)]
// 测试名与 Go TestInitLogNoPermission 一致，便于对照 CI 日志。
fn TestInitLogNoPermission() {
    // root 无法可靠 chmod 000，Go 侧同样 skip。
    if effective_uid() == 0 {
        return;
    }

    let mut tmp_dir = TempDir::new();
    // 与 Go 用例相同：从可写基线出发，再逐项收紧权限验证错误文案。
    let mut conf = Config {
        Level: "debug".to_owned(),
        File: tmp_dir
            .path()
            .join("test.log")
            .to_string_lossy()
            .into_owned(),
        Format: "text".to_owned(),
        ..Config::default()
    };

    // 基线：可写路径应成功，且 stacktrace_at 为 DPanic。
    // 后续场景会覆写 conf.File，但 Level/Format 保持不变。
    let (logger, _properties) = InitAppLogger(&conf).expect("initialize writable log path");
    assert_eq!(logger.stacktrace_at(), crate::Level::DPanic);

    // Directory permission denied.
    // 根临时目录 chmod 000 → 建目录/写文件失败，错误含 permission denied。
    // 失败后必须恢复 755，否则后续 fixture 无法创建。
    let root_path = tmp_dir.path().to_owned();
    tmp_dir.set_mode(&root_path, 0o000);
    assert_error_contains(InitAppLogger(&conf), "permission denied");
    tmp_dir.set_mode(&root_path, 0o755);

    // Directory exists but doesn't allow file creation.
    // 只读子目录 555：可在父级建目录但无法在子目录创建日志文件。
    // 对应 Go 中对父目录可遍历、子目录不可写的组合。
    let read_only_dir_path = tmp_dir.path().join("readonly-dir");
    fs::create_dir(&read_only_dir_path).expect("create read-only fixture directory");
    tmp_dir.set_mode(&read_only_dir_path, 0o555);
    conf.File = read_only_dir_path
        .join("test.log")
        .to_string_lossy()
        .into_owned();
    assert_error_contains(InitAppLogger(&conf), "permission denied");
    // 恢复子目录权限，避免 Drop 前清理失败。
    tmp_dir.set_mode(&read_only_dir_path, 0o755);

    // Using a directory as log file.
    // 日志路径指向已存在目录 → can't use directory as log file name。
    let dir_log_path = tmp_dir.path().join("dir-as-log");
    fs::create_dir(&dir_log_path).expect("create directory-as-log fixture");
    conf.File = dir_log_path.to_string_lossy().into_owned();
    assert_error_contains(InitAppLogger(&conf), "can't use directory as log file name");

    // File exists but is not writable.
    // 已存在只读文件 444 → permission denied。
    let file_path = tmp_dir.path().join("readonly.log");
    drop(File::create(&file_path).expect("create read-only log fixture"));
    tmp_dir.set_mode(&file_path, 0o444);
    conf.File = file_path.to_string_lossy().into_owned();
    assert_error_contains(InitAppLogger(&conf), "permission denied");
    // 恢复文件可写
    tmp_dir.set_mode(&file_path, 0o644);

    // Ensure parent directory is created successfully.
    // 嵌套父路径不存在时 init_file_log 应 create_dir_all，InitAppLogger 成功。
    let nested_path = tmp_dir.path().join("nested/path/to");
    conf.File = nested_path.join("test.log").to_string_lossy().into_owned();
    InitAppLogger(&conf).expect("initialize nested log path");
    // nested/path/to 目录应由 create_dir_all 创建，不要求日志文件此时存在。
    assert!(nested_path.is_dir(), "nested parent directory must exist");
}

#[test]
fn file_logger_rotates_at_configured_max_size() {
    let tmp_dir = TempDir::new();
    let log_path = tmp_dir.path().join("rotate.log");
    let (logger, _properties) = InitAppLogger(&Config {
        Level: "info".to_owned(),
        File: log_path.to_string_lossy().into_owned(),
        FileMaxSize: 1,
        FileMaxBackups: 2,
        Format: "text".to_owned(),
        ..Config::default()
    })
    .expect("initialize rotating file logger");

    let payload = "x".repeat(600 * 1024);
    logger.Info(&payload, []);
    logger.Info(&payload, []);

    let log_files = fs::read_dir(tmp_dir.path())
        .expect("read temporary log directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .count();
    assert_eq!(
        log_files, 2,
        "Go pingcap/log uses lumberjack and must retain the rotated file plus the active log"
    );
    assert!(log_path.is_file(), "rotation must recreate the active log");
}

#[test]
fn terminal_log_levels_match_zap_behavior() {
    let logger = ZapLogger::capture(Level::Debug);
    logger.DPanic("diagnostic panic", []);
    assert!(
        logger.entries()[0].contains("[DPANIC] diagnostic panic"),
        "DPanic must log without panicking in the production logger"
    );

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        logger.Panic("terminal panic", []);
    }));
    assert!(panic.is_err(), "zap Panic must panic after logging");
    assert!(
        logger
            .entries()
            .iter()
            .any(|entry| entry.contains("[PANIC] terminal panic")),
        "Panic must write its record before unwinding"
    );
}

#[test]
fn fatal_level_logs_before_exit() {
    const CHILD_LOG: &str = "ASTERSQL_DUMPLING_LOG_FATAL_CHILD";
    if let Some(path) = std::env::var_os(CHILD_LOG) {
        let (logger, _) = InitAppLogger(&Config {
            File: PathBuf::from(path).to_string_lossy().into_owned(),
            Format: "text".to_owned(),
            ..Config::default()
        })
        .expect("initialize child file logger");
        logger.Fatal("fatal record", []);
    }

    let tmp_dir = TempDir::new();
    let log_path = tmp_dir.path().join("fatal.log");
    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", "log_test::fatal_level_logs_before_exit"])
        .env(CHILD_LOG, &log_path)
        .status()
        .expect("run fatal logger child");
    assert_eq!(status.code(), Some(1), "Fatal must exit with status 1");
    let record = fs::read_to_string(log_path).expect("read fatal log record");
    assert!(
        record.contains("[FATAL] fatal record"),
        "Fatal must write before exiting: {record:?}"
    );
}
