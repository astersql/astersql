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
// Worker，再由 Final Worker 合并；Go 并行/单线程完整控制流保留在下方注释块。

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
use std::collections::VecDeque;
use std::sync::Arc;

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
        }
    }
    /// Open：清空结果并重置 executed / 统计，标记已打开。
    pub fn open(&mut self) {
        self.results.clear();
        self.executed = false;
        self.opened = true;
        self.runtime_stats = HashAggRuntimeStats::default();
    }
    /// Close：释放结果队列并复位打开状态。
    pub fn close(&mut self) {
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
            self.execute()?;
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
/*
}

// Open implements the Executor Open interface.
// Open 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn Open(&mut self, /* Go args: ctx context.Context */) /* Go returns: error */ {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("mockHashAggExecBaseExecutorOpenReturnedError", func(val failpoint.Value) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if val, _ := val.(bool); val {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
            failpoint.Return(errors.New("mock HashAggExec.baseExecutor.Open returned error"))
        }
    })

    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    if err := e.BaseExecutor.Open(ctx); err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        return err
    }
    return e.OpenSelf()
}
}

// OpenSelf just opens the hash aggregation executor.
// OpenSelf 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn OpenSelf(&mut self/* Go args:  */) /* Go returns: error */ {
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    e.prepared.Store(false)

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.memTracker != nil {
        e.memTracker.Reset()
    } else {
        e.memTracker = memory.NewTracker(e.ID(), -1)
    }
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.Ctx().GetSessionVars().TrackAggregateMemoryUsage {
        e.memTracker.AttachTo(e.Ctx().GetSessionVars().StmtCtx.MemTracker)
    }

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.IsUnparallelExec {
        e.initForUnparallelExec()
        return nil
    }
    return e.initForParallelExec(e.Ctx())
}
}

// initForUnparallelExec 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn initForUnparallelExec(&mut self/* Go args:  */) {
    var setSize int64
    e.groupSet, setSize = set.NewStringSetWithMemoryUsage()
    e.partialResultMap = aggfuncs.NewAggPartialResultMapper()
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("ConsumeRandomPanic", nil)
    e.memTracker.Consume(int64(e.partialResultMap.Bytes) + setSize)
    e.groupKeyBuffer = make([][]byte, 0, 8)
    e.childResult = exec.TryNewCacheChunk(e.Children(0))
    e.memTracker.Consume(e.childResult.MemoryUsage())

    e.offsetOfSpilledChks, e.numOfSpilledChks = 0, 0
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    e.executed.Store(false)
    e.isChildDrained = false
    e.dataInDisk = chunk.NewDataInDiskByChunks(exec.RetTypes(e.Children(0)), e.FileNamePrefixForTest)

    e.tmpChkForSpill = exec.TryNewCacheChunk(e.Children(0))
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if vars := e.Ctx().GetSessionVars(); vars.TrackAggregateMemoryUsage && vardef.EnableTmpStorageOnOOM.Load() {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.diskTracker != nil {
            e.diskTracker.Reset()
        } else {
            e.diskTracker = disk.NewTracker(e.ID(), -1)
        }
        e.diskTracker.AttachTo(vars.StmtCtx.DiskTracker)
        e.dataInDisk.GetDiskTracker().AttachTo(e.diskTracker)
        vars.MemTracker.FallbackOldAndSetNewActionForSoftLimit(e.ActionSpill())
    }
}
}

