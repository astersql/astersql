// Copyright 2026 AsterSQL.

use std::sync::{Mutex, Once};

use super::printer;

struct RecordingLogger {
    messages: Mutex<Vec<String>>,
}

impl log::Log for RecordingLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.messages
                .lock()
                .expect("recording logger lock poisoned")
                .push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static LOGGER: RecordingLogger = RecordingLogger {
    messages: Mutex::new(Vec::new()),
};
static INSTALL_LOGGER: Once = Once::new();

fn install_logger() {
    INSTALL_LOGGER.call_once(|| {
        log::set_logger(&LOGGER).expect("printer test logger must install once");
        log::set_max_level(log::LevelFilter::Info);
    });
}

#[test]
fn print_info_logs_welcome_and_build_fields_without_raw_info_duplication() {
    install_logger();
    LOGGER
        .messages
        .lock()
        .expect("recording logger lock poisoned")
        .clear();
    printer::PrintInfo("TiDB");

    let messages = LOGGER
        .messages
        .lock()
        .expect("recording logger lock poisoned");
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert!(message.starts_with("Welcome to TiDB"));
    assert!(!message.contains('\n'));
    assert!(!message.contains("App Name:"));
    assert!(message.contains("Release Version="));
    assert!(message.contains("Git Commit Hash="));
    assert!(message.contains("Git Branch="));
    assert!(message.contains("UTC Build Time="));
    assert!(message.contains("Rust Version="));
}
