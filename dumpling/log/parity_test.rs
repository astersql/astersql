// Copyright 2026 AsterSQL.

//! Parity tests for `dumpling/log` public contracts vs Go `log.go` / `log_test.go`.
// dumpling/log 公开契约 parity 测试：对照 Go log.go 与 log_test.go 的行为与错误文本。

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::{Config, Field, InitAppLogger, NewAppLogger, ShortError, Zap, ZapLogger};

// 创建进程级唯一临时目录，供文件路径类用例使用。
fn tmp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "astersql-dumpling-log-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn chmod(path: &Path, mode: u32) {
    // Unix 权限桩，parity 与 log_test 共用 chmod 模式。
    let perms = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, perms).expect("chmod");
}

// 检测是否 root；权限类用例与 Go 一致在 root 下跳过。
fn is_root() -> bool {
    use std::process::Command;
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1)
        == 0
}

#[test]
// 聚合四类契约子场景，单测失败时可定位到具体 contract_* 函数。
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary_format_and_level();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
fn init_checks_file_before_parsing_logger_config() {
    let tmp = tmp_dir();
    let directory_as_file = tmp.join("log-directory");
    fs::create_dir(&directory_as_file).expect("create directory-as-file fixture");

    let err = InitAppLogger(&Config {
        Level: "invalid".into(),
        File: directory_as_file.to_string_lossy().into_owned(),
        Format: "invalid".into(),
        ..Config::default()
    })
    .expect_err("pingcap/log validates the file sink before level and format");
    assert!(
        err.contains("can't use directory as log file name"),
        "unexpected error precedence: {err}"
    );

    fs::remove_dir_all(tmp).expect("remove temporary directory");
}

#[test]
fn json_format_writes_json_records() {
    let tmp = tmp_dir();
    let file = tmp.join("json.log");
    let (logger, _) = InitAppLogger(&Config {
        Level: "info".into(),
        File: file.to_string_lossy().into_owned(),
        Format: "json".into(),
        ..Config::default()
    })
    .expect("initialize json logger");

    logger.Info("quoted \"message\"", [Field::string("key", "line\nvalue")]);
    let record = fs::read_to_string(&file).expect("read json log record");
    assert!(
        record.starts_with('{'),
        "expected JSON object, got {record:?}"
    );
    assert!(record.contains("\"level\":\"INFO\""), "record: {record}");
    assert!(
        record.contains("\"message\":\"quoted \\\"message\\\"\""),
        "record: {record}"
    );
    assert!(
        record.contains("\"key\":\"line\\nvalue\""),
        "record: {record}"
    );

    fs::remove_dir_all(tmp).expect("remove temporary directory");
}

#[test]
fn with_fields_keeps_the_same_log_sink() {
    let base = ZapLogger::capture(crate::Level::Info);
    let child = base.with([Field::string("component", "dump")]);
    child.Info("shared", []);

    let entries = base.entries();
    assert_eq!(
        entries.len(),
        1,
        "zap.With must preserve the original core/sink"
    );
    assert!(
        entries[0].contains("component=dump"),
        "entry: {:?}",
        entries[0]
    );
}

