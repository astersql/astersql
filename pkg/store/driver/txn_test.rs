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

// `tikvTxn` 驱动层集成测试。
//
// 验证 Get / BatchGet / 范围扫描在脏写缓冲与快照合并下的语义，
// 以及 SnapInterceptor 仅在回落到快照读时生效（缓冲命中优先）。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use astersql_store_driver_txn as txn;
use tikv_client::proto::kvrpcpb::{PessimisticLockKeyResult, PessimisticLockKeyResultType};
use tikv_client::transaction::FairLockDetails;

/// 总是返回固定错误的快照拦截器，用于故障注入。
struct ErrInterceptor {
    err: txn::DriverError,
}

impl txn::SnapshotInterceptor for ErrInterceptor {
    fn on_get(
        &self,
        _: &txn::tikvSnapshot,
        _: &[u8],
        _: &[txn::GetOption],
    ) -> Result<txn::ValueEntry, txn::DriverError> {
        Err(self.err.clone())
    }

    fn on_batch_get(
        &self,
        _: &txn::tikvSnapshot,
        _: &[txn::Key],
        _: &[txn::BatchGetOption],
    ) -> Result<HashMap<txn::Key, txn::ValueEntry>, txn::DriverError> {
        Err(self.err.clone())
    }

    fn on_iter(
        &self,
        _: &txn::tikvSnapshot,
        _: &[u8],
        _: Option<&[u8]>,
    ) -> Result<Box<dyn txn::KvIterator>, txn::DriverError> {
        Err(self.err.clone())
    }

    fn on_iter_reverse(
        &self,
        _: &txn::tikvSnapshot,
        _: Option<&[u8]>,
        _: Option<&[u8]>,
    ) -> Result<Box<dyn txn::KvIterator>, txn::DriverError> {
        Err(self.err.clone())
    }
}

/// 构造带固定 commit_ts=42 的初始存储映射。
fn prepare_store(data: &[(&str, &str)]) -> Arc<RwLock<BTreeMap<txn::Key, txn::ValueEntry>>> {
    let mut map = BTreeMap::new();
    for (key, value) in data {
        map.insert(
            key.as_bytes().to_vec(),
            txn::ValueEntry::new(value.as_bytes().to_vec(), 42),
        );
    }
    Arc::new(RwLock::new(map))
}

#[test]
fn fair_lock_details_are_classified_from_protobuf_key_results() {
    let result = |kind, conflict_ts| PessimisticLockKeyResult {
        r#type: kind as i32,
        locked_with_conflict_ts: conflict_ts,
        ..Default::default()
    };
    let details = FairLockDetails::from_key_results(
        &[
            result(PessimisticLockKeyResultType::LockResultNormal, 0),
            result(
                PessimisticLockKeyResultType::LockResultLockedWithConflict,
                42,
            ),
        ],
        2,
        true,
    );

    assert_eq!(details.aggressive_lock_new_count, 2);
    assert_eq!(details.aggressive_lock_derived_count, 0);
    assert_eq!(details.locked_with_conflict_count, 1);
    let mut retried = details;
    retried.merge(FairLockDetails {
        aggressive_lock_derived_count: 2,
        ..Default::default()
    });
    assert_eq!(retried.aggressive_lock_new_count, 2);
    assert_eq!(retried.aggressive_lock_derived_count, 2);
    assert_eq!(retried.locked_with_conflict_count, 1);
    assert_eq!(
        FairLockDetails::from_key_results(&[], 2, true).aggressive_lock_new_count,
        2
    );
    assert_eq!(
        FairLockDetails::from_key_results(
            &[result(
                PessimisticLockKeyResultType::LockResultLockedWithConflict,
                42,
            )],
            1,
            false,
        ),
        FairLockDetails::default()
    );
}

