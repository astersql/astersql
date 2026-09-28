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

// TSO bookkeeping / RC write-check-ts coverage for `pkg/sessiontxn`.
//
// The Go suite (`txn_rc_tso_optimize_test.go`) asserts exact TSO
// request/constant/wait counts (`sessiontxn.TsoRequestCount`,
// `TsoUseConstantCount`, `TsoWaitCount`) and lock-error counters
// (`sessiontxn.AssertLockErr`) after running dozens of concrete prepared
// statements (`SELECT ... FOR UPDATE`, joins, unions, subqueries, ...)
// through the real optimizer/executor under READ-COMMITTED with
// `tidb_rc_write_check_ts` enabled. Those exact counts are an emergent
// property of the planner's plan shape classification (point-get vs.
// range scan, etc.) inside `pkg/sessiontxn/isolation` and the
// executor/planner packages. Although those crates are available as dev
// dependencies, the Rust SQL execution path does not currently publish the
// session value hooks used by the Go failpoints; fabricating plan-shape
// counters in this test would invent new production semantics.
//
// What *is* in scope, and real production logic in `failpoint.rs`, is the
// counter bookkeeping itself (`TsoRequestCountInc`, `TsoWaitCountInc`,
// `TsoUseConstantCountInc`, `OnStmtRetryCountInc`,
// `AddAssertEntranceForLockError`, `ExecTestHook`) plus the
// `OnStmtErrorForNextAction` dispatch through `TxnManager`/
// `TxnContextProvider` that the RC write-check-ts retry loop
// (`TestConflictErrorsUseRcWriteCheckTs`) drives. This file exercises that
// real logic end to end against the `MockSession`/`MockManager`/
// `MockProvider` harness from `txn_manager_test.rs`.
//
// 覆盖 TSO（Timestamp Oracle，全局时间戳服务）计数与 RC
// （READ-COMMITTED，读已提交隔离级别）下 `tidb_rc_write_check_ts` 写检查时间戳
// 重试路径的记账逻辑。完整 Go 套件依赖优化器/执行器给出精确 TSO 次数；本 crate
// 不拥有那些依赖，故只验证 `failpoint.rs` 中的计数器与 `TxnManager` 错误分发。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::txn_manager_test::*;
use crate::*;

#[test]
/// 验证 TSO 请求计数跨多次调用累加。
fn tso_request_count_inc_accumulates_across_calls() {
    let (mut session, _calls) = MockSession::new();

    TsoRequestCountInc(&mut session);
    TsoRequestCountInc(&mut session);
    TsoRequestCountInc(&mut session);

    let count = session
        .Value(TsoRequestCount)
        .and_then(|value| value.downcast_ref::<u64>())
        .copied();
    assert_eq!(count, Some(3));
}

#[test]
/// 验证等待 TSO 的次数跨调用累加。
fn tso_wait_count_inc_accumulates_across_calls() {
    let (mut session, _calls) = MockSession::new();

    TsoWaitCountInc(&mut session);
    TsoWaitCountInc(&mut session);

    let count = session
        .Value(TsoWaitCount)
        .and_then(|value| value.downcast_ref::<u64>())
        .copied();
    assert_eq!(count, Some(2));
}

#[test]
/// 验证使用常量 TSO（复用已分配时间戳）的计数累加。
fn tso_use_constant_count_inc_accumulates_across_calls() {
    let (mut session, _calls) = MockSession::new();

    for _ in 0..5 {
        TsoUseConstantCountInc(&mut session);
    }

    let count = session
        .Value(TsoUseConstantCount)
        .and_then(|value| value.downcast_ref::<u64>())
        .copied();
    assert_eq!(count, Some(5));
}

#[test]
/// Go 的类型断言失败时计数器从零重新开始，而不是沿用错误类型的旧值。
fn tso_counters_restart_at_one_when_the_stored_value_has_the_wrong_type() {
    let (mut session, _calls) = MockSession::new();

    for (key, increment) in [
        (TsoRequestCount, TsoRequestCountInc as fn(&mut MockSession)),
        (TsoWaitCount, TsoWaitCountInc as fn(&mut MockSession)),
        (
            TsoUseConstantCount,
            TsoUseConstantCountInc as fn(&mut MockSession),
        ),
    ] {
        session.SetValue(key, Box::new("not a uint64".to_owned()));
        increment(&mut session);
        assert_eq!(
            session
                .Value(key)
                .and_then(|value| value.downcast_ref::<u64>()),
            Some(&1),
            "counter {key} must follow Go's failed-type-assertion branch"
        );
    }
}

#[test]
/// 验证语句重试计数以 i64 累加。
fn on_stmt_retry_count_inc_accumulates_an_i64_counter() {
    let (mut session, _calls) = MockSession::new();

    OnStmtRetryCountInc(&mut session);
    OnStmtRetryCountInc(&mut session);

    let count = session
        .Value(CallOnStmtRetryCount)
        .and_then(|value| value.downcast_ref::<i64>())
        .copied();
    assert_eq!(count, Some(2));
}

#[test]
/// Go 的 int 类型断言失败时语句重试计数重置为一。
fn on_stmt_retry_count_restarts_at_one_for_a_wrong_typed_value() {
    let (mut session, _calls) = MockSession::new();
    session.SetValue(CallOnStmtRetryCount, Box::new(9_u64));

    OnStmtRetryCountInc(&mut session);

    assert_eq!(
        session
            .Value(CallOnStmtRetryCount)
            .and_then(|value| value.downcast_ref::<i64>()),
        Some(&1)
    );
}

