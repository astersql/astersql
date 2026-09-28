// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Aster 迁移补充单测：方法归类、nil 接收者、merge/Display 与并发累加。
//
// 在 Go 原有用例之外，验证 `Option` no-op、快照字符串格式以及多线程
// 原子计数不丢更新。

use super::{AccessStats, Requests};
use http::{Method, Request};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

/// 校验 GET/HEAD→get、PUT/POST→put，以及 DELETE 不计入。
#[test]
fn requests_recording_matches_go_method_classification() {
    let stats = AccessStats::default();

    let check_counts = |get, put| {
        assert_eq!(stats.requests.snapshot(), (get, put));
    };

    AccessStats::rec_request(Some(&stats), None::<&Request<()>>);
    check_counts(0, 0);

    let request = Request::builder().method(Method::GET).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_counts(1, 0);
    let request = Request::builder().method(Method::HEAD).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_counts(2, 0);
    let request = Request::builder().method(Method::PUT).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_counts(2, 1);
    let request = Request::builder().method(Method::POST).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_counts(2, 2);
    let request = Request::builder().method(Method::DELETE).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));
    check_counts(2, 2);
}

/// `stats` 为 `None` 时各记录入口应为 no-op（对齐 Go nil 接收者）。
#[test]
fn nil_stats_receiver_is_a_noop() {
    let request = Request::builder().method(Method::GET).body(()).unwrap();

    AccessStats::rec_request(None, Some(&request));
    AccessStats::rec_read(None, 13);
    AccessStats::rec_write(None, 21);
}

/// merge 后 Display 字符串应与 Go 快照格式一致。
#[test]
fn merge_and_display_match_go_snapshots_and_format() {
    let stats = AccessStats::default();
    AccessStats::rec_read(Some(&stats), 13);
    AccessStats::rec_write(Some(&stats), 21);
    let request = Request::builder().method(Method::GET).body(()).unwrap();
    AccessStats::rec_request(Some(&stats), Some(&request));

    let other = AccessStats::default();
    other.requests.get.store(2, Ordering::Relaxed);
    other.requests.put.store(3, Ordering::Relaxed);
    other.traffic.read.store(5, Ordering::Relaxed);
    other.traffic.write.store(8, Ordering::Relaxed);
    stats.merge(&other);

    assert_eq!(
        stats.to_string(),
        "{requests: {get: 3, put: 3}, traffic: {r: 18, w: 29}}"
    );
    assert_eq!(Requests::default().to_string(), "{get: 0, put: 0}");
}

/// 多线程并发更新时原子计数不应丢失。
#[test]
fn concurrent_updates_are_not_lost() {
    const WORKERS: usize = 8;
    const ITERATIONS: usize = 2_000;
    let stats = Arc::new(AccessStats::default());
    let mut workers = Vec::with_capacity(WORKERS);

    for _ in 0..WORKERS {
        let stats = Arc::clone(&stats);
        workers.push(thread::spawn(move || {
            let get = Request::builder().method(Method::GET).body(()).unwrap();
            let post = Request::builder().method(Method::POST).body(()).unwrap();
            for _ in 0..ITERATIONS {
                AccessStats::rec_request(Some(&stats), Some(&get));
                AccessStats::rec_request(Some(&stats), Some(&post));
                AccessStats::rec_read(Some(&stats), 2);
                AccessStats::rec_write(Some(&stats), 3);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    let operations = (WORKERS * ITERATIONS) as u64;
    assert_eq!(stats.requests.get.load(Ordering::Relaxed), operations);
    assert_eq!(stats.requests.put.load(Ordering::Relaxed), operations);
    assert_eq!(stats.traffic.read.load(Ordering::Relaxed), operations * 2);
    assert_eq!(stats.traffic.write.load(Ordering::Relaxed), operations * 3);
}
