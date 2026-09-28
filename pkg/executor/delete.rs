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

// DELETE 语句执行器：按 chunk 删除单表或多表行，并处理外键检查与级联。
//
// 通过 `DeleteRuntime` 抽象子执行器拉取、事务批提交、handle（行标识）构建与
// FK（Foreign Key，外键）校验。支持 batch DML：达到批次大小后提交当前语句事务并开启新事务。
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::hash::Hash;

/// 累加「删除行 × 列」度量：忽略非正增量，溢出时饱和到 `i64::MAX`。
pub fn addDeleteRowsColMultiply(total: i64, delta: i64) -> i64 {
    if delta <= 0 || total == i64::MAX {
        total
    } else {
        total.saturating_add(delta)
    }
}

/// 子执行器返回的待删除数据块及其内存占用估计。
pub struct DeleteChunk<D> {
    pub rows: Vec<Vec<D>>,
    pub memory_usage: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 多表 DELETE 中某表在拼接行中的列区间 `[start, end)`。
pub struct TableColumnPosition {
    pub table_id: i64,
    pub start: usize,
    pub end: usize,
}

/// 待删除行的 handle 取值与其在 positions 数组中的下标。
pub struct HandleInfoPair<D> {
    pub handle_values: Vec<D>,
    pub position_index: usize,
}

/// 多表去重映射：`table_id -> (handle -> 行信息)`，避免同一行删除多次。
pub type TableRowMap<H, D> = HashMap<i64, HashMap<H, HandleInfoPair<D>>>;

/// DELETE 执行所需的运行时边界：子树拉取、事务、FK 与内存记账。
pub trait DeleteRuntime {
    type Context;
    type Request;
    type Datum: Clone;
    type Handle: Clone + Eq + Hash;
    type ForeignKeyCheck;
    type ForeignKeyCascade;
    type Error;

    fn reset_request(&self, request: &mut Self::Request);
    fn is_multi_table(&self) -> bool;
    fn next_child_chunk(
        &mut self,
        context: &mut Self::Context,
    ) -> Result<Option<DeleteChunk<Self::Datum>>, Self::Error>;
    fn consume_memory(&mut self, delta: i64);
    fn reset_memory_usage(&mut self);
    fn may_flush_transaction(&mut self) -> Result<(), Self::Error>;
    fn single_table_id(&self) -> i64;
    fn single_table_has_extra_handle(&self) -> bool;
    fn filter_single_table_row(&self, joined_row: &[Self::Datum]) -> Vec<Self::Datum>;
    fn build_handle(
        &mut self,
        table_id: i64,
        position: Option<&TableColumnPosition>,
        row: &[Self::Datum],
    ) -> Result<Self::Handle, Self::Error>;
    fn multi_table_positions(&self) -> Vec<TableColumnPosition>;
    fn unmatched_outer_row(&self, position: &TableColumnPosition, row: &[Self::Datum]) -> bool;
    fn handle_extra_memory(&self, handle: &Self::Handle) -> i64;
    fn estimated_row_memory(&self, row: &[Self::Datum]) -> i64;

    fn batch_delete_enabled(&self) -> bool;
    fn batch_dml_size(&self) -> usize;
    fn commit_statement(&mut self, context: &mut Self::Context);
    fn new_transaction_in_statement(
        &mut self,
        context: &mut Self::Context,
    ) -> Result<(), Self::Error>;
    fn batch_delete_error(&self, error: Self::Error) -> Self::Error;
    fn record_rows_column_multiply(&mut self, total: i64);

    fn ignore_errors(&self) -> bool;
    fn check_fk_ignore_error(
        &mut self,
        context: &mut Self::Context,
        table_id: i64,
        row: &[Self::Datum],
    ) -> Result<bool, Self::Error>;
    fn remove_record(
        &mut self,
        context: &mut Self::Context,
        table_id: i64,
        handle: &Self::Handle,
        data: &[Self::Datum],
        position: Option<&TableColumnPosition>,
    ) -> Result<(), Self::Error>;
    fn add_affected_rows(&mut self, rows: u64);
    fn foreign_key_delete_checks(
        &mut self,
        table_id: i64,
        data: &[Self::Datum],
    ) -> Result<(), Self::Error>;
    fn foreign_key_delete_cascades(
        &mut self,
        table_id: i64,
        data: &[Self::Datum],
    ) -> Result<(), Self::Error>;
    fn all_foreign_key_checks(&self) -> Vec<&Self::ForeignKeyCheck>;
    fn all_foreign_key_cascades(&self) -> Vec<&Self::ForeignKeyCascade>;