#[test]
/// 验证锁错误入口按错误名分桶计数（对齐 Go map[string]int）。
fn add_assert_entrance_for_lock_error_counts_named_entries_like_go_map_int() {
    let (mut session, _calls) = MockSession::new();

    AddAssertEntranceForLockError(&mut session, "errWriteConflict");
    AddAssertEntranceForLockError(&mut session, "errWriteConflict");
    AddAssertEntranceForLockError(&mut session, "errDuplicateKey");

    let records = session
        .Value(AssertLockErr)
        .and_then(|value| value.downcast_ref::<LockErrorRecords>())
        .expect("AddAssertEntranceForLockError must have initialized the record map");
    assert_eq!(records.get("errWriteConflict"), Some(&2));
    assert_eq!(records.get("errDuplicateKey"), Some(&1));
    assert_eq!(records.get("errNeverRecorded"), None);
}

#[test]
/// 锁错误记录类型不匹配时按 Go 逻辑重新创建 map。
fn add_assert_entrance_replaces_a_wrong_typed_record_store() {
    let (mut session, _calls) = MockSession::new();
    session.SetValue(AssertLockErr, Box::new(7_u64));

    AddAssertEntranceForLockError(&mut session, "errWriteConflict");

    let records = session
        .Value(AssertLockErr)
        .and_then(|value| value.downcast_ref::<LockErrorRecords>())
        .expect("wrong typed value must be replaced with the Go-shaped map");
    assert_eq!(records.len(), 1);
    assert_eq!(records.get("errWriteConflict"), Some(&1));
}

#[test]
/// 验证测试钩子取出并恰好执行一次已排队闭包。
fn exec_test_hook_runs_the_queued_closure_exactly_once() {
    let (mut session, _calls) = MockSession::new();
    let (sender, receiver) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
    let ran = Arc::new(AtomicBool::new(false));
    let ran_for_hook = ran.clone();
    sender
        .send(Box::new(move || ran_for_hook.store(true, Ordering::SeqCst)))
        .unwrap();
    session.SetValue("myHook", Box::new(receiver));

    ExecTestHook(&session, "myHook");

    assert!(ran.load(Ordering::SeqCst), "the queued hook must have run");
}

#[test]
/// 未注册钩子时 ExecTestHook 应为空操作且不阻塞。
fn exec_test_hook_is_a_no_op_when_no_hook_was_queued() {
    let (session, _calls) = MockSession::new();

    // Must simply return without panicking or blocking when the session
    // never registered a `TestHook` under this key.
    // 会话未注册该键的 TestHook 时直接返回。

    ExecTestHook(&session, "neverRegistered");
}

#[test]
/// 已注册键的值不是 channel 时，Go 类型断言失败并直接返回。
fn exec_test_hook_is_a_no_op_for_a_wrong_typed_value() {
    let (mut session, _calls) = MockSession::new();
    session.SetValue("wrongHook", Box::new("not a receiver".to_owned()));

    ExecTestHook(&session, "wrongHook");
}

#[test]
/// 模拟 RC write-check-ts 冲突重试：记录锁错误入口、递增重试计数并调用 OnStmtRetry。
fn rc_write_check_ts_retry_loop_records_lock_error_and_increments_retry_counter() {
    // Mirrors the observable session-level bookkeeping in
    // `TestConflictErrorsUseRcWriteCheckTs`: a write-conflict error is
    // reported to the provider, which advises a retry; the caller then
    // records the lock-error entrance and bumps the retry counter before
    // calling `OnStmtRetry`, exactly like the RC pessimistic-lock retry
    // path does around `sessiontxn.AddAssertEntranceForLockError` /
    // `sessiontxn.OnStmtRetryCountInc`.
    // 对齐 Go 可观察会话记账：写冲突后 Provider 建议重试，再记锁错误入口并递增重试计数。

    let (mut session, calls) = MockSession::new();
    // 让 Provider 在悲观锁错误点返回 RetryReady。
    session.manager.provider.retry_ready = true;
    let ctx = request_context();

    let (action, error) = GetTxnManager(&mut session).OnStmtErrorForNextAction(
        &ctx,
        StmtErrAfterPessimisticLock,
        Error::from("errWriteConflict".to_owned()),
    );
    assert_eq!(action, StmtActionRetryReady);
    assert!(error.is_none());

    AddAssertEntranceForLockError(&mut session, "errWriteConflict");
    OnStmtRetryCountInc(&mut session);
    GetTxnManager(&mut session).OnStmtRetry(&ctx).unwrap();

    let records = session
        .Value(AssertLockErr)
        .and_then(|value| value.downcast_ref::<LockErrorRecords>())
        .expect("lock error record map must have been initialized");
    assert_eq!(records.get("errWriteConflict"), Some(&1));
    let retry_count = session
        .Value(CallOnStmtRetryCount)
        .and_then(|value| value.downcast_ref::<i64>())
        .copied();
    assert_eq!(retry_count, Some(1));

    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:on-stmt-error:StmtErrAfterPessimisticLock".to_owned(),
            "provider:on-stmt-error:StmtErrAfterPessimisticLock:errWriteConflict".to_owned(),
            "manager:on-stmt-retry".to_owned(),
            "provider:on-stmt-retry".to_owned(),
        ]
    );
}
