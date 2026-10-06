// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `SELECT` 锁键去重单元测试。
//
// 验证 `deduplicateLockKeys`：去掉重复键的同时保留首次出现的相对顺序，
// 与悲观锁（pessimistic lock）加锁前的键集整理行为一致。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::select::{
    ExecuteLimitValues, LockContext, LockMode, SelectLockExec, SelectRuntime, deduplicateLockKeys,
    filterLockTableKeys, newLockCtx,
};

#[derive(Default)]
struct LockRuntime {
    batches: VecDeque<Vec<i32>>,
    locked: Vec<Vec<i32>>,
    lock_table_filter_enabled: bool,
    allow_shared_lock_upgrade: bool,
}

impl SelectRuntime for LockRuntime {
    type Context = ();
    type Chunk = Vec<i32>;
    type Row = i32;
    type Key = i32;
    type Table = ();
    type Statement = ();
    type UpdateStatement = ();
    type DeleteStatement = ();
    type Snapshot = ();
    type Error = String;

    fn reset_chunk(&mut self, chunk: &mut Self::Chunk) {
        chunk.clear();
    }
    fn grow_and_reset_chunk(&mut self, chunk: &mut Self::Chunk, _: usize) {
        chunk.clear();
    }
    fn chunk_rows(&self, chunk: &Self::Chunk) -> usize {
        chunk.len()
    }
    fn chunk_capacity(&self, chunk: &Self::Chunk) -> usize {
        chunk.capacity().max(1)
    }
    fn truncate_chunk(&mut self, chunk: &mut Self::Chunk, rows: usize) {
        chunk.truncate(rows);
    }
    fn append_null_row(&mut self, chunk: &mut Self::Chunk, _: usize) {
        chunk.push(0);
    }
    fn swap_chunk(&mut self, destination: &mut Self::Chunk, source: &mut Self::Chunk) {
        std::mem::swap(destination, source);
    }
    fn new_chunk(&mut self, capacity: usize) -> Self::Chunk {
        Vec::with_capacity(capacity)
    }
    fn open_child(&mut self, _: &mut Self::Context, _: usize) -> Result<(), Self::Error> {
        Ok(())
    }
    fn close_child(&mut self, _: usize) -> Result<(), Self::Error> {
        Ok(())
    }
    fn next_child(
        &mut self,
        _: &mut Self::Context,
        _: usize,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error> {
        *chunk = self.batches.pop_front().unwrap_or_default();
        Ok(())
    }
    fn child_initial_capacity(&self, _: usize) -> usize {
        1
    }
    fn child_max_chunk_size(&self, _: usize) -> usize {
        1
    }
    fn schema_columns(&self) -> usize {
        1
    }
    fn select_lock_enabled(&self) -> bool {
        true
    }
    fn select_lock_mode(&self) -> LockMode {
        LockMode::ForUpdate
    }
    fn select_lock_wait_time(&self) -> i64 {
        0
    }
    fn allow_shared_lock_upgrade(&self) -> bool {
        self.allow_shared_lock_upgrade
    }
    fn lock_keys_from_chunk(&mut self, chunk: &Self::Chunk) -> Result<Vec<Self::Key>, Self::Error> {
        Ok(chunk.clone())
    }
    fn key_is_temporary_table(&self, _: &Self::Key) -> bool {
        false
    }
    fn key_is_lock_table(&self, key: &Self::Key) -> bool {
        *key == 2
    }
    fn lock_table_filter_enabled(&self) -> bool {
        self.lock_table_filter_enabled
    }
    fn key_is_untouched_index(&self, _: &Self::Key) -> bool {
        false
    }
    fn pessimistic_lock(
        &mut self,
        _: &mut Self::Context,
        lock_context: &mut LockContext<Self::Key>,
    ) -> Result<(), Self::Error> {
        self.locked.push(lock_context.keys.clone());
        Ok(())
    }
    fn record_locked_keys(&mut self, _: usize) {}
    fn lock_error_is_deadlock(&self, _: &Self::Error) -> bool {
        false
    }
    fn record_deadlock(&mut self, _: &Self::Error) {}
    fn max_execution_deadline(&self) -> Option<Instant> {
        None
    }
    fn interrupted_error(&self) -> Self::Error {
        "interrupted".to_owned()
    }
    fn subquery_more_than_one_row_error(&self) -> Self::Error {
        "multiple rows".to_owned()
    }
    fn lock_wait_timeout(&self, _: i64) -> Result<Duration, Self::Error> {
        Ok(Duration::ZERO)
    }
    fn selection_batched(&self) -> bool {
        false
    }
    fn selection_vectorized_filter(
        &mut self,
        _: &mut Self::Context,
        input: &Self::Chunk,
    ) -> Result<Vec<bool>, Self::Error> {
        Ok(vec![true; input.len()])
    }
    fn selection_append_selected(
        &mut self,
        _: &mut Self::Chunk,
        _: &Self::Chunk,
        _: &[bool],
        _: &mut usize,
    ) {
    }
    fn selection_take_selected(&mut self, _: &mut Self::Chunk, _: &Self::Chunk, _: &[bool]) {}
    fn table_scan_all(&mut self, _: &mut Self::Context) -> Result<Vec<Self::Chunk>, Self::Error> {
        Ok(Vec::new())
    }
    fn reset_statement_context(&mut self, _: &Self::Statement) -> Result<(), Self::Error> {
        Ok(())
    }
    fn reset_update_statement_context(&mut self, _: &Self::UpdateStatement) {}
    fn reset_delete_statement_context(&mut self, _: &Self::DeleteStatement) {}
    fn set_top_sql_snapshot_options(&mut self, _: &mut Self::Snapshot) {}
    fn weak_consistency_read(&self, _: &Self::Statement) -> bool {
        false
    }
}

#[test]
fn new_lock_context_propagates_shared_lock_upgrade_gate() {
    let runtime = LockRuntime {
        allow_shared_lock_upgrade: true,
        ..LockRuntime::default()
    };

    let context = newLockCtx(&runtime, 123, vec![1], true).expect("lock context");

    assert_eq!(context.mode, LockMode::Shared);
    assert!(context.allow_shared_lock_upgrade);
}

/// 重复键只保留第一次出现，不重排其余键的相对顺序。
#[test]
fn select_lock_keys_are_deduplicated_without_reordering_first_occurrences() {
    let mut keys = vec![
        b"k2".to_vec(),
        b"k1".to_vec(),
        b"k2".to_vec(),
        b"k3".to_vec(),
    ];
    deduplicateLockKeys(&mut keys);
    assert_eq!(keys, vec![b"k2".to_vec(), b"k1".to_vec(), b"k3".to_vec()]);
}

/// 与 Go `LimitExec.Next` 一致：OFFSET 可跨 chunk，最终仅返回窗口内的行。
#[test]
fn limit_values_follow_go_limit_exec_chunk_lifecycle() {
    let rows = (0..10).collect::<Vec<_>>();

    assert_eq!(
        ExecuteLimitValues(rows.clone(), 3, 4, 2).expect("execute LIMIT across chunks"),
        vec![3, 4, 5, 6]
    );
    assert_eq!(
        ExecuteLimitValues(rows.clone(), 4, 2, 2).expect("resume after exact chunk boundary"),
        vec![4, 5]
    );
    assert_eq!(
        ExecuteLimitValues(rows.clone(), 9, 5, 3).expect("truncate LIMIT at input end"),
        vec![9]
    );
    assert_eq!(
        ExecuteLimitValues(rows, 4, 0, 2).expect("zero count returns no rows"),
        Vec::<i32>::new()
    );
}

/// Go buffers keys from all non-empty chunks and acquires locks once EOF is observed.
#[test]
fn select_lock_buffers_all_chunks_and_locks_only_at_eof() {
    let runtime = LockRuntime {
        batches: VecDeque::from([vec![1], vec![2], Vec::new()]),
        locked: Vec::new(),
        lock_table_filter_enabled: false,
        allow_shared_lock_upgrade: false,
    };
    let mut executor = SelectLockExec {
        runtime,
        keys: Vec::new(),
    };
    let mut context = ();
    let mut chunk = Vec::new();

    executor.Open(&mut context).expect("open select lock");
    executor
        .Next(&mut context, &mut chunk)
        .expect("first chunk");
    assert!(executor.runtime.locked.is_empty());
    executor
        .Next(&mut context, &mut chunk)
        .expect("second chunk");
    assert!(executor.runtime.locked.is_empty());
    executor
        .Next(&mut context, &mut chunk)
        .expect("lock at EOF");
    assert_eq!(executor.runtime.locked, vec![vec![1, 2]]);
}

/// Go leaves all keys untouched without LOCK TABLES metadata.
#[test]
fn select_lock_table_filter_is_disabled_without_a_whitelist() {
    let runtime = LockRuntime::default();
    let mut keys = vec![1, 2, 3];
    filterLockTableKeys(&runtime, &mut keys);
    assert_eq!(keys, vec![1, 2, 3]);
}

/// Go retains only keys whose table IDs are present when LOCK TABLES is active.
#[test]
fn select_lock_table_filter_retains_whitelisted_keys() {
    let runtime = LockRuntime {
        lock_table_filter_enabled: true,
        ..LockRuntime::default()
    };
    let mut keys = vec![1, 2, 3];
    filterLockTableKeys(&runtime, &mut keys);
    assert_eq!(keys, vec![2]);
}
