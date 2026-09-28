// Copyright 2026 AsterSQL.
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

// AsterSQL 迁移补充：mock 包核心行为回归测试。
//
// 覆盖切片迭代器、错误注入、共享 Client 响应、原子指标、Store 默认值，
// 以及会话 Context 的事务/Future/沙箱等语义。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::{Client, MetricsCounter, MockedIter, NewContext, NewSliceIter, Store, kv, sessionctx};

/// 构造测试用 KV Entry。
fn entry(key: &str, value: &str) -> kv::Entry {
    kv::Entry {
        Key: kv::Key(key.as_bytes().to_vec()),
        Value: value.as_bytes().to_vec(),
    }
}

/// 逐条比较 Entry 的 Key/Value。
fn assert_entries_equal(actual: &[kv::Entry], expected: &[kv::Entry]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.Key, expected.Key);
        assert_eq!(actual.Value, expected.Value);
    }
}

/// 计数每次 `Next` 调用的 Response 桩。
struct CountingResponse(Arc<AtomicUsize>);

impl kv::Response for CountingResponse {
    fn Next(
        &mut self,
        _ctx: &kv::context::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, kv::errors::SharedError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }

    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }
}

/// 固定返回给定 TS 的 Oracle Future 桩。
/// Oracle：TiDB 时间戳授予服务，用于事务起止时间戳。
struct FixedFuture(u64);

impl sessionctx::OracleFuture for FixedFuture {
    fn Wait(self: Box<Self>) -> Result<u64, sessionctx::GoError> {
        Ok(self.0)
    }
}

#[test]
/// 验证 SliceIter 游标前进、无效 Next 报错与 Close 后空键值。
fn slice_iter_matches_go_cursor_and_close_behavior() {
    let input = vec![entry("k1", "v1"), entry("k0", ""), entry("k2", "v2")];
    let mut iter = NewSliceIter(input.clone());

    for expected in &input {
        assert!(iter.Valid());
        assert_eq!(iter.Key(), expected.Key);
        assert_eq!(iter.Value(), expected.Value);
        iter.Next().unwrap();
    }
    assert!(!iter.Valid());
    assert_eq!(iter.Next().unwrap_err().to_string(), "iterator is invalid");
    assert_entries_equal(iter.GetSlice(), &input);

    iter.Close();
    assert!(!iter.Valid());
    assert!(iter.Key().0.is_empty());
    assert!(iter.Value().is_empty());
    assert_entries_equal(iter.GetSlice(), &input);
}

#[test]
/// 验证 MockedIter 注入错误不推进游标，并跟踪 Close 状态。
fn mocked_iter_injects_error_without_advancing_and_tracks_close() {
    let mut iter = MockedIter::new(Box::new(*NewSliceIter(vec![entry("k", "v")])), false);
    iter.InjectNextError("injected next failure");

    assert_eq!(
        iter.Next().unwrap_err().to_string(),
        "injected next failure"
    );
    assert_eq!(iter.Key(), kv::Key(b"k".to_vec()));
    assert_eq!(
        iter.GetInjectedNextError().unwrap().to_string(),
        "injected next failure"
    );

    iter.ClearInjectedNextError();
    iter.Next().unwrap();
    assert!(!iter.Valid());
    iter.Close();
    assert!(iter.Closed());
}

#[test]
#[should_panic(expected = "Multi close iter")]
/// 验证开启多次 Close 失败时第二次 Close 会 panic。
fn mocked_iter_can_fail_on_multiple_close_calls() {
    let mut iter = MockedIter::new(Box::new(*NewSliceIter(Vec::new())), true);
    iter.Close();
    iter.Close();
}

#[test]
/// 验证 Client 多次 SendMockResponse 共享同一底层响应。
fn client_returns_handles_to_the_same_configured_response() {
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Client::new(Box::new(CountingResponse(Arc::clone(&calls))));
    let mut first = client.SendMockResponse().unwrap();
    let mut second = client.SendMockResponse().unwrap();
    let context = kv::context::Context::default();

    first.Next(&context).unwrap();
    second.Next(&context).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        client
            .RequestTypeSupportedChecker
            .IsRequestTypeSupported(kv::ReqTypeAnalyze, kv::ReqSubTypeBasic)
    );
}