// initPartialWorkers 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn initPartialWorkers(&mut self, /* Go args: partialConcurrency int, finalConcurrency int, ctx sessionctx.Context */) {
    memUsage := int64(0)

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range partialConcurrency {
        partialResultsMap := make([]aggfuncs.AggPartialResultMapper, finalConcurrency)
        sz := int64(0)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for i := range finalConcurrency {
            r := aggfuncs.NewAggPartialResultMapper()
            partialResultsMap[i] = r
            sz += int64(r.Bytes)
        }

        partialResultsBuffer, groupKeyBuf := getBuffer()
        e.partialWorkers[i] = HashAggPartialWorker{
            baseHashAggWorker:     newBaseHashAggWorker(e.finishCh, e.PartialAggFuncs, e.MaxChunkSize(), e.memTracker),
            idForTest:             i,
            ctx:                   ctx,
            inputCh:               e.partialInputChs[i],
            outputChs:             e.partialOutputChs,
            giveBackCh:            e.inputCh,
            partialResultsBuffer:  *partialResultsBuffer,
            globalOutputCh:        e.finalOutputCh,
            partialResultsMap:     partialResultsMap,
            groupByItems:          e.GroupByItems,
            chk:                   e.NewChunkWithCapacity(e.Children(0).RetFieldTypes(), 0, e.MaxChunkSize()),
            groupKeyBuf:           *groupKeyBuf,
            serializeHelpers:      aggfuncs.NewSerializeHelper(),
            isSpillPrepared:       false,
            spillHelper:           e.spillHelper,
            inflightChunkSync:     e.inflightChunkSync,
            fileNamePrefixForTest: e.FileNamePrefixForTest,
        }

        memUsage += e.partialWorkers[i].chk.MemoryUsage()
        e.partialWorkers[i].partialResultNumInRow = e.partialWorkers[i].getPartialResultSliceLenConsiderByteAlign()
        // There is a bucket in the empty partialResultsMap.
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
        failpoint.Inject("ConsumeRandomPanic", nil)
        e.memTracker.Consume(sz)
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.stats != nil {
            e.partialWorkers[i].stats = &AggWorkerStat{}
            e.stats.PartialStats = append(e.stats.PartialStats, e.partialWorkers[i].stats)
        }
        input := &HashAggInput{
            chk:        chunk.New(e.Children(0).RetFieldTypes(), 0, e.MaxChunkSize()),
            giveBackCh: e.partialWorkers[i].inputCh,
        }
        memUsage += input.chk.MemoryUsage()
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        e.inputCh <- input
    }

    e.memTracker.Consume(memUsage)
}
}

// initFinalWorkers 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn initFinalWorkers(&mut self, /* Go args: finalConcurrency int */) {
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range finalConcurrency {
        e.finalWorkers[i] = HashAggFinalWorker{
            baseHashAggWorker:          newBaseHashAggWorker(e.finishCh, e.FinalAggFuncs, e.MaxChunkSize(), e.memTracker),
            partialResultMap:           aggfuncs.NewAggPartialResultMapper(),
            inputCh:                    e.partialOutputChs[i],
            outputCh:                   e.finalOutputCh,
            finalResultHolderCh:        make(chan *chunk.Chunk, 1),
            spillHelper:                e.spillHelper,
            restoredAggResultMapperMem: 0,
        }
        // There is a bucket in the empty partialResultsMap.
        e.memTracker.Consume(int64(e.finalWorkers[i].partialResultMap.Bytes))
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.stats != nil {
            e.finalWorkers[i].stats = &AggWorkerStat{}
            e.stats.FinalStats = append(e.stats.FinalStats, e.finalWorkers[i].stats)
        }
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        e.finalWorkers[i].finalResultHolderCh <- chunk.New(e.RetFieldTypes(), 0, e.MaxChunkSize())
    }
}
}

