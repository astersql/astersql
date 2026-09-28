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

// Hash 聚合 Partial Worker：消费 child chunk、更新 partial result、按需 spill。
//
// Partial 阶段对输入行计算 group key、维护 AggMap；内存超限时通过
// `ParallelHashAggSpillHelper` 落盘。shuffle 阶段按 key 哈希分发给 Final Worker。

// HashAggPartialWorker 负责从 child chunk 计算 group key、更新 partial result、按 final worker 分片并在需要时 spill。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// HashAggPartialWorker indicates the partial workers of parallel hash agg execution,
// the number of the worker can be set by `tidb_hashagg_partial_concurrency`.
// HashAggPartialWorker 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct HashAggPartialWorker {
//     pub baseHashAggWorker: baseHashAggWorker,
//     pub idForTest: i32,
//     pub ctx: sessionctx::Context,
//     pub inputCh: channel::Channel</* Go: chan *chunk.Chunk */>,
//     pub outputChs: Vec<channel::Channel</* Go: chan aggfuncs.AggPartialResultMapper */>>,
//     pub globalOutputCh: channel::Channel</* Go: chan *AfFinalResult */>,
// Partial worker transmit the HashAggInput by this channel,
// so that the data fetcher could get the partial worker's HashAggInput
//     pub giveBackCh: channel::Channel</* Go: chan<- *HashAggInput */>,
//     pub partialResultsBuffer: Vec<Vec<aggfuncs::PartialResult>>,
//     pub partialResultNumInRow: i32,
// Length of this map is equal to the number of final workers
// All data in one AggPartialResultMapper are specifically sent to a target final worker.
// e.g. all data in partialResultsMap[3] should be sent to final worker 3.
//     pub partialResultsMap: Vec<aggfuncs::AggPartialResultMapper>,
//     pub partialResultsMapMem: atomic::Int64,
//     pub groupByItems: Vec<expression::Expression>,
//     pub groupKeyBuf: Vec<Vec<byte>>,
// chk stores the input data from child,
// and is reused by childExec and partial worker.
//     pub chk: Box<chunk::Chunk>,
//     pub isSpillPrepared: bool,
//     pub spillHelper: Box<parallelHashAggSpillHelper>,
//     pub tmpChksForSpill: Vec<Box<chunk::Chunk>>,
//     pub serializeHelpers: Box<aggfuncs::SerializeHelper>,
//     pub spilledChunksIO: Vec<Box<chunk::DataInDiskByChunks>>,
// It's useful when spill is triggered and the fetcher could know when partial workers finish their works.
//     pub inflightChunkSync: Box<sync::WaitGroup>,
//     pub fileNamePrefixForTest: String,
// }
// getChildInput 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
// impl HashAggPartialWorker {
//     pub fn getChildInput(&mut self/* Go args:  */) /* Go returns: (*chunk.Chunk, bool) */ {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     select {
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case <-w.finishCh:
//         return nil, false
// channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
//     case chk, ok := <-w.inputCh:
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if !ok {
//             return nil, false
//         }
//         return chk, true
//     }
// }
// }
// */
use crate::agg_spill::ParallelHashAggSpillHelper;
use crate::agg_util::{AggMap, AggState, Aggregation, Chunk, Row, get_group_key};
use std::sync::Arc;

/// `twmb/murmur3.Sum32`: MurmurHash3 x86 32-bit with seed 0.
pub(crate) fn murmur3_sum32(input: &[u8]) -> u32 {
    let mut hash = 0_u32;
    let mut chunks = input.chunks_exact(4);
    for chunk in &mut chunks {
        let mut key = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        key = key.wrapping_mul(0xcc9e_2d51);
        key = key.rotate_left(15);
        key = key.wrapping_mul(0x1b87_3593);
        hash ^= key;
        hash = hash.rotate_left(13);
        hash = hash.wrapping_mul(5).wrapping_add(0xe654_6b64);
    }

    let tail = chunks.remainder();
    let mut key = 0_u32;
    match tail.len() {
        3 => {
            key ^= u32::from(tail[2]) << 16;
            key ^= u32::from(tail[1]) << 8;
            key ^= u32::from(tail[0]);
        }
        2 => {
            key ^= u32::from(tail[1]) << 8;
            key ^= u32::from(tail[0]);
        }
        1 => key ^= u32::from(tail[0]),
        _ => {}
    }
    if !tail.is_empty() {
        key = key.wrapping_mul(0xcc9e_2d51);
        key = key.rotate_left(15);
        key = key.wrapping_mul(0x1b87_3593);
        hash ^= key;
    }

    hash ^= input.len() as u32;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^ (hash >> 16)
}

/// Partial Worker 状态：分组列、聚合描述、可选 spill、内存估算与 AggMap。
pub struct HashAggPartialWorker {
    pub group_columns: Vec<usize>,
    pub aggregations: Arc<Vec<Aggregation>>,
    pub spill: Option<Arc<ParallelHashAggSpillHelper>>,
    map: AggMap,
    memory_usage: usize,
}

impl HashAggPartialWorker {
    /// 构造 Partial Worker，初始 map 为空。
    pub fn new(
        group_columns: Vec<usize>,
        aggregations: Arc<Vec<Aggregation>>,
        spill: Option<Arc<ParallelHashAggSpillHelper>>,
    ) -> Self {
        Self {
            group_columns,
            aggregations,
            spill,
            map: AggMap::new(),
            memory_usage: 0,
        }
    }

    /// 消费一个输入 chunk：编码 group key、更新各聚合态，必要时触发 spill。
    pub fn update_partial_result(&mut self, chunk: &Chunk) -> Result<(), String> {
        for row in chunk {
            let key = get_group_key(row, &self.group_columns)?;
            let group_row = self
                .group_columns
                .iter()
                .map(|index| {
                    row.get(*index)
                        .cloned()
                        .ok_or_else(|| format!("group column {index} out of range"))
                })
                .collect::<Result<Row, _>>()?;
            // 仅新 group 计入内存增量，避免重复累加已有槽位。
            let is_new = !self.map.contains_key(&key);
            let (_, states) = self.map.entry(key.clone()).or_insert_with(|| {
                (
                    group_row,
                    self.aggregations.iter().map(|_| AggState::new()).collect(),
                )
            });
            for (state, aggregation) in states.iter_mut().zip(self.aggregations.iter()) {
                state.update(aggregation, row)?;
            }
            if is_new {
                self.memory_usage = self
                    .memory_usage
                    .saturating_add(key.len() + states.len() * std::mem::size_of::<AggState>());
            }
        }
        if let Some(spill) = &self.spill {
            // 超限则交出当前 map 落盘，并清零内存计数。
            if spill.set_need_spill(self.memory_usage) {
                let data = std::mem::take(&mut self.map);
                spill.spill(data)?;
                self.memory_usage = 0;
            }
        }
        Ok(())
    }

    /// 按 group key 哈希把中间结果分片到各 Final Worker 对应的 map。
    pub fn shuffle_intermediate_data(&mut self, final_concurrency: usize) -> Vec<AggMap> {
        let mut outputs = (0..final_concurrency.max(1))
            .map(|_| AggMap::new())
            .collect::<Vec<_>>();
        for (key, value) in std::mem::take(&mut self.map) {
            let worker = murmur3_sum32(&key) as usize % outputs.len();
            outputs[worker].insert(key, value);
        }
        self.memory_usage = 0;
        outputs
    }

    /// 强制把残留 map 落盘（收尾阶段）；未配置 spill 则报错。
    pub fn spill_remaining(&mut self) -> Result<(), String> {
        if self.map.is_empty() {
            return Ok(());
        }
        match &self.spill {
            Some(spill) => {
                let data = std::mem::take(&mut self.map);
                spill.spill(data)?;
                self.memory_usage = 0;
                Ok(())
            }
            None => Err("spill helper is not configured".to_string()),
        }
    }
}
/*

// fetchChunkAndProcess 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn fetchChunkAndProcess(&mut self, /* Go args: ctx sessionctx.Context, hasError *bool, needShuffle *bool */) /* Go returns: bool */ {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if w.spillHelper.checkError() {
        *hasError = true
        return false
    }

    waitStart := time.Now()
    chk, ok := w.getChildInput()
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if !ok {
        return false
    }

    // 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    defer w.inflightChunkSync.Done()
    updateWaitTime(w.stats, waitStart)

    w.intestDuringPartialWorkerRun()

    w.chk.SwapColumns(chk)
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
    w.giveBackCh <- &HashAggInput{
        chk:        chk,
        giveBackCh: w.inputCh,
    }

    execStart := time.Now()
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    if err := w.updatePartialResult(ctx, w.chk, len(w.partialResultsMap)); err != nil {
        *hasError = true
        w.processError(err)
        return false
    }
    updateExecTime(w.stats, execStart)

    // The intermData can be promised to be not empty if reaching here,
    // so we set needShuffle to be true.
    *needShuffle = true

    w.intestDuringPartialWorkerRun()
    return true
}
}