#[test]
/// 覆盖单键 Get：快照读、返回 commit_ts、Set/Delete、以及拦截器路径。
fn TestTxnGet() {
    let storage = prepare_store(&[("k1", "v1")]);
    let mut txn = txn::NewTiKVTxn(Arc::clone(&storage), 1, false);
    assert!(txn.Valid());

    let entry = txn.Get(b"k1", &[]).unwrap();
    assert_eq!(entry, txn::ValueEntry::new(b"v1".to_vec(), 0));

    let entry = txn.Get(b"k1", &[txn::WithReturnCommitTS()]).unwrap();
    assert_eq!(entry, txn::ValueEntry::new(b"v1".to_vec(), 42));

    txn.Set(b"k1".to_vec(), b"v2".to_vec()).unwrap();
    let entry = txn.Get(b"k1", &[]).unwrap();
    assert_eq!(entry, txn::ValueEntry::new(b"v2".to_vec(), 0));
    let entry = txn.Get(b"k1", &[txn::WithReturnCommitTS()]).unwrap();
    assert_eq!(entry, txn::ValueEntry::new(b"v2".to_vec(), 0));

    txn.Set(b"k2".to_vec(), b"v2+".to_vec()).unwrap();
    assert_eq!(
        txn.Get(b"k2", &[]).unwrap(),
        txn::ValueEntry::new(b"v2+".to_vec(), 0)
    );

    txn.Delete(b"k1".to_vec()).unwrap();
    assert!(txn.Get(b"k1", &[]).unwrap_err().is_not_found());
    assert!(
        txn.Get(b"k1", &[txn::WithReturnCommitTS()])
            .unwrap_err()
            .is_not_found()
    );
    assert!(txn.Get(b"missing", &[]).unwrap_err().is_not_found());

    let interceptor = Arc::new(ErrInterceptor {
        err: txn::DriverError::Backend("mock err".into()),
    });
    txn.SetOption(txn::TxnOption::SnapInterceptor(interceptor))
        .unwrap();
    // Dirty buffer still wins before snapshot interceptor.
    // 脏写缓冲优先于快照拦截器。
    assert!(txn.Get(b"k1", &[]).unwrap_err().is_not_found());
    assert_eq!(txn.Get(b"k2", &[]).unwrap().value, b"v2+");
    // Snapshot miss path propagates interceptor error.
    // 快照未命中时才走拦截器并传播错误。
    assert_eq!(
        txn.Get(b"missing", &[]).unwrap_err().to_string(),
        "mock err"
    );
}

#[test]
/// 覆盖 BatchGet：部分命中、commit_ts、缓冲覆盖/删除，以及未解析键走拦截器。
fn TestTxnBatchGet() {
    let storage = prepare_store(&[("k1", "v1"), ("k2", "v2"), ("k3", "v3"), ("k4", "v4")]);
    let mut txn = txn::NewTiKVTxn(Arc::clone(&storage), 1, false);

    let result = txn
        .BatchGet(
            &[
                b"k1".to_vec(),
                b"k2".to_vec(),
                b"k3".to_vec(),
                b"kn".to_vec(),
            ],
            &[],
        )
        .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[&b"k1".to_vec()].value, b"v1");
    assert_eq!(result[&b"k2".to_vec()].value, b"v2");
    assert_eq!(result[&b"k3".to_vec()].value, b"v3");

    let result = txn
        .BatchGet(
            &[
                b"k1".to_vec(),
                b"k2".to_vec(),
                b"k3".to_vec(),
                b"kn".to_vec(),
            ],
            &[txn::WithReturnCommitTSBatch()],
        )
        .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[&b"k1".to_vec()].commit_ts, 42);
    assert_eq!(result[&b"k2".to_vec()].commit_ts, 42);
    assert_eq!(result[&b"k3".to_vec()].commit_ts, 42);

    txn.Set(b"k1".to_vec(), b"x1".to_vec()).unwrap();
    txn.Set(b"k4".to_vec(), b"x4".to_vec()).unwrap();
    txn.Delete(b"k2".to_vec()).unwrap();
    let result = txn
        .BatchGet(
            &[
                b"k1".to_vec(),
                b"k2".to_vec(),
                b"k3".to_vec(),
                b"k4".to_vec(),
                b"kn".to_vec(),
            ],
            &[],
        )
        .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[&b"k1".to_vec()].value, b"x1");
    assert_eq!(result[&b"k3".to_vec()].value, b"v3");
    assert_eq!(result[&b"k4".to_vec()].value, b"x4");
    assert!(!result.contains_key(&b"k2".to_vec()));

    let result = txn
        .BatchGet(&[b"k1".to_vec(), b"k4".to_vec()], &[])
        .unwrap();
    assert_eq!(result.len(), 2);

    let result = txn
        .BatchGet(
            &[
                b"k1".to_vec(),
                b"k2".to_vec(),
                b"k3".to_vec(),
                b"k4".to_vec(),
            ],
            &[txn::WithReturnCommitTSBatch()],
        )
        .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[&b"k1".to_vec()].commit_ts, 0);
    assert_eq!(result[&b"k3".to_vec()].commit_ts, 42);
    assert_eq!(result[&b"k4".to_vec()].commit_ts, 0);

    let interceptor = Arc::new(ErrInterceptor {
        err: txn::DriverError::Backend("batch err".into()),
    });
    txn.SetOption(txn::TxnOption::SnapInterceptor(interceptor))
        .unwrap();
    // Unresolved keys still hit the snapshot interceptor.
    // 缓冲未覆盖的键仍会打到快照拦截器。
    assert_eq!(
        txn.BatchGet(&[b"k3".to_vec()], &[])
            .unwrap_err()
            .to_string(),
        "batch err"
    );
    for keys in [
        vec![b"k1".to_vec(), b"k3".to_vec(), b"k4".to_vec()],
        vec![b"k1".to_vec(), b"k4".to_vec(), b"kn".to_vec()],
    ] {
        assert_eq!(
            txn.BatchGet(&keys, &[]).unwrap_err().to_string(),
            "batch err"
        );
    }
}

