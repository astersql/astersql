// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Lightning 日志子系统迁移对齐单元测试。
//
// 对照 Go 行为核对：配置默认值与别名、基于调用方路径的过滤、
// 测试用内存 Logger 的 JSON 输出，以及任务 Begin/End 在成功、取消、失败时的日志规则。

use astersql_lightning_log::filter::{Entry, Field, FilterCore, Level};
use astersql_lightning_log::log::{CancellationError, Config, Logger};
use astersql_lightning_log::testlogger::MakeTestLogger;

/// 验证 Adjust 填充默认级别/滚动参数，并把 `warning` 规范为 `warn`。
#[test]
fn migration_config_adjust_matches_go_defaults_and_alias() {
    let mut cfg = Config::default();
    cfg.Adjust();
    assert_eq!(cfg.Level, "info");
    assert_eq!(cfg.FileMaxSize, 512);
    assert_eq!(cfg.FileMaxDays, 7);

    // Go 接受 MySQL 风格的 warning 别名，Adjust 后应写成 zap 使用的 warn。
    cfg.Level = "warning".to_owned();
    cfg.Adjust();
    assert_eq!(cfg.Level, "warn");
}

/// 验证 FilterCore 按调用方函数路径过滤，并保留 With 附加字段。
#[test]
fn migration_filter_uses_caller_function_and_preserves_with_fields() {
    let (logger, buffer) = MakeTestLogger([]);
    // 路径包含 `/lightning/` 的调用方应放行，并与 core.with 字段合并输出。
    let core = FilterCore::new(logger.core(), ["/lightning/"]);
    let core = core.with([Field::string("a", "b")]);
    core.write(
        Entry::new(Level::Warn, "the message")
            .with_caller("github.com/pingcap/tidb/pkg/lightning/log.test_filter"),
        [
            Field::int("number", 123456),
            Field::ints("array", [7, 8, 9]),
        ],
    )
    .unwrap();
    assert_eq!(
        buffer.stripped(),
        r#"{"$lvl":"WARN","$msg":"the message","a":"b","number":123456,"array":[7,8,9]}"#
    );

    // 调用方不在白名单：即使字段值含 `/lightning/`，消息仍应被丢弃。
    core.write(
        Entry::new(Level::Warn, "hidden").with_caller("github.com/pingcap/tidb/br/task.run"),
        [Field::string("stack", "/lightning/")],
    )
    .unwrap();
    assert!(!buffer.stripped().contains("hidden"));
}

/// 验证过滤是子串匹配而非目录前缀：末尾斜杠会改变命中结果。
#[test]
fn migration_filter_contains_is_not_directory_prefix_matching() {
    let (logger, buffer) = MakeTestLogger([]);
    let matching = FilterCore::new(logger.core(), ["/ingestor/ingestctrl"]);
    let entry = Entry::new(Level::Warn, "retryable write")
        .with_caller("github.com/pingcap/tidb/pkg/ingestor/ingestctrl.(*worker).runJob");
    matching.write(entry.clone(), []).unwrap();
    assert!(buffer.stripped().contains("retryable write"));

    // 带尾斜杠的模式不会匹配 `ingestctrl.(*worker)` 这段路径。
    let (logger, buffer) = MakeTestLogger([]);
    let not_matching = FilterCore::new(logger.core(), ["/ingestor/ingestctrl/"]);
    not_matching.write(entry, []).unwrap();
    assert!(buffer.stripped().is_empty());
}

/// 验证测试 Logger 输出与 Go 兼容的 `$lvl`/`$msg` JSON 形状。
#[test]
fn migration_test_logger_emits_go_compatible_json() {
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

/// 验证任务 End：成功保留字段、取消降为 DEBUG 并丢字段、失败保留错误并丢临时字段。
#[test]
fn migration_task_end_matches_success_failure_and_cancel_rules() {
    let (logger, buffer) = MakeTestLogger([]);
    let success = logger.Begin(Level::Info, "load");
    success.End(Level::Error, None, [Field::string("table", "t")]);

    // ContextCanceled：结束日志级别降到 DEBUG，且不输出 End 传入的附加字段。
    let canceled = logger.Begin(Level::Info, "download");
    let cancel = CancellationError::ContextCanceled;
    canceled.End(
        Level::Error,
        Some(&cancel),
        [Field::string("must_disappear", "yes")],
    );

    // 普通错误：使用 End 指定级别，写入 error 字段，同样丢弃临时附加字段。
    let failed = logger.Begin(Level::Info, "import");
    let err = std::io::Error::other("disk full");
    failed.End(
        Level::Warn,
        Some(&err),
        [Field::string("must_disappear", "yes")],
    );

    // 三次 Begin/End 共 6 行：每条任务各有开始与结束各一行。
    let lines = buffer.lines();
    assert_eq!(lines.len(), 6);
    assert!(lines[1].contains(r#""$msg":"load completed"#));
    assert!(lines[1].contains(r#""table":"t"#));
    assert!(lines[3].contains(r#""$lvl":"DEBUG"#));
    assert!(lines[3].contains(r#""$msg":"download canceled"#));
    assert!(!lines[3].contains("must_disappear"));
    assert!(lines[5].contains(r#""$lvl":"WARN"#));
    assert!(lines[5].contains(r#""error":"disk full"#));
    assert!(!lines[5].contains("must_disappear"));
}

/// 验证 Named/With 派生 Logger 与父级共享同一内存 sink。
#[test]
fn migration_logger_with_and_named_share_the_same_sink() {
    let (logger, buffer) = MakeTestLogger([]);
    let child: Logger = logger.Named("worker").With([Field::string("engine", "42")]);
    child.Info("ready", []);
    let output = buffer.stripped();
    assert!(output.contains(r#""logger":"worker"#));
    assert!(output.contains(r#""engine":"42"#));
}
