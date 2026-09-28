// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

// `StaleReadProcessor`/`parse_and_validate_as_of` coverage for
// `pkg/sessiontxn/staleread`.
//
// The Go suite (`processor_test.go`) drives `StmtStaleness`/`AsOfPriority`
// scenarios through a real `testkit` session: `SELECT ... AS OF
// TIMESTAMP`, `SET TRANSACTION READ ONLY AS OF ...`, `tx_read_ts`,
// `tidb_read_staleness`, and external-timestamp session variables, then
// asserts on `sessiontxn.staleread.IsStmtStaleness` /
// `GetSessionSnapshotInfoSchema`. This crate's Rust port
// (`processor.rs`/`util.rs`) keeps the same evaluation priority and error
// messages but reaches the real SQL engine only through the
// `SessionBackend` trait, so this file drives the real
// `StaleReadProcessor`/`Processor` state machine against the
// `MockBackend` harness from `main_test.rs` instead of a real session.

//
// 中文概述：覆盖 `StaleReadProcessor` 与 `parse_and_validate_as_of`。
// 验证 AS OF / tx_read_ts / tidb_read_staleness / 外部 ts 的优先级、
// 事务内限制、预编译执行路径，以及快照 InfoSchema 解析。

use crate::main_test::*;
use crate::*;

#[test]
/// 无 AS OF、无会话状态时为普通读。
fn on_select_table_without_as_of_or_session_state_is_not_stale_read() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("a plain table reference must not fail");

    assert!(!processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 0);
    assert!(processor.staleness_info_schema().is_none());
    assert!(!is_stmt_staleness(&session));
}

#[test]
/// 日期时间 AS OF 标记过期读并解析 InfoSchema。
fn on_select_table_with_datetime_as_of_marks_staleness_and_resolves_info_schema() {
    let (session, backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1699999999000")),
        })
        .expect("a valid AS OF datetime literal must be accepted");

    let expected_ts = millis_to_tso(1_699_999_999_000).unwrap();
    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), expected_ts);
    assert_eq!(
        processor.staleness_info_schema().unwrap().snapshot_ts,
        expected_ts
    );
    assert!(is_stmt_staleness(&session));
    assert_eq!(
        backend.calls.lock().unwrap().snapshot_info_schema_ts,
        vec![expected_ts]
    );
}

#[test]
/// AS OF 日期时间也必须经过快照读时间校验。
fn on_select_table_rejects_a_future_datetime_as_of() {
    let (session, backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1800000000000")),
        })
        .unwrap_err();

    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(
        error.message,
        "mock backend: snapshot read ts is in the future"
    );
    assert_eq!(backend.calls.lock().unwrap().validated_read_ts.len(), 1);
    assert!(!processor.is_staleness());
}

#[test]
/// NULL AS OF 被拒绝。
fn on_select_table_rejects_a_null_as_of_expression() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("null")),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(error.message, "as of timestamp cannot be NULL");
}

#[test]
/// 无法解析的 AS OF 表达式报错。
fn on_select_table_rejects_an_unparsable_as_of_expression() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("str:not-a-timestamp")),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(
        error.message,
        "cannot parse AS OF TIMESTAMP expression as datetime or TSO"
    );
}

#[test]
/// 2013 年之前的 TSO 非法。
fn on_select_table_rejects_a_tso_before_2013() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("tso:400")),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(
        error.message,
        "invalid TSO timestamp: TSO is before 2013-01-01"
    );
}

#[test]
/// 字符串形式的零 TSO 应归类为“2013 年之前”，而不是不可解析。
fn on_select_table_rejects_zero_string_tso_as_an_invalid_tso() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("str:0")),
        })
        .unwrap_err();

    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(
        error.message,
        "invalid TSO timestamp: TSO is before 2013-01-01"
    );
}

