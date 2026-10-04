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

// INSERT 执行器核心：重复键检查模式、运行时抽象与批更新路径。
//
// 对应 Go 的 insert executor：通过 `InsertRuntime` 解耦存储/会话细节，
// `InsertExec` 负责普通插入、ON DUPLICATE KEY UPDATE（冲突时按赋值更新已有行）、
// 以及 SELECT→INSERT 的生命周期（Open/Next/Close）。
// 重复键检查可分为 Lazy（延迟到预写/加锁）与 InPlace（写入时立即检查）。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::hash::Hash;

/// 普通 INSERT 的重复键（duplicate key）检查时机。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DupKeyCheckMode {
    /// 延迟检查：写入时不立刻校验唯一约束，交给后续阶段。
    Lazy,
    /// 原地检查：写入路径上立即做约束校验。
    InPlace,
}

/// 悲观事务下 Lazy 重复键检查落点（Prewrite 预写 或 AcquireLock 加锁）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PessimisticLazyDupKeyCheckMode {
    /// 在两阶段提交的 Prewrite（预写）阶段检查。
    InPrewrite,
    /// 在悲观加锁（AcquireLock）阶段检查。
    InAcquireLock,
}

/// INSERT 执行所需的运行时能力边界（事务、写行、预取、外键等）。
pub trait InsertRuntime {
    type Context;
    type Request;
    type Row;
    type CheckedRow;
    type Key: Clone + Eq + Hash;
    type Value;
    type Handle: Clone;
    type Transaction;
    type Assignment;
    type ForeignKeyCheck;
    type ForeignKeyCascade;
    type Error: std::error::Error + 'static;

    fn transaction(&mut self) -> Result<Self::Transaction, Self::Error>;
    fn set_top_sql_option(&mut self, transaction: &mut Self::Transaction);
    fn collect_runtime_stats_enabled(&self) -> bool;
    fn begin_snapshot_stats(&mut self, transaction: &mut Self::Transaction);
    fn end_snapshot_stats(&mut self, transaction: &mut Self::Transaction);
    fn add_record_rows(&mut self, rows: u64);
    fn on_duplicate_assignments(&self) -> &[Self::Assignment];
    fn ignore_errors(&self) -> bool;
    fn shard_allocate_step(&self) -> usize;
    fn add_record(
        &mut self,
        context: &mut Self::Context,
        row: Self::Row,
        mode: DupKeyCheckMode,
    ) -> Result<(), Self::Error>;
    fn add_record_with_auto_id_hint(
        &mut self,
        context: &mut Self::Context,
        row: Self::Row,
        size_hint: usize,
        mode: DupKeyCheckMode,
    ) -> Result<(), Self::Error>;
    fn batch_check_and_insert(
        &mut self,
        context: &mut Self::Context,
        rows: Vec<Self::Row>,
    ) -> Result<(), Self::Error>;
    fn may_flush(&mut self, transaction: &mut Self::Transaction) -> Result<(), Self::Error>;
    fn clean_buffers(&mut self);
    fn record_check_insert_elapsed(&mut self, elapsed: std::time::Duration);