// initForParallelExec 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn initForParallelExec(&mut self, /* Go args: ctx sessionctx.Context */) /* Go returns: error */ {
    sessionVars := e.Ctx().GetSessionVars()
    partialConcurrency := sessionVars.HashAggPartialConcurrency()
    finalConcurrency := sessionVars.HashAggFinalConcurrency()

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if partialConcurrency == 0 || finalConcurrency == 0 {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        return errors.New("partialConcurrency or finalConcurrency is 0")
    }

    e.IsChildReturnEmpty = true
    e.finalOutputCh = make(chan *AfFinalResult, finalConcurrency+partialConcurrency+1)
    e.inputCh = make(chan *HashAggInput, partialConcurrency)
    e.finishCh = make(chan struct{}, 1)

    e.partialInputChs = make([]chan *chunk.Chunk, partialConcurrency)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range e.partialInputChs {
        e.partialInputChs[i] = make(chan *chunk.Chunk, 1)
    }
    e.partialOutputChs = make([]chan aggfuncs.AggPartialResultMapper, finalConcurrency)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range e.partialOutputChs {
        e.partialOutputChs[i] = make(chan aggfuncs.AggPartialResultMapper, partialConcurrency)
    }

    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    e.inflightChunkSync = &sync.WaitGroup{}

    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    isTrackerEnabled := e.Ctx().GetSessionVars().TrackAggregateMemoryUsage && vardef.EnableTmpStorageOnOOM.Load()
    isParallelHashAggSpillEnabled := e.Ctx().GetSessionVars().EnableParallelHashaggSpill

    baseRetTypeNum := len(e.RetFieldTypes())

    // Intermediate result for aggregate function also need to be spilled,
    // so the number of spillChunkFieldTypes should be added 1.
    spillChunkFieldTypes := make([]*types.FieldType, baseRetTypeNum+1)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range baseRetTypeNum {
        spillChunkFieldTypes[i] = types.NewFieldType(mysql.TypeVarString)
    }

    var err error
    spillChunkFieldTypes[baseRetTypeNum] = types.NewFieldType(mysql.TypeString)
    e.spillHelper, err = newSpillHelper(e.memTracker, e.PartialAggFuncs, e.FinalAggFuncs, func() *chunk.Chunk {
        return chunk.New(spillChunkFieldTypes, e.InitCap(), e.MaxChunkSize())
    }, spillChunkFieldTypes)
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        return err
    }

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if isTrackerEnabled && isParallelHashAggSpillEnabled && !e.HasDistinct {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.diskTracker != nil {
            e.diskTracker.Reset()
        } else {
            e.diskTracker = disk.NewTracker(e.ID(), -1)
        }
        e.diskTracker.AttachTo(sessionVars.StmtCtx.DiskTracker)
        e.spillHelper.diskTracker = e.diskTracker
        sessionVars.MemTracker.FallbackOldAndSetNewActionForSoftLimit(e.ActionSpill())
    }

    e.partialWorkers = make([]HashAggPartialWorker, partialConcurrency)
    e.finalWorkers = make([]HashAggFinalWorker, finalConcurrency)
    e.initRuntimeStats()

    e.initPartialWorkers(partialConcurrency, finalConcurrency, ctx)
    e.initFinalWorkers(finalConcurrency)
    e.parallelExecValid = true
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    e.executed.Store(false)
    return nil
}
}

// Next implements the Executor Next interface.
// Next 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn Next(&mut self, /* Go args: ctx context.Context, req *chunk.Chunk */) /* Go returns: error */ {
    req.Reset()
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.IsUnparallelExec {
        return e.unparallelExec(ctx, req)
    }
    return e.parallelExec(ctx, req)
}
}

// fetchChildData 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn fetchChildData(&mut self, /* Go args: ctx context.Context, waitGroup *sync.WaitGroup */) {
    var (
        input *HashAggInput
        chk   *chunk.Chunk
        ok    bool
        err   error
    )

    // 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
    defer func() {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if r := recover(); r != nil {
            recoveryHashAgg(e.finalOutputCh, r)
        }

        // Wait for the finish of all partial workers
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        e.inflightChunkSync.Wait()

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if !e.spillHelper.isNoSpill() && !e.spillHelper.checkError() {
            // Spill the remaining data
            e.spill()

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
            for i := range e.partialWorkers {
                e.spillHelper.addListInDisks(e.partialWorkers[i].spilledChunksIO)
                e.partialWorkers[i].spilledChunksIO = e.partialWorkers[i].spilledChunksIO[:0]
            }
        }

        // When error happens, some disk files may not be closed.
        // We need to manually check and close them.
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for i := range e.partialWorkers {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if len(e.partialWorkers[i].spilledChunksIO) > 0 {
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
                for _, disk := range e.partialWorkers[i].spilledChunksIO {
                    disk.Close()
                }
            }
        }

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for i := range e.partialInputChs {
            close(e.partialInputChs[i])
        }
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        waitGroup.Done()
    }()

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for {
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        select {
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        case <-e.finishCh:
            return
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        case input, ok = <-e.inputCh:
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if !ok {
                return
            }
            chk = input.chk
        }

        mSize := chk.MemoryUsage()
        err = exec.Next(ctx, e.Children(0), chk)
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if err != nil {
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            e.finalOutputCh <- &AfFinalResult{err: err}
            e.memTracker.Consume(-mSize)
            return
        }

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if chk.NumRows() == 0 {
            e.memTracker.Consume(-mSize)
            return
        }

    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
        failpoint.Inject("ConsumeRandomPanic", nil)
        e.memTracker.Consume(chk.MemoryUsage() - mSize)
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        e.inflightChunkSync.Add(1)
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        input.giveBackCh <- chk

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if hasError := e.spillIfNeed(); hasError {
            e.memTracker.Consume(-mSize)
            return
        }
    }
}
}

