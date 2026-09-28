// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// `logutil` 核心单元测试。
//
// 覆盖追踪字段、文件 logger 上下文键、级别解析、慢查询/General 工厂、
// 共享文件、压缩校验、全局替换、代理环境字段与采样工厂。

use super::general_logger::{newGeneralLogConfig, newGeneralLogger};
use super::log::{
    DefaultLogFormat, EmptyFileLogConfig, FileLogConfig, LogContext, LogField, LogFieldCategory,
    LogLevel, Logger, NewLogConfig, ReplaceLogger, SetLevel, TraceInfo, WithConnID, WithKeyValue,
    WithSessionAlias, WithTraceFields, background_logger, fields_from_trace_info,
    initialize_loggers, logger_with_trace_info, proxy_fields, sample_logger_factory,
};
use super::slow_query_logger::{newSlowQueryLogConfig, newSlowQueryLogger};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// 串行化依赖全局 logger / 环境变量的测试，避免互相干扰。
/// 串行化依赖全局 logger 状态的测试，避免并行互相干扰。
fn serial_guard() -> MutexGuard<'static, ()> {
    static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 生成唯一临时日志路径。
/// 生成进程内唯一的临时日志文件路径。
fn temp_log(name: &str) -> PathBuf {
    static NEXT: OnceLock<Mutex<u64>> = OnceLock::new();
    let mut next = NEXT
        .get_or_init(|| Mutex::new(0))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *next += 1;
    std::env::temp_dir().join(format!(
        "logutil-{name}-{}-{}.log",
        std::process::id(),
        *next
    ))
}

/// 构造指向给定路径的文件日志配置。
/// 构造指向给定路径的默认文件日志配置（max_size=4096）。
fn file_config(path: &PathBuf) -> FileLogConfig {
    FileLogConfig {
        filename: path.to_string_lossy().into_owned(),
        max_size: 4096,
        ..FileLogConfig::default()
    }
}

/// 验证 `fields_from_trace_info` 对 nil/空/部分字段的映射。
#[test]
/// 验证 TraceInfo → LogField 映射：空、仅 conn、仅 alias、两者兼有。
fn TestFieldsFromTraceInfo() {
    assert!(fields_from_trace_info(None).is_empty());
    assert!(fields_from_trace_info(Some(&TraceInfo::default())).is_empty());
    assert_eq!(
        vec![LogField::U64("conn".into(), 1)],
        fields_from_trace_info(Some(&TraceInfo {
            connection_id: 1,
            ..TraceInfo::default()
        }))
    );
    assert_eq!(
        vec![LogField::String("session_alias".into(), "alias123".into())],
        fields_from_trace_info(Some(&TraceInfo {
            session_alias: "alias123".into(),
            ..TraceInfo::default()
        }))
    );
    assert_eq!(
        vec![
            LogField::U64("conn".into(), 1),
            LogField::String("session_alias".into(), "alias123".into())
        ],
        fields_from_trace_info(Some(&TraceInfo {
            connection_id: 1,
            session_alias: "alias123".into()
        }))
    );
}

/// 向文件 logger 写入多级别消息并断言字段与级别过滤。
/// 向上下文 logger 写多级日志，断言文件中仅保留 info+ 且含期望字段。
fn test_zap_logger(context: &LogContext, file_name: &PathBuf, expected_fields: &[&str]) {
    let logger = context.logger();
    logger.log(
        LogLevel::Debug,
        "debug msg",
        [LogField::String("test with key".into(), "true".into())],
    );
    for (level, message) in [
        (LogLevel::Info, "info msg"),
        (LogLevel::Warn, "warn msg"),
        (LogLevel::Error, "error msg"),
    ] {
        logger.log(
            level,
            message,
            [LogField::String("test with key".into(), "true".into())],
        );
    }

    let content = fs::read_to_string(file_name).expect("open generated log");
    let lines: Vec<_> = content.lines().collect();
    // info 级别下 debug 被过滤，应只剩 info/warn/error 三行。
    assert_eq!(3, lines.len(), "debug must be filtered at info level");
    for line in lines {
        assert!(line.contains("[test with key=true]"));
        for field in expected_fields {
            assert!(line.contains(field), "missing {field:?} in {line:?}");
        }
        assert!(!line.contains("stack"));
        assert!(!line.contains("errorVerbose"));
    }
}

