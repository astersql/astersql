// Copyright 2026 AsterSQL.

use super::log::{FileLogConfig, LogConfig, LogField, LogLevel, Logger};
use super::slow_query_logger::{new_slow_query_logger, new_slow_query_logger_from_logger};
use std::fs;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn serial_guard() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn temp_log(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("logutil-slow-{name}-{}.log", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn slow_query_factories_replace_the_normal_file_encoder() {
    let _serial = serial_guard();
    for from_existing in [false, true] {
        let path = temp_log(if from_existing { "existing" } else { "config" });
        let _ = fs::remove_file(&path);
        let logger = if from_existing {
            new_slow_query_logger_from_logger(&Logger::file(LogLevel::Info, &path))
        } else {
            new_slow_query_logger(&LogConfig {
                slow_query_file: path.clone(),
                file: FileLogConfig::default(),
                ..LogConfig::default()
            })
            .expect("create slow query logger")
        };
        logger.log(
            LogLevel::Info,
            "select 1;",
            [LogField::String("ignored".into(), "field".into())],
        );

        let output = fs::read_to_string(&path).expect("read slow query log");
        assert!(
            output.starts_with("# Time: "),
            "unexpected output: {output:?}"
        );
        assert!(
            output.ends_with("\nselect 1;\n"),
            "unexpected output: {output:?}"
        );
        assert!(
            !output.contains("ignored"),
            "fields must be ignored: {output:?}"
        );
        fs::remove_file(path).expect("remove slow query log");
    }
}