// spillIfNeed 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn spillIfNeed(&mut self/* Go args:  */) /* Go returns: bool */ {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.spillHelper.checkError() {
        return true
    }

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if !e.spillHelper.checkNeedSpill() {
        return false
    }

    // Wait for the finish of all partial workers
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    e.inflightChunkSync.Wait()
    e.spill()
    return false
}
}

// spill 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn spill(&mut self/* Go args:  */) {
    e.spillHelper.setInSpilling()
    // 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
    defer e.spillHelper.setSpillTriggered()

    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    spillWaiter := &sync.WaitGroup{}
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    spillWaiter.Add(len(e.partialWorkers))

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range e.partialWorkers {
    // 并发说明：Go goroutine 在这里启动后台 worker；只保留调度意图。
        go func(worker *HashAggPartialWorker) {
    // 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
            defer spillWaiter.Done()
            err := worker.spillDataToDisk()
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            if err != nil {
                worker.processError(err)
            }
        }(&e.partialWorkers[i])
    }

    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    spillWaiter.Wait()
}
}

// waitPartialWorkerAndCloseOutputChs 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn waitPartialWorkerAndCloseOutputChs(&mut self, /* Go args: waitGroup *sync.WaitGroup */) {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    waitGroup.Wait()
    close(e.inputCh)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for input := range e.inputCh {
        e.memTracker.Consume(-input.chk.MemoryUsage())
    }
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for _, ch := range e.partialOutputChs {
        close(ch)
    }
}
}

// waitAllWorkersAndCloseFinalOutputCh 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn waitAllWorkersAndCloseFinalOutputCh(&mut self, /* Go args: waitGroups ...*sync.WaitGroup */) {
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for _, waitGroup := range waitGroups {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        waitGroup.Wait()
    }
    close(e.finalOutputCh)
}
}

