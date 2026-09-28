// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `TxnManagerInCompile`/`TxnManagerInRebuildPlan`/... style assertions for
// `pkg/sessiontxn`.
//
// The Go suite (`txn_context_test.go`) is a ~1100-line integration test:
// it builds a real mock TiKV store/domain, drives statements through the
// full executor/planner/session stack with several `failpoint.Enable`
// hooks (`assertTxnManagerInCompile`, `assertTxnManagerInRebuildPlan`,
// ...), and uses `sessiontxn.AssertTxnManagerInfoSchema` /
// `sessiontxn.AssertTxnManagerReadTS` (the Go counterparts of
// `failpoint.rs`'s `AssertTxnManagerInfoSchema`/`AssertTxnManagerReadTS`)
// to check the transaction manager's InfoSchema/read-ts at each of those
// injection points. None of the executor/planner/session infrastructure
// those failpoints are wired into exists in this crate (the same gap
// already recorded for `pkg/sessiontxn/isolation`), so this file instead
// exercises the real, in-scope `failpoint.rs` assertion helpers end to end
// against the `MockSession`/`MockManager`/`MockProvider` harness defined in
// `txn_manager_test.rs`, the same way the Go failpoints exercise them
// against a real `sessionctx.Context`.

//
// 中文概述：用 MockSession/MockManager 行使 `failpoint.rs` 断言辅助，
// 对应 Go `txn_context_test.go` 中编译/重建计划等注入点对
// InfoSchema / 读 ts 的检查，以及语句生命周期回调顺序。

use crate::txn_manager_test::*;
use crate::*;

#[test]
/// RecordAssert 按名存储并可覆盖条目。
fn record_assert_stores_and_overwrites_named_entries() {
    let (mut session, _calls) = MockSession::new();

    RecordAssert(&mut session, "answer", Box::new(41_i32));
    RecordAssert(&mut session, "answer", Box::new(42_i32));
    RecordAssert(&mut session, "other", Box::new("hello".to_owned()));

    let records = session
        .Value(AssertRecordsKey)
        .and_then(|value| value.downcast_ref::<AssertRecords>())
        .expect("RecordAssert must have initialized the assert-records map");
    assert_eq!(
        records
            .get("answer")
            .and_then(|value| value.downcast_ref::<i32>()),
        Some(&42)
    );
    assert_eq!(
        records
            .get("other")
            .and_then(|value| value.downcast_ref::<String>()),
        Some(&"hello".to_owned())
    );
}

#[test]
/// InfoSchema 版本匹配时断言通过。
fn assert_txn_manager_info_schema_accepts_a_matching_version() {
    let (mut session, _calls) = MockSession::new();
    session.manager.provider.info_schema = mock_info_schema(7);

    AssertTxnManagerInfoSchema(&mut session, Some(mock_info_schema(7)));
}

#[test]
#[should_panic(expected = "transaction InfoSchema version mismatch")]
/// InfoSchema 版本不匹配时 panic。
fn assert_txn_manager_info_schema_panics_on_mismatched_version() {
    let (mut session, _calls) = MockSession::new();
    session.manager.provider.info_schema = mock_info_schema(7);

    AssertTxnManagerInfoSchema(&mut session, Some(mock_info_schema(8)));
}

#[test]
/// 无显式期望时回退到已存储的 AssertTxnInfoSchemaKey。
fn assert_txn_manager_info_schema_falls_back_to_the_stored_expectation() {
    let (mut session, _calls) = MockSession::new();
    session.manager.provider.info_schema = mock_info_schema(5);
    session.SetValue(AssertTxnInfoSchemaKey, Box::new(mock_info_schema(5)));

    // No explicit expectation is passed in; the stored
    // `AssertTxnInfoSchemaKey` value is used instead, mirroring the Go
    // `assertTxnManagerInfoSchema(ctx, nil)` call sites that rely on a
    // previously recorded expectation.
    AssertTxnManagerInfoSchema(&mut session, None);
}

