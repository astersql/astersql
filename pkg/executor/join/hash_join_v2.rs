// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Hash Join 执行器 v2（分区化）。
//
// 先将构建侧按哈希分区，再并行建表与 probe；内存不足时可对分区 spill（落盘）
// 并在后续轮次恢复。对应 Go `hash_join_v2.go`。

// HashJoin V2 的分区、溢写恢复、构建/探测 worker 和哈希表上下文流程。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// #![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables)]
// use std::any::Any;
// use std::collections::HashMap;
// 常量声明对应 Go const；保留原始取值和分组顺序。
// const minimalHashTableLen = 32
// 变量声明对应 Go var；全局状态和测试钩子的并发语义后续需用 Rust 原语重建。
// var (
//     _ exec.Executor = &HashJoinV2Exec{}
// EnableHashJoinV2 enable hash join v2, used for test
//     EnableHashJoinV2 = "set tidb_hash_join_version = " + joinversion.HashJoinVersionOptimized
// DisableHashJoinV2 disable hash join v2, used for test
//     DisableHashJoinV2 = "set tidb_hash_join_version = " + joinversion.HashJoinVersionLegacy
// HashJoinV2Strings is used for test
//     HashJoinV2Strings = []string{DisableHashJoinV2, EnableHashJoinV2}
// fakeSel is used when chunk does not have sel field
//     fakeSel []int
// the length of fakeSelLength, default max_chunk_size is 1024,
// we set fakeSel size to 4*max_chunk_size so it should be enough for most cases
//     fakeSelLength = 4096
// )
// init 对应 Go 声明 `func init() {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn init() {
//     fakeSel = make([]int, fakeSelLength)
//     for i = range fakeSel {
//         fakeSel[i] = i
//     }
// }
// hashTableContext 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct hashTableContext {
// rowTables is used during split partition stage, each buildWorker has
// its own rowTable
//     pub rowTables: Vec<Vec<Option<Box<rowTable>>>>,
//     pub hashTable: Option<Box<hashTableV2>>,
//     pub tagHelper: Option<Box<tagPtrHelper>>,
//     pub memoryTracker: Option<Box<memory::Tracker>>,
// }
// reset 对应 Go 声明 `func (htc *hashTableContext) reset() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn reset(&mut self) {
//     self.rowTables = None
//     self.hashTable = None
//     self.tagHelper = None
//     self.memoryTracker.Detach()
// }
// }
// getAllMemoryUsageInHashTable 对应 Go 声明 `func (htc *hashTableContext) getAllMemoryUsageInHashTable() int64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn getAllMemoryUsageInHashTable(&mut self) -> i64 {
//     partNum = len(self.hashTable.tables)
//     totalMemoryUsage = int64(0)
//     for i = range partNum {
//         mem = self.hashTable.getPartitionMemoryUsage(i)
//         totalMemoryUsage += mem
//     }
//     return totalMemoryUsage
// }
// }
// clearHashTable 对应 Go 声明 `func (htc *hashTableContext) clearHashTable() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn clearHashTable(&mut self) {
//     partNum = len(self.hashTable.tables)
//     for i = range partNum {
//         self.hashTable.clearPartitionSegments(i)
//     }
// }
// }
// getPartitionMemoryUsage 对应 Go 声明 `func (htc *hashTableContext) getPartitionMemoryUsage(partID int) int64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn getPartitionMemoryUsage(&mut self, partID: i32) -> i64 {
//     totalMemoryUsage = int64(0)
//     for _, tables = range self.rowTables {
//         if tables != None && tables[partID] != None {
//             totalMemoryUsage += tables[partID].getTotalMemoryUsage()
//         }
//     }
//     return totalMemoryUsage
// }
// }
// getSegmentsInRowTable 对应 Go 声明 `func (htc *hashTableContext) getSegmentsInRowTable(workerID, partitionID int) []*rowTableSegment {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn getSegmentsInRowTable(&mut self, workerID: _, partitionID: i32) -> Vec<Option<Box<rowTableSegment>>> {
//     if self.rowTables[workerID] != None && self.rowTables[workerID][partitionID] != None {
//         return self.rowTables[workerID][partitionID].getSegments()
//     }
//     return None
// }
// }
// getAllSegmentsMemoryUsageInRowTable 对应 Go 声明 `func (htc *hashTableContext) getAllSegmentsMemoryUsageInRowTable() int64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn getAllSegmentsMemoryUsageInRowTable(&mut self) -> i64 {
//     totalMemoryUsage = int64(0)
//     for _, tables = range self.rowTables {
//         for _, table = range tables {
//             if table != None {
//                 totalMemoryUsage += table.getTotalMemoryUsage()
//             }
//         }
//     }
//     return totalMemoryUsage
// }
// }
// clearAllSegmentsInRowTable 对应 Go 声明 `func (htc *hashTableContext) clearAllSegmentsInRowTable() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn clearAllSegmentsInRowTable(&mut self) {
//     for _, tables = range self.rowTables {
//         for _, table = range tables {
//             if table != None {
//                 table.clearSegments()
//             }
//         }
//     }
// }
// }
// clearSegmentsInRowTable 对应 Go 声明 `func (htc *hashTableContext) clearSegmentsInRowTable(workerID, partitionID int) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn clearSegmentsInRowTable(&mut self, workerID: _, partitionID: i32) {
//     if self.rowTables[workerID] != None && self.rowTables[workerID][partitionID] != None {
//         self.rowTables[workerID][partitionID].clearSegments()
//     }
// }
// }
// build 对应 Go 声明 `func (htc *hashTableContext) build(task *buildTask) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn build(&mut self, task: Option<Box<buildTask>>) {
//     self.hashTable.tables[task.partitionIdx].build(task.segStartIdx, task.segEndIdx, self.tagHelper)
// }
// }
// lookup 对应 Go 声明 `func (htc *hashTableContext) lookup(partitionIndex int, hashValue uint64) taggedPtr {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn lookup(&mut self, partitionIndex: i32, hashValue: u64) -> taggedPtr {
//     return self.hashTable.tables[partitionIndex].lookup(hashValue, self.tagHelper)
// }
// }
// appendRowSegment 对应 Go 声明 `func (htc *hashTableContext) appendRowSegment(workerID, partitionID int, seg *rowTableSegment) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn appendRowSegment(&mut self, workerID: _, partitionID: i32, seg: Option<Box<rowTableSegment>>) {
//     if len(seg.hashValues) == 0 {
//         return
//     }
//     if self.rowTables[workerID][partitionID] == None {
//         self.rowTables[workerID][partitionID] = newRowTable()
//     }
//     seg.initTaggedBits()
//     self.rowTables[workerID][partitionID].segments = append(self.rowTables[workerID][partitionID].segments, seg)
// }
// }
// calculateHashTableMemoryUsage 对应 Go 声明 `func (*hashTableContext) calculateHashTableMemoryUsage(rowTables []*rowTable) (int64, []int64) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn calculateHashTableMemoryUsage(&mut self, rowTables: Vec<Option<Box<rowTable>>>) -> (i64, Vec<i64>) {
//     totalMemoryUsage = int64(0)
//     partitionsMemoryUsage = make([]int64, 0)
//     for _, table = range rowTables {
//         hashTableLength = getHashTableLengthByRowTable(table)
//         memoryUsage = getHashTableMemoryUsage(hashTableLength)
//         partitionsMemoryUsage = append(partitionsMemoryUsage, memoryUsage)
//         totalMemoryUsage += memoryUsage
//     }
//     return totalMemoryUsage, partitionsMemoryUsage
// }
// }
// In order to avoid the allocation of hash table, we pre-calculate the memory usage in advance
// to know which hash tables need to be created.
// tryToSpill 对应 Go 声明 `func (htc *hashTableContext) tryToSpill(rowTables []*rowTable, spillHelper *hashJoinSpillHelper) ([]*rowTable, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn tryToSpill(&mut self, rowTables: Vec<Option<Box<rowTable>>>, spillHelper: Option<Box<hashJoinSpillHelper>>) -> (Vec<Option<Box<rowTable>>>, errors::Error) {
//     totalMemoryUsage, hashTableMemoryUsage = self.calculateHashTableMemoryUsage(rowTables)
// Pre-consume the memory usage
//     self.memoryTracker.Consume(totalMemoryUsage)
//     if spillHelper != None && spillHelper.isSpillNeeded() {
//         spillHelper.spillTriggeredBeforeBuildingHashTableForTest = true
//         err = spillHelper.spillRowTable(hashTableMemoryUsage)
//         if err != None {
//             return None, err
//         }
//         spilledPartition = spillHelper.getSpilledPartitions()
//         for _, partID = range spilledPartition {
// Clear spilled row tables
//             rowTables[partID].clearSegments()
//         }
// Though some partitions have been spilled or are empty, their hash tables are still be created
// because probe rows in these partitions may access their hash tables.
// We need to consider these memory usage.
//         totalDefaultMemUsage = getHashTableMemoryUsage(minimalHashTableLen) * int64(len(spilledPartition))
// Hash table memory usage has already been released in spill operation.
// So it's unnecessary to release them again.
//         self.memoryTracker.Consume(totalDefaultMemUsage)
//     }
//     return rowTables, None
// }
// }
// mergeRowTablesToHashTable 对应 Go 声明 `func (htc *hashTableContext) mergeRowTablesToHashTable(partitionNumber uint, spillHelper *hashJoinSpillHelper) (int, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashTableContext {
//     pub fn mergeRowTablesToHashTable(&mut self, partitionNumber: u32, spillHelper: Option<Box<hashJoinSpillHelper>>) -> (i32, errors::Error) {
//     rowTables = make([]*rowTable, partitionNumber)
//     for i = range partitionNumber {
//         rowTables[i] = newRowTable()
//     }
//     totalSegmentCnt = 0
//     for _, rowTablesPerWorker = range self.rowTables {
//         for partIdx, rt = range rowTablesPerWorker {
//             if rt == None {
//                 continue
//             }
//             rowTables[partIdx].merge(rt)
//             totalSegmentCnt += len(rt.segments)
//         }
//     }
//     let mut err: errors::Error = Default::default()
// spillHelper may be None in ut
//     if spillHelper != None {
//         rowTables, err = self.tryToSpill(rowTables, spillHelper)
//         if err != None {
//             return 0, err
//         }
//         spillHelper.setCanSpillFlag(false)
//     }
//     taggedBits = uint8(maxTaggedBits)
//     for i = range partitionNumber {
//         for _, seg = range rowTables[i].segments {
//             taggedBits = min(taggedBits, seg.taggedBits)
//         }
//         self.hashTable.tables[i] = newSubTable(rowTables[i])
//     }
//     self.tagHelper = &tagPtrHelper{}
//     self.tagHelper.init(taggedBits)
//     self.clearAllSegmentsInRowTable()
//     return totalSegmentCnt, None
// }
// }
// HashJoinCtxV2 is the hash join ctx used in hash join v2
// HashJoinCtxV2 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct HashJoinCtxV2 {
//     pub hashJoinCtxBase: hashJoinCtxBase,
//     pub partitionNumber: u32,
//     pub partitionMaskOffset: i32,
//     pub ProbeKeyTypes: Vec<Option<Box<types::FieldType>>>,
//     pub BuildKeyTypes: Vec<Option<Box<types::FieldType>>>,
//     pub stats: Option<Box<hashJoinRuntimeStatsV2>>,
//     pub RightAsBuildSide: bool,
//     pub BuildFilter: expression::CNFExprs,
//     pub ProbeFilter: expression::CNFExprs,
//     pub OtherCondition: expression::CNFExprs,
//     pub hashTableContext: Option<Box<hashTableContext>>,
//     pub hashTableMeta: Option<Box<joinTableMeta>>,
//     pub needScanRowTableAfterProbeDone: bool,
//     pub LUsed: Vec<i32>, pub RUsed: Vec<i32>,
//     pub LUsedInOtherCondition: Vec<i32>, pub RUsedInOtherCondition: Vec<i32>,
//     pub maxSpillRound: i32,
//     pub spillHelper: Option<Box<hashJoinSpillHelper>>,
//     pub spillAction: Option<Box<hashJoinSpillAction>>,
// }
// resetHashTableContextForRestore 对应 Go 声明 `func (hCtx *HashJoinCtxV2) resetHashTableContextForRestore() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinCtxV2 {
//     pub fn resetHashTableContextForRestore(&mut self) {
//     memoryUsage = self.hashTableContext.getAllSegmentsMemoryUsageInRowTable()
//     if intest.InTest && memoryUsage != 0 {
// panic/recover 分支对应 Go 的故障保护，Rust 迁移时需映射为 catch_unwind 或错误返回。
//         panic("All rowTables in hashTableContext should be cleared")
//     }
//     memoryUsage = self.hashTableContext.getAllMemoryUsageInHashTable()
//     self.hashTableContext.clearHashTable()
//     self.hashTableContext.memoryTracker.Consume(-memoryUsage)
// }
// }
// partitionNumber is always power of 2
// genHashJoinPartitionNumber 对应 Go 声明 `func genHashJoinPartitionNumber(partitionHint uint) uint {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn genHashJoinPartitionNumber(partitionHint: u32) -> u32 {
//     partitionNumber = uint(1)
//     for partitionNumber < partitionHint && partitionNumber < 16 {
//         partitionNumber <<= 1
//     }
//     return partitionNumber
// }
// getPartitionMaskOffset 对应 Go 声明 `func getPartitionMaskOffset(partitionNumber uint) int {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn getPartitionMaskOffset(partitionNumber: u32) -> i32 {
//     msbPos = bits.TrailingZeros64(uint64(partitionNumber))
// top MSB bits in hash value will be used to partition data
//     return 64 - msbPos
// }
// SetupPartitionInfo set up partitionNumber and partitionMaskOffset based on concurrency
// SetupPartitionInfo 对应 Go 声明 `func (hCtx *HashJoinCtxV2) SetupPartitionInfo() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinCtxV2 {
//     pub fn SetupPartitionInfo(&mut self) {
//     self.partitionNumber = genHashJoinPartitionNumber(self.Concurrency)
//     self.partitionMaskOffset = getPartitionMaskOffset(self.partitionNumber)
// }
// }
// initHashTableContext create hashTableContext for current HashJoinCtxV2
// initHashTableContext 对应 Go 声明 `func (hCtx *HashJoinCtxV2) initHashTableContext() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinCtxV2 {
//     pub fn initHashTableContext(&mut self) {
//     self.hashTableContext = &hashTableContext{}
//     self.hashTableContext.rowTables = make([][]*rowTable, self.Concurrency)
//     for index = range self.hashTableContext.rowTables {
//         self.hashTableContext.rowTables[index] = make([]*rowTable, self.partitionNumber)
//     }
//     self.hashTableContext.hashTable = &hashTableV2{
//         tables:          make([]*subTable, self.partitionNumber),
//         partitionNumber: uint64(self.partitionNumber),
//     }
//     self.hashTableContext.memoryTracker = memory.NewTracker(memory.LabelForHashTableInHashJoinV2, -1)
// }
// }
// ProbeSideTupleFetcherV2 reads tuples from ProbeSideExec and send them to ProbeWorkers.
// ProbeSideTupleFetcherV2 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct ProbeSideTupleFetcherV2 {
//     pub probeSideTupleFetcherBase: probeSideTupleFetcherBase,
//     pub HashJoinCtxV2: Option<Box<HashJoinCtxV2>>,
//     pub canSkipProbeIfHashTableIsEmpty: bool,
// }
// ProbeWorkerV2 is the probe worker used in hash join v2
// ProbeWorkerV2 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct ProbeWorkerV2 {
//     pub probeWorkerBase: probeWorkerBase,
//     pub HashJoinCtx: Option<Box<HashJoinCtxV2>>,
// We build individual joinProbe for each join worker when use chunk-based
// execution, to avoid the concurrency of joiner.chk and joiner.selected.
//     pub JoinProbe: ProbeV2,
//     pub restoredChkBuf: Option<Box<chunk::Chunk>>,
// }
// updateProbeStatistic 对应 Go 声明 `func (w *ProbeWorkerV2) updateProbeStatistic(start time.Time, probeTime int64) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn updateProbeStatistic(&mut self, start: time::Time, probeTime: i64) {
//     t = time.Since(start)
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//     atomic.AddInt64(&self.HashJoinCtx.stats.probe, probeTime)
//     atomic.AddInt64(&self.HashJoinCtx.stats.workerFetchAndProbe, int64(t))
//     setMaxValue(&self.HashJoinCtx.stats.maxProbeForCurrentRound, probeTime)
//     setMaxValue(&self.HashJoinCtx.stats.maxWorkerFetchAndProbeForCurrentRound, int64(t))
// }
// }
// restoreAndProbe 对应 Go 声明 `func (w *ProbeWorkerV2) restoreAndProbe(inDisk *chunk.DataInDiskByChunks, start time.Time) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn restoreAndProbe(&mut self, inDisk: Option<Box<chunk::DataInDiskByChunks>>, start: time::Time) {
//     probeTime = int64(0)
//     if self.HashJoinCtx.stats != None {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer func() {
//             self.updateProbeStatistic(start, probeTime)
//         }()
//     }
//     ok, joinResult = self.getNewJoinResult()
//     if !ok {
//         return
//     }
//     chunkNum = inDisk.NumChunks()
//     for i = range chunkNum {
// Go select 同时监听 channel/context；保留分支结构，后续需替换为异步 select。
//         select {
//         case <-self.HashJoinCtx.closeCh:
//             return
//         default:
//         }
//         failpoint.Inject("ConsumeRandomPanic", None)
//         err = inDisk.FillChunk(i, self.restoredChkBuf)
//         if err != None {
//             joinResult.err = err
//             break
//         }
//         err = triggerIntest(2)
//         if err != None {
//             joinResult.err = err
//             break
//         }
//         start = time.Now()
//         waitTime = int64(0)
//         ok, waitTime, joinResult = self.processOneRestoredProbeChunk(joinResult)
//         probeTime += int64(time.Since(start)) - waitTime
//         if !ok {
//             break
//         }
//     }
//     err = self.JoinProbe.SpillRemainingProbeChunks()
//     if err != None {
//         joinResult.err = err
//     }
//     if joinResult.err != None || (joinResult.chk != None && joinResult.chk.NumRows() > 0) {
//         self.HashJoinCtx.joinResultCh <- joinResult
//     } else if joinResult.chk != None && joinResult.chk.NumRows() == 0 {
//         self.joinChkResourceCh <- joinResult.chk
//     }
// }
// }
// BuildWorkerV2 is the build worker used in hash join v2
// BuildWorkerV2 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct BuildWorkerV2 {
//     pub buildWorkerBase: buildWorkerBase,
//     pub HashJoinCtx: Option<Box<HashJoinCtxV2>>,
//     pub BuildTypes: Vec<Option<Box<types::FieldType>>>,
//     pub HasNullableKey: bool,
//     pub WorkerID: u32,
//     pub builder: Option<Box<rowTableBuilder>>,
//     pub restoredChkBuf: Option<Box<chunk::Chunk>>,
// }
// getSegmentsInRowTable 对应 Go 声明 `func (b *BuildWorkerV2) getSegmentsInRowTable(partID int) []*rowTableSegment {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn getSegmentsInRowTable(&mut self, partID: i32) -> Vec<Option<Box<rowTableSegment>>> {
//     return self.HashJoinCtx.hashTableContext.getSegmentsInRowTable(int(self.WorkerID), partID)
// }
// }
// clearSegmentsInRowTable 对应 Go 声明 `func (b *BuildWorkerV2) clearSegmentsInRowTable(partID int) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn clearSegmentsInRowTable(&mut self, partID: i32) {
//     self.HashJoinCtx.hashTableContext.clearSegmentsInRowTable(int(self.WorkerID), partID)
// }
// }
// updatePartitionData 对应 Go 声明 `func (b *BuildWorkerV2) updatePartitionData(cost int64) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn updatePartitionData(&mut self, cost: i64) {
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//     atomic.AddInt64(&self.HashJoinCtx.stats.partitionData, cost)
//     setMaxValue(&self.HashJoinCtx.stats.maxPartitionDataForCurrentRound, cost)
// }
// }
// processOneRestoredChunk 对应 Go 声明 `func (b *BuildWorkerV2) processOneRestoredChunk(cost *int64) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn processOneRestoredChunk(&mut self, cost: Option<Box<i64>>) -> Result<(), errors::Error> {
//     start = time.Now()
//     err = self.builder.processOneRestoredChunk(self.restoredChkBuf, self.HashJoinCtx, int(self.WorkerID), int(self.HashJoinCtx.partitionNumber))
//     if err != None {
//         return err
//     }
//     *cost += int64(time.Since(start))
//     return None
// }
// }
// splitPartitionAndAppendToRowTableForRestoreImpl 对应 Go 声明 `func (b *BuildWorkerV2) splitPartitionAndAppendToRowTableForRestoreImpl(i int, inDisk *chunk.DataInDiskByChunks, fetcherAndWorkerSyncer *sync.WaitGroup, hasErr bool, cost *int64) (err error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn splitPartitionAndAppendToRowTableForRestoreImpl(&mut self, i: i32, inDisk: Option<Box<chunk::DataInDiskByChunks>>, fetcherAndWorkerSyncer: Option<Box<sync::WaitGroup>>, hasErr: bool, cost: Option<Box<i64>>) -> (errors::Error) {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() {
//         fetcherAndWorkerSyncer.Done()
// panic/recover 分支对应 Go 的故障保护，Rust 迁移时需映射为 catch_unwind 或错误返回。
//         if r = recover(); r != None {
// We shouldn't throw the panic out of this function, or
// we can't continue to consume `syncCh` channel and call
// the `Done` function of `fetcherAndWorkerSyncer`.
// So it's necessary to handle it here.
//             err = util.GetRecoverError(r)
//         }
//     }()
//     if hasErr {
//         return None
//     }
//     err = inDisk.FillChunk(i, self.restoredChkBuf)
//     if err != None {
//         return err
//     }
//     err = triggerIntest(3)
//     if err != None {
//         return err
//     }
//     err = self.processOneRestoredChunk(cost)
//     if err != None {
//         return err
//     }
//     return None
// }
// }
// splitPartitionAndAppendToRowTableForRestore 对应 Go 声明 `func (b *BuildWorkerV2) splitPartitionAndAppendToRowTableForRestore(inDisk *chunk.DataInDiskByChunks, syncCh chan *chunk.Chunk, fetcherAndWorkerSyncer *sync.WaitGroup, errCh chan error, doneCh chan struct{}) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn splitPartitionAndAppendToRowTableForRestore(&mut self, inDisk: Option<Box<chunk::DataInDiskByChunks>>, syncCh chan: Option<Box<chunk::Chunk>>, fetcherAndWorkerSyncer: Option<Box<sync::WaitGroup>>, errCh chan: errors::Error, doneCh chan: ()) {
//     cost = int64(0)
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() {
//         if self.HashJoinCtx.stats != None {
//             self.updatePartitionData(cost)
//         }
//     }()
// When error happens, hasErr will be set to true.
// However, we should not directly exit the function, as we must
// call `fetcherAndWorkerSyncer.Done()` in `splitPartitionAndAppendToRowTableForRestoreImpl`
// fetcherAndWorkerSyncer is a counter for synchronizing, it should be `Done` for `chunkNum`.
// When `hasErr` is set, `splitPartitionAndAppendToRowTableForRestoreImpl` could exit early.
//     hasErr = false
//     chunkNum = inDisk.NumChunks()
//     for i = range chunkNum {
//         _, ok = <-syncCh
//         if !ok {
//             break
//         }
//         err = self.splitPartitionAndAppendToRowTableForRestoreImpl(i, inDisk, fetcherAndWorkerSyncer, hasErr, &cost)
//         if err != None {
//             hasErr = true
//             handleErr(err, errCh, doneCh)
//         }
//     }
// }
// }
// splitPartitionAndAppendToRowTable 对应 Go 声明 `func (b *BuildWorkerV2) splitPartitionAndAppendToRowTable(typeCtx types.Context, fetcherAndWorkerSyncer *sync.WaitGroup, srcChkCh chan *chunk.Chunk, errCh chan error, doneCh chan struct{}) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn splitPartitionAndAppendToRowTable(&mut self, typeCtx: types::Context, fetcherAndWorkerSyncer: Option<Box<sync::WaitGroup>>, srcChkCh chan: Option<Box<chunk::Chunk>>, errCh chan: errors::Error, doneCh chan: ()) {
//     cost = int64(0)
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() {
//         if self.HashJoinCtx.stats != None {
//             self.updatePartitionData(cost)
//         }
//     }()
// When error happens, hasErr will be set to true.
// However, we should not directly exit the function, as we must
// call `fetcherAndWorkerSyncer.Done()` in `splitPartitionAndAppendToRowTableImpl`
// fetcherAndWorkerSyncer is a counter for synchronizing, it should be `Done` for `chunkNum`.
// When `hasErr` is set, `splitPartitionAndAppendToRowTableImpl` could exit early.
//     hasErr = false
//     for chk = range srcChkCh {
//         err = self.splitPartitionAndAppendToRowTableImpl(typeCtx, chk, fetcherAndWorkerSyncer, hasErr, &cost)
//         if err != None {
//             hasErr = true
//             handleErr(err, errCh, doneCh)
//         }
//     }
// }
// }
// processOneChunk 对应 Go 声明 `func (b *BuildWorkerV2) processOneChunk(typeCtx types.Context, chk *chunk.Chunk, cost *int64) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn processOneChunk(&mut self, typeCtx: types::Context, chk: Option<Box<chunk::Chunk>>, cost: Option<Box<i64>>) -> Result<(), errors::Error> {
//     start = time.Now()
//     err = self.builder.processOneChunk(chk, typeCtx, self.HashJoinCtx, int(self.WorkerID))
//     failpoint.Inject("splitPartitionPanic", None)
//     *cost += int64(time.Since(start))
//     return err
// }
// }
// splitPartitionAndAppendToRowTableImpl 对应 Go 声明 `func (b *BuildWorkerV2) splitPartitionAndAppendToRowTableImpl(typeCtx types.Context, chk *chunk.Chunk, fetcherAndWorkerSyncer *sync.WaitGroup, hasErr bool, cost *int64) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn splitPartitionAndAppendToRowTableImpl(&mut self, typeCtx: types::Context, chk: Option<Box<chunk::Chunk>>, fetcherAndWorkerSyncer: Option<Box<sync::WaitGroup>>, hasErr: bool, cost: Option<Box<i64>>) -> Result<(), errors::Error> {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() {
//         fetcherAndWorkerSyncer.Done()
//     }()
//     if hasErr {
//         return None
//     }
//     err = triggerIntest(5)
//     if err != None {
//         return err
//     }
//     err = self.processOneChunk(typeCtx, chk, cost)
//     if err != None {
//         return err
//     }
//     return None
// }
// }
// buildHashTableForList builds hash table from `list`.
// buildHashTable 对应 Go 声明 `func (b *BuildWorkerV2) buildHashTable(taskCh chan *buildTask) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV2 {
//     pub fn buildHashTable(&mut self, taskCh chan: Option<Box<buildTask>>) -> Result<(), errors::Error> {
//     cost = int64(0)
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() {
//         if self.HashJoinCtx.stats != None {
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//             atomic.AddInt64(&self.HashJoinCtx.stats.buildHashTable, cost)
//             setMaxValue(&self.HashJoinCtx.stats.maxBuildHashTableForCurrentRound, cost)
//         }
//     }()
//     for task = range taskCh {
//         start = time.Now()
//         self.HashJoinCtx.hashTableContext.build(task)
//         failpoint.Inject("buildHashTablePanic", None)
//         cost += int64(time.Since(start))
//         err = triggerIntest(5)
//         if err != None {
//             return err
//         }
//     }
//     return None
// }
// }
// NewJoinBuildWorkerV2 create a BuildWorkerV2
// NewJoinBuildWorkerV2 对应 Go 声明 `func NewJoinBuildWorkerV2(ctx *HashJoinCtxV2, workID uint, buildSideExec exec.Executor, buildKeyColIdx []int, buildTypes []*types.FieldType) *BuildWorkerV2 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn NewJoinBuildWorkerV2(ctx: Option<Box<HashJoinCtxV2>>, workID: u32, buildSideExec: exec::Executor, buildKeyColIdx: Vec<i32>, buildTypes: Vec<Option<Box<types::FieldType>>>) -> Option<Box<BuildWorkerV2>> {
//     hasNullableKey = false
//     for _, idx = range buildKeyColIdx {
//         if !mysql.HasNotNullFlag(buildTypes[idx].GetFlag()) {
//             hasNullableKey = true
//             break
//         }
//     }
//     worker = &BuildWorkerV2{
//         HashJoinCtx:    ctx,
//         BuildTypes:     buildTypes,
//         WorkerID:       workID,
//         HasNullableKey: hasNullableKey,
//     }
//     worker.BuildSideExec = buildSideExec
//     worker.BuildKeyColIdx = buildKeyColIdx
//     return worker
// }
// HashJoinV2Exec implements the hash join algorithm.
// HashJoinV2Exec 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct HashJoinV2Exec {
//     pub BaseExecutor: exec::BaseExecutor,
//     pub HashJoinCtxV2: Option<Box<HashJoinCtxV2>>,
//     pub ProbeSideTupleFetcher: Option<Box<ProbeSideTupleFetcherV2>>,
//     pub ProbeWorkers: Vec<Option<Box<ProbeWorkerV2>>>,
//     pub BuildWorkers: Vec<Option<Box<BuildWorkerV2>>>,
//     pub workerWg: util::WaitGroupWrapper,
//     pub waiterWg: util::WaitGroupWrapper,
//     pub restoredBuildInDisk: Vec<Option<Box<chunk::DataInDiskByChunks>>>,
//     pub restoredProbeInDisk: Vec<Option<Box<chunk::DataInDiskByChunks>>>,
//     pub prepared: bool,
//     pub inRestore: bool,
//     pub IsGA: bool,
//     pub isMemoryClearedForTest: bool,
//     pub FileNamePrefixForTest: String,
// }
// isAllMemoryClearedForTest 对应 Go 声明 `func (e *HashJoinV2Exec) isAllMemoryClearedForTest() bool {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn isAllMemoryClearedForTest(&mut self) -> bool {
//     return self.isMemoryClearedForTest
// }
// }
// initMaxSpillRound 对应 Go 声明 `func (e *HashJoinV2Exec) initMaxSpillRound() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn initMaxSpillRound(&mut self) {
//     if self.partitionNumber > 1024 {
//         self.maxSpillRound = 1
//         return
//     }
// Calculate the minimum number of rounds required for the total partitions to exceed 1024
//     self.maxSpillRound = int(math.Log(1024) / math.Log(float64(self.partitionNumber)))
// }
// }
// Close implements the Executor Close interface.
// Close 对应 Go 声明 `func (e *HashJoinV2Exec) Close() error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn Close(&mut self) -> Result<(), errors::Error> {
//     if self.closeCh != None {
//         close(self.closeCh)
//     }
//     self.finished.Store(true)
//     if self.prepared {
//         if self.buildFinished != None {
// channel 工具调用保留 Go 的队列清理/发送语义。
//             channel.Clear(self.buildFinished)
//         }
//         if self.joinResultCh != None {
//             channel.Clear(self.joinResultCh)
//         }
//         if self.ProbeSideTupleFetcher.probeChkResourceCh != None {
//             close(self.ProbeSideTupleFetcher.probeChkResourceCh)
// channel 工具调用保留 Go 的队列清理/发送语义。
//             channel.Clear(self.ProbeSideTupleFetcher.probeChkResourceCh)
//         }
//         for i = range self.ProbeSideTupleFetcher.probeResultChs {
//             channel.Clear(self.ProbeSideTupleFetcher.probeResultChs[i])
//         }
//         for i = range self.ProbeWorkers {
//             close(self.ProbeWorkers[i].joinChkResourceCh)
// channel 工具调用保留 Go 的队列清理/发送语义。
//             channel.Clear(self.ProbeWorkers[i].joinChkResourceCh)
//         }
//         self.ProbeSideTupleFetcher.probeChkResourceCh = None
//         self.waiterWg.Wait()
//         self.hashTableContext.reset()
//     }
//     for _, w = range self.ProbeWorkers {
//         w.joinChkResourceCh = None
//     }
//     if self.stats != None {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer self.Ctx().GetSessionVars().StmtCtx.RuntimeStatsColl.RegisterStats(self.ID(), self.stats)
//     }
//     self.releaseDisk()
//     if self.spillHelper != None {
//         self.spillHelper.close()
//     }
//     err = self.BaseExecutor.Close()
//     return err
// }
// }
// Open implements the Executor Open interface.
// Open 对应 Go 声明 `func (e *HashJoinV2Exec) Open(ctx context.Context) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn Open(&mut self, ctx: context::Context) -> Result<(), errors::Error> {
//     if err = self.BaseExecutor.Open(ctx); err != None {
//         self.closeCh = None
//         self.prepared = false
//         return err
//     }
//     return self.OpenSelf()
// }
// }
// OpenSelf opens hash join itself and initializes the hash join context.
// OpenSelf 对应 Go 声明 `func (e *HashJoinV2Exec) OpenSelf() error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn OpenSelf(&mut self) -> Result<(), errors::Error> {
//     self.prepared = false
//     self.inRestore = false
//     needScanRowTableAfterProbeDone = self.ProbeWorkers[0].JoinProbe.NeedScanRowTable()
//     self.HashJoinCtxV2.needScanRowTableAfterProbeDone = needScanRowTableAfterProbeDone
//     if self.RightAsBuildSide {
//         self.hashTableMeta = newTableMeta(self.BuildWorkers[0].BuildKeyColIdx, self.BuildWorkers[0].BuildTypes,
//             self.BuildKeyTypes, self.ProbeKeyTypes, self.RUsedInOtherCondition, self.RUsed, needScanRowTableAfterProbeDone)
//     } else {
//         self.hashTableMeta = newTableMeta(self.BuildWorkers[0].BuildKeyColIdx, self.BuildWorkers[0].BuildTypes,
//             self.BuildKeyTypes, self.ProbeKeyTypes, self.LUsedInOtherCondition, self.LUsed, needScanRowTableAfterProbeDone)
//     }
//     self.HashJoinCtxV2.ChunkAllocPool = self.AllocPool
//     if self.memTracker != None {
//         self.memTracker.Reset()
//     } else {
//         self.memTracker = memory.NewTracker(self.ID(), -1)
//     }
//     self.memTracker.AttachTo(self.Ctx().GetSessionVars().StmtCtx.MemTracker)
//     if self.diskTracker != None {
//         self.diskTracker.Reset()
//     } else {
//         self.diskTracker = disk.NewTracker(self.ID(), -1)
//     }
//     self.diskTracker.AttachTo(self.Ctx().GetSessionVars().StmtCtx.DiskTracker)
//     self.spillHelper = newHashJoinSpillHelper(e, int(self.partitionNumber), self.ProbeSideTupleFetcher.ProbeSideExec.RetFieldTypes(), self.FileNamePrefixForTest)
//     self.maxSpillRound = 1
//     if vardef.EnableTmpStorageOnOOM.Load() && self.partitionNumber > 1 {
//         self.initMaxSpillRound()
//         self.spillAction = newHashJoinSpillAction(self.spillHelper)
//         self.Ctx().GetSessionVars().MemTracker.FallbackOldAndSetNewAction(self.spillAction)
//     }
//     self.workerWg = util.WaitGroupWrapper{}
//     self.waiterWg = util.WaitGroupWrapper{}
//     self.closeCh = make(chan struct{})
//     self.finished.Store(false)
//     if self.RuntimeStats() != None && self.stats == None {
//         self.stats = &hashJoinRuntimeStatsV2{}
//         self.stats.concurrent = int(self.Concurrency)
//     }
//     if self.stats != None {
//         self.stats.reset()
//         self.stats.spill.partitionNum = int(self.partitionNumber)
//         self.stats.isHashJoinGA = self.IsGA
//     }
//     return None
// }
// }
// shouldLimitProbeFetchSize 对应 Go 声明 `func (fetcher *ProbeSideTupleFetcherV2) shouldLimitProbeFetchSize() bool {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeSideTupleFetcherV2 {
//     pub fn shouldLimitProbeFetchSize(&mut self) -> bool {
//     if self.JoinType == base.LeftOuterJoin && self.RightAsBuildSide {
//         return true
//     }
//     if self.JoinType == base.RightOuterJoin && !self.RightAsBuildSide {
//         return true
//     }
//     return false
// }
// }
// canSkipProbeIfHashTableIsEmpty 对应 Go 声明 `func (e *HashJoinV2Exec) canSkipProbeIfHashTableIsEmpty() bool {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn canSkipProbeIfHashTableIsEmpty(&mut self) -> bool {
//     switch self.JoinType {
//     case base.InnerJoin:
//         return true
//     case base.LeftOuterJoin:
//         return !self.RightAsBuildSide
//     case base.RightOuterJoin:
//         return self.RightAsBuildSide
//     case base.SemiJoin:
//         return self.RightAsBuildSide
//     default:
//         return false
//     }
// }
// }
// initializeForProbe 对应 Go 声明 `func (e *HashJoinV2Exec) initializeForProbe() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn initializeForProbe(&mut self) {
//     self.ProbeSideTupleFetcher.HashJoinCtxV2 = self.HashJoinCtxV2
// self.joinResultCh is for transmitting the join result chunks to the main thread.
//     self.joinResultCh = make(chan *hashjoinWorkerResult, self.Concurrency+1)
//     self.ProbeSideTupleFetcher.initializeForProbeBase(self.Concurrency, self.joinResultCh)
//     self.ProbeSideTupleFetcher.canSkipProbeIfHashTableIsEmpty = self.canSkipProbeIfHashTableIsEmpty()
// set buildSuccess to false by default, it will be set to true if build finishes successfully
//     self.ProbeSideTupleFetcher.buildSuccess = false
//     for i = range self.Concurrency {
//         self.ProbeWorkers[i].initializeForProbe(self.ProbeSideTupleFetcher.probeChkResourceCh, self.ProbeSideTupleFetcher.probeResultChs[i], e)
//         self.ProbeWorkers[i].JoinProbe.ResetProbeCollision()
//     }
// }
// }
// startProbeFetcher 对应 Go 声明 `func (e *HashJoinV2Exec) startProbeFetcher(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn startProbeFetcher(&mut self, ctx: context::Context) {
//     if !self.inRestore {
//         fetchProbeSideChunksFunc = func() {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//             defer trace.StartRegion(ctx, "HashJoinProbeSideFetcher").End()
//             self.ProbeSideTupleFetcher.fetchProbeSideChunks(
//                 ctx,
//                 self.MaxChunkSize(),
//                 func() bool { return self.ProbeSideTupleFetcher.hashTableContext.hashTable.isHashTableEmpty() },
//                 func() bool { return self.spillHelper.isSpillTriggered() },
//                 self.ProbeSideTupleFetcher.canSkipProbeIfHashTableIsEmpty,
//                 self.ProbeSideTupleFetcher.needScanRowTableAfterProbeDone,
//                 self.ProbeSideTupleFetcher.shouldLimitProbeFetchSize(),
//                 &self.ProbeSideTupleFetcher.hashJoinCtxBase)
//         }
//         self.workerWg.RunWithRecover(fetchProbeSideChunksFunc, self.ProbeSideTupleFetcher.handleProbeSideFetcherPanic)
//     }
// }
// }
// startProbeJoinWorkers 对应 Go 声明 `func (e *HashJoinV2Exec) startProbeJoinWorkers(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn startProbeJoinWorkers(&mut self, ctx: context::Context) {
//     let mut start: time::Time = Default::default()
//     if self.HashJoinCtxV2.stats != None {
//         start = time.Now()
//     }
//     if self.inRestore {
// Wait for the restore build
//         err = <-self.buildFinished
//         if err != None {
//             return
//         }
// in restore, there is no standalone probe fetcher goroutine, so set buildSuccess here
//         self.ProbeSideTupleFetcher.buildSuccess = true
//     }
//     for i = range self.Concurrency {
//         workerID = i
//         self.workerWg.RunWithRecover(func() {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//             defer trace.StartRegion(ctx, "HashJoinWorker").End()
//             if self.inRestore {
//                 self.ProbeWorkers[workerID].restoreAndProbe(self.restoredProbeInDisk[workerID], start)
//             } else {
//                 self.ProbeWorkers[workerID].runJoinWorker(start)
//             }
//         }, self.ProbeWorkers[workerID].handleProbeWorkerPanic)
//     }
// }
// }
// fetchAndProbeHashTable 对应 Go 声明 `func (e *HashJoinV2Exec) fetchAndProbeHashTable(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn fetchAndProbeHashTable(&mut self, ctx: context::Context) {
//     start = time.Now()
//     self.startProbeFetcher(ctx)
// Join workers directly read data from disk when we are in restore status
// and read data from fetcher otherwise.
//     self.startProbeJoinWorkers(ctx)
//     self.waiterWg.RunWithRecover(
//         func() {
//             self.waitJoinWorkers(start)
//         }, None)
// }
// }
// handleProbeWorkerPanic 对应 Go 声明 `func (w *ProbeWorkerV2) handleProbeWorkerPanic(r any) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn handleProbeWorkerPanic(&mut self, r: Box<dyn Any>) {
//     if r != None {
//         self.HashJoinCtx.joinResultCh <- &hashjoinWorkerResult{err: util.GetRecoverError(r)}
//     }
// }
// }
// handleJoinWorkerPanic 对应 Go 声明 `func (e *HashJoinV2Exec) handleJoinWorkerPanic(r any) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn handleJoinWorkerPanic(&mut self, r: Box<dyn Any>) {
//     if r != None {
//         self.joinResultCh <- &hashjoinWorkerResult{err: util.GetRecoverError(r)}
//     }
// }
// }
// waitJoinWorkers 对应 Go 声明 `func (e *HashJoinV2Exec) waitJoinWorkers(start time.Time) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn waitJoinWorkers(&mut self, start: time::Time) {
//     self.workerWg.Wait()
//     if self.stats != None {
//         self.HashJoinCtxV2.stats.fetchAndProbe += int64(time.Since(start))
//         for _, prober = range self.ProbeWorkers {
//             self.stats.probeCollision += int64(prober.JoinProbe.GetProbeCollision())
//         }
//     }
//     if self.ProbeSideTupleFetcher.buildSuccess {
// only scan row table if build is successful
//         if self.ProbeWorkers[0] != None && self.ProbeWorkers[0].JoinProbe.NeedScanRowTable() {
//             for i = range self.Concurrency {
//                 let mut workerID: = i = Default::default()
//                 self.workerWg.RunWithRecover(func() {
//                     self.ProbeWorkers[workerID].scanRowTableAfterProbeDone()
//                 }, self.handleJoinWorkerPanic)
//             }
//             self.workerWg.Wait()
//         }
//     }
// }
// }
// scanRowTableAfterProbeDone 对应 Go 声明 `func (w *ProbeWorkerV2) scanRowTableAfterProbeDone() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn scanRowTableAfterProbeDone(&mut self) {
//     self.JoinProbe.InitForScanRowTable()
//     ok, joinResult = self.getNewJoinResult()
//     if !ok {
//         return
//     }
//     for !self.JoinProbe.IsScanRowTableDone() {
//         joinResult = self.JoinProbe.ScanRowTable(joinResult, &self.HashJoinCtx.SessCtx.GetSessionVars().SQLKiller)
//         if joinResult.err != None {
//             self.HashJoinCtx.joinResultCh <- joinResult
//             return
//         }
//         err = triggerIntest(4)
//         if err != None {
//             self.HashJoinCtx.joinResultCh <- &hashjoinWorkerResult{err: err}
//             return
//         }
//         if joinResult.chk.IsFull() {
//             self.HashJoinCtx.joinResultCh <- joinResult
//             ok, joinResult = self.getNewJoinResult()
//             if !ok {
//                 return
//             }
//         }
//     }
//     if joinResult.err != None || (joinResult.chk != None && joinResult.chk.NumRows() > 0) {
//         self.HashJoinCtx.joinResultCh <- joinResult
//     } else if joinResult.chk != None && joinResult.chk.NumRows() == 0 {
//         self.joinChkResourceCh <- joinResult.chk
//     }
// }
// }
// processOneRestoredProbeChunk 对应 Go 声明 `func (w *ProbeWorkerV2) processOneRestoredProbeChunk(joinResult *hashjoinWorkerResult) (ok bool, waitTime int64, _ *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn processOneRestoredProbeChunk(&mut self, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     joinResult.err = self.JoinProbe.SetRestoredChunkForProbe(self.restoredChkBuf)
//     if joinResult.err != None {
//         return false, 0, joinResult
//     }
//     return self.probeAndSendResult(joinResult)
// }
// }
// processOneProbeChunk 对应 Go 声明 `func (w *ProbeWorkerV2) processOneProbeChunk(probeChunk *chunk.Chunk, joinResult *hashjoinWorkerResult) (ok bool, waitTime int64, _ *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn processOneProbeChunk(&mut self, probeChunk: Option<Box<chunk::Chunk>>, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     joinResult.err = self.JoinProbe.SetChunkForProbe(probeChunk)
//     if joinResult.err != None {
//         return false, 0, joinResult
//     }
//     return self.probeAndSendResult(joinResult)
// }
// }
// probeAndSendResult 对应 Go 声明 `func (w *ProbeWorkerV2) probeAndSendResult(joinResult *hashjoinWorkerResult) (bool, int64, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn probeAndSendResult(&mut self, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     if self.HashJoinCtx.spillHelper.areAllPartitionsSpilled() {
//         if intest.InTest && self.HashJoinCtx.spillHelper.hashJoinExec.inRestore {
//             self.HashJoinCtx.spillHelper.skipProbeInRestoreForTest.Store(true)
//         }
//         return true, 0, joinResult
//     }
//     let mut ok: bool = Default::default()
//     waitTime = int64(0)
//     for !self.JoinProbe.IsCurrentChunkProbeDone() {
//         ok, joinResult = self.JoinProbe.Probe(joinResult, &self.HashJoinCtx.SessCtx.GetSessionVars().SQLKiller)
//         if !ok || joinResult.err != None {
//             return ok, waitTime, joinResult
//         }
//         failpoint.Inject("processOneProbeChunkPanic", None)
//         if joinResult.chk.IsFull() {
//             waitStart = time.Now()
//             self.HashJoinCtx.joinResultCh <- joinResult
//             ok, joinResult = self.getNewJoinResult()
//             waitTime += int64(time.Since(waitStart))
//             if !ok {
//                 return false, waitTime, joinResult
//             }
//         }
//     }
//     return true, waitTime, joinResult
// }
// }
// runJoinWorker 对应 Go 声明 `func (w *ProbeWorkerV2) runJoinWorker(start time.Time) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn runJoinWorker(&mut self, start: time::Time) {
//     probeTime = int64(0)
//     if self.HashJoinCtx.stats != None {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer func() {
//             self.updateProbeStatistic(start, probeTime)
//         }()
//     }
//     var (
//         probeSideResult *chunk.Chunk
//     )
//     ok, joinResult = self.getNewJoinResult()
//     if !ok {
//         return
//     }
// Read and filter probeSideResult, and join the probeSideResult with the build side rows.
//     emptyProbeSideResult = &probeChkResource{
//         dest: self.probeResultCh,
//     }
//     for ok = true; ok; {
// Go select 同时监听 channel/context；保留分支结构，后续需替换为异步 select。
//         select {
//         case <-self.HashJoinCtx.closeCh:
//             return
//         case probeSideResult, ok = <-self.probeResultCh:
//         }
//         failpoint.Inject("ConsumeRandomPanic", None)
//         if !ok {
//             break
//         }
//         err = triggerIntest(2)
//         if err != None {
//             joinResult.err = err
//             break
//         }
//         start = time.Now()
//         waitTime = int64(0)
//         ok, waitTime, joinResult = self.processOneProbeChunk(probeSideResult, joinResult)
//         probeTime += int64(time.Since(start)) - waitTime
//         if !ok {
//             break
//         }
//         probeSideResult.Reset()
//         emptyProbeSideResult.chk = probeSideResult
// Give back to probe fetcher
//         self.probeChkResourceCh <- emptyProbeSideResult
//     }
//     err = self.JoinProbe.SpillRemainingProbeChunks()
//     if err != None {
//         joinResult.err = err
//     }
//     if joinResult.err != None || (joinResult.chk != None && joinResult.chk.NumRows() > 0) {
//         self.HashJoinCtx.joinResultCh <- joinResult
//     } else if joinResult.chk != None && joinResult.chk.NumRows() == 0 {
//         self.joinChkResourceCh <- joinResult.chk
//     }
// }
// }
// getNewJoinResult 对应 Go 声明 `func (w *ProbeWorkerV2) getNewJoinResult() (bool, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV2 {
//     pub fn getNewJoinResult(&mut self) -> (bool, Option<Box<hashjoinWorkerResult>>) {
//     joinResult = &hashjoinWorkerResult{
//         src: self.joinChkResourceCh,
//     }
//     ok = true
// Go select 同时监听 channel/context；保留分支结构，后续需替换为异步 select。
//     select {
//     case <-self.HashJoinCtx.closeCh:
//         ok = false
//     case joinResult.chk, ok = <-self.joinChkResourceCh:
//     }
//     return ok, joinResult
// }
// }
// reset 对应 Go 声明 `func (e *HashJoinV2Exec) reset() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn reset(&mut self) {
//     self.resetProbeStatus()
//     self.releaseDisk()
// set buildSuccess to false by default, it will be set to true if build finishes successfully
//     self.ProbeSideTupleFetcher.buildSuccess = false
//     self.resetHashTableContextForRestore()
//     self.spillHelper.setCanSpillFlag(true)
//     if self.HashJoinCtxV2.stats != None {
//         self.HashJoinCtxV2.stats.resetCurrentRound()
//     }
// }
// }
// collectSpillStats 对应 Go 声明 `func (e *HashJoinV2Exec) collectSpillStats() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn collectSpillStats(&mut self) {
//     if self.stats == None || !self.spillHelper.isSpillTriggered() {
//         return
//     }
//     round = self.spillHelper.round
//     if len(self.stats.spill.totalSpillBytesPerRound) < round+1 {
//         self.stats.spill.totalSpillBytesPerRound = append(self.stats.spill.totalSpillBytesPerRound, 0)
//         self.stats.spill.spillBuildRowTableBytesPerRound = append(self.stats.spill.spillBuildRowTableBytesPerRound, 0)
//         self.stats.spill.spillBuildHashTableBytesPerRound = append(self.stats.spill.spillBuildHashTableBytesPerRound, 0)
//         self.stats.spill.spilledPartitionNumPerRound = append(self.stats.spill.spilledPartitionNumPerRound, 0)
//     }
//     buildRowTableSpillBytes = self.spillHelper.getBuildSpillBytes()
//     buildHashTableSpillBytes = getHashTableMemoryUsage(getHashTableLengthByRowLen(self.spillHelper.spilledValidRowNum.Load()))
//     probeSpillBytes = self.spillHelper.getProbeSpillBytes()
//     spilledPartitionNum = self.spillHelper.getSpilledPartitionsNum()
//     self.stats.spill.spillBuildRowTableBytesPerRound[round] += buildRowTableSpillBytes
//     self.stats.spill.spillBuildHashTableBytesPerRound[round] += buildHashTableSpillBytes
//     self.stats.spill.totalSpillBytesPerRound[round] += buildRowTableSpillBytes + probeSpillBytes
//     self.stats.spill.spilledPartitionNumPerRound[round] += spilledPartitionNum
// }
// }
// startBuildAndProbe 对应 Go 声明 `func (e *HashJoinV2Exec) startBuildAndProbe(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn startBuildAndProbe(&mut self, ctx: context::Context) {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() {
// panic/recover 分支对应 Go 的故障保护，Rust 迁移时需映射为 catch_unwind 或错误返回。
//         if r = recover(); r != None {
//             self.joinResultCh <- &hashjoinWorkerResult{err: util.GetRecoverError(r)}
//         }
//         close(self.joinResultCh)
//     }()
//     lastRound = 0
//     for {
//         if self.finished.Load() {
//             return
//         }
//         self.buildFinished = make(chan error, 1)
//         self.fetchAndBuildHashTable(ctx)
//         self.fetchAndProbeHashTable(ctx)
//         self.waiterWg.Wait()
//         self.collectSpillStats()
//         self.reset()
//         self.spillHelper.spillRoundForTest = max(self.spillHelper.spillRoundForTest, lastRound)
//         err = self.spillHelper.prepareForRestoring(lastRound)
//         if err != None {
//             self.joinResultCh <- &hashjoinWorkerResult{err: err}
//             return
//         }
//         restoredPartition = self.spillHelper.stack.pop()
//         if restoredPartition == None {
// No more data to restore
//             return
//         }
//         self.spillHelper.round = restoredPartition.round
//         if self.memTracker.BytesConsumed() != 0 {
//             self.isMemoryClearedForTest = false
//         }
//         lastRound = restoredPartition.round
//         self.restoredBuildInDisk = restoredPartition.buildSideChunks
//         self.restoredProbeInDisk = restoredPartition.probeSideChunks
//         if self.stats != None && self.stats.spill.round < lastRound {
//             self.stats.spill.round = lastRound
//         }
//         self.inRestore = true
//     }
// }
// }
// resetProbeStatus 对应 Go 声明 `func (e *HashJoinV2Exec) resetProbeStatus() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn resetProbeStatus(&mut self) {
//     for _, probe = range self.ProbeWorkers {
//         probe.JoinProbe.ResetProbe()
//     }
// }
// }
// releaseDisk 对应 Go 声明 `func (e *HashJoinV2Exec) releaseDisk() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn releaseDisk(&mut self) {
//     if self.restoredBuildInDisk != None {
//         for _, inDisk = range self.restoredBuildInDisk {
//             inDisk.Close()
//         }
//         self.restoredBuildInDisk = None
//     }
//     if self.restoredProbeInDisk != None {
//         for _, inDisk = range self.restoredProbeInDisk {
//             inDisk.Close()
//         }
//         self.restoredProbeInDisk = None
//     }
// }
// }
// Next implements the Executor Next interface.
// hash join constructs the result following these steps:
// step 1. fetch data from build side child and build a hash table;
// step 2. fetch data from probe child in a background goroutine and probe the hash table in multiple join workers.
// Next 对应 Go 声明 `func (e *HashJoinV2Exec) Next(ctx context.Context, req *chunk.Chunk) (err error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn Next(&mut self, ctx: context::Context, req: Option<Box<chunk::Chunk>>) -> (errors::Error) {
//     if !self.prepared {
//         self.initHashTableContext()
//         self.initializeForProbe()
//         self.spillHelper.setCanSpillFlag(true)
//         self.buildFinished = make(chan error, 1)
//         self.hashTableContext.memoryTracker.AttachTo(self.memTracker)
// Go 这里启动 goroutine；仅保留异步/并发启动点，不真正调度线程。
//         go self.startBuildAndProbe(ctx)
//         self.prepared = true
//     }
//     if self.ProbeSideTupleFetcher.shouldLimitProbeFetchSize() {
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//         atomic.StoreInt64(&self.ProbeSideTupleFetcher.requiredRows, int64(req.RequiredRows()))
//     }
//     req.Reset()
//     result, ok = <-self.joinResultCh
//     if !ok {
//         return None
//     }
//     if result.err != None {
//         self.finished.Store(true)
//         return result.err
//     }
//     req.SwapColumns(result.chk)
//     result.src <- result.chk
//     return None
// }
// }
// handleFetchAndBuildHashTablePanic 对应 Go 声明 `func (e *HashJoinV2Exec) handleFetchAndBuildHashTablePanic(r any) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn handleFetchAndBuildHashTablePanic(&mut self, r: Box<dyn Any>) {
//     if r != None {
//         self.buildFinished <- util.GetRecoverError(r)
//     }
//     close(self.buildFinished)
// }
// }
// checkBalance checks whether the segment count of each partition is balanced.
// checkBalance 对应 Go 声明 `func (e *HashJoinV2Exec) checkBalance(totalSegmentCnt int) bool {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn checkBalance(&mut self, totalSegmentCnt: i32) -> bool {
//     isBalanced = self.Concurrency == self.partitionNumber
//     if !isBalanced {
//         return false
//     }
//     avgSegCnt = totalSegmentCnt / int(self.partitionNumber)
//     balanceThreshold = int(float64(avgSegCnt) * 0.8)
//     subTables = self.HashJoinCtxV2.hashTableContext.hashTable.tables
//     for _, subTable = range subTables {
//         if math.Abs(float64(len(subTable.rowData.segments)-avgSegCnt)) > float64(balanceThreshold) {
//             isBalanced = false
//             break
//         }
//     }
//     return isBalanced
// }
// }
// createTasks 对应 Go 声明 `func (e *HashJoinV2Exec) createTasks(buildTaskCh chan<- *buildTask, totalSegmentCnt int, doneCh chan struct{}) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn createTasks(&mut self, buildTaskCh chan<-: Option<Box<buildTask>>, totalSegmentCnt: i32, doneCh chan: ()) {
//     isBalanced = self.checkBalance(totalSegmentCnt)
//     segStep = max(1, totalSegmentCnt/int(self.Concurrency))
//     subTables = self.HashJoinCtxV2.hashTableContext.hashTable.tables
//     createBuildTask = func(partIdx int, segStartIdx int, segEndIdx int) *buildTask {
//         return &buildTask{partitionIdx: partIdx, segStartIdx: segStartIdx, segEndIdx: segEndIdx}
//     }
//     failpoint.Inject("createTasksPanic", None)
//     if isBalanced {
//         for partIdx, subTable = range subTables {
//             _ = triggerIntest(5)
//             segmentsLen = len(subTable.rowData.segments)
// Go select 同时监听 channel/context；保留分支结构，后续需替换为异步 select。
//             select {
//             case <-doneCh:
//                 return
//             case buildTaskCh <- createBuildTask(partIdx, 0, segmentsLen):
//             }
//         }
//         return
//     }
//     partitionStartIndex = make([]int, len(subTables))
//     partitionSegmentLength = make([]int, len(subTables))
//     for i = range subTables {
//         partitionStartIndex[i] = 0
//         partitionSegmentLength[i] = len(subTables[i].rowData.segments)
//     }
//     for {
//         hasNewTask = false
//         for partIdx = range subTables {
// create table by round-robin all the partitions so the build thread is likely to build different partition at the same time
//             if partitionStartIndex[partIdx] < partitionSegmentLength[partIdx] {
//                 startIndex = partitionStartIndex[partIdx]
//                 endIndex = min(startIndex+segStep, partitionSegmentLength[partIdx])
//                 select {
//                 case <-doneCh:
//                     return
//                 case buildTaskCh <- createBuildTask(partIdx, startIndex, endIndex):
//                 }
//                 partitionStartIndex[partIdx] = endIndex
//                 hasNewTask = true
//             }
//         }
//         if !hasNewTask {
//             break
//         }
//     }
// }
// }
// fetchAndBuildHashTable 对应 Go 声明 `func (e *HashJoinV2Exec) fetchAndBuildHashTable(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn fetchAndBuildHashTable(&mut self, ctx: context::Context) {
//     self.workerWg.RunWithRecover(func() {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer trace.StartRegion(ctx, "HashJoinHashTableBuilder").End()
//         self.fetchAndBuildHashTableImpl(ctx)
//     }, self.handleFetchAndBuildHashTablePanic)
// }
// }
// fetchAndBuildHashTableImpl 对应 Go 声明 `func (e *HashJoinV2Exec) fetchAndBuildHashTableImpl(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn fetchAndBuildHashTableImpl(&mut self, ctx: context::Context) {
//     if self.stats != None {
//         start = time.Now()
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer func() {
//             self.stats.fetchAndBuildHashTable += int64(time.Since(start))
//         }()
//     }
//     waitJobDone = func(wg *sync.WaitGroup, errCh chan error) bool {
//         wg.Wait()
//         close(errCh)
//         if err = <-errCh; err != None {
//             self.buildFinished <- err
//             return false
//         }
//         return true
//     }
// It's useful when spill is triggered and the fetcher could know when workers finish their works.
//     fetcherAndWorkerSyncer = &sync.WaitGroup{}
//     wg = new(sync.WaitGroup)
//     errCh = make(chan error, 1+self.Concurrency)
// doneCh is used by the consumer(splitAndAppendToRowTable) to info the producer(fetchBuildSideRows) that the consumer meet error and stop consume data
//     doneCh = make(chan struct{}, self.Concurrency)
// init builder, todo maybe the builder can be reused during the whole life cycle of the executor
//     hashJoinCtx = self.HashJoinCtxV2
//     for _, worker = range self.BuildWorkers {
//         worker.builder = createRowTableBuilder(worker.BuildKeyColIdx, hashJoinCtx.BuildKeyTypes, hashJoinCtx.partitionNumber, worker.HasNullableKey, hashJoinCtx.BuildFilter != None, hashJoinCtx.needScanRowTableAfterProbeDone, hashJoinCtx.hashTableMeta.nullMapLength)
//     }
//     srcChkCh = self.fetchBuildSideRows(ctx, fetcherAndWorkerSyncer, wg, errCh, doneCh)
//     self.splitAndAppendToRowTable(srcChkCh, fetcherAndWorkerSyncer, wg, errCh, doneCh)
//     success = waitJobDone(wg, errCh)
//     if !success {
//         return
//     }
//     if self.spillHelper.spillTriggered {
//         self.spillHelper.spillTriggedInBuildingStageForTest = true
//     }
//     totalSegmentCnt, err = self.hashTableContext.mergeRowTablesToHashTable(self.partitionNumber, self.spillHelper)
//     if err != None {
//         self.buildFinished <- err
//         return
//     }
//     wg = new(sync.WaitGroup)
//     errCh = make(chan error, 1+self.Concurrency)
// doneCh is used by the consumer(buildHashTable) to info the producer(createBuildTasks) that the consumer meet error and stop consume data
//     doneCh = make(chan struct{}, self.Concurrency)
//     buildTaskCh = self.createBuildTasks(totalSegmentCnt, wg, errCh, doneCh)
//     self.buildHashTable(buildTaskCh, wg, errCh, doneCh)
//     waitJobDone(wg, errCh)
// }
// }
// fetchBuildSideRows 对应 Go 声明 `func (e *HashJoinV2Exec) fetchBuildSideRows(ctx context.Context, fetcherAndWorkerSyncer *sync.WaitGroup, wg *sync.WaitGroup, errCh chan error, doneCh chan struct{}) chan *chunk.Chunk {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn fetchBuildSideRows(&mut self, ctx: context::Context, fetcherAndWorkerSyncer: Option<Box<sync::WaitGroup>>, wg: Option<Box<sync::WaitGroup>>, errCh chan: errors::Error, doneCh chan: ()) -> channel::Channel<Option<Box<chunk::Chunk>>> {
//     srcChkCh = make(chan *chunk.Chunk, 1)
//     wg.Add(1)
//     self.workerWg.RunWithRecover(
//         func() {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//             defer trace.StartRegion(ctx, "HashJoinBuildSideFetcher").End()
//             if self.inRestore {
//                 chunkNum = self.getRestoredBuildChunkNum()
//                 self.controlWorkersForRestore(chunkNum, srcChkCh, fetcherAndWorkerSyncer, errCh, doneCh)
//             } else {
//                 fetcher = self.BuildWorkers[0]
//                 fetcher.fetchBuildSideRows(ctx, &fetcher.HashJoinCtx.hashJoinCtxBase, fetcherAndWorkerSyncer, self.spillHelper, srcChkCh, errCh, doneCh)
//             }
//         },
//         func(r any) {
//             if r != None {
//                 errCh <- util.GetRecoverError(r)
//             }
//             wg.Done()
//         },
//     )
//     return srcChkCh
// }
// }
// getRestoredBuildChunkNum 对应 Go 声明 `func (e *HashJoinV2Exec) getRestoredBuildChunkNum() int {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn getRestoredBuildChunkNum(&mut self) -> i32 {
//     chunkNum = 0
//     for _, inDisk = range self.restoredBuildInDisk {
//         chunkNum += inDisk.NumChunks()
//     }
//     return chunkNum
// }
// }
// controlWorkersForRestore 对应 Go 声明 `func (e *HashJoinV2Exec) controlWorkersForRestore(chunkNum int, syncCh chan *chunk.Chunk, fetcherAndWorkerSyncer *sync.WaitGroup, errCh chan<- error, doneCh <-chan struct{}) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn controlWorkersForRestore(&mut self, chunkNum: i32, syncCh chan: Option<Box<chunk::Chunk>>, fetcherAndWorkerSyncer: Option<Box<sync::WaitGroup>>, errCh chan<-: errors::Error, doneCh <-chan: ()) {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() {
//         close(syncCh)
//         hasError = false
// panic/recover 分支对应 Go 的故障保护，Rust 迁移时需映射为 catch_unwind 或错误返回。
//         if r = recover(); r != None {
//             errCh <- util.GetRecoverError(r)
//             hasError = true
//         }
//         fetcherAndWorkerSyncer.Wait()
// Spill remaining rows
//         if !hasError && self.spillHelper.isSpillTriggered() {
//             err = self.spillHelper.spillRemainingRows()
//             if err != None {
//                 errCh <- err
//             }
//         }
//     }()
//     for range chunkNum {
//         if self.finished.Load() {
//             return
//         }
//         err = checkAndSpillRowTableIfNeeded(fetcherAndWorkerSyncer, self.spillHelper)
//         if err != None {
//             errCh <- err
//             return
//         }
//         err = triggerIntest(2)
//         if err != None {
//             errCh <- err
//             return
//         }
//         fetcherAndWorkerSyncer.Add(1)
// Go select 同时监听 channel/context；保留分支结构，后续需替换为异步 select。
//         select {
//         case <-doneCh:
//             fetcherAndWorkerSyncer.Done()
//             return
//         case <-self.hashJoinCtxBase.closeCh:
//             fetcherAndWorkerSyncer.Done()
//             return
//         case syncCh <- None:
//         }
//     }
// }
// }
// handleErr 对应 Go 声明 `func handleErr(err error, errCh chan error, doneCh chan struct{}) {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn handleErr(err: errors::Error, errCh chan: errors::Error, doneCh chan: ()) {
//     errCh <- err
//     doneCh <- struct{}{}
// }
// splitAndAppendToRowTable 对应 Go 声明 `func (e *HashJoinV2Exec) splitAndAppendToRowTable(srcChkCh chan *chunk.Chunk, fetcherAndWorkerSyncer *sync.WaitGroup, wg *sync.WaitGroup, errCh chan error, doneCh chan struct{}) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn splitAndAppendToRowTable(&mut self, srcChkCh chan: Option<Box<chunk::Chunk>>, fetcherAndWorkerSyncer: Option<Box<sync::WaitGroup>>, wg: Option<Box<sync::WaitGroup>>, errCh chan: errors::Error, doneCh chan: ()) {
//     wg.Add(int(self.Concurrency))
//     for i = range self.Concurrency {
//         workIndex = i
//         self.workerWg.RunWithRecover(
//             func() {
//                 if self.inRestore {
//                     self.BuildWorkers[workIndex].splitPartitionAndAppendToRowTableForRestore(self.restoredBuildInDisk[workIndex], srcChkCh, fetcherAndWorkerSyncer, errCh, doneCh)
//                 } else {
//                     self.BuildWorkers[workIndex].splitPartitionAndAppendToRowTable(self.SessCtx.GetSessionVars().StmtCtx.TypeCtx(), fetcherAndWorkerSyncer, srcChkCh, errCh, doneCh)
//                 }
//             },
//             func(r any) {
//                 if r != None {
//                     errCh <- util.GetRecoverError(r)
//                     doneCh <- struct{}{}
//                 }
//                 wg.Done()
//             },
//         )
//     }
// }
// }
// createBuildTasks 对应 Go 声明 `func (e *HashJoinV2Exec) createBuildTasks(totalSegmentCnt int, wg *sync.WaitGroup, errCh chan error, doneCh chan struct{}) chan *buildTask {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn createBuildTasks(&mut self, totalSegmentCnt: i32, wg: Option<Box<sync::WaitGroup>>, errCh chan: errors::Error, doneCh chan: ()) -> channel::Channel<Option<Box<buildTask>>> {
//     buildTaskCh = make(chan *buildTask, self.Concurrency)
//     wg.Add(1)
//     self.workerWg.RunWithRecover(
//         func() { self.createTasks(buildTaskCh, totalSegmentCnt, doneCh) },
//         func(r any) {
//             if r != None {
//                 errCh <- util.GetRecoverError(r)
//             }
//             close(buildTaskCh)
//             wg.Done()
//         },
//     )
//     return buildTaskCh
// }
// }
// buildHashTable 对应 Go 声明 `func (e *HashJoinV2Exec) buildHashTable(buildTaskCh chan *buildTask, wg *sync.WaitGroup, errCh chan error, doneCh chan struct{}) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV2Exec {
//     pub fn buildHashTable(&mut self, buildTaskCh chan: Option<Box<buildTask>>, wg: Option<Box<sync::WaitGroup>>, errCh chan: errors::Error, doneCh chan: ()) {
//     for i = range self.Concurrency {
//         wg.Add(1)
//         workID = i
//         self.workerWg.RunWithRecover(
//             func() {
//                 err = self.BuildWorkers[workID].buildHashTable(buildTaskCh)
//                 if err != None {
//                     errCh <- err
//                     doneCh <- struct{}{}
//                 }
//             },
//             func(r any) {
//                 if r != None {
//                     errCh <- util.GetRecoverError(r)
//                     doneCh <- struct{}{}
//                 }
//                 wg.Done()
//             },
//         )
//     }
// }
// }
// buildTask 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct buildTask {
//     pub partitionIdx: i32,
//     pub segStartIdx: i32,
//     pub segEndIdx: i32,
// }
// generatePartitionIndex 对应 Go 声明 `func generatePartitionIndex(hashValue uint64, partitionMaskOffset int) uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn generatePartitionIndex(hashValue: u64, partitionMaskOffset: i32) -> u64 {
//     return hashValue >> uint64(partitionMaskOffset)
// }
// getProbeSpillChunkFieldTypes 对应 Go 声明 `func getProbeSpillChunkFieldTypes(probeFieldTypes []*types.FieldType) []*types.FieldType {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn getProbeSpillChunkFieldTypes(probeFieldTypes: Vec<Option<Box<types::FieldType>>>) -> Vec<Option<Box<types::FieldType>>> {
//     ret = make([]*types.FieldType, 0, len(probeFieldTypes)+2)
//     hashValueField = types.NewFieldType(mysql.TypeLonglong)
//     hashValueField.AddFlag(mysql.UnsignedFlag)
//     ret = append(ret, hashValueField)                    // hash value
//     ret = append(ret, types.NewFieldType(mysql.TypeBit)) // serialized key
//     ret = append(ret, probeFieldTypes...)                // row data
//     return ret
// }
// rehash 对应 Go 声明 `func rehash(oldHashValue uint64, rehashBuf []byte, hash hash.Hash64) uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn rehash(oldHashValue: u64, rehashBuf: Vec<u8>, hash: hash::Hash64) -> u64 {
// unsafe 指针/地址操作来自 Go 实现，只保留数据流并提示后续审查所有权。
//     *(*uint64)(unsafe.Pointer(&rehashBuf[0])) = oldHashValue
//     hash.Reset()
//     hash.Write(rehashBuf)
//     return hash.Sum64()
// }
// issue59377Intest 对应 Go 声明 `func issue59377Intest(err *error) {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn issue59377Intest(err: Option<Box<errors::Error>>) {
//     failpoint.Inject("Issue59377", func() {
//         *err = errors.New("Random failpoint error is triggered")
//     })
// }
// triggerIntest 对应 Go 声明 `func triggerIntest(errProbability int) error {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn triggerIntest(errProbability: i32) -> Result<(), errors::Error> {
//     failpoint.Inject("slowWorkers", func(val failpoint.Value) {
//         if val.(bool) {
//             num = rand.Intn(100000)
//             if num < 2 {
//                 time.Sleep(time.Duration(num) * time.Millisecond)
//             }
//         }
//     })
//     let mut err: errors::Error = Default::default()
//     failpoint.Inject("panicOrError", func(val failpoint.Value) {
//         if val.(bool) {
//             num = rand.Intn(100000)
//             if num < errProbability/2 {
// panic/recover 分支对应 Go 的故障保护，Rust 迁移时需映射为 catch_unwind 或错误返回。
//                 panic("Random failpoint panic")
//             } else if num < errProbability {
//                 err = errors.New("Random failpoint error is triggered")
//             }
//         }
//     })
//     return err
// }
// */
use crate::hash_join_base::{
    BuildWorkerBase, HashJoinContextBase, HashJoinWorkerResult, ProbeWorkerBase,
};
use crate::hash_join_stats::HashJoinRuntimeStatsV2;
use crate::hash_join_v1::ExecutorState;
use crate::hash_table_v2::{HashTableV2, RowPos};
use crate::join_row_table::RowTable;
use crate::join_table_meta::{FieldKind, FieldType, JoinTableMeta, new_table_meta};
use crate::joiner::{JoinType, Joiner, NaajType, Row};
use crate::row_table_builder::{Chunk, RowTableBuilder, Value};
use astersql_util_execdetails::execdetails::{HashStateRuntimeStats, RuntimeStatsColl};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 构建任务：覆盖哈希表某一行号区间，供多 worker 并行建表。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildTask {
    pub partition_index: usize,
    pub segment_start_index: usize,
    pub segment_end_index: usize,
}

/// v2 内嵌的简化 spill helper：记录已 spill 分区与探测落盘 chunk。

#[derive(Clone, Debug, Default)]
pub struct HashJoinSpillHelper {
    spilled_build: Vec<Option<Vec<Row>>>,
    spilled_probe: Vec<Vec<Row>>,
    pub spilled_bytes: Vec<i64>,
    pub restored_bytes: Vec<i64>,
    pub rounds: Vec<usize>,
    closed: bool,
}

impl HashJoinSpillHelper {
    /// 构造实例。
    pub fn new(partitions: usize) -> Self {
        Self {
            spilled_build: vec![None; partitions],
            spilled_probe: vec![Vec::new(); partitions],
            ..Self::default()
        }
    }
    /// 标记构建侧分区已 spill。
    pub fn spill_build(&mut self, partition: usize, rows: Vec<Row>) {
        let bytes = rows.iter().map(row_size).sum::<usize>() as i64;
        self.spilled_build[partition] = Some(rows);
        grow_add(&mut self.spilled_bytes, 0, bytes);
    }
    /// 将探测侧 chunk 记入指定分区的 spill 缓冲。
    pub fn spill_probe(&mut self, partition: usize, row: Row) {
        grow_add(&mut self.spilled_bytes, 0, row_size(&row) as i64);
        self.spilled_probe[partition].push(row);
    }
    /// 已 spill 的分区下标。
    pub fn spilled_partitions(&self) -> Vec<usize> {
        self.spilled_build
            .iter()
            .enumerate()
            .filter_map(|(index, rows)| rows.as_ref().map(|_| index))
            .collect()
    }
    /// 恢复指定分区的探测侧落盘数据。
    pub fn restore_partition(&mut self, partition: usize) -> Option<(Vec<Row>, Vec<Row>)> {
        let build = self.spilled_build.get_mut(partition)?.take()?;
        let probe = std::mem::take(&mut self.spilled_probe[partition]);
        let bytes = build.iter().chain(&probe).map(row_size).sum::<usize>() as i64;
        grow_add(&mut self.restored_bytes, 0, bytes);
        Some((build, probe))
    }
    /// 关闭执行器。
    pub fn close(&mut self) {
        self.spilled_build.clear();
        self.spilled_probe.clear();
        self.closed = true;
    }
    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// 分区化哈希表上下文：按分区存放构建行并支持查找/未匹配扫描。

pub struct HashTableContext {
    pub hash_table: HashTableV2,
    pub original_rows: Vec<Vec<Row>>,
    pub meta: JoinTableMeta,
    pub memory_bytes: i64,
}

impl HashTableContext {
    /// 为指定分区区间建立可查找索引。
    pub fn build(
        partitioned_rows: Vec<Vec<Row>>,
        meta: JoinTableMeta,
        key_indices: &[usize],
    ) -> Result<Self, String> {
        let mut tables = Vec::with_capacity(partitioned_rows.len());
        for rows in &partitioned_rows {
            tables.push(build_row_table(rows, &meta, key_indices)?);
        }
        let hash_table = HashTableV2::new(tables);
        let memory_bytes = hash_table.total_memory_usage();
        Ok(Self {
            hash_table,
            original_rows: partitioned_rows,
            meta,
            memory_bytes,
        })
    }
    /// 重置执行状态与统计。
    pub fn reset(&mut self) {
        for partition in 0..self.original_rows.len() {
            self.hash_table.clear_partition_segments(partition);
        }
        self.original_rows.iter_mut().for_each(Vec::clear);
        self.memory_bytes = 0;
    }
    /// 在指定分区查找与探测行键相等的构建行。
    pub fn lookup(&self, partition: usize, hash: u64) -> Vec<RowPos> {
        self.hash_table.lookup(partition, hash)
    }
    /// 按全局下标取原始构建行。
    pub fn original_row(&self, position: RowPos) -> Option<&Row> {
        let table = self
            .hash_table
            .create_row_iter(0, self.hash_table.total_row_count())
            .ok()?;
        let mut partition_offset = 0_usize;
        for prior in 0..position.row_segment_index {
            partition_offset += self.hash_table_row_count(position.sub_table_index, prior);
        }
        let _ = table;
        self.original_rows
            .get(position.sub_table_index)?
            .get(partition_offset + position.row_index)
    }
    /// 哈希表总行数。
    fn hash_table_row_count(&self, partition: usize, segment: usize) -> usize {
        self.hash_table
            .create_row_iter(0, self.hash_table.total_row_count())
            .ok()
            .map(|iter| {
                iter.filter(|position| {
                    position.sub_table_index == partition && position.row_segment_index == segment
                })
                .count()
            })
            .unwrap_or(0)
    }
    /// 标记构建行已匹配。
    pub fn mark_used(&self, position: RowPos) {
        if let Some(row) = self.hash_table.get_row(position) {
            self.meta.set_used_flag(row);
        }
    }
    /// 返回指定分区中未匹配的构建行。
    pub fn unmatched_rows(&self) -> Vec<Row> {
        let mut result = Vec::new();
        for position in self
            .hash_table
            .create_row_iter(0, self.hash_table.total_row_count())
            .into_iter()
            .flatten()
        {
            if let Some(encoded) = self.hash_table.get_row(position) {
                if !self.meta.is_current_row_used_atomic(encoded) {
                    if let Some(row) = self.original_row(position) {
                        result.push(row.clone());
                    }
                }
            }
        }
        result
    }
    /// 清空某分区数据。
    pub fn clear_partition(&mut self, partition: usize) {
        self.hash_table.clear_partition_segments(partition);
        self.memory_bytes = self.hash_table.total_memory_usage();
    }
}

/// Hash Join v2 上下文：键列、分区数、并发、spill 与运行时统计。

#[derive(Clone)]
pub struct HashJoinCtxV2 {
    pub base: HashJoinContextBase,
    pub partition_number: usize,
    pub partition_mask_offset: u32,
    pub build_key_indices: Vec<usize>,
    pub probe_key_indices: Vec<usize>,
    pub join_type: JoinType,
    pub right_as_build_side: bool,
    pub null_aware: bool,
    pub concurrency: usize,
    pub max_chunk_size: usize,
    pub memory_limit: Option<i64>,
    pub max_spill_round: usize,
    pub need_scan_row_table_after_probe_done: bool,
}

impl HashJoinCtxV2 {
    /// 构造实例。
    pub fn new(
        join_type: JoinType,
        build_key_indices: Vec<usize>,
        probe_key_indices: Vec<usize>,
        right_as_build_side: bool,
        null_aware: bool,
        concurrency: usize,
        max_chunk_size: usize,
        memory_limit: Option<i64>,
    ) -> Result<Self, String> {
        if build_key_indices.len() != probe_key_indices.len() || build_key_indices.is_empty() {
            return Err("hash join key metadata mismatch".into());
        }
        if concurrency == 0 || max_chunk_size == 0 {
            return Err("concurrency and max chunk size must be positive".into());
        }
        let partition_number = gen_hash_join_partition_number(concurrency);
        let partition_mask_offset = get_partition_mask_offset(partition_number);
        let max_spill_round = if partition_number > 1024 {
            1
        } else {
            ((1024_f64.ln() / (partition_number.max(2) as f64).ln()).floor() as usize).max(1)
        };
        Ok(Self {
            base: HashJoinContextBase::default(),
            partition_number,
            partition_mask_offset,
            build_key_indices,
            probe_key_indices,
            join_type,
            right_as_build_side,
            null_aware,
            concurrency,
            max_chunk_size,
            memory_limit,
            max_spill_round,
            need_scan_row_table_after_probe_done: matches!(
                join_type,
                JoinType::LeftOuter | JoinType::RightOuter
            ),
        })
    }
    /// 设置分区数与掩码偏移。
    pub fn setup_partition_info(&mut self) {
        self.partition_number = gen_hash_join_partition_number(self.concurrency);
        self.partition_mask_offset = get_partition_mask_offset(self.partition_number);
    }
}

/// v2 构建 worker：负责分区建表任务。

#[derive(Clone)]
pub struct BuildWorkerV2 {
    pub base: BuildWorkerBase,
    pub worker_id: usize,
}
impl BuildWorkerV2 {
    /// 构造实例。
    pub fn new(worker_id: usize, context: HashJoinContextBase, memory_limit: Option<i64>) -> Self {
        Self {
            base: BuildWorkerBase::new(worker_id, context, memory_limit),
            worker_id,
        }
    }
    /// 按连接键哈希把行写入对应分区桶。
    pub fn split_partition_and_append(
        &self,
        context: &HashJoinCtxV2,
        chunks: &[Chunk],
    ) -> Result<Vec<Vec<Row>>, String> {
        self.base.run_guarded(|| {
            let mut partitions = vec![Vec::new(); context.partition_number];
            for row in chunks.iter().flatten() {
                let hash = hash_row(row, &context.build_key_indices)?;
                let partition = generate_partition_index(hash, context.partition_mask_offset)
                    as usize
                    & (context.partition_number - 1);
                partitions[partition].push(row.clone());
            }
            Ok(partitions)
        })
    }
}

/// v2 探测 worker：对探测行在分区哈希表上 probe。

#[derive(Clone)]
pub struct ProbeWorkerV2 {
    pub base: ProbeWorkerBase,
    pub worker_id: usize,
}
impl ProbeWorkerV2 {
    /// 构造实例。
    pub fn new(worker_id: usize, context: HashJoinContextBase) -> Self {
        Self {
            base: ProbeWorkerBase::new(worker_id, context),
            worker_id,
        }
    }
    /// 对单行探测并产出连接结果。
    fn probe_row(
        &self,
        context: &HashJoinCtxV2,
        table: &HashTableContext,
        joiner: &Joiner,
        probe: &Row,
        output: &mut Vec<Row>,
    ) -> Result<(), String> {
        let hash = hash_row(probe, &context.probe_key_indices)?;
        let partition = generate_partition_index(hash, context.partition_mask_offset) as usize
            & (context.partition_number - 1);
        let positions: Vec<RowPos> = table
            .lookup(partition, hash)
            .into_iter()
            .filter(|position| {
                table.original_row(*position).is_some_and(|build| {
                    keys_equal(
                        build,
                        probe,
                        &context.build_key_indices,
                        &context.probe_key_indices,
                    )
                })
            })
            .collect();
        let builds: Vec<Row> = positions
            .iter()
            .filter_map(|position| table.original_row(*position).cloned())
            .collect();
        let probe_key_has_null = context
            .probe_key_indices
            .iter()
            .any(|key| matches!(probe.get(*key), Some(Value::Null)));
        let build_has_null_key = table.original_rows.iter().flatten().any(|row| {
            context
                .build_key_indices
                .iter()
                .any(|key| matches!(row.get(*key), Some(Value::Null)))
        });
        let outer_side_build = matches!(
            (context.join_type, context.right_as_build_side),
            (JoinType::LeftOuter, false) | (JoinType::RightOuter, true)
        );
        if outer_side_build {
            let statuses = joiner.try_to_match_outers(&builds, probe, output)?;
            for (position, status) in positions.into_iter().zip(statuses) {
                if status == crate::joiner::OuterRowStatus::Matched {
                    table.mark_used(position);
                }
            }
        } else {
            let naaj = if context.null_aware {
                if probe_key_has_null {
                    NaajType::LeftHasNullRightNotNull
                } else {
                    NaajType::LeftNotNullRightNotNull
                }
            } else {
                NaajType::Unknown
            };
            let result = joiner.try_to_match_inners(probe, &builds, output, naaj)?;
            if result.matched {
                for position in positions {
                    table.mark_used(position);
                }
            } else {
                let has_null = if context.null_aware
                    && matches!(context.join_type, JoinType::AntiLeftOuterSemi)
                {
                    probe_key_has_null || build_has_null_key
                } else {
                    result.has_null
                };
                joiner.on_miss_match(has_null, probe, output);
            }
        }
        Ok(())
    }
}

/// v2 探测侧取数器。

pub struct ProbeSideTupleFetcherV2 {
    chunks: Vec<Chunk>,
    cursor: usize,
    pub can_skip_probe_if_hash_table_is_empty: bool,
}
impl ProbeSideTupleFetcherV2 {
    /// 构造实例。
    pub fn new(chunks: Vec<Chunk>) -> Self {
        Self {
            chunks,
            cursor: 0,
            can_skip_probe_if_hash_table_is_empty: false,
        }
    }
    /// 取下一块探测数据。
    pub fn next_chunk(&mut self) -> Option<Chunk> {
        let result = self.chunks.get(self.cursor).cloned();
        self.cursor += usize::from(result.is_some());
        result
    }
    /// 重置执行状态与统计。
    pub fn reset(&mut self) {
        self.cursor = 0;
    }
}

/// Hash Join v2 执行器：分区建表、并行 probe，支持 spill/恢复。

pub struct HashJoinV2Exec {
    pub context: HashJoinCtxV2,
    pub joiner: Joiner,
    build_chunks: Vec<Chunk>,
    probe_fetcher: ProbeSideTupleFetcherV2,
    hash_table_context: Option<HashTableContext>,
    spill_helper: HashJoinSpillHelper,
    output: Vec<Row>,
    cursor: usize,
    state: ExecutorState,
    prepared: bool,
    in_restore: bool,
    pub stats: HashJoinRuntimeStatsV2,
    hash_state_stats: Option<HashStateRuntimeStats>,
    runtime_stats_coll: Option<Arc<Mutex<RuntimeStatsColl>>>,
    plan_id: i32,
}

impl HashJoinV2Exec {
    /// 构造实例。
    pub fn new(
        context: HashJoinCtxV2,
        joiner: Joiner,
        build_chunks: Vec<Chunk>,
        probe_chunks: Vec<Chunk>,
    ) -> Result<Self, String> {
        if context.join_type != joiner.join_type() {
            return Err("joiner type differs from V2 context".into());
        }
        let partitions = context.partition_number;
        Ok(Self {
            context,
            joiner,
            build_chunks,
            probe_fetcher: ProbeSideTupleFetcherV2::new(probe_chunks),
            hash_table_context: None,
            spill_helper: HashJoinSpillHelper::new(partitions),
            output: Vec::new(),
            cursor: 0,
            state: ExecutorState::Created,
            prepared: false,
            in_restore: false,
            stats: HashJoinRuntimeStatsV2::default(),
            hash_state_stats: None,
            runtime_stats_coll: None,
            plan_id: 0,
        })
    }
    /// 为执行器启用 typed hash-state 运行时证据。
    pub fn with_runtime_stats(
        mut self,
        plan_id: i32,
        runtime_stats_coll: Arc<Mutex<RuntimeStatsColl>>,
    ) -> Self {
        self.plan_id = plan_id;
        self.runtime_stats_coll = Some(runtime_stats_coll);
        self
    }
    /// 替换下一次 Open 使用的构建侧输入，覆盖 Go 重复 Open 的执行器复用路径。
    pub fn set_build_chunks(&mut self, build_chunks: Vec<Chunk>) {
        self.build_chunks = build_chunks;
    }
    /// 打开并启动构建与探测流水线。
    pub fn open(&mut self) -> Result<(), String> {
        self.context.base.reset();
        self.context.setup_partition_info();
        self.probe_fetcher.reset();
        self.spill_helper = HashJoinSpillHelper::new(self.context.partition_number);
        self.hash_table_context = None;
        self.output.clear();
        self.cursor = 0;
        self.prepared = false;
        self.stats.reset();
        self.stats.concurrency = self.context.concurrency;
        self.hash_state_stats = self
            .runtime_stats_coll
            .as_ref()
            .map(|_| HashStateRuntimeStats::default());
        self.state = ExecutorState::Open;
        Ok(())
    }
    /// 取构建侧数据并建立分区哈希表。
    fn fetch_and_build_hash_table(&mut self) -> Result<(), String> {
        let start = Instant::now();
        let worker = BuildWorkerV2::new(0, self.context.base.clone(), self.context.memory_limit);
        let partitions = worker.split_partition_and_append(&self.context, &self.build_chunks)?;
        let build_types = infer_field_types(
            self.build_chunks
                .iter()
                .flatten()
                .next()
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        let key_types: Vec<FieldType> = self
            .context
            .build_key_indices
            .iter()
            .map(|index| {
                build_types
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| format!("build key index {index} is out of bounds"))
            })
            .collect::<Result<_, _>>()?;
        let meta = new_table_meta(
            &self.context.build_key_indices,
            &build_types,
            &key_types,
            &key_types,
            &[],
            &(0..build_types.len()).collect::<Vec<_>>(),
            self.context.need_scan_row_table_after_probe_done,
        )?;
        let mut table = HashTableContext::build(partitions, meta, &self.context.build_key_indices)?;
        if let Some(limit) = self.context.memory_limit {
            if table.memory_bytes > limit {
                let mut candidates: Vec<(usize, i64)> = (0..self.context.partition_number)
                    .map(|partition| {
                        (
                            partition,
                            table.hash_table.partition_memory_usage(partition),
                        )
                    })
                    .collect();
                candidates.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
                let mut memory = table.memory_bytes;
                let mut count = 0;
                for (partition, bytes) in candidates {
                    if memory <= limit {
                        break;
                    }
                    let rows = std::mem::take(&mut table.original_rows[partition]);
                    table.clear_partition(partition);
                    memory = memory.saturating_sub(bytes);
                    if rows.is_empty() {
                        continue;
                    }
                    // 内存压力下标记分区 spill。
                    self.spill_helper.spill_build(partition, rows);
                    count += 1;
                }
                if count > 0 {
                    self.context.base.set_spilled();
                }
                self.spill_helper.rounds.push(count);
            }
        }
        self.stats.fetch_and_build += start.elapsed();
        self.stats.max_build_hash_table = self.stats.max_build_hash_table.max(start.elapsed());
        if let Some(stats) = &self.hash_state_stats {
            stats.AddRows(table.hash_table.total_row_count() as u64);
        }
        self.hash_table_context = Some(table);
        self.context.base.finish_build();
        Ok(())
    }
    /// 空构建表时是否可跳过 probe（取决于 join 类型）。
    pub fn can_skip_probe_if_hash_table_is_empty(&self) -> bool {
        match self.context.join_type {
            JoinType::Inner => true,
            JoinType::LeftOuter => !self.context.right_as_build_side,
            JoinType::RightOuter => self.context.right_as_build_side,
            JoinType::Semi => self.context.right_as_build_side,
            _ => false,
        }
    }
    /// 取探测侧数据并做 probe。
    fn fetch_and_probe_hash_table(&mut self) -> Result<(), String> {
        self.context.base.wait_for_build_side()?;
        let start = Instant::now();
        let worker = ProbeWorkerV2::new(0, self.context.base.clone());
        let table = self
            .hash_table_context
            .as_ref()
            .ok_or_else(|| "V2 hash table missing".to_string())?;
        self.probe_fetcher.can_skip_probe_if_hash_table_is_empty =
            self.can_skip_probe_if_hash_table_is_empty();
        let has_spilled_build = !self.spill_helper.spilled_partitions().is_empty();
        if !(table.hash_table.is_hash_table_empty()
            && self.probe_fetcher.can_skip_probe_if_hash_table_is_empty
            && !has_spilled_build)
        {
            while let Some(chunk) = self.probe_fetcher.next_chunk() {
                worker.base.run_guarded(|| {
                    for probe in chunk {
                        let hash = hash_row(&probe, &self.context.probe_key_indices)?;
                        let partition =
                            generate_partition_index(hash, self.context.partition_mask_offset)
                                as usize
                                & (self.context.partition_number - 1);
                        if self.spill_helper.spilled_build[partition].is_some() {
                            self.spill_helper.spill_probe(partition, probe);
                        } else {
                            worker.probe_row(
                                &self.context,
                                table,
                                &self.joiner,
                                &probe,
                                &mut self.output,
                            )?;
                        }
                    }
                    Ok(())
                })?;
            }
        }
        self.stats.fetch_and_probe += start.elapsed();
        self.stats.probe += start.elapsed();
        Ok(())
    }
    /// 从 spill 恢复分区后继续 probe。
    fn restore_and_probe(&mut self) -> Result<(), String> {
        self.in_restore = true;
        for partition in self.spill_helper.spilled_partitions() {
            let Some((build, probe)) = self.spill_helper.restore_partition(partition) else {
                continue;
            };
            let build_types = infer_field_types(build.first().map(Vec::as_slice).unwrap_or(&[]));
            let key_types: Vec<FieldType> = self
                .context
                .build_key_indices
                .iter()
                .filter_map(|index| build_types.get(*index).cloned())
                .collect();
            let meta = new_table_meta(
                &self.context.build_key_indices,
                &build_types,
                &key_types,
                &key_types,
                &[],
                &(0..build_types.len()).collect::<Vec<_>>(),
                self.context.need_scan_row_table_after_probe_done,
            )?;
            let mut partitions = vec![Vec::new(); self.context.partition_number];
            partitions[partition] = build;
            let table = HashTableContext::build(partitions, meta, &self.context.build_key_indices)?;
            if let Some(stats) = &self.hash_state_stats {
                stats.AddRows(table.hash_table.total_row_count() as u64);
            }
            let worker = ProbeWorkerV2::new(0, self.context.base.clone());
            for row in probe {
                worker.probe_row(&self.context, &table, &self.joiner, &row, &mut self.output)?;
            }
            if self.context.need_scan_row_table_after_probe_done {
                for row in table.unmatched_rows() {
                    self.joiner.on_miss_match(false, &row, &mut self.output);
                }
            }
        }
        self.in_restore = false;
        Ok(())
    }
    /// 启动构建哈希表并准备探测。
    fn start_build_and_probe(&mut self) -> Result<(), String> {
        self.fetch_and_build_hash_table()?;
        self.fetch_and_probe_hash_table()?;
        self.restore_and_probe()?;
        if self.context.need_scan_row_table_after_probe_done {
            if let Some(table) = &self.hash_table_context {
                for row in table.unmatched_rows() {
                    self.joiner.on_miss_match(false, &row, &mut self.output);
                }
            }
        }
        self.collect_spill_stats();
        if let Some(stats) = &self.hash_state_stats {
            stats.Complete();
        }
        self.prepared = true;
        Ok(())
    }
    /// 汇总 spill 统计到 runtime stats。
    fn collect_spill_stats(&mut self) {
        self.stats.spill.spilled_partition_num = self.spill_helper.rounds.clone();
        self.stats.spill.spilled_bytes = self.spill_helper.spilled_bytes.clone();
        self.stats.spill.restored_bytes = self.spill_helper.restored_bytes.clone();
    }
    /// 拉取下一批结果。
    pub fn next(&mut self) -> Result<HashJoinWorkerResult, String> {
        if self.state == ExecutorState::Created {
            self.open()?;
        }
        if self.state == ExecutorState::Closed {
            return Err("hash join V2 is closed".into());
        }
        if !self.prepared {
            if let Err(error) = self.start_build_and_probe() {
                if let Some(stats) = &self.hash_state_stats {
                    stats.Invalidate();
                }
                self.context.base.fail(error.clone());
                return Err(error);
            }
        }
        if self.cursor >= self.output.len() {
            self.state = ExecutorState::Exhausted;
            return Ok(HashJoinWorkerResult::default());
        }
        let end = (self.cursor + self.context.max_chunk_size).min(self.output.len());
        let rows = self.output[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(HashJoinWorkerResult { rows, error: None })
    }
    /// 执行并收集全部结果。
    pub fn execute_all(&mut self) -> Result<Vec<Row>, String> {
        let mut rows = Vec::new();
        loop {
            let result = self.next()?;
            if result.rows.is_empty() {
                break;
            }
            rows.extend(result.rows);
        }
        Ok(rows)
    }
    /// 测试：内存是否已全部清理。
    pub fn is_all_memory_cleared_for_test(&self) -> bool {
        self.hash_table_context
            .as_ref()
            .is_none_or(|table| table.memory_bytes == 0)
    }
    /// 关闭执行器。
    pub fn close(&mut self) {
        if let (Some(collection), Some(stats)) =
            (&self.runtime_stats_coll, self.hash_state_stats.take())
        {
            collection
                .lock()
                .expect("runtime stats lock poisoned")
                .RegisterStats(self.plan_id, Box::new(stats));
        }
        self.context.base.cancel();
        if let Some(table) = self.hash_table_context.as_mut() {
            table.reset();
        }
        self.hash_table_context = None;
        self.spill_helper.close();
        self.output.clear();
        self.state = ExecutorState::Closed;
    }
}

/// 根据 hint 规范化分区数（通常取 2 的幂）。

pub fn gen_hash_join_partition_number(partition_hint: usize) -> usize {
    let mut partitions = 1;
    while partitions < partition_hint && partitions < 16 {
        partitions <<= 1;
    }
    partitions
}
/// 由分区数得到哈希掩码偏移。
pub fn get_partition_mask_offset(partition_number: usize) -> u32 {
    64 - partition_number.trailing_zeros()
}
/// 由哈希值与掩码偏移计算分区下标。
pub fn generate_partition_index(hash_value: u64, partition_mask_offset: u32) -> u64 {
    if partition_mask_offset >= 64 {
        0
    } else {
        hash_value >> partition_mask_offset
    }
}
/// 对旧哈希值再哈希（多轮 spill 再分区时用）。
pub fn rehash(old_hash_value: u64) -> u64 {
    hash_bytes(&old_hash_value.to_le_bytes())
}
/// 按并发度切分建表任务区间。
pub fn create_build_tasks(table: &HashTableV2, concurrency: usize) -> Vec<BuildTask> {
    let total = table.total_row_count() as usize;
    if total == 0 {
        return Vec::new();
    }
    let task_size = total.div_ceil(concurrency.max(1));
    (0..total)
        .step_by(task_size)
        .map(|start| BuildTask {
            partition_index: 0,
            segment_start_index: start,
            segment_end_index: (start + task_size).min(total),
        })
        .collect()
}
/// 检查建表任务负载是否相对均衡。
pub fn check_balance(tasks: &[BuildTask]) -> bool {
    let sizes: Vec<usize> = tasks
        .iter()
        .map(|task| {
            task.segment_end_index
                .saturating_sub(task.segment_start_index)
        })
        .collect();
    sizes
        .iter()
        .max()
        .zip(sizes.iter().min())
        .is_none_or(|(max, min)| max.saturating_sub(*min) <= 1)
}

/// 把 chunk 行写入分区行表。

fn build_row_table(
    rows: &[Row],
    meta: &JoinTableMeta,
    key_indices: &[usize],
) -> Result<RowTable, String> {
    if rows.is_empty() {
        return Ok(RowTable::default());
    }
    let mut builder = RowTableBuilder::new(
        key_indices.to_vec(),
        1,
        true,
        false,
        true,
        meta.null_map_length,
    )?;
    builder.process_chunk(&rows.to_vec(), meta, None, 0)
}
/// 从样例行推断字段类型。
fn infer_field_types(row: &[Value]) -> Vec<FieldType> {
    row.iter()
        .map(|value| FieldType {
            kind: match value {
                Value::Null | Value::Int(_) => FieldKind::SignedInt,
                Value::UInt(_) => FieldKind::UnsignedInt,
                Value::Bool(_) => FieldKind::UnsignedInt,
                Value::Float(_) => FieldKind::Float,
                Value::Bytes(_) => FieldKind::Bytes,
                Value::Text(_) => FieldKind::Text {
                    collation: "binary".into(),
                },
            },
            fixed_length: match value {
                Value::Bytes(_) | Value::Text(_) => None,
                _ => Some(8),
            },
            nullable: matches!(value, Value::Null),
        })
        .collect()
}
/// 计算一行连接键的哈希。
fn hash_row(row: &Row, indices: &[usize]) -> Result<u64, String> {
    let mut bytes = Vec::new();
    let variable_serialized = indices
        .iter()
        .any(|index| matches!(row.get(*index), Some(Value::Bytes(_) | Value::Text(_))));
    for index in indices {
        let start = bytes.len();
        encode_value(
            row.get(*index)
                .ok_or_else(|| format!("join key index {index} is out of bounds"))?,
            &mut bytes,
        );
        if variable_serialized {
            let length = (bytes.len() - start) as u32;
            bytes.splice(start..start, length.to_le_bytes());
        }
    }
    Ok(hash_bytes(&bytes))
}
/// 编码单个 `Value` 到字节缓冲。
fn encode_value(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Null => output.push(0),
        Value::Bool(v) => output.push(u8::from(*v)),
        Value::Int(v) => output.extend_from_slice(&v.to_le_bytes()),
        Value::UInt(v) => output.extend_from_slice(&v.to_le_bytes()),
        Value::Float(v) => output.extend_from_slice(&v.to_bits().to_le_bytes()),
        Value::Bytes(v) => {
            output.extend_from_slice(&(v.len() as u32).to_le_bytes());
            output.extend_from_slice(v);
        }
        Value::Text(v) => {
            output.extend_from_slice(&(v.len() as u32).to_le_bytes());
            output.extend_from_slice(v.as_bytes());
        }
    }
}
/// 字节序列哈希。
fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1469598103934665603_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}
/// 构建/探测键列交叉等值比较。
fn keys_equal(build: &Row, probe: &Row, build_indices: &[usize], probe_indices: &[usize]) -> bool {
    build_indices
        .iter()
        .zip(probe_indices)
        .all(|(build_index, probe_index)| {
            match (build.get(*build_index), probe.get(*probe_index)) {
                (Some(Value::Null), _) | (_, Some(Value::Null)) => false,
                (Some(left), Some(right)) => left == right,
                _ => false,
            }
        })
}
/// 估算行字节占用。
fn row_size(row: &Row) -> usize {
    row.iter()
        .map(|value| match value {
            Value::Bytes(value) => value.len(),
            Value::Text(value) => value.len(),
            _ => 8,
        })
        .sum()
}
/// 按轮次扩容向量并累加数值。
fn grow_add(values: &mut Vec<i64>, round: usize, value: i64) {
    values.resize(round + 1, 0);
    values[round] += value;
}