    fn keys_need_check(
        &mut self,
        rows: Vec<Self::Row>,
    ) -> Result<Vec<Self::CheckedRow>, Self::Error>;
    fn checked_row_ignored(&self, row: &Self::CheckedRow) -> bool;
    fn checked_handle_key(&self, row: &Self::CheckedRow) -> Option<Self::Key>;
    fn checked_unique_keys(&self, row: &Self::CheckedRow) -> Vec<Self::Key>;
    fn batch_get(
        &mut self,
        context: &mut Self::Context,
        transaction: &mut Self::Transaction,
        keys: Vec<Self::Key>,
    ) -> Result<HashMap<Self::Key, Self::Value>, Self::Error>;
    fn temporary_index_key(&self, key: &Self::Key) -> bool;
    fn decode_handle_in_index_value(
        &self,
        value: &Self::Value,
    ) -> Result<Self::Handle, Self::Error>;
    fn record_key(&self, row: &Self::CheckedRow, handle: &Self::Handle) -> Self::Key;
    fn table_is_temporary(&self) -> bool;
    fn decode_row_key(&self, key: &Self::Key) -> Result<Self::Handle, Self::Error>;
    fn fetch_duplicated_handle(
        &mut self,
        context: &mut Self::Context,
        transaction: &mut Self::Transaction,
        key: &Self::Key,
    ) -> Result<Option<Self::Handle>, Self::Error>;
    fn update_duplicate_row(
        &mut self,
        context: &mut Self::Context,
        row_index: usize,
        transaction: &mut Self::Transaction,
        row: &Self::CheckedRow,
        handle: Self::Handle,
        mode: DupKeyCheckMode,
        auto_increment_column: Option<usize>,
    ) -> Result<(), Self::Error>;
    fn error_is_not_found(&self, error: &Self::Error) -> bool;
    fn log_inconsistent_unique_index(
        &self,
        key: &Self::Key,
        handle: &Self::Handle,
        row: &Self::CheckedRow,
    );
    fn checked_row_into_row(&mut self, row: Self::CheckedRow) -> Self::Row;
    fn auto_increment_column(&self) -> Option<usize>;
    fn update_duplicate_key_mode(&self, transaction: &Self::Transaction) -> DupKeyCheckMode;
    fn normal_duplicate_key_mode(&self, transaction: &Self::Transaction) -> DupKeyCheckMode;

