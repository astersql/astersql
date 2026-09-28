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

// sessiontxn 迁移期单元测试。
//
// 覆盖常量 Future 与会话 Oracle 契约、语句错误动作辅助与进入新事务请求默认值、
// 断言记录/锁错误计数/TSO 计数器，以及测试钩子一次性消费行为。

use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use astersql_sessionctx as sessionctx;

use crate::*;

/// 简易会话键值存储，实现 `SessionValueStore` 供断言与钩子测试使用。
#[derive(Default)]
struct ValueStore {
    /// 以字符串键保存会话侧任意值。
    values: HashMap<String, SessionValue>,
}

impl SessionValueStore for ValueStore {
    fn Value(&self, key: &str) -> Option<&dyn Any> {
        self.values.get(key).map(Box::as_ref)
    }

    fn ValueMut(&mut self, key: &str) -> Option<&mut dyn Any> {
        match self.values.get_mut(key) {
            Some(value) => Some(value.as_mut()),
            None => None,
        }
    }

    fn SetValue(&mut self, key: &'static str, value: SessionValue) {
        self.values.insert(key.to_owned(), value);
    }
}

/// 验证 `ConstantFuture` 满足会话 Oracle Future 契约（Wait 返回固定时间戳）。
#[test]
fn constant_future_implements_session_oracle_contract() {
    assert_eq!(ConstantFuture(u64::MAX).Wait().unwrap(), u64::MAX);
    assert_eq!(
        sessionctx::OracleFuture::Wait(Box::new(ConstantFuture(42))).unwrap(),
        42
    );
}

/// 验证错误动作辅助与 `EnterNewTxnRequest` 默认值与 Go 侧一致。
#[test]
fn action_helpers_and_new_txn_request_keep_go_defaults() {
    let error: Error = Box::new(std::io::Error::other("boom"));
    let (action, returned) = ErrorAction(error);
    assert_eq!(action, StmtActionError);
    assert_eq!(returned.unwrap().to_string(), "boom");
    assert_eq!(RetryReady().0, StmtActionRetryReady);
    assert!(RetryReady().1.is_none());
    assert_eq!(NoIdea().0, StmtActionNoIdea);
    assert!(NoIdea().1.is_none());

    let request = EnterNewTxnRequest::default();
    assert_eq!(request.Type, EnterNewTxnDefault);
    assert!(request.Provider.is_none());
    assert!(request.TxnMode.is_empty());
    assert!(!request.CausalConsistencyOnly);
    assert_eq!(request.StaleReadTS, 0);
}

/// 验证断言记录、锁错误入口计数与 TSO/重试计数器的类型与累加语义。
#[test]
fn assertion_records_lock_entries_and_counters_preserve_types() {
    let mut store = ValueStore::default();
    RecordAssert(&mut store, "phase", Box::new("started".to_owned()));
    RecordAssert(&mut store, "attempt", Box::new(2_u64));
    RecordAssert(&mut store, "phase", Box::new("finished".to_owned()));
    let records = store
        .Value(AssertRecordsKey)
        .unwrap()
        .downcast_ref::<AssertRecords>()
        .unwrap();
    assert_eq!(
        records["phase"].downcast_ref::<String>().unwrap(),
        "finished"
    );
    assert_eq!(*records["attempt"].downcast_ref::<u64>().unwrap(), 2);

    // 同一锁错误键多次登记应累加
    AddAssertEntranceForLockError(&mut store, "query");
    AddAssertEntranceForLockError(&mut store, "query");
    let locks = store
        .Value(AssertLockErr)
        .unwrap()
        .downcast_ref::<LockErrorRecords>()
        .unwrap();
    assert_eq!(locks["query"], 2);

    TsoRequestCountInc(&mut store);
    TsoRequestCountInc(&mut store);
    TsoWaitCountInc(&mut store);
    TsoUseConstantCountInc(&mut store);
    OnStmtRetryCountInc(&mut store);
    assert_eq!(
        *store
            .Value(TsoRequestCount)
            .unwrap()
            .downcast_ref::<u64>()
            .unwrap(),
        2
    );
    assert_eq!(
        *store
            .Value(TsoWaitCount)
            .unwrap()
            .downcast_ref::<u64>()
            .unwrap(),
        1
    );
    assert_eq!(
        *store
            .Value(TsoUseConstantCount)
            .unwrap()
            .downcast_ref::<u64>()
            .unwrap(),
        1
    );
    assert_eq!(
        *store
            .Value(CallOnStmtRetryCount)
            .unwrap()
            .downcast_ref::<i64>()
            .unwrap(),
        1
    );

    // Go 的类型断言失败时按零值重新计数；Rust 也必须丢弃错误类型。
    store.SetValue(TsoRequestCount, Box::new(-1_i64));
    store.SetValue(CallOnStmtRetryCount, Box::new(9_u64));
    TsoRequestCountInc(&mut store);
    OnStmtRetryCountInc(&mut store);
    assert_eq!(
        *store
            .Value(TsoRequestCount)
            .unwrap()
            .downcast_ref::<u64>()
            .unwrap(),
        1
    );
    assert_eq!(
        *store
            .Value(CallOnStmtRetryCount)
            .unwrap()
            .downcast_ref::<i64>()
            .unwrap(),
        1
    );
}

/// 验证测试钩子只消费一次回调，且缺失键被忽略；断言键常量互不相同。
#[test]
fn test_hook_consumes_one_callback_and_ignores_missing_keys() {
    let mut store = ValueStore::default();
    let (sender, receiver) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
    let calls = Arc::new(Mutex::new(0_u64));
    let calls_for_hook = Arc::clone(&calls);
    sender
        .send(Box::new(move || *calls_for_hook.lock().unwrap() += 1))
        .unwrap();
    store.SetValue(BreakPointBeforeExecutorFirstRun, Box::new(receiver));

    // 已注册钩子执行一次；缺失键静默忽略
    ExecTestHook(&store, BreakPointBeforeExecutorFirstRun);
    ExecTestHook(&store, "missing-hook");
    assert_eq!(*calls.lock().unwrap(), 1);
    assert_ne!(AssertTxnInfoSchemaKey, AssertTxnInfoSchemaAfterRetryKey);
}
