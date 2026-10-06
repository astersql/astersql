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

// 流水线窗口执行器。
//
// 不必缓冲整个分区即可逐步产出结果：按当前行计算帧起止，滑动更新窗口函数状态，
// 并及时丢弃不再需要的前缀行以控制内存。`OrderedWindowExec` 是其有序包装。

use std::collections::VecDeque;
use std::mem::size_of;

use crate::window::{
    BoundType, ChildExecutor, Chunk, ExecContext, FrameBound, GroupChecker, OrderBy, Result, Row,
    Value, WindowFunction, WindowMemoryTracker, reset_partial_result_and_release_memory,
    reset_partial_results_and_release_memory, update_partial_result_and_track_memory,
};

/// 缓存的子计划 Chunk 元信息：剩余待产出行数与累计行水位。
pub struct DataInfo {
    /// 已投影输入列、待追加窗口结果列的数据块。
    pub chunk: Chunk,
    /// 该块中尚未为窗口结果填充的行数。
    pub remaining: u64,
    /// 读到该块末尾时的全局累计行数（用于判断可丢弃水位）。
    pub accumulated: u64,
}

/// 流水线窗口执行器状态机。
pub struct PipelinedWindowExec {
    /// 子执行器（已按分区/排序输出）。
    pub child: Box<dyn ChildExecutor>,
    /// 输入列数（不含窗口结果列）。
    pub input_columns: usize,
    /// 窗口函数实例。
    pub window_functions: Vec<Box<dyn WindowFunction>>,
    /// 帧起点边界。
    pub start: FrameBound,
    /// 帧终点边界。
    pub end: FrameBound,
    /// 分区边界检测器。
    pub group_checker: GroupChecker,
    /// 最近一次从子计划拉取的原始块。
    pub child_result: Option<Chunk>,
    /// 待产出结果块队列。
    pub data: VecDeque<DataInfo>,
    /// 当前正在 produce 的块下标。
    pub data_index: isize,
    /// 子计划是否已读尽。
    pub done: bool,
    /// 已从子计划累计读取的行数。
    pub accumulated: u64,
    /// 已丢弃的前缀行数（相对全局水位）。
    pub dropped: u64,
    /// 本轮从当前分区还需并入 rows 缓冲的行数。
    pub rows_to_consume: u64,
    /// 是否刚跨入新分区，需要 finish/reset。
    pub new_partition: bool,
    /// 当前分区内正在计算的行下标。
    pub current_row: u64,
    /// 上一行帧起点（滑动优化）。
    pub last_start_row: u64,
    /// 上一行帧终点。
    pub last_end_row: u64,
    /// RANGE 帧探测到的候选起点。
    pub staged_start_row: u64,
    /// RANGE 帧探测到的候选终点。
    pub staged_end_row: u64,
    /// `rows` 缓冲对应的全局起点偏移。
    pub row_start: u64,
    /// ORDER BY，供 RANGE 边界比较。
    pub order_by: Vec<OrderBy>,
    /// 当前分区已缓冲、尚未丢弃的行。
    pub rows: Vec<Row>,
    /// 当前分区已纳入的行总数（含已丢弃前缀的逻辑长度）。
    pub row_count: u64,
    /// 当前分区数据是否已完整（可对尾部行放心产出）。
    pub whole: bool,
    /// 是否为 RANGE 帧（否则按 ROWS 偏移）。
    pub range_frame: bool,
    /// 上一结果是否来自空帧（避免重复 reset）。
    pub empty_frame: bool,
    /// 滑动窗口状态是否已初始化。
    pub initialized_sliding_window: bool,
    pub memory_tracker: WindowMemoryTracker,
    pub(crate) data_memory: i64,
    pub(crate) rows_memory: i64,
}

/// 有序窗口：内部委托 PipelinedWindowExec。
pub struct OrderedWindowExec {
    /// 底层流水线执行器。
    pub inner: PipelinedWindowExec,
}

impl OrderedWindowExec {
    /// 打开有序窗口执行器。
    pub fn open(&mut self, context: &ExecContext) -> Result<()> {
        self.inner.open(context)
    }

    /// 拉取下一批窗口结果。
    pub fn next(&mut self, context: &ExecContext, output: &mut Chunk) -> Result<()> {
        self.inner.next(context, output)
    }