// prepare4ParallelExec 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn prepare4ParallelExec(&mut self, /* Go args: ctx context.Context */) {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    fetchChildWorkerWaitGroup := &sync.WaitGroup{}
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    fetchChildWorkerWaitGroup.Add(1)
    // 并发说明：Go goroutine 在这里启动后台 worker；只保留调度意图。
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    go e.fetchChildData(ctx, fetchChildWorkerWaitGroup)

    // We get the pointers here instead of when we are all finished and adding the time because:
    // (1) If there is Apply in the plan tree, executors may be reused (Open()ed and Close()ed multiple times)
    // (2) we don't wait all goroutines of HashAgg to exit in HashAgg.Close()
    // So we can't write something like:
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    //     atomic.AddInt64(&e.stats.PartialWallTime, int64(time.Since(partialStart)))
    // Because the next execution of HashAgg may have started when this goroutine haven't exited and then there will be data race.
    var partialWallTimePtr, finalWallTimePtr *int64
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.stats != nil {
        partialWallTimePtr = &e.stats.PartialWallTime
        finalWallTimePtr = &e.stats.FinalWallTime
    }

    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    partialWorkerWaitGroup := &sync.WaitGroup{}
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    partialWorkerWaitGroup.Add(len(e.partialWorkers))
    partialStart := time.Now()
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range e.partialWorkers {
    // 并发说明：Go goroutine 在这里启动后台 worker；只保留调度意图。
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        go e.partialWorkers[i].run(e.Ctx(), partialWorkerWaitGroup, len(e.finalWorkers))
    }

    // 并发说明：Go goroutine 在这里启动后台 worker；只保留调度意图。
    go func() {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        e.waitPartialWorkerAndCloseOutputChs(partialWorkerWaitGroup)
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if partialWallTimePtr != nil {
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
            atomic.AddInt64(partialWallTimePtr, int64(time.Since(partialStart)))
        }
    }()

    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    finalWorkerWaitGroup := &sync.WaitGroup{}
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    finalWorkerWaitGroup.Add(len(e.finalWorkers))
    finalStart := time.Now()
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range e.finalWorkers {
    // 并发说明：Go goroutine 在这里启动后台 worker；只保留调度意图。
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        go e.finalWorkers[i].run(e.Ctx(), finalWorkerWaitGroup, partialWorkerWaitGroup)
    }

    // 并发说明：Go goroutine 在这里启动后台 worker；只保留调度意图。
    go func() {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        finalWorkerWaitGroup.Wait()
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if finalWallTimePtr != nil {
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
            atomic.AddInt64(finalWallTimePtr, int64(time.Since(finalStart)))
        }
    }()

    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    // All workers may send error message to e.finalOutputCh when they panic.
    // And e.finalOutputCh should be closed after all goroutines gone.
    // 并发说明：Go goroutine 在这里启动后台 worker；只保留调度意图。
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    go e.waitAllWorkersAndCloseFinalOutputCh(fetchChildWorkerWaitGroup, partialWorkerWaitGroup, finalWorkerWaitGroup)
}
}

// HashAggExec employs one input reader, M partial workers and N final workers to execute parallelly.
// The parallel execution flow is:
// 1. input reader reads data from child executor and send them to partial workers.
// 2. partial worker receives the input data, updates the partial results, and shuffle the partial results to the final workers.
// 3. final worker receives partial results from all the partial workers, evaluates the final results and sends the final results to the main thread.
// parallelExec 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn parallelExec(&mut self, /* Go args: ctx context.Context, chk *chunk.Chunk */) /* Go returns: error */ {
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.prepared.CompareAndSwap(false, true) {
        e.prepare4ParallelExec(ctx)
    }

    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("parallelHashAggError", func(val failpoint.Value) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if val, _ := val.(bool); val {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
            failpoint.Return(errors.New("HashAggExec.parallelExec error"))
        }
    })

    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.executed.Load() {
        return nil
    }

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for {
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        result, ok := <-e.finalOutputCh
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if !ok {
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
            e.executed.Store(true)
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if e.IsChildReturnEmpty && e.DefaultVal != nil {
                chk.Append(e.DefaultVal, 0, 1)
            }
            return nil
        }
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if result.err != nil {
            return result.err
        }
        chk.SwapColumns(result.chk)
        result.chk.Reset()

        // So that we can reuse the chunk
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        result.giveBackCh <- result.chk
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if chk.NumRows() > 0 {
            e.IsChildReturnEmpty = false
            return nil
        }
    }
}
}

