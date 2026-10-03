// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `main_test.go`: TestMain setup helpers shared by export package tests.
//!
//! 这个文件提供 export 包测试共用的最小初始化入口，
//! 主要负责准备包级 logger、列类型接收器映射以及测试默认配置。

use std::sync::OnceLock;

use astersql_dumpling_log as log;

use crate::*;

// APP_LOGGER 模拟 Go 包级全局 logger，只初始化一次供所有测试复用。
static APP_LOGGER: OnceLock<log::Logger> = OnceLock::new();

/// Go package-level `appLogger`.
pub fn app_logger() -> log::Logger {
    APP_LOGGER
        .get_or_init(|| {
            // logger 初始化前先把列类型接收器映射建好，避免测试顺序依赖。
            initColumnTypeSets();
            let conf = log::Config {
                Level: "debug".into(),
                File: String::new(),
                Format: "text".into(),
                ..Default::default()
            };
            match log::InitAppLogger(&conf) {
                Ok((logger, _guard)) => logger,
                Err(_) => log::Zap(),
            }
        })
        .clone()
}

/// Go `defaultConfigForTest`.
pub fn default_config_for_test() -> Config {
    // 这里返回的是“已经过 adjustFileFormat 规范化”的配置，便于各测试直接使用。
    let mut config = DefaultConfig();
    adjustFileFormat(&mut config).expect("adjustFileFormat");
    config
}

#[test]
fn test_main_initializes_logger_and_col_types() {
    // 这一条烟雾测试锁住共享初始化 helper 的最基本契约。
    let _ = app_logger();
    initColumnTypeSets();
    let conf = default_config_for_test();
    // 默认测试配置最终应落到 SQL 文本导出格式。
    assert_eq!(conf.FileType, FileFormatSQLTextString);
}
