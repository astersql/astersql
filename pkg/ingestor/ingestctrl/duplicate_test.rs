// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 重复键检测相关单元测试。
//
// 覆盖本地 DupKVStream 的范围过滤/排序/关闭语义，以及 DupeDetector 在
// Error 策略下返回原始冲突、PendingKeyRanges 切分与 pendingIndexHandles 排序清空。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::duplicate::{
    DataConflictInfo, DupKVStream, ErrorManager, NewDupeDetector, NewLocalDupKVStream,
    PendingIndexHandle, RetrieveKeyAndValueFromErrFoundDuplicateKeys, Transaction,
    TransactionFactory, makePendingIndexHandlesWithCapacity, newPendingKeyRanges,
};
use crate::iterator::NoopKeyAdapter;
use crate::{CancellationToken, DuplicateResolution, Error, KeyRange, KvPair, Result};

/// 空操作错误管理器：不持久化冲突。
struct NoopErrors;
impl ErrorManager for NoopErrors {
    fn RecordDataConflictError(&self, _: &[DataConflictInfo]) -> Result<()> {
        Ok(())
    }
    fn RecordIndexConflictError(&self, _: &[DataConflictInfo]) -> Result<()> {
        Ok(())
    }
}

/// 空操作事务：BatchGet 恒为空、Delete/Commit 成功。
struct NoopTransaction;
impl Transaction for NoopTransaction {
    fn BatchGet(&mut self, _: &[Vec<u8>]) -> Result<HashMap<Vec<u8>, Vec<u8>>> {
        Ok(HashMap::new())
    }
    fn Delete(&mut self, _: &[u8]) -> Result<()> {
        Ok(())
    }
    fn Commit(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

/// 空操作事务工厂。
struct NoopTransactions;
impl TransactionFactory for NoopTransactions {
    fn Begin(&self, _: &CancellationToken) -> Result<Box<dyn Transaction>> {
        Ok(Box::new(NoopTransaction))
    }
}

struct FailingStream {
    closed: Arc<AtomicBool>,
}

impl DupKVStream for FailingStream {
    fn Next(&mut self) -> Result<Option<KvPair>> {
        Err(Error::Retryable("stream failed".into()))
    }

    fn Close(&mut self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

/// 本地流按 [start,end) 过滤、按 key 排序，关闭后 Next 返回 Closed。
#[test]
fn local_duplicate_stream_filters_sorts_and_closes_canonically() {
    let pairs = vec![
        KvPair {
            key: b"c".to_vec(),
            value: b"3".to_vec(),
        },
        KvPair {
            key: b"a".to_vec(),
            value: b"1".to_vec(),
        },
        KvPair {
            key: b"b".to_vec(),
            value: b"2".to_vec(),
        },
    ];
    // 仅保留 [b, d) 内的 b、c，并按 key 序产出
    let mut stream = NewLocalDupKVStream(
        pairs,
        Arc::new(NoopKeyAdapter),
        &KeyRange {
            start: b"b".to_vec(),
            end: b"d".to_vec(),
        },
    );
    assert_eq!(stream.Next().unwrap().unwrap().key, b"b");
    assert_eq!(stream.Next().unwrap().unwrap().key, b"c");
    assert_eq!(stream.Next().unwrap(), None);
    stream.Close().unwrap();
    assert_eq!(stream.Next().unwrap_err(), Error::Closed);
}

/// Error 策略返回 Conflict，PendingKeyRanges.finish 切分，句柄可排序与清空。
#[test]
fn duplicate_detector_returns_original_conflict_and_pending_ranges_split() {
    let detector = NewDupeDetector(
        "test.t".into(),
        Arc::new(NoopErrors),
        Arc::new(NoopTransactions),
    );
    let mut stream = NewLocalDupKVStream(
        vec![KvPair {
            key: b"k".to_vec(),
            value: b"v".to_vec(),
        }],
        Arc::new(NoopKeyAdapter),
        &KeyRange {
            start: Vec::new(),
            end: Vec::new(),
        },
    );
    let error = detector
        .RecordDataConflictError(
            &CancellationToken::default(),
            &mut stream,
            DuplicateResolution::Error,
        )
        .unwrap_err();
    assert_eq!(
        RetrieveKeyAndValueFromErrFoundDuplicateKeys(&error).unwrap(),
        (b"k".to_vec(), b"v".to_vec())
    );
    assert!(detector.HasDuplicate());

    // finish([d,m)) 将 [a,z) 切成 [a,d) 与 [m,z)
    let mut pending = newPendingKeyRanges(KeyRange {
        start: b"a".to_vec(),
        end: b"z".to_vec(),
    });
    pending.finish(&KeyRange {
        start: b"d".to_vec(),
        end: b"m".to_vec(),
    });
    assert_eq!(
        pending.list(),
        vec![
            KeyRange {
                start: b"a".to_vec(),
                end: b"d".to_vec()
            },
            KeyRange {
                start: b"m".to_vec(),
                end: b"z".to_vec()
            },
        ]
    );

    let mut handles = makePendingIndexHandlesWithCapacity(2);
    handles.append(PendingIndexHandle {
        raw_handle: b"z".to_vec(),
        ..PendingIndexHandle::default()
    });
    handles.append(PendingIndexHandle {
        raw_handle: b"a".to_vec(),
        ..PendingIndexHandle::default()
    });
    handles.sort();
    assert_eq!(handles.Len(), 2);
    handles.truncate();
    assert_eq!(handles.Len(), 0);
}

#[test]
fn pending_ranges_finish_unbounded_range_does_not_reintroduce_work() {
    let mut pending = newPendingKeyRanges(KeyRange {
        start: b"a".to_vec(),
        end: b"z".to_vec(),
    });

    pending.finish(&KeyRange {
        start: b"d".to_vec(),
        end: Vec::new(),
    });

    assert_eq!(
        pending.list(),
        vec![KeyRange {
            start: b"a".to_vec(),
            end: b"d".to_vec(),
        }]
    );
}

#[test]
fn duplicate_detector_closes_stream_when_read_fails() {
    let detector = NewDupeDetector(
        "test.t".into(),
        Arc::new(NoopErrors),
        Arc::new(NoopTransactions),
    );
    let closed = Arc::new(AtomicBool::new(false));
    let mut stream = FailingStream {
        closed: Arc::clone(&closed),
    };

    assert_eq!(
        detector
            .RecordDataConflictError(
                &CancellationToken::default(),
                &mut stream,
                DuplicateResolution::Record,
            )
            .unwrap_err(),
        Error::Retryable("stream failed".into())
    );
    assert!(closed.load(Ordering::Acquire));
}
