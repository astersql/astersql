// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Go-equivalent tests for `br/pkg/operation/context_test.go`.
//!
//! 覆盖 NewContext、SetHintField 多场景与 LockMeta 正/负向校验。
//! 通过 begin_log_capture 断言 Info/Warn 次数与字段，对齐 Go zap observer。
//! 拷贝独立性与 HintFields 返回副本是切片 clone 语义的关键回归点。
//! 未初始化 Context 写入非空 hint 必须被忽略，防止零值污染锁 Hint。
//! 各子块用独立 `_guard` 作用域，避免捕获缓冲跨场景串味。
//! LockMeta 正向用例固定 UTC 时间戳，保证 Hint 字符串可精确匹配。
//! 负向表驱动用例检查错误子串，而非完整英文句子，降低措辞漂移成本。
//! logged_string_field 在缺字段时 panic，使失败点指向具体 zap 字段名。
//! 不直接测 hostname 探测路径，避免 CI 主机名环境差异导致脆断言。
//! SetHintField 空 key 路径由实现短路，此处不单独开用例。
//! 与 Go 同名消息字符串必须字节级一致，否则 FilterMessage 会静默漏计。

use crate::{
    CapturedLog, Context, HintField, LockResourceMigrationRead, LockResourceType, NewContext,
    begin_log_capture, captured_logs, filter_captured_message, time_utc,
};

/// 新上下文必须生成非空 OperationID，且 StartedAt 非 Go 零值时间。
/// 对应 Go：创建后即可参与锁与 hint 写入。
#[test]
fn test_new_context_generates_operation_i_d() {
    // command 参数仅影响 started 日志，不进入 Context 字段。
    let ctx = NewContext("log-restore").expect("NewContext");
    // UUID 字符串非空即可；格式由生产路径保证。
    assert!(!ctx.OperationID.is_empty());
    // NewContext 必须离开 Context 的 Go time.Time 零值。
    assert_ne!(
        ctx.StartedAt,
        Context::default().StartedAt,
        "StartedAt must be set"
    );
}

/// 聚合 Go TestSetHintField* 场景：记录、忽略、延后、拷贝、不可变返回、变更告警、删除。
/// 故意合成单测以共享辅助导入，子块注释标明对应 Go 语义。
#[test]
fn test_set_hint_field_behavior() {
    {
        // initialized context records hint field each time
        // 已初始化：同 key 重复写入仍各打一条 resolved。
        let _guard = begin_log_capture();
        let mut ctx = NewContext("log-restore").expect("NewContext");

        ctx.SetHintField("lineage_id", "123");
        ctx.SetHintField("lineage_id", "123");

        assert_eq!(
            ctx.HintFields(),
            vec![HintField {
                Key: "lineage_id".into(),
                Value: "123".into()
            }]
        );
        let logs = captured_logs();
        assert_eq!(
            filter_captured_message(&logs, "BR operation hint field resolved").len(),
            2
        );
    }

    {
        // zero context ignores hint field
        // 零值上下文忽略非空 hint，且不产生 resolved 日志。
        let _guard = begin_log_capture();
        let mut ctx = Context::default();

        ctx.SetHintField("lineage_id", "123");

        assert!(ctx.HintFields().is_empty());
        let logs = captured_logs();
        assert_eq!(
            filter_captured_message(&logs, "BR operation hint field resolved").len(),
            0
        );
    }

    {
        // ignored hint field before context creation can be recorded later
        // 初始化前的写入被丢弃；NewContext 之后再写只计 1 次。
        let _guard = begin_log_capture();
        let mut ctx = Context::default();

        ctx.SetHintField("lineage_id", "123");
        assert!(ctx.HintFields().is_empty());

        ctx = NewContext("log-restore").expect("NewContext");
        ctx.SetHintField("lineage_id", "123");

        assert_eq!(
            ctx.HintFields(),
            vec![HintField {
                Key: "lineage_id".into(),
                Value: "123".into()
            }]
        );
        let logs = captured_logs();
        assert_eq!(
            filter_captured_message(&logs, "BR operation hint field resolved").len(),
            1
        );
    }

    {
        // copied initialized context records hint field independently
        // clone 后改值不影响源上下文，验证切片 clone-before-mutate。
        let _guard = begin_log_capture();
        let mut ctx = NewContext("log-restore").expect("NewContext");

        ctx.SetHintField("lineage_id", "123");
        let mut copied_ctx = ctx.clone();
        copied_ctx.SetHintField("lineage_id", "456");

        assert_eq!(
            ctx.HintFields(),
            vec![HintField {
                Key: "lineage_id".into(),
                Value: "123".into()
            }]
        );
        assert_eq!(
            copied_ctx.HintFields(),
            vec![HintField {
                Key: "lineage_id".into(),
                Value: "456".into()
            }]
        );
        let logs = captured_logs();
        // 源一次 + 副本一次（含变更）共两条 resolved。
        assert_eq!(
            filter_captured_message(&logs, "BR operation hint field resolved").len(),
            2
        );
    }

    {
        // returned hint fields cannot mutate context
        // HintFields 返回副本，外部改 Value 不得回写内部。
        let mut ctx = NewContext("log-restore").expect("NewContext");

        ctx.SetHintField("lineage_id", "123");
        let mut fields = ctx.HintFields();
        fields[0].Value = "456".into();

        assert_eq!(
            ctx.HintFields(),
            vec![HintField {
                Key: "lineage_id".into(),
                Value: "123".into()
            }]
        );
    }

    {
        // changed hint field logs warning and updates value
        // 值变化：两条 resolved + 一条 changed，并校验 old/new 字段。
        let _guard = begin_log_capture();
        let mut ctx = NewContext("log-restore").expect("NewContext");

        ctx.SetHintField("lineage_id", "123");
        ctx.SetHintField("lineage_id", "456");

        assert_eq!(
            ctx.HintFields(),
            vec![HintField {
                Key: "lineage_id".into(),
                Value: "456".into()
            }]
        );
        let logs = captured_logs();
        assert_eq!(
            filter_captured_message(&logs, "BR operation hint field resolved").len(),
            2
        );
        let warn_logs = filter_captured_message(&logs, "BR operation hint field changed");
        assert_eq!(warn_logs.len(), 1);
        assert_eq!(logged_string_field(&warn_logs[0], "hint_key"), "lineage_id");
        assert_eq!(logged_string_field(&warn_logs[0], "old_value"), "123");
        assert_eq!(logged_string_field(&warn_logs[0], "new_value"), "456");
    }

    {
        // empty value removes hint field
        // 空 value 删除字段；删除本身不打 resolved。
        let _guard = begin_log_capture();
        let mut ctx = NewContext("log-restore").expect("NewContext");

        ctx.SetHintField("lineage_id", "123");
        ctx.SetHintField("lineage_id", "");

        assert!(ctx.HintFields().is_empty());
        let logs = captured_logs();
        assert_eq!(
            filter_captured_message(&logs, "BR operation hint field resolved").len(),
            1
        );
    }
}