    fn reset_request(&self, request: &mut Self::Request);
    fn enable_rows_column_metric(&mut self);
    fn has_select_executor(&self) -> bool;
    fn insert_rows_from_select(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>;
    fn insert_rows(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>;
    fn handle_auto_increment_read_error(&mut self, error: Self::Error) -> Self::Error;
    fn error_is_auto_increment_read_failure(&self, error: &Self::Error) -> bool;
    fn register_runtime_stats(&mut self);
    fn reset_memory_usage(&mut self);
    fn set_insert_message(&mut self);
    fn close_select_executor(&mut self) -> Result<(), Self::Error>;
    fn open_select_executor(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>;
    fn initialize_duplicate_evaluation_buffer(&mut self);
    fn initialize_evaluation_buffer(&mut self);
    fn all_assignments_are_constant(&self) -> bool;
    fn foreign_key_checks(&self) -> &[Self::ForeignKeyCheck];
    fn foreign_key_cascades(&self) -> &[Self::ForeignKeyCascade];
}

/// INSERT 执行器：持有运行时并编排插入 / 冲突更新路径。
pub struct InsertExec<R: InsertRuntime> {
    pub runtime: R,
}

impl<R: InsertRuntime> InsertExec<R> {
    /// 执行一批行插入；结束后清理缓冲。
    pub fn exec(&mut self, context: &mut R::Context, rows: Vec<R::Row>) -> Result<(), R::Error> {
        let result = self.exec_inner(context, rows);
        self.runtime.clean_buffers();
        result
    }

    /// 按是否存在 ON DUPLICATE 赋值、IGNORE 等分支选择插入策略。
    fn exec_inner(&mut self, context: &mut R::Context, rows: Vec<R::Row>) -> Result<(), R::Error> {
        let mut transaction = self.runtime.transaction()?;
        self.runtime.set_top_sql_option(&mut transaction);
        if self.runtime.collect_runtime_stats_enabled() {
            self.runtime.begin_snapshot_stats(&mut transaction);
        }
        self.runtime.add_record_rows(rows.len() as u64);
        // ON DUPLICATE → 批更新；IGNORE → 批量检查插入；否则逐行 add_record
        let result = if !self.runtime.on_duplicate_assignments().is_empty() {
            self.batchUpdateDupRows(context, rows, &mut transaction)
        } else if self.runtime.ignore_errors() {
            self.runtime.batch_check_and_insert(context, rows)
        } else {
            let started = std::time::Instant::now();
            let mode = self.runtime.normal_duplicate_key_mode(&transaction);
            let step = self.runtime.shard_allocate_step().max(1);
            let row_count = rows.len();
            let mut result = Ok(());
            // 按分片步长在部分行上带 auto_id hint，减少分配次数
            for (index, row) in rows.into_iter().enumerate() {
                let current = if index % step == 0 {
                    self.runtime.add_record_with_auto_id_hint(
                        context,
                        row,
                        step.min(row_count - index),
                        mode,
                    )
                } else {
                    self.runtime.add_record(context, row, mode)
                };
                if let Err(error) = current {
                    result = Err(error);
                    break;
                }
            }
            self.runtime.record_check_insert_elapsed(started.elapsed());
            result
        };
        let result = match result {
            Ok(()) => self.runtime.may_flush(&mut transaction),
            Err(error) => Err(error),
        };
        self.runtime.end_snapshot_stats(&mut transaction);
        result
    }

    /// 预取待检行的 handle 键与唯一索引键对应的 KV 值。
    pub fn prefetchUniqueIndices(
        &mut self,
        context: &mut R::Context,
        transaction: &mut R::Transaction,
        rows: &[R::CheckedRow],
    ) -> Result<HashMap<R::Key, R::Value>, R::Error> {
        let mut keys = Vec::new();
        for row in rows {
            if self.runtime.checked_row_ignored(row) {
                continue;
            }
            keys.extend(self.runtime.checked_handle_key(row));
            keys.extend(self.runtime.checked_unique_keys(row));
        }
        self.runtime.batch_get(context, transaction, keys)
    }

    /// 根据唯一索引命中值解码 handle，再预取冲突旧行记录键。
    pub fn prefetchConflictedOldRows(
        &mut self,
        context: &mut R::Context,
        transaction: &mut R::Transaction,
        rows: &[R::CheckedRow],
        values: &HashMap<R::Key, R::Value>,
    ) -> Result<(), R::Error> {
        let mut record_keys = Vec::new();
        for row in rows {
            for key in self.runtime.checked_unique_keys(row) {
                if self.runtime.temporary_index_key(&key) {
                    continue;
                }
                if let Some(value) = values.get(&key) {
                    let handle = self.runtime.decode_handle_in_index_value(value)?;
                    record_keys.push(self.runtime.record_key(row, &handle));
                }
            }
        }
        self.runtime
            .batch_get(context, transaction, record_keys)
            .map(|_| ())
    }

    /// 非临时表时预取唯一索引与冲突旧行，填充数据缓存。
    pub fn prefetchDataCache(
        &mut self,
        context: &mut R::Context,
        transaction: &mut R::Transaction,
        rows: &[R::CheckedRow],
    ) -> Result<(), R::Error> {
        if self.runtime.table_is_temporary() {
            return Ok(());
        }
        let values = self.prefetchUniqueIndices(context, transaction, rows)?;
        self.prefetchConflictedOldRows(context, transaction, rows, &values)
    }

    /// ON DUPLICATE KEY UPDATE：先检键并预取，再对冲突行更新，否则插入。
    pub fn batchUpdateDupRows(
        &mut self,
        context: &mut R::Context,
        rows: Vec<R::Row>,
        transaction: &mut R::Transaction,
    ) -> Result<(), R::Error> {
        let started = std::time::Instant::now();
        let checked_rows = self.runtime.keys_need_check(rows)?;
        self.prefetchDataCache(context, transaction, &checked_rows)?;
        let update_mode = self.runtime.update_duplicate_key_mode(transaction);
        let insert_mode = self.runtime.normal_duplicate_key_mode(transaction);
        let auto_column = self.runtime.auto_increment_column();

        for (index, checked_row) in checked_rows.into_iter().enumerate() {
            let mut inserted = true;
            // 优先用行 handle 尝试更新已存在记录
            if let Some(key) = self.runtime.checked_handle_key(&checked_row) {
                let handle = self.runtime.decode_row_key(&key)?;
                match self.runtime.update_duplicate_row(
                    context,
                    index,
                    transaction,
                    &checked_row,
                    handle,
                    update_mode,
                    auto_column,
                ) {
                    Ok(()) => inserted = false,
                    Err(error) if self.runtime.error_is_not_found(&error) => {}
                    Err(error) => return Err(error),
                }
            }
            // handle 未命中时，再按唯一索引查找冲突 handle 并更新
            if inserted {
                for key in self.runtime.checked_unique_keys(&checked_row) {
                    let Some(handle) =
                        self.runtime
                            .fetch_duplicated_handle(context, transaction, &key)?
                    else {
                        continue;
                    };
                    if let Err(error) = self.runtime.update_duplicate_row(
                        context,
                        index,
                        transaction,
                        &checked_row,
                        handle.clone(),
                        update_mode,
                        auto_column,
                    ) {
                        if self.runtime.error_is_not_found(&error) {
                            self.runtime
                                .log_inconsistent_unique_index(&key, &handle, &checked_row);
                        }
                        return Err(error);
                    }
                    inserted = false;
                    break;
                }
            }
            if inserted {
                let row = self.runtime.checked_row_into_row(checked_row);
                self.runtime.add_record(context, row, insert_mode)?;
            }
        }
        self.runtime.record_check_insert_elapsed(started.elapsed());
        Ok(())
    }

    /// 执行器迭代一步：INSERT…SELECT 或普通 INSERT；处理自增读失败。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Request,
    ) -> Result<(), R::Error> {
        self.runtime.reset_request(request);
        self.runtime.enable_rows_column_metric();
        if self.runtime.has_select_executor() {
            self.runtime.insert_rows_from_select(context)
        } else {
            self.runtime.insert_rows(context).map_err(|error| {
                // Rebase errors can bypass InsertValues::handleErr.
                if crate::insert_common::is_terminal_auto_id_error(&error) {
                    return error;
                }
                if self.runtime.error_is_auto_increment_read_failure(&error)
                    && self.runtime.on_duplicate_assignments().is_empty()
                {
                    self.runtime.handle_auto_increment_read_error(error)
                } else {
                    error
                }
            })
        }
    }

    /// 关闭执行器：登记统计、重置内存，并关闭子 SELECT 执行器。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        self.runtime.register_runtime_stats();
        self.runtime.reset_memory_usage();
        self.runtime.set_insert_message();
        if self.runtime.has_select_executor() {
            self.runtime.close_select_executor()
        } else {
            Ok(())
        }
    }

    /// 打开执行器：初始化求值缓冲，必要时打开子 SELECT。
    pub fn Open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        if !self.runtime.on_duplicate_assignments().is_empty() {
            self.runtime.initialize_duplicate_evaluation_buffer();
        }
        if self.runtime.has_select_executor() {
            self.runtime.open_select_executor(context)
        } else {
            if !self.runtime.all_assignments_are_constant() {
                self.runtime.initialize_evaluation_buffer();
            }
            Ok(())
        }
    }

