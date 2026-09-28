// Copyright 2026 AsterSQL.
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

// DELETE 执行器指标累加辅助函数的单元测试。
//
// `addDeleteRowsColMultiply` 用于累计「删除行数 × 列数」类度量：
// 忽略非正增量，并用饱和加法防止 `i64` 溢出。

use std::collections::VecDeque;

use crate::delete::{
    DeleteChunk, DeleteExec, DeleteRuntime, TableColumnPosition, addDeleteRowsColMultiply,
    onRemoveRowForFK,
};

/// 验证正增量累加、非正增量忽略，以及接近上限时饱和到 `i64::MAX`。
#[test]
fn delete_metric_accumulation_saturates_and_ignores_non_positive_delta() {
    assert_eq!(addDeleteRowsColMultiply(10, 5), 15);
    assert_eq!(addDeleteRowsColMultiply(10, -1), 10);
    assert_eq!(addDeleteRowsColMultiply(i64::MAX - 1, 10), i64::MAX);
}

#[derive(Default)]
struct TestRuntime {
    chunks: VecDeque<DeleteChunk<i64>>,
    multi_table: bool,
    positions: Vec<TableColumnPosition>,
    ignored_handles: Vec<i64>,
    batch_size: usize,
    extra_handle: bool,
    memory_deltas: Vec<i64>,
    removed: Vec<(i64, i64, Vec<i64>)>,
    metrics: Vec<i64>,
    commits: usize,
    new_transactions: usize,
    flushes: usize,
    affected_rows: u64,
    fk_checks: usize,
    fk_cascades: usize,
    opened: usize,
    closed: usize,
    reset_memory: usize,
    close_error: bool,
}

impl DeleteRuntime for TestRuntime {
    type Context = ();
    type Request = usize;
    type Datum = i64;
    type Handle = i64;
    type ForeignKeyCheck = &'static str;
    type ForeignKeyCascade = &'static str;
    type Error = &'static str;

    fn reset_request(&self, request: &mut Self::Request) {
        *request = 0;
    }

    fn is_multi_table(&self) -> bool {
        self.multi_table
    }

