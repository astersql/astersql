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

// `UnionIter` 合并扫描行为与错误传播测试。
//
// 覆盖：仅 dirty / 仅 snapshot / 二者合并、删除 tombstone、正反向扫描，
// 以及创建期与 Next 路径上的错误注入（确认失败时不 Close 调用方迭代器）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::*;

/// 测试用键值记录；value 为空表示删除。
#[derive(Clone)]
struct Entry {
    key: Vec<u8>,
    value: Vec<u8>,
}

/// 便捷构造：`"nil"` 表示空字节值（tombstone）。
fn r(key: &str, value: &str) -> Entry {
    let b_value = if value == "nil" {
        Vec::new()
    } else {
        value.as_bytes().to_vec()
    };
    Entry {
        key: key.as_bytes().to_vec(),
        value: b_value,
    }
}

/// 可跟踪 Close 与注入 Next 错误的测试迭代器。
struct TrackedIter {
    data: Vec<Entry>,
    cur: isize,
    closed: Arc<AtomicBool>,
    next_err: Arc<Mutex<Option<DriverError>>>,
}

impl TrackedIter {
    fn new(
        records: Vec<Entry>,
        closed: Arc<AtomicBool>,
        next_err: Arc<Mutex<Option<DriverError>>>,
    ) -> Box<Self> {
        Box::new(Self {
            data: records,
            cur: 0,
            closed,
            next_err,
        })
    }
}

impl KvIterator for TrackedIter {
    fn next(&mut self) -> Result<(), DriverError> {
        if let Some(error) = self.next_err.lock().unwrap().clone() {
            return Err(error);
        }
        if self.cur < 0 || (self.cur as usize) >= self.data.len() {
            return Err(DriverError::Backend("iterator is invalid".to_owned()));
        }
        self.cur += 1;
        Ok(())
    }

    fn key(&self) -> &[u8] {
        if self.cur < 0 || (self.cur as usize) >= self.data.len() {
            return &[];
        }
        &self.data[self.cur as usize].key
    }

    fn value(&self) -> &[u8] {
        if self.cur < 0 || (self.cur as usize) >= self.data.len() {
            return &[];
        }
        &self.data[self.cur as usize].value
    }

    fn valid(&self) -> bool {
        self.cur >= 0 && (self.cur as usize) < self.data.len()
    }

    fn close(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cur = -1;
    }
}

#[test]
/// 验证 UnionIter 在多种 dirty/snapshot 组合下的合并结果（含反向）。
fn TestUnionIter() {
    let snap_records = vec![
        r("k00", "v0"),
        r("k01", "v1"),
        r("k03", "v3"),
        r("k06", "v6"),
        r("k10", "v10"),
        r("k12", "v12"),
        r("k15", "v15"),
        r("k16", "v16"),
    ];
    let dirty_records = vec![
        r("k00", ""),
        r("k000", ""),
        r("k03", "x3"),
        r("k05", "x5"),
        r("k07", "x7"),
        r("k08", "x8"),
    ];

    assertUnionIter(
        dirty_records.clone(),
        Vec::new(),
        vec![
            r("k03", "x3"),
            r("k05", "x5"),
            r("k07", "x7"),
            r("k08", "x8"),
        ],
    );
    assertUnionIter(
        Vec::new(),
        snap_records.clone(),
        vec![
            r("k00", "v0"),
            r("k01", "v1"),
            r("k03", "v3"),
            r("k06", "v6"),
            r("k10", "v10"),
            r("k12", "v12"),
            r("k15", "v15"),
            r("k16", "v16"),
        ],
    );
    assertUnionIter(
        dirty_records,
        snap_records.clone(),
        vec![
            r("k01", "v1"),
            r("k03", "x3"),
            r("k05", "x5"),
            r("k06", "v6"),
            r("k07", "x7"),
            r("k08", "x8"),
            r("k10", "v10"),
            r("k12", "v12"),
            r("k15", "v15"),
            r("k16", "v16"),
        ],
    );

    let dirty_records = vec![
        r("k03", "x3"),
        r("k05", "x5"),
        r("k07", "x7"),
        r("k08", "x8"),
        r("k17", "x17"),
        r("k18", "x18"),
    ];
    assertUnionIter(
        dirty_records,
        snap_records,
        vec![
            r("k00", "v0"),
            r("k01", "v1"),
            r("k03", "x3"),
            r("k05", "x5"),
            r("k06", "v6"),
            r("k07", "x7"),
            r("k08", "x8"),
            r("k10", "v10"),
            r("k12", "v12"),
            r("k15", "v15"),
            r("k16", "v16"),
            r("k17", "x17"),
            r("k18", "x18"),
        ],
    );
}

