// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 流式聚合（Stream Aggregation）执行器。
//
// 假定输入已按 group by key 有序。顺序扫描时，group key 变化即刷出上一组
// 的聚合结果；无需构建完整哈希表，内存占用更低。

use crate::agg_util::{AggState, Aggregation, Chunk, Row, get_group_key};
use std::collections::VecDeque;

/// 流式聚合内存增量批量刷盘阈值（1KB）：降低逐 group 调用 tracker 的开销。
pub const STREAM_AGG_MEM_DELTA_FLUSH_THRESHOLD: usize = 1 << 10;

/// 流式聚合执行器：有序输入、分组列、聚合描述、结果队列。
pub struct StreamAggExec {
    input: Vec<Chunk>,
    group_columns: Vec<usize>,
    aggregations: Vec<Aggregation>,
    max_chunk_size: usize,
    opened: bool,
    results: VecDeque<Chunk>,
}

impl StreamAggExec {
    /// 构造执行器；`max_chunk_size` 至少为 1。
    pub fn new(
        input: Vec<Chunk>,
        group_columns: Vec<usize>,
        aggregations: Vec<Aggregation>,
        max_chunk_size: usize,
    ) -> Self {
        Self {
            input,
            group_columns,
            aggregations,
            max_chunk_size: max_chunk_size.max(1),
            opened: false,
            results: VecDeque::new(),
        }
    }
    /// Open：消费全部有序输入，预先生成按 chunk 切分的结果队列。
    pub fn open(&mut self) -> Result<(), String> {
        self.results.clear();
        self.consume_groups()?;
        self.opened = true;
        Ok(())
    }
    /// Close：清空结果并复位。
    pub fn close(&mut self) {
        self.results.clear();
        self.opened = false;
    }
    /// Next：从预计算结果队列弹出下一个 chunk。
    pub fn next(&mut self) -> Result<Option<Chunk>, String> {
        if !self.opened {
            return Err("stream aggregate is not open".to_string());
        }
        Ok(self.results.pop_front())
    }

    /// Returns at most `required_rows` aggregate rows, preserving any unused
    /// suffix for the following call. This mirrors Go's `Chunk.RequiredRows`
    /// contract instead of forcing callers to accept the executor's internal
    /// materialization chunk size.
    pub fn next_required(&mut self, required_rows: usize) -> Result<Chunk, String> {
        if !self.opened {
            return Err("stream aggregate is not open".to_string());
        }
        if required_rows == 0 {
            return Ok(Chunk::new());
        }

        let mut output = Chunk::with_capacity(required_rows);
        while output.len() < required_rows {
            let Some(mut chunk) = self.results.pop_front() else {
                break;
            };
            let take = (required_rows - output.len()).min(chunk.len());
            output.extend(chunk.drain(..take));
            if !chunk.is_empty() {
                self.results.push_front(chunk);
            }
        }
        Ok(output)
    }

    /// 单次扫描：key 变化时刷出上一组；无 group by 时整表视为一组。
    fn consume_groups(&mut self) -> Result<(), String> {
        let mut current_key: Option<Vec<u8>> = None;
        let mut current_group = Row::new();
        let mut states = self
            .aggregations
            .iter()
            .map(|_| AggState::new())
            .collect::<Vec<_>>();
        let mut output = Chunk::new();
        for row in self.input.iter().flatten() {
            let key = get_group_key(row, &self.group_columns)?;
            // group key 变化：先输出上一组，再重置聚合态。
            if current_key.as_ref().is_some_and(|current| current != &key) {
                append_group(
                    &mut output,
                    std::mem::take(&mut current_group),
                    &states,
                    &self.aggregations,
                );
                states = self.aggregations.iter().map(|_| AggState::new()).collect();
                if output.len() >= self.max_chunk_size {
                    self.results.push_back(std::mem::take(&mut output));
                }
            }
            if current_key.as_ref() != Some(&key) {
                current_group = self
                    .group_columns
                    .iter()
                    .map(|index| {
                        row.get(*index)
                            .cloned()
                            .ok_or_else(|| format!("group column {index} out of range"))
                    })
                    .collect::<Result<Row, _>>()?;
                current_key = Some(key);
            }
            for (state, aggregation) in states.iter_mut().zip(&self.aggregations) {
                state.update(aggregation, row)?;
            }
        }
        // 扫完后刷出最后一组；无 GROUP BY 时即使空输入也可能需要默认行（由上层处理）。
        if current_key.is_some() || self.group_columns.is_empty() {
            append_group(&mut output, current_group, &states, &self.aggregations);
        }
        if !output.is_empty() {
            self.results.push_back(output);
        }
        Ok(())
    }
}

/// 把分组列值与各聚合最终结果拼成一行，追加到输出 chunk。
fn append_group(
    output: &mut Chunk,
    mut group: Row,
    states: &[AggState],
    aggregations: &[Aggregation],
) {
    group.extend(
        states
            .iter()
            .zip(aggregations)
            .map(|(state, aggregation)| state.result(aggregation.kind)),
    );
    output.push(group);
}

// consumeOneGroup 对应 Go 方法：接收者为 `e *StreamAggExec`，保留原方法的控制流、错误返回和资源处理顺序。
