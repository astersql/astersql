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

// REPLACE 执行器：冲突时先删后插，实现“替换写入”。
//
// 对应 Go replace executor：通过 `ReplaceRuntime` 解耦事务/唯一键/外键细节；
// `ReplaceExec` 在 handle 或唯一索引冲突时删除旧行，再 `add_record` 写入新行；
// 支持常量行与 SELECT→REPLACE 两条路径（Open/Next/Close）。

#![allow(non_snake_case)]

use std::time::Duration;

use astersql_util_chunk::Chunk;

/// REPLACE 运行时能力边界（事务、唯一键探测、删行、插入、预取、外键等）。
pub trait ReplaceRuntime {
    type Context;
    type Row;
    type CheckedRow;
    type Handle;
    type Transaction;
    type DuplicateKeyCheckMode;
    type ForeignKeyCheck;
    type ForeignKeyCascade;
    type Error;

    fn attach_memory_tracker(&mut self);
    fn open_select(&mut self, ctx: &mut Self::Context) -> Result<(), Self::Error>;
    fn close_select(&mut self) -> Result<(), Self::Error>;
    fn has_select_executor(&self) -> bool;
    fn initialize_evaluation_buffer(&mut self);
    fn register_runtime_stats(&mut self);
    fn transaction(&mut self) -> Result<Self::Transaction, Self::Error>;
    fn handle_key(&self, row: &Self::CheckedRow) -> Option<Vec<u8>>;
    fn unique_keys(&self, row: &Self::CheckedRow) -> Vec<Vec<u8>>;
    fn decode_row_key(&self, key: &[u8]) -> Result<Self::Handle, Self::Error>;
    fn transaction_get(
        &mut self,
        ctx: &mut Self::Context,
        transaction: &mut Self::Transaction,
        key: &[u8],
    ) -> Result<bool, Self::Error>;
    fn error_is_not_found(&self, error: &Self::Error) -> bool;
    fn fetch_duplicated_handle(
        &mut self,
        ctx: &mut Self::Context,
        transaction: &mut Self::Transaction,
        key: &[u8],
    ) -> Result<Option<Self::Handle>, Self::Error>;
    fn remove_row(
        &mut self,
        ctx: &mut Self::Context,
        transaction: &mut Self::Transaction,
        handle: Self::Handle,
        row: &Self::CheckedRow,
    ) -> Result<bool, Self::Error>;
    fn add_record(
        &mut self,
        ctx: &mut Self::Context,
        row: &Self::CheckedRow,
        duplicate_check: &Self::DuplicateKeyCheckMode,
    ) -> Result<(), Self::Error>;
    fn keys_need_check(
        &mut self,
        rows: Vec<Self::Row>,
    ) -> Result<Vec<Self::CheckedRow>, Self::Error>;
    fn begin_snapshot_runtime_stats(&mut self, transaction: &mut Self::Transaction);
    fn end_snapshot_runtime_stats(&mut self, transaction: &mut Self::Transaction);
    fn set_top_sql_option(&mut self, transaction: &mut Self::Transaction);
    fn prefetch_data_cache(
        &mut self,
        ctx: &mut Self::Context,
        transaction: &mut Self::Transaction,
        rows: &[Self::CheckedRow],
    ) -> Result<(), Self::Error>;
    fn set_prefetch_duration(&mut self, duration: Duration);
    fn add_record_rows(&mut self, rows: u64);
    /// Start a fresh processed-write accounting lifecycle when runtime stats are enabled.
    fn reset_write_runtime_stats(&mut self);
    /// Charge processed target rows using the runtime-owned table/index metadata.
    fn record_write_cpu_work(&mut self, rows: usize);
    fn optimize_duplicate_key_check(
        &self,
        transaction: &Self::Transaction,
    ) -> Self::DuplicateKeyCheckMode;
    fn may_flush(&mut self, transaction: &mut Self::Transaction) -> Result<(), Self::Error>;
    fn collect_runtime_stats_enabled(&self) -> bool;
    fn reset_output_chunk(&self, chunk: &mut Chunk);
    fn enable_ruv2_rows_column_metric(&mut self);
    fn insert_rows_from_select(&mut self, ctx: &mut Self::Context) -> Result<(), Self::Error>;
    fn insert_rows(&mut self, ctx: &mut Self::Context) -> Result<(), Self::Error>;
    fn has_child_executor(&self) -> bool;
    fn record_rows(&self) -> u64;
    fn warning_count(&self) -> u64;
    fn affected_rows(&self) -> u64;
    fn set_statement_message(&mut self, message: String);
    fn foreign_key_checks(&self) -> &[Self::ForeignKeyCheck];
    fn foreign_key_cascades(&self) -> &[Self::ForeignKeyCascade];
}

/// REPLACE 执行器：持有运行时与语句优先级。
pub struct ReplaceExec<R: ReplaceRuntime> {
    pub runtime: R,
    pub priority: i32,
}