    fn open_child(&mut self, context: &mut Self::Context) -> Result<(), Self::Error>;
    fn close_child(&mut self) -> Result<(), Self::Error>;
}

/// DELETE 执行器，持有具体 `DeleteRuntime` 实现。
pub struct DeleteExec<R: DeleteRuntime> {
    pub runtime: R,
}

impl<R: DeleteRuntime> DeleteExec<R> {
    /// 入口：按单表/多表路径删除子树产出的全部行。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Request,
    ) -> Result<(), R::Error> {
        self.runtime.reset_request(request);
        if self.runtime.is_multi_table() {
            self.deleteMultiTablesByChunk(context)
        } else {
            self.deleteSingleTableByChunk(context)
        }
    }

    /// 删除单行：构建 handle 并调用 `removeRow`（可忽略末尾 extra handle 列）。
    pub fn deleteOneRow(
        &mut self,
        context: &mut R::Context,
        table_id: i64,
        position: Option<&TableColumnPosition>,
        extra_handle: bool,
        row: &[R::Datum],
    ) -> Result<(), R::Error> {
        let end = row.len() - usize::from(extra_handle);
        let handle = self.runtime.build_handle(table_id, position, row)?;
        self.removeRow(context, table_id, &handle, &row[..end], position)
    }

    /// 单表路径：按 chunk 迭代删除，可选 batch 提交。
    pub fn deleteSingleTableByChunk(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        let table_id = self.runtime.single_table_id();
        let extra_handle = self.runtime.single_table_has_extra_handle();
        let batch_delete = self.runtime.batch_delete_enabled() && self.runtime.batch_dml_size() > 0;
        let batch_size = self.runtime.batch_dml_size();
        let mut row_count = 0;
        let mut previous_chunk_memory = 0;
        // 按 chunk 拉取；释放上一块内存后再计入当前块
        let mut rows_column_multiply = 0;

        loop {
            self.runtime.consume_memory(-previous_chunk_memory);
            let Some(chunk) = self.runtime.next_child_chunk(context)? else {
                break;
            };
            previous_chunk_memory = chunk.memory_usage;
            self.runtime.consume_memory(previous_chunk_memory);
            for joined_row in chunk.rows {
                // 达到 batch 大小则提交并开启新事务
                if batch_delete && row_count >= batch_size {
                    self.runtime
                        .record_rows_column_multiply(rows_column_multiply);
                    rows_column_multiply = 0;
                    self.doBatchDelete(context)?;
                    row_count = 0;
                }
                let row = self.runtime.filter_single_table_row(&joined_row);
                // IGNORE 模式下外键冲突则跳过该行
                if self.runtime.ignore_errors()
                    && self
                        .runtime
                        .check_fk_ignore_error(context, table_id, &row)?
                {
                    continue;
                }
                let column_count = row.len() - usize::from(extra_handle);
                self.deleteOneRow(context, table_id, None, extra_handle, &row)?;
                rows_column_multiply =
                    addDeleteRowsColMultiply(rows_column_multiply, column_count as i64);
                row_count += 1;
            }
            self.runtime.may_flush_transaction()?;
        }
        self.runtime
            .record_rows_column_multiply(rows_column_multiply);
        Ok(())
    }

    /// 提交当前语句事务并在语句内开启新事务（batch DELETE）。
    pub fn doBatchDelete(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.runtime.commit_statement(context);
        self.runtime
            .new_transaction_in_statement(context)
            .map_err(|error| self.runtime.batch_delete_error(error))
    }

