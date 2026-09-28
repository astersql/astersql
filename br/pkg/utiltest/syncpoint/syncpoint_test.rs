// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! syncpoint 单元测试：覆盖乱序命中仍按序执行，以及无活跃序列时忽略注册步。
//! 与 Go `syncpoint_test.go` 场景对齐；全程持 TEST_LOCK 避免 failpoint 全局表竞态。
//! 断言依赖事件通道与原子计数，不依赖墙钟顺序本身。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use crate::{Context, New, Step};

/// Go 包路径前缀；InjectCall 短名在 Rust 侧需拼成完整 failpoint 路径。
const SYNC_SCRIPT_TEST_PATH: &str = "github.com/pingcap/tidb/br/pkg/utiltest/syncpoint";

/// 拼接包前缀与短名，保证与 BeginSeq 注册路径一致。
fn failpoint_path(name: &str) -> String {
    format!("{SYNC_SCRIPT_TEST_PATH}/{name}")
}

/// 乱序并发触发 c/b/a，事件通道仍应按 a→b→c 顺序收到。
/// 超时 Context 防止 worker 卡死拖垮整套测试。
#[test]
fn test_sequence() {
    // 全局锁串行化 failpoint 表，避免与其他 syncpoint 测试交错。
    let _test_guard = crate::TEST_LOCK.lock().unwrap();
    let script = New();
    let (seq_ctx, _cancel) = Context::with_timeout(Duration::from_secs(5));
    // 有界 sync_channel：容量=步数，防止发送端在断言前阻塞。
    let (events_tx, events_rx) = mpsc::sync_channel::<&'static str>(3);

    script.BeginSeq(
        Some(&seq_ctx),
        vec![
            Step(failpoint_path("sync-script-a"), {
                let events_tx = events_tx.clone();
                move || {
                    events_tx
                        .send("a")
                        .expect("event receiver must remain alive")
                }
            }),
            Step(failpoint_path("sync-script-b"), {
                let events_tx = events_tx.clone();
                move || {
                    events_tx
                        .send("b")
                        .expect("event receiver must remain alive")
                }
            }),
            Step(failpoint_path("sync-script-c"), {
                let events_tx = events_tx.clone();
                move || {
                    events_tx
                        .send("c")
                        .expect("event receiver must remain alive")
                }
            }),
        ],
    );
    // 关闭发送端，序列完成后 recv 不会永久挂起。
    drop(events_tx);

    // 故意按 c→b→a 启动，验证 Script 内部 Condvar 重排。
    let workers: Vec<_> = ["sync-script-c", "sync-script-b", "sync-script-a"]
        .into_iter()
        .map(|name| thread::spawn(move || trigger_sync_script_point(name)))
        .collect();

    // 按注册序收取；超时则说明排序/唤醒逻辑回归。
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "a");
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "b");
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(5)).unwrap(), "c");

    script.EndSeq();
    // 收尾 join，确保 inject 线程已退出且无 panic。
    for worker in workers {
        worker.join().expect("syncpoint worker must not panic");
    }
}

/// Go 在匹配步骤时先推进 next，再于锁外执行回调；因此后一步可在前一步结束前启动。
#[test]
fn test_callbacks_can_overlap_after_ordered_release() {
    let _test_guard = crate::TEST_LOCK.lock().unwrap();
    let script = New();
    let (seq_ctx, _cancel) = Context::with_timeout(Duration::from_secs(5));
    let (started_tx, started_rx) = mpsc::sync_channel::<()>(1);
    let (second_started_tx, second_started_rx) = mpsc::sync_channel::<()>(1);
    let release_first = Arc::new(Barrier::new(2));

    script.BeginSeq(
        Some(&seq_ctx),
        vec![
            Step(failpoint_path("sync-script-a"), {
                let release_first = Arc::clone(&release_first);
                move || {
                    started_tx.send(()).unwrap();
                    release_first.wait();
                }
            }),
            Step(failpoint_path("sync-script-b"), {
                move || second_started_tx.send(()).unwrap()
            }),
        ],
    );

    let workers = ["sync-script-b", "sync-script-a"]
        .into_iter()
        .map(|name| thread::spawn(move || trigger_sync_script_point(name)))
        .collect::<Vec<_>>();

    started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("first callback must start");
    second_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second callback must start while first callback is blocked");
    release_first.wait();
    for worker in workers {
        worker.join().expect("syncpoint worker must not panic");
    }
    script.EndSeq();
}

/// EndSeq 后同名 failpoint 仍注册，但无活跃序列时应静默忽略回调。
#[test]
fn test_ignores_registered_steps_without_active_sequence() {
    let _test_guard = crate::TEST_LOCK.lock().unwrap();
    let script = New();
    let hits = Arc::new(AtomicI32::new(0));
    let (seq_ctx, _cancel) = Context::with_timeout(Duration::from_secs(5));
    let callback_hits = Arc::clone(&hits);

    script.BeginSeq(
        Some(&seq_ctx),
        vec![Step(failpoint_path("sync-script-a"), move || {
            callback_hits.fetch_add(1, Ordering::SeqCst);
        })],
    );

    trigger_sync_script_point("sync-script-a");
    script.EndSeq();
    // 活跃期内应恰好命中一次。
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // 序列结束后再 inject：advance 见空 seq，计数不得增加。
    trigger_sync_script_point("sync-script-a");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// 将短名映射到完整路径后 inject；未知名直接 panic 以免静默漏测。
fn trigger_sync_script_point(name: &str) {
    match name {
        "sync-script-a" | "sync-script-b" | "sync-script-c" => {
            astersql_testkit_testfailpoint::inject(&failpoint_path(name));
        }
        _ => panic!("unknown syncpoint test failpoint: {name}"),
    }
}
