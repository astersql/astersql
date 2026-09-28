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

// Snapshot 拦截器（SnapshotInterceptor）行为测试。
//
// 快照（snapshot）是事务在某一版本（时间戳）下的只读视图。拦截器可在
// Get / BatchGet / Iter / IterReverse 前后注入观测或错误，用于诊断与测试。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use astersql_store_driver_txn as txn;

/// 将字符串转为键/值字节向量。
fn make_bytes(value: &str) -> Vec<u8> {
    value.as_bytes().to_vec()
}

/// 用给定键值对构造内存存储与快照；值的 commit_ts 固定为 100。
fn prepare_snapshot(
    data: &[(&str, &str)],
) -> (
    Arc<RwLock<BTreeMap<txn::Key, txn::ValueEntry>>>,
    txn::tikvSnapshot,
) {
    let mut map = BTreeMap::new();
    for (key, value) in data {
        map.insert(
            make_bytes(key),
            txn::ValueEntry::new(make_bytes(value), 100),
        );
    }
    let storage = Arc::new(RwLock::new(map));
    let snap = txn::NewSnapshot(Arc::clone(&storage));
    (storage, snap)
}

/// 遍历迭代器并与期望键值序列逐项比对，最后关闭迭代器。
fn check_iter(mut iter: Box<dyn txn::KvIterator>, expected: &[(&str, &str)]) {
    let mut got = Vec::new();
    while iter.valid() {
        got.push((
            String::from_utf8_lossy(iter.key()).into_owned(),
            String::from_utf8_lossy(iter.value()).into_owned(),
        ));
        iter.next().unwrap();
    }
    assert_eq!(
        got.len(),
        expected.len(),
        "got={got:?} expected={expected:?}"
    );
    for (idx, (key, value)) in expected.iter().enumerate() {
        assert_eq!(got[idx].0.as_bytes(), key.as_bytes());
        assert_eq!(got[idx].1.as_bytes(), value.as_bytes());
    }
    iter.close();
}

/// 无拦截器时验证快照点查、批量查、正/反向扫描的边界与缺失键行为。
#[test]
fn TestSnapshotWithoutInterceptor() {
    let (_storage, snap) = prepare_snapshot(&[("k1", "v1"), ("k2", "v2"), ("k3", "v3")]);

    // 点查：命中键返回值；不存在键报 not found。
    let val = snap.Get(b"k1", &[]).unwrap();
    assert_eq!(val, txn::ValueEntry::new(b"v1".to_vec(), 0));
    let val = snap.Get(b"k2", &[]).unwrap();
    assert_eq!(val, txn::ValueEntry::new(b"v2".to_vec(), 0));
    assert!(snap.Get(b"kn", &[]).unwrap_err().is_not_found());

    // 批量查：仅返回存在的键；空键列表得空结果。
    let result = snap
        .BatchGet(&[b"k1".to_vec(), b"k3".to_vec()], &[])
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[&b"k1".to_vec()].value, b"v1");
    assert_eq!(result[&b"k3".to_vec()].value, b"v3");

    let result = snap
        .BatchGet(&[b"k3".to_vec(), b"kn".to_vec()], &[])
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[&b"k3".to_vec()].value, b"v3");

    let result = snap
        .BatchGet(&[b"kn".to_vec(), b"kn2".to_vec()], &[])
        .unwrap();
    assert!(result.is_empty());
    assert!(snap.BatchGet(&[], &[]).unwrap().is_empty());

    // 正向扫描：上界为开区间，下界含端点。
    check_iter(
        snap.Iter(&[], None).unwrap(),
        &[("k1", "v1"), ("k2", "v2"), ("k3", "v3")],
    );
    check_iter(
        snap.Iter(&[], Some(b"k3")).unwrap(),
        &[("k1", "v1"), ("k2", "v2")],
    );
    check_iter(snap.Iter(b"k2", Some(b"k3")).unwrap(), &[("k2", "v2")]);
    check_iter(
        snap.Iter(b"k2", None).unwrap(),
        &[("k2", "v2"), ("k3", "v3")],
    );
    check_iter(snap.Iter(b"k4", None).unwrap(), &[]);

    // 反向扫描：从上限向下遍历，可选下界。
    check_iter(
        snap.IterReverse(Some(b"k5"), None).unwrap(),
        &[("k3", "v3"), ("k2", "v2"), ("k1", "v1")],
    );
    check_iter(
        snap.IterReverse(Some(b"k3"), None).unwrap(),
        &[("k2", "v2"), ("k1", "v1")],
    );
    check_iter(
        snap.IterReverse(Some(b"k4"), Some(b"k2")).unwrap(),
        &[("k3", "v3"), ("k2", "v2")],
    );
    check_iter(snap.IterReverse(Some(b"k1"), None).unwrap(), &[]);
    check_iter(snap.IterReverse(Some(b"k0"), None).unwrap(), &[]);
}

/// 间谍拦截器：记录每次回调参数，并对空键输入返回与 Go 测试一致的错误。
struct SpyInterceptor {
    /// 按调用顺序记录的观测字符串。
    spy: std::sync::Mutex<Vec<String>>,
}

impl txn::SnapshotInterceptor for SpyInterceptor {
    fn on_get(
        &self,
        snapshot: &txn::tikvSnapshot,
        key: &[u8],
        options: &[txn::GetOption],
    ) -> Result<txn::ValueEntry, txn::DriverError> {
        // 记录键与是否要求返回 commit_ts（提交时间戳）。
        self.spy.lock().unwrap().push(format!(
            "OnGet:{}:{}",
            String::from_utf8_lossy(key),
            txn::wants_return_commit_ts_get(options)
        ));
        if key.is_empty() {
            return Err(txn::DriverError::Backend("MockErrOnGet".into()));
        }
        snapshot.Get(key, options)
    }