// intestDuringPartialWorkerRun 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn intestDuringPartialWorkerRun(&mut self/* Go args:  */) {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("enableAggSpillIntest", func(val failpoint.Value) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if val.(bool) {
            num := rand.Intn(10000)
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if num < 3 {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
                panic("Intest panic: partial worker is panicked when running")
            } else if num < 6 {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                w.processError(errors.Errorf("Random fail is triggered in partial worker"))
            } else if num < 9 {
                consumedMem := int64(500000)
                w.memTracker.Consume(consumedMem)
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
                w.partialResultsMapMem.Add(consumedMem)
            }

            // Slow some partial workers
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if w.idForTest%2 == 0 && num < 15 {
                time.Sleep(1 * time.Millisecond)
            }
        }
    })

    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("slowSomePartialWorkers", func(val failpoint.Value) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if val.(bool) {
            num := rand.Intn(10000)
            // Slow some partial workers
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if w.idForTest%2 == 0 && num < 10 {
                time.Sleep(1 * time.Millisecond)
            }
        }
    })
}
}

// intestBeforePartialWorkerRun 对应 Go 函数：保留原参数含义、控制流、错误返回和外部依赖调用顺序。
pub fn intestBeforePartialWorkerRun(/* Go args:  */) {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("enableAggSpillIntest", func(val failpoint.Value) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if val.(bool) {
            num := rand.Intn(100)
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if num < 2 {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
                panic("Intest panic: partial worker is panicked before start")
            } else if num >= 2 && num < 4 {
                time.Sleep(1 * time.Millisecond)
            }
        }
    })
}

