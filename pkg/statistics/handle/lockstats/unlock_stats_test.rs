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

// 统计解锁 SQL 常量契约测试。
//
// 确保 `DeleteLockSQL` 与 Go 侧一致：按精确 table_id 从 `mysql.stats_table_locked` 删除锁定行。

use crate::{
    RemoveLockedPartitions, RemoveLockedTables, RestrictedSQLExecutor, SqlRow, SqlValue,
    StatsError, StatsLockTable,
};
use std::collections::HashMap;

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd)]
enum UnlockCall {
    Apply { id: i64, count: i64, modify: i64 },
    Delete(i64),
}

struct UnlockExecutor {
    locked: Vec<i64>,
    deltas: HashMap<i64, (i64, i64)>,
    calls: Vec<UnlockCall>,
    fail_delta: bool,
    fail_delete: bool,
}

impl Default for UnlockExecutor {
    fn default() -> Self {
        Self {
            locked: Vec::new(),
            deltas: HashMap::new(),
            calls: Vec::new(),
            fail_delta: false,
            fail_delete: false,
        }
    }
}

impl RestrictedSQLExecutor for UnlockExecutor {
    fn ExecRestrictedSQL(
        &mut self,
        _sql: &str,
        _arguments: &[SqlValue],
    ) -> Result<Vec<SqlRow>, StatsError> {
        Ok(Vec::new())
    }

    fn StartTS(&self) -> u64 {
        1000
    }

    fn LockedTableIds(&mut self) -> Result<Vec<i64>, StatsError> {
        Ok(self.locked.clone())
    }

    fn InsertStatsLock(&mut self, _table_id: i64) -> Result<(), StatsError> {
        Ok(())
    }

    fn UpdateStatsMetaVersion(&mut self, _table_id: i64) -> Result<(), StatsError> {
        Ok(())
    }

    fn LockedStatsDelta(&mut self, table_id: i64) -> Result<(i64, i64), StatsError> {
        if self.fail_delta {
            return Err(StatsError("delta failed".into()));
        }
        Ok(self.deltas.get(&table_id).copied().unwrap_or_default())
    }

    fn ApplyStatsDelta(
        &mut self,
        table_id: i64,
        count: i64,
        modify_count: i64,
    ) -> Result<(), StatsError> {
        self.calls.push(UnlockCall::Apply {
            id: table_id,
            count,
            modify: modify_count,
        });
        Ok(())
    }

    fn DeleteStatsLock(&mut self, table_id: i64) -> Result<(), StatsError> {
        self.calls.push(UnlockCall::Delete(table_id));
        if self.fail_delete {
            return Err(StatsError("delete failed".into()));
        }
        Ok(())
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

/// 校验解锁删除语句只针对给定的 table_id，避免误删其他锁定记录。
#[test]
fn canonical_unlock_sql_targets_exact_locked_table_id() {
    assert_eq!(
        crate::DeleteLockSQL,
        "DELETE FROM mysql.stats_table_locked WHERE table_id = %?"
    );
}

#[test]
fn remove_locked_tables_matches_go_delta_reconciliation_and_skips() {
    let mut executor = UnlockExecutor {
        locked: vec![1, 4],
        deltas: HashMap::from([(4, (1, 1))]),
        ..Default::default()
    };
    let tables = HashMap::from([
        (1, table("test.t1", &[(4, "p1")])),
        (2, table("test.t2", &[])),
        (3, table("test.t3", &[])),
    ]);

    let message = RemoveLockedTables(&mut executor, &tables).unwrap();
    assert_eq!(
        message,
        "skip unlocking unlocked tables: test.t2, test.t3, other tables unlocked successfully"
    );
    let mut calls = executor.calls;
    calls.sort();
    assert_eq!(
        calls,
        vec![
            UnlockCall::Apply {
                id: 1,
                count: 0,
                modify: 0,
            },
            UnlockCall::Apply {
                id: 1,
                count: 1,
                modify: 1,
            },
            UnlockCall::Apply {
                id: 4,
                count: 1,
                modify: 1,
            },
            UnlockCall::Delete(1),
            UnlockCall::Delete(4),
        ]
    );
}

#[test]
fn remove_locked_partitions_updates_partition_and_table_or_skips_parent_lock() {
    let mut executor = UnlockExecutor {
        locked: vec![2],
        deltas: HashMap::from([(2, (1, 1))]),
        ..Default::default()
    };
    let message = RemoveLockedPartitions(
        &mut executor,
        1,
        "test.t1",
        &HashMap::from([(2, "p1".into())]),
    )
    .unwrap();
    assert_eq!(message, "");
    assert_eq!(
        executor.calls,
        vec![
            UnlockCall::Apply {
                id: 2,
                count: 1,
                modify: 1,
            },
            UnlockCall::Apply {
                id: 1,
                count: 1,
                modify: 1,
            },
            UnlockCall::Delete(2),
        ]
    );

    let mut parent_locked = UnlockExecutor {
        locked: vec![1],
        ..Default::default()
    };
    let message = RemoveLockedPartitions(
        &mut parent_locked,
        1,
        "test.t1",
        &HashMap::from([(2, "p1".into())]),
    )
    .unwrap();
    assert_eq!(
        message,
        "skip unlocking partitions of locked table: test.t1"
    );
    assert!(parent_locked.calls.is_empty());
}

#[test]
fn remove_locked_tables_propagates_delta_and_delete_errors() {
    let tables = HashMap::from([(1, table("test.t1", &[]))]);
    let mut delta_error = UnlockExecutor {
        locked: vec![1],
        fail_delta: true,
        ..Default::default()
    };
    assert_eq!(
        RemoveLockedTables(&mut delta_error, &tables).unwrap_err(),
        StatsError("delta failed".into())
    );
    assert!(delta_error.calls.is_empty());

    let mut delete_error = UnlockExecutor {
        locked: vec![1],
        fail_delete: true,
        ..Default::default()
    };
    assert_eq!(
        RemoveLockedTables(&mut delete_error, &tables).unwrap_err(),
        StatsError("delete failed".into())
    );
    assert_eq!(
        delete_error.calls,
        vec![
            UnlockCall::Apply {
                id: 1,
                count: 0,
                modify: 0,
            },
            UnlockCall::Delete(1),
        ]
    );
}