#[test]
/// 覆盖正向/反向扫描合并，以及 Iter 创建时快照拦截器失败。
fn TestTxnScan() {
    let storage = prepare_store(&[
        ("k1", "v1"),
        ("k3", "v3"),
        ("k5", "v5"),
        ("k7", "v7"),
        ("k9", "v9"),
    ]);
    let mut txn = txn::NewTiKVTxn(Arc::clone(&storage), 1, false);

    let mut iter = txn.Iter(b"k3", Some(b"k9")).unwrap();
    let mut keys = Vec::new();
    while iter.valid() {
        keys.push(String::from_utf8(iter.key().to_vec()).unwrap());
        iter.next().unwrap();
    }
    assert_eq!(keys, ["k3", "k5", "k7"]);

    let mut iter = txn.IterReverse(Some(b"k9"), None).unwrap();
    let mut keys = Vec::new();
    while iter.valid() {
        keys.push(String::from_utf8(iter.key().to_vec()).unwrap());
        iter.next().unwrap();
    }
    assert_eq!(keys, ["k7", "k5", "k3", "k1"]);

    let mut iter = txn.IterReverse(Some(b"k9"), Some(b"k3")).unwrap();
    let mut keys = Vec::new();
    while iter.valid() {
        keys.push(String::from_utf8(iter.key().to_vec()).unwrap());
        iter.next().unwrap();
    }
    assert_eq!(keys, ["k7", "k5", "k3"]);

    txn.Set(b"k1".to_vec(), b"v1+".to_vec()).unwrap();
    txn.Set(b"k3".to_vec(), b"v3+".to_vec()).unwrap();
    txn.Set(b"k31".to_vec(), b"v31+".to_vec()).unwrap();
    txn.Delete(b"k5".to_vec()).unwrap();
    let mut iter = txn.Iter(b"k3", Some(b"k9")).unwrap();
    let mut pairs = Vec::new();
    while iter.valid() {
        pairs.push((
            String::from_utf8(iter.key().to_vec()).unwrap(),
            String::from_utf8(iter.value().to_vec()).unwrap(),
        ));
        iter.next().unwrap();
    }
    assert_eq!(
        pairs,
        [
            ("k3".into(), "v3+".into()),
            ("k31".into(), "v31+".into()),
            ("k7".into(), "v7".into()),
        ]
    );

    let mut iter = txn.IterReverse(Some(b"k9"), None).unwrap();
    let mut keys = Vec::new();
    while iter.valid() {
        keys.push(String::from_utf8(iter.key().to_vec()).unwrap());
        iter.next().unwrap();
    }
    assert_eq!(keys, ["k7", "k31", "k3", "k1"]);

    let interceptor = Arc::new(ErrInterceptor {
        err: txn::DriverError::Backend("scan err".into()),
    });
    txn.SetOption(txn::TxnOption::SnapInterceptor(interceptor))
        .unwrap();
    let err = match txn.Iter(b"k1", Some(b"k2")) {
        Ok(_) => panic!("expected Iter to fail"),
        Err(error) => error,
    };
    assert_eq!(err.to_string(), "scan err");
}
