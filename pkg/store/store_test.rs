// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Store 注册表与事务语义的回归测试。
//
// 测试以 MockStorage 覆盖基础读写、迭代、回滚及并发隔离，同时验证驱动注册、
// 打开重试、系统存储初始化，以及 TiKV 规范实例的共享与本地存储边界。

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_kv as kv;
use astersql_store_driver::{InMemoryBackend, TiKVDriver};
use astersql_store_mockstore_mockstorage::{KVStore, MockStorage, NewMockStorage};
use config_dependency::{self as config, StoreType, StoreTypeMockTiKV, StoreTypeTiKV};
use kv::{Getter, Mutator, Retriever, Storage as KvStorage};
use serial_test::serial;

use crate::{
    Driver, LocalStoreDriver, New, Register, ResetStoreStateForTest, StoreError, StoreErrorKind,
    TiKVStoreDriver, newStoreWithRetry,
};

const START_INDEX: i32 = 0;
const TEST_COUNT: i32 = 2;
const INDEX_STEP: i32 = 2;

// 构造彼此隔离的内存存储，避免事务用例共享持久状态。
fn new_mock_store() -> Arc<MockStorage> {
    NewMockStorage(KVStore::NewMemory(), None).expect("create mock storage")
}

fn begin(store: &MockStorage) -> Box<dyn kv::Transaction> {
    KvStorage::Begin(store, &[]).expect("begin transaction")
}

// 固定宽度编码保证整数的字典序与数值顺序一致，便于验证迭代器 seek 语义。
fn encode_int(value: i32) -> Vec<u8> {
    format!("{value:010}").into_bytes()
}

fn insert_data(txn: &mut dyn kv::Transaction) {
    for index in START_INDEX..TEST_COUNT {
        let value = encode_int(index * INDEX_STEP);
        txn.Set(kv::Key(value.clone()), value).unwrap();
    }
}

fn delete_data(txn: &mut dyn kv::Transaction) {
    for index in START_INDEX..TEST_COUNT {
        txn.Delete(kv::Key(encode_int(index * INDEX_STEP))).unwrap();
    }
}

fn assert_get(txn: &dyn kv::Transaction) {
    for index in START_INDEX..TEST_COUNT {
        let key = encode_int(index * INDEX_STEP);
        assert_eq!(
            txn.Get(&kv::Context::todo(), kv::Key(key.clone()), &[])
                .unwrap(),
            kv::NewValueEntry(key, 0)
        );
    }
}

fn assert_not_get(txn: &dyn kv::Transaction) {
    for index in START_INDEX..TEST_COUNT {
        let key = encode_int(index * INDEX_STEP);
        assert!(txn.Get(&kv::Context::todo(), kv::Key(key), &[]).is_err());
    }
}

fn assert_seek(txn: &dyn kv::Transaction) {
    // 命中已有键时，迭代器应从该键开始并返回对应值。
    for index in START_INDEX..TEST_COUNT {
        let key = encode_int(index * INDEX_STEP);
        let mut iterator = txn.Iter(kv::Key(key.clone()), None).unwrap();
        assert_eq!(iterator.Key().0, key);
        assert_eq!(
            String::from_utf8(iterator.Value())
                .unwrap()
                .parse::<i32>()
                .unwrap(),
            index * INDEX_STEP
        );
        iterator.Close();
    }

    // Next 必须推进到字典序中的下一条记录。
    for index in START_INDEX..TEST_COUNT - 1 {
        let key = encode_int(index * INDEX_STEP);
        let mut iterator = txn.Iter(kv::Key(key.clone()), None).unwrap();
        assert_eq!(iterator.Key().0, key);
        iterator.Next().unwrap();
        assert!(iterator.Valid());
        let next = encode_int((index + 1) * INDEX_STEP);
        assert_eq!(iterator.Key().0, next);
        assert_eq!(iterator.Value(), next);
        iterator.Close();
    }

    // 起点越过最大键时没有有效记录。
    let iterator = txn
        .Iter(kv::Key(encode_int(TEST_COUNT * INDEX_STEP)), None)
        .unwrap();
    assert!(!iterator.Valid());

    // 起点落在两个键之间时，seek 返回第一个严格更大的键。
    let between = encode_int((TEST_COUNT - 1) * INDEX_STEP - 1);
    let last = encode_int((TEST_COUNT - 1) * INDEX_STEP);
    let mut iterator = txn.Iter(kv::Key(between.clone()), None).unwrap();
    assert!(iterator.Valid());
    assert_ne!(iterator.Key().0, between);
    assert_eq!(iterator.Key().0, last);
    iterator.Close();
}

