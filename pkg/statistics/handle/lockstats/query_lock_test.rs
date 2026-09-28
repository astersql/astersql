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

// 查询已锁定统计表过滤逻辑的单元测试。
//
// 覆盖 `GetLockedTables`：从全局锁定集合中筛出请求 ID 里实际已锁定的表/分区。
// 「锁定统计」指禁止对该表自动或手动更新统计信息（stats），避免分析任务干扰。

use crate::{
    GetLockedTables, QueryLockedTables, RestrictedSQLExecutor, SessionRef, SqlRow, SqlValue,
    StatsError, StatsSession,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

struct QueryExecutor {
    ids: Vec<i64>,
    error: bool,
}

impl RestrictedSQLExecutor for QueryExecutor {
    fn ExecRestrictedSQL(
        &mut self,
        _sql: &str,
        _arguments: &[SqlValue],
    ) -> Result<Vec<SqlRow>, StatsError> {
        Ok(Vec::new())
    }

    fn StartTS(&self) -> u64 {
        0
    }

    fn LockedTableIds(&mut self) -> Result<Vec<i64>, StatsError> {
        if self.error {
            Err(StatsError("query failed".into()))
        } else {
            Ok(self.ids.clone())
        }
    }

    fn InsertStatsLock(&mut self, _table_id: i64) -> Result<(), StatsError> {
        Ok(())
    }

    fn UpdateStatsMetaVersion(&mut self, _table_id: i64) -> Result<(), StatsError> {
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

struct QuerySession {
    executor: Mutex<QueryExecutor>,
}

impl StatsSession for QuerySession {
    fn WithSession(
        &self,
        wrap_transaction: bool,
        callback: &mut dyn FnMut(&mut dyn RestrictedSQLExecutor) -> Result<(), StatsError>,
    ) -> Result<(), StatsError> {
        assert!(!wrap_transaction);
        callback(&mut *self.executor.lock().unwrap())
    }
}

/// 验证仅返回请求列表中、且确实处于锁定状态的 table_id。
#[test]
fn canonical_locked_table_filter_returns_only_requested_locked_ids() {
    // 模拟已锁定的表 ID 集合：1、3、5
    let locked = HashMap::from([(1, ()), (3, ()), (5, ())]);
    // 请求 2/3/5/8：仅 3、5 既在请求中又已锁定
    assert_eq!(
        GetLockedTables(&locked, &[2, 3, 5, 8]),
        HashMap::from([(3, ()), (5, ())])
    );
    assert!(GetLockedTables(&HashMap::new(), &[1, 2, 3]).is_empty());
    assert!(GetLockedTables(&locked, &[]).is_empty());
}

#[test]
fn query_locked_tables_returns_all_ids_and_propagates_errors() {
    let session: SessionRef = Arc::new(QuerySession {
        executor: Mutex::new(QueryExecutor {
            ids: vec![1, 2],
            error: false,
        }),
    });
    assert_eq!(
        QueryLockedTables(&session).unwrap(),
        HashMap::from([(1, ()), (2, ())])
    );

    let session: SessionRef = Arc::new(QuerySession {
        executor: Mutex::new(QueryExecutor {
            ids: Vec::new(),
            error: true,
        }),
    });
    assert_eq!(
        QueryLockedTables(&session).unwrap_err(),
        StatsError("query failed".into())
    );
}

#[test]
fn query_locked_tables_deduplicates_storage_rows() {
    let session: SessionRef = Arc::new(QuerySession {
        executor: Mutex::new(QueryExecutor {
            ids: vec![7, 7, 8],
            error: false,
        }),
    });
    assert_eq!(
        QueryLockedTables(&session).unwrap(),
        HashMap::from([(7, ()), (8, ())])
    );
}

#[test]
fn query_locked_tables_returns_empty_map_for_empty_storage() {
    let session: SessionRef = Arc::new(QuerySession {
        executor: Mutex::new(QueryExecutor {
            ids: Vec::new(),
            error: false,
        }),
    });
    assert!(QueryLockedTables(&session).unwrap().is_empty());
}
