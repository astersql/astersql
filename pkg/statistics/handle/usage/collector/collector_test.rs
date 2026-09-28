// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Same-path Go->Rust mapping for `collector_test.go`. Drives the real
// `NewGlobalCollector` worker against the same SendDelta / SendDeltaSync
// acceptance and flush semantics as Go.
//
// Go `collector_test.go` 同路径映射：对真实 `NewGlobalCollector` worker 驱动
// `SendDelta` / `SendDeltaSync`，校验接受计数与 Close 后 flush 语义。

#![allow(non_snake_case)]

use crate::NewGlobalCollector;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;

/// 单会话多次 SendDelta：Close 后合并值等于被接受次数。
#[test]
fn TestSessionSendDelta() {
    let num = Arc::new(AtomicI64::new(0));
    let merged = Arc::clone(&num);
    let g = NewGlobalCollector(move |delta| {
        merged.fetch_add(delta, Ordering::SeqCst);
    });
    g.StartWorker();
    let mut s = g.SpawnSession();
    let mut expect = 0;
    for _ in 0..256 {
        if s.SendDelta(1) {
            expect += 1;
        }
    }

    g.Close();
    assert_eq!(i64::from(expect), num.load(Ordering::SeqCst));
}

/// 多会话并行 SendDelta：接受计数与合并结果一致（允许通道满丢弃）。
#[test]
fn TestSessionParallelSendDelta() {
    let num = Arc::new(AtomicI64::new(0));
    let expect = Arc::new(AtomicI64::new(0));
    let merged = Arc::clone(&num);
    let g = Arc::new(NewGlobalCollector(move |delta| {
        merged.fetch_add(delta, Ordering::SeqCst);
    }));
    g.StartWorker();
    let session_count = 256;
    let mut workers = Vec::with_capacity(session_count);
    for _ in 0..session_count {
        let mut s = g.SpawnSession();
        let expect = Arc::clone(&expect);
        workers.push(thread::spawn(move || {
            for _ in 0..256 {
                if s.SendDelta(1) {
                    expect.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }

    for worker in workers {
        worker.join().unwrap();
    }
    g.Close();
    assert_eq!(expect.load(Ordering::SeqCst), num.load(Ordering::SeqCst));
}

/// 多会话并行 SendDeltaSync：同步路径应无丢失，合计等于会话数×次数。
#[test]
fn TestSessionParallelSendDeltaSync() {
    let num = Arc::new(AtomicI64::new(0));
    let merged = Arc::clone(&num);
    let g = Arc::new(NewGlobalCollector(move |delta| {
        merged.fetch_add(delta, Ordering::SeqCst);
    }));
    g.StartWorker();
    let session_count = 256;
    let mut workers = Vec::with_capacity(session_count);

    for _ in 0..session_count {
        let mut s = g.SpawnSession();
        workers.push(thread::spawn(move || {
            for _ in 0..256 {
                s.SendDeltaSync(1);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    g.Close();
    assert_eq!((session_count * 256) as i64, num.load(Ordering::SeqCst));
}
