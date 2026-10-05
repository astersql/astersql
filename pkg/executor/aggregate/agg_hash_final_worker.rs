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

// Hash 聚合 Final Worker：合并 partial 中间结果并产出最终 chunk。
//
// 在并行 HashAgg 中，Final Worker 接收各 Partial Worker 按 key 哈希分片后的
// AggPartialResultMapper，合并同一 group 的 partial result；若触发 spill，
// 则从磁盘分区恢复后再生成最终结果。

// HashAggFinalWorker 负责接收 partial worker 的中间结果、合并 partial result，并向主线程发送最终 chunk。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// AfFinalResult indicates aggregation functions final result.
// AfFinalResult 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct AfFinalResult {
//     pub chk: Box<chunk::Chunk>,
//     pub err: errors::Error,
//     pub giveBackCh: channel::Channel</* Go: chan *chunk.Chunk */>,
// }
// HashAggFinalWorker indicates the final workers of parallel hash agg execution,
// the number of the worker can be set by `tidb_hashagg_final_concurrency`.
// HashAggFinalWorker 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct HashAggFinalWorker {
//     pub baseHashAggWorker: baseHashAggWorker,
//     pub partialResultMap: aggfuncs::AggPartialResultMapper,
//     pub inputCh: channel::Channel</* Go: chan aggfuncs.AggPartialResultMapper */>,
//     pub outputCh: channel::Channel</* Go: chan *AfFinalResult */>,
//     pub finalResultHolderCh: channel::Channel</* Go: chan *chunk.Chunk */>,
//     pub spillHelper: Box<parallelHashAggSpillHelper>,
//     pub restoredAggResultMapperMem: i64,
// }
// getInputFromDisk 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn getInputFromDisk(&mut self, /* Go args: sctx sessionctx.Context */) /* Go returns: (ret aggfuncs.AggPartialResultMapper, restoredMem int64, err error) */ {
//     ret, restoredMem, err = w.spillHelper.restoreOnePartition(sctx)
//     w.intestDuringFinalWorkerRun(&err)
//     return ret, restoredMem, err
// }
// }
// getPartialInput 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn getPartialInput(&mut self/* Go args:  */) /* Go returns: (input aggfuncs.AggPartialResultMapper, ok bool) */ {
//     waitStart := time.Now()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
//     defer updateWaitTime(w.stats, waitStart)
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     select {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case <-w.finishCh:
//         return nil, false
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case input, ok = <-w.inputCh:
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if !ok {
//             return nil, false
//         }
//     }
//     return
// }
// }
// mergeInputIntoResultMap 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn mergeInputIntoResultMap(&mut self, /* Go args: sctx sessionctx.Context, input aggfuncs.AggPartialResultMapper */) /* Go returns: error */ {
// As the w.partialResultMap is empty when we get the first input.
// So it's better to directly assign the input to w.partialResultMap
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if len(w.partialResultMap.M) == 0 {
//         w.partialResultMap = input
//         return nil
//     }
//     execStart := time.Now()
//     allMemDelta := int64(0)
//     exprCtx := sctx.GetExprCtx()
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for key, value := range input.M {
//         dstVal, ok := w.partialResultMap.M[key]
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if !ok {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if deltaBytes := w.partialResultMap.Set(key, value); deltaBytes > 0 {
//                 w.memTracker.Consume(deltaBytes)
//             }
//             continue
//         }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for j, af := range w.aggFuncs {
//             memDelta, err := af.MergePartialResult(exprCtx.GetEvalCtx(), value[j], dstVal[j])
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             if err != nil {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//                 return err
//             }
//             allMemDelta += memDelta
//         }
//     }
//     w.memTracker.Consume(allMemDelta)
//     updateExecTime(w.stats, execStart)
//     return nil
// }
// }
// consumeIntermData 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn consumeIntermData(&mut self, /* Go args: sctx sessionctx.Context */) /* Go returns: error */ {
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for {
//         input, ok := w.getPartialInput()
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if !ok {
//             return nil
//         }
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//         failpoint.Inject("ConsumeRandomPanic", nil)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         if err := w.mergeInputIntoResultMap(sctx, input); err != nil {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             return err
//         }
//     }
// }
// }
// generateResultAndSend 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn generateResultAndSend(&mut self, /* Go args: sctx sessionctx.Context, result *chunk.Chunk */) {
//     var finished bool
//     exprCtx := sctx.GetExprCtx()
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, results := range w.partialResultMap.M {
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for j, af := range w.aggFuncs {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             if err := af.AppendFinalResult2Chunk(exprCtx.GetEvalCtx(), results[j], result); err != nil {
//                 logutil.BgLogger().Error("HashAggFinalWorker failed to append final result to Chunk", zap.Error(err))
//             }
//         }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if len(w.aggFuncs) == 0 {
//             result.SetNumVirtualRows(result.NumRows() + 1)
//         }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if result.IsFull() {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//             w.outputCh <- &AfFinalResult{chk: result, giveBackCh: w.finalResultHolderCh}
//             result, finished = w.receiveFinalResultHolder()
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if finished {
//                 return
//             }
//         }
//     }
// }
// }
// sendFinalResult 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn sendFinalResult(&mut self, /* Go args: sctx sessionctx.Context */) {
//     waitStart := time.Now()
//     result, finished := w.receiveFinalResultHolder()
//     updateWaitTime(w.stats, waitStart)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if finished {
//         return
//     }
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//     failpoint.Inject("ConsumeRandomPanic", nil)
//     execStart := time.Now()
//     updateExecTime(w.stats, execStart)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if w.spillHelper.isSpilledChunksIOEmpty() {
//         w.generateResultAndSend(sctx, result)
//     } else {
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if w.checkFinishChClosed() {
//                 return
//             }
//             eof, hasError := w.restoreDataFromDisk(sctx)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if hasError {
//                 return
//             }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if eof {
//                 break
//             }
//             w.generateResultAndSend(sctx, result)
//         }
//     }
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     w.outputCh <- &AfFinalResult{chk: result, giveBackCh: w.finalResultHolderCh}
// }
// }
// restoreDataFromDisk 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn restoreDataFromDisk(&mut self, /* Go args: sctx sessionctx.Context */) /* Go returns: (eof bool, hasError bool) */ {
//     var err error
// Since data is restored partition by partition, only one partition is in memory at any given time.
// Therefore, it's necessary to release the memory used by the previous partition.
//     w.spillHelper.memTracker.Consume(-w.restoredAggResultMapperMem)
//     w.partialResultMap, w.restoredAggResultMapperMem, err = w.getInputFromDisk(sctx)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//     if err != nil {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         w.outputCh <- &AfFinalResult{err: err}
//         return false, true
//     }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if w.partialResultMap == nil {
// All partitions have been restored
//         return true, false
//     }
//     return false, false
// }
// }
// receiveFinalResultHolder 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn receiveFinalResultHolder(&mut self/* Go args:  */) /* Go returns: (*chunk.Chunk, bool) */ {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     select {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case <-w.finishCh:
//         return nil, true
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case result, ok := <-w.finalResultHolderCh:
//         return result, !ok
//     }
// }
// }
// run 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn run(&mut self, /* Go args: ctx sessionctx.Context, waitGroup *sync.WaitGroup, partialWorkerWaiter *sync.WaitGroup */) {
//     start := time.Now()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
//     defer w.cleanup(start, waitGroup)
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     partialWorkerWaiter.Wait()
//     intestBeforeFinalWorkerStart()
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if w.spillHelper.isSpilledChunksIOEmpty() {
//         err := w.consumeIntermData(ctx)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         if err != nil {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             w.outputCh <- &AfFinalResult{err: err}
//             return
//         }
//     } else {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if w.spillHelper.checkError() {
//             return
//         }
//     }
//     w.sendFinalResult(ctx)
// }
// }
// cleanup 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn cleanup(&mut self, /* Go args: start time.Time, waitGroup *sync.WaitGroup */) {
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if r := recover(); r != nil {
//         recoveryHashAgg(w.outputCh, r)
//     }
//     updateWorkerTime(w.stats, start)
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     waitGroup.Done()
// }
// }
// intestBeforeFinalWorkerStart 对应 Go 函数：保留原参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn intestBeforeFinalWorkerStart(/* Go args:  */) {
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//     failpoint.Inject("enableAggSpillIntest", func(val failpoint.Value) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if val.(bool) {
//             num := rand.Intn(50)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if num < 3 {
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//                 panic("Intest panic: final worker is panicked before start")
//             } else if num < 6 {
//                 time.Sleep(1 * time.Millisecond)
//             }
//         }
//     })
// }
// intestDuringFinalWorkerRun 对应 Go 方法：接收者为 `w *HashAggFinalWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggFinalWorker {
//     pub fn intestDuringFinalWorkerRun(&mut self, /* Go args: err *error */) {
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//     failpoint.Inject("enableAggSpillIntest", func(val failpoint.Value) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if val.(bool) {
//             num := rand.Intn(10000)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//             if num < 5 {
// failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
//                 panic("Intest panic: final worker is panicked when running")
//             } else if num < 10 {
//                 time.Sleep(1 * time.Millisecond)
//             } else if num < 15 {
//                 w.memTracker.Consume(1000000)
//             } else if num < 20 {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//                 *err = errors.New("Random fail is triggered in final worker")
//             }
//         }
//     })
// }
// }
// */
use crate::agg_spill::ParallelHashAggSpillHelper;
use crate::agg_util::{AggMap, Aggregation, Chunk};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
/// Final Worker 向主线程回传的一批结果：chunk 或错误信息。
pub struct FinalResult {
    pub chunk: Chunk,
    pub error: Option<String>,
}

/// Final 阶段 worker：持有聚合描述、可选 spill 辅助器与合并后的结果 map。
pub struct HashAggFinalWorker {
    pub aggregations: Arc<Vec<Aggregation>>,
    pub spill: Option<Arc<ParallelHashAggSpillHelper>>,
    result: AggMap,
}

impl HashAggFinalWorker {
    /// 构造 Final Worker，初始结果 map 为空。
    pub fn new(
        aggregations: Arc<Vec<Aggregation>>,
        spill: Option<Arc<ParallelHashAggSpillHelper>>,
    ) -> Self {
        Self {
            aggregations,
            spill,
            result: AggMap::new(),
        }
    }
    /// 将一批 partial map 合并进本 worker 的 result；同 key 调用 AggState::merge。
    pub fn merge_input(&mut self, input: AggMap) -> Result<(), String> {
        for (key, (group, states)) in input {
            let (_, target) = self.result.entry(key).or_insert_with(|| {
                (
                    group,
                    self.aggregations
                        .iter()
                        .map(|_| crate::agg_util::AggState::new())
                        .collect(),
                )
            });
            // partial / final 聚合宽度必须一致，否则无法按列 merge。
            if target.len() != states.len() || target.len() != self.aggregations.len() {
                return Err("partial result width mismatch".to_string());
            }
            for ((target, source), aggregation) in
                target.iter_mut().zip(&states).zip(self.aggregations.iter())
            {
                target.merge(aggregation, source);
            }
        }
        Ok(())
    }
    /// 若配置了 spill，按分区恢复落盘数据并 merge；返回恢复的 group 条目数。
    pub fn restore_from_disk(&mut self) -> Result<usize, String> {
        let Some(spill) = self.spill.clone() else {
            return Ok(0);
        };
        let mut restored = 0;
        while let Some(partition) = spill.next_partition() {
            for input in spill.restore_partition(partition)? {
                restored += input.len();
                self.merge_input(input)?;
            }
        }
        Ok(restored)
    }
    /// 返回当前 final map 中已经完成构建的分组数。
    pub fn hash_state_rows(&self) -> usize {
        self.result.len()
    }
    /// 取出全部 group，附加各聚合最终值，按 max_chunk_size 切成多个 FinalResult。
    pub fn generate_result(&mut self, max_chunk_size: usize) -> Vec<FinalResult> {
        let mut chunks = Vec::new();
        let mut chunk = Chunk::new();
        for (_, (mut group, states)) in std::mem::take(&mut self.result) {
            group.extend(
                states
                    .iter()
                    .zip(self.aggregations.iter())
                    .map(|(state, aggregation)| state.result(aggregation.kind)),
            );
            chunk.push(group);
            // chunk 写满后切分，避免单次向主线程回传过大结果。
            if chunk.len() >= max_chunk_size.max(1) {
                chunks.push(FinalResult {
                    chunk: std::mem::take(&mut chunk),
                    error: None,
                });
            }
        }
        if !chunk.is_empty() {
            chunks.push(FinalResult { chunk, error: None });
        }
        chunks
    }
}
