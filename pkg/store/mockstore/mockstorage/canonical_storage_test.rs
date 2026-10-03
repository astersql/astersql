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

#[test]
fn crossks_align_schema_checker_rejection_preserves_error_and_uncommitted_writes() {
    let _guard = transaction_test_guard();
    let mut storage = new_storage();
    let store: &mut dyn kv::Storage = &mut storage;
    let mut txn = store.Begin(&[]).unwrap();
    let key = kv::Key(b"schema-rejected-write".to_vec());
    txn.Set(key.clone(), b"private".to_vec()).unwrap();
    txn.SetOption(
        kv::SchemaChecker,
        Some(Box::new(kv::TransactionSchemaChecker(Arc::new(|_| {
            Err(kv::ErrTxnRetryable.FastGenByArgs(&[]))
        })))),
    );
    let error = txn.Commit(&kv::Context::default()).unwrap_err();
    assert!(kv::ErrTxnRetryable.Equal(Some(&error)));
    assert!(txn.Valid());
    assert_eq!(
        txn.Get(&kv::Context::default(), key.clone(), &[])
            .unwrap()
            .Value,
        b"private"
    );
    txn.Rollback().unwrap();
    let version = store.CurrentVersion("global").unwrap();
    assert!(kv::IsErrNotFound(
        &store
            .GetSnapshot(version)
            .Get(&kv::Context::default(), key, &[])
            .unwrap_err()
    ));
}

#[test]
fn crossks_align_schema_checker_validates_the_timestamp_published_by_mvcc() {
    let _guard = transaction_test_guard();
    for asynchronous in [false, true] {
        let mut storage = new_storage();
        let store: &mut dyn kv::Storage = &mut storage;
        let checked = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let observed = checked.clone();
        let mut txn = store.Begin(&[]).unwrap();
        let key = kv::Key(b"schema-checked-write".to_vec());
        txn.Set(key.clone(), b"published".to_vec()).unwrap();
        txn.SetOption(kv::EnableAsyncCommit, Some(Box::new(asynchronous)));
        txn.SetOption(
            kv::SchemaChecker,
            Some(Box::new(kv::TransactionSchemaChecker(Arc::new(
                move |timestamp| {
                    observed.store(timestamp, Ordering::Release);
                    Ok(())
                },
            )))),
        );
        txn.Commit(&kv::Context::default()).unwrap();
        assert_eq!(checked.load(Ordering::Acquire), txn.CommitTS());
        let published = store
            .GetSnapshot(kv::NewVersion(txn.CommitTS()))
            .Get(&kv::Context::default(), key, &[])
            .unwrap();
        assert_eq!(published.Value, b"published");
    }
}

#[test]
fn test_canonical_optimistic_lock_retains_mvcc_conflict_dependency() {
    let _guard = transaction_test_guard();
    let mut store = new_storage();
    let ctx = kv::Context::default();
    let key = kv::Key(b"backfill-source".to_vec());
    let mut seed = kv::Storage::Begin(&mut store, &[]).unwrap();
    seed.Set(key.clone(), b"old".to_vec()).unwrap();
    seed.Commit(&ctx).unwrap();
    let mut reader = kv::Storage::Begin(&mut store, &[]).unwrap();
    reader
        .LockKeys(&ctx, &mut kv::LockCtx::default(), &[key.clone()])
        .unwrap();
    reader
        .Set(kv::Key(b"index".to_vec()), b"old".to_vec())
        .unwrap();
    let mut writer = kv::Storage::Begin(&mut store, &[]).unwrap();
    writer.Set(key.clone(), b"new".to_vec()).unwrap();
    writer.Commit(&ctx).unwrap();
    let error = reader.Commit(&ctx).unwrap_err();
    assert!(kv::IsTxnRetryableError(Some(&error)), "{error}");
    reader.Rollback().unwrap();
    let snapshot = kv::Storage::GetSnapshot(
        &store,
        kv::Storage::CurrentVersion(&store, "global").unwrap(),
    );
    assert_eq!(kv::GetValue(&ctx, snapshot.as_ref(), key).unwrap(), b"new");
    assert!(
        kv::ErrNotExist.Equal(
            kv::GetValue(&ctx, snapshot.as_ref(), kv::Key(b"index".to_vec()))
                .as_ref()
                .err()
        )
    );
}