#[test]
/// 合法 TSO 字面量可接受。
fn on_select_table_accepts_a_valid_tso_literal() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);
    let tso = (1_700_000_000_000_i64 as u64) << TSO_LOGICAL_BITS;

    processor
        .on_select_table(&TableName {
            as_of: Some(expr(format!("tso:{tso}"))),
        })
        .expect("a valid TSO literal must be accepted");

    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), tso);
}

#[test]
/// 原始 TSO 的逻辑位精度不能丢失。
fn on_select_table_preserves_the_logical_part_of_a_tso() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);
    let tso = ((1_700_000_000_000_i64 as u64) << TSO_LOGICAL_BITS) + 1;

    processor
        .on_select_table(&TableName {
            as_of: Some(expr(format!("tso:{tso}"))),
        })
        .unwrap();

    assert_eq!(processor.staleness_read_ts(), tso);
}

#[test]
/// Go 测试中的普通字符串错误与紧凑日期格式保持对应。
fn as_of_string_literals_match_go_datetime_and_error_branches() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session.clone());
    let datetime_ts = millis_to_tso(1_699_999_999_000).unwrap();

    processor
        .on_select_table(&TableName {
            as_of: Some(expr("2023-11-14 22:13:19.000")),
        })
        .unwrap();
    assert_eq!(processor.staleness_read_ts(), datetime_ts);

    let mut compact = StaleReadProcessor::new(Context, session);
    compact
        .on_select_table(&TableName {
            as_of: Some(expr("20231114221319")),
        })
        .unwrap();
    assert_eq!(compact.staleness_read_ts(), datetime_ts);
}

#[test]
/// 活跃事务内禁止设置 AS OF。
fn on_select_table_rejects_as_of_inside_an_active_transaction() {
    let (session, _backend) = mock_session();
    session.lock().unwrap().in_txn = true;
    let mut processor = StaleReadProcessor::new(Context, session);

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1699999999000")),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(
        error.message,
        "as of timestamp can't be set in transaction."
    );
}

#[test]
/// 事务内复用已有过期读事务上下文，并挂接本地临时表标记。
fn on_select_table_reuses_the_active_stale_transaction_context() {
    let (session, _backend) = mock_session();
    {
        let mut state = session.lock().unwrap();
        state.in_txn = true;
        state.txn_context = Some(TransactionContext {
            info_schema: InfoSchema {
                snapshot_ts: 555,
                local_temporary_tables_attached: false,
            },
            start_ts: 555,
            is_staleness: true,
            txn_scope: "global".to_owned(),
        });
    }
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("reusing an active stale transaction must succeed");

    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 555);
    // Local temporary tables must always be treated as attached once a
    // transaction-scoped InfoSchema is reused by a statement.
    assert!(
        processor
            .staleness_info_schema()
            .unwrap()
            .local_temporary_tables_attached
    );
}

#[test]
/// 同一语句流中冲突的 AS OF 时间被拒绝。
fn on_select_table_rejects_a_second_conflicting_as_of_expression() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);
    processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1699999999000")),
        })
        .unwrap();

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1699999998000")),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(error.message, "can not set different time in the as of");
}

#[test]
/// UNION 场景允许重复相同 AS OF（幂等）。
fn on_select_table_allows_the_same_as_of_expression_across_a_union() {
    let (session, backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);

    processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1699999999000")),
        })
        .unwrap();
    processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1699999999000")),
        })
        .expect("repeating the exact same AS OF expression must be idempotent");

    assert_eq!(backend.calls.lock().unwrap().evaluated_expressions.len(), 2);
}

#[test]
/// 回退到 tidb_read_staleness，并缓存可复用 evaluator。
fn on_select_table_falls_back_to_read_staleness_session_variable() {
    let (session, _backend) = mock_session();
    session.lock().unwrap().read_staleness_millis = -5000;
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("tidb_read_staleness must be honoured when nothing else applies");

    assert!(processor.is_staleness());
    assert!(processor.staleness_read_ts() > 0);
    assert!(
        processor.staleness_ts_evaluator_for_prepare().is_some(),
        "a read-staleness evaluator must be cacheable for prepared execution"
    );
}

