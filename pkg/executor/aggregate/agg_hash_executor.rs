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

// Hash 聚合执行器：编排 Partial/Final Worker，可选 spill，输出聚合结果。
//
// 对应物理计划中的 HashAgg。本文件的可运行部分用线程 scope 并行跑 Partial
// Worker，再由 Final Worker 合并。

// HashAggExec 哈希聚合执行器，保留单线程/并行执行、worker 编排、spill 模式和结果输出的 Go 控制流。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// HashAggInput indicates the input of hash agg exec.
// HashAggInput 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct HashAggInput {
//     pub chk: Box<chunk::Chunk>,
// giveBackCh is bound with specific partial worker,
// it's used to reuse the `chk`,
// and tell the data-fetcher which partial worker it should send data to.
//     pub giveBackCh: channel::Channel</* Go: chan<- *chunk.Chunk */>,
// }
// HashAggExec deals with all the aggregate functions.
// It is built from the Aggregate Plan. When Next() is called, it reads all the data from Src
// and updates all the items in PartialAggFuncs.
// The parallel execution flow is as the following graph shows:
// /*
//                             +-------------+
//                             | Main Thread |
//                             +------+------+
//                                    ^
//                                    |
//                                    +
//                               +-+-            +-+
//                               | |    ......   | |  finalOutputCh
//                               +++-            +-+
//                                ^
//                                |
//                                +---------------+
//                                |               |
//                  +--------------+             +--------------+
//                  | final worker |     ......  | final worker |
//                  +------------+-+             +-+------------+
//                               ^                 ^
//                               |                 |
//                              +-+  +-+  ......  +-+
//                              | |  | |          | |
//                              ...  ...          ...    partialOutputChs
//                              | |  | |          | |
//                              +++  +++          +++
//                               ^    ^            ^
//           +-+                 |    |            |
//           | |        +--------o----+            |
//  inputCh  +-+        |        +-----------------+---+
//           | |        |                              |
//           ...    +---+------------+            +----+-----------+
//           | |    | partial worker |   ......   | partial worker |
//           +++    +--------------+-+            +-+--------------+
//            |                     ^                ^
//            |                     |                |
//       +----v---------+          +++ +-+          +++
//       | data fetcher | +------> | | | |  ......  | |   partialInputChs
//       +--------------+          +-+ +-+          +-+
// */
// HashAggExec 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct HashAggExec {
//     pub exec_BaseExecutor: exec::BaseExecutor,
//     pub Sc: Box<stmtctx::StatementContext>,
//     pub PartialAggFuncs: Vec<aggfuncs::AggFunc>,
//     pub FinalAggFuncs: Vec<aggfuncs::AggFunc>,
//     pub partialResultMap: aggfuncs::AggPartialResultMapper,
//     pub groupSet: set::StringSetWithMemoryUsage,
//     pub groupKeys: Vec<String>,
//     pub cursor4GroupKey: i32,
//     pub GroupByItems: Vec<expression::Expression>,
//     pub groupKeyBuffer: Vec<Vec<byte>>,
//     pub finishCh: channel::Channel</* Go: chan struct{} */>,
//     pub finalOutputCh: channel::Channel</* Go: chan *AfFinalResult */>,
//     pub partialOutputChs: Vec<channel::Channel</* Go: chan aggfuncs.AggPartialResultMapper */>>,
//     pub inputCh: channel::Channel</* Go: chan *HashAggInput */>,
//     pub partialInputChs: Vec<channel::Channel</* Go: chan *chunk.Chunk */>>,
//     pub partialWorkers: Vec<HashAggPartialWorker>,
//     pub finalWorkers: Vec<HashAggFinalWorker>,
//     pub DefaultVal: Box<chunk::Chunk>,
//     pub childResult: Box<chunk::Chunk>,
// IsChildReturnEmpty indicates whether the child executor only returns an empty input.
//     pub IsChildReturnEmpty: bool,
// After we support parallel execution for aggregation functions with distinct,
// we can remove this attribute.
//     pub IsUnparallelExec: bool,
//     pub parallelExecValid: bool,
//     pub prepared: atomic::Bool,
//     pub executed: atomic::Bool,
//     pub memTracker: Box<memory::Tracker>, // track memory usage.
//     pub diskTracker: Box<disk::Tracker>,
//     pub stats: Box<HashAggRuntimeStats>,
// dataInDisk is the chunks to store row values for spilled data.
// The HashAggExec may be set to `spill mode` multiple times, and all spilled data will be appended to DataInDiskByRows.
//     pub dataInDisk: Box<chunk::DataInDiskByChunks>,
// numOfSpilledChks indicates the number of all the spilled chunks.
//     pub numOfSpilledChks: i32,
// offsetOfSpilledChks indicates the offset of the chunk be read from the disk.
// In each round of processing, we need to re-fetch all the chunks spilled in the last one.
//     pub offsetOfSpilledChks: i32,
// inSpillMode indicates whether HashAgg is in `spill mode`.
// When HashAgg is in `spill mode`, the size of `partialResultMap` is no longer growing and all the data fetched
// from the child executor is spilled to the disk.
//     pub inSpillMode: u32,
// tmpChkForSpill is the temp chunk for spilling.
//     pub tmpChkForSpill: Box<chunk::Chunk>,
// The `inflightChunkSync` calls `Add(1)` when the data fetcher goroutine inserts a chunk into the channel,
// and `Done()` when any partial worker retrieves a chunk from the channel and updates it in the `partialResultMap`.
// In scenarios where it is necessary to wait for all partial workers to finish processing the inflight chunk,
// `inflightChunkSync` can be used for synchronization.
//     pub inflightChunkSync: Box<sync::WaitGroup>,
// spillAction save the Action for spilling.
//     pub spillAction: Box<AggSpillDiskAction>,
// parallelAggSpillAction save the Action for spilling of parallel aggregation.
//     pub parallelAggSpillAction: Box<ParallelAggSpillDiskAction>,
// spillHelper helps to carry out the spill action
//     pub spillHelper: Box<parallelHashAggSpillHelper>,
// isChildDrained indicates whether the all data from child has been taken out.
//     pub isChildDrained: bool,
//     pub HasDistinct: bool,
//     pub invalidMemoryUsageForTrackingTest: bool,
//     pub FileNamePrefixForTest: String,
// }
// Close implements the Executor Close interface.
// Close 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
// impl HashAggExec {
//     pub fn Close(&mut self/* Go args:  */) /* Go returns: error */ {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if e.stats != nil {
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
//         defer e.Ctx().GetSessionVars().StmtCtx.RuntimeStatsColl.RegisterStats(e.ID(), e.stats)
//     }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if e.IsUnparallelExec {
//         e.childResult = nil
//         e.groupSet, _ = set.NewStringSetWithMemoryUsage()
//         e.partialResultMap = nil
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if e.memTracker != nil {
//             e.memTracker.ReplaceBytesUsed(0)
//         }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if e.dataInDisk != nil {
//             e.dataInDisk.Close()
//         }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if e.spillAction != nil {
//             e.spillAction.SetFinished()
//         }
//         e.spillAction, e.tmpChkForSpill = nil, nil
//         err := e.BaseExecutor.Close()
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         if err != nil {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             return err
//         }
//         return nil
//     }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if e.parallelExecValid {
// `Close` may be called after `Open` without calling `Next` in test.
// 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if e.prepared.CompareAndSwap(false, true) {
//             close(e.inputCh)
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//             for _, ch := range e.partialOutputChs {
//                 close(ch)
//             }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//             for _, ch := range e.partialInputChs {
//                 close(ch)
//             }
//             close(e.finalOutputCh)
//         }
//         close(e.finishCh)
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for _, ch := range e.partialOutputChs {
//             channel.Clear(ch)
//         }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for _, ch := range e.partialInputChs {
//             channel.Clear(ch)
//         }
//         channel.Clear(e.finalOutputCh)
// 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
//         e.executed.Store(false)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if e.memTracker != nil {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if e.memTracker.BytesConsumed() < 0 {
//                 logutil.BgLogger().Warn("Memory tracker's counter is invalid", zap.Int64("counter", e.memTracker.BytesConsumed()))
//                 e.invalidMemoryUsageForTrackingTest = true
//             }
//             e.memTracker.ReplaceBytesUsed(0)
//         }
//         e.parallelExecValid = false
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if e.parallelAggSpillAction != nil {
//             e.parallelAggSpillAction.SetFinished()
//             e.parallelAggSpillAction = nil
//             e.spillHelper.close()
//         }
//     }
//     err := e.BaseExecutor.Close()
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//     failpoint.Inject("injectHashAggClosePanic", func(val failpoint.Value) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if enabled := val.(bool); enabled {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if e.Ctx().GetSessionVars().ConnectionID != 0 {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//                 panic(errors.New("test"))
//             }
//         }
//     })
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//     return err
// }
// */
use crate::agg_hash_final_worker::{FinalResult, HashAggFinalWorker};
use crate::agg_hash_partial_worker::HashAggPartialWorker;
use crate::agg_spill::ParallelHashAggSpillHelper;
use crate::agg_util::{AggMap, AggState, Aggregation, Chunk, HashAggRuntimeStats};
use astersql_util_execdetails::execdetails::{HashStateRuntimeStats, RuntimeStatsColl};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
/// HashAgg 输入：待聚合 chunk、分组列下标与聚合描述列表。
pub struct HashAggInput {
    pub chunks: Vec<Chunk>,
    pub group_columns: Vec<usize>,
    pub aggregations: Vec<Aggregation>,
}

/// Hash 聚合执行器：并发度、chunk 上限、spill 阈值、结果队列与运行时统计。
pub struct HashAggExec {
    input: HashAggInput,
    partial_concurrency: usize,
    final_concurrency: usize,
    max_chunk_size: usize,
    spill_limit: Option<usize>,
    opened: bool,
    executed: bool,
    results: VecDeque<FinalResult>,
    pub runtime_stats: HashAggRuntimeStats,
    hash_state_stats: Option<HashStateRuntimeStats>,
    runtime_stats_coll: Option<Arc<Mutex<RuntimeStatsColl>>>,
    plan_id: i32,
}

impl HashAggExec {
    /// 构造执行器；并发度与 chunk 大小至少为 1。
    pub fn new(
        input: HashAggInput,
        partial_concurrency: usize,
        final_concurrency: usize,
        max_chunk_size: usize,
        spill_limit: Option<usize>,
    ) -> Self {
        Self {
            input,
            partial_concurrency: partial_concurrency.max(1),
            final_concurrency: final_concurrency.max(1),
            max_chunk_size: max_chunk_size.max(1),
            spill_limit,
            opened: false,
            executed: false,
            results: VecDeque::new(),
            runtime_stats: HashAggRuntimeStats::default(),
            hash_state_stats: None,
            runtime_stats_coll: None,
            plan_id: 0,
        }
    }
    /// 为执行器启用与 Go RuntimeStatsColl 相同的 typed hash-state 证据。
    pub fn with_runtime_stats(
        mut self,
        plan_id: i32,
        runtime_stats_coll: Arc<Mutex<RuntimeStatsColl>>,
    ) -> Self {
        self.plan_id = plan_id;
        self.runtime_stats_coll = Some(runtime_stats_coll);
        self
    }
    /// Open：清空结果并重置 executed / 统计，标记已打开。
    pub fn open(&mut self) {
        self.results.clear();
        self.executed = false;
        self.opened = true;
        self.runtime_stats = HashAggRuntimeStats::default();
        self.hash_state_stats = self
            .runtime_stats_coll
            .as_ref()
            .map(|_| HashStateRuntimeStats::default());
    }
    /// Close：释放结果队列并复位打开状态。
    pub fn close(&mut self) {
        if let (Some(collection), Some(stats)) =
            (&self.runtime_stats_coll, self.hash_state_stats.take())
        {
            collection
                .lock()
                .expect("runtime stats lock poisoned")
                .RegisterStats(self.plan_id, Box::new(stats));
        }
        self.results.clear();
        self.opened = false;
        self.executed = false;
    }
    /// Next：首次调用时执行完整聚合，之后从结果队列弹出 chunk。
    pub fn next(&mut self) -> Result<Option<Chunk>, String> {
        if !self.opened {
            return Err("hash aggregate is not open".to_string());
        }
        if !self.executed {
            if let Err(error) = self.execute() {
                return Err(error);
            }
            self.executed = true;
        }
        match self.results.pop_front() {
            Some(result) => match result.error {
                Some(error) => Err(error),
                None => Ok(Some(result.chunk)),
            },
            None => Ok(None),
        }
    }

    /// 并行 Partial → Final 合并 → 可选 restore spill → 生成结果 chunk。
    fn execute(&mut self) -> Result<(), String> {
        let aggregations = Arc::new(self.input.aggregations.clone());
        let spill = self.spill_limit.map(|limit| {
            Arc::new(ParallelHashAggSpillHelper::new(
                self.final_concurrency,
                limit,
            ))
        });
        // Partial worker 数不超过输入 chunk 数，避免空闲线程。
        let worker_count = self.partial_concurrency.min(self.input.chunks.len().max(1));
        let buckets = (0..worker_count)
            .map(|_| Vec::new())
            .collect::<Vec<Vec<Chunk>>>();
        let mut buckets = buckets;
        for (index, chunk) in self.input.chunks.clone().into_iter().enumerate() {
            buckets[index % worker_count].push(chunk);
        }
        let group_columns = self.input.group_columns.clone();
        // 线程 scope 保证所有 Partial Worker 在返回前 join 完毕。
        let final_concurrency = self.final_concurrency;
        let partial_outputs = std::thread::scope(|scope| {
            let mut joins = Vec::new();
            for chunks in buckets {
                let aggregations = aggregations.clone();
                let spill = spill.clone();
                let group_columns = group_columns.clone();
                joins.push(scope.spawn(move || {
                    let mut worker = HashAggPartialWorker::new(group_columns, aggregations, spill);
                    for chunk in &chunks {
                        worker.update_partial_result(chunk)?;
                    }
                    Ok::<_, String>(worker.shuffle_intermediate_data(final_concurrency))
                }));
            }
            joins
                .into_iter()
                .map(|join| {
                    join.join()
                        .map_err(|_| "partial aggregate worker panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        let mut final_workers = (0..final_concurrency)
            .map(|_| HashAggFinalWorker::new(aggregations.clone(), spill.clone()))
            .collect::<Vec<_>>();
        let spill_triggered = spill
            .as_ref()
            .is_some_and(|spill| spill.status() != crate::agg_spill::SpillStatus::NoSpill);
        for outputs in partial_outputs {
            for (worker, output) in final_workers.iter_mut().zip(outputs) {
                if spill_triggered {
                    // Go forces every partial worker's remaining map through the
                    // spill path once any worker spills. Otherwise one group can
                    // be split between a final worker and the restore consumer.
                    if !output.is_empty() {
                        spill
                            .as_ref()
                            .expect("triggered spill has a helper")
                            .spill(output)?;
                    }
                } else {
                    worker.merge_input(output)?;
                }
            }
        }
        // Spill partitions are restored once. The restored rows are merged into
        // the first final worker; the final result is independent of worker
        // placement, while this preserves the helper's single-consumer cursor.
        if let Some(first_worker) = final_workers.first_mut() {
            first_worker.restore_from_disk()?;
        }
        // 只要发生过 spill 或仍有落盘数据，累计 spill_count。
        if spill.as_ref().is_some_and(|spill| {
            !spill.is_empty() || spill.status() != crate::agg_spill::SpillStatus::NoSpill
        }) {
            self.runtime_stats.spill_count += 1;
        }
        if self.input.group_columns.is_empty()
            && self.input.chunks.iter().all(|chunk| chunk.is_empty())
        {
            let empty_group = AggMap::from([(
                Vec::new(),
                (
                    Vec::new(),
                    self.input
                        .aggregations
                        .iter()
                        .map(|_| AggState::new())
                        .collect(),
                ),
            )]);
            final_workers
                .first_mut()
                .expect("final concurrency is normalized to at least one")
                .merge_input(empty_group)?;
        }
        for final_worker in &mut final_workers {
            if let Some(stats) = &self.hash_state_stats {
                stats.AddRows(final_worker.hash_state_rows() as u64);
            }
            self.results
                .extend(final_worker.generate_result(self.max_chunk_size));
        }
        Ok(())
    }

    /// 测试/诊断：是否曾触发 spill。
    pub fn is_spill_triggered(&self) -> bool {
        self.runtime_stats.spill_count > 0
    }
}
