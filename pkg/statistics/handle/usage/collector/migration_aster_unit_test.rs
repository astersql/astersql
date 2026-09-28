// Copyright 2024 PingCAP, Inc.
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

// 收集器迁移期补充单元测试。
//
// 通过 `#[path]` 直接挂载 `collector.rs`，覆盖与 Go 对齐的接受/flush 语义，
// 以及 Close 解除阻塞同步发送方的行为。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

#[path = "collector.rs"]
mod collector;

use collector::NewGlobalCollector;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::thread;

/// 单会话 SendDelta：Close flush 后合并值等于接受次数。
#[test]
fn session_send_delta_matches_go_and_close_flushes_accepted_data() {
    let merged = Arc::new(AtomicI64::new(0));
    let merged_for_fn = Arc::clone(&merged);
    let global = NewGlobalCollector(move |delta| {
        merged_for_fn.fetch_add(delta, Ordering::SeqCst);
    });
    global.StartWorker();

    let mut session = global.SpawnSession();
    let mut expected = 0;
    for _ in 0..256 {
        if session.SendDelta(1) {
            expected += 1;
        }
    }

    global.Close();
    assert_eq!(i64::from(expected), merged.load(Ordering::SeqCst));
}

/// 并行 SendDelta：接受计数与合并结果一致。
#[test]
fn parallel_send_delta_matches_go_accepted_count() {
    let merged = Arc::new(AtomicI64::new(0));
    let expected = Arc::new(AtomicI64::new(0));
    let merged_for_fn = Arc::clone(&merged);
    let global = Arc::new(NewGlobalCollector(move |delta| {
        merged_for_fn.fetch_add(delta, Ordering::SeqCst);
    }));
    global.StartWorker();

    let mut workers = Vec::with_capacity(256);
    for _ in 0..256 {
        let mut session = global.SpawnSession();
        let expected = Arc::clone(&expected);
        workers.push(thread::spawn(move || {
            for _ in 0..256 {
                if session.SendDelta(1) {
                    expected.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    global.Close();
    assert_eq!(
        expected.load(Ordering::SeqCst),
        merged.load(Ordering::SeqCst)
    );
}

/// 并行 SendDeltaSync：同步路径无丢失。
#[test]
fn parallel_send_delta_sync_matches_go_without_loss() {
    let merged = Arc::new(AtomicI64::new(0));
    let merged_for_fn = Arc::clone(&merged);
    let global = Arc::new(NewGlobalCollector(move |delta| {
        merged_for_fn.fetch_add(delta, Ordering::SeqCst);
    }));
    global.StartWorker();

    let mut workers = Vec::with_capacity(256);
    for _ in 0..256 {
        let mut session = global.SpawnSession();
        workers.push(thread::spawn(move || {
            for _ in 0..256 {
                assert!(session.SendDeltaSync(1));
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }

    global.Close();
    assert_eq!(256 * 256, merged.load(Ordering::SeqCst));
}

/// 填满通道后阻塞的 SendDeltaSync，应在 Close 时被唤醒并返回失败。
#[test]
fn close_unblocks_a_synchronous_sender_and_reports_failure() {
    let global = NewGlobalCollector(|_: i32| {});
    let mut session = global.SpawnSession();
    // 不启 worker，填满高优先级通道后下一发将阻塞
    for value in 0..collector::DEFAULT_CHANNEL_SIZE {
        assert!(session.SendDeltaSync(value as i32));
    }

    let (ready_sender, ready_receiver) = mpsc::channel();
    let blocked_sender = thread::spawn(move || {
        ready_sender.send(()).unwrap();
        session.SendDeltaSync(99)
    });
    ready_receiver.recv().unwrap();
    global.Close();

    assert!(!blocked_sender.join().unwrap());
}
