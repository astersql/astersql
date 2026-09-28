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

// mockStorage 对规范 `kv::Storage` trait 的实现合规性测试。
//
// 覆盖 Begin/Commit/GetSnapshot/Rollback/Close 等事务与快照基本路径。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard, OnceLock};

use astersql_kv as kv;

use super::{KVStore, NewMockStorage, mockStorage};

/// 编译期断言：类型 `T` 实现了规范 Storage 接口。
fn assert_canonical_storage<T: kv::Storage>() {}

/// 构造单所有者的 mockStorage，便于可变借用跑事务。
fn new_storage() -> mockStorage {
    Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create mock storage"))
        .unwrap_or_else(|_| panic!("new storage must have one owner"))
}

/// Serialize tests that exercise transactions because `TxnTotalSizeLimit` is a
/// process-wide setting, matching the corresponding Go package global.
fn transaction_test_guard() -> MutexGuard<'static, ()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct TxnTotalSizeLimitGuard(u64);

impl TxnTotalSizeLimitGuard {
    fn set(limit: u64) -> Self {
        Self(kv::TxnTotalSizeLimit.swap(limit, Ordering::SeqCst))
    }
}

impl Drop for TxnTotalSizeLimitGuard {
    fn drop(&mut self) {
        kv::TxnTotalSizeLimit.store(self.0, Ordering::SeqCst);
    }
}

/// 确认 mockStorage 满足 `kv::Storage` 约束。
#[test]
fn test_mock_storage_implements_canonical_storage() {
    assert_canonical_storage::<mockStorage>();
}

/// 验证事务提交可见、回滚不可见，以及 Close 后无法再 Begin。
#[test]
fn test_canonical_begin_commit_snapshot_rollback_and_close() {
    let _guard = transaction_test_guard();
    let mut store = new_storage();
    let storage: &mut dyn kv::Storage = &mut store;
    let ctx = kv::Context::default();
    let committed_key = kv::Key(b"committed".to_vec());
    let rolled_back_key = kv::Key(b"rolled-back".to_vec());

    // 开启事务，写入并读回本事务未提交写（read-your-writes）。
    let mut txn = storage.Begin(&[]).expect("begin transaction");
    txn.Set(committed_key.clone(), b"value".to_vec())
        .expect("set value");
    assert_eq!(
        txn.Get(&ctx, committed_key.clone(), &[])
            .expect("read own write")
            .Value,
        b"value".to_vec()
    );
    txn.Commit(&ctx).expect("commit transaction");

    // 提交后通过当前版本快照（Snapshot：某一时间点的只读视图）读到已提交值。
    let version = storage.CurrentVersion("global").expect("current version");
    let snapshot = storage.GetSnapshot(version);
    assert_eq!(
        snapshot
            .Get(&ctx, committed_key.clone(), &[])
            .expect("read committed value")
            .Value,
        b"value".to_vec()
    );

    // 另一事务写入后 Rollback，快照中不应出现该键。
    let mut txn = storage.Begin(&[]).expect("begin rollback transaction");
    txn.Set(rolled_back_key.clone(), b"discarded".to_vec())
        .expect("set rollback value");
    txn.Rollback().expect("rollback transaction");

    let version = storage.CurrentVersion("global").expect("current version");
    let snapshot = storage.GetSnapshot(version);
    assert!(snapshot.Get(&ctx, rolled_back_key, &[]).is_err());

    // Close 后 Begin 应失败。
    storage.Close().expect("close storage");
    assert!(storage.Begin(&[]).is_err());
}

#[test]
fn test_canonical_transaction_enforces_total_size_limit() {
    let _guard = transaction_test_guard();
    let mut store = new_storage();
    let storage: &mut dyn kv::Storage = &mut store;
    let _limit = TxnTotalSizeLimitGuard::set(8);
    let mut transaction = storage.Begin(&[]).expect("begin size-limited transaction");
    transaction
        .Set(kv::Key(b"a".to_vec()), b"1234".to_vec())
        .expect("first write fits");
    let error = transaction
        .Set(kv::Key(b"b".to_vec()), b"5678".to_vec())
        .expect_err("second write exceeds total limit");
    assert!(
        error.to_string().to_ascii_lowercase().contains("too large"),
        "unexpected size-limit error: {error}"
    );
}

/// A stale-read transaction must retain its selected timestamp instead of
/// allocating a fresh oracle timestamp at `Begin`.
#[test]
fn test_canonical_begin_with_explicit_start_ts() {
    let _guard = transaction_test_guard();
    let mut store = new_storage();
    let storage: &mut dyn kv::Storage = &mut store;
    let transaction = storage
        .Begin(&[kv::tikv::TxnOption::StartTS(42)])
        .expect("begin transaction at stale timestamp");
    assert_eq!(transaction.StartTS(), 42);
}

/// Go mockstore transaction reads expose a zero CommitTs and reject a stale
/// writer with the canonical retryable transaction error.
#[test]
fn test_canonical_transaction_get_and_write_conflict_match_go() {
    let _guard = transaction_test_guard();
    let mut store = new_storage();
    let storage: &mut dyn kv::Storage = &mut store;
    let ctx = kv::Context::default();
    let key = kv::Key(b"conflict".to_vec());

    let mut first = storage.Begin(&[]).expect("begin first transaction");
    let mut stale = storage.Begin(&[]).expect("begin stale transaction");
    first
        .Set(key.clone(), b"first".to_vec())
        .expect("set first value");
    stale
        .Set(key.clone(), b"stale".to_vec())
        .expect("set stale value");
    first.Commit(&ctx).expect("commit first transaction");

    let mut reader = storage.Begin(&[]).expect("begin reader");
    let entry = reader.Get(&ctx, key, &[]).expect("read committed value");
    assert_eq!(entry.Value, b"first");
    assert_eq!(entry.CommitTs, 0);
    reader.Rollback().expect("rollback reader");

    let error = stale.Commit(&ctx).expect_err("stale writer must conflict");
    assert!(kv::IsTxnRetryableError(Some(&error)));
}

/// Async commit reuses the oracle upper bound established during prewrite
/// instead of allocating a later timestamp at commit time.
#[test]
fn test_async_commit_reuses_prewrite_oracle_upper_bound() {
    let _guard = transaction_test_guard();
    let mut store = new_storage();
    let storage: &mut dyn kv::Storage = &mut store;
    let ctx = kv::Context::default();
    let mut transaction = storage.Begin(&[]).expect("begin async transaction");
    transaction
        .Set(kv::Key(b"async".to_vec()), b"value".to_vec())
        .expect("buffer async write");
    transaction.SetOption(kv::EnableAsyncCommit, Some(Box::new(true)));

    let prewrite_oracle_upper_bound = storage
        .CurrentVersion("global")
        .expect("advance oracle during prewrite")
        .Ver;
    transaction.Commit(&ctx).expect("commit async transaction");

    assert_eq!(transaction.CommitTS(), prewrite_oracle_upper_bound);
}