#[test]
#[serial]
fn test_new_rejects_unregistered_scheme() {
    ResetStoreStateForTest();
    let error = match New("goleveldb://relative/path") {
        Ok(_) => panic!("unregistered scheme must fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("is not registered"));
}

#[test]
fn test_get_set() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    insert_data(transaction.as_mut());
    assert_get(transaction.as_ref());
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    assert_get(transaction.as_ref());
    delete_data(transaction.as_mut());
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_seek() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    insert_data(transaction.as_mut());
    assert_seek(transaction.as_ref());
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    assert_seek(transaction.as_ref());
    delete_data(transaction.as_mut());
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_inc() {
    let store = new_mock_store();
    let key = kv::Key(b"incKey".to_vec());
    let mut transaction = begin(&store);
    assert_eq!(kv::IncInt64(transaction.as_mut(), &key, 100).unwrap(), 100);
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    assert_eq!(
        kv::IncInt64(transaction.as_mut(), &key, -200).unwrap(),
        -100
    );
    transaction.Delete(key.clone()).unwrap();
    assert_eq!(kv::IncInt64(transaction.as_mut(), &key, 100).unwrap(), 100);
    transaction.Delete(key).unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_delete() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    insert_data(transaction.as_mut());
    delete_data(transaction.as_mut());
    assert_not_get(transaction.as_ref());
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    assert_not_get(transaction.as_ref());
    insert_data(transaction.as_mut());
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    delete_data(transaction.as_mut());
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    assert_not_get(transaction.as_ref());
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_delete_while_iterating() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    for key in [
        "DATA_test_tbl_department_record__0000000001_0003",
        "DATA_test_tbl_department_record__0000000001_0004",
        "DATA_test_tbl_department_record__0000000002_0003",
        "DATA_test_tbl_department_record__0000000002_0004",
    ] {
        transaction
            .Set(kv::Key(key.as_bytes().to_vec()), b"test".to_vec())
            .unwrap();
    }
    transaction.Commit(&kv::Context::default()).unwrap();

    // 遍历期间登记删除，提交后应清空该前缀下的所有记录。
    let mut transaction = begin(&store);
    let mut iterator = transaction
        .Iter(
            kv::Key(b"DATA_test_tbl_department_record__0000000001_0003".to_vec()),
            None,
        )
        .unwrap();
    while iterator.Valid() {
        transaction.Delete(iterator.Key()).unwrap();
        iterator.Next().unwrap();
    }
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    let iterator = transaction
        .Iter(
            kv::Key(b"DATA_test_tbl_department_record__000000000".to_vec()),
            None,
        )
        .unwrap();
    assert!(!iterator.Valid());
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_set_nil() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    assert!(transaction.Set(kv::Key(b"1".to_vec()), Vec::new()).is_err());
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_basic_seek() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    transaction
        .Set(kv::Key(b"1".to_vec()), b"1".to_vec())
        .unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();

    let mut transaction = begin(&store);
    assert!(
        !transaction
            .Iter(kv::Key(b"2".to_vec()), None)
            .unwrap()
            .Valid()
    );
    transaction.Delete(kv::Key(b"1".to_vec())).unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_basic_table() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    for value in 1..5 {
        let bytes = value.to_string().into_bytes();
        transaction
            .Set(kv::Key(bytes.clone()), bytes.clone())
            .unwrap();
    }
    transaction.Commit(&kv::Context::default()).unwrap();

    // 同一事务内的迭代器必须合并快照数据与尚未提交的增删变更。
    let mut transaction = begin(&store);
    transaction
        .Set(kv::Key(b"1".to_vec()), b"1".to_vec())
        .unwrap();
    assert_eq!(
        transaction
            .Iter(kv::Key(b"0".to_vec()), None)
            .unwrap()
            .Key()
            .0,
        b"1"
    );
    transaction
        .Set(kv::Key(b"0".to_vec()), b"0".to_vec())
        .unwrap();
    assert_eq!(
        transaction
            .Iter(kv::Key(b"0".to_vec()), None)
            .unwrap()
            .Key()
            .0,
        b"0"
    );
    transaction.Delete(kv::Key(b"0".to_vec())).unwrap();
    transaction.Delete(kv::Key(b"1".to_vec())).unwrap();
    assert_eq!(
        transaction
            .Iter(kv::Key(b"0".to_vec()), None)
            .unwrap()
            .Key()
            .0,
        b"2"
    );
    transaction.Delete(kv::Key(b"3".to_vec())).unwrap();
    assert_eq!(
        transaction
            .Iter(kv::Key(b"2".to_vec()), None)
            .unwrap()
            .Key()
            .0,
        b"2"
    );
    assert_eq!(
        transaction
            .Iter(kv::Key(b"3".to_vec()), None)
            .unwrap()
            .Key()
            .0,
        b"4"
    );
    transaction.Delete(kv::Key(b"2".to_vec())).unwrap();
    transaction.Delete(kv::Key(b"4".to_vec())).unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_rollback() {
    let store = new_mock_store();
    begin(&store).Rollback().unwrap();

    // 回滚前可读到自身写入；回滚后新事务不得观察到这些值。
    let mut transaction = begin(&store);
    insert_data(transaction.as_mut());
    assert_get(transaction.as_ref());
    transaction.Rollback().unwrap();

    let mut transaction = begin(&store);
    for index in START_INDEX..TEST_COUNT {
        assert!(
            transaction
                .Get(
                    &kv::Context::todo(),
                    kv::Key(index.to_string().into_bytes()),
                    &[],
                )
                .is_err()
        );
    }
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_seek_min() {
    let store = new_mock_store();
    let rows = [
        (
            "DATA_test_main_db_tbl_tbl_test_record__00000000000000000001",
            "lock-version",
        ),
        (
            "DATA_test_main_db_tbl_tbl_test_record__00000000000000000001_0002",
            "1",
        ),
        (
            "DATA_test_main_db_tbl_tbl_test_record__00000000000000000001_0003",
            "hello",
        ),
        (
            "DATA_test_main_db_tbl_tbl_test_record__00000000000000000002",
            "lock-version",
        ),
        (
            "DATA_test_main_db_tbl_tbl_test_record__00000000000000000002_0002",
            "2",
        ),
        (
            "DATA_test_main_db_tbl_tbl_test_record__00000000000000000002_0003",
            "hello",
        ),
    ];
    let mut transaction = begin(&store);
    for (key, value) in rows {
        transaction
            .Set(kv::Key(key.as_bytes().to_vec()), value.as_bytes().to_vec())
            .unwrap();
    }
    // 空起点可遍历到末尾；再次 seek 仍应从目标之后的最小键开始。
    let mut iterator = transaction.Iter(kv::Key::default(), None).unwrap();
    while iterator.Valid() {
        iterator.Next().unwrap();
    }
    let iterator = transaction
        .Iter(
            kv::Key(b"DATA_test_main_db_tbl_tbl_test_record__00000000000000000000".to_vec()),
            None,
        )
        .unwrap();
    assert_eq!(iterator.Key().0, rows[0].0.as_bytes());
    for (key, _) in rows {
        transaction
            .Delete(kv::Key(key.as_bytes().to_vec()))
            .unwrap();
    }
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_condition_if_not_exist() {
    let store = new_mock_store();
    let successes = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    // 多个事务竞争创建同一键；允许冲突失败，但至少一个提交必须成功。
    for _ in 0..100 {
        let store = store.clone();
        let successes = successes.clone();
        workers.push(std::thread::spawn(move || {
            let mut transaction = begin(&store);
            transaction
                .Set(kv::Key(b"1".to_vec()), b"1".to_vec())
                .unwrap();
            if transaction.Commit(&kv::Context::default()).is_ok() {
                successes.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(successes.load(Ordering::SeqCst) > 0);
    let mut transaction = begin(&store);
    transaction.Delete(kv::Key(b"1".to_vec())).unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_condition_if_equal() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    transaction
        .Set(kv::Key(b"1".to_vec()), b"1".to_vec())
        .unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();

    let successes = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    // 并发事务基于同一旧值更新，验证提交时的条件冲突检测。
    for _ in 0..100 {
        let store = store.clone();
        let successes = successes.clone();
        workers.push(std::thread::spawn(move || {
            let mut transaction = begin(&store);
            transaction
                .Set(kv::Key(b"1".to_vec()), b"newValue".to_vec())
                .unwrap();
            if transaction.Commit(&kv::Context::default()).is_ok() {
                successes.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(successes.load(Ordering::SeqCst) > 0);
    let mut transaction = begin(&store);
    transaction.Delete(kv::Key(b"1".to_vec())).unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_condition_update() {
    let store = new_mock_store();
    let mut transaction = begin(&store);
    transaction.Delete(kv::Key(b"b".to_vec())).unwrap();
    kv::IncInt64(transaction.as_mut(), &kv::Key(b"a".to_vec()), 1).unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
#[ignore = "matches the skipped Go TestDBClose"]
fn test_db_close() {
    let store = new_mock_store();
    // 关闭后拒绝新事务，且关闭前开启的事务也不能再提交。
    let mut transaction = begin(&store);
    transaction
        .Set(kv::Key(b"a".to_vec()), b"b".to_vec())
        .unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    let version = KvStorage::CurrentVersion(store.as_ref(), kv::GlobalTxnScope).unwrap();
    let snapshot = KvStorage::GetSnapshot(store.as_ref(), kv::MaxVersion);
    assert!(
        snapshot
            .Get(&kv::Context::todo(), kv::Key(b"a".to_vec()), &[])
            .is_ok()
    );
    let mut transaction = begin(&store);
    store.Close().unwrap();
    assert!(KvStorage::Begin(store.as_ref(), &[]).is_err());
    assert!(version.Ver < kv::MaxVersion.Ver);
    transaction
        .Set(kv::Key(b"a".to_vec()), b"b".to_vec())
        .unwrap();
    assert!(transaction.Commit(&kv::Context::default()).is_err());
}

#[test]
fn test_isolation_inc() {
    let store = new_mock_store();
    let ids = Arc::new(Mutex::new(HashSet::with_capacity(400)));
    let mut workers = Vec::new();
    // 可重试新事务串行化同一计数器的并发自增，所有返回 ID 必须唯一。
    for _ in 0..4 {
        let store = store.clone();
        let ids = ids.clone();
        workers.push(std::thread::spawn(move || {
            for _ in 0..100 {
                let mut id = 0;
                kv::RunInNewTxn(
                    &kv::Context::default(),
                    store.as_ref(),
                    true,
                    |_, transaction| {
                        id = kv::IncInt64(transaction, &kv::Key(b"key".to_vec()), 1)?;
                        Ok(())
                    },
                )
                .unwrap();
                assert!(ids.lock().unwrap().insert(id), "duplicate id {id}");
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(ids.lock().unwrap().len(), 400);
    let mut transaction = begin(&store);
    transaction.Delete(kv::Key(b"key".to_vec())).unwrap();
    transaction.Commit(&kv::Context::default()).unwrap();
    store.Close().unwrap();
}

#[test]
fn test_isolation_multi_inc() {
    let store = new_mock_store();
    let keys: Vec<_> = (0..4)
        .map(|index| kv::Key(format!("test_key_{index}").into_bytes()))
        .collect();
    let mut workers = Vec::new();
    // 每次事务原子地递增全部键，最终每个键都应包含完整的提交次数。
    for _ in 0..4 {
        let store = store.clone();
        let keys = keys.clone();
        workers.push(std::thread::spawn(move || {
            for _ in 0..100 {
                kv::RunInNewTxn(
                    &kv::Context::default(),
                    store.as_ref(),
                    true,
                    |_, transaction| {
                        for key in &keys {
                            kv::IncInt64(transaction, key, 1)?;
                        }
                        Ok(())
                    },
                )
                .unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    kv::RunInNewTxn(
        &kv::Context::default(),
        store.as_ref(),
        false,
        |ctx, transaction| {
            for key in &keys {
                assert_eq!(kv::GetInt64(ctx, transaction, key)?, 400);
                transaction.Delete(key.clone())?;
            }
            Ok(())
        },
    )
    .unwrap();
    store.Close().unwrap();
}

// 始终返回可重试错误，并记录 Open 次数以校验重试策略。
struct BrokenDriver {
    opens: AtomicUsize,
}

impl Driver for BrokenDriver {
    fn Open(&self, _path: &str) -> Result<crate::StorageRef, StoreError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Err(StoreError::new(
            StoreErrorKind::TxnRetryable,
            "transaction retryable",
        ))
    }
}

#[test]
#[serial]
fn test_retry_open_store() {
    ResetStoreStateForTest();
    let driver = Arc::new(BrokenDriver {
        opens: AtomicUsize::new(0),
    });
    Register(StoreTypeMockTiKV, driver.clone()).unwrap();
    let started = Instant::now();
    assert!(newStoreWithRetry("mocktikv://dummy-store", 3).is_err());
    assert_eq!(driver.opens.load(Ordering::SeqCst), 3);
    assert!(started.elapsed() >= Duration::from_secs(3));

    let tso = StoreError::new(StoreErrorKind::PdClientGetTso, "not leader");
    let leader = StoreError::new(StoreErrorKind::PdClientGetLeader, "not leader");
    assert!(crate::isNewStoreRetryableError(Some(&tso)));
    assert!(crate::isNewStoreRetryableError(Some(&leader)));
    assert!(!crate::isNewStoreRetryableError(Some(&StoreError::other(
        "autoid service: not leader"
    ))));
    assert!(!crate::IsNotTSOLeaderError(Some(&StoreError::other(
        "br: not leader"
    ))));
}

#[test]
#[serial]
fn test_zero_retry_count_matches_go_nil_success() {
    ResetStoreStateForTest();
    let driver = Arc::new(BrokenDriver {
        opens: AtomicUsize::new(0),
    });
    Register(StoreTypeMockTiKV, driver.clone()).unwrap();
    // 对齐 Go RunWithRetry：重试次数为零时不调用操作并返回空存储成功值。
    let storage =
        newStoreWithRetry("mocktikv://dummy-store", 0).expect("RunWithRetry(0) returns no error");
    assert!(storage.is_none());
    assert_eq!(driver.opens.load(Ordering::SeqCst), 0);
}

#[test]
#[serial]
fn test_register() {
    ResetStoreStateForTest();
    let invalid = Register(
        StoreType::from("retry"),
        Arc::new(BrokenDriver {
            opens: AtomicUsize::new(0),
        }),
    )
    .unwrap_err();
    assert!(invalid.to_string().contains("invalid storage"));
    Register(
        StoreTypeMockTiKV,
        Arc::new(BrokenDriver {
            opens: AtomicUsize::new(0),
        }),
    )
    .unwrap();
    assert!(
        Register(
            StoreTypeMockTiKV,
            Arc::new(BrokenDriver {
                opens: AtomicUsize::new(0),
            }),
        )
        .unwrap_err()
        .to_string()
        .contains("already registered")
    );
}

#[test]
#[serial]
fn test_init_storage() {
    ResetStoreStateForTest();
    let previous = config::get_global_config();
    let mut current = previous.as_ref().clone();
    current.store = "unistore".to_owned();
    current.path = "store-test".to_owned();
    let keyspace_name = if kerneltype::IsNextGen() {
        keyspace_dependency::System
    } else {
        ""
    };
    current.keyspace_name = keyspace_name.to_owned();
    config::store_global_config(current);

    // next-gen 的 SYSTEM keyspace 初始化后必须同步写入全局系统存储槽位。
    Register(config::StoreTypeUniStore, Arc::new(LocalStoreDriver)).unwrap();
    let storage = crate::MustInitStorage(keyspace_name);
    if kerneltype::IsNextGen() {
        let system = crate::GetSystemStorage().expect("nextgen initializes SYSTEM storage");
        assert!(Arc::ptr_eq(&storage, &system));
    } else {
        assert!(crate::GetSystemStorage().is_none());
    }
    storage.Close().unwrap();

    crate::SetSystemStorage(None);
    config::store_global_config(previous.as_ref().clone());
    ResetStoreStateForTest();
}

#[test]
#[serial]
fn registered_tikv_storage_exposes_the_same_canonical_store_clone() {
    ResetStoreStateForTest();
    Register(
        StoreTypeTiKV,
        Arc::new(TiKVStoreDriver::new(TiKVDriver::with_backend(Arc::new(
            InMemoryBackend::default(),
        )))),
    )
    .unwrap();

    // 多次取得的规范 TiKV 实例共享底层状态，而注册表包装仍保留独立版本视图。
    let storage = New("tikv://shared-store-pd:2379?keyspaceName=listener").unwrap();
    let canonical = storage.CanonicalTiKVStore().unwrap();
    let canonical_again = storage.CanonicalTiKVStore().unwrap();
    assert_eq!(storage.GetClusterID(), Some(canonical.GetClusterID()));
    let registry_version = storage.CurrentVersion("global").unwrap().unwrap();
    let canonical_version = canonical.CurrentVersion("global").unwrap().0;
    assert_eq!(canonical_version, registry_version + 1);
    canonical.SetOption("shared-store-identity", Some(0xA57E_u64));
    assert_eq!(
        *canonical_again
            .GetOption::<u64>("shared-store-identity")
            .unwrap(),
        0xA57E
    );
    storage.Close().unwrap();
    canonical.Close().unwrap();
    canonical_again.Close().unwrap();
}

#[test]
fn local_storage_rejects_canonical_tikv_clone_requests() {
    // 规范 TiKV 克隆只适用于远端 TiKV 驱动，本地存储必须明确拒绝。
    let storage = LocalStoreDriver.Open("unistore://local").unwrap();
    assert!(
        storage
            .CanonicalTiKVStore()
            .unwrap_err()
            .to_string()
            .contains("canonical TiKV store")
    );
}