/// 初始化临时文件 logger，应用字段变换后复用 `test_zap_logger`。
/// 初始化临时文件 logger，注入上下文字段后调用 `test_zap_logger`。
fn run_file_context_case(fields: impl FnOnce(LogContext) -> LogContext, expected: &[&str]) {
    let file_name = temp_log("keys");
    let cfg = NewLogConfig(
        "info",
        DefaultLogFormat,
        "",
        "",
        file_config(&file_name),
        false,
    );
    initialize_loggers(&cfg).expect("initialize file logger");
    let context = fields(LogContext::new(background_logger()));
    test_zap_logger(&context, &file_name, expected);
    fs::remove_file(file_name).expect("remove generated log");
}

/// 覆盖 WithConnID / SessionAlias / TraceFields / KeyValue 等上下文键。
#[test]
/// 覆盖 WithConnID/WithSessionAlias/WithTraceFields/WithKeyValue 等上下文注入。
fn TestZapLoggerWithKeys() {
    let _serial = serial_guard();
    run_file_context_case(|ctx| WithConnID(&ctx, 123), &["[conn=123]"]);
    run_file_context_case(
        |ctx| WithSessionAlias(&WithConnID(&ctx, 123), "alias123"),
        &["[conn=123]", "[session_alias=alias123]"],
    );
    run_file_context_case(
        |ctx| {
            ctx.with_fields([
                LogField::I64("conn".into(), 123),
                LogField::String("session_alias".into(), "alias456".into()),
            ])
        },
        &["[conn=123]", "[session_alias=alias456]"],
    );
    run_file_context_case(
        |ctx| {
            WithTraceFields(
                &ctx,
                Some(&TraceInfo {
                    connection_id: 456,
                    session_alias: "alias789".into(),
                }),
            )
        },
        &["[conn=456]", "[session_alias=alias789]"],
    );
    run_file_context_case(
        |ctx| WithTraceFields(&ctx, Some(&TraceInfo::default())),
        &["[conn=0]", "[session_alias=]"],
    );
    run_file_context_case(
        |ctx| {
            LogContext::new(logger_with_trace_info(
                ctx.logger(),
                Some(&TraceInfo {
                    connection_id: 789,
                    session_alias: "alias012".into(),
                }),
            ))
        },
        &["[conn=789]", "[session_alias=alias012]"],
    );
    run_file_context_case(
        |ctx| LogContext::new(logger_with_trace_info(ctx.logger(), None)),
        &[],
    );
    run_file_context_case(
        |ctx| WithKeyValue(&ctx, "ctxKey", "ctxValue"),
        &["[ctxKey=ctxValue]"],
    );
}

/// 验证直接在 logger 上附加 core 字段也会写入文件行。
#[test]
/// 验证 logger.with_fields 注入的核心字段会出现在文件输出中。
fn TestZapLoggerWithCore() {
    let _serial = serial_guard();
    run_file_context_case(
        |ctx| {
            LogContext::new(
                ctx.logger()
                    .with_fields([LogField::String("coreKey".into(), "coreValue".into())]),
            )
        },
        &["[coreKey=coreValue]"],
    );
}

/// 验证级别过滤与 `SetLevel` 解析（大小写不敏感）。
#[test]
/// 验证 SetLevel 切换全局级别，且初始 info 下 debug 被过滤。
fn TestSetLevel() {
    let _serial = serial_guard();
    let cfg = NewLogConfig("info", DefaultLogFormat, "", "", EmptyFileLogConfig, false);
    let background = initialize_loggers(&cfg).unwrap().background;
    background.debug("filtered");
    background.info("kept");
    assert_eq!(1, background.entries().len());
    assert_eq!(LogLevel::Warn, SetLevel("warn").unwrap());
    background.info("filtered after warn");
    background.warn("kept after warn");
    assert_eq!(2, background.entries().len());
    assert_eq!(LogLevel::Error, SetLevel("Error").unwrap());
    background.warn("filtered after error");
    background.error("kept after error");
    assert_eq!(3, background.entries().len());
    assert_eq!(LogLevel::Debug, SetLevel("DEBUG").unwrap());
    background.debug("kept after debug");
    assert_eq!(4, background.entries().len());
}