#[test]
/// tx_read_ts 优先于 tidb_read_staleness。
fn on_select_table_prefers_transaction_read_ts_over_read_staleness() {
    let (session, _backend) = mock_session();
    {
        let mut state = session.lock().unwrap();
        state.txn_read_ts = 999;
        state.read_staleness_millis = -5000;
    }
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("tx_read_ts must take priority over tidb_read_staleness");

    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 999);
    assert!(session.lock().unwrap().txn_read_ts_used);
}

#[test]
/// 已设事务级 AS OF 时不能再使用语句级 AS OF。
fn on_select_table_rejects_as_of_when_transaction_read_ts_is_already_set() {
    let (session, _backend) = mock_session();
    session.lock().unwrap().txn_read_ts = 999;
    let mut processor = StaleReadProcessor::new(Context, session);

    let error = processor
        .on_select_table(&TableName {
            as_of: Some(expr("datetime:1699999999000")),
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(
        error.message,
        "can't use select as of while already set transaction as of"
    );
}

#[test]
/// 启用外部 ts 读时回退到外部时间戳。
fn on_select_table_falls_back_to_external_timestamp_when_enabled() {
    let (session, backend) = mock_session();
    session.lock().unwrap().enable_external_ts_read = true;
    backend.external_ts.set(999);
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("external timestamp read must be honoured when enabled");

    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 999);
    assert!(processor.staleness_ts_evaluator_for_prepare().is_none());
}

#[test]
/// 受限 SQL 忽略外部时间戳。
fn on_select_table_ignores_external_timestamp_for_restricted_sql() {
    let (session, backend) = mock_session();
    {
        let mut state = session.lock().unwrap();
        state.enable_external_ts_read = true;
        state.restricted_sql = true;
    }
    backend.external_ts.set(999);
    let mut processor = StaleReadProcessor::new(Context, session);

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("restricted SQL must not become a stale read");

    assert!(!processor.is_staleness());
    assert_eq!(backend.calls.lock().unwrap().external_timestamp_calls, 0);
}

#[test]
/// 预编译执行无 evaluator、无会话状态时为普通读。
fn on_execute_prepared_stmt_without_evaluator_and_without_state_is_not_stale_read() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);

    processor
        .on_execute_prepared_stmt(None)
        .expect("no AS OF and no session state means a normal read");

    assert!(!processor.is_staleness());
}

#[test]
/// 预编译执行用传入 evaluator 计算并固化 ts。
fn on_execute_prepared_stmt_with_an_evaluator_computes_and_caches_the_ts() {
    let (session, backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);
    let evaluator: StalenessTsEvaluator = std::sync::Arc::new(|_, _| Ok(777));

    processor
        .on_execute_prepared_stmt(Some(evaluator))
        .expect("a supplied evaluator must be used to compute the ts");

    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 777);
    assert_eq!(processor.staleness_info_schema().unwrap().snapshot_ts, 777);
    assert_eq!(
        backend.calls.lock().unwrap().snapshot_info_schema_ts,
        vec![777]
    );
}

#[test]
/// 预编译语句的 ts evaluator 失败时应原样返回错误且不完成求值。
fn on_execute_prepared_stmt_propagates_evaluator_errors() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);
    let evaluator: StalenessTsEvaluator =
        std::sync::Arc::new(|_, _| Err(Error::as_of("mock evaluator failed")));

    let error = processor
        .on_execute_prepared_stmt(Some(evaluator))
        .unwrap_err();

    assert_eq!(error.message, "mock evaluator failed");
    assert!(!processor.is_staleness());
    assert!(processor.staleness_info_schema().is_none());
}

