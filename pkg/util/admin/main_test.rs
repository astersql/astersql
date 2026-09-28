// Copyright 2021 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// CheckIndicesCount 单元测试：计数方向、下标与会话标志恢复。
//
// 用队列模拟受限 SQL 返回值，验证 snapshot 选择与 invisible 索引标志还原。

use std::collections::VecDeque;
use std::sync::Mutex;

use super::*;

/// 按入队顺序吐出 COUNT 结果，并记录所用 snapshot。
#[derive(Default)]
struct QueueExecutor {
    rows: Mutex<VecDeque<Result<Vec<CountRow>, AdminError>>>,
    snapshots: Mutex<Vec<u64>>,
}

impl RestrictedSqlExecutor for QueueExecutor {
    fn exec_restricted_sql(
        &self,
        snapshot: u64,
        _sql: &str,
        _args: &[String],
    ) -> Result<Vec<CountRow>, AdminError> {
        self.snapshots.lock().unwrap().push(snapshot);
        self.rows.lock().unwrap().pop_front().unwrap()
    }
}

/// 可配置 invisible/txn/snapshot 的测试会话。
struct TestSession {
    invisible: bool,
    txn_ts: Option<u64>,
    snapshot: u64,
    exec: QueueExecutor,
}

impl SessionContext for TestSession {
    fn optimizer_use_invisible_indexes(&self) -> bool {
        self.invisible
    }

    fn set_optimizer_use_invisible_indexes(&mut self, enabled: bool) {
        self.invisible = enabled;
    }

    fn transaction_start_ts(&self) -> Result<Option<u64>, AdminError> {
        Ok(self.txn_ts)
    }

    fn snapshot_ts(&self) -> u64 {
        self.snapshot
    }

    fn restricted_sql_executor(&self) -> &dyn RestrictedSqlExecutor {
        &self.exec
    }
}

/// 第二索引行数偏少时返回 TblCntGreater 与 offset=1，并恢复 invisible 标志。
#[test]
fn check_indices_count_reports_direction_offset_and_restores_session_flag() {
    let mut session = TestSession {
        invisible: false,
        txn_ts: Some(10),
        snapshot: 20,
        exec: QueueExecutor::default(),
    };
    // 表 COUNT=5，索引 equal=5，索引 short=3 → 表更大
    session.exec.rows.lock().unwrap().extend([
        Ok(vec![CountRow(5)]),
        Ok(vec![CountRow(5)]),
        Ok(vec![CountRow(3)]),
    ]);

    let (greater, offset, error) = CheckIndicesCount(
        &mut session,
        "db",
        "table name",
        &["equal".into(), "short".into()],
    );
    assert_eq!(greater, TblCntGreater);
    assert_eq!(offset, 1);
    assert!(matches!(error, Err(AdminError::CountMismatch { .. })));
    assert!(!session.invisible);
    assert_eq!(*session.exec.snapshots.lock().unwrap(), [20, 20, 20]);
}

/// COUNT 返回空行集时传播 InvalidCountRows，并恢复原 invisible=true。
#[test]
fn check_indices_count_propagates_bad_sql_shape_and_restores_flag() {
    let mut session = TestSession {
        invisible: true,
        txn_ts: Some(11),
        snapshot: 0,
        exec: QueueExecutor::default(),
    };
    session.exec.rows.lock().unwrap().push_back(Ok(Vec::new()));
    let result = CheckIndicesCount(&mut session, "db", "t", &[]);
    assert_eq!(result.0, 0);
    assert_eq!(result.1, 0);
    assert_eq!(result.2, Err(AdminError::InvalidCountRows(0)));
    assert!(session.invisible);
}