impl<R: ReplaceRuntime> ReplaceExec<R> {
    /// 收尾：设置客户端消息、登记运行时统计，并关闭可选的 SELECT 子执行器。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        self.setMessage();
        let result = if self.runtime.has_select_executor() {
            self.runtime.close_select()
        } else {
            Ok(())
        };
        // Go registers the statistics in a defer, so child close runs first and
        // registration still happens when closing the child returns an error.
        self.runtime.register_runtime_stats();
        result
    }

    /// 打开：挂接内存追踪；有 SELECT 则 open_select，否则初始化求值缓冲。
    pub fn Open(&mut self, ctx: &mut R::Context) -> Result<(), R::Error> {
        self.runtime.reset_write_runtime_stats();
        self.runtime.attach_memory_tracker();
        if self.runtime.has_select_executor() {
            self.runtime.open_select(ctx)
        } else {
            self.runtime.initialize_evaluation_buffer();
            Ok(())
        }
    }

    /// 替换单行：若主键 handle 已存在则删旧行；再清唯一索引冲突后插入新行。
    pub fn replaceRow(
        &mut self,
        ctx: &mut R::Context,
        transaction: &mut R::Transaction,
        row: &R::CheckedRow,
        duplicate_check: &R::DuplicateKeyCheckMode,
    ) -> Result<(), R::Error> {
        // 有显式 handle 键时：探测事务中是否已有该行，有则先 remove_row。
        if let Some(handle_key) = self.runtime.handle_key(row) {
            let handle = self.runtime.decode_row_key(&handle_key)?;
            match self.runtime.transaction_get(ctx, transaction, &handle_key) {
                Ok(true) => {
                    if self.runtime.remove_row(ctx, transaction, handle, row)? {
                        return Ok(());
                    }
                }
                Ok(false) => {}
                Err(error) if self.runtime.error_is_not_found(&error) => {}
                Err(error) => return Err(error),
            }
        }

        // 循环清除唯一索引上的冲突行，直到无重复或发现行未变化可短路。
        loop {
            let (row_unchanged, duplicate_found) = self.removeIndexRow(ctx, transaction, row)?;
            if row_unchanged {
                return Ok(());
            }
            if !duplicate_found {
                break;
            }
        }
        self.runtime.add_record(ctx, row, duplicate_check)
    }

    /// 扫描行的唯一键，找到重复 handle 则删除对应旧行；返回 (行未变, 发现重复)。
    pub fn removeIndexRow(
        &mut self,
        ctx: &mut R::Context,
        transaction: &mut R::Transaction,
        row: &R::CheckedRow,
    ) -> Result<(bool, bool), R::Error> {
        for key in self.runtime.unique_keys(row) {
            let Some(handle) = self
                .runtime
                .fetch_duplicated_handle(ctx, transaction, &key)?
            else {
                continue;
            };
            let unchanged = self.runtime.remove_row(ctx, transaction, handle, row)?;
            return Ok((unchanged, true));
        }
        Ok((false, false))
    }

    /// 批量 REPLACE：约束检查 → 预取缓存 → 逐行 replaceRow → 可选 flush。
    pub fn exec(&mut self, ctx: &mut R::Context, rows: Vec<R::Row>) -> Result<(), R::Error> {
        let processed_rows = rows.len();
        let record_rows = processed_rows as u64;
        // 将输入行转为需做唯一键检查的 CheckedRow。
        let checked_rows = self.runtime.keys_need_check(rows)?;
        let mut transaction = self.runtime.transaction()?;
        let collect_runtime_stats = self.runtime.collect_runtime_stats_enabled();
        if collect_runtime_stats {
            self.runtime.begin_snapshot_runtime_stats(&mut transaction);
        }
        self.runtime.set_top_sql_option(&mut transaction);
        // 预取相关 KV 到缓存，减少逐行探测往返。
        let prefetch_started = std::time::Instant::now();
        let result = self
            .runtime
            .prefetch_data_cache(ctx, &mut transaction, &checked_rows);
        self.runtime
            .set_prefetch_duration(prefetch_started.elapsed());
        if let Err(error) = result {
            if collect_runtime_stats {
                self.runtime.end_snapshot_runtime_stats(&mut transaction);
            }
            return Err(error);
        }

        self.runtime.add_record_rows(record_rows);
        self.runtime.record_write_cpu_work(processed_rows);
        let duplicate_check = self.runtime.optimize_duplicate_key_check(&transaction);
        for row in &checked_rows {
            if let Err(error) = self.replaceRow(ctx, &mut transaction, row, &duplicate_check) {
                if collect_runtime_stats {
                    self.runtime.end_snapshot_runtime_stats(&mut transaction);
                }
                return Err(error);
            }
        }
        let result = self.runtime.may_flush(&mut transaction);
        if collect_runtime_stats {
            self.runtime.end_snapshot_runtime_stats(&mut transaction);
        }
        result
    }

    /// 驱动一次产出：有子执行器则从 SELECT 插入，否则插入常量行。
    pub fn Next(&mut self, ctx: &mut R::Context, request: &mut Chunk) -> Result<(), R::Error> {
        self.runtime.reset_output_chunk(request);
        self.runtime.enable_ruv2_rows_column_metric();
        if self.runtime.has_child_executor() {
            self.runtime.insert_rows_from_select(ctx)
        } else {
            self.runtime.insert_rows(ctx)
        }
    }

    /// 组装客户端 Records/Duplicates/Warnings 提示（多行或 SELECT 路径时）。
    pub fn setMessage(&mut self) {
        let records = self.runtime.record_rows();
        if self.runtime.has_select_executor() || records > 1 {
            let duplicates = self.runtime.affected_rows().saturating_sub(records);
            let warnings = self.runtime.warning_count();
            self.runtime.set_statement_message(format!(
                "Records: {}  Duplicates: {}  Warnings: {}",
                records, duplicates, warnings
            ));
        }
    }

    /// 返回外键检查（Foreign Key Check）列表。
    pub fn GetFKChecks(&self) -> &[R::ForeignKeyCheck] {
        self.runtime.foreign_key_checks()
    }

    /// 返回外键级联（Foreign Key Cascade）列表。
    pub fn GetFKCascades(&self) -> &[R::ForeignKeyCascade] {
        self.runtime.foreign_key_cascades()
    }

    /// 是否配置了外键级联动作。
    pub fn HasFKCascades(&self) -> bool {
        !self.runtime.foreign_key_cascades().is_empty()
    }
}
