// Copyright 2021 PingCAP, Inc.
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

// 本 crate 的 Wait 使用同步 channel 超时。下方用 Weak 验证等待者、队列及通知
// 负载的释放；并发测试另行 join 全部线程。

use crate::*;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn waiter_resources_are_released_after_completion_and_manager_drop() {
    let manager = NewManager(&config::DefaultConf);
    for completion in 0..3 {
        let waiter = manager.NewWaiter(1, 2, 3, Duration::ZERO);
        let weak = Arc::downgrade(&waiter);
        match completion {
            0 => {
                assert_eq!(waiter.Wait().WakeupSleepTime, WaitTimeout);
                manager.CleanUp(&waiter);
            }
            1 => manager.WakeUp(2, 4, &[3]),
            _ => manager.CleanUp(&waiter),
        }
        assert_eq!(manager.waiter_count(3), 0);
        drop(waiter);
        assert!(
            weak.upgrade().is_none(),
            "completion {completion} retained waiter"
        );
    }

    let pending = manager.NewWaiter(1, 2, 3, Duration::from_secs(60));
    let weak = Arc::downgrade(&pending);
    drop(pending);
    assert!(weak.upgrade().is_some(), "queue must own pending waiter");
    drop(manager);
    assert!(weak.upgrade().is_none(), "manager retained pending waiter");
}

#[test]
fn unread_deadlock_payload_is_released_by_cleanup_or_waiter_drop() {
    for cleanup in [false, true] {
        let manager = NewManager(&config::DefaultConf);
        let waiter = manager.NewWaiter(1, 2, 3, Duration::from_secs(60));
        let weak_waiter = Arc::downgrade(&waiter);
        let response = Arc::new(DeadlockResponse {
            Entry: WaitForEntry {
                Txn: 1,
                WaitForTxn: 2,
                KeyHash: 3,
            },
            DeadlockKeyHash: 4,
        });
        let weak_response = Arc::downgrade(&response);
        manager.WakeUpForDeadlock(response);
        assert_eq!(manager.waiter_count(3), 0);
        assert!(
            weak_response.upgrade().is_some(),
            "notification must own payload"
        );
        if cleanup {
            manager.CleanUp(&waiter);
            assert!(
                weak_response.upgrade().is_none(),
                "cleanup did not drain payload"
            );
        }
        drop(waiter);
        assert!(weak_waiter.upgrade().is_none());
        assert!(
            weak_response.upgrade().is_none(),
            "channel retained payload"
        );
    }
}
