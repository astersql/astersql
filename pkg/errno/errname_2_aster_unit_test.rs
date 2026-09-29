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

// errname / infoschema 迁移单元测试。
//
// 覆盖：
// - `MySQLErrName` 表规模、抽样消息原文与脱敏下标；
// - 错误统计（global / user / host）深拷贝与 Flush 重置；
// - 并发 `IncrementError` 不丢计数。
//
// 统计测试通过进程级互斥锁串行化，避免与 `infoschema_test` 共享可变全局状态时互相干扰。

use super::{errcode::*, errname::MySQLErrName, infoschema::*};
use std::sync::{Mutex, MutexGuard};

#[test]
fn go_merge_3_shared_lock_lost_error_matches_go() {
    assert_eq!(ErrSharedLockLost, 9015);
    let message = &MySQLErrName[&ErrSharedLockLost];
    assert_eq!(
        message.Raw,
        "Shared lock was lost during lock upgrade; transaction cannot continue, txnStartTS=%d, key=%s"
    );
    assert_eq!(message.RedactArgPos, vec![1]);
}

/// 保护 infoschema 全局统计的测试互斥锁。
static STATS_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 获取统计测试锁；若锁被毒化则恢复并继续（测试隔离优先）。
pub(crate) fn lock_stats_test() -> MutexGuard<'static, ()> {
    STATS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 校验错误名表非空截断，并抽查 Raw 文案与 WriteConflict 脱敏位置。
#[test]
fn mysql_error_names_cover_errno_codes_and_redaction_metadata() {
    let names = &*MySQLErrName;

    assert!(names.len() > 1_000, "the errno table must not be truncated");
    assert_eq!(names[&ErrNoDB].Raw, "No database selected");
    assert_eq!(
        names[&ErrDupEntry].Raw,
        "Duplicate entry '%-.64s' for key '%-.192s'"
    );
    assert_eq!(names[&ErrWriteConflict].RedactArgPos, vec![3, 4, 5, 6]);
    assert_eq!(
        names[&ErrUserPrefixMismatch].Raw,
        "User name prefix does not match the assigned keyspace."
    );
}

/// 快照为深拷贝：后续增量不影响已取出的 map；FlushStats 清空三个维度。
#[test]
fn statistics_snapshots_are_deep_copies_and_flush_resets_all_dimensions() {
    let _guard = lock_stats_test();
    FlushStats();
    IncrementError(123, "user", "host");
    IncrementWarning(123, "user", "host");

    let global_copy = GlobalStats();
    let user_copy = UserStats();
    let host_copy = HostStats();

    // 快照之后继续增量，copy 应保持旧值。
    IncrementError(123, "user", "host");
    IncrementWarning(999, "later", "new-host");

    assert_eq!(global_copy[&123].ErrorCount, 1);
    assert_eq!(global_copy[&123].WarningCount, 1);
    assert_eq!(user_copy["user"][&123].ErrorCount, 1);
    assert_eq!(host_copy["host"][&123].WarningCount, 1);
    assert!(!user_copy.contains_key("later"));
    assert!(!host_copy.contains_key("new-host"));
    assert_eq!(GlobalStats()[&123].ErrorCount, 2);

    FlushStats();
    assert!(GlobalStats().is_empty());
    assert!(UserStats().is_empty());
    assert!(HostStats().is_empty());
}

/// 多线程并发 IncrementError 后，三维度计数均等于总增量次数。
#[test]
fn concurrent_increments_are_not_lost() {
    let _guard = lock_stats_test();
    FlushStats();
    let workers: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..250 {
                    IncrementError(321, "concurrent-user", "concurrent-host");
                }
            })
        })
        .collect();

    for worker in workers {
        worker.join().expect("increment worker must not panic");
    }

    assert_eq!(GlobalStats()[&321].ErrorCount, 2_000);
    assert_eq!(UserStats()["concurrent-user"][&321].ErrorCount, 2_000);
    assert_eq!(HostStats()["concurrent-host"][&321].ErrorCount, 2_000);
}