#[test]
/// 外部 ts 缓存应以语句为边界，而不是污染后续语句。
fn external_timestamp_cache_is_reset_for_each_processor_statement() {
    let (session, backend) = mock_session();
    session.lock().unwrap().enable_external_ts_read = true;
    backend.external_ts.set(100);

    let mut first = StaleReadProcessor::new(Context, session.clone());
    first.on_select_table(&TableName { as_of: None }).unwrap();
    assert_eq!(first.staleness_read_ts(), 100);

    backend.external_ts.set(200);
    let mut second = StaleReadProcessor::new(Context, session.clone());
    second.on_select_table(&TableName { as_of: None }).unwrap();

    assert_eq!(second.staleness_read_ts(), 200);
    assert_eq!(backend.calls.lock().unwrap().external_timestamp_calls, 2);
}

#[test]
/// 预编译路径重复求值报 AlreadyEvaluated。
fn on_execute_prepared_stmt_evaluated_twice_errors() {
    let (session, _backend) = mock_session();
    let mut processor = StaleReadProcessor::new(Context, session);
    processor.on_execute_prepared_stmt(None).unwrap();

    let error = processor.on_execute_prepared_stmt(None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::AlreadyEvaluated);
}

#[test]
/// 事务内预编译执行拒绝带 AS OF 的 evaluator。
fn on_execute_prepared_stmt_rejects_an_evaluator_inside_a_transaction() {
    let (session, _backend) = mock_session();
    session.lock().unwrap().in_txn = true;
    let mut processor = StaleReadProcessor::new(Context, session);
    let evaluator: StalenessTsEvaluator = std::sync::Arc::new(|_, _| Ok(1));

    let error = processor
        .on_execute_prepared_stmt(Some(evaluator))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(
        error.message,
        "as of timestamp can't be set in transaction."
    );
}

#[test]
/// 事务内无 evaluator 时复用过期读事务上下文。
fn on_execute_prepared_stmt_without_an_evaluator_reuses_transaction_staleness() {
    let (session, _backend) = mock_session();
    {
        let mut state = session.lock().unwrap();
        state.in_txn = true;
        state.txn_context = Some(TransactionContext {
            info_schema: InfoSchema {
                snapshot_ts: 42,
                local_temporary_tables_attached: false,
            },
            start_ts: 42,
            is_staleness: true,
            txn_scope: "global".to_owned(),
        });
    }
    let mut processor = StaleReadProcessor::new(Context, session);

    processor.on_execute_prepared_stmt(None).unwrap();

    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 42);
}

#[test]
/// 无表达式时 parse_and_validate_as_of 返回 0。
fn parse_and_validate_as_of_with_no_expression_returns_zero() {
    let (session, backend) = mock_session();
    let ts = parse_and_validate_as_of(&session, None).expect("no expression must be accepted");
    assert_eq!(ts, 0);
    assert!(backend.calls.lock().unwrap().validated_read_ts.is_empty());
}

#[test]
/// 解析后对后端做快照读 ts 校验。
fn parse_and_validate_as_of_validates_the_resolved_ts_against_the_backend() {
    let (session, backend) = mock_session();
    let expression = expr("datetime:1699999999000");
    let expected_ts = millis_to_tso(1_699_999_999_000).unwrap();

    let ts = parse_and_validate_as_of(&session, Some(&expression))
        .expect("a past timestamp must pass validation");

    assert_eq!(ts, expected_ts);
    assert_eq!(
        backend.calls.lock().unwrap().validated_read_ts,
        vec![expected_ts]
    );
}

#[test]
/// 未来时间戳被校验拒绝。
fn parse_and_validate_as_of_rejects_a_future_timestamp() {
    let (session, _backend) = mock_session();
    // now_millis defaults to 1_700_000_000_000 in `MockBackend::default`.
    let expression = expr("datetime:1800000000000");

    let error = parse_and_validate_as_of(&session, Some(&expression)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::AsOf);
}
