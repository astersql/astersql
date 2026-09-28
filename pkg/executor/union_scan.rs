// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// UnionScan 执行器：合并事务内脏行与快照扫描流。
//
// 在悲观/乐观事务（transaction）中，尚未提交的写入位于内存脏表（dirty table）。
// UnionScan 将脏表新增行与底层 Snapshot（MVCC 快照读）结果归并输出，
// 并跳过已被本事务修改过的快照行，保证读己之写（read-your-writes）。

#![allow(non_camel_case_types, non_snake_case)]

use std::cmp::Ordering;

use astersql_util_chunk::Chunk;

/// 运行时边界：打开子执行器、拉取脏行/快照行、比较与投影条件求值。
pub trait UnionScanRuntime {
    type Context;
    type Row: Clone;
    type Error;

    /// 打开底层子执行器（表/索引扫描等）。
    fn open_base(&mut self, ctx: &mut Self::Context) -> Result<(), Self::Error>;
    /// 构建事务脏表（dirty table）新增行迭代器。
    fn build_added_rows_iterator(&mut self, ctx: &mut Self::Context) -> Result<(), Self::Error>;
    /// 物理表 ID 列下标（分区表场景）；无则 None。
    fn physical_table_id_column(&self) -> Option<usize>;
    /// 分配用于拉取快照批次的 Chunk。
    fn new_snapshot_chunk(&mut self) -> Chunk;
    /// 输出 Chunk 最大行数上限。
    fn maximum_chunk_size(&self) -> usize;
    /// 重置输出 Chunk 容量。
    fn reset_output_chunk(&self, chunk: &mut Chunk, maximum_size: usize);
    /// 当前输出 Chunk 可写入容量。
    fn output_capacity(&self, chunk: &Chunk) -> usize;
    /// 当前输出 Chunk 已有行数。
    fn output_rows(&self, chunk: &Chunk) -> usize;
    /// 取下一条脏表新增行。
    fn next_added_row(&mut self) -> Result<Option<Self::Row>, Self::Error>;
    /// 从子扫描拉取下一批快照行。
    fn next_snapshot_rows(
        &mut self,
        ctx: &mut Self::Context,
        chunk: &mut Chunk,
    ) -> Result<Vec<Self::Row>, Self::Error>;
    /// 快照行是否已被本事务脏表修改（应被遮蔽）。
    fn snapshot_row_was_modified(
        &self,
        row: &Self::Row,
        physical_table_id_column: Option<usize>,
    ) -> Result<bool, Self::Error>;
    /// 比较两行键序（用于归并）。
    fn compare_rows(&self, left: &Self::Row, right: &Self::Row) -> Result<Ordering, Self::Error>;
    /// 计算虚拟列并应用过滤条件；不满足则返回 None。
    fn evaluate_virtual_columns_and_conditions(
        &mut self,
        row: Self::Row,
    ) -> Result<Option<Self::Row>, Self::Error>;
    /// 将行追加到输出 Chunk。
    fn append_row(&self, chunk: &mut Chunk, row: &Self::Row);
    /// 是否在读缓存表（此时跳过快照侧）。
    fn reading_cached_table(&self) -> bool;
    /// 关闭脏表行迭代器。
    fn close_added_rows_iterator(&mut self);
    /// 关闭子执行器。
    fn close_child(&mut self) -> Result<(), Self::Error>;
}

/// Merges transaction-local dirty rows with the snapshot stream.
/// 合并事务本地脏行与快照扫描流的执行器。
pub struct UnionScanExec<R: UnionScanRuntime> {
    /// 运行时依赖（子执行器、脏表迭代等）。
    pub runtime: R,
    /// 当前待合并的脏表行（预取缓存）。
    pub added_row: Option<R::Row>,
    /// 当前快照批次中尚未消费的行缓冲。
    pub snapshot_rows: Vec<R::Row>,
    /// `snapshot_rows` 内游标。
    pub snapshot_cursor: usize,
    /// 向子扫描拉取快照批次时复用的 Chunk。
    pub snapshot_chunk: Option<Chunk>,
    /// 物理表 ID 列下标（分区表场景用于判定脏行遮蔽）。
    pub physical_table_id_column: Option<usize>,
}

impl<R: UnionScanRuntime> UnionScanExec<R> {
    /// 打开子执行器并初始化脏行迭代器与快照 Chunk。
    pub fn Open(&mut self, ctx: &mut R::Context) -> Result<(), R::Error> {
        self.runtime.open_base(ctx)?;
        self.open(ctx)
    }

    /// 初始化物理表列、脏行迭代器与快照 Chunk（Open 内部路径）。
    pub fn open(&mut self, ctx: &mut R::Context) -> Result<(), R::Error> {
        self.physical_table_id_column = self.runtime.physical_table_id_column();
        self.runtime.build_added_rows_iterator(ctx)?;
        self.snapshot_chunk = Some(self.runtime.new_snapshot_chunk());
        Ok(())
    }