#[test]
/// Go 的零值 mock.Client 没有响应对象，Send 应返回 nil 而不是 panic。
fn default_client_send_returns_none() {
    fn nullable_send(
        client: &Client,
        context: &kv::context::Context,
        request: &kv::Request,
        vars: &dyn std::any::Any,
        option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        kv::Client::Send(client, context, request, vars, option)
    }

    let _ = nullable_send;
    assert!(Client::default().SendMockResponse().is_none());
}

#[test]
/// 验证 MetricsCounter 多线程原子累加结果。
fn metrics_counter_is_atomic_across_threads() {
    let counter = Arc::new(MetricsCounter::default());
    let mut workers = Vec::new();
    for _ in 0..8 {
        let counter = Arc::clone(&counter);
        workers.push(thread::spawn(move || {
            for _ in 0..1_000 {
                counter.Inc();
                counter.Add(0.5);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(counter.Val(), 12_000.0);
}

#[test]
/// 验证 Store 固定默认值与 Go 对齐。
fn store_returns_the_same_fixed_defaults_as_go() {
    let store = Store::default();
    assert!(store.GetClient().is_none());
    assert!(store.GetMPPClient().is_none());
    assert!(store.GetOracle().is_none());
    assert!(store.Begin(&[]).unwrap().is_none());
    assert!(store.GetSnapshot(kv::Version { Ver: 42 }).is_none());
    store.Close().unwrap();
    assert_eq!(store.UUID(), "mock");
    assert_eq!(store.CurrentVersion("global").unwrap().Ver, 0);
    assert!(!store.SupportDeleteRange());
    assert_eq!(store.Name(), "UtilMockStorage");
    assert_eq!(
        store.Describe(),
        "UtilMockStorage is a mock Store implementation, only for unittests in util package"
    );
    assert!(store.GetMemCache().is_none());
    assert!(
        store
            .ShowStatus(&kv::context::Context::default(), "status")
            .unwrap()
            .is_none()
    );
    assert_eq!(store.GetMinSafeTS("global"), 0);
    assert!(store.GetLockWaits().unwrap().is_none());
    assert!(store.GetCodec().is_none());
    let (value, found) = store.GetOption(&"missing");
    assert!(value.is_none());
    assert!(!found);
    store.SetOption(Box::new("ignored"), Box::new(42_i32));
    let (value, found) = store.GetOption(&"ignored");
    assert!(value.is_none());
    assert!(!found);
    assert_eq!(store.GetClusterID(), 1);
    assert_eq!(store.GetKeyspace(), "");
}

#[test]
/// 验证 Context 会话变量默认值、本地 Value 与 DDL Owner/沙箱标志。
/// DDL Owner：集群中负责执行 DDL 的角色；沙箱模式限制危险操作。
fn context_preserves_local_values_flags_and_go_defaults() {
    let mut ctx = NewContext();
    assert_eq!(ctx.GetSessionVars().InitChunkSize, 2);
    assert_eq!(ctx.GetSessionVars().MaxChunkSize, 32);
    assert!(ctx.GetSessionVars().EnableChunkRPC);
    assert_eq!(
        ctx.GetSessionVars()
            .GetSystemVar("max_allowed_packet")
            .as_deref(),
        Some("67108864")
    );
    assert_eq!(
        ctx.GetSessionVars()
            .GetSystemVar("character_set_connection")
            .as_deref(),
        Some("utf8mb4")
    );

    ctx.SetValue("mock_key", 1_i32);
    assert_eq!(ctx.Value::<i32>("mock_key"), Some(&1));
    ctx.ClearValue("mock_key");
    assert!(ctx.Value::<i32>("mock_key").is_none());

    assert!(!ctx.IsDDLOwner());
    ctx.SetIsDDLOwner(true);
    assert!(ctx.IsDDLOwner());
    assert!(!ctx.InSandBoxMode());
    ctx.EnableSandBoxMode();
    assert!(ctx.InSandBoxMode());
    ctx.DisableSandBoxMode();
    assert!(!ctx.InSandBoxMode());
}

#[test]
/// 验证假事务创建及 Commit/Rollback 后清除 InTxn。
fn context_creates_fake_transaction_and_clears_in_txn_on_finish() {
    let mut ctx = NewContext();
    let txn = ctx.Txn(true).unwrap();
    assert!(txn.Valid());
    assert_eq!(txn.StartTS(), 1);

    ctx.GetSessionVarsMut().SetInTxn(true);
    ctx.CommitTxn(&ctx.GoCtx()).unwrap();
    assert!(!ctx.GetSessionVars().InTxn());

    ctx.GetSessionVarsMut().SetInTxn(true);
    ctx.RollbackTxn(&ctx.GoCtx());
    assert!(!ctx.GetSessionVars().InTxn());
}

#[test]
/// 验证挂起的 TS Future 与 Cancel 取消执行上下文语义。
fn context_preserves_pending_future_and_cancel_semantics() {
    let mut ctx = NewContext();
    let execution_context = ctx.GoCtx();
    ctx.PrepareTSFuture(&execution_context, Box::new(FixedFuture(42)), "global")
        .unwrap();
    assert!(ctx.GetPreparedTxnFuture().is_some());
    assert!(!ctx.Txn(true).unwrap().Valid());
    let error = match ctx
        .GetPreparedTxnFuture()
        .unwrap()
        .Wait(&execution_context, None)
    {
        Ok(_) => panic!("pending transaction without a store must fail"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "mock.Context has no store for a pending transaction"
    );

    assert!(!execution_context.is_cancelled());
    ctx.Cancel();
    assert!(execution_context.is_cancelled());
}

#[test]
/// Future 选出的事务时间戳必须通过 WithStartTS 传给 Storage::Begin。
fn pending_future_starts_storage_transaction_at_selected_ts() {
    let store =
        mockstorage_crate::NewMockStorage(mockstorage_crate::KVStore::NewMemory(), None).unwrap();
    let mut ctx = NewContext();
    let execution_context = ctx.GoCtx();
    ctx.PrepareTSFuture(&execution_context, Box::new(FixedFuture(42)), "global")
        .unwrap();
    let transaction = ctx
        .GetPreparedTxnFuture()
        .unwrap()
        .Wait(&execution_context, Some(store.as_ref()))
        .unwrap();
    assert_eq!(transaction.StartTS(), 42);
}

#[test]
/// 验证未实现接口返回 Not Supported，以及锁相关默认行为。
fn context_unsupported_and_default_interfaces_match_go() {
    let ctx = NewContext();
    assert_eq!(
        ctx.Execute("select 1").unwrap_err().to_string(),
        "Not Supported"
    );
    assert_eq!(
        ctx.ParseWithParams("select ?", &[])
            .unwrap_err()
            .to_string(),
        "Not Supported"
    );
    assert!(!ctx.IsCrossKS());
    assert!(!ctx.HasDirtyContent(1));
    assert!(ctx.GetBuiltinFunctionUsage().is_empty());
    assert!(ctx.GetAdvisoryLock("lock", 1).is_ok());
    assert_eq!(ctx.IsUsedAdvisoryLock("lock"), 0);
    assert!(ctx.ReleaseAdvisoryLock("lock"));
    assert_eq!(ctx.ReleaseAllAdvisoryLocks(), 0);
    assert_eq!(ctx.CheckTableLocked(1), (false, crate::TableLockType::None));
    assert!(ctx.GetAllTableLocks().is_empty());
}

#[test]
#[should_panic(expected = "mock.Context domain is not bound")]
fn get_sql_server_panics_when_domain_is_not_bound_like_go_type_assertion() {
    let ctx = NewContext();
    let _ = ctx.GetSQLServer::<u64>();
}
