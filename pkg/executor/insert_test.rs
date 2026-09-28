// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// INSERT 重复键检查模式选择的单元测试。
//
// 覆盖：
// - `optimizeDupKeyCheckForNormalInsert`：按原地约束检查、悲观事务、流水线决定 Lazy/InPlace；
// - `getPessimisticLazyCheckMode`：悲观延迟检查落在 Prewrite 还是 AcquireLock 阶段。

use crate::insert::{
    DupKeyCheckMode, InsertExec, InsertRuntime, PessimisticLazyDupKeyCheckMode,
    getPessimisticLazyCheckMode, optimizeDupKeyCheckForNormalInsert,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Debug)]
struct CheckedRow {
    handle_key: Option<i32>,
    unique_keys: Vec<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TestError {
    NotFound,
    Flush,
}

#[derive(Default)]
struct TestRuntime {
    snapshot_ended: bool,
    fail_flush: bool,
    on_duplicate: Vec<()>,
    conflicting_unique_key: bool,
    inconsistent_index_logs: Cell<usize>,
}

impl InsertRuntime for TestRuntime {
    type Context = ();
    type Request = ();
    type Row = i32;
    type CheckedRow = CheckedRow;
    type Key = i32;
    type Value = i32;
    type Handle = i32;
    type Transaction = ();
    type Assignment = ();
    type ForeignKeyCheck = ();
    type ForeignKeyCascade = ();
    type Error = TestError;