    /// 关闭有序窗口执行器。
    pub fn close(&mut self) -> Result<()> {
        self.inner.close()
    }
}

impl PipelinedWindowExec {
    /// 打开子执行器并复位自身状态。
    pub fn open(&mut self, context: &ExecContext) -> Result<()> {
        self.child.open(context)?;
        self.memory_tracker.open(&context.statement_memory_tracker);
        self.open_self()
    }

    /// 关闭子执行器。
    pub fn close(&mut self) -> Result<()> {
        self.child_result = None;
        self.data.clear();
        self.rows.clear();
        self.group_checker.reset();
        self.data_memory = 0;
        self.rows_memory = 0;
        self.memory_tracker.close();
        self.child.close()
    }

    /// 复位分区/滑动窗口相关状态，并 reset 所有窗口函数。
    pub fn open_self(&mut self) -> Result<()> {
        // `open` always establishes the statement parent before resetting state.
        self.done = false;
        self.new_partition = false;
        self.whole = false;
        self.initialized_sliding_window = false;
        self.data_index = 0;
        self.current_row = 0;
        self.dropped = 0;
        self.rows_to_consume = 0;
        self.accumulated = 0;
        self.last_start_row = 0;
        self.last_end_row = 0;
        self.staged_start_row = 0;
        self.staged_end_row = 0;
        self.row_start = 0;
        self.row_count = 0;
        self.rows.clear();
        self.data.clear();
        self.child_result = None;
        self.group_checker.reset();
        self.data_memory = 0;
        self.rows_memory = 0;
        for function in &mut self.window_functions {
            function.reset();
        }
        Ok(())
    }

    /// 队首结果块是否尚未准备好弹出（仍有剩余行或水位未到）。
    pub fn first_result_chunk_not_ready(&self) -> bool {
        if !self.done && self.data.is_empty() {
            return true;
        }
        self.data
            .front()
            .is_some_and(|first| first.remaining != 0 || first.accumulated > self.dropped)
    }

    /// 主循环：补齐分区数据、produce 窗口值，再弹出队首块。
    pub fn next(&mut self, context: &ExecContext, output: &mut Chunk) -> Result<()> {
        output.reset();
        // 直到队首块可弹出：必要时拉分区行、结束分区、produce。
        while self.first_result_chunk_not_ready() {
            let mut enough = self.enough_to_produce()?;
            if !enough {
                if !self.done && self.rows_to_consume == 0 {
                    self.get_rows_in_partition(context)?;
                }
                if self.done || self.new_partition {
                    self.finish();
                    enough = self.enough_to_produce()?;
                    if enough {
                        continue;
                    }
                    self.new_partition = false;
                    self.reset_partition();
                    if self.rows_to_consume == 0 {
                        break;
                    }
                }
                self.row_count += self.rows_to_consume;
                self.rows_to_consume = 0;
            }

            let index = self.data_index as usize;
            if index < self.data.len() && self.data[index].remaining != 0 {
                let remaining = self.data[index].remaining;
                let old_chunk_memory = self.data[index].chunk.memory_usage();
                let produced = self.produce(index, remaining);
                self.memory_tracker
                    .consume(self.data[index].chunk.memory_usage() - old_chunk_memory);
                let produced = produced?;
                self.data[index].remaining -= produced;
                if self.data[index].remaining == 0 {
                    self.data_index += 1;
                }
            }
        }
        if let Some(mut first) = self.data.pop_front() {
            self.memory_tracker.consume(-first.chunk.memory_usage());
            output.swap_columns(&mut first.chunk);
            self.data_index -= 1;
            if self.data.is_empty() {
                self.memory_tracker.consume(-self.data_memory);
                self.data_memory = 0;
            }
        }
        Ok(())
    }

    /// 从当前分区切下一组行并入 `rows` 缓冲。
    pub fn get_rows_in_partition(&mut self, context: &ExecContext) -> Result<()> {
        self.new_partition = true;
        if self.rows.is_empty() {
            self.new_partition = false;
        }
        if self.group_checker.is_exhausted() {
            if self.fetch_child(context)? {
                self.done = true;
                return Ok(());
            }
            let same_partition = self
                .group_checker
                .split_into_groups(self.child_result.as_ref().unwrap())?;
            if same_partition {
                self.new_partition = false;
            }
        }
        let (begin, end) = self.group_checker.next_group()?;
        self.rows_to_consume += (end - begin) as u64;
        self.rows
            .extend_from_slice(&self.child_result.as_ref().unwrap().rows[begin..end]);
        self.refresh_rows_memory();
        Ok(())
    }