// finalizeWorkerProcess 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn finalizeWorkerProcess(&mut self, /* Go args: needShuffle bool, finalConcurrency int, hasError bool */) {
    // Consume all chunks to avoid hang of fetcher
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for range w.inputCh {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        w.inflightChunkSync.Done()
    }

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if w.checkFinishChClosed() {
        return
    }

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if hasError {
        return
    }

    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if needShuffle && w.spillHelper.isSpilledChunksIOEmpty() {
        w.shuffleIntermData(finalConcurrency)
    }
}
}

// run 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn run(&mut self, /* Go args: ctx sessionctx.Context, waitGroup *sync.WaitGroup, finalConcurrency int */) {
    start := time.Now()
    hasError := false
    needShuffle := false

    // 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
    defer func() {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if r := recover(); r != nil {
            recoveryHashAgg(w.globalOutputCh, r)
        }

        w.finalizeWorkerProcess(needShuffle, finalConcurrency, hasError)

        w.memTracker.Consume(-w.chk.MemoryUsage())
        updateWorkerTime(w.stats, start)

    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
        // We must ensure that there is no panic before `waitGroup.Done()` or there will be hang
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
        waitGroup.Done()

        tryRecycleBuffer(&w.partialResultsBuffer, &w.groupKeyBuf)
    }()

    intestBeforePartialWorkerRun()

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for w.fetchChunkAndProcess(ctx, &hasError, &needShuffle) {
    }
}
}

