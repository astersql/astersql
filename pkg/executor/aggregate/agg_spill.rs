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

// 并行 Hash 聚合的 spill（落盘）状态机与分区恢复。
//
// 当聚合占用内存超过配额时，把 AggMap 按 key 哈希拆到多个分区写入；
// Final Worker 再按分区游标依次 restore。`has_enough_data_to_spill` 用于
// OOM Action 判断是否值得触发落盘。

// 聚合 spill 辅助结构和 OOM action，保留并行 HashAgg spill 状态机、分区恢复和内存动作触发顺序。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// 关键分支、参数解析、资源收尾、错误处理，以及并发、异步、IO、外部依赖旁保留中文说明；跨包类型与调用均是后续接线占位。
// spillStatus 对应 Go type 声明，保留底层类型语义。
// pub type spillStatus = i32;
// Go const noSpill：保持原常量/iota 语义。
// pub const noSpill: spillStatus = 0;
// Go const needSpill：保持原常量/iota 语义。
// pub const needSpill: spillStatus = 1;
// Go const inSpilling：保持原常量/iota 语义。
// pub const inSpilling: spillStatus = 2;
// Go const spillTriggered：保持原常量/iota 语义。
// pub const spillTriggered: spillStatus = 3;
// maxSpillTimes indicates how many times the data can spill at most.
// Go const maxSpillTimes：保持原常量/iota 语义。
// pub const maxSpillTimes: i32 = 10;
// Go const spilledPartitionNum：保持原常量/iota 语义。
// pub const spilledPartitionNum: i32 = 256;
// Go const spillLogInfo：保持原常量/iota 语义。
// pub const spillLogInfo: String = "memory exceeds quota, set aggregate mode to spill-mode";
// parallelHashAggSpillHelper 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct parallelHashAggSpillHelper {
//     pub lock: parallelHashAggSpillHelperLock,
//     pub memTracker: Box<memory::Tracker>,
//     pub diskTracker: Box<disk::Tracker>,
//     pub hasError: atomic::Bool,
// These agg functions are partial agg functions that are same with partial workers'.
// They only be used for restoring data that are spilled to disk in partial stage.
//     pub aggFuncsForRestoring: Vec<aggfuncs::AggFunc>,
//     pub finalWorkerAggFuncs: Vec<aggfuncs::AggFunc>,
//     pub getNewSpillChunkFunc: Box<dyn Fn() /* Go: func() *chunk.Chunk */>,
//     pub spillChunkFieldTypes: Vec<Box<types::FieldType>>,
// }
// parallelHashAggSpillHelperLock 对应 Go 中内嵌的匿名 lock struct。
// 这里拆成具名类型，保留 Mutex/Cond、分区游标、spill IO 列表和内存阈值字段顺序。
// pub struct parallelHashAggSpillHelperLock {
//     pub sync_Mutex: Box<sync::Mutex>,
//     pub waitIfInSpilling: Box<sync::Cond>,
//     pub nextPartitionIdx: i32,
//     pub spilledChunksIO: Vec<Vec<Box<chunk::DataInDiskByChunks>>>,
//     pub status: spillStatus,
//     pub memoryConsumption: i64,
//     pub memoryQuota: i64,
// }
// newSpillHelper 对应 Go 函数：保留原参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn newSpillHelper(/* Go args: tracker *memory.Tracker, aggFuncsForRestoring []aggfuncs.AggFunc, finalWorkerAggFuncs []aggfuncs.AggFunc, getNewSpillChunkFunc func() *chunk.Chunk, spillChunkFieldTypes []*types.FieldType */) /* Go returns: (*parallelHashAggSpillHelper, error) */ {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if len(aggFuncsForRestoring) != len(finalWorkerAggFuncs) {
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         return nil, errors.NewNoStackError("len(aggFuncsForRestoring) != len(finalWorkerAggFuncs)")
//     }
//     mu := new(sync.Mutex)
//     helper := &parallelHashAggSpillHelper{
//         lock: struct {
//             *sync.Mutex
//             waitIfInSpilling  *sync.Cond
//             nextPartitionIdx  int
//             spilledChunksIO   [][]*chunk.DataInDiskByChunks
//             status            spillStatus
//             memoryConsumption int64
//             memoryQuota       int64
//         }{
//             Mutex:             mu,
//             waitIfInSpilling:  sync.NewCond(mu),
//             spilledChunksIO:   make([][]*chunk.DataInDiskByChunks, spilledPartitionNum),
//             status:            noSpill,
//             nextPartitionIdx:  spilledPartitionNum - 1,
//             memoryConsumption: 0,
//             memoryQuota:       0,
//         },
//         memTracker:           tracker,
// 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
//         hasError:             atomic.Bool{},
//         aggFuncsForRestoring: aggFuncsForRestoring,
//         finalWorkerAggFuncs:  finalWorkerAggFuncs,
//         getNewSpillChunkFunc: getNewSpillChunkFunc,
//         spillChunkFieldTypes: spillChunkFieldTypes,
//     }
//     return helper, nil
// }
// close 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn close(&mut self/* Go args:  */) {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, ios := range p.lock.spilledChunksIO {
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for _, io := range ios {
//             io.Close()
//         }
//     }
// }
// }
// isSpilledChunksIOEmpty 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn isSpilledChunksIOEmpty(&mut self/* Go args:  */) /* Go returns: bool */ {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for i := range p.lock.spilledChunksIO {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if len(p.lock.spilledChunksIO[i]) > 0 {
//             return false
//         }
//     }
//     return true
// }
// }
// getNextPartition 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn getNextPartition(&mut self/* Go args:  */) /* Go returns: (int, bool) */ {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
//     partitionIdx := p.lock.nextPartitionIdx
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if partitionIdx < 0 {
//         return -1, false
//     }
//     p.lock.nextPartitionIdx--
//     return partitionIdx, true
// }
// }
// addListInDisks 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn addListInDisks(&mut self, /* Go args: dataInDisk []*chunk.DataInDiskByChunks */) {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for i, data := range dataInDisk {
//         p.lock.spilledChunksIO[i] = append(p.lock.spilledChunksIO[i], data)
//     }
// }
// }
// getListInDisks 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn getListInDisks(&mut self, /* Go args: partitionNum int */) /* Go returns: []*chunk.DataInDiskByChunks */ {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
//     return p.lock.spilledChunksIO[partitionNum]
// }
// }
// setInSpilling 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn setInSpilling(&mut self/* Go args:  */) {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
//     p.lock.status = inSpilling
//     logutil.BgLogger().Info(spillLogInfo,
//         zap.Int64("consumed", p.lock.memoryConsumption),
//         zap.Int64("quota", p.lock.memoryQuota))
// }
// }
// isNoSpill 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn isNoSpill(&mut self/* Go args:  */) /* Go returns: bool */ {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
//     return p.lock.status == noSpill
// }
// }
// setSpillTriggered 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn setSpillTriggered(&mut self/* Go args:  */) {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
//     p.lock.status = spillTriggered
//     p.lock.waitIfInSpilling.Broadcast()
// }
// }
// checkNeedSpill 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn checkNeedSpill(&mut self/* Go args:  */) /* Go returns: bool */ {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
//     return p.lock.status == needSpill
// }
// }
// Return true if we successfully set flag
// setNeedSpill 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn setNeedSpill(&mut self, /* Go args: executorTracker *memory.Tracker, triggeredTracker *memory.Tracker */) /* Go returns: bool */ {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if hasEnoughDataToSpill(executorTracker, triggeredTracker) {
//         p.lock.status = needSpill
//         p.lock.memoryConsumption = triggeredTracker.BytesConsumed()
//         p.lock.memoryQuota = triggeredTracker.GetBytesLimit()
//         return true
//     }
//     return false
// }
// }
// waitForTheEndOfSpill 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn waitForTheEndOfSpill(&mut self/* Go args:  */) {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     p.lock.Lock()
// 资源收尾说明：Go defer 延迟执行清理、统计或错误回收；先保留收尾时机。
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//     defer p.lock.Unlock()
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for p.lock.status == inSpilling {
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//         p.lock.waitIfInSpilling.Wait()
//     }
// }
// }
// We need to check error with atmoic as multi partial workers may access it.
// checkError 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn checkError(&mut self/* Go args:  */) /* Go returns: bool */ {
// 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
//     return p.hasError.Load()
// }
// }
// We need to set error with atmoic as multi partial workers may access it.
// setError 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn setError(&mut self/* Go args:  */) {
// 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
//     p.hasError.Store(true)
// }
// }
// restoreOnePartition 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn restoreOnePartition(&mut self, /* Go args: ctx sessionctx.Context */) /* Go returns: (aggfuncs.AggPartialResultMapper, int64, error) */ {
//     restoredData := aggfuncs.NewAggPartialResultMapper()
//     restoredMem := int64(0)
//     restoredPartitionIdx, isSuccess := p.getNextPartition()
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if !isSuccess {
//         return nil, restoredMem, nil
//     }
//     spilledFilesIO := p.getListInDisks(restoredPartitionIdx)
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for _, spilledFile := range spilledFilesIO {
//         memDelta, expandMem, err := p.restoreFromOneSpillFile(ctx, restoredData, spilledFile)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         if err != nil {
//             return nil, restoredMem, err
//         }
//         p.memTracker.Consume(memDelta)
//         restoredMem += memDelta + expandMem
//     }
//     return restoredData, restoredMem, nil
// }
// }
// processRowContext 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct processRowContext {
//     pub ctx: sessionctx::Context,
//     pub chunk: Box<chunk::Chunk>,
//     pub rowPos: i32,
//     pub keyColPos: i32,
//     pub aggFuncNum: i32,
//     pub restoreadData: aggfuncs::AggPartialResultMapper,
//     pub partialResultsRestored: Vec<Vec<aggfuncs::PartialResult>>,
// }
// restoreFromOneSpillFile 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn restoreFromOneSpillFile(&mut self, /* Go args: ctx sessionctx.Context, restoreadData aggfuncs.AggPartialResultMapper, diskIO *chunk.DataInDiskByChunks */) /* Go returns: (totalMemDelta int64, totalExpandMem int64, err error) */ {
//     chunkNum := diskIO.NumChunks()
//     aggFuncNum := len(p.aggFuncsForRestoring)
//     processRowContext := &processRowContext{
//         ctx:                    ctx,
//         chunk:                  nil, // Will be set in the loop
//         rowPos:                 0,   // Will be set in the loop
//         keyColPos:              aggFuncNum,
//         aggFuncNum:             aggFuncNum,
//         restoreadData:          restoreadData,
//         partialResultsRestored: make([][]aggfuncs.PartialResult, aggFuncNum),
//     }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//     for i := range chunkNum {
//         chunk, err := diskIO.GetChunk(i)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//         if err != nil {
//             return totalMemDelta, totalExpandMem, err
//         }
// Deserialize bytes to agg function's meta data
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for aggPos, aggFunc := range p.aggFuncsForRestoring {
//             partialResult, memDelta := aggFunc.DeserializePartialResult(chunk)
//             processRowContext.partialResultsRestored[aggPos] = partialResult
//             totalMemDelta += memDelta
//         }
// Merge or create results
//         rowNum := chunk.NumRows()
//         processRowContext.chunk = chunk
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for rowPos := range rowNum {
//             processRowContext.rowPos = rowPos
//             memDelta, expandMem, err := p.processRow(processRowContext)
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             if err != nil {
//                 return totalMemDelta, totalExpandMem, err
//             }
//             totalMemDelta += memDelta
//             totalExpandMem += expandMem
//         }
//     }
//     return totalMemDelta, totalExpandMem, nil
// }
// }
// processRow 对应 Go 方法：接收者为 `p *parallelHashAggSpillHelper`，保留控制流、错误返回和资源处理顺序。
// impl parallelHashAggSpillHelper {
//     pub fn processRow(&mut self, /* Go args: context *processRowContext */) /* Go returns: (totalMemDelta int64, expandMem int64, err error) */ {
//     key := context.chunk.GetRow(context.rowPos).GetString(context.keyColPos)
//     prs, ok := context.restoreadData.M[key]
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if ok {
//         exprCtx := context.ctx.GetExprCtx()
// The key has appeared before, merge results.
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for aggPos := range context.aggFuncNum {
//             memDelta, err := p.finalWorkerAggFuncs[aggPos].MergePartialResult(exprCtx.GetEvalCtx(), context.partialResultsRestored[aggPos][context.rowPos], prs[aggPos])
// 错误处理说明：保留 Go err 变量的传播路径，错误类型后续统一接线。
//             if err != nil {
//                 return totalMemDelta, 0, err
//             }
//             totalMemDelta += memDelta
//         }
//     } else {
//         totalMemDelta += int64(len(key))
//         results := make([]aggfuncs.PartialResult, context.aggFuncNum)
//         delta := context.restoreadData.Set(key, results)
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//         if delta > 0 {
//             p.memTracker.Consume(delta)
//         }
// 循环说明：保留 Go 的迭代边界和提前退出条件，slice/chunk 的所有权语义暂不展开。
//         for aggPos := range context.aggFuncNum {
//             results[aggPos] = context.partialResultsRestored[aggPos][context.rowPos]
//         }
//     }
//     return totalMemDelta, expandMem, nil
// }
// }
// Guarantee that processed data is at least 20% of the threshold, to avoid spilling too frequently.
// hasEnoughDataToSpill 对应 Go 函数：保留原参数含义、控制流、错误返回和外部依赖调用顺序。
// pub fn hasEnoughDataToSpill(/* Go args: aggTracker *memory.Tracker, passedInTracker *memory.Tracker */) /* Go returns: bool */ {
//     return aggTracker.BytesConsumed() >= passedInTracker.GetBytesLimit()/5
// }
// AggSpillDiskAction implements memory.ActionOnExceed for unparalleled HashAgg.
// If the memory quota of a query is exceeded, AggSpillDiskAction.Action is
// triggered.
// AggSpillDiskAction 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct AggSpillDiskAction {
//     pub memory_BaseOOMAction: memory::BaseOOMAction,
//     pub e: Box<HashAggExec>,
//     pub spillTimes: u32,
// }
// Action set HashAggExec spill mode.
// Action 对应 Go 方法：接收者为 `a *AggSpillDiskAction`，保留控制流、错误返回和资源处理顺序。
// impl AggSpillDiskAction {
//     pub fn Action(&mut self, /* Go args: t *memory.Tracker */) {
// 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if atomic.LoadUint32(&a.e.inSpillMode) == 0 && hasEnoughDataToSpill(a.e.memTracker, t) && a.spillTimes < maxSpillTimes {
//         a.spillTimes++
//         logutil.BgLogger().Info(spillLogInfo,
//             zap.Uint32("spillTimes", a.spillTimes),
//             zap.Int64("consumed", t.BytesConsumed()),
//             zap.Int64("quota", t.GetBytesLimit()))
// 原子状态说明：保留 Go atomic 标志的读写位置，用于表达 worker 生命周期。
//         atomic.StoreUint32(&a.e.inSpillMode, 1)
// 并发同步说明：互斥锁、条件变量或 WaitGroup 只保留 Go 同步顺序。
//         memory.QueryForceDisk.Add(1)
//         return
//     }
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if fallback := a.GetFallback(); fallback != nil {
//         fallback.Action(t)
//     }
// }
// }
// GetPriority get the priority of the Action
// GetPriority 对应 Go 方法：接收者为 `*AggSpillDiskAction`，保留控制流、错误返回和资源处理顺序。
// impl AggSpillDiskAction {
//     pub fn GetPriority(&mut self/* Go args:  */) /* Go returns: int64 */ {
//     return memory.DefSpillPriority
// }
// }
// ParallelAggSpillDiskAction implements memory.ActionOnExceed for parallel HashAgg.
// ParallelAggSpillDiskAction 对应 Go struct，字段顺序保持不变；指针、slice、map、channel 的所有权语义均待后续接线。
// pub struct ParallelAggSpillDiskAction {
//     pub memory_BaseOOMAction: memory::BaseOOMAction,
//     pub e: Box<HashAggExec>,
//     pub spillHelper: Box<parallelHashAggSpillHelper>,
// }
// Action set HashAggExec spill mode.
// Action 对应 Go 方法：接收者为 `p *ParallelAggSpillDiskAction`，保留控制流、错误返回和资源处理顺序。
// impl ParallelAggSpillDiskAction {
//     pub fn Action(&mut self, /* Go args: t *memory.Tracker */) {
// 分支说明：保留 Go 的条件判断顺序，涉及 nil/channel/state 的语义后续再接线。
//     if p.actionImpl(t) {
//         return
//     }
//     p.TriggerFallBackAction(t)
// }
// }
// */
use crate::agg_hash_partial_worker::murmur3_sum32;
use crate::agg_util::AggMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Spill 状态：未触发 / 需落盘 / 落盘中 / 已触发过。
pub enum SpillStatus {
    NoSpill = 0,
    NeedSpill = 1,
    Spilling = 2,
    Triggered = 3,
}

/// 并行 spill 辅助器：分区锁列表、下一分区游标、状态、错误标志与内存上限。
pub struct ParallelHashAggSpillHelper {
    storage: Mutex<crate::agg_hash_partial_worker::PartialResultSpill>,
    next_partition: AtomicUsize,
    status: AtomicU8,
    error: AtomicBool,
    memory_limit: usize,
}

impl ParallelHashAggSpillHelper {
    /// 创建至少 1 个分区的 spill 辅助器。
    pub fn new(partition_count: usize, memory_limit: usize) -> Self {
        Self {
            storage: Mutex::new(crate::agg_hash_partial_worker::PartialResultSpill::new(
                partition_count,
                1,
                1024,
            )),
            // Go starts at `spilledPartitionNum - 1` and decrements after each claim.
            // Store the exclusive upper bound so concurrent callers can claim the same
            // descending sequence without an integer underflow sentinel.
            next_partition: AtomicUsize::new(partition_count.max(1)),
            status: AtomicU8::new(SpillStatus::NoSpill as u8),
            error: AtomicBool::new(false),
            memory_limit,
        }
    }
    /// 读取当前 spill 状态枚举。
    pub fn status(&self) -> SpillStatus {
        match self.status.load(Ordering::Acquire) {
            1 => SpillStatus::NeedSpill,
            2 => SpillStatus::Spilling,
            3 => SpillStatus::Triggered,
            _ => SpillStatus::NoSpill,
        }
    }
    /// 内存超限时 CAS 置 NeedSpill；成功返回 true，表示调用方应执行 spill。
    pub fn set_need_spill(&self, memory_usage: usize) -> bool {
        if memory_usage < self.memory_limit / 5 {
            return false;
        }
        loop {
            let status = self.status.load(Ordering::Acquire);
            if status == SpillStatus::Spilling as u8 {
                return false;
            }
            if status == SpillStatus::NeedSpill as u8 {
                return true;
            }
            match self.status.compare_exchange(
                status,
                SpillStatus::NeedSpill as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(_) => continue,
            }
        }
    }
    /// 将 AggMap 按 key 哈希拆入各分区并标记 Triggered；返回写入条目数。

    pub fn spill(&self, data: AggMap) -> Result<usize, String> {
        use astersql_executor_aggfuncs::StateSerializer;
        let count = data.len();
        self.status
            .store(SpillStatus::Spilling as u8, Ordering::Release);
        let map = data
            .into_iter()
            .map(|(key, (group, states))| {
                (
                    key,
                    vec![Box::new(SpillEntry { group, states })
                        as astersql_executor_aggfuncs::PartialResult],
                )
            })
            .collect();
        let function = StateSerializer {
            ordinal: 0,
            template: SpillEntry::default(),
        };
        let result = self
            .storage
            .lock()
            .map_err(|_| "spill storage poisoned".to_string())
            .and_then(|mut storage| storage.spill_maps(vec![map], &[&function]));
        if result.is_err() {
            self.set_error();
        }
        self.status
            .store(SpillStatus::Triggered as u8, Ordering::Release);
        result.map(|_| count)
    }
    pub fn disk_bytes(&self) -> i64 {
        self.storage.lock().unwrap().disk_bytes()
    }
    pub fn buffered_groups(&self) -> usize {
        0
    }
    /// 原子递减分区游标，按 Go 的最高分区到 0 的顺序供 restore 消费。
    pub fn next_partition(&self) -> Option<usize> {
        let previous = self
            .next_partition
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                next.checked_sub(1)
            })
            .ok()?;
        Some(previous - 1)
    }
    /// 取出指定分区上已落盘的全部 AggMap（take 语义，取后分区清空）。

    pub fn restore_partition(&self, partition: usize) -> Result<Vec<AggMap>, String> {
        let function = astersql_executor_aggfuncs::StateSerializer {
            ordinal: 0,
            template: SpillEntry::default(),
        };
        let result = self
            .storage
            .lock()
            .map_err(|_| "spill storage poisoned".to_string())
            .and_then(|mut storage| storage.restore_partition(partition, &[&function]));
        match result {
            Ok((maps, _)) => Ok(maps
                .into_iter()
                .map(|map| {
                    map.into_iter()
                        .map(|(key, mut values)| {
                            let entry = values
                                .remove(0)
                                .downcast::<SpillEntry>()
                                .expect("restored aggregate state type");
                            (key, (entry.group, entry.states))
                        })
                        .collect()
                })
                .collect()),
            Err(error) => {
                self.set_error();
                Err(error)
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.storage.lock().unwrap().is_empty()
    }
    /// 标记 spill 路径发生错误，供其他 worker 快速失败。
    pub fn set_error(&self) {
        self.error.store(true, Ordering::Release);
    }
    /// 查询是否已置错误标志。
    pub fn has_error(&self) -> bool {
        self.error.load(Ordering::Acquire)
    }
}

/// 判断聚合占用是否达到触发阈值（Go 使用触发 tracker 限额的五分之一）。
pub fn has_enough_data_to_spill(aggregate_bytes: usize, trigger_bytes: usize) -> bool {
    aggregate_bytes >= trigger_bytes / 5
}
/*

// Return true if we successfully set flag
// actionImpl 对应 Go 方法：接收者为 `p *ParallelAggSpillDiskAction`，保留控制流、错误返回和资源处理顺序。
impl ParallelAggSpillDiskAction {
    pub fn actionImpl(&mut self, /* Go args: t *memory.Tracker */) /* Go returns: bool */ {
    p.spillHelper.waitForTheEndOfSpill()
    return p.spillHelper.setNeedSpill(p.e.memTracker, t)
}
}

// GetPriority get the priority of the Action
// GetPriority 对应 Go 方法：接收者为 `*ParallelAggSpillDiskAction`，保留控制流、错误返回和资源处理顺序。
impl ParallelAggSpillDiskAction {
    pub fn GetPriority(&mut self/* Go args:  */) /* Go returns: int64 */ {
    return memory.DefSpillPriority
}
}
*/

// Local wiring: preserve every field of the existing worker state on disk.
// Typed aggregate functions use their own StateSerializer codecs above this
// adapter, without conversion to the legacy worker's Value representation.
#[derive(Clone, Default)]
struct SpillEntry {
    group: crate::agg_util::Row,
    states: Vec<crate::agg_util::AggState>,
}
impl astersql_executor_aggfuncs::SpillState for SpillEntry {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        use astersql_util_serialization as s;
        let mut buffer = write_row(&self.group, buffer);
        buffer = s::SerializeInt(self.states.len() as isize, buffer);
        for state in &self.states {
            buffer = s::SerializeUint64(state.count, buffer);
            buffer = s::SerializeBool(state.number.is_some(), buffer);
            if let Some(value) = state.number {
                buffer = s::SerializeFloat64(value, buffer);
            }
            buffer = s::SerializeBool(state.value.is_some(), buffer);
            if let Some(value) = &state.value {
                buffer = write_value(value, buffer);
            }
            buffer = s::SerializeInt(state.distinct_values.len() as isize, buffer);
            for (key, value) in &state.distinct_values {
                buffer = write_bytes(key, buffer);
                buffer = write_value(value, buffer);
            }
        }
        buffer
    }
    fn read_spill(&mut self, input: &mut astersql_util_serialization::PosAndBuf) -> i64 {
        use astersql_util_serialization as s;
        self.group = read_row(input);
        self.states.clear();
        let mut memory = 0;
        for _ in 0..s::DeserializeInt(input) {
            let mut state = crate::agg_util::AggState::new();
            state.count = s::DeserializeUint64(input);
            if s::DeserializeBool(input) {
                state.number = Some(s::DeserializeFloat64(input));
            }
            if s::DeserializeBool(input) {
                state.value = Some(read_value(input));
            }
            for _ in 0..s::DeserializeInt(input) {
                let key = read_bytes(input);
                let value = read_value(input);
                memory += key.len() as i64;
                state.distinct_values.insert(key, value);
            }
            self.states.push(state);
        }
        memory
    }
}
fn write_bytes(value: &[u8], buffer: Vec<u8>) -> Vec<u8> {
    let mut buffer = astersql_util_serialization::SerializeInt(value.len() as isize, buffer);
    buffer.extend(value);
    buffer
}
fn read_bytes(input: &mut astersql_util_serialization::PosAndBuf) -> Vec<u8> {
    let size = astersql_util_serialization::DeserializeInt(input) as usize;
    let start = input.Pos as usize;
    input.Pos += size as i64;
    input.Buf[start..start + size].to_vec()
}
fn write_row(row: &crate::agg_util::Row, buffer: Vec<u8>) -> Vec<u8> {
    let mut buffer = astersql_util_serialization::SerializeInt(row.len() as isize, buffer);
    for value in row {
        buffer = write_value(value, buffer);
    }
    buffer
}
fn read_row(input: &mut astersql_util_serialization::PosAndBuf) -> crate::agg_util::Row {
    (0..astersql_util_serialization::DeserializeInt(input))
        .map(|_| read_value(input))
        .collect()
}
fn write_value(value: &crate::agg_util::Value, mut buffer: Vec<u8>) -> Vec<u8> {
    use crate::agg_util::Value;
    use astersql_util_serialization as s;
    match value {
        Value::Null => {
            buffer.push(0);
            buffer
        }
        Value::Integer(value) => {
            buffer.push(1);
            s::SerializeInt64(*value, buffer)
        }
        Value::Float(value) => {
            buffer.push(2);
            s::SerializeFloat64(*value, buffer)
        }
        Value::Text(value) => {
            buffer.push(3);
            write_bytes(value.as_bytes(), buffer)
        }
        Value::Bytes(value) => {
            buffer.push(4);
            write_bytes(value, buffer)
        }
        Value::Bool(value) => {
            buffer.push(5);
            s::SerializeBool(*value, buffer)
        }
    }
}
fn read_value(input: &mut astersql_util_serialization::PosAndBuf) -> crate::agg_util::Value {
    use crate::agg_util::Value;
    use astersql_util_serialization as s;
    match s::DeserializeUint8(input) {
        0 => Value::Null,
        1 => Value::Integer(s::DeserializeInt64(input)),
        2 => Value::Float(s::DeserializeFloat64(input)),
        3 => Value::Text(String::from_utf8(read_bytes(input)).expect("stored UTF-8 text")),
        4 => Value::Bytes(read_bytes(input)),
        5 => Value::Bool(s::DeserializeBool(input)),
        _ => panic!("invalid aggregate value tag"),
    }
}