    /// 返回外键检查列表。
    pub fn GetFKChecks(&self) -> &[R::ForeignKeyCheck] {
        self.runtime.foreign_key_checks()
    }

    /// 返回外键级联动作列表。
    pub fn GetFKCascades(&self) -> &[R::ForeignKeyCascade] {
        self.runtime.foreign_key_cascades()
    }

    /// 是否存在外键级联。
    pub fn HasFKCascades(&self) -> bool {
        !self.runtime.foreign_key_cascades().is_empty()
    }
}

/// 普通 INSERT 的重复键检查模式优化：非原地/悲观/流水线则 Lazy。
pub fn optimizeDupKeyCheckForNormalInsert(
    constraint_check_in_place: bool,
    pessimistic: bool,
    pipelined: bool,
) -> DupKeyCheckMode {
    if !constraint_check_in_place || pessimistic || pipelined {
        DupKeyCheckMode::Lazy
    } else {
        DupKeyCheckMode::InPlace
    }
}

/// 选择悲观 Lazy 检查阶段：满足事务条件则 Prewrite，否则 AcquireLock。
pub fn getPessimisticLazyCheckMode(
    constraint_check_in_place_pessimistic: bool,
    in_transaction: bool,
    restricted_sql: bool,
    connection_id: u64,
) -> PessimisticLazyDupKeyCheckMode {
    if !constraint_check_in_place_pessimistic
        && in_transaction
        && !restricted_sql
        && connection_id > 0
    {
        PessimisticLazyDupKeyCheckMode::InPrewrite
    } else {
        PessimisticLazyDupKeyCheckMode::InAcquireLock
    }
}
