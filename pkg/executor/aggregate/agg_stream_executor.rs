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

// StreamAggExec 流式聚合执行器，保留按 group key 消费 child chunk、刷新内存增量、输出聚合结果的 Go 控制流。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// streamAggMemDeltaFlushThreshold is the threshold for flushing buffered memory delta to the tracker.
// Consuming memory for every group is expensive due to atomic operations traversing the tracker tree.
// We buffer deltas across groups and flush in batch to reduce Consume call frequency.
// Go const streamAggMemDeltaFlushThreshold：保持原常量表达式。 原注释：1KB
// pub const streamAggMemDeltaFlushThreshold: usize = 1 << 10;
// StreamAggExec deals with all the aggregate functions.
// It assumes all the input data is sorted by group by key.
// When Next() is called, it will return a result for the same group.
// StreamAggExec 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct StreamAggExec {
//     pub exec_BaseExecutor: exec::BaseExecutor,
//     pub executed: bool,
// IsChildReturnEmpty indicates whether the child executor only returns an empty input.
//     pub IsChildReturnEmpty: bool,
//     pub DefaultVal: Box<chunk::Chunk>,
//     pub GroupChecker: Box<vecgroupchecker::VecGroupChecker>,
//     pub inputIter: Box<chunk::Iterator4Chunk>,
//     pub inputRow: chunk::Row,
//     pub AggFuncs: Vec<aggfuncs::AggFunc>,
//     pub partialResults: Vec<aggfuncs::PartialResult>,
//     pub groupRows: Vec<chunk::Row>,
//     pub childResult: Box<chunk::Chunk>,
//     pub memTracker: Box<memory::Tracker>,
// memUsageOfInitialPartialResult indicates the memory usage of all partial results after initialization.
// All partial results will be reset after processing one group data, and the memory usage should also be reset.
// We can't get memory delta from ResetPartialResult, so record the memory usage here.
//     pub memUsageOfInitialPartialResult: i64,
// pendingMemDelta buffers memory deltas across groups and is flushed to memTracker in batch.
// Cleared in appendResult2Chunk via ReplaceBytesUsed, which resets to the correct baseline.
//     pub pendingMemDelta: i64,
// }
// Open implements the Executor Open interface.
// Open 对应 Go 方法：接收者为 `e *StreamAggExec`，保留原方法的控制流、错误返回和资源处理顺序。
// impl StreamAggExec {
//     pub fn Open(&mut self, /* Go args: ctx context.Context */) /* Go returns: error */ {
//     failpoint.Inject("mockStreamAggExecBaseExecutorOpenReturnedError", func(val failpoint.Value) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//         if val, _ := val.(bool); val {
//             failpoint.Return(errors.New("mock StreamAggExec.baseExecutor.Open returned error"))
//         }
//     })
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if err := e.BaseExecutor.Open(ctx); err != nil {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         return err
//     }
// If panic in Open, the children executor should be closed because they are open.
// 资源收尾说明：Go defer 延迟执行清理/统计；保留收尾意图。
//     defer closeBaseExecutor(&e.BaseExecutor)
//     return e.OpenSelf()
// }
// }
// OpenSelf just opens the StreamAggExec.
// OpenSelf 对应 Go 方法：接收者为 `e *StreamAggExec`，保留原方法的控制流、错误返回和资源处理顺序。
// impl StreamAggExec {
//     pub fn OpenSelf(&mut self) /* Go returns: error */ {
//     e.childResult = exec.TryNewCacheChunk(e.Children(0))
//     e.executed = false
//     e.IsChildReturnEmpty = true
//     e.inputIter = chunk.NewIterator4Chunk(e.childResult)
//     e.inputRow = e.inputIter.End()
//     e.partialResults = make([]aggfuncs.PartialResult, 0, len(e.AggFuncs))
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, aggFunc := range e.AggFuncs {
//         partialResult, memDelta := aggFunc.AllocPartialResult()
//         e.partialResults = append(e.partialResults, partialResult)
//         e.memUsageOfInitialPartialResult += memDelta
//     }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if e.memTracker != nil {
//         e.memTracker.Reset()
//     } else {
// bytesLimit <= 0 means no limit, for now we just track the memory footprint
//         e.memTracker = memory.NewTracker(e.ID(), -1)
//     }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/error/context 的语义后续再接线。
//     if e.Ctx().GetSessionVars().TrackAggregateMemoryUsage {
//         e.memTracker.AttachTo(e.Ctx().GetSessionVars().StmtCtx.MemTracker)
//     }
//     failpoint.Inject("ConsumeRandomPanic", nil)
//     e.memTracker.Consume(e.childResult.MemoryUsage() + e.memUsageOfInitialPartialResult)
// 返回说明：沿用 Go 的 nil/error 返回约定，Rust Result 形状仅作提示。
//     return nil
// }
// }
// */
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