    fn transaction(&mut self) -> Result<Self::Transaction, Self::Error> {
        Ok(())
    }
    fn set_top_sql_option(&mut self, _: &mut Self::Transaction) {}
    fn collect_runtime_stats_enabled(&self) -> bool {
        true
    }
    fn begin_snapshot_stats(&mut self, _: &mut Self::Transaction) {}
    fn end_snapshot_stats(&mut self, _: &mut Self::Transaction) {
        self.snapshot_ended = true;
    }
    fn add_record_rows(&mut self, _: u64) {}
    fn on_duplicate_assignments(&self) -> &[Self::Assignment] {
        &self.on_duplicate
    }
    fn ignore_errors(&self) -> bool {
        false
    }
    fn shard_allocate_step(&self) -> usize {
        1
    }
    fn add_record(
        &mut self,
        _: &mut Self::Context,
        _: Self::Row,
        _: DupKeyCheckMode,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn add_record_with_auto_id_hint(
        &mut self,
        _: &mut Self::Context,
        _: Self::Row,
        _: usize,
        _: DupKeyCheckMode,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn batch_check_and_insert(
        &mut self,
        _: &mut Self::Context,
        _: Vec<Self::Row>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn may_flush(&mut self, _: &mut Self::Transaction) -> Result<(), Self::Error> {
        if self.fail_flush {
            Err(TestError::Flush)
        } else {
            Ok(())
        }
    }
    fn clean_buffers(&mut self) {}
    fn record_check_insert_elapsed(&mut self, _: Duration) {}
    fn keys_need_check(
        &mut self,
        rows: Vec<Self::Row>,
    ) -> Result<Vec<Self::CheckedRow>, Self::Error> {
        Ok(rows
            .into_iter()
            .map(|_| CheckedRow {
                handle_key: None,
                unique_keys: self
                    .conflicting_unique_key
                    .then_some(7)
                    .into_iter()
                    .collect(),
            })
            .collect())
    }
    fn checked_row_ignored(&self, _: &Self::CheckedRow) -> bool {
        false
    }
    fn checked_handle_key(&self, row: &Self::CheckedRow) -> Option<Self::Key> {
        row.handle_key
    }
    fn checked_unique_keys(&self, row: &Self::CheckedRow) -> Vec<Self::Key> {
        row.unique_keys.clone()
    }
    fn batch_get(
        &mut self,
        _: &mut Self::Context,
        _: &mut Self::Transaction,
        _: Vec<Self::Key>,
    ) -> Result<HashMap<Self::Key, Self::Value>, Self::Error> {
        Ok(HashMap::new())
    }
    fn temporary_index_key(&self, _: &Self::Key) -> bool {
        false
    }
    fn decode_handle_in_index_value(
        &self,
        value: &Self::Value,
    ) -> Result<Self::Handle, Self::Error> {
        Ok(*value)
    }
    fn record_key(&self, _: &Self::CheckedRow, handle: &Self::Handle) -> Self::Key {
        *handle
    }
    fn table_is_temporary(&self) -> bool {
        false
    }
    fn decode_row_key(&self, key: &Self::Key) -> Result<Self::Handle, Self::Error> {
        Ok(*key)
    }
    fn fetch_duplicated_handle(
        &mut self,
        _: &mut Self::Context,
        _: &mut Self::Transaction,
        _: &Self::Key,
    ) -> Result<Option<Self::Handle>, Self::Error> {
        Ok(Some(42))
    }
    fn update_duplicate_row(
        &mut self,
        _: &mut Self::Context,
        _: usize,
        _: &mut Self::Transaction,
        _: &Self::CheckedRow,
        _: Self::Handle,
        _: DupKeyCheckMode,
        _: Option<usize>,
    ) -> Result<(), Self::Error> {
        Err(TestError::NotFound)
    }
    fn error_is_not_found(&self, error: &Self::Error) -> bool {
        *error == TestError::NotFound
    }
    fn log_inconsistent_unique_index(&self, _: &Self::Key, _: &Self::Handle, _: &Self::CheckedRow) {
        self.inconsistent_index_logs
            .set(self.inconsistent_index_logs.get() + 1);
    }
    fn checked_row_into_row(&mut self, _: Self::CheckedRow) -> Self::Row {
        0
    }
    fn auto_increment_column(&self) -> Option<usize> {
        None
    }
    fn update_duplicate_key_mode(&self, _: &Self::Transaction) -> DupKeyCheckMode {
        DupKeyCheckMode::InPlace
    }
    fn normal_duplicate_key_mode(&self, _: &Self::Transaction) -> DupKeyCheckMode {
        DupKeyCheckMode::InPlace
    }
    fn reset_request(&self, _: &mut Self::Request) {}
    fn enable_rows_column_metric(&mut self) {}
    fn has_select_executor(&self) -> bool {
        false
    }
    fn insert_rows_from_select(&mut self, _: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }
    fn insert_rows(&mut self, _: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }
    fn handle_auto_increment_read_error(&mut self, error: Self::Error) -> Self::Error {
        error
    }
    fn error_is_auto_increment_read_failure(&self, _: &Self::Error) -> bool {
        false
    }
    fn register_runtime_stats(&mut self) {}
    fn reset_memory_usage(&mut self) {}
    fn set_insert_message(&mut self) {}
    fn close_select_executor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn open_select_executor(&mut self, _: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }
    fn initialize_duplicate_evaluation_buffer(&mut self) {}
    fn initialize_evaluation_buffer(&mut self) {}
    fn all_assignments_are_constant(&self) -> bool {
        true
    }
    fn foreign_key_checks(&self) -> &[Self::ForeignKeyCheck] {
        &[]
    }
    fn foreign_key_cascades(&self) -> &[Self::ForeignKeyCascade] {
        &[]
    }
}

/// 核对普通 INSERT 与悲观延迟检查在事务/流水线规则下的模式选择。
#[test]
fn insert_duplicate_check_modes_follow_transaction_and_pipeline_rules() {
    // 原地约束检查且非悲观、非流水线 → InPlace
    assert_eq!(
        optimizeDupKeyCheckForNormalInsert(true, false, false),
        DupKeyCheckMode::InPlace
    );
    // 悲观事务强制 Lazy
    assert_eq!(
        optimizeDupKeyCheckForNormalInsert(true, true, false),
        DupKeyCheckMode::Lazy
    );
    assert_eq!(
        getPessimisticLazyCheckMode(false, true, false, 42),
        PessimisticLazyDupKeyCheckMode::InPrewrite
    );
    // 受限 SQL 时改在加锁阶段检查
    assert_eq!(
        getPessimisticLazyCheckMode(false, true, true, 42),
        PessimisticLazyDupKeyCheckMode::InAcquireLock
    );
}

#[test]
fn insert_ends_snapshot_stats_when_flush_fails() {
    let mut executor = InsertExec {
        runtime: TestRuntime {
            fail_flush: true,
            ..TestRuntime::default()
        },
    };

    assert_eq!(executor.exec(&mut (), vec![1]), Err(TestError::Flush));
    assert!(executor.runtime.snapshot_ended);
}

#[test]
fn insert_logs_unique_index_pointing_to_missing_row() {
    let runtime = TestRuntime {
        on_duplicate: vec![()],
        conflicting_unique_key: true,
        ..TestRuntime::default()
    };
    let mut executor = InsertExec { runtime };

    assert_eq!(executor.exec(&mut (), vec![1]), Err(TestError::NotFound));
    assert_eq!(executor.runtime.inconsistent_index_logs.get(), 1);
}
