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

// `tidb_external_ts` / `tidb_enable_external_ts_read` coverage for
// `pkg/sessiontxn/staleread`.
//
// The Go suite (`externalts_test.go`) drives a real `testkit` session
// (`SET GLOBAL tidb_external_ts=...`, `SET tidb_enable_external_ts_read`,
// `INSERT`, `InRestrictedSQL`) and asserts that enabling
// `tidb_enable_external_ts_read` turns the session read-only (an `INSERT`
// fails) unless the statement is an internal/restricted one. This crate's
// Rust port doesn't own statement execution or read-only enforcement --
// that lives in the executor, outside this package -- but it does own the
// exact mechanism the executor relies on: `get_external_timestamp`
// resolving (and caching) the external read timestamp via
// `SessionBackend::external_timestamp`, and
// `StaleReadProcessor::on_select_table` turning a statement into a stale
// read from it whenever `enable_external_ts_read` is set and the
// statement is not restricted/internal. This file exercises that exact
// mechanism end to end against the `MockBackend` harness from
// `main_test.rs`, mirroring each of the Go test's scenarios.

//
// 中文概述：覆盖 `tidb_external_ts` 与 `tidb_enable_external_ts_read`。
// 外部时间戳读会把普通语句钉在全局外部 ts 上变成过期读；
// 执行器据此拒绝写语句。本文件用 MockBackend 端到端验证解析与缓存机制。

use crate::main_test::*;
use crate::*;

#[test]
/// 未设置时外部 ts 默认为 0。
fn external_timestamp_defaults_to_zero_like_tidb_external_ts() {
    let (session, backend) = mock_session();
    // Mirrors `tk.MustQuery("select @@tidb_external_ts").Check(testkit.Rows("0"))`:
    // an unset `tidb_external_ts` resolves to zero from the backend.
    backend.external_ts.set(0);

    let ts = get_external_timestamp(&session).expect("a zero external ts must not error");

    assert_eq!(ts, 0);
    assert_eq!(backend.calls.lock().unwrap().external_timestamp_calls, 1);
}

#[test]
/// 同一会话内 external_timestamp 只向后端请求一次（缓存）。
fn get_external_timestamp_caches_the_backend_result_for_the_session() {
    let (session, backend) = mock_session();
    backend.external_ts.set(12345);

    let first = get_external_timestamp(&session).unwrap();
    let second = get_external_timestamp(&session).unwrap();

    assert_eq!(first, 12345);
    assert_eq!(second, 12345);
    // Mirrors the Go session's per-statement/session external-ts caching:
    // the backend must only be asked once, not once per read.
    assert_eq!(backend.calls.lock().unwrap().external_timestamp_calls, 1);
}

#[test]
/// 后端错误包装为 AsOf 错误种类。
fn get_external_timestamp_wraps_backend_errors_as_as_of_errors() {
    let (session, backend) = mock_session();
    *backend.external_timestamp_error.lock().unwrap() = Some("pd unavailable".to_owned());

    let error = get_external_timestamp(&session).unwrap_err();

    assert_eq!(error.kind, ErrorKind::AsOf);
    assert_eq!(error.message, "pd unavailable");
}

#[test]
/// 开启外部 ts 读后，无 AS OF 的普通语句变为过期读。
fn enabling_external_ts_read_turns_a_statement_into_a_stale_read() {
    // Mirrors `set tidb_enable_external_ts_read=ON` followed by
    // `insert into t values (0)` failing: once external-ts read is on, a
    // plain statement (no AS OF, no active transaction) resolves as a
    // stale read pinned to `tidb_external_ts`, which the executor then
    // rejects for a write. This package's contribution is exactly that
    // resolution, so we assert on it directly.
    let (session, backend) = mock_session();
    backend.external_ts.set(777);
    session.lock().unwrap().enable_external_ts_read = true;
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("resolving the external timestamp must not fail");

    assert!(
        processor.is_staleness(),
        "tidb_enable_external_ts_read=ON must make the statement a stale read"
    );
    assert_eq!(processor.staleness_read_ts(), 777);
    assert!(is_stmt_staleness(&session));
}

#[test]
/// 关闭标志后恢复普通读，即使后端仍返回外部 ts。
fn disabling_external_ts_read_restores_a_normal_statement() {
    // Mirrors `set tidb_enable_external_ts_read=OFF` followed by
    // `insert into t values (0)` succeeding: with the flag off, the
    // resolved timestamp must be back to zero (not a stale read) even
    // though the backend would still happily answer `tidb_external_ts`.
    let (session, backend) = mock_session();
    backend.external_ts.set(777);
    session.lock().unwrap().enable_external_ts_read = false;
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("a plain statement must not fail");

    assert!(!processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 0);
    assert_eq!(backend.calls.lock().unwrap().external_timestamp_calls, 0);
}

#[test]
/// 受限/内部 SQL 不受外部 ts 读影响。
fn restricted_sql_is_not_affected_by_external_ts_read() {
    // Mirrors `tk.Session().GetSessionVars().InRestrictedSQL = true`
    // followed by an internal `INSERT` succeeding even while
    // `tidb_enable_external_ts_read` is ON: internal/restricted
    // statements must never be turned into a stale read.
    let (session, backend) = mock_session();
    backend.external_ts.set(777);
    {
        let mut state = session.lock().unwrap();
        state.enable_external_ts_read = true;
        state.restricted_sql = true;
    }
    let mut processor = StaleReadProcessor::new(Context, session.clone());

    processor
        .on_select_table(&TableName { as_of: None })
        .expect("a restricted/internal statement must not fail");

    assert!(
        !processor.is_staleness(),
        "InRestrictedSQL must bypass tidb_enable_external_ts_read entirely"
    );
    assert_eq!(backend.calls.lock().unwrap().external_timestamp_calls, 0);
}

#[test]
/// 受限 SQL 结束后，后续普通语句重新按外部 ts 解析。
fn re_enabling_after_being_restricted_still_resolves_the_external_timestamp() {
    // Once the internal statement above finishes, `InRestrictedSQL` is
    // reset back to `false` in the Go test, and external-ts read applies
    // again for the next ordinary statement.
    let (session, backend) = mock_session();
    backend.external_ts.set(777);
    session.lock().unwrap().enable_external_ts_read = true;
    session.lock().unwrap().restricted_sql = true;
    let mut restricted_processor = StaleReadProcessor::new(Context, session.clone());
    restricted_processor
        .on_select_table(&TableName { as_of: None })
        .unwrap();
    assert!(!restricted_processor.is_staleness());

    // 重置 InRestrictedSQL 后，下一条普通语句应再次走外部 ts。
    session.lock().unwrap().restricted_sql = false;
    let mut processor = StaleReadProcessor::new(Context, session.clone());
    processor
        .on_select_table(&TableName { as_of: None })
        .unwrap();

    assert!(processor.is_staleness());
    assert_eq!(processor.staleness_read_ts(), 777);
    assert_eq!(backend.calls.lock().unwrap().external_timestamp_calls, 1);
}
