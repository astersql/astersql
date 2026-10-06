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

use std::collections::VecDeque;
use std::time::Duration;

use astersql_util_chunk::Chunk;

use crate::replace::{ReplaceExec, ReplaceRuntime};

#[derive(Clone)]
struct CheckedRow {
    handle_key: Option<u8>,
    unique_keys: Vec<u8>,
}

struct TestRuntime {
    checked_rows: Vec<CheckedRow>,
    transaction_get: Result<bool, &'static str>,
    duplicated_handles: VecDeque<Option<u8>>,
    remove_results: VecDeque<bool>,
    events: Vec<&'static str>,
    record_rows_added: u64,
    snapshot_stats: bool,
    snapshot_ends: usize,
    added: usize,
    close_error: bool,
    write_rows: Vec<usize>,
    write_stats_resets: usize,
}

impl Default for TestRuntime {
    fn default() -> Self {
        Self {
            checked_rows: Vec::new(),
            transaction_get: Ok(false),
            duplicated_handles: VecDeque::new(),
            remove_results: VecDeque::new(),
            events: Vec::new(),
            record_rows_added: 0,
            snapshot_stats: false,
            snapshot_ends: 0,
            added: 0,
            close_error: false,
            write_rows: Vec::new(),
            write_stats_resets: 0,
        }
    }
}

impl ReplaceRuntime for TestRuntime {
    type Context = ();
    type Row = u8;
    type CheckedRow = CheckedRow;
    type Handle = u8;
    type Transaction = ();
    type DuplicateKeyCheckMode = ();
    type ForeignKeyCheck = ();
    type ForeignKeyCascade = ();
    type Error = &'static str;