/// 错误注入用例：指定在第几次 Next 或创建时注入 dirty/snap 错误。
struct ErrorCase {
    dirty: Vec<Entry>,
    snap: Vec<Entry>,
    next_times_when_error_happens: usize,
    inject_dirty_error: Option<DriverError>,
    inject_snap_error: Option<DriverError>,
}

#[test]
/// 验证错误路径：创建失败或 Next 失败时底层迭代器不被自动 Close。
fn TestUnionIterErrors() {
    let cases = [
        ErrorCase {
            dirty: vec![r("k0", ""), r("k1", "v1")],
            snap: Vec::new(),
            next_times_when_error_happens: 0,
            inject_dirty_error: Some(DriverError::Backend("error".into())),
            inject_snap_error: None,
        },
        ErrorCase {
            dirty: vec![r("k0", ""), r("k1", "v1")],
            snap: vec![r("k1", "x1")],
            next_times_when_error_happens: 0,
            inject_dirty_error: Some(DriverError::Backend("error".into())),
            inject_snap_error: None,
        },
        ErrorCase {
            dirty: vec![r("k0", "v1"), r("k1", "v1")],
            snap: vec![r("k0", "x0"), r("k1", "x1")],
            next_times_when_error_happens: 0,
            inject_dirty_error: None,
            inject_snap_error: Some(DriverError::Backend("error".into())),
        },
        ErrorCase {
            dirty: vec![r("k0", ""), r("k1", "v1")],
            snap: vec![r("k0", "x0"), r("k1", "x1")],
            next_times_when_error_happens: 0,
            inject_dirty_error: Some(DriverError::Backend("error".into())),
            inject_snap_error: None,
        },
        ErrorCase {
            dirty: vec![r("k0", ""), r("k1", "v1")],
            snap: vec![r("k0", "x0"), r("k1", "x1")],
            next_times_when_error_happens: 0,
            inject_dirty_error: None,
            inject_snap_error: Some(DriverError::Backend("error".into())),
        },
        ErrorCase {
            dirty: vec![r("k0", "v0"), r("k1", "v1")],
            snap: vec![r("k1", "x1")],
            next_times_when_error_happens: 1,
            inject_dirty_error: Some(DriverError::Backend("error".into())),
            inject_snap_error: None,
        },
        ErrorCase {
            dirty: vec![r("k1", "v1")],
            snap: vec![r("k0", "x0"), r("k1", "x1")],
            next_times_when_error_happens: 1,
            inject_dirty_error: None,
            inject_snap_error: Some(DriverError::Backend("error".into())),
        },
        ErrorCase {
            dirty: vec![r("k0", "v0"), r("k1", "v1")],
            snap: vec![r("k1", "x1")],
            next_times_when_error_happens: 1,
            inject_dirty_error: Some(DriverError::Backend("error".into())),
            inject_snap_error: None,
        },
        ErrorCase {
            dirty: vec![r("k1", "v1")],
            snap: vec![r("k0", "x0"), r("k1", "x1")],
            next_times_when_error_happens: 1,
            inject_dirty_error: None,
            inject_snap_error: Some(DriverError::Backend("error".into())),
        },
    ];

    for case in cases {
        let dirty_closed = Arc::new(AtomicBool::new(false));
        let snap_closed = Arc::new(AtomicBool::new(false));
        let dirty_err = Arc::new(Mutex::new(None));
        let snap_err = Arc::new(Mutex::new(None));
        let dirty_iter =
            TrackedIter::new(case.dirty.clone(), dirty_closed.clone(), dirty_err.clone());
        let snap_iter = TrackedIter::new(case.snap.clone(), snap_closed.clone(), snap_err.clone());

        if case.next_times_when_error_happens > 0 {
            // 先成功创建，前进若干步后再注入错误。
            let mut iter = NewUnionIter(dirty_iter, snap_iter, false).unwrap();
            for _ in 0..case.next_times_when_error_happens.saturating_sub(1) {
                iter.Next().unwrap();
            }
            *dirty_err.lock().unwrap() = case.inject_dirty_error.clone();
            *snap_err.lock().unwrap() = case.inject_snap_error.clone();
            let err = iter.Next().unwrap_err();
            if let Some(expected) = &case.inject_dirty_error {
                assert_eq!(&err, expected);
            }
            if let Some(expected) = &case.inject_snap_error {
                assert_eq!(&err, expected);
            }
            assert!(!dirty_closed.load(Ordering::SeqCst));
            assert!(!snap_closed.load(Ordering::SeqCst));
            assertClose(&mut iter, &dirty_closed, &snap_closed);
        } else {
            *dirty_err.lock().unwrap() = case.inject_dirty_error.clone();
            *snap_err.lock().unwrap() = case.inject_snap_error.clone();
            let err = match NewUnionIter(dirty_iter, snap_iter, false) {
                Ok(_) => panic!("expected NewUnionIter to fail"),
                Err(error) => error,
            };
            if let Some(expected) = &case.inject_dirty_error {
                assert_eq!(&err, expected);
            }
            if let Some(expected) = &case.inject_snap_error {
                assert_eq!(&err, expected);
            }
            assert!(!dirty_closed.load(Ordering::SeqCst));
            assert!(!snap_closed.load(Ordering::SeqCst));
        }
    }
}

