// Copyright 2026 AsterSQL.

// 慢查询（slow query）强制写日志路径的单元测试。
//
// 验证 `WriteForcedSlowLogTo`：仅当语句 hint 显式要求时，才把已填好的
// `SlowQueryLogItems` 格式化写入日志；未强制时不产生任何日志条目。

use astersql_sessionctx_variable::session::SessionVars;
use astersql_sessionctx_variable::slow_log::SlowQueryLogItems;
use astersql_util_logutil::log::{LogLevel, Logger};

use crate::adapter_slow_log::WriteForcedSlowLogTo;

/// 非强制不写日志；强制后写入含 Txn_start_ts 与原始 SQL 的格式化条目。
#[test]
fn write_slow_log_hint_emits_real_formatted_item_only_when_forced() {
    let logger = Logger::memory(LogLevel::Warn);
    let session_vars = SessionVars::new();
    let items = SlowQueryLogItems {
        TxnTS: 42,
        SQL: "select /*+ WRITE_SLOW_LOG */ 42".to_owned(),
        Succ: true,
        ..SlowQueryLogItems::default()
    };

    // 未强制：不应写出任何慢查询日志
    assert!(!WriteForcedSlowLogTo(&logger, false, &session_vars, &items,));
    assert!(logger.entries().is_empty());

    // 强制：写出一条包含事务起始时间戳与 SQL 文本的日志
    assert!(WriteForcedSlowLogTo(&logger, true, &session_vars, &items,));
    let entries = logger.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].level, LogLevel::Warn);
    assert!(entries[0].message.contains("Txn_start_ts: 42"));
    assert!(
        entries[0]
            .message
            .contains("select /*+ WRITE_SLOW_LOG */ 42")
    );
}

#[test]
fn plan_digest_registration_precedes_rule_parsing() {
    use astersql_sessionctx_variable::slow_log::{ParseGlobalSlowLogRules, Threshold};
    let rules = ParseGlobalSlowLogRules("Conn_ID:7,Plan_digest:AbCd").unwrap();
    let rule = &rules.rules_map[&7].rules[0];
    assert!(rule.conditions.iter().any(|condition| {
        condition.field == "plan_digest" && condition.threshold == Threshold::String("AbCd".into())
    }));
}
