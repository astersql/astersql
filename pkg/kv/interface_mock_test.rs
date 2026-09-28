// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// MockTxn / MockStorage / MockMap 测试夹具与 Go 辅助实现的行为对齐验证。

use crate::test_fixtures::{MockMap, MockStorage, MockTxn};
use kv::{FairLockingController, Getter, Mutator, Snapshot, Storage, Transaction};
use kv_dependency as kv;

/// 核对 mock 事务选项、版本、快照 NotFound 以及内存 Map 读写语义。
#[test]
fn test_mock_fixture_matches_go_helpers() {
    // MockTxn：Commit 始终返回可重试错误；其余可观察状态与 Go 空实现一致。
    let mut txn = MockTxn::default();
    assert!(txn.Valid());
    let commit_error = txn.Commit(&kv::Context::todo()).unwrap_err();
    assert!(kv::IsTxnRetryableError(Some(&commit_error)));
    assert_eq!("", txn.String());
    assert!(txn.IsReadOnly());
    assert_eq!(0, txn.StartTS());
    assert_eq!(0, txn.CommitTS());
    assert_eq!(0, txn.Len());
    assert_eq!(0, txn.Size());
    assert_eq!(0, txn.Mem());
    assert!(!txn.MemHookSet());
    assert!(!txn.IsPessimistic());
    assert!(!txn.IsInFairLockingMode());
    assert!(!txn.IsPipelined());

    txn.SetOption(23, Some(Box::new(42_i32)));
    assert_eq!(
        Some(&42),
        txn.GetOption(23).and_then(|v| v.downcast_ref::<i32>())
    );
    txn.Rollback().unwrap();
    assert!(!txn.Valid());

    // MockStorage：构造的事务有效，固定元数据与 Go mockStorage 一致。
    let storage = MockStorage::default();
    assert!(storage.Begin(&[]).unwrap().Valid());
    assert_eq!(
        1,
        storage
            .CurrentVersion(kv::oracle::GlobalTxnScope)
            .unwrap()
            .Ver
    );
    assert_eq!("KVMockStorage", storage.Name());
    assert_eq!(
        "KVMockStorage is a mock Store implementation, only for unittests in KV package",
        storage.Describe()
    );
    assert_eq!(1, storage.GetClusterID());
    assert_eq!("", storage.GetKeyspace());
    assert!(!storage.SupportDeleteRange());
    assert_eq!(0, storage.GetMinSafeTS(kv::oracle::GlobalTxnScope));
    assert!(storage.GetLockWaits().unwrap().is_empty());

    // 空快照的 Get 报 NotFound，BatchGet 跳过所有未命中键。
    let snapshot = storage.GetSnapshot(kv::NewVersion(1));
    let missing = snapshot
        .Get(&kv::Context::todo(), kv::Key(b"missing".to_vec()), &[])
        .unwrap_err();
    assert!(kv::IsErrNotFound(&missing));
    assert!(
        snapshot
            .BatchGet(
                &kv::Context::todo(),
                &[kv::Key(b"a".to_vec()), kv::Key(b"b".to_vec())],
                &[],
            )
            .unwrap()
            .is_empty()
    );

    // MockMap：Set 插入/覆盖、Get 和 Delete 完整对齐 Go 线性 map。
    let mut map = MockMap::default();
    map.Set(kv::Key(b"key".to_vec()), b"value".to_vec())
        .unwrap();
    assert_eq!(
        b"value",
        map.Get(&kv::Context::todo(), kv::Key(b"key".to_vec()), &[])
            .unwrap()
            .Value
            .as_slice()
    );
    map.Set(kv::Key(b"key".to_vec()), b"updated".to_vec())
        .unwrap();
    assert_eq!(
        b"updated",
        map.Get(&kv::Context::todo(), kv::Key(b"key".to_vec()), &[])
            .unwrap()
            .Value
            .as_slice()
    );
    map.Delete(kv::Key(b"key".to_vec())).unwrap();
    let deleted = map
        .Get(&kv::Context::todo(), kv::Key(b"key".to_vec()), &[])
        .unwrap_err();
    assert!(kv::IsErrNotFound(&deleted));
    map.Delete(kv::Key(b"absent".to_vec())).unwrap();
}
