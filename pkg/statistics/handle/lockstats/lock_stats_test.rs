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

// 统计锁跳过消息稳定性单元测试。
//
// 验证表/分区名称排序与「部分成功」文案与 Go 侧规范字符串一致。

use crate::{
    AddLockedPartitions, AddLockedTables, InsertLockAndUpdateVersion, RestrictedSQLExecutor,
    SessionRef, SqlRow, SqlValue, StatsError, StatsLockTable, StatsSession,
    generateStableSkippedPartitionsMessage, generateStableSkippedTablesMessage,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Call {
    Insert(i64),
    UpdateVersion(i64),
}

#[derive(Default)]
struct RecordingExecutor {
    locked: Vec<i64>,
    calls: Vec<Call>,
    start_ts: u64,
    fail_insert: bool,
    fail_update: bool,
}

impl RestrictedSQLExecutor for RecordingExecutor {
    fn ExecRestrictedSQL(
        &mut self,
        _sql: &str,
        _arguments: &[SqlValue],
    ) -> Result<Vec<SqlRow>, StatsError> {
        Ok(Vec::new())
    }

    fn StartTS(&self) -> u64 {
        self.start_ts
    }

    fn LockedTableIds(&mut self) -> Result<Vec<i64>, StatsError> {
        Ok(self.locked.clone())
    }

    fn InsertStatsLock(&mut self, table_id: i64) -> Result<(), StatsError> {
        self.calls.push(Call::Insert(table_id));
        if self.fail_insert {
            return Err(StatsError("insert failed".into()));
        }
        Ok(())
    }

    fn UpdateStatsMetaVersion(&mut self, table_id: i64) -> Result<(), StatsError> {
        self.calls.push(Call::UpdateVersion(table_id));
        if self.fail_update {
            return Err(StatsError("update failed".into()));
        }
        Ok(())
    }

    fn LockedStatsDelta(&mut self, _table_id: i64) -> Result<(i64, i64), StatsError> {
        Ok((0, 0))
    }

    fn ApplyStatsDelta(
        &mut self,
        _table_id: i64,
        _count: i64,
        _modify_count: i64,
    ) -> Result<(), StatsError> {
        Ok(())
    }

    fn DeleteStatsLock(&mut self, _table_id: i64) -> Result<(), StatsError> {
        Ok(())
    }
}

struct RecordingSession {
    executor: Mutex<RecordingExecutor>,
    wraps: Mutex<Vec<bool>>,
}

impl RecordingSession {
    fn new(executor: RecordingExecutor) -> Arc<Self> {
        Arc::new(Self {
            executor: Mutex::new(executor),
            wraps: Mutex::new(Vec::new()),
        })
    }
}

impl StatsSession for RecordingSession {
    fn WithSession(
        &self,
        wrap_transaction: bool,
        callback: &mut dyn FnMut(&mut dyn RestrictedSQLExecutor) -> Result<(), StatsError>,
    ) -> Result<(), StatsError> {
        self.wraps.lock().unwrap().push(wrap_transaction);
        callback(&mut *self.executor.lock().unwrap())
    }
}

fn table(name: &str, partitions: &[(i64, &str)]) -> StatsLockTable {
    StatsLockTable {
        FullName: name.into(),
        PartitionInfo: partitions
            .iter()
            .map(|(id, name)| (*id, (*name).into()))
            .collect(),
    }
}

/// 乱序输入名称时应排序，并拼接 other … successfully 的部分成功提示。
#[test]
fn canonical_lock_messages_are_stable_and_report_partial_success() {
    assert_eq!(
        generateStableSkippedTablesMessage(
            3,
            vec!["db.t2".into(), "db.t1".into()],
            "locking",
            "locked",
        ),
        "skip locking locked tables: db.t1, db.t2, other tables locked successfully"
    );
    assert_eq!(
        generateStableSkippedPartitionsMessage(
            &[1, 2],
            "db.t",
            vec!["p1".into()],
            "locking",
            "locked",
        ),
        "skip locking locked partitions of table db.t: p1, other partitions locked successfully"
    );
}

#[test]
fn lock_message_cases_match_go_for_empty_single_all_and_unlock() {
    assert_eq!(
        generateStableSkippedTablesMessage(3, vec![], "locking", "locked"),
        ""
    );
    assert_eq!(
        generateStableSkippedTablesMessage(1, vec!["t1".into()], "locking", "locked"),
        "skip locking locked table: t1"
    );
    assert_eq!(
        generateStableSkippedTablesMessage(
            4,
            vec!["t4".into(), "t2".into(), "t3".into(), "t1".into()],
            "locking",
            "locked",
        ),
        "skip locking locked tables: t1, t2, t3, t4"
    );
    assert_eq!(
        generateStableSkippedTablesMessage(
            4,
            vec!["t4".into(), "t2".into(), "t3".into(), "t1".into()],
            "unlocking",
            "unlocked",
        ),
        "skip unlocking unlocked tables: t1, t2, t3, t4"
    );
}

#[test]
fn lock_message_partition_cases_match_go() {
    assert_eq!(
        generateStableSkippedPartitionsMessage(&[1, 2, 3], "test.t", vec![], "locking", "locked"),
        ""
    );
    assert_eq!(
        generateStableSkippedPartitionsMessage(
            &[1],
            "test.t",
            vec!["p1".into()],
            "locking",
            "locked"
        ),
        "skip locking locked partition of table test.t: p1"
    );
    assert_eq!(
        generateStableSkippedPartitionsMessage(
            &[1, 2, 3, 4],
            "test.t",
            vec!["p4".into(), "p2".into(), "p3".into()],
            "locking",
            "locked",
        ),
        "skip locking locked partitions of table test.t: p2, p3, p4, other partitions locked successfully"
    );
    assert_eq!(
        generateStableSkippedPartitionsMessage(
            &[1, 2, 3, 4],
            "test.t",
            vec!["p4".into(), "p2".into(), "p3".into(), "p1".into()],
            "unlocking",
            "unlocked",
        ),
        "skip unlocking unlocked partitions of table test.t: p1, p2, p3, p4"
    );
}

#[test]
fn add_locked_tables_matches_go_skip_and_lock_side_effects() {
    let mut executor = RecordingExecutor {
        locked: vec![1],
        start_ts: 1000,
        ..Default::default()
    };
    let tables = HashMap::from([
        (1, table("test.t1", &[(4, "p1")])),
        (2, table("test.t2", &[])),
        (3, table("test.t3", &[])),
    ]);

    let message = AddLockedTables(&mut executor, &tables).unwrap();
    assert_eq!(
        message,
        "skip locking locked tables: test.t1, other tables locked successfully"
    );
    let mut calls = executor.calls;
    calls.sort();
    assert_eq!(
        calls,
        vec![
            Call::Insert(2),
            Call::Insert(3),
            Call::Insert(4),
            Call::UpdateVersion(2),
            Call::UpdateVersion(3),
            Call::UpdateVersion(4),
        ]
    );
}

#[test]
fn add_locked_partitions_matches_go_and_skips_when_table_is_locked() {
    let mut executor = RecordingExecutor {
        start_ts: 1000,
        ..Default::default()
    };
    let message = AddLockedPartitions(
        &mut executor,
        1,
        "test.t1",
        &HashMap::from([(2, "p1".into()), (3, "p2".into())]),
    )
    .unwrap();
    assert_eq!(message, "");
    assert_eq!(executor.calls.len(), 4);

    let mut locked_executor = RecordingExecutor {
        locked: vec![1],
        ..Default::default()
    };
    let message = AddLockedPartitions(
        &mut locked_executor,
        1,
        "test.t1",
        &HashMap::from([(2, "p1".into()), (3, "p2".into())]),
    )
    .unwrap();
    assert_eq!(message, "skip locking partitions of locked table: test.t1");
    assert!(locked_executor.calls.is_empty());
}

#[test]
fn insert_lock_stops_on_the_first_error() {
    let mut insert_error = RecordingExecutor {
        fail_insert: true,
        ..Default::default()
    };
    assert_eq!(
        InsertLockAndUpdateVersion(&mut insert_error, 1).unwrap_err(),
        StatsError("insert failed".into())
    );
    assert_eq!(insert_error.calls, vec![Call::Insert(1)]);

    let mut update_error = RecordingExecutor {
        fail_update: true,
        ..Default::default()
    };
    assert_eq!(
        InsertLockAndUpdateVersion(&mut update_error, 1).unwrap_err(),
        StatsError("update failed".into())
    );
    assert_eq!(
        update_error.calls,
        vec![Call::Insert(1), Call::UpdateVersion(1)]
    );
}

#[test]
fn public_lock_methods_wrap_all_mutations_in_a_transaction() {
    let session = RecordingSession::new(RecordingExecutor {
        locked: vec![1],
        ..Default::default()
    });
    let pool: SessionRef = session.clone();
    let lock = crate::NewStatsLock(pool);
    let tables = HashMap::from([(1, table("test.t1", &[]))]);

    assert_eq!(
        lock.LockTables(&tables).unwrap(),
        "skip locking locked table: test.t1"
    );
    assert_eq!(
        lock.LockPartitions(1, "test.t1", &HashMap::new()).unwrap(),
        "skip locking partitions of locked table: test.t1"
    );
    assert_eq!(lock.RemoveLockedTables(&tables).unwrap(), "");
    assert_eq!(
        lock.RemoveLockedPartitions(1, "test.t1", &HashMap::new())
            .unwrap(),
        "skip unlocking partitions of locked table: test.t1"
    );
    assert_eq!(*session.wraps.lock().unwrap(), vec![true, true, true, true]);
}
