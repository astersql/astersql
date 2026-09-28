// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// SQLKiller 迁移期单元测试：校验首信号胜出、事件广播、Reset 世代、
// 错误映射、Finish 回调替换，以及连接存活检查的节流语义。
//
// 对应 Go `sqlkiller` 包行为，确保 Rust 侧 kill 路径与 TiDB 错误文案一致。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::sqlkiller::{
    ConnectionAliveFn, KilledByMemArbitrator, MaxExecTimeExceeded, QueryInterrupted,
    QueryMemoryExceeded, RunawayQueryExceeded, SQLKiller, ServerMemoryExceeded,
};

/// 首次 kill 信号写入后不可被后续信号覆盖，且等待方会被唤醒。
#[test]
fn first_signal_wins_and_kill_event_is_broadcast() {
    let killer = SQLKiller::default();
    let event = killer.GetKillEventChan();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let waiters = (0..4)
        .map(|_| {
            let event = event.clone();
            let ready_tx = ready_tx.clone();
            thread::spawn(move || {
                ready_tx.send(()).expect("report waiter readiness");
                event.wait_timeout(Duration::from_secs(30))
            })
        })
        .collect::<Vec<_>>();
    drop(ready_tx);
    for _ in 0..waiters.len() {
        ready_rx.recv().expect("waiter thread started");
    }

    killer.SendKillSignal(QueryInterrupted);
    killer.SendKillSignal(MaxExecTimeExceeded);

    for waiter in waiters {
        assert!(waiter.join().unwrap(), "kill event must wake every waiter");
    }
    assert!(event.is_closed());
    assert_eq!(killer.GetKillSignal(), QueryInterrupted);
    assert!(killer.GetKillEventChan().is_closed());
}

/// Reset 关闭旧事件通道并开启新世代；新通道可再次携带 kill 原因。
#[test]
fn reset_closes_old_event_and_starts_a_fresh_generation() {
    let killer = SQLKiller::default();
    let old_event = killer.GetKillEventChan();

    killer.Reset();

    assert!(old_event.is_closed(), "old waiters must not be stranded");
    assert_eq!(killer.GetKillSignal(), 0);
    let new_event = killer.GetKillEventChan();
    assert!(!new_event.is_closed());

    killer.SendKillSignalWithKillEventReason(KilledByMemArbitrator, "quota pressure".into());
    assert!(new_event.is_closed());
    let err = killer.HandleSignal().unwrap_err();
    assert!(err.to_string().contains("quota pressure"));
}

/// 各 Go 侧 kill 信号常量映射到真实 TiDB 错误文案。
#[test]
fn all_go_kill_signals_map_to_real_tidb_errors() {
    let cases = [
        (QueryInterrupted, "Query execution was interrupted"),
        (
            MaxExecTimeExceeded,
            "maximum statement execution time exceeded",
        ),
        (
            QueryMemoryExceeded,
            "allowed memory limit for a single SQL query",
        ),
        (
            ServerMemoryExceeded,
            "allowed memory limit for the tidb-server instance",
        ),
        (RunawayQueryExceeded, "runaway exceed tidb side"),
    ];

    for (signal, message) in cases {
        let killer = SQLKiller::default();
        killer.ConnID.store(42, Ordering::SeqCst);
        killer.SendKillSignal(signal);
        let err = killer.HandleSignal().unwrap_err();
        assert!(
            err.to_string().contains(message),
            "signal {signal} produced unexpected error: {err}"
        );
    }
}

/// Finish 回调可设置、触发一次，清除后再调用不再执行。
#[test]
fn finish_callback_can_be_replaced_and_cleared() {
    let killer = SQLKiller::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = Arc::clone(&calls);
    killer.SetFinishFunc(Box::new(move || {
        callback_calls.fetch_add(1, Ordering::SeqCst);
    }));

    killer.FinishResultSet();
    killer.ClearFinishFunc();
    killer.FinishResultSet();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// Go mutex 在 Finish panic 后仍可复用；Rust 侧不得因 poisoning 永久损坏 SQLKiller。
#[test]
fn finish_callback_panic_does_not_poison_future_finish_operations() {
    let killer = SQLKiller::default();
    killer.SetFinishFunc(Box::new(|| panic!("finish failed")));
    assert!(catch_unwind(AssertUnwindSafe(|| killer.FinishResultSet())).is_err());

    let calls = Arc::new(AtomicUsize::new(0));
    let callback_calls = Arc::clone(&calls);
    killer.SetFinishFunc(Box::new(move || {
        callback_calls.fetch_add(1, Ordering::SeqCst);
    }));
    killer.FinishResultSet();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// 连接存活检查：节流路径首次只记时，之后与即时 Check 路径行为一致。
#[test]
fn connection_liveness_matches_immediate_and_throttled_go_paths() {
    let killer = SQLKiller::default();
    let checks = Arc::new(AtomicUsize::new(0));
    let callback_checks = Arc::clone(&checks);
    let callback: ConnectionAliveFn = Arc::new(move || {
        callback_checks.fetch_add(1, Ordering::SeqCst);
        false
    });
    killer.IsConnectionAlive.Store(Some(callback));

    killer.HandleSignal().unwrap();
    assert_eq!(
        checks.load(Ordering::SeqCst),
        0,
        "first throttled call only records time"
    );
    thread::sleep(Duration::from_millis(5));
    assert!(killer.HandleSignal().is_err());
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    assert_eq!(killer.GetKillSignal(), QueryInterrupted);

    killer.Reset();
    killer.CheckConnectionAlive();
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert_eq!(killer.GetKillSignal(), QueryInterrupted);
}