/// 验证慢查询/General 专用 logger 级别清空与文件配置派生。
#[test]
/// 慢查询与通用专用 logger：默认 info 级别过滤 debug，配置文件字段透传。
fn TestSlowQueryLoggerAndGeneralLoggerCreation() {
    let _serial = serial_guard();
    for slow in [true, false] {
        let cfg = NewLogConfig("Error", DefaultLogFormat, "", "", EmptyFileLogConfig, false);
        let logger = if slow {
            newSlowQueryLogger(&cfg)
        } else {
            newGeneralLogger(&cfg)
        }
        .expect("create dedicated logger");
        assert_eq!("Error", cfg.level);
        logger.debug("filtered");
        logger.info("kept");
        assert_eq!(1, logger.entries().len(), "dedicated level must be info");

        let file = FileLogConfig {
            filename: "test.log".into(),
            max_size: 10,
            max_days: 10,
            max_backups: 10,
            ..FileLogConfig::default()
        };
        let cfg = NewLogConfig(
            "warn",
            DefaultLogFormat,
            "test.log",
            "test.log",
            file.clone(),
            false,
        );
        let dedicated = if slow {
            newSlowQueryLogConfig(&cfg)
        } else {
            newGeneralLogConfig(&cfg)
        };
        assert_eq!(file, dedicated.file);
        assert_eq!("warn", cfg.level);
    }
}

/// 慢查询与 General 共用主日志文件时应都能写入同一文件。
#[test]
/// 慢查询与通用 logger 可写同一文件，内容均可见。
fn TestSlowQueryLoggerAndGeneralUseSameLogFileName() {
    let _serial = serial_guard();
    let file_name = temp_log("shared");
    let cfg = NewLogConfig(
        "info",
        DefaultLogFormat,
        "",
        "",
        file_config(&file_name),
        false,
    );
    let globals = initialize_loggers(&cfg).expect("initialize shared loggers");
    globals.slow_query.info("123");
    globals.general.log(
        LogLevel::Info,
        "GENERAL LOG",
        [LogField::I64("test".into(), 123)],
    );
    let content = fs::read_to_string(&file_name).expect("read shared log");
    assert!(content.contains("123"));
    assert!(content.contains("GENERAL LOG"));
    assert!(content.contains("[test=123]"));
    fs::remove_file(file_name).expect("remove shared log");
}

/// 非法压缩算法应失败，gzip 应通过。
#[test]
/// 非法 compression 应失败；gzip 应初始化成功。
fn TestCompressedLog() {
    let _serial = serial_guard();
    let file_name = temp_log("compression");
    let mut file = file_config(&file_name);
    file.compression = "xxx".into();
    let cfg = NewLogConfig("warn", DefaultLogFormat, "test.log", "", file, false);
    assert!(initialize_loggers(&cfg).is_err());

    let mut file = file_config(&file_name);
    file.compression = "gzip".into();
    let cfg = NewLogConfig("warn", DefaultLogFormat, "test.log", "", file, false);
    assert!(initialize_loggers(&cfg).is_ok());
}

/// GRPC_DEBUG 仅在值非空时启用调试日志，对齐 Go 的 len(os.Getenv(...)) > 0。
#[test]
fn grpc_debug_requires_a_non_empty_value() {
    let _serial = serial_guard();
    let key = super::log::GRPCDebugEnvName;
    let _restore = EnvRestore(vec![(key.into(), std::env::var(key).ok())]);
    let cfg = NewLogConfig("info", DefaultLogFormat, "", "", EmptyFileLogConfig, false);

    unsafe { std::env::set_var(key, "") };
    assert!(!initialize_loggers(&cfg).unwrap().grpc_debug);

    unsafe { std::env::set_var(key, "1") };
    assert!(initialize_loggers(&cfg).unwrap().grpc_debug);
}