// unparallelExec executes hash aggregation algorithm in single thread.
// unparallelExec 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn unparallelExec(&mut self, /* Go args: ctx context.Context, chk *chunk.Chunk */) /* Go returns: error */ {
    chk.Reset()
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for {
        exprCtx := e.Ctx().GetExprCtx()
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.prepared.Load() {
            // Since we return e.MaxChunkSize() rows every time, so we should not traverse
            // `groupSet` because of its randomness.
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
            for ; e.cursor4GroupKey < len(e.groupKeys); e.cursor4GroupKey++ {
                partialResults := e.getPartialResults(e.groupKeys[e.cursor4GroupKey])
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
                if len(e.PartialAggFuncs) == 0 {
                    chk.SetNumVirtualRows(chk.NumRows() + 1)
                }
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
                for i, af := range e.PartialAggFuncs {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                    if err := af.AppendFinalResult2Chunk(exprCtx.GetEvalCtx(), partialResults[i], chk); err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                        return err
                    }
                }
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
                if chk.IsFull() {
                    e.cursor4GroupKey++
                    return nil
                }
            }
            e.resetSpillMode()
        }
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.executed.Load() {
            return nil
        }
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if err := e.execute(ctx); err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            return err
        }
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if len(e.groupSet.M) == 0 && len(e.GroupByItems) == 0 {
            // If no groupby and no data, we should add an empty group.
            // For example:
            // "select count(c) from t;" should return one row [0]
            // "select count(c) from t group by c1;" should return empty result set.
            e.memTracker.Consume(e.groupSet.Insert(""))
            e.groupKeys = append(e.groupKeys, "")
        }
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
        e.prepared.Store(true)
    }
}
}

// resetSpillMode 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn resetSpillMode(&mut self/* Go args:  */) {
    e.cursor4GroupKey, e.groupKeys = 0, e.groupKeys[:0]
    var setSize int64
    e.groupSet, setSize = set.NewStringSetWithMemoryUsage()
    e.partialResultMap = aggfuncs.NewAggPartialResultMapper()
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    e.prepared.Store(false)
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    e.executed.Store(e.numOfSpilledChks == e.dataInDisk.NumChunks()) // No data is spilling again, all data have been processed.
    e.numOfSpilledChks = e.dataInDisk.NumChunks()
    e.memTracker.ReplaceBytesUsed(setSize)
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    atomic.StoreUint32(&e.inSpillMode, 0)
}
}

// execute fetches Chunks from src and update each aggregate function for each row in Chunk.
// execute 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn execute(&mut self, /* Go args: ctx context.Context */) /* Go returns: (err error) */ {
    // 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
    defer func() {
        if e.tmpChkForSpill.NumRows() > 0 && err == nil {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
            err = e.dataInDisk.Add(e.tmpChkForSpill)
            e.tmpChkForSpill.Reset()
        }
    }()
    exprCtx := e.Ctx().GetExprCtx()
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for {
        mSize := e.childResult.MemoryUsage()
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if err := e.getNextChunk(ctx); err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            return err
        }
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
        failpoint.Inject("ConsumeRandomPanic", nil)
        e.memTracker.Consume(e.childResult.MemoryUsage() - mSize)
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            return err
        }

    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
        failpoint.Inject("unparallelHashAggError", func(val failpoint.Value) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if val, _ := val.(bool); val {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
                failpoint.Return(errors.New("HashAggExec.unparallelExec error"))
            }
        })

        // no more data.
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.childResult.NumRows() == 0 {
            return nil
        }
        e.groupKeyBuffer, err = GetGroupKey(e.Ctx(), e.childResult, e.groupKeyBuffer, e.GroupByItems)
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            return err
        }

        allMemDelta := int64(0)
        sel := make([]int, 0, e.childResult.NumRows())
        var tmpBuf [1]chunk.Row
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for j := range e.childResult.NumRows() {
            groupKey := string(e.groupKeyBuffer[j]) // do memory copy here, because e.groupKeyBuffer may be reused.
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if _, ok := e.groupSet.M[groupKey]; !ok {
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
                if atomic.LoadUint32(&e.inSpillMode) == 1 && len(e.groupSet.M) > 0 {
                    sel = append(sel, j)
                    continue
                }
                allMemDelta += e.groupSet.Insert(groupKey)
                e.groupKeys = append(e.groupKeys, groupKey)
            }
            partialResults := e.getPartialResults(groupKey)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
            for i, af := range e.PartialAggFuncs {
                tmpBuf[0] = e.childResult.GetRow(j)
                memDelta, err := af.UpdatePartialResult(exprCtx.GetEvalCtx(), tmpBuf[:], partialResults[i])
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                    return err
                }
                allMemDelta += memDelta
            }
        }

        // spill unprocessed data when exceeded.
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if len(sel) > 0 {
            e.childResult.SetSel(sel)
            err = e.spillUnprocessedData(len(sel) == cap(sel))
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                return err
            }
        }

    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
        failpoint.Inject("ConsumeRandomPanic", nil)
        e.memTracker.Consume(allMemDelta)
    }
}
}