    fn next_child_chunk(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<Option<DeleteChunk<Self::Datum>>, Self::Error> {
        Ok(self.chunks.pop_front())
    }

    fn consume_memory(&mut self, delta: i64) {
        self.memory_deltas.push(delta);
    }

    fn reset_memory_usage(&mut self) {
        self.reset_memory += 1;
    }

    fn may_flush_transaction(&mut self) -> Result<(), Self::Error> {
        self.flushes += 1;
        Ok(())
    }

    fn single_table_id(&self) -> i64 {
        7
    }

    fn single_table_has_extra_handle(&self) -> bool {
        self.extra_handle
    }

    fn filter_single_table_row(&self, joined_row: &[Self::Datum]) -> Vec<Self::Datum> {
        joined_row.to_vec()
    }

    fn build_handle(
        &mut self,
        _table_id: i64,
        position: Option<&TableColumnPosition>,
        row: &[Self::Datum],
    ) -> Result<Self::Handle, Self::Error> {
        Ok(row[position.map_or(0, |position| position.start)])
    }

    fn multi_table_positions(&self) -> Vec<TableColumnPosition> {
        self.positions.clone()
    }

    fn unmatched_outer_row(&self, position: &TableColumnPosition, row: &[Self::Datum]) -> bool {
        row[position.start] == -1
    }

    fn handle_extra_memory(&self, _handle: &Self::Handle) -> i64 {
        3
    }

    fn estimated_row_memory(&self, row: &[Self::Datum]) -> i64 {
        row.len() as i64
    }

    fn batch_delete_enabled(&self) -> bool {
        self.batch_size > 0
    }

    fn batch_dml_size(&self) -> usize {
        self.batch_size
    }

    fn commit_statement(&mut self, _context: &mut Self::Context) {
        self.commits += 1;
    }

    fn new_transaction_in_statement(
        &mut self,
        _context: &mut Self::Context,
    ) -> Result<(), Self::Error> {
        self.new_transactions += 1;
        Ok(())
    }

    fn batch_delete_error(&self, _error: Self::Error) -> Self::Error {
        "batch delete failed"
    }

    fn record_rows_column_multiply(&mut self, total: i64) {
        self.metrics.push(total);
    }

    fn ignore_errors(&self) -> bool {
        !self.ignored_handles.is_empty()
    }

    fn check_fk_ignore_error(
        &mut self,
        _context: &mut Self::Context,
        _table_id: i64,
        row: &[Self::Datum],
    ) -> Result<bool, Self::Error> {
        Ok(self.ignored_handles.contains(&row[0]))
    }

    fn remove_record(
        &mut self,
        _context: &mut Self::Context,
        table_id: i64,
        handle: &Self::Handle,
        data: &[Self::Datum],
        _position: Option<&TableColumnPosition>,
    ) -> Result<(), Self::Error> {
        self.removed.push((table_id, *handle, data.to_vec()));
        Ok(())
    }

    fn add_affected_rows(&mut self, rows: u64) {
        self.affected_rows += rows;
    }

    fn foreign_key_delete_checks(
        &mut self,
        _table_id: i64,
        _data: &[Self::Datum],
    ) -> Result<(), Self::Error> {
        self.fk_checks += 1;
        Ok(())
    }

    fn foreign_key_delete_cascades(
        &mut self,
        _table_id: i64,
        _data: &[Self::Datum],
    ) -> Result<(), Self::Error> {
        self.fk_cascades += 1;
        Ok(())
    }

    fn all_foreign_key_checks(&self) -> Vec<&Self::ForeignKeyCheck> {
        Vec::new()
    }

    fn all_foreign_key_cascades(&self) -> Vec<&Self::ForeignKeyCascade> {
        Vec::new()
    }

    fn open_child(&mut self, _context: &mut Self::Context) -> Result<(), Self::Error> {
        self.opened += 1;
        Ok(())
    }

    fn close_child(&mut self) -> Result<(), Self::Error> {
        self.closed += 1;
        if self.close_error {
            Err("close failed")
        } else {
            Ok(())
        }
    }
}

#[test]
fn single_table_delete_matches_batch_ignore_fk_metrics_and_memory_flow() {
    let runtime = TestRuntime {
        chunks: VecDeque::from([
            DeleteChunk {
                rows: vec![vec![1, 10], vec![2, 20]],
                memory_usage: 10,
            },
            DeleteChunk {
                rows: vec![vec![3, 30]],
                memory_usage: 20,
            },
        ]),
        ignored_handles: vec![2],
        batch_size: 1,
        ..TestRuntime::default()
    };
    let mut exec = DeleteExec { runtime };
    let mut request = 99;

    exec.Next(&mut (), &mut request).unwrap();

    assert_eq!(request, 0);
    assert_eq!(
        exec.runtime.removed,
        vec![(7, 1, vec![1, 10]), (7, 3, vec![3, 30])]
    );
    assert_eq!(exec.runtime.affected_rows, 2);
    assert_eq!(exec.runtime.metrics, vec![2, 2]);
    assert_eq!(exec.runtime.commits, 1);
    assert_eq!(exec.runtime.new_transactions, 1);
    assert_eq!(exec.runtime.flushes, 2);
    assert_eq!(exec.runtime.memory_deltas, vec![0, 10, -10, 20, -20]);
    assert_eq!(exec.runtime.fk_checks, 0);
    assert_eq!(exec.runtime.fk_cascades, 2);
}

#[test]
fn multi_table_delete_deduplicates_handles_skips_outer_rows_and_keeps_latest_values() {
    let positions = vec![
        TableColumnPosition {
            table_id: 1,
            start: 0,
            end: 2,
        },
        TableColumnPosition {
            table_id: 2,
            start: 2,
            end: 4,
        },
    ];
    let runtime = TestRuntime {
        chunks: VecDeque::from([DeleteChunk {
            rows: vec![vec![10, 100, 20, 200], vec![10, 101, -1, 0]],
            memory_usage: 8,
        }]),
        multi_table: true,
        positions,
        ..TestRuntime::default()
    };
    let mut exec = DeleteExec { runtime };

    exec.Next(&mut (), &mut 1).unwrap();

    exec.runtime.removed.sort_by_key(|entry| entry.0);
    assert_eq!(
        exec.runtime.removed,
        vec![(1, 10, vec![10, 101]), (2, 20, vec![20, 200])]
    );
    assert_eq!(exec.runtime.affected_rows, 2);
    assert_eq!(exec.runtime.metrics, vec![4]);
    assert_eq!(exec.runtime.flushes, 1);
    assert_eq!(exec.runtime.memory_deltas, vec![0, 8, 14, 0, -8]);
}

#[test]
fn foreign_key_and_lifecycle_hooks_match_delete_contract() {
    let mut runtime = TestRuntime::default();
    onRemoveRowForFK(&mut runtime, 1, &[1]).unwrap();
    assert_eq!((runtime.fk_checks, runtime.fk_cascades), (1, 1));

    runtime.ignored_handles.push(1);
    onRemoveRowForFK(&mut runtime, 1, &[1]).unwrap();
    assert_eq!((runtime.fk_checks, runtime.fk_cascades), (1, 2));

    runtime.close_error = true;
    let mut exec = DeleteExec { runtime };
    exec.Open(&mut ()).unwrap();
    assert_eq!(exec.Close(), Err("close failed"));
    assert_eq!(exec.runtime.opened, 1);
    assert_eq!(exec.runtime.closed, 1);
    assert_eq!(exec.runtime.reset_memory, 1);
}