#[test]
#[should_panic(expected = "transaction InfoSchema version mismatch")]
/// 同时校验显式期望与已存储期望（任一不匹配即失败）。
fn assert_txn_manager_info_schema_checks_both_explicit_and_stored_expectations() {
    let (mut session, _calls) = MockSession::new();
    session.manager.provider.info_schema = mock_info_schema(5);
    session.SetValue(AssertTxnInfoSchemaKey, Box::new(mock_info_schema(9)));

    // Even though the explicit expectation matches, the stale stored
    // expectation must still be honoured (Go checks both).
    AssertTxnManagerInfoSchema(&mut session, Some(mock_info_schema(5)));
}

#[test]
/// 本地临时表身份一致时通过。
fn assert_txn_manager_info_schema_accepts_matching_local_temporary_tables_identity() {
    let (mut session, _calls) = MockSession::new();
    session.local_temp_tables = Some(9);
    session.manager.provider.txn_local_temp_tables = Some(9);

    AssertTxnManagerInfoSchema(&mut session, None);
}

#[test]
#[should_panic(expected = "local temporary tables must be shared with transaction InfoSchema")]
/// 本地临时表身份不一致时 panic。
fn assert_txn_manager_info_schema_panics_when_local_temporary_tables_identity_diverges() {
    let (mut session, _calls) = MockSession::new();
    session.local_temp_tables = Some(9);
    session.manager.provider.txn_local_temp_tables = Some(3);

    AssertTxnManagerInfoSchema(&mut session, None);
}

#[test]
/// 读 ts 匹配时通过。
fn assert_txn_manager_read_ts_accepts_a_matching_timestamp() {
    let (mut session, _calls) = MockSession::new();
    session.manager.provider.stmt_read_ts = 424242;

    AssertTxnManagerReadTS(&mut session, 424242);
}

#[test]
#[should_panic(expected = "transaction read timestamp mismatch")]
/// 读 ts 不匹配时 panic。
fn assert_txn_manager_read_ts_panics_on_mismatched_timestamp() {
    let (mut session, _calls) = MockSession::new();
    session.manager.provider.stmt_read_ts = 424242;

    AssertTxnManagerReadTS(&mut session, 1);
}

#[test]
/// 语句生命周期回调按 NewTxn → OnStmtStart → OnStmtCommit → OnStmtEnd 顺序到达 Provider。
fn txn_context_lifecycle_calls_reach_the_provider_in_order() {
    // Mirrors the shape of `setupTxnContextTest`'s statement lifecycle
    // (`OnInitialize` -> `OnStmtStart` -> `OnStmtCommit`) without any real
    // executor/planner: the manager forwards each callback to its
    // provider, and `AssertTxnManagerInfoSchema`/`AssertTxnManagerReadTS`
    // can be interleaved at any point, just like the Go failpoints do
    // inside `Compile`/`RebuildPlan`/`BuildExecutor`.
    let (mut session, calls) = MockSession::new();
    session.manager.provider.info_schema = mock_info_schema(3);
    session.manager.provider.stmt_read_ts = 100;
    let ctx = request_context();

    NewTxn(&ctx, &mut session).unwrap();
    AssertTxnManagerInfoSchema(&mut session, Some(mock_info_schema(3)));

    let statement = dummy_statement("select * from t1");
    GetTxnManager(&mut session)
        .OnStmtStart(&ctx, Some(statement))
        .unwrap();
    AssertTxnManagerReadTS(&mut session, 100);
    GetTxnManager(&mut session).OnStmtCommit(&ctx).unwrap();
    GetTxnManager(&mut session).OnStmtEnd();

    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:enter-new-txn:EnterNewTxnDefault".to_owned(),
            "provider:on-initialize:EnterNewTxnDefault".to_owned(),
            "manager:on-stmt-start".to_owned(),
            "provider:on-stmt-start".to_owned(),
            "provider:get-stmt-read-ts".to_owned(),
            "manager:on-stmt-commit".to_owned(),
            "provider:on-stmt-commit".to_owned(),
            "manager:on-stmt-end".to_owned(),
        ]
    );
    assert!(session.manager.GetCurrentStmt().is_none());
}