/// 正向 LockMeta：OwnerID/LockType 正确，Hint 含时间、lineage 与 Quote(detail)。
/// 手工填 ID/时间再 SetHintField，跳过 NewContext 的随机 UUID。
#[test]
fn test_lock_meta() {
    // 固定 StartedAt，避免 now() 导致 Hint 断言抖动。
    let started_at = time_utc(2026, 6, 15, 12, 0, 0);
    let mut ctx = Context::default();
    // 手动赋值使 isInitialized 为真，允许 hint 写入。
    ctx.OperationID = "operation-id".into();
    ctx.StartedAt = started_at;
    ctx.SetHintField("lineage_id", "123");

    // migration-read 常量与 Go LockResourceMigrationRead 字面量一致。
    let meta = ctx
        .LockMeta(LockResourceMigrationRead, "test hint")
        .expect("LockMeta");

    // OwnerID 直接复用 OperationID。
    assert_eq!(meta.OwnerID, "operation-id");
    assert_eq!(meta.LockType, LockResourceMigrationRead.0);
    assert!(
        meta.Hint
            .contains("operation_started_at=2026-06-15T12:00:00Z"),
        "hint={}",
        meta.Hint
    );
    assert!(meta.Hint.contains("lineage_id=123"), "hint={}", meta.Hint);
    // detail 使用 Debug/Quote 风格双引号包裹。
    assert!(
        meta.Hint.contains("detail=\"test hint\""),
        "hint={}",
        meta.Hint
    );
}

/// Go 的 `time.Time{}` 是公元 1 年，不是 Unix epoch；epoch 与 epoch 前时间都有效。
#[test]
fn unix_epoch_and_pre_epoch_times_are_not_go_zero_time() {
    let cases = [
        (
            std::time::SystemTime::UNIX_EPOCH,
            "operation_started_at=1970-01-01T00:00:00Z",
        ),
        (
            std::time::SystemTime::UNIX_EPOCH - std::time::Duration::from_secs(1),
            "operation_started_at=1969-12-31T23:59:59Z",
        ),
    ];

    for (started_at, expected_hint) in cases {
        let mut ctx = Context::default();
        ctx.OperationID = "operation-id".into();
        ctx.StartedAt = started_at;
        ctx.SetHintField("lineage_id", "123");

        assert_eq!(ctx.HintFields().len(), 1);
        let meta = ctx
            .LockMeta(LockResourceMigrationRead, "control\u{1}\n")
            .expect("Unix epoch is a valid Go time");
        assert!(meta.Hint.contains(expected_hint), "hint={}", meta.Hint);
        assert!(
            meta.Hint.contains("detail=\"control\\x01\\n\""),
            "Go strconv.Quote parity: hint={}",
            meta.Hint
        );
    }
}