// spillUnprocessedData 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn spillUnprocessedData(&mut self, /* Go args: isFullChk bool */) /* Go returns: (err error) */ {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if isFullChk {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        return e.dataInDisk.Add(e.childResult)
    }
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range e.childResult.NumRows() {
        e.tmpChkForSpill.AppendRow(e.childResult.GetRow(i))
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.tmpChkForSpill.IsFull() {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
            err = e.dataInDisk.Add(e.tmpChkForSpill)
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                return err
            }
            e.tmpChkForSpill.Reset()
        }
    }
    return nil
}
}

// getNextChunk 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn getNextChunk(&mut self, /* Go args: ctx context.Context */) /* Go returns: (err error) */ {
    e.childResult.Reset()
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if !e.isChildDrained {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if err := exec.Next(ctx, e.Children(0), e.childResult); err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            return err
        }
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if e.childResult.NumRows() != 0 {
            return nil
        }
        e.isChildDrained = true
    }
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.offsetOfSpilledChks < e.numOfSpilledChks {
        e.childResult, err = e.dataInDisk.GetChunk(e.offsetOfSpilledChks)
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            return err
        }
        e.offsetOfSpilledChks++
    }
    return nil
}
}

// getPartialResults 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn getPartialResults(&mut self, /* Go args: groupKey string */) /* Go returns: []aggfuncs.PartialResult */ {
    partialResults, ok := e.partialResultMap.M[groupKey]
    allMemDelta := int64(0)
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if !ok {
        partialResults = make([]aggfuncs.PartialResult, 0, len(e.PartialAggFuncs))
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for _, af := range e.PartialAggFuncs {
            partialResult, memDelta := af.AllocPartialResult()
            partialResults = append(partialResults, partialResult)
            allMemDelta += memDelta
        }
        deltaBytes := e.partialResultMap.Set(groupKey, partialResults)
        allMemDelta += int64(len(groupKey))
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if deltaBytes > 0 {
            e.memTracker.Consume(deltaBytes)
        }
    }
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("ConsumeRandomPanic", nil)
    e.memTracker.Consume(allMemDelta)
    return partialResults
}
}

// initRuntimeStats 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn initRuntimeStats(&mut self/* Go args:  */) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if e.RuntimeStats() != nil {
        stats := &HashAggRuntimeStats{
            PartialConcurrency: e.Ctx().GetSessionVars().HashAggPartialConcurrency(),
            FinalConcurrency:   e.Ctx().GetSessionVars().HashAggFinalConcurrency(),
        }
        stats.PartialStats = make([]*AggWorkerStat, 0, stats.PartialConcurrency)
        stats.FinalStats = make([]*AggWorkerStat, 0, stats.FinalConcurrency)
        e.stats = stats
    }
}
}

// IsSpillTriggeredForTest is for test.
// IsSpillTriggeredForTest 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn IsSpillTriggeredForTest(&mut self/* Go args:  */) /* Go returns: bool */ {
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range e.spillHelper.lock.spilledChunksIO {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if len(e.spillHelper.lock.spilledChunksIO[i]) > 0 {
            return true
        }
    }
    return false
}
}

// IsInvalidMemoryUsageTrackingForTest is for test
// IsInvalidMemoryUsageTrackingForTest 对应 Go 方法：接收者为 `e *HashAggExec`，保留控制流、错误返回和资源处理顺序。
impl HashAggExec {
    pub fn IsInvalidMemoryUsageTrackingForTest(&mut self/* Go args:  */) /* Go returns: bool */ {
    return e.invalidMemoryUsageForTrackingTest
}
}
*/