    fn attach_memory_tracker(&mut self) {
        self.events.push("attach");
    }
    fn open_select(&mut self, _: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }
    fn close_select(&mut self) -> Result<(), Self::Error> {
        self.events.push("close");
        if self.close_error {
            Err("close failed")
        } else {
            Ok(())
        }
    }
    fn has_select_executor(&self) -> bool {
        true
    }
    fn initialize_evaluation_buffer(&mut self) {
        self.events.push("init");
    }
    fn register_runtime_stats(&mut self) {
        self.events.push("register");
    }
    fn transaction(&mut self) -> Result<Self::Transaction, Self::Error> {
        Ok(())
    }
    fn handle_key(&self, row: &Self::CheckedRow) -> Option<Vec<u8>> {
        row.handle_key.map(|key| vec![key])
    }
    fn unique_keys(&self, row: &Self::CheckedRow) -> Vec<Vec<u8>> {
        row.unique_keys.iter().map(|key| vec![*key]).collect()
    }
    fn decode_row_key(&self, key: &[u8]) -> Result<Self::Handle, Self::Error> {
        Ok(key[0])
    }
    fn transaction_get(&mut self, _: &mut (), _: &mut (), _: &[u8]) -> Result<bool, Self::Error> {
        self.transaction_get
    }
    fn error_is_not_found(&self, error: &Self::Error) -> bool {
        *error == "not found"
    }
    fn fetch_duplicated_handle(
        &mut self,
        _: &mut (),
        _: &mut (),
        _: &[u8],
    ) -> Result<Option<Self::Handle>, Self::Error> {
        Ok(self.duplicated_handles.pop_front().flatten())
    }
    fn remove_row(
        &mut self,
        _: &mut (),
        _: &mut (),
        _: Self::Handle,
        _: &Self::CheckedRow,
    ) -> Result<bool, Self::Error> {
        self.events.push("remove");
        Ok(self.remove_results.pop_front().unwrap_or(false))
    }
    fn add_record(&mut self, _: &mut (), _: &Self::CheckedRow, _: &()) -> Result<(), Self::Error> {
        self.added += 1;
        Ok(())
    }
    fn keys_need_check(&mut self, _: Vec<Self::Row>) -> Result<Vec<Self::CheckedRow>, Self::Error> {
        Ok(std::mem::take(&mut self.checked_rows))
    }
    fn begin_snapshot_runtime_stats(&mut self, _: &mut ()) {
        self.events.push("begin_stats");
    }
    fn end_snapshot_runtime_stats(&mut self, _: &mut ()) {
        self.snapshot_ends += 1;
    }
    fn set_top_sql_option(&mut self, _: &mut ()) {}
    fn prefetch_data_cache(
        &mut self,
        _: &mut (),
        _: &mut (),
        _: &[Self::CheckedRow],
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_prefetch_duration(&mut self, _: Duration) {}
    fn add_record_rows(&mut self, rows: u64) {
        self.record_rows_added += rows;
    }
    fn reset_write_runtime_stats(&mut self) {
        self.write_stats_resets += 1;
    }
    fn record_write_cpu_work(&mut self, rows: usize) {
        self.write_rows.push(rows);
    }
    fn optimize_duplicate_key_check(&self, _: &()) -> Self::DuplicateKeyCheckMode {}
    fn may_flush(&mut self, _: &mut ()) -> Result<(), Self::Error> {
        Ok(())
    }
    fn collect_runtime_stats_enabled(&self) -> bool {
        self.snapshot_stats
    }
    fn reset_output_chunk(&self, _: &mut Chunk) {}
    fn enable_ruv2_rows_column_metric(&mut self) {}
    fn insert_rows_from_select(&mut self, _: &mut ()) -> Result<(), Self::Error> {
        Ok(())
    }
    fn insert_rows(&mut self, _: &mut ()) -> Result<(), Self::Error> {
        Ok(())
    }
    fn has_child_executor(&self) -> bool {
        false
    }
    fn record_rows(&self) -> u64 {
        0
    }
    fn warning_count(&self) -> u64 {
        0
    }
    fn affected_rows(&self) -> u64 {
        0
    }
    fn set_statement_message(&mut self, _: String) {}
    fn foreign_key_checks(&self) -> &[Self::ForeignKeyCheck] {
        &[]
    }
    fn foreign_key_cascades(&self) -> &[Self::ForeignKeyCascade] {
        &[]
    }
}

#[test]
fn exec_counts_input_rows_and_only_closes_enabled_snapshot_stats() {
    let runtime = TestRuntime {
        checked_rows: vec![CheckedRow {
            handle_key: None,
            unique_keys: vec![],
        }],
        transaction_get: Ok(false),
        ..TestRuntime::default()
    };
    let mut executor = ReplaceExec {
        runtime,
        priority: 0,
    };

    executor.exec(&mut (), vec![1, 2, 3]).unwrap();

    assert_eq!(executor.runtime.record_rows_added, 3);
    assert_eq!(executor.runtime.write_rows, vec![3]);
    assert_eq!(executor.runtime.snapshot_ends, 0);
    assert_eq!(executor.runtime.added, 1);
}

#[test]
fn open_resets_replace_write_runtime_stats() {
    let mut executor = ReplaceExec {
        runtime: TestRuntime::default(),
        priority: 0,
    };

    executor.Open(&mut ()).unwrap();

    assert_eq!(executor.runtime.write_stats_resets, 1);
}

#[test]
fn close_registers_stats_after_child_close_even_when_close_fails() {
    let runtime = TestRuntime {
        transaction_get: Ok(false),
        close_error: true,
        ..TestRuntime::default()
    };
    let mut executor = ReplaceExec {
        runtime,
        priority: 0,
    };

    assert_eq!(executor.Close(), Err("close failed"));
    assert_eq!(executor.runtime.events, vec!["close", "register"]);
}

#[test]
fn replace_removes_handle_and_each_unique_conflict_before_insert() {
    let runtime = TestRuntime {
        transaction_get: Ok(true),
        duplicated_handles: VecDeque::from([Some(8), Some(9), None]),
        remove_results: VecDeque::from([false, false, false]),
        ..TestRuntime::default()
    };
    let mut executor = ReplaceExec {
        runtime,
        priority: 0,
    };
    let row = CheckedRow {
        handle_key: Some(1),
        unique_keys: vec![2, 3],
    };

    executor.replaceRow(&mut (), &mut (), &row, &()).unwrap();

    assert_eq!(executor.runtime.events, vec!["remove", "remove", "remove"]);
    assert_eq!(executor.runtime.added, 1);
}