// Zap/NewAppLogger/InitAppLogger/ShortError 正常路径契约。
/// Normal: Zap nop, NewAppLogger wrap, InitAppLogger stdout, ShortError message.
// 正常路径：Zap nop、NewAppLogger 包装、stdout InitAppLogger、ShortError 字段形状。
fn contract_normal_paths() {
    // 验证 capture→NewAppLogger→Info 链路能写入 Memory sink。
    let zap = Zap();
    // 包级 Zap 为 nop：stacktrace_at 默认 Error，无内存条目。
    assert_eq!(zap.stacktrace_at(), crate::Level::Error);
    assert!(zap.entries().is_empty());

    let capture = ZapLogger::capture(crate::Level::Debug);
    let wrapped = NewAppLogger(capture.clone());
    wrapped.Info("hello", [Field::string("k", "v")]);
    let entries = wrapped.entries();
    assert_eq!(entries.len(), 1);
    // 文本行应含级别、消息与字段 k=v。
    assert!(entries[0].contains("[INFO] hello"));
    assert!(entries[0].contains("k=v"));

    let (logger, props) = InitAppLogger(&Config {
        Level: "info".into(),
        File: String::new(),
        Format: "text".into(),
        ..Config::default()
    })
    .expect("stdout logger");
    // stdout 模式 Filename 为空，Level/Format 来自配置。
    assert_eq!(props.Level, crate::Level::Info);
    assert!(props.Filename.is_empty());
    assert_eq!(props.Format, "text");
    // Go InitAppLogger adds AddStacktrace(DPanicLevel).
    // Go InitAppLogger 附加 AddStacktrace(DPanicLevel)。
    assert_eq!(logger.stacktrace_at(), crate::Level::DPanic);

    let err = std::io::Error::new(std::io::ErrorKind::Other, "boom");
    let field = ShortError(Some(&err));
    assert!(!field.is_skip());
    assert_eq!(field.key, "error");
    assert_eq!(field.value, "boom");
    // None 分支必须 skip，避免空 error 字段污染日志。
    assert!(ShortError(None).is_skip());
}

// Level/Format 默认值、FileMaxSize 回落、非法配置错误文本。
/// Boundary: empty level/format, default max size, nested parent creation.
// 边界：空 level/format 默认值、FileMaxSize=0 回落 300、嵌套目录预创建与探针文件删除。
fn contract_boundary_format_and_level() {
    let (logger, props) = InitAppLogger(&Config::default()).expect("defaults");
    // Config 零值：Level 空→Info，Format 空→text。
    assert_eq!(props.Level, crate::Level::Info);
    assert_eq!(props.Format, "text");
    assert_eq!(logger.stacktrace_at(), crate::Level::DPanic);

    let dir = tmp_dir();
    let nested = dir.join("nested/path/to");
    let file = nested.join("test.log");
    // FileMaxSize=0 时应回落 300，Format=json 写入 props。
    let (logger, props) = InitAppLogger(&Config {
        Level: "debug".into(),
        File: file.to_string_lossy().into(),
        Format: "json".into(),
        FileMaxSize: 0, // pingcap/log default 300
        ..Config::default()
    })
    .expect("nested init");
    assert_eq!(props.Format, "json");
    assert_eq!(props.FileMaxSize, 300);
    assert_eq!(props.Level, crate::Level::Debug);
    assert_eq!(logger.stacktrace_at(), crate::Level::DPanic);
    // 嵌套父目录应由 init_file_log create_dir_all。
    assert!(nested.is_dir(), "parent dirs must be created eagerly");
    // initFileLog creates then removes empty file for lumberjack.
    // initFileLog 创建后删除空探针文件，lumberjack 首次写入时再创建。
    assert!(
        !file.exists(),
        "empty probe file removed; lumberjack creates on write"
    );

    let err = InitAppLogger(&Config {
        Level: "nope".into(),
        ..Config::default()
    });
    // 非法 level 字符串应返回 unrecognized level。
    assert!(err.unwrap_err().contains("unrecognized level"));

    let err = InitAppLogger(&Config {
        Format: "xml".into(),
        ..Config::default()
    });
    // 不支持 format 时错误文案含 unsupport log format。
    assert!(err.unwrap_err().contains("unsupport log format"));

    let _ = fs::remove_dir_all(&dir);
}

