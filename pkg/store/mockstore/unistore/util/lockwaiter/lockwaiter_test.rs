// Copyright 2019-present PingCAP, Inc.
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

// lockwaiter 单元测试：覆盖注册、最早等待者唤醒、死锁唤醒与并发场景。

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use super::*;

/// 基本流程：新建等待者、按 startTS 取最早者、WakeUp 与 WakeUpForDeadlock。
#[test]
fn TestLockwaiterBasic() {
    let mgr = NewManager(&config::DefaultConf);
    let key_hash = 100_u64;
    mgr.NewWaiter(1, 2, key_hash, Duration::from_nanos(10));

    // 校验队列已创建且等待者字段正确。
    {
        let queues = mgr.waitingQueues.lock().unwrap();
        let queue = queues.get(&key_hash).expect("queue must be created");
        let waiter = &queue.waiters[0];
        assert_eq!(waiter.startTS, 1);
        assert_eq!(waiter.LockTS, 2);
        assert_eq!(waiter.KeyHash, key_hash);
    }

    // get_oldest_waiter 应按 startTS 取出并移出队列。
    {
        let mut queues = mgr.waitingQueues.lock().unwrap();
        let ready = queues
            .get_mut(&key_hash)
            .expect("queue must exist")
            .get_oldest_waiter();
        assert_eq!(ready.startTS, 1);
        assert_eq!(ready.LockTS, 2);
        assert_eq!(ready.KeyHash, key_hash);
    }

    // 正常唤醒：应收到 commitTS，且队列清空。
    let waiter = mgr.NewWaiter(3, 2, key_hash, Duration::from_secs(1));
    mgr.WakeUp(2, 222, &[key_hash]);
    let result = waiter.receiver.recv().unwrap();
    assert_eq!(result.CommitTS, 222);
    assert_eq!(mgr.waiter_count(key_hash), 0);
    assert!(!mgr.waitingQueues.lock().unwrap().contains_key(&key_hash));

    // 死锁唤醒：应携带 DeadlockResponse 且队列清空。
    let waiter = mgr.NewWaiter(3, 4, key_hash, Duration::from_secs(1));
    let response = Arc::new(DeadlockResponse {
        Entry: WaitForEntry {
            Txn: 3,
            WaitForTxn: 4,
            KeyHash: key_hash,
        },
        DeadlockKeyHash: 30192,
    });
    mgr.WakeUpForDeadlock(response);
    let result = waiter.receiver.recv().unwrap();
    let deadlock = result.DeadlockResp.expect("deadlock response must be set");
    assert_eq!(deadlock.Entry.Txn, 3);
    assert_eq!(deadlock.Entry.WaitForTxn, 4);
    assert_eq!(deadlock.Entry.KeyHash, key_hash);
    assert_eq!(deadlock.DeadlockKeyHash, 30192);
    assert!(!mgr.waitingQueues.lock().unwrap().contains_key(&key_hash));
}

/// 并发：偶数事务期望提交唤醒，奇数期望死锁唤醒，最后一号 CleanUp 后超时。
#[test]
fn TestLockwaiterConcurrent() {
    let mgr = Arc::new(NewManager(&config::DefaultConf));
    let wait_for_txn = 100_u64;
    let commit_ts = 199_u64;
    let deadlock_key_hash = 299_u64;
    let numbers = 10_u64;
    let (ready_tx, ready_rx) = mpsc::channel();
    let mut handles = Vec::with_capacity(numbers as usize);

    // 启动多个等待线程；最后一个先 CleanUp，应走超时路径。
    for number in 0..numbers {
        let mgr = Arc::clone(&mgr);
        let ready_tx = ready_tx.clone();
        handles.push(thread::spawn(move || {
            let waiter = mgr.NewWaiter(
                number,
                wait_for_txn,
                number * 10,
                Duration::from_millis(100),
            );
            if number == numbers - 1 {
                mgr.CleanUp(&waiter);
            }
            ready_tx.send(()).unwrap();

            let result = waiter.Wait();
            if number == numbers - 1 {
                assert_eq!(result.WakeupSleepTime, WaitTimeout);
                assert_eq!(result.CommitTS, 0);
                assert!(result.DeadlockResp.is_none());
            } else if number % 2 == 0 {
                assert_eq!(result.CommitTS, commit_ts);
            } else {
                let deadlock = result
                    .DeadlockResp
                    .expect("odd waiter needs deadlock result");
                assert_eq!(deadlock.DeadlockKeyHash, deadlock_key_hash);
            }
        }));
    }
    drop(ready_tx);
    // 等待所有线程完成注册后再统一唤醒。
    for _ in 0..numbers {
        ready_rx.recv().unwrap();
    }

    for number in 0..numbers {
        if number % 2 == 0 {
            mgr.WakeUp(wait_for_txn, commit_ts, &[number * 10]);
        } else {
            mgr.WakeUpForDeadlock(Arc::new(DeadlockResponse {
                Entry: WaitForEntry {
                    Txn: number,
                    WaitForTxn: wait_for_txn,
                    KeyHash: number * 10,
                },
                DeadlockKeyHash: deadlock_key_hash,
            }));
        }
    }

    for handle in handles {
        handle.join().unwrap();
    }
}