/// `ReplaceLogger` 后全局 background 应写入新配置对应文件。
#[test]
/// ReplaceLogger 后后台 logger 应继续写入同一文件。
fn TestGlobalLoggerReplace() {
    let _serial = serial_guard();
    let file_name = temp_log("replace");
    let mut cfg = NewLogConfig(
        "info",
        DefaultLogFormat,
        "",
        "",
        file_config(&file_name),
        false,
    );
    initialize_loggers(&cfg).expect("initialize logger");
    cfg.file.max_days = 14;
    ReplaceLogger(&cfg).expect("replace logger");
    background_logger().info("after replace");
    assert!(
        fs::read_to_string(&file_name)
            .expect("read replaced logger output")
            .contains("after replace")
    );
    fs::remove_file(file_name).expect("remove replaced logger log");
}

/// RAII：测试结束时恢复环境变量。
/// Drop 时恢复测试改动过的环境变量。
struct EnvRestore(Vec<(String, Option<String>)>);
impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

/// 穷举 http/https/no_proxy 掩码组合，验证 `proxy_fields`。
#[test]
/// 按位掩码枚举 http(s)_proxy/no_proxy 组合，校验 proxy_fields 输出。
fn TestProxyFields() {
    let _serial = serial_guard();
    let envs = ["http_proxy", "https_proxy", "no_proxy"];
    let upper = ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"];
    let preset = [
        "http://127.0.0.1:8080",
        "https://127.0.0.1:8443",
        "localhost,127.0.0.1",
    ];
    let _restore = EnvRestore(
        envs.iter()
            .chain(upper.iter())
            .map(|key| ((*key).to_string(), std::env::var(key).ok()))
            .collect(),
    );
    for key in envs.iter().chain(upper.iter()) {
        unsafe { std::env::remove_var(key) };
    }
    let reverse = HashMap::from([("http_proxy", 0), ("https_proxy", 1), ("no_proxy", 2)]);

    // 三位掩码分别对应 http_proxy / https_proxy / no_proxy 是否设置。
    for mask in 0_u32..=0b111 {
        for key in envs {
            unsafe { std::env::remove_var(key) };
        }
        for index in 0..3 {
            if (1 << index) & mask != 0 {
                unsafe { std::env::set_var(envs[index], preset[index]) };
            }
        }
        let fields = proxy_fields();
        assert_eq!(mask.count_ones() as usize, fields.len());
        for field in fields {
            let LogField::String(key, value) = field else {
                panic!("proxy field must be a string")
            };
            let index = reverse[&key.as_str()];
            assert_ne!(0, (1 << index) & mask);
            assert_eq!(preset[index], value);
        }
    }
}

/// 采样工厂在同一 tick 内只放行前 `first` 条，并保留 category 字段。
#[test]
/// 采样工厂在窗口内最多输出 sample 次，并附带 category 字段。
fn TestSampleLoggerFactory() {
    let _serial = serial_guard();
    let logger = Logger::memory(LogLevel::Info);
    let factory = sample_logger_factory(
        logger.clone(),
        Duration::from_secs(60),
        3,
        vec![LogField::String(LogFieldCategory.into(), "ddl".into())],
    );
    for _ in 0..100 {
        factory().info("sample log test");
    }
    let entries = logger.entries();
    assert_eq!(3, entries.len());
    assert!(entries.iter().all(|entry| {
        entry.message == "sample log test"
            && entry
                .fields
                .contains(&LogField::String("category".into(), "ddl".into()))
    }));
}

#[test]
fn string_array_fields_keep_order_and_escape_names() {
    let names = vec!["p,0".to_owned(), "p\"1".to_owned(), "".to_owned()];
    let field = LogField::Strings("partitions".into(), names.clone());
    assert_eq!(field.key(), "partitions");
    let logger = Logger::memory(LogLevel::Info);
    logger.log(LogLevel::Info, "partition probe", [field.clone()]);
    assert_eq!(logger.entries()[0].fields, vec![field]);
    let path = std::env::temp_dir().join(format!("astersql-1301-log-{}.txt", std::process::id()));
    let file_logger = Logger::file(LogLevel::Info, path.to_string_lossy());
    file_logger.log(
        LogLevel::Info,
        "partition probe",
        [LogField::Strings("partitions".into(), names)],
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(r#"[partitions=["p,0", "p\"1", ""]]"#));
    std::fs::remove_file(path).unwrap();
}