    /// 将拼接行拆入各表的 handle 映射，跳过未匹配的外连接空行。
    pub fn composeTblRowMap(
        &mut self,
        table_rows: &mut TableRowMap<R::Handle, R::Datum>,
        positions: &[TableColumnPosition],
        joined_row: &[R::Datum],
    ) -> Result<(), R::Error> {
        let mut memory_delta = 0;
        for (position_index, position) in positions.iter().enumerate() {
            if self.runtime.unmatched_outer_row(position, joined_row) {
                continue;
            }
            let handle =
                self.runtime
                    .build_handle(position.table_id, Some(position), joined_row)?;
            let table_map = table_rows.entry(position.table_id).or_default();
            let existed = table_map.contains_key(&handle);
            table_map.insert(
                handle.clone(),
                HandleInfoPair {
                    handle_values: joined_row[position.start..position.end].to_vec(),
                    position_index,
                },
            );
            if !existed {
                memory_delta += self.runtime.estimated_row_memory(joined_row);
                memory_delta += self.runtime.handle_extra_memory(&handle);
            }
        }
        self.runtime.consume_memory(memory_delta);
        Ok(())
    }

    /// 多表路径：先收集去重后的待删行，再统一删除。
    pub fn deleteMultiTablesByChunk(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        let positions = self.runtime.multi_table_positions();
        let mut table_rows = HashMap::new();
        let mut previous_chunk_memory = 0;
        loop {
            self.runtime.consume_memory(-previous_chunk_memory);
            let Some(chunk) = self.runtime.next_child_chunk(context)? else {
                break;
            };
            previous_chunk_memory = chunk.memory_usage;
            self.runtime.consume_memory(previous_chunk_memory);
            for joined_row in chunk.rows {
                self.composeTblRowMap(&mut table_rows, &positions, &joined_row)?;
            }
            self.runtime.may_flush_transaction()?;
        }
        self.removeRowsInTblRowMap(context, table_rows, &positions)
    }

    /// 遍历 `TableRowMap` 逐行删除并累计列乘积度量。
    pub fn removeRowsInTblRowMap(
        &mut self,
        context: &mut R::Context,
        table_rows: TableRowMap<R::Handle, R::Datum>,
        positions: &[TableColumnPosition],
    ) -> Result<(), R::Error> {
        let mut rows_column_multiply = 0;
        for (table_id, rows) in table_rows {
            for (handle, pair) in rows {
                if self.runtime.ignore_errors()
                    && self
                        .runtime
                        .check_fk_ignore_error(context, table_id, &pair.handle_values)?
                {
                    continue;
                }
                self.removeRow(
                    context,
                    table_id,
                    &handle,
                    &pair.handle_values,
                    Some(&positions[pair.position_index]),
                )?;
                rows_column_multiply =
                    addDeleteRowsColMultiply(rows_column_multiply, pair.handle_values.len() as i64);
            }
        }
        self.runtime
            .record_rows_column_multiply(rows_column_multiply);
        Ok(())
    }

    /// 删除存储记录、触发 FK 检查/级联，并增加 affected rows。
    pub fn removeRow(
        &mut self,
        context: &mut R::Context,
        table_id: i64,
        handle: &R::Handle,
        data: &[R::Datum],
        position: Option<&TableColumnPosition>,
    ) -> Result<(), R::Error> {
        self.runtime
            .remove_record(context, table_id, handle, data, position)?;
        onRemoveRowForFK(&mut self.runtime, table_id, data)?;
        self.runtime.add_affected_rows(1);
        Ok(())
    }

    /// 关闭子执行器并复位内存记账。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        let result = self.runtime.close_child();
        self.runtime.reset_memory_usage();
        result
    }

    /// 打开子执行器。
    pub fn Open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.runtime.open_child(context)
    }

    /// 返回所有外键检查器。
    pub fn GetFKChecks(&self) -> Vec<&R::ForeignKeyCheck> {
        self.runtime.all_foreign_key_checks()
    }

    /// 返回所有外键级联动作。
    pub fn GetFKCascades(&self) -> Vec<&R::ForeignKeyCascade> {
        self.runtime.all_foreign_key_cascades()
    }

    /// 是否存在外键级联。
    pub fn HasFKCascades(&self) -> bool {
        !self.runtime.all_foreign_key_cascades().is_empty()
    }
}

/// 删除行后的外键收尾：非 ignore 时先检查，再执行级联。
pub fn onRemoveRowForFK<R: DeleteRuntime>(
    runtime: &mut R,
    table_id: i64,
    data: &[R::Datum],
) -> Result<(), R::Error> {
    if !runtime.ignore_errors() {
        runtime.foreign_key_delete_checks(table_id, data)?;
    }
    runtime.foreign_key_delete_cascades(table_id, data)
}