// 对齐 Go TestInitLogNoPermission：permission denied 与目录作文件名。
/// Error: permission denied paths and directory-as-file (Go TestInitLogNoPermission).
// 错误路径：权限拒绝与目录作文件名，对齐 Go TestInitLogNoPermission 断言子串。
fn contract_error_paths() {
    if is_root() {
        return;
    }

    let tmp = tmp_dir();
    let conf_file = tmp.join("test.log");
    let mut conf = Config {
        Level: "debug".into(),
        File: conf_file.to_string_lossy().into(),
        Format: "text".into(),
        ..Config::default()
    };
    InitAppLogger(&conf).expect("initial writable path");

    // 以下四个子场景顺序与 Go log_test.go 保持一致。
    // Directory permission denied
    // 临时根目录 000 → permission denied。
    chmod(&tmp, 0o000);
    let err = InitAppLogger(&conf).expect_err("chmod 000 dir");
    assert!(
        err.contains("permission denied"),
        "expected permission denied, got {err}"
    );
    chmod(&tmp, 0o755);
    // 恢复 tmp 权限供后续子场景

    // Directory exists but doesn't allow file creation
    // 只读子目录内创建日志 → permission denied。
    let read_only = tmp.join("readonly-dir");
    fs::create_dir(&read_only).unwrap();
    chmod(&read_only, 0o555);
    conf.File = read_only.join("test.log").to_string_lossy().into();
    let err = InitAppLogger(&conf).expect_err("readonly dir");
    assert!(
        err.contains("permission denied"),
        "expected permission denied, got {err}"
    );
    // 恢复只读子目录权限，避免影响后续用例与清理。
    chmod(&read_only, 0o755);

    // Using a directory as log file
    // 路径为目录 → can't use directory as log file name。
    let dir_as_log = tmp.join("dir-as-log");
    fs::create_dir(&dir_as_log).unwrap();
    conf.File = dir_as_log.to_string_lossy().into();
    let err = InitAppLogger(&conf).expect_err("dir as file");
    assert!(
        err.contains("can't use directory as log file name"),
        "got {err}"
    );

    // File exists but is not writable
    // 只读已有文件 → permission denied。
    let readonly_log = tmp.join("readonly.log");
    File::create(&readonly_log).unwrap();
    chmod(&readonly_log, 0o444);
    conf.File = readonly_log.to_string_lossy().into();
    let err = InitAppLogger(&conf).expect_err("readonly file");
    assert!(
        err.contains("permission denied"),
        "expected permission denied, got {err}"
    );
    chmod(&readonly_log, 0o644);
    // 恢复日志文件权限

    // 清理 parity 临时目录
    let _ = fs::remove_dir_all(&tmp);
}

/// Resource: nested path survives init; Zap global stays nop (Go never mutates appLogger).
// 资源与全局状态：InitAppLogger 不替换包级 Zap nop；写入后日志文件存在。
fn contract_resource_cleanup() {
    let tmp = tmp_dir();
    let nested = tmp.join("nested/path/to");
    let file = nested.join("test.log");
    let before = Zap();
    // 记录 Init 前全局 Zap 的 stacktrace 阈值作对照。
    let (_logger, _props) = InitAppLogger(&Config {
        Level: "debug".into(),
        File: file.to_string_lossy().into(),
        Format: "text".into(),
        ..Config::default()
    })
    .expect("nested");
    assert!(nested.is_dir());
    // InitAppLogger must not replace package-level Zap() nop.
    // InitAppLogger 不得修改包级 Zap() 的 nop 全局 logger。
    assert_eq!(Zap().stacktrace_at(), before.stacktrace_at());
    // 全局 nop 仍无捕获条目
    assert!(Zap().entries().is_empty());

    // Writing through returned logger creates the file (lumberjack-equivalent sink).
    // 经返回的 logger 写入应在 sink 侧创建日志文件。
    let (logger, props) = InitAppLogger(&Config {
        Level: "info".into(),
        File: tmp.join("write.log").to_string_lossy().into(),
        Format: "text".into(),
        ..Config::default()
    })
    .expect("write path");
    // Probe file was removed; first write recreates.
    // 探针文件已删，首次 Info 写入后文件应存在。
    logger.Info("resource", []);
    // 文件路径来自 props.Filename 或 tmp/write.log 二选一存在即可。
    assert!(
        Path::new(&props.Filename).exists() || tmp.join("write.log").exists(),
        "log file created on write"
    );

    let _ = fs::remove_dir_all(&tmp);
}