    fn on_batch_get(
        &self,
        snapshot: &txn::tikvSnapshot,
        keys: &[txn::Key],
        options: &[txn::BatchGetOption],
    ) -> Result<HashMap<txn::Key, txn::ValueEntry>, txn::DriverError> {
        self.spy.lock().unwrap().push(format!(
            "OnBatchGet:{}:{}",
            keys.len(),
            txn::wants_return_commit_ts_batch(options)
        ));
        // 空键列表在此故意报错，便于测试拦截路径。
        if keys.is_empty() {
            return Err(txn::DriverError::Backend("MockErrOnBatchGet".into()));
        }
        snapshot.BatchGet(keys, options)
    }

    fn on_iter(
        &self,
        snapshot: &txn::tikvSnapshot,
        key: &[u8],
        upper_bound: Option<&[u8]>,
    ) -> Result<Box<dyn txn::KvIterator>, txn::DriverError> {
        self.spy.lock().unwrap().push(format!(
            "OnIter:{}:{}",
            String::from_utf8_lossy(key),
            upper_bound
                .map(|bound| String::from_utf8_lossy(bound).into_owned())
                .unwrap_or_default()
        ));
        if key.is_empty() {
            return Err(txn::DriverError::Backend("MockErrOnIter".into()));
        }
        snapshot.Iter(key, upper_bound)
    }

    fn on_iter_reverse(
        &self,
        snapshot: &txn::tikvSnapshot,
        key: Option<&[u8]>,
        lower_bound: Option<&[u8]>,
    ) -> Result<Box<dyn txn::KvIterator>, txn::DriverError> {
        self.spy.lock().unwrap().push(format!(
            "OnIterReverse:{}:{}",
            key.map(|k| String::from_utf8_lossy(k).into_owned())
                .unwrap_or_default(),
            lower_bound
                .map(|bound| String::from_utf8_lossy(bound).into_owned())
                .unwrap_or_default()
        ));
        if key.is_none_or(|key| key.is_empty()) {
            return Err(txn::DriverError::Backend("MockErrOnIterReverse".into()));
        }
        snapshot.IterReverse(key, lower_bound)
    }
}

/// 挂载拦截器后验证回调观测、commit_ts 选项透传，以及拦截错误短路。
#[test]
fn TestSnapshotWitInterceptor() {
    let (_storage, mut snap) = prepare_snapshot(&[("k1", "v1"), ("k2", "v2"), ("k3", "v3")]);
    let interceptor = Arc::new(SpyInterceptor {
        spy: std::sync::Mutex::new(Vec::new()),
    });
    snap.SetOption(txn::SnapshotOption::SnapInterceptor(interceptor.clone()));

    let val = snap.Get(b"k1", &[]).unwrap();
    assert_eq!(val.value, b"v1");
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnGet:k1:false"
    );

    // WithReturnCommitTS 要求返回提交时间戳，spy 中应为 true。
    let val = snap.Get(b"k2", &[txn::WithReturnCommitTS()]).unwrap();
    assert_eq!(val, txn::ValueEntry::new(b"v2".to_vec(), 100));
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnGet:k2:true"
    );

    assert_eq!(snap.Get(&[], &[]).unwrap_err().to_string(), "MockErrOnGet");
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnGet::false"
    );

    let result = snap
        .BatchGet(&[b"k2".to_vec(), b"k3".to_vec()], &[])
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[&b"k2".to_vec()],
        txn::ValueEntry::new(b"v2".to_vec(), 0)
    );
    assert_eq!(
        result[&b"k3".to_vec()],
        txn::ValueEntry::new(b"v3".to_vec(), 0)
    );
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnBatchGet:2:false"
    );

    let result = snap
        .BatchGet(
            &[b"k2".to_vec(), b"k3".to_vec()],
            &[txn::WithReturnCommitTSBatch()],
        )
        .unwrap();
    assert_eq!(result[&b"k2".to_vec()].commit_ts, 100);
    assert_eq!(result[&b"k3".to_vec()].commit_ts, 100);
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnBatchGet:2:true"
    );

    assert!(
        snap.BatchGet(&[], &[])
            .unwrap_err()
            .to_string()
            .contains("MockErrOnBatchGet")
    );

    check_iter(
        snap.Iter(b"k1", Some(b"k3")).unwrap(),
        &[("k1", "v1"), ("k2", "v2")],
    );
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnIter:k1:k3"
    );
    let err = match snap.Iter(&[], Some(b"k3")) {
        Ok(_) => panic!("empty Iter key must be rejected by the interceptor"),
        Err(err) => err,
    };
    assert_eq!(err.to_string(), "MockErrOnIter");
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnIter::k3"
    );

    check_iter(
        snap.IterReverse(Some(b"k3"), None).unwrap(),
        &[("k2", "v2"), ("k1", "v1")],
    );
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnIterReverse:k3:"
    );
    check_iter(
        snap.IterReverse(Some(b"k3"), Some(b"k2")).unwrap(),
        &[("k2", "v2")],
    );
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnIterReverse:k3:k2"
    );
    let err = match snap.IterReverse(Some(&[]), None) {
        Ok(_) => panic!("empty IterReverse key must be rejected by the interceptor"),
        Err(err) => err,
    };
    assert_eq!(err.to_string(), "MockErrOnIterReverse");
    assert_eq!(
        interceptor.spy.lock().unwrap().last().unwrap(),
        "OnIterReverse::"
    );
}