/// 负向：缺 ID、缺启动时间、空资源类型均返回对应错误子串。
/// 与 Go errors.New 文案保持可子串匹配，便于双语对照。
#[test]
fn test_lock_meta_validation() {
    /// 单条校验用例：构造残缺 Context/资源并期望错误关键词。
    struct Case {
        name: &'static str,
        ctx: Context,
        resource: LockResourceType,
        expected_err: &'static str,
    }

    // 有 ID 但 StartedAt 仍为 Go time.Time 零值。
    let mut missing_started = Context::default();
    missing_started.OperationID = "operation-id".into();

    // 有 ID 与时间，但资源类型为空串。
    let mut missing_resource = Context::default();
    missing_resource.OperationID = "operation-id".into();
    // now() 仅需非纪元；具体时刻不参与错误断言。
    missing_resource.StartedAt = std::time::SystemTime::now();

    // 三分支覆盖 LockMeta 前置校验的全部 early-return。
    let cases = [
        Case {
            name: "missing operation ID",
            // 完全 Default：ID 与时间皆缺，先命中 ID 检查。
            ctx: Context::default(),
            resource: LockResourceMigrationRead,
            expected_err: "operation ID",
        },
        Case {
            name: "missing started time",
            ctx: missing_started,
            resource: LockResourceMigrationRead,
            expected_err: "operation started time",
        },
        Case {
            name: "missing resource type",
            ctx: missing_resource,
            // 空 LockResourceType 对应 Go 空字符串资源。
            resource: LockResourceType(""),
            expected_err: "resource type",
        },
    ];

    for c in cases {
        // expect_err 携带 case 名，失败时定位具体分支。
        let err = c.ctx.LockMeta(c.resource, "test hint").expect_err(c.name);
        assert!(
            err.contains(c.expected_err),
            "{}: err={err:?} want contains {:?}",
            c.name,
            c.expected_err
        );
    }
}

const PRODUCTION_LOG_PROBE_ENV: &str = "ASTERSQL_CONTEXT_PRODUCTION_LOG_PROBE";

/// 子进程探针：绕过测试日志 capture，覆盖真实生产日志路径。
#[test]
fn production_logging_probe() {
    if std::env::var(PRODUCTION_LOG_PROBE_ENV).as_deref() != Ok("1") {
        return;
    }

    let mut ctx = NewContext("log-restore").expect("NewContext");
    ctx.SetHintField("lineage_id", "123");
    ctx.SetHintField("lineage_id", "456");
}

/// Go 在未安装 observer 时仍写生产日志，并从操作系统读取 hostname。
#[test]
fn production_logging_emits_go_messages_with_os_hostname() {
    let hostname_output = std::process::Command::new("hostname")
        .output()
        .expect("run hostname for parity fixture");
    assert!(hostname_output.status.success(), "hostname command failed");
    let expected_hostname = String::from_utf8_lossy(&hostname_output.stdout)
        .trim()
        .to_string();
    assert!(
        !expected_hostname.is_empty(),
        "hostname command returned empty output"
    );

    let output = std::process::Command::new(std::env::current_exe().expect("current test binary"))
        .arg("production_logging_probe")
        .arg("--nocapture")
        .env(PRODUCTION_LOG_PROBE_ENV, "1")
        .env("HOSTNAME", "poisoned-env-hostname")
        .env("COMPUTERNAME", "poisoned-env-computername")
        .output()
        .expect("run production logging probe");
    assert!(
        output.status.success(),
        "probe failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(logs.contains("BR operation started"), "logs={logs}");
    assert!(
        logs.contains("BR operation hint field resolved"),
        "logs={logs}"
    );
    assert!(
        logs.contains("BR operation hint field changed"),
        "logs={logs}"
    );
    assert!(
        logs.contains(&format!("host={expected_hostname:?}")),
        "logs={logs}"
    );
    assert!(!logs.contains("poisoned-env-hostname"), "logs={logs}");
    assert!(logs.contains("command=\"log-restore\""), "logs={logs}");
}

/// 从捕获日志中取命名字段；缺失则 panic，便于定位断言失败。
fn logged_string_field(entry: &CapturedLog, key: &str) -> String {
    for (k, v) in &entry.fields {
        if k == key {
            return v.clone();
        }
    }
    panic!("missing log field {key}");
}