    /// 从子计划拉下一块；空则返回 true 表示耗尽。
    pub fn fetch_child(&mut self, context: &ExecContext) -> Result<bool> {
        let Some(child) = self.child.next(context)? else {
            return Ok(true);
        };
        if child.rows.is_empty() {
            return Ok(true);
        }
        let row_count = child.num_rows();
        self.accumulated += row_count as u64;
        // 投影输入列入队，供后续回填窗口结果。
        self.data.push_back(DataInfo {
            chunk: child.projected(self.input_columns),
            remaining: row_count as u64,
            accumulated: self.accumulated,
        });
        self.memory_tracker
            .consume(self.data.back().unwrap().chunk.memory_usage());
        let data_memory = (self.data.capacity() * size_of::<DataInfo>()) as i64;
        self.memory_tracker.consume(data_memory - self.data_memory);
        self.data_memory = data_memory;
        self.child_result = Some(child);
        Ok(false)
    }

    /// 按绝对行号取缓冲中的行（相对 `row_start` 索引）。
    pub fn row(&self, absolute: u64) -> &Row {
        &self.rows[(absolute - self.row_start) as usize]
    }

    /// 标记当前分区已完整可读。
    pub fn finish(&mut self) {
        self.whole = true;
    }

    /// 计算当前行的帧起点（ROWS 偏移或 RANGE 扫描）。
    pub fn start_row(&mut self) -> Result<u64> {
        if self.start.unbounded {
            return Ok(0);
        }
        // RANGE：从上次起点向后跳过仍 before_start 的行。
        if self.range_frame {
            let mut start = self.last_start_row.max(self.staged_start_row);
            while start < self.row_count
                && self.start.before_start(
                    self.row(start),
                    self.row(self.current_row),
                    &self.order_by,
                )?
            {
                start += 1;
            }
            self.staged_start_row = start;
            return Ok(start);
        }
        Ok(match self.start.bound_type {
            BoundType::Preceding => self.current_row.saturating_sub(self.start.num),
            BoundType::Following => self.current_row.saturating_add(self.start.num),
            BoundType::CurrentRow => self.current_row,
        })
    }

    /// 计算当前行的帧终点（半开区间上界）。
    pub fn end_row(&mut self) -> Result<u64> {
        if self.end.unbounded {
            return Ok(self.row_count);
        }
        // RANGE：扩展终点直到 beyond_end。
        if self.range_frame {
            let mut end = self.last_end_row.max(self.staged_end_row);
            while end < self.row_count
                && !self.end.beyond_end(
                    self.row(end),
                    self.row(self.current_row),
                    &self.order_by,
                )?
            {
                end += 1;
            }
            self.staged_end_row = end;
            return Ok(end);
        }
        Ok(match self.end.bound_type {
            BoundType::Preceding => {
                if self.current_row >= self.end.num {
                    self.current_row - self.end.num + 1
                } else {
                    0
                }
            }
            BoundType::Following => self
                .current_row
                .saturating_add(self.end.num)
                .saturating_add(1),
            BoundType::CurrentRow => self.current_row.saturating_add(1),
        })
    }