// If the group key has appeared before, reuse the partial result.
// If the group key has not appeared before, create empty partial results.
// getPartialResultsOfEachRow 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn getPartialResultsOfEachRow(&mut self, /* Go args: groupKey [][]byte, finalConcurrency int */) /* Go returns: [][]aggfuncs.PartialResult */ {
    mapper := w.partialResultsMap
    numRows := len(groupKey)
    allMemDelta := int64(0)
    w.partialResultsBuffer = w.partialResultsBuffer[0:0]

    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range numRows {
        finalWorkerIdx := int(murmur3.Sum32(groupKey[i])) % finalConcurrency
        tmp, ok := mapper[finalWorkerIdx].M[string(hack.String(groupKey[i]))]

        // This group by key has appeared before, reuse the partial result.
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if ok {
            w.partialResultsBuffer = append(w.partialResultsBuffer, tmp)
            continue
        }

        // It's the first time that this group by key appeared, create it
        w.partialResultsBuffer = append(w.partialResultsBuffer, make([]aggfuncs.PartialResult, w.partialResultNumInRow))
        lastIdx := len(w.partialResultsBuffer) - 1
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for j, af := range w.aggFuncs {
            partialResult, memDelta := af.AllocPartialResult()
            w.partialResultsBuffer[lastIdx][j] = partialResult
            allMemDelta += memDelta // the memory usage of PartialResult
        }
        allMemDelta += int64(w.partialResultNumInRow * 8)
        delta := mapper[finalWorkerIdx].Set(string(groupKey[i]), w.partialResultsBuffer[lastIdx])
        allMemDelta += int64(len(groupKey[i]))
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if delta > 0 {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
            w.partialResultsMapMem.Add(delta)
            w.memTracker.Consume(delta)
        }
    }
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    w.partialResultsMapMem.Add(allMemDelta)
    w.memTracker.Consume(allMemDelta)
    return w.partialResultsBuffer
}
}

// updatePartialResult 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn updatePartialResult(&mut self, /* Go args: ctx sessionctx.Context, chk *chunk.Chunk, finalConcurrency int */) /* Go returns: (err error) */ {
    memSize := getGroupKeyMemUsage(w.groupKeyBuf)
    w.groupKeyBuf, err = GetGroupKey(w.ctx, chk, w.groupKeyBuf, w.groupByItems)
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    failpoint.Inject("ConsumeRandomPanic", nil)
    w.memTracker.Consume(getGroupKeyMemUsage(w.groupKeyBuf) - memSize)
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        return err
    }

    partialResultOfEachRow := w.getPartialResultsOfEachRow(w.groupKeyBuf, finalConcurrency)

    numRows := chk.NumRows()
    rows := make([]chunk.Row, 1)
    allMemDelta := int64(0)
    exprCtx := ctx.GetExprCtx()
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range numRows {
        partialResult := partialResultOfEachRow[i]
        rows[0] = chk.GetRow(i)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for j, af := range w.aggFuncs {
            memDelta, err := af.UpdatePartialResult(exprCtx.GetEvalCtx(), rows, partialResult[j])
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                return err
            }
            allMemDelta += memDelta
        }
    }
    w.memTracker.Consume(allMemDelta)
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
    w.partialResultsMapMem.Add(allMemDelta)
    return nil
}
}

// shuffleIntermData 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn shuffleIntermData(&mut self, /* Go args: finalConcurrency int */) {
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range finalConcurrency {
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
        w.outputChs[i] <- w.partialResultsMap[i]
    }
}
}