/// 正向与反向各跑一遍，并断言 Close 会关闭两侧迭代器。
fn assertUnionIter(dirty: Vec<Entry>, snap: Vec<Entry>, expected: Vec<Entry>) {
    let dirty_closed = Arc::new(AtomicBool::new(false));
    let snap_closed = Arc::new(AtomicBool::new(false));
    let mut iter = NewUnionIter(
        TrackedIter::new(
            dirty.clone(),
            dirty_closed.clone(),
            Arc::new(Mutex::new(None)),
        ),
        TrackedIter::new(
            snap.clone(),
            snap_closed.clone(),
            Arc::new(Mutex::new(None)),
        ),
        false,
    )
    .unwrap();
    assertIter(&mut iter, &expected);
    assertClose(&mut iter, &dirty_closed, &snap_closed);

    let dirty_closed = Arc::new(AtomicBool::new(false));
    let snap_closed = Arc::new(AtomicBool::new(false));
    let mut iter = NewUnionIter(
        TrackedIter::new(
            reverseRecords(dirty),
            dirty_closed.clone(),
            Arc::new(Mutex::new(None)),
        ),
        TrackedIter::new(
            reverseRecords(snap),
            snap_closed.clone(),
            Arc::new(Mutex::new(None)),
        ),
        true,
    )
    .unwrap();
    assertIter(&mut iter, &reverseRecords(expected));
    assertClose(&mut iter, &dirty_closed, &snap_closed);
}

/// 遍历迭代器收集记录并与期望比对。
fn assertIter(iter: &mut UnionIter, expected: &[Entry]) {
    let mut records = Vec::new();
    while iter.Valid() {
        records.push(Entry {
            key: iter.Key(),
            value: iter.Value(),
        });
        iter.Next().unwrap();
    }
    assert_eq!(records.len(), expected.len());
    for (got, want) in records.iter().zip(expected.iter()) {
        assert_eq!(got.key, want.key);
        assert_eq!(got.value, want.value);
    }
}

/// 确认 Close 前未关、Close 后两侧均关闭，且重复 Close 安全。
fn assertClose(iter: &mut UnionIter, dirty_closed: &AtomicBool, snap_closed: &AtomicBool) {
    assert!(!dirty_closed.load(Ordering::SeqCst));
    assert!(!snap_closed.load(Ordering::SeqCst));
    iter.Close();
    assert!(dirty_closed.load(Ordering::SeqCst));
    assert!(snap_closed.load(Ordering::SeqCst));
    iter.Close();
}

/// 反转记录顺序，用于反向扫描期望值。
fn reverseRecords(records: Vec<Entry>) -> Vec<Entry> {
    records.into_iter().rev().collect()
}