    /// 为指定结果块填充最多 `remained` 行的窗口函数值，并裁剪无用前缀。
    pub fn produce(&mut self, data_index: usize, mut remained: u64) -> Result<u64> {
        let mut produced = 0;
        while remained > 0 && self.enough_to_produce()? {
            let mut start = self.start_row()?;
            let mut end = self.end_row()?;
            end = end.min(self.row_count);
            start = start.min(self.row_count);
            let output_row = self.data[data_index].chunk.num_rows()
                - self.data[data_index].remaining as usize
                + produced as usize;
            let mut values = Vec::with_capacity(self.window_functions.len());
            // 空帧：返回 reset 后的默认结果。
            if start >= end {
                for (index, function) in self.window_functions.iter_mut().enumerate() {
                    if function.ignores_frame() {
                        values.push(function.result()?);
                        continue;
                    }
                    if !self.empty_frame {
                        reset_partial_result_and_release_memory(
                            &self.memory_tracker,
                            index,
                            function.as_mut(),
                        );
                    }
                    values.push(function.result()?);
                }
                if !self.empty_frame {
                    self.empty_frame = true;
                    self.initialized_sliding_window = false;
                }
            } else {
                self.empty_frame = false;
                let relative_start = start - self.row_start;
                let relative_end = end - self.row_start;
                for (index, function) in self.window_functions.iter_mut().enumerate() {
                    if function.ignores_frame() {
                        values.push(function.result()?);
                        continue;
                    }
                    // 优先滑动更新；失败则全量 rebuild 窗口。
                    let slid = if self.initialized_sliding_window {
                        function.slide(&self.rows, relative_start, relative_end)?
                    } else {
                        false
                    };
                    if !slid {
                        function.set_window_start(start);
                        reset_partial_result_and_release_memory(
                            &self.memory_tracker,
                            index,
                            function.as_mut(),
                        );
                        update_partial_result_and_track_memory(
                            &self.memory_tracker,
                            index,
                            function.as_mut(),
                            &self.rows[relative_start as usize..relative_end as usize],
                        )?;
                    } else {
                        self.memory_tracker
                            .update_partial_result(index, function.partial_result_memory_usage());
                    }
                    values.push(function.result()?);
                }
                self.initialized_sliding_window = true;
            }
            self.data[data_index]
                .chunk
                .append_results(output_row, values)?;
            self.current_row += 1;
            self.last_start_row = start;
            self.last_end_row = end;
            produced += 1;
            remained -= 1;
        }
        // 丢弃帧与当前行都不再需要的前缀，降低内存。
        let extend = self
            .current_row
            .min(self.last_end_row)
            .min(self.last_start_row);
        if extend > self.row_start {
            let drop_count = extend - self.row_start;
            self.dropped += drop_count;
            self.rows.drain(..drop_count as usize);
            if self.rows.is_empty() {
                self.rows = Vec::new();
            }
            self.refresh_rows_memory();
            self.row_start = extend;
            // Sliding implementations receive offsets relative to `rows`.
            // Once the prefix is dropped, their previous offsets no longer
            // refer to the same rows, so rebuild the next frame.
            self.initialized_sliding_window = false;
        }
        Ok(produced)
    }

    /// 当前行的帧是否已完全落在已缓冲行范围内（或分区已结束）。
    pub fn enough_to_produce(&mut self) -> Result<bool> {
        if self.current_row >= self.row_count {
            return Ok(false);
        }
        if self.whole {
            return Ok(true);
        }
        let start = self.start_row()?;
        let end = self.end_row()?;
        Ok(end < self.row_count && start < self.row_count)
    }

    /// 切换分区：清空缓冲与滑动状态，reset 窗口函数。
    pub fn reset_partition(&mut self) {
        self.last_start_row = 0;
        self.last_end_row = 0;
        self.staged_start_row = 0;
        self.staged_end_row = 0;
        self.empty_frame = false;
        self.current_row = 0;
        self.whole = false;
        let drop_count = self.row_count - self.row_start;
        self.dropped += drop_count;
        self.rows.drain(..drop_count as usize);
        if self.rows.is_empty() {
            self.rows = Vec::new();
        }
        self.refresh_rows_memory();
        self.row_start = 0;
        self.row_count = 0;
        self.initialized_sliding_window = false;
        reset_partial_results_and_release_memory(&self.memory_tracker, &mut self.window_functions);
    }

    fn refresh_rows_memory(&mut self) {
        let rows_memory = (self.rows.capacity() * size_of::<Row>()) as i64
            + self
                .rows
                .iter()
                .map(|row| {
                    (row.capacity() * size_of::<Value>()) as i64
                        + row.iter().map(value_heap_memory_usage).sum::<i64>()
                })
                .sum::<i64>();
        self.memory_tracker.consume(rows_memory - self.rows_memory);
        self.rows_memory = rows_memory;
    }

    pub fn memory_bytes(&self) -> i64 {
        self.memory_tracker.bytes_consumed()
    }
}

fn value_heap_memory_usage(value: &Value) -> i64 {
    match value {
        Value::Text(value) => value.capacity() as i64,
        _ => 0,
    }
}
