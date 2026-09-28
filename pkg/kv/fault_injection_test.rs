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

// fault_injection 故障注入包装器的基本行为测试。

use crate::test_fixtures::MockStorage;
use kv_dependency as kv;
use std::sync::Arc;

/// 验证 Get/BatchGet/Commit 注入错误可开关，清除后回落到底层 MockStorage 语义。
#[test]
fn test_fault_injection_basic() {
    let cfg = Arc::new(kv::InjectionConfig::default());
    let injected = kv::errors::New("foo");
    // 同时注入读路径与提交路径错误。
    cfg.SetGetError(Some(injected.clone()));
    cfg.SetCommitError(Some(injected.clone()));

    let mut storage = kv::NewInjectedStore(Box::new(MockStorage::default()), Arc::clone(&cfg));
    let mut txn = storage.Begin(&[]).unwrap();
    storage
        .Begin(&[
            kv::tikv::TxnOption::default(),
            kv::tikv::TxnOption::default(),
        ])
        .unwrap();
    let snapshot = storage.GetSnapshot(kv::Version { Ver: 1 });
    let ctx = kv::Context::todo();

    // 注入开启时，事务与快照的读/提交均应返回同一注入错误。
    let err = txn.Get(&ctx, kv::Key(b"a".to_vec()), &[]).unwrap_err();
    assert_eq!(injected.to_string(), err.to_string());
    let err = snapshot.Get(&ctx, kv::Key(b"a".to_vec()), &[]).unwrap_err();
    assert_eq!(injected.to_string(), err.to_string());
    let err = txn.BatchGet(&ctx, &[], &[]).unwrap_err();
    assert_eq!(injected.to_string(), err.to_string());
    let err = snapshot.BatchGet(&ctx, &[], &[]).unwrap_err();
    assert_eq!(injected.to_string(), err.to_string());
    let err = txn.Commit(&ctx).unwrap_err();
    assert_eq!(injected.to_string(), err.to_string());

    // 清除注入后，行为回到 MockStorage：空键 NotFound，Commit 可重试。
    cfg.SetGetError(None);
    cfg.SetCommitError(None);
    storage = kv::NewInjectedStore(Box::new(MockStorage::default()), cfg);
    let mut txn = storage.Begin(&[]).unwrap();
    let snapshot = storage.GetSnapshot(kv::Version { Ver: 1 });
    assert_eq!(
        kv::ValueEntry::default(),
        txn.Get(&ctx, kv::Key(b"a".to_vec()), &[]).unwrap()
    );
    assert!(txn.BatchGet(&ctx, &[], &[]).unwrap().is_empty());
    let err = snapshot.Get(&ctx, kv::Key(b"a".to_vec()), &[]).unwrap_err();
    assert!(kv::IsErrNotFound(&err));
    assert!(
        snapshot
            .BatchGet(&ctx, &[kv::Key(b"a".to_vec())], &[])
            .unwrap()
            .is_empty()
    );
    let err = txn.Commit(&ctx).unwrap_err();
    assert!(kv::ErrTxnRetryable.Equal(Some(&err)));
}
