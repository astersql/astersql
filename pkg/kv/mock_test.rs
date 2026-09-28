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

// Mock Storage / Transaction 接口冒烟测试。
//
// 使用 `test_fixtures` 中的 `MockStorage` / `MockTxn` 走一遍 Storage、Snapshot、
// Transaction 等核心 trait 的常用方法，确认 mock 实现与 Go 侧 mock 语义一致
//（例如 Commit 返回可重试错误、读写为空操作等）。

use crate::test_fixtures::{MockStorage, MockTxn};
use kv::{Mutator, Storage, Transaction};
use kv_dependency as kv;

/// 覆盖 MockStorage / MockTxn 上主要接口方法的调用与基本断言。
#[test]
fn test_interface() {
    // —— Storage：客户端、UUID、版本与快照 ——
    let mut storage = MockStorage::default();
    storage.GetClient();
    assert_eq!("", storage.UUID());
    let version = storage.CurrentVersion(kv::oracle::GlobalTxnScope).unwrap();

    let mut snapshot = storage.GetSnapshot(version);
    assert!(
        snapshot
            .BatchGet(
                &kv::Context::todo(),
                &[kv::Key(b"abc".to_vec()), kv::Key(b"def".to_vec())],
                &[]
            )
            .unwrap()
            .is_empty()
    );
    snapshot.SetOption(kv::PriorityNormal, None);

    // —— Transaction：加锁、选项、读写与迭代；Commit 应失败（可重试错误） ——
    let mut transaction = storage.Begin(&[]).unwrap();
    transaction
        .LockKeys(
            &kv::Context::todo(),
            &mut kv::LockCtx::default(),
            &[kv::Key(b"lock".to_vec())],
        )
        .unwrap();
    transaction.SetOption(23, Some(Box::new(())));
    assert!(transaction.GetOption(23).is_some());
    assert_eq!(0, transaction.StartTS());
    assert!(transaction.IsReadOnly());
    transaction
        .Get(&kv::Context::todo(), kv::Key(b"lock".to_vec()), &[])
        .unwrap();
    transaction
        .Set(kv::Key(b"lock".to_vec()), Vec::new())
        .unwrap();
    transaction.Iter(kv::Key(b"lock".to_vec()), None).unwrap();
    transaction
        .IterReverse(Some(kv::Key(b"lock".to_vec())), None)
        .unwrap();
    assert!(transaction.Commit(&kv::Context::todo()).is_err());

    // —— MockTxn 自身：Valid/Reset/Rollback/Delete ——
    let mut concrete = MockTxn::default();
    assert_eq!("", concrete.String());
    assert!(concrete.Valid());
    assert_eq!(0, concrete.Len());
    assert_eq!(0, concrete.Size());
    concrete.Reset();
    concrete.Rollback().unwrap();
    assert!(!concrete.Valid());
    assert!(!concrete.IsPessimistic());
    concrete.Delete(kv::Key::default()).unwrap();

    // —— Storage 元信息与关闭 ——
    storage.GetOracle();
    assert_eq!("KVMockStorage", storage.Name());
    assert_eq!(
        "KVMockStorage is a mock Store implementation, only for unittests in KV package",
        storage.Describe()
    );
    assert!(!storage.SupportDeleteRange());
    assert!(
        storage
            .ShowStatus(&kv::Context::todo(), "")
            .unwrap()
            .is::<()>()
    );
    storage.Close().unwrap();
}
