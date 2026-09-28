// Copyright 2026 AsterSQL.

// AsterSQL 迁移补充测试：验证延迟唤醒、超时清理、死锁精确匹配与并发语义。

use super::*;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// 构造带指定唤醒延迟（毫秒）的 Manager。
fn manager(delay_ms: i64) -> Manager {
    let mut conf = config::DefaultConf.clone();
    conf.PessimisticTxn.WakeUpDelayDuration = delay_ms;
    NewManager(&conf)
}

/// 最早等待者立即唤醒，较晚者收到延迟唤醒且仍留在队列中。
#[test]
fn wakes_oldest_waiter_and_delays_the_rest() {
    let mgr = manager(20);
    let younger = mgr.NewWaiter(20, 7, 100, Duration::from_secs(1));
    let oldest = mgr.NewWaiter(10, 7, 100, Duration::from_secs(1));

    mgr.WakeUp(7, 88, &[100]);

    let first = oldest.Wait();
    assert_eq!(first.WakeupSleepTime, WakeUpThisWaiter);
    assert_eq!(first.CommitTS, 88);

    let delayed = younger.Wait();
    assert_eq!(delayed.WakeupSleepTime, WakeupDelayTimeout);
    assert_eq!(delayed.CommitTS, 88);
    assert_eq!(mgr.waiter_count(100), 1);
}

/// CleanUp 后 Wait 应超时，且耗时至少覆盖延迟配置，队列清空。
#[test]
fn timeout_and_cleanup_match_go_behavior() {
    let mgr = manager(10);
    let waiter = mgr.NewWaiter(1, 2, 3, Duration::from_millis(15));
    mgr.CleanUp(&waiter);

    let started = Instant::now();
    let result = waiter.Wait();
    assert_eq!(result.WakeupSleepTime, WaitTimeout);
    assert!(result.DeadlockResp.is_none());
    assert!(started.elapsed() >= Duration::from_millis(10));
    assert_eq!(mgr.waiter_count(3), 0);
}

/// 死锁响应只移除匹配的等待者，同键其他等待者保留。
#[test]
fn deadlock_response_removes_only_matching_waiter() {
    let mgr = manager(10);
    let matching = mgr.NewWaiter(3, 4, 99, Duration::from_secs(1));
    let other = mgr.NewWaiter(5, 4, 99, Duration::from_secs(1));
    let response = Arc::new(DeadlockResponse {
        Entry: WaitForEntry {
            Txn: 3,
            WaitForTxn: 4,
            KeyHash: 99,
        },
        DeadlockKeyHash: 30_192,
    });

    mgr.WakeUpForDeadlock(response.clone());
    let result = matching.Wait();

    assert!(Arc::ptr_eq(
        result.DeadlockResp.as_ref().unwrap(),
        &response
    ));
    assert_eq!(result.DeadlockResp.unwrap().DeadlockKeyHash, 30_192);
    assert_eq!(mgr.waiter_count(99), 1);
    mgr.CleanUp(&other);
}

/// 并发多等待者：偶数收 commitTS，奇数收死锁响应。
#[test]
fn concurrent_waiters_receive_commit_or_deadlock() {
    let mgr = Arc::new(manager(5));
    let mut joins = Vec::new();
    for txn in 0..8_u64 {
        let waiter = mgr.NewWaiter(txn, 100, txn * 10, Duration::from_secs(1));
        joins.push(thread::spawn(move || waiter.Wait()));
    }

    for txn in 0..8_u64 {
        if txn % 2 == 0 {
            mgr.WakeUp(100, 199, &[txn * 10]);
        } else {
            mgr.WakeUpForDeadlock(Arc::new(DeadlockResponse {
                Entry: WaitForEntry {
                    Txn: txn,
                    WaitForTxn: 100,
                    KeyHash: txn * 10,
                },
                DeadlockKeyHash: 299,
            }));
        }
    }

    for (txn, join) in joins.into_iter().enumerate() {
        let result = join.join().unwrap();
        if txn % 2 == 0 {
            assert_eq!(result.CommitTS, 199);
        } else {
            assert_eq!(result.DeadlockResp.unwrap().DeadlockKeyHash, 299);
        }
    }
}
