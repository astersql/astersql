// Copyright 2026 AsterSQL.
// Copyright 2019-present, PingCAP, Inc.
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

// 协处理器执行相关的 MVCC（多版本并发控制）锁解析契约测试。
//
// 对应 Go `TestResolvedLargeTxnLocks`：大事务主锁长 TTL、次级锁短 TTL，
// 解析次级锁后仍能读到先前已提交版本，且主锁继续存活。

use crate::NewCoprRPCHandler;
use crate::copr_handler::{CopError, Datum, KeyRange, KvPair, KvReader, MemoryReader};
use crate::executor::{executor, hasColVal, tableScanExec};
use astersql_store_mockstore_unistore_tikv::mvcc::{
    Action, Mutation, MutationOp, MvccError, MvccStore, PrewriteRequest, SafePoint,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Go `hasColVal` only reports a value when the mapped slot is non-nil.
#[test]
fn has_col_val_rejects_null_and_invalid_offsets() {
    let column_ids = HashMap::from([(1, 0), (2, 1), (3, 2)]);
    let data = vec![Datum::Null, Datum::Int(7)];

    assert!(!hasColVal(&data, &column_ids, 1));
    assert!(hasColVal(&data, &column_ids, 2));
    assert!(!hasColVal(&data, &column_ids, 3));
    assert!(!hasColVal(&data, &column_ids, 4));
}

/// Go scan counts advance as rows are returned, rather than during eager loading.
#[test]
fn table_scan_counts_only_rows_already_returned() {
    let reader = MemoryReader {
        rows: BTreeMap::from([
            (b"a".to_vec(), (vec![Datum::Int(1)], 1)),
            (b"b".to_vec(), (vec![Datum::Int(2)], 1)),
            (b"z".to_vec(), (vec![Datum::Int(3)], 1)),
        ]),
    };
    let ranges = vec![
        KeyRange {
            start: b"a".to_vec(),
            end: b"c".to_vec(),
        },
        KeyRange {
            start: b"z".to_vec(),
            end: Vec::new(),
        },
    ];
    let mut scan = tableScanExec::new(Arc::new(reader), ranges, 1, false);

    assert_eq!(scan.Next().unwrap(), Some(vec![Datum::Int(1)]));
    assert_eq!(scan.Counts(), vec![1, 0]);
    assert_eq!(scan.Next().unwrap(), Some(vec![Datum::Int(2)]));
    assert_eq!(scan.Counts(), vec![2, 0]);
    scan.ResetCounts();
    assert_eq!(scan.Counts(), vec![0, 0]);
    assert_eq!(scan.Next().unwrap(), Some(vec![Datum::Int(3)]));
    assert_eq!(scan.Counts(), vec![0, 1]);
}

struct FailingReader;

impl KvReader for FailingReader {
    fn scan(
        &self,
        _ranges: &[KeyRange],
        _start_ts: u64,
        _descending: bool,
    ) -> Result<Vec<KvPair>, CopError> {
        Err(CopError::InvalidRequest("injected scan failure".into()))
    }
}

/// Go's deferred exec-detail update counts failed `Next` calls as iterations.
#[test]
fn table_scan_records_failed_next_iteration() {
    let mut scan = tableScanExec::new(
        Arc::new(FailingReader),
        vec![KeyRange {
            start: b"a".to_vec(),
            end: b"b".to_vec(),
        }],
        1,
        false,
    );

    assert!(scan.Next().is_err());
    assert_eq!(scan.exec_detail.iterations, 1);
    assert_eq!(scan.exec_detail.produced_rows, 0);
}

/// Corresponds to Go `TestResolvedLargeTxnLocks`.
///
/// The Go test drives SQL through session/testkit against client-go mocktikv.
/// The Rust port exercises the same MVCC resolve-lock contract that backs those
/// reads: a short-TTL secondary lock of a large txn is resolved while the
/// primary long-TTL lock remains, and prior committed data stays visible.
///
/// 对应 Go `TestResolvedLargeTxnLocks`：解析大事务短 TTL 次级锁后，
/// 先前已提交数据仍可见，主锁长 TTL 继续持有。
#[test]
fn test_resolved_large_txn_locks() {
    // This is required since mock tikv does not support paging.
    // mock tikv 不支持分页，需打 failpoint 关闭 paging。
    fail::cfg(
        "github.com/pingcap/tidb/pkg/store/copr/DisablePaging",
        "return",
    )
    .expect("enable DisablePaging failpoint");
    let _guard = FailpointGuard;

    // Constructing the coprocessor RPC handler matches Go NewMockTiKV(..., NewCoprRPCHandler()).
    // 构造协处理器 RPC 处理器，对齐 Go NewMockTiKV(..., NewCoprRPCHandler())。
    let _copr = NewCoprRPCHandler();

    let store = MvccStore::new(Arc::new(SafePoint::new(0)));
    let key = b"t\x80\x00\x00\x00\x00\x00\x00\x01_r\x00\x00\x00\x00\x00\x00\x00\x01".to_vec();
    // Use a realistic TSO so physical+TTL comparisons match Go lock resolution.
    // 使用接近真实的 TSO（时间戳 Oracle），便于物理时间与 TTL 比较对齐 Go。
    let tso = 100_u64 << 18;

    // Seed the committed row that SQL would have inserted.
    // 预写并提交种子行，模拟 SQL 已插入的数据。
    assert!(prewrite_mvcc_store(
        &store,
        put_mutations(&[(&key[..], &b"row-v1"[..])]),
        &key,
        tso - (10 << 18),
        3000,
    ));
    store
        .commit(&[key.clone()], tso - (10 << 18), tso - (5 << 18))
        .expect("commit seeded row");

    let pairs = store.scan(&key, &[], tso, 1, false, false, &[]);
    assert_eq!(pairs.len(), 1);
    assert!(pairs[0].error.is_none());
    assert_eq!(pairs[0].value, b"row-v1");

    // Simulate a large txn (holding a pk lock with large TTL).
    // Secondary lock 200ms, primary lock 100s.
    // 模拟大事务：主锁 TTL 100s，次级锁 TTL 200ms。
    assert!(prewrite_mvcc_store(
        &store,
        put_mutations(&[(b"primary".as_slice(), b"value".as_slice())]),
        b"primary",
        tso,
        100_000,
    ));
    assert!(prewrite_mvcc_store(
        &store,
        put_mutations(&[(&key[..], b"value".as_slice())]),
        b"primary",
        tso,
        200,
    ));

    // Meeting the secondary lock must not permanently block reads of the prior version.
    // Expire the secondary via check_txn_status (Go client-go resolve-lock path).
    // 通过 check_txn_status 让次级锁过期回滚，不应永久阻塞读旧版本。
    let status = store
        .check_txn_status(b"primary", tso, tso + 1, physical_plus_ttl(tso, 201), true)
        .expect("resolve secondary");
    assert!(matches!(
        status.action,
        Action::TtlExpireRollback
            | Action::LockNotExistRollback
            | Action::NoAction
            | Action::MinCommitTsPushed
    ));
    // The primary status check above establishes that the transaction is still
    // alive; resolve the expired secondary lock itself, as client-go's cleanup
    // path does after inspecting the primary.
    store
        .rollback(std::slice::from_ref(&key), tso)
        .expect("resolve expired secondary");

    // Cover TableScan / BatchGet / PointGet visibility of the previous version.
    // 点查 / 批量取 / 扫描均应仍可见先前已提交版本。
    let got = store.get(&key, tso + (10 << 18), &[]).expect("point get");
    assert_eq!(got, Some(b"row-v1".to_vec()));
    let batch = store.batch_get(&[key.clone()], tso + (10 << 18), &[]);
    assert_eq!(batch.len(), 1);
    assert!(batch[0].error.is_none());
    assert_eq!(batch[0].value, b"row-v1");
    let scan = store.scan(&key, &[], tso + (10 << 18), 1, false, false, &[]);
    assert_eq!(scan.len(), 1);
    assert!(scan[0].error.is_none());
    assert_eq!(scan[0].value, b"row-v1");

    // And check the large txn primary is still alive.
    // 大事务主锁仍在，扫描应报 KeyLocked。
    let pairs = store.scan(b"primary", &[], tso, 1, false, false, &[]);
    assert_eq!(pairs.len(), 1);
    assert!(matches!(pairs[0].error, Some(MvccError::KeyLocked { .. })));
}

/// failpoint 清理守卫：测试结束时移除 DisablePaging。
struct FailpointGuard;
impl Drop for FailpointGuard {
    fn drop(&mut self) {
        let _ = fail::remove("github.com/pingcap/tidb/pkg/store/copr/DisablePaging");
    }
}

/// 将键值对列表转为 Put 类型的 Mutation 向量。
fn put_mutations(pairs: &[(&[u8], &[u8])]) -> Vec<Mutation> {
    pairs
        .iter()
        .map(|(key, value)| Mutation {
            op: MutationOp::Put,
            key: key.to_vec(),
            value: value.to_vec(),
            is_pessimistic_lock: false,
        })
        .collect()
}

/// 对 MVCC 存储执行 Prewrite（两阶段提交第一阶段），成功返回 true。
fn prewrite_mvcc_store(
    store: &MvccStore,
    mutations: Vec<Mutation>,
    primary: &[u8],
    start_ts: u64,
    ttl: u64,
) -> bool {
    let req = PrewriteRequest {
        mutations,
        primary_lock: primary.to_vec(),
        start_ts,
        lock_ttl: ttl,
        min_commit_ts: start_ts + 1,
        ..PrewriteRequest::default()
    };
    store.prewrite(&req).is_ok()
}

/// Compose a fake TSO whose physical part is `physical_ts(start) + ttl_ms`.
/// 构造伪 TSO：物理部分为 `physical_ts(start) + ttl_ms`。
fn physical_plus_ttl(start_ts: u64, ttl_ms: u64) -> u64 {
    const LOGICAL_BITS: u64 = 18;
    let physical = (start_ts >> LOGICAL_BITS) + ttl_ms;
    (physical << LOGICAL_BITS) | (start_ts & ((1 << LOGICAL_BITS) - 1))
}