#[test]
fn embedded_rpc_transactions_preserve_snapshot_buffer_and_conflict_errors() {
    use astersql_kv as kv;
    let store = crate::KVStore::NewEmbeddedRpc().unwrap();
    let rpc = store.EmbeddedRpc().unwrap();
    let client = rpc.client();
    let storage = crate::NewMockStorage(store.clone(), None).unwrap();
    let mut first = kv::Storage::Begin(storage.as_ref(), &[]).unwrap();
    first.Set(kv::Key(b"a".to_vec()), b"old".to_vec()).unwrap();
    first.Commit(&kv::Context::default()).unwrap();
    let snapshot = kv::Storage::GetSnapshot(storage.as_ref(), kv::NewVersion(first.CommitTS()));
    let mut stale = kv::Storage::Begin(storage.as_ref(), &[]).unwrap();
    let mut second = kv::Storage::Begin(storage.as_ref(), &[]).unwrap();
    second.Set(kv::Key(b"a".to_vec()), b"new".to_vec()).unwrap();
    second.Set(kv::Key(b"b".to_vec()), b"row".to_vec()).unwrap();
    // Union scan must include buffered writes and mask buffered deletes.
    second.Delete(kv::Key(b"a".to_vec())).unwrap();
    let mut iter = second
        .Iter(kv::Key(b"a".to_vec()), Some(kv::Key(b"c".to_vec())))
        .unwrap();
    assert!(iter.Valid());
    assert_eq!(iter.Key().0, b"b");
    iter.Next().unwrap();
    assert!(!iter.Valid());
    second.Set(kv::Key(b"a".to_vec()), b"new".to_vec()).unwrap();
    second.Commit(&kv::Context::default()).unwrap();
    assert_eq!(
        snapshot
            .Get(&kv::Context::default(), kv::Key(b"a".to_vec()), &[])
            .unwrap()
            .Value,
        b"old"
    );
    stale
        .Set(kv::Key(b"a".to_vec()), b"stale".to_vec())
        .unwrap();
    let error = stale.Commit(&kv::Context::default()).unwrap_err();
    assert!(kv::ErrTxnRetryable.Equal(Some(&error)), "{error}");
    let fresh = kv::Storage::GetSnapshot(storage.as_ref(), kv::NewVersion(u64::MAX));
    assert_eq!(
        fresh
            .Get(&kv::Context::default(), kv::Key(b"a".to_vec()), &[])
            .unwrap()
            .Value,
        b"new"
    );
    client.set_request_interceptor(Some(std::sync::Arc::new(|_| {
        Err(astersql_store_mockstore_unistore::RpcError::Server(
            "read rejected".into(),
        ))
    })));
    // An interceptor error must not be mistaken for a missing key by BatchGet.
    assert!(
        fresh
            .BatchGet(&kv::Context::default(), &[kv::Key(b"b".to_vec())], &[])
            .unwrap_err()
            .to_string()
            .contains("read rejected")
    );
    client.set_request_interceptor(None);
    store.Close().unwrap();
}

#[test]
fn physical_sst_import_preserves_timestamp_and_historical_mvcc_order() {
    let store = new_storage();
    let pair = |value: &[u8]| vec![(b"sst-key".to_vec(), value.to_vec())];
    kv::Storage::ImportSST(&store, 10, pair(b"old")).unwrap();
    let get = |version| {
        kv::Storage::GetSnapshot(&store, kv::Version { Ver: version }).Get(
            &kv::Context::todo(),
            kv::Key(b"sst-key".to_vec()),
            &[],
        )
    };
    assert!(get(9).is_err());
    assert_eq!(get(10).unwrap().Value, b"old");
    kv::Storage::ImportSST(&store, 20, pair(b"new")).unwrap();
    kv::Storage::ImportSST(&store, 15, pair(b"middle")).unwrap();
    kv::Storage::ImportSST(&store, 10, pair(b"old")).unwrap();
    assert_eq!(get(10).unwrap().Value, b"old");
    assert_eq!(get(15).unwrap().Value, b"middle");
    assert_eq!(get(20).unwrap().Value, b"new");
    assert_eq!(get(u64::MAX).unwrap().Value, b"new");
    assert!(kv::Storage::CurrentVersion(&store, "global").unwrap().Ver >= 20);
}

#[test]
fn test_canonical_transaction_delete_enforces_total_size_limit() {
    let _guard = transaction_test_guard();
    let mut store = new_storage();
    let storage: &mut dyn kv::Storage = &mut store;
    let ctx = kv::Context::default();
    let first = kv::Key(b"first".to_vec());
    let second = kv::Key(b"second".to_vec());
    let mut seed = storage.Begin(&[]).unwrap();
    seed.Set(first.clone(), b"first-value".to_vec()).unwrap();
    seed.Set(second.clone(), b"second-value".to_vec()).unwrap();
    seed.Commit(&ctx).unwrap();
    let _limit = TxnTotalSizeLimitGuard::set(8);
    let mut transaction = storage.Begin(&[]).unwrap();
    transaction.Delete(first.clone()).unwrap();
    let error = transaction
        .Delete(second.clone())
        .expect_err("deletion tombstones must obey the transaction size limit");
    assert!(error.to_string().to_ascii_lowercase().contains("too large"));
    assert_eq!(
        transaction.Get(&ctx, second.clone(), &[]).unwrap().Value,
        b"second-value"
    );
    transaction.Rollback().unwrap();
    let snapshot = storage.GetSnapshot(storage.CurrentVersion("global").unwrap());
    assert_eq!(
        snapshot.Get(&ctx, first, &[]).unwrap().Value,
        b"first-value"
    );
    assert_eq!(
        snapshot.Get(&ctx, second, &[]).unwrap().Value,
        b"second-value"
    );
}