    /// 拉取一批合并后的行写入 request Chunk，直到满批或两端皆空。
    pub fn Next(&mut self, ctx: &mut R::Context, request: &mut Chunk) -> Result<(), R::Error> {
        let maximum_size = self.runtime.maximum_chunk_size();
        self.runtime.reset_output_chunk(request, maximum_size);
        let batch_size = self.runtime.output_capacity(request);
        while self.runtime.output_rows(request) < batch_size {
            let Some(row) = self.getOneRow(ctx)? else {
                return Ok(());
            };
            // 虚拟列求值与过滤条件；不满足谓词则丢弃该行。
            if let Some(row) = self.runtime.evaluate_virtual_columns_and_conditions(row)? {
                self.runtime.append_row(request, &row);
            }
        }
        Ok(())
    }

    /// 关闭脏行迭代器与子执行器，并清空本地缓冲。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        self.added_row = None;
        self.snapshot_cursor = 0;
        self.snapshot_rows.clear();
        self.runtime.close_added_rows_iterator();
        self.runtime.close_child()
    }

    /// 从快照流与脏表流中取较小的一行（归并排序一步）。
    pub fn getOneRow(&mut self, ctx: &mut R::Context) -> Result<Option<R::Row>, R::Error> {
        let snapshot = self.getSnapshotRow(ctx)?;
        let added = self.getAddedRow()?;
        match (snapshot, added) {
            (None, None) => Ok(None),
            (Some(row), None) => {
                self.snapshot_cursor += 1;
                Ok(Some(row))
            }
            (None, Some(row)) => {
                self.added_row = None;
                Ok(Some(row))
            }
            (Some(snapshot), Some(added)) => {
                // 键序更小的一侧先输出；相等时优先脏行（覆盖快照）。
                if self.runtime.compare_rows(&snapshot, &added)? == Ordering::Less {
                    self.snapshot_cursor += 1;
                    Ok(Some(snapshot))
                } else {
                    self.added_row = None;
                    Ok(Some(added))
                }
            }
        }
    }

    /// 取得下一条未被脏表遮蔽的快照行；必要时向子扫描再拉一批。
    pub fn getSnapshotRow(&mut self, ctx: &mut R::Context) -> Result<Option<R::Row>, R::Error> {
        // 缓存表场景不再读底层快照，仅依赖脏表侧。
        if self.runtime.reading_cached_table() {
            return Ok(None);
        }
        if let Some(row) = self.snapshot_rows.get(self.snapshot_cursor) {
            return Ok(Some(row.clone()));
        }

        self.snapshot_cursor = 0;
        self.snapshot_rows.clear();
        while self.snapshot_rows.is_empty() {
            let rows = self.runtime.next_snapshot_rows(
                ctx,
                self.snapshot_chunk
                    .as_mut()
                    .expect("UnionScan snapshot chunk must be initialized by Open"),
            )?;
            if rows.is_empty() {
                return Ok(None);
            }
            for row in rows {
                // A dirty-table key shadows the same snapshot row. A conflicting
                // inserted row is left to commit-time consistency checks, as Go.
                // 脏表键遮蔽同键快照行；冲突插入留给提交期一致性检查（与 Go 一致）。
                if !self
                    .runtime
                    .snapshot_row_was_modified(&row, self.physical_table_id_column)?
                {
                    self.snapshot_rows.push(row);
                }
            }
        }
        Ok(self.snapshot_rows.first().cloned())
    }

    /// 预取并返回当前脏表行（无则向迭代器再取一条）。
    pub fn getAddedRow(&mut self) -> Result<Option<R::Row>, R::Error> {
        if self.added_row.is_none() {
            self.added_row = self.runtime.next_added_row()?;
        }
        Ok(self.added_row.clone())
    }
}

/// 按索引列与 handle 比较两行的辅助结构（UnionScan/索引归并共用）。
pub struct compareExec<C> {
    /// 各列的排序规则（collator）。
    pub collators: Vec<C>,
    /// 参与比较的索引列下标序列。
    pub used_index: Vec<usize>,
    /// 是否降序（翻转比较结果）。
    pub descending: bool,
    /// 是否需要额外排序（保留字段，与 Go 对齐）。
    pub need_extra_sorting: bool,
}

impl<C> compareExec<C> {
    /// 先按 `used_index` 列比较，全相等再比较 handle；降序时取反。
    pub fn compare<D, E>(
        &self,
        left: &[D],
        right: &[D],
        mut compare_column: impl FnMut(usize, &D, &D, &C) -> Result<Ordering, E>,
        compare_handle: impl FnOnce(&[D], &[D], &[C]) -> Result<Ordering, E>,
    ) -> Result<Ordering, E> {
        for &column in &self.used_index {
            let order = compare_column(
                column,
                &left[column],
                &right[column],
                &self.collators[column],
            )?;
            if order != Ordering::Equal {
                return Ok(if self.descending {
                    order.reverse()
                } else {
                    order
                });
            }
        }
        // 索引列全相等：用 handle（行唯一标识）打破平局。
        let order = compare_handle(left, right, &self.collators)?;
        Ok(if self.descending {
            order.reverse()
        } else {
            order
        })
    }
}