// prepareForSpill 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn prepareForSpill(&mut self/* Go args:  */) {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if !w.isSpillPrepared {
        w.tmpChksForSpill = make([]*chunk.Chunk, spilledPartitionNum)
        w.spilledChunksIO = make([]*chunk.DataInDiskByChunks, spilledPartitionNum)
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for i := range spilledPartitionNum {
            w.tmpChksForSpill[i] = w.spillHelper.getNewSpillChunkFunc()
            w.spilledChunksIO[i] = chunk.NewDataInDiskByChunks(w.spillHelper.spillChunkFieldTypes, w.fileNamePrefixForTest)
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if w.spillHelper.diskTracker != nil {
                w.spilledChunksIO[i].GetDiskTracker().AttachTo(w.spillHelper.diskTracker)
            }
        }
        w.isSpillPrepared = true
    }
}
}

// spillDataToDisk 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn spillDataToDisk(&mut self/* Go args:  */) /* Go returns: error */ {
    err := w.spillDataToDiskImpl()
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    if err == nil {
        err = failpointError()
    }
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    return err
}
}

// spillDataToDiskImpl 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn spillDataToDiskImpl(&mut self/* Go args:  */) /* Go returns: error */ {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
    if len(w.partialResultsMap) == 0 {
        return nil
    }

    // 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
    defer func() {
    // failpoint/panic 说明：测试注入和 recover 路径按 Go 语义保留为占位。
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if r := recover(); r != nil {
            recoveryHashAgg(w.globalOutputCh, r)
        }

        // Clear the partialResultsMap
        w.partialResultsMap = make([]aggfuncs.AggPartialResultMapper, len(w.partialResultsMap))
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for i := range w.partialResultsMap {
            w.partialResultsMap[i] = aggfuncs.NewAggPartialResultMapper()
        }

    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
        w.memTracker.Consume(-w.partialResultsMapMem.Load())
    // 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
        w.partialResultsMapMem.Store(0)
    }()

    w.prepareForSpill()
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for _, partialResultsMap := range w.partialResultsMap {
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
        for key, partialResults := range partialResultsMap.M {
            partitionNum := int(murmur3.Sum32(hack.Slice(key))) % spilledPartitionNum

            // Spill data when tmp chunk is full
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
            if w.tmpChksForSpill[partitionNum].IsFull() {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
                err := w.spilledChunksIO[partitionNum].Add(w.tmpChksForSpill[partitionNum])
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                    return err
                }
                w.tmpChksForSpill[partitionNum].Reset()
            }

            // Serialize agg meta data to the tmp chunk
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
            for i, aggFunc := range w.aggFuncs {
                aggFunc.SerializePartialResult(partialResults[i], w.tmpChksForSpill[partitionNum], w.serializeHelpers)
            }

            // Append key
            w.tmpChksForSpill[partitionNum].AppendString(len(w.aggFuncs), key)
        }
    }

    // Trigger the spill of remaining data
    err := w.spillRemainingDataToDisk()
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
        return err
    }
    return nil
}
}

// Some tmp chunks may no be full, so we need to manually trigger the spill action.
// spillRemainingDataToDisk 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn spillRemainingDataToDisk(&mut self/* Go args:  */) /* Go returns: error */ {
    // 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
    for i := range spilledPartitionNum {
    // 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
        if w.tmpChksForSpill[i].NumRows() > 0 {
    // 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
            err := w.spilledChunksIO[i].Add(w.tmpChksForSpill[i])
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
            if err != nil {
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
                return err
            }
            w.tmpChksForSpill[i].Reset()
        }
    }
    return nil
}
}

// processError 对应 Go 方法：接收者为 `w *HashAggPartialWorker`，保留控制流、错误返回和资源处理顺序。
impl HashAggPartialWorker {
    pub fn processError(&mut self, /* Go args: err error */) {
    // channel 说明：这里对应 Go channel 发送/接收或 select，不建立真实异步运行时。
    // 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
    w.globalOutputCh <- &AfFinalResult{err: err}
    w.spillHelper.setError()
}
}
*/
