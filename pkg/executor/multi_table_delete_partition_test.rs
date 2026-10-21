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

use std::collections::VecDeque;

use crate::delete::{DeleteChunk, DeleteExec, DeleteRuntime, TableColumnPosition};

#[derive(Default)]
struct PartitionDeleteRuntime {
    chunks: VecDeque<DeleteChunk<i64>>,
    removed: Vec<(i64, Vec<i64>)>,
    affected_rows: u64,
}

impl DeleteRuntime for PartitionDeleteRuntime {
    type Context = ();
    type Request = ();
    type Datum = i64;
    type Handle = i64;
    type ForeignKeyCheck = ();
    type ForeignKeyCascade = ();
    type Error = &'static str;

    fn reset_request(&self, _: &mut Self::Request) {}
    fn is_multi_table(&self) -> bool {
        true
    }
    fn next_child_chunk(
        &mut self,
        _: &mut Self::Context,
    ) -> Result<Option<DeleteChunk<i64>>, Self::Error> {
        Ok(self.chunks.pop_front())
    }
    fn consume_memory(&mut self, _: i64) {}
    fn reset_memory_usage(&mut self) {}
    fn may_flush_transaction(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn single_table_id(&self) -> i64 {
        1
    }
    fn single_table_has_extra_handle(&self) -> bool {
        false
    }
    fn filter_single_table_row(&self, row: &[i64]) -> Vec<i64> {
        row.to_vec()
    }
    fn build_handle(
        &mut self,
        _: i64,
        position: Option<&TableColumnPosition>,
        row: &[i64],
    ) -> Result<i64, Self::Error> {
        Ok(row[position.map_or(0, |position| position.start)])
    }
    fn physical_row_identity(
        &mut self,
        _: i64,
        position: &TableColumnPosition,
        row: &[i64],
        handle: &i64,
    ) -> Result<i64, Self::Error> {
        Ok(row[position.start + 1] * 1_000 + handle)
    }
    fn remove_record_handle(&self, physical_identity: &i64) -> i64 {
        physical_identity % 1_000
    }
    fn multi_table_positions(&self) -> Vec<TableColumnPosition> {
        vec![TableColumnPosition {
            table_id: 1,
            start: 0,
            end: 3,
        }]
    }
    fn unmatched_outer_row(&self, _: &TableColumnPosition, _: &[i64]) -> bool {
        false
    }
    fn handle_extra_memory(&self, _: &i64) -> i64 {
        0
    }
    fn estimated_row_memory(&self, row: &[i64]) -> i64 {
        row.len() as i64
    }
    fn batch_delete_enabled(&self) -> bool {
        false
    }
    fn batch_dml_size(&self) -> usize {
        0
    }
    fn commit_statement(&mut self, _: &mut Self::Context) {}
    fn new_transaction_in_statement(&mut self, _: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }
    fn batch_delete_error(&self, error: Self::Error) -> Self::Error {
        error
    }
    fn reset_write_runtime_stats(&mut self) {}
    fn record_write_cpu_work(&mut self, _: i64, _: usize) {}
    fn ignore_errors(&self) -> bool {
        false
    }
    fn check_fk_ignore_error(
        &mut self,
        _: &mut Self::Context,
        _: i64,
        _: &[i64],
    ) -> Result<bool, Self::Error> {
        Ok(false)
    }
    fn remove_record(
        &mut self,
        _: &mut Self::Context,
        _: i64,
        handle: &i64,
        data: &[i64],
        _: Option<&TableColumnPosition>,
    ) -> Result<(), Self::Error> {
        self.removed.push((*handle, data.to_vec()));
        Ok(())
    }
    fn add_affected_rows(&mut self, rows: u64) {
        self.affected_rows += rows;
    }
    fn foreign_key_delete_checks(&mut self, _: i64, _: &[i64]) -> Result<(), Self::Error> {
        Ok(())
    }
    fn foreign_key_delete_cascades(&mut self, _: i64, _: &[i64]) -> Result<(), Self::Error> {
        Ok(())
    }
    fn all_foreign_key_checks(&self) -> Vec<&Self::ForeignKeyCheck> {
        Vec::new()
    }
    fn all_foreign_key_cascades(&self) -> Vec<&Self::ForeignKeyCascade> {
        Vec::new()
    }
    fn open_child(&mut self, _: &mut Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }
    fn close_child(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn multi_table_delete_partition_keeps_same_rowid_from_each_partition() {
    let runtime = PartitionDeleteRuntime {
        chunks: VecDeque::from([DeleteChunk {
            // [handle, physical partition ID, value]
            rows: vec![
                vec![1, 101, 10],
                vec![1, 101, 10],
                vec![1, 102, 11],
                vec![1, 102, 11],
            ],
            memory_usage: 0,
        }]),
        ..PartitionDeleteRuntime::default()
    };
    let mut exec = DeleteExec { runtime };

    exec.Next(&mut (), &mut ()).unwrap();

    exec.runtime.removed.sort_by_key(|entry| entry.1[1]);
    assert_eq!(
        exec.runtime.removed,
        vec![(1, vec![1, 101, 10]), (1, vec![1, 102, 11])]
    );
    assert_eq!(exec.runtime.affected_rows, 2);
}
