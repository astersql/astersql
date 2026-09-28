// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Hash Join 执行器 v1。
//
// 先构建侧建立哈希表，再对探测侧逐批 probe；支持 null-aware anti join（NAAJ）。
// 同文件还包含 Nested Loop Apply 执行器骨架。对应 Go `hash_join.go` v1 路径。

// HashJoin V1 的构建端、探测端、NestedLoopApply 和运行时统计流程。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// #![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables)]
// use std::any::Any;
// use std::collections::HashMap;
// IsChildCloseCalledForTest is used for test
// 变量声明对应 Go var；全局状态和测试钩子的并发语义后续需用 Rust 原语重建。
// let mut IsChildCloseCalledForTest: atomic::Bool = Default::default()
// 变量声明对应 Go var；全局状态和测试钩子的并发语义后续需用 Rust 原语重建。
// var (
//     _ exec.Executor = &HashJoinV1Exec{}
//     _ exec.Executor = &NestedLoopApplyExec{}
// )
// HashJoinCtxV1 is the context used in hash join
// HashJoinCtxV1 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct HashJoinCtxV1 {
//     pub hashJoinCtxBase: hashJoinCtxBase,
//     pub UseOuterToBuild: bool,
//     pub IsOuterJoin: bool,
//     pub RowContainer: Option<Box<hashRowContainer>>,
//     pub outerMatchedStatus: Vec<Option<Box<bitmap::ConcurrentBitmap>>>,
//     pub ProbeTypes: Vec<Option<Box<types::FieldType>>>,
//     pub BuildTypes: Vec<Option<Box<types::FieldType>>>,
//     pub OuterFilter: expression::CNFExprs,
//     pub stats: Option<Box<hashJoinRuntimeStats>>,
// }
// ProbeSideTupleFetcherV1 reads tuples from ProbeSideExec and send them to ProbeWorkers.
// ProbeSideTupleFetcherV1 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct ProbeSideTupleFetcherV1 {
//     pub probeSideTupleFetcherBase: probeSideTupleFetcherBase,
//     pub HashJoinCtxV1: Option<Box<HashJoinCtxV1>>,
// }
// ProbeWorkerV1 is the probe side worker in hash join
// ProbeWorkerV1 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct ProbeWorkerV1 {
//     pub probeWorkerBase: probeWorkerBase,
//     pub HashJoinCtx: Option<Box<HashJoinCtxV1>>,
//     pub ProbeKeyColIdx: Vec<i32>,
//     pub ProbeNAKeyColIdx: Vec<i32>,
// We pre-alloc and reuse the Rows and RowPtrs for each probe goroutine, to avoid allocation frequently
//     pub buildSideRows: Vec<chunk::Row>,
//     pub buildSideRowPtrs: Vec<chunk::RowPtr>,
// We build individual joiner for each join worker when use chunk-based
// execution, to avoid the concurrency of joiner.chk and joiner.selected.
//     pub Joiner: Joiner,
//     pub rowIters: Option<Box<chunk::Iterator4Slice>>,
//     pub rowContainerForProbe: Option<Box<hashRowContainer>>,
// for every naaj probe worker, pre-allocate the int slice for store the join column index to check.
//     pub needCheckBuildColPos: Vec<i32>,
//     pub needCheckProbeColPos: Vec<i32>,
//     pub needCheckBuildTypes: Vec<Option<Box<types::FieldType>>>,
//     pub needCheckProbeTypes: Vec<Option<Box<types::FieldType>>>,
// }
// BuildWorkerV1 is the build side worker in hash join
// BuildWorkerV1 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct BuildWorkerV1 {
//     pub buildWorkerBase: buildWorkerBase,
//     pub HashJoinCtx: Option<Box<HashJoinCtxV1>>,
//     pub BuildNAKeyColIdx: Vec<i32>,
// }
// HashJoinV1Exec implements the hash join algorithm.
// HashJoinV1Exec 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct HashJoinV1Exec {
//     pub BaseExecutor: exec::BaseExecutor,
//     pub HashJoinCtxV1: Option<Box<HashJoinCtxV1>>,
//     pub ProbeSideTupleFetcher: Option<Box<ProbeSideTupleFetcherV1>>,
//     pub ProbeWorkers: Vec<Option<Box<ProbeWorkerV1>>>,
//     pub BuildWorker: Option<Box<BuildWorkerV1>>,
//     pub workerWg: util::WaitGroupWrapper,
//     pub waiterWg: util::WaitGroupWrapper,
//     pub Prepared: bool,
// }
// Close implements the Executor Close interface.
// Close 对应 Go 声明 `func (e *HashJoinV1Exec) Close() error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn Close(&mut self) -> Result<(), errors::Error> {
//     if self.closeCh != None {
//         close(self.closeCh)
//     }
//     self.finished.Store(true)
//     if self.Prepared {
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
//         util.WithRecovery(func() {
//             err = self.RowContainer.Close()
//             if err != None {
//                 logutil.BgLogger().Warn("RowContainer encounters error",
//                     zap.Error(err),
//                     zap.Stack("stack trace"))
//             }
//         }, None)
//         self.HashJoinCtxV1.SessCtx.GetSessionVars().MemTracker.UnbindActionFromHardLimit(self.RowContainer.ActionSpill())
//         self.waiterWg.Wait()
//     }
//     self.outerMatchedStatus = self.outerMatchedStatus[:0]
//     for _, w = range self.ProbeWorkers {
//         w.buildSideRows = None
//         w.buildSideRowPtrs = None
//         w.needCheckBuildColPos = None
//         w.needCheckProbeColPos = None
//         w.needCheckBuildTypes = None
//         w.needCheckProbeTypes = None
//         w.joinChkResourceCh = None
//     }
//     if self.stats != None && self.RowContainer != None {
//         self.stats.hashStat = *self.RowContainer.stat
//     }
//     if self.stats != None {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer self.Ctx().GetSessionVars().StmtCtx.RuntimeStatsColl.RegisterStats(self.ID(), self.stats)
//     }
//     IsChildCloseCalledForTest.Store(true)
//     return self.BaseExecutor.Close()
// }
// }
// Open implements the Executor Open interface.
// Open 对应 Go 声明 `func (e *HashJoinV1Exec) Open(ctx context.Context) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn Open(&mut self, ctx: context::Context) -> Result<(), errors::Error> {
//     if err = self.BaseExecutor.Open(ctx); err != None {
//         self.closeCh = None
//         self.Prepared = false
//         return err
//     }
//     return self.OpenSelf()
// }
// }
// OpenSelf opens join itself and initializes the hash join context.
// OpenSelf 对应 Go 声明 `func (e *HashJoinV1Exec) OpenSelf() error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn OpenSelf(&mut self) -> Result<(), errors::Error> {
//     self.Prepared = false
//     if self.HashJoinCtxV1.memTracker != None {
//         self.HashJoinCtxV1.memTracker.Reset()
//     } else {
//         self.HashJoinCtxV1.memTracker = memory.NewTracker(self.ID(), -1)
//     }
//     self.HashJoinCtxV1.memTracker.AttachTo(self.Ctx().GetSessionVars().StmtCtx.MemTracker)
//     if self.HashJoinCtxV1.diskTracker != None {
//         self.HashJoinCtxV1.diskTracker.Reset()
//     } else {
//         self.HashJoinCtxV1.diskTracker = disk.NewTracker(self.ID(), -1)
//     }
//     self.HashJoinCtxV1.diskTracker.AttachTo(self.Ctx().GetSessionVars().StmtCtx.DiskTracker)
//     self.workerWg = util.WaitGroupWrapper{}
//     self.waiterWg = util.WaitGroupWrapper{}
//     self.closeCh = make(chan struct{})
//     self.finished.Store(false)
//     if self.RuntimeStats() != None {
//         self.stats = &hashJoinRuntimeStats{
//             concurrent: int(self.Concurrency),
//         }
//     }
//     return None
// }
// }
// initializeForProbe 对应 Go 声明 `func (e *HashJoinV1Exec) initializeForProbe() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn initializeForProbe(&mut self) {
//     self.ProbeSideTupleFetcher.HashJoinCtxV1 = self.HashJoinCtxV1
// self.joinResultCh is for transmitting the join result chunks to the main
// thread.
//     self.joinResultCh = make(chan *hashjoinWorkerResult, self.Concurrency+1)
//     self.ProbeSideTupleFetcher.initializeForProbeBase(self.Concurrency, self.joinResultCh)
//     for i = range self.Concurrency {
//         self.ProbeWorkers[i].initializeForProbe(self.ProbeSideTupleFetcher.probeChkResourceCh, self.ProbeSideTupleFetcher.probeResultChs[i], e)
//     }
// }
// }
// fetchAndProbeHashTable 对应 Go 声明 `func (e *HashJoinV1Exec) fetchAndProbeHashTable(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn fetchAndProbeHashTable(&mut self, ctx: context::Context) {
//     self.initializeForProbe()
//     self.workerWg.RunWithRecover(func() {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer trace.StartRegion(ctx, "HashJoinProbeSideFetcher").End()
//         self.ProbeSideTupleFetcher.fetchProbeSideChunks(
//             ctx,
//             self.MaxChunkSize(),
//             func() bool {
//                 return self.ProbeSideTupleFetcher.RowContainer.Len() == uint64(0)
//             },
//             func() bool { return false },
//             self.ProbeSideTupleFetcher.JoinType == base.InnerJoin || self.ProbeSideTupleFetcher.JoinType == base.SemiJoin,
//             false,
//             self.ProbeSideTupleFetcher.IsOuterJoin,
//             &self.ProbeSideTupleFetcher.hashJoinCtxBase)
//     }, self.ProbeSideTupleFetcher.handleProbeSideFetcherPanic)
//     for i = range self.Concurrency {
//         workerID = i
//         self.workerWg.RunWithRecover(func() {
//             defer trace.StartRegion(ctx, "HashJoinWorker").End()
//             self.ProbeWorkers[workerID].runJoinWorker()
//         }, self.ProbeWorkers[workerID].handleProbeWorkerPanic)
//     }
//     self.waiterWg.RunWithRecover(self.waitJoinWorkersAndCloseResultChan, None)
// }
// }
// handleProbeWorkerPanic 对应 Go 声明 `func (w *ProbeWorkerV1) handleProbeWorkerPanic(r any) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn handleProbeWorkerPanic(&mut self, r: Box<dyn Any>) {
//     if r != None {
//         self.HashJoinCtx.joinResultCh <- &hashjoinWorkerResult{err: util.GetRecoverError(r)}
//     }
// }
// }
// handleJoinWorkerPanic 对应 Go 声明 `func (e *HashJoinV1Exec) handleJoinWorkerPanic(r any) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn handleJoinWorkerPanic(&mut self, r: Box<dyn Any>) {
//     if r != None {
//         self.joinResultCh <- &hashjoinWorkerResult{err: util.GetRecoverError(r)}
//     }
// }
// }
// Concurrently handling unmatched rows from the hash table
// handleUnmatchedRowsFromHashTable 对应 Go 声明 `func (w *ProbeWorkerV1) handleUnmatchedRowsFromHashTable() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn handleUnmatchedRowsFromHashTable(&mut self) {
//     ok, joinResult = self.getNewJoinResult()
//     if !ok {
//         return
//     }
//     numChks = self.rowContainerForProbe.NumChunks()
//     for i = int(self.WorkerID); i < numChks; i += int(self.HashJoinCtx.Concurrency) {
//         chk, err = self.rowContainerForProbe.GetChunk(i)
//         if err != None {
// Catching the error and send it
//             joinResult.err = err
//             self.HashJoinCtx.joinResultCh <- joinResult
//             return
//         }
//         for j = range chk.NumRows() {
//             if !self.HashJoinCtx.outerMatchedStatus[i].UnsafeIsSet(j) { // process unmatched Outer rows
//                 self.Joiner.OnMissMatch(false, chk.GetRow(j), joinResult.chk)
//             }
//             if joinResult.chk.IsFull() {
//                 self.HashJoinCtx.joinResultCh <- joinResult
//                 ok, joinResult = self.getNewJoinResult()
//                 if !ok {
//                     return
//                 }
//             }
//         }
//     }
//     if joinResult == None {
//         return
//     } else if joinResult.err != None || (joinResult.chk != None && joinResult.chk.NumRows() > 0) {
//         self.HashJoinCtx.joinResultCh <- joinResult
//     }
// }
// }
// waitJoinWorkersAndCloseResultChan 对应 Go 声明 `func (e *HashJoinV1Exec) waitJoinWorkersAndCloseResultChan() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn waitJoinWorkersAndCloseResultChan(&mut self) {
//     self.workerWg.Wait()
//     if self.UseOuterToBuild {
// Concurrently handling unmatched rows from the hash table at the tail
//         for i = range self.Concurrency {
//             let mut workerID: = i = Default::default()
//             self.workerWg.RunWithRecover(func() { self.ProbeWorkers[workerID].handleUnmatchedRowsFromHashTable() }, self.handleJoinWorkerPanic)
//         }
//         self.workerWg.Wait()
//     }
//     close(self.joinResultCh)
// }
// }
// runJoinWorker 对应 Go 声明 `func (w *ProbeWorkerV1) runJoinWorker() {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn runJoinWorker(&mut self) {
//     probeTime = int64(0)
//     if self.HashJoinCtx.stats != None {
//         start = time.Now()
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer func() {
//             t = time.Since(start)
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//             atomic.AddInt64(&self.HashJoinCtx.stats.probe, probeTime)
//             atomic.AddInt64(&self.HashJoinCtx.stats.fetchAndProbe, int64(t))
//             setMaxValue(&self.HashJoinCtx.stats.maxFetchAndProbe, int64(t))
//         }()
//     }
//     var (
//         probeSideResult *chunk.Chunk
//         selected        = make([]bool, 0, chunk.InitialCapacity)
//     )
//     ok, joinResult = self.getNewJoinResult()
//     if !ok {
//         return
//     }
// Read and filter probeSideResult, and join the probeSideResult with the build side rows.
//     emptyProbeSideResult = &probeChkResource{
//         dest: self.probeResultCh,
//     }
//     hCtx = &HashContext{
//         AllTypes:    self.HashJoinCtx.ProbeTypes,
//         KeyColIdx:   self.ProbeKeyColIdx,
//         NaKeyColIdx: self.ProbeNAKeyColIdx,
//     }
//     for ok = true; ok; {
//         if self.HashJoinCtx.finished.Load() {
//             break
//         }
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
//         start = time.Now()
// waitTime is the time cost on self.sendingResult(), it should not be added to probe time, because if
// parent executor does not call `e.Next()`, `sendingResult()` will hang, and this hang has nothing to do
// with the probe
//         waitTime = int64(0)
//         if self.HashJoinCtx.UseOuterToBuild {
//             ok, waitTime, joinResult = self.join2ChunkForOuterHashJoin(probeSideResult, hCtx, joinResult)
//         } else {
//             ok, waitTime, joinResult = self.join2Chunk(probeSideResult, hCtx, joinResult, selected)
//         }
//         probeTime += int64(time.Since(start)) - waitTime
//         if !ok {
//             break
//         }
//         probeSideResult.Reset()
//         emptyProbeSideResult.chk = probeSideResult
//         self.probeChkResourceCh <- emptyProbeSideResult
//     }
// note joinResult.chk may be None when getNewJoinResult fails in loops
//     if joinResult == None {
//         return
//     } else if joinResult.err != None || (joinResult.chk != None && joinResult.chk.NumRows() > 0) {
//         self.HashJoinCtx.joinResultCh <- joinResult
//     } else if joinResult.chk != None && joinResult.chk.NumRows() == 0 {
//         self.joinChkResourceCh <- joinResult.chk
//     }
// }
// }
// joinMatchedProbeSideRow2ChunkForOuterHashJoin 对应 Go 声明 `func (w *ProbeWorkerV1) joinMatchedProbeSideRow2ChunkForOuterHashJoin(probeKey uint64, probeSideRow chunk.Row, hCtx *HashContext, joinResult *hashjoinWorkerResult) (bool, int64, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn joinMatchedProbeSideRow2ChunkForOuterHashJoin(&mut self, probeKey: u64, probeSideRow: chunk::Row, hCtx: Option<Box<HashContext>>, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     let mut err: errors::Error = Default::default()
//     waitTime = int64(0)
//     oneWaitTime = int64(0)
//     self.buildSideRows, self.buildSideRowPtrs, err = self.rowContainerForProbe.GetMatchedRowsAndPtrs(probeKey, probeSideRow, hCtx, self.buildSideRows, self.buildSideRowPtrs, true)
//     buildSideRows, rowsPtrs = self.buildSideRows, self.buildSideRowPtrs
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     if len(buildSideRows) == 0 {
//         return true, waitTime, joinResult
//     }
//     iter = self.rowIters
//     iter.Reset(buildSideRows)
//     let mut outerMatchStatus: Vec<outerRowStatusFlag> = Default::default()
//     rowIdx, ok = 0, false
//     for iter.Begin(); iter.Current() != iter.End(); {
//         outerMatchStatus, err = self.Joiner.TryToMatchOuters(iter, probeSideRow, joinResult.chk, outerMatchStatus)
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//         for i = range outerMatchStatus {
//             if outerMatchStatus[i] == outerRowMatched {
//                 self.HashJoinCtx.outerMatchedStatus[rowsPtrs[rowIdx+i].ChkIdx].Set(int(rowsPtrs[rowIdx+i].RowIdx))
//             }
//         }
//         rowIdx += len(outerMatchStatus)
//         if joinResult.chk.IsFull() {
//             ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//             waitTime += oneWaitTime
//             if !ok {
//                 return false, waitTime, joinResult
//             }
//         }
//     }
//     return true, waitTime, joinResult
// }
// }
// joinNAALOSJMatchProbeSideRow2Chunk implement the matching logic for NA-AntiLeftOuterSemiJoin
// joinNAALOSJMatchProbeSideRow2Chunk 对应 Go 声明 `func (w *ProbeWorkerV1) joinNAALOSJMatchProbeSideRow2Chunk(probeKey uint64, probeKeyNullBits *bitmap.ConcurrentBitmap, probeSideRow chunk.Row, hCtx *HashContext, joinResult *hashjoinWorkerResult) (bool, int64, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn joinNAALOSJMatchProbeSideRow2Chunk(&mut self, probeKey: u64, probeKeyNullBits: Option<Box<bitmap::ConcurrentBitmap>>, probeSideRow: chunk::Row, hCtx: Option<Box<HashContext>>, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     var (
//         err error
//         ok  bool
//     )
//     waitTime = int64(0)
//     oneWaitTime = int64(0)
//     if probeKeyNullBits == None {
// step1: match the same key bucket first.
// because AntiLeftOuterSemiJoin cares about the scalar value. If we both have a match from null
// bucket and same key bucket, we should return the result as <rhs-row, 0> from same-key bucket
// rather than <rhs-row, null> from null bucket.
//         self.buildSideRows, err = self.rowContainerForProbe.GetMatchedRows(probeKey, probeSideRow, hCtx, self.buildSideRows)
//         buildSideRows = self.buildSideRows
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//         if len(buildSideRows) != 0 {
//             iter1 = self.rowIters
//             iter1.Reset(buildSideRows)
//             for iter1.Begin(); iter1.Current() != iter1.End(); {
//                 matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter1, joinResult.chk, LeftNotNullRightNotNull)
//                 if err != None {
//                     joinResult.err = err
//                     return false, waitTime, joinResult
//                 }
// here matched means: there is a valid same-key bucket row from right side.
// as said in the comment, once we meet a same key (NOT IN semantic) in CNF, we can determine the result as <rhs, 0>.
//                 if matched {
//                     return true, waitTime, joinResult
//                 }
//                 if joinResult.chk.IsFull() {
//                     ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//                     waitTime += oneWaitTime
//                     if !ok {
//                         return false, waitTime, joinResult
//                     }
//                 }
//             }
//         }
// step2: match the null bucket secondly.
//         self.buildSideRows, err = self.rowContainerForProbe.GetNullBucketRows(hCtx, probeSideRow, probeKeyNullBits, self.buildSideRows, self.needCheckBuildColPos, self.needCheckProbeColPos, self.needCheckBuildTypes, self.needCheckProbeTypes)
//         buildSideRows = self.buildSideRows
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//         if len(buildSideRows) == 0 {
// when reach here, it means we couldn't find a valid same key match from same-key bucket yet
// and the null bucket is empty. so the result should be <rhs, 1>.
//             self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//             return true, waitTime, joinResult
//         }
//         iter2 = self.rowIters
//         iter2.Reset(buildSideRows)
//         for iter2.Begin(); iter2.Current() != iter2.End(); {
//             matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter2, joinResult.chk, LeftNotNullRightHasNull)
//             if err != None {
//                 joinResult.err = err
//                 return false, waitTime, joinResult
//             }
// here matched means: there is a valid null bucket row from right side.
// as said in the comment, once we meet a null in CNF, we can determine the result as <rhs, null>.
//             if matched {
//                 return true, waitTime, joinResult
//             }
//             if joinResult.chk.IsFull() {
//                 ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//                 waitTime += oneWaitTime
//                 if !ok {
//                     return false, waitTime, joinResult
//                 }
//             }
//         }
// step3: if we couldn't return it quickly in null bucket and same key bucket, here means two cases:
// case1: x NOT IN (empty set): if other key bucket don't have the valid rows yet.
// case2: x NOT IN (l,m,n...): if other key bucket do have the valid rows.
// both cases mean the result should be <rhs, 1>
//         self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//         return true, waitTime, joinResult
//     }
// when left side has null values, all we want is to find a valid build side rows (past other condition)
// so we can return it as soon as possible. here means two cases:
// case1: <?, null> NOT IN (empty set): ----------------------> result is <rhs, 1>.
// case2: <?, null> NOT IN (at least a valid inner row) ------------------> result is <rhs, null>.
// Step1: match null bucket (assumption that null bucket is quite smaller than all hash table bucket rows)
//     self.buildSideRows, err = self.rowContainerForProbe.GetNullBucketRows(hCtx, probeSideRow, probeKeyNullBits, self.buildSideRows, self.needCheckBuildColPos, self.needCheckProbeColPos, self.needCheckBuildTypes, self.needCheckProbeTypes)
//     buildSideRows = self.buildSideRows
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     if len(buildSideRows) != 0 {
//         iter1 = self.rowIters
//         iter1.Reset(buildSideRows)
//         for iter1.Begin(); iter1.Current() != iter1.End(); {
//             matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter1, joinResult.chk, LeftHasNullRightHasNull)
//             if err != None {
//                 joinResult.err = err
//                 return false, waitTime, joinResult
//             }
// here matched means: there is a valid null bucket row from right side. (not empty)
// as said in the comment, once we found at least a valid row, we can determine the result as <rhs, null>.
//             if matched {
//                 return true, waitTime, joinResult
//             }
//             if joinResult.chk.IsFull() {
//                 ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//                 waitTime += oneWaitTime
//                 if !ok {
//                     return false, waitTime, joinResult
//                 }
//             }
//         }
//     }
// Step2: match all hash table bucket build rows (use probeKeyNullBits to filter if any).
//     self.buildSideRows, err = self.rowContainerForProbe.GetAllMatchedRows(hCtx, probeSideRow, probeKeyNullBits, self.buildSideRows, self.needCheckBuildColPos, self.needCheckProbeColPos, self.needCheckBuildTypes, self.needCheckProbeTypes)
//     buildSideRows = self.buildSideRows
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     if len(buildSideRows) == 0 {
// when reach here, it means we couldn't return it quickly in null bucket, and same-bucket is empty,
// which means x NOT IN (empty set) or x NOT IN (l,m,n), the result should be <rhs, 1>
//         self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//         return true, waitTime, joinResult
//     }
//     iter2 = self.rowIters
//     iter2.Reset(buildSideRows)
//     for iter2.Begin(); iter2.Current() != iter2.End(); {
//         matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter2, joinResult.chk, LeftHasNullRightNotNull)
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
// here matched means: there is a valid same key bucket row from right side. (not empty)
// as said in the comment, once we found at least a valid row, we can determine the result as <rhs, null>.
//         if matched {
//             return true, waitTime, joinResult
//         }
//         if joinResult.chk.IsFull() {
//             ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//             waitTime += oneWaitTime
//             if !ok {
//                 return false, waitTime, joinResult
//             }
//         }
//     }
// step3: if we couldn't return it quickly in null bucket and all hash bucket, here means only one cases:
// case1: <?, null> NOT IN (empty set):
// empty set comes from no rows from all bucket can pass other condition. the result should be <rhs, 1>
//     self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//     return true, waitTime, joinResult
// }
// }
// joinNAASJMatchProbeSideRow2Chunk implement the matching logic for NA-AntiSemiJoin
// joinNAASJMatchProbeSideRow2Chunk 对应 Go 声明 `func (w *ProbeWorkerV1) joinNAASJMatchProbeSideRow2Chunk(probeKey uint64, probeKeyNullBits *bitmap.ConcurrentBitmap, probeSideRow chunk.Row, hCtx *HashContext, joinResult *hashjoinWorkerResult) (bool, int64, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn joinNAASJMatchProbeSideRow2Chunk(&mut self, probeKey: u64, probeKeyNullBits: Option<Box<bitmap::ConcurrentBitmap>>, probeSideRow: chunk::Row, hCtx: Option<Box<HashContext>>, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     var (
//         err error
//         ok  bool
//     )
//     waitTime = int64(0)
//     oneWaitTime = int64(0)
//     if probeKeyNullBits == None {
// step1: match null bucket first.
// need fetch the "valid" rows every time. (nullBits map check is necessary)
//         self.buildSideRows, err = self.rowContainerForProbe.GetNullBucketRows(hCtx, probeSideRow, probeKeyNullBits, self.buildSideRows, self.needCheckBuildColPos, self.needCheckProbeColPos, self.needCheckBuildTypes, self.needCheckProbeTypes)
//         buildSideRows = self.buildSideRows
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//         if len(buildSideRows) != 0 {
//             iter1 = self.rowIters
//             iter1.Reset(buildSideRows)
//             for iter1.Begin(); iter1.Current() != iter1.End(); {
//                 matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter1, joinResult.chk)
//                 if err != None {
//                     joinResult.err = err
//                     return false, waitTime, joinResult
//                 }
// here matched means: there is a valid null bucket row from right side.
// as said in the comment, once we meet a rhs null in CNF, we can determine the reject of lhs row.
//                 if matched {
//                     return true, waitTime, joinResult
//                 }
//                 if joinResult.chk.IsFull() {
//                     ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//                     waitTime += oneWaitTime
//                     if !ok {
//                         return false, waitTime, joinResult
//                     }
//                 }
//             }
//         }
// step2: then same key bucket.
//         self.buildSideRows, err = self.rowContainerForProbe.GetMatchedRows(probeKey, probeSideRow, hCtx, self.buildSideRows)
//         buildSideRows = self.buildSideRows
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//         if len(buildSideRows) == 0 {
// when reach here, it means we couldn't return it quickly in null bucket, and same-bucket is empty,
// which means x NOT IN (empty set), accept the rhs Row.
//             self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//             return true, waitTime, joinResult
//         }
//         iter2 = self.rowIters
//         iter2.Reset(buildSideRows)
//         for iter2.Begin(); iter2.Current() != iter2.End(); {
//             matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter2, joinResult.chk)
//             if err != None {
//                 joinResult.err = err
//                 return false, waitTime, joinResult
//             }
// here matched means: there is a valid same key bucket row from right side.
// as said in the comment, once we meet a false in CNF, we can determine the reject of lhs Row.
//             if matched {
//                 return true, waitTime, joinResult
//             }
//             if joinResult.chk.IsFull() {
//                 ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//                 waitTime += oneWaitTime
//                 if !ok {
//                     return false, waitTime, joinResult
//                 }
//             }
//         }
// step3: if we couldn't return it quickly in null bucket and same key bucket, here means two cases:
// case1: x NOT IN (empty set): if other key bucket don't have the valid rows yet.
// case2: x NOT IN (l,m,n...): if other key bucket do have the valid rows.
// both cases should accept the rhs row.
//         self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//         return true, waitTime, joinResult
//     }
// when left side has null values, all we want is to find a valid build side rows (passed from other condition)
// so we can return it as soon as possible. here means two cases:
// case1: <?, null> NOT IN (empty set): ----------------------> accept rhs row.
// case2: <?, null> NOT IN (at least a valid inner row) ------------------> unknown result, refuse rhs row.
// Step1: match null bucket (assumption that null bucket is quite smaller than all hash table bucket rows)
//     self.buildSideRows, err = self.rowContainerForProbe.GetNullBucketRows(hCtx, probeSideRow, probeKeyNullBits, self.buildSideRows, self.needCheckBuildColPos, self.needCheckProbeColPos, self.needCheckBuildTypes, self.needCheckProbeTypes)
//     buildSideRows = self.buildSideRows
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     if len(buildSideRows) != 0 {
//         iter1 = self.rowIters
//         iter1.Reset(buildSideRows)
//         for iter1.Begin(); iter1.Current() != iter1.End(); {
//             matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter1, joinResult.chk)
//             if err != None {
//                 joinResult.err = err
//                 return false, waitTime, joinResult
//             }
// here matched means: there is a valid null bucket row from right side. (not empty)
// as said in the comment, once we found at least a valid row, we can determine the reject of lhs row.
//             if matched {
//                 return true, waitTime, joinResult
//             }
//             if joinResult.chk.IsFull() {
//                 ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//                 waitTime += oneWaitTime
//                 if !ok {
//                     return false, waitTime, joinResult
//                 }
//             }
//         }
//     }
// Step2: match all hash table bucket build rows.
//     self.buildSideRows, err = self.rowContainerForProbe.GetAllMatchedRows(hCtx, probeSideRow, probeKeyNullBits, self.buildSideRows, self.needCheckBuildColPos, self.needCheckProbeColPos, self.needCheckBuildTypes, self.needCheckProbeTypes)
//     buildSideRows = self.buildSideRows
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     if len(buildSideRows) == 0 {
// when reach here, it means we couldn't return it quickly in null bucket, and same-bucket is empty,
// which means <?,null> NOT IN (empty set) or <?,null> NOT IN (no valid rows) accept the rhs row.
//         self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//         return true, waitTime, joinResult
//     }
//     iter2 = self.rowIters
//     iter2.Reset(buildSideRows)
//     for iter2.Begin(); iter2.Current() != iter2.End(); {
//         matched, _, err = self.Joiner.TryToMatchInners(probeSideRow, iter2, joinResult.chk)
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
// here matched means: there is a valid key row from right side. (not empty)
// as said in the comment, once we found at least a valid row, we can determine the reject of lhs row.
//         if matched {
//             return true, waitTime, joinResult
//         }
//         if joinResult.chk.IsFull() {
//             ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//             waitTime += oneWaitTime
//             if !ok {
//                 return false, waitTime, joinResult
//             }
//         }
//     }
// step3: if we couldn't return it quickly in null bucket and all hash bucket, here means only one cases:
// case1: <?, null> NOT IN (empty set):
// empty set comes from no rows from all bucket can pass other condition. we should accept the rhs row.
//     self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//     return true, waitTime, joinResult
// }
// }
// joinNAAJMatchProbeSideRow2Chunk implement the matching priority logic for NA-AntiSemiJoin and NA-AntiLeftOuterSemiJoin
// there are some bucket-matching priority difference between them.
//		Since NA-AntiSemiJoin don't need to append the scalar value with the left side row, there is a quick matching path.
//		1: lhs row has null:
//	       lhs row has null can't determine its result in advance, we should judge whether the right valid set is empty
//	       or not. For semantic like x NOT IN(y set), If y set is empty, the scalar result is 1; Otherwise, the result
//	       is 0. Since NA-AntiSemiJoin don't care about the scalar value, we just try to find a valid row from right side,
//	       once we found it then just return the left side row instantly. (same as NA-AntiLeftOuterSemiJoin)
//		2: lhs row without null:
//	       same-key bucket and null-bucket which should be the first to match? For semantic like x NOT IN(y set), once y
//	       set has a same key x, the scalar value is 0; else if y set has a null key, then the scalar value is null. Both
//	       of them lead the refuse of the lhs row without any difference. Since NA-AntiSemiJoin don't care about the scalar
//	       value, we can just match the null bucket first and refuse the lhs row as quickly as possible, because a null of
//	       yi in the CNF (x NA-EQ yi) can always determine a negative value (refuse lhs row) in advance here.
//	       For NA-AntiLeftOuterSemiJoin, we couldn't match null-bucket first, because once y set has a same key x and null
//	       key, we should return the result as left side row appended with a scalar value 0 which is from same key matching failure.
// joinNAAJMatchProbeSideRow2Chunk 对应 Go 声明 `func (w *ProbeWorkerV1) joinNAAJMatchProbeSideRow2Chunk(probeKey uint64, probeKeyNullBits *bitmap.ConcurrentBitmap, probeSideRow chunk.Row, hCtx *HashContext, joinResult *hashjoinWorkerResult) (bool, int64, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn joinNAAJMatchProbeSideRow2Chunk(&mut self, probeKey: u64, probeKeyNullBits: Option<Box<bitmap::ConcurrentBitmap>>, probeSideRow: chunk::Row, hCtx: Option<Box<HashContext>>, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     naAntiSemiJoin = self.HashJoinCtx.JoinType == base.AntiSemiJoin && self.HashJoinCtx.IsNullAware
//     naAntiLeftOuterSemiJoin = self.HashJoinCtx.JoinType == base.AntiLeftOuterSemiJoin && self.HashJoinCtx.IsNullAware
//     if naAntiSemiJoin {
//         return self.joinNAASJMatchProbeSideRow2Chunk(probeKey, probeKeyNullBits, probeSideRow, hCtx, joinResult)
//     }
//     if naAntiLeftOuterSemiJoin {
//         return self.joinNAALOSJMatchProbeSideRow2Chunk(probeKey, probeKeyNullBits, probeSideRow, hCtx, joinResult)
//     }
// shouldn't be here, not a valid NAAJ.
//     return false, 0, joinResult
// }
// }
// joinMatchedProbeSideRow2Chunk 对应 Go 声明 `func (w *ProbeWorkerV1) joinMatchedProbeSideRow2Chunk(probeKey uint64, probeSideRow chunk.Row, hCtx *HashContext, joinResult *hashjoinWorkerResult) (bool, int64, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn joinMatchedProbeSideRow2Chunk(&mut self, probeKey: u64, probeSideRow: chunk::Row, hCtx: Option<Box<HashContext>>, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     let mut err: errors::Error = Default::default()
//     waitTime = int64(0)
//     oneWaitTime = int64(0)
//     let mut buildSideRows: Vec<chunk::Row> = Default::default()
//     if self.Joiner.isSemiJoinWithoutCondition() {
//         let mut rowPtr: Option<Box<chunk::Row>> = Default::default()
//         rowPtr, err = self.rowContainerForProbe.GetOneMatchedRow(probeKey, probeSideRow, hCtx)
//         if rowPtr != None {
//             buildSideRows = append(buildSideRows, *rowPtr)
//         }
//     } else {
//         self.buildSideRows, err = self.rowContainerForProbe.GetMatchedRows(probeKey, probeSideRow, hCtx, self.buildSideRows)
//         buildSideRows = self.buildSideRows
//     }
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     if len(buildSideRows) == 0 {
//         self.Joiner.OnMissMatch(false, probeSideRow, joinResult.chk)
//         return true, waitTime, joinResult
//     }
//     iter = self.rowIters
//     iter.Reset(buildSideRows)
//     hasMatch, hasNull, ok = false, false, false
//     for iter.Begin(); iter.Current() != iter.End(); {
//         matched, isNull, err = self.Joiner.TryToMatchInners(probeSideRow, iter, joinResult.chk)
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//         hasMatch = hasMatch || matched
//         hasNull = hasNull || isNull
//         if joinResult.chk.IsFull() {
//             ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//             waitTime += oneWaitTime
//             if !ok {
//                 return false, waitTime, joinResult
//             }
//         }
//     }
//     if !hasMatch {
//         self.Joiner.OnMissMatch(hasNull, probeSideRow, joinResult.chk)
//     }
//     return true, waitTime, joinResult
// }
// }
// getNewJoinResult 对应 Go 声明 `func (w *ProbeWorkerV1) getNewJoinResult() (bool, *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
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
// join2Chunk 对应 Go 声明 `func (w *ProbeWorkerV1) join2Chunk(probeSideChk *chunk.Chunk, hCtx *HashContext, joinResult *hashjoinWorkerResult, selected []bool) (ok bool, waitTime int64, _ *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn join2Chunk(&mut self, probeSideChk: Option<Box<chunk::Chunk>>, hCtx: Option<Box<HashContext>>, joinResult: Option<Box<hashjoinWorkerResult>>, selected: Vec<bool>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     let mut err: errors::Error = Default::default()
//     waitTime = 0
//     oneWaitTime = int64(0)
//     selected, err = expression.VectorizedFilter(self.HashJoinCtx.SessCtx.GetExprCtx().GetEvalCtx(), self.HashJoinCtx.SessCtx.GetSessionVars().EnableVectorizedExpression, self.HashJoinCtx.OuterFilter, chunk.NewIterator4Chunk(probeSideChk), selected)
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     numRows = probeSideChk.NumRows()
//     hCtx.InitHash(numRows)
// By now, path 1 and 2 won't be conducted at the same time.
// 1: write the row data of join key to hashVals. (normal EQ key should ignore the null values.) null-EQ for Except statement is an exception.
//     for keyIdx, i = range hCtx.KeyColIdx {
//         ignoreNull = len(self.HashJoinCtx.IsNullEQ) > keyIdx && self.HashJoinCtx.IsNullEQ[keyIdx]
//         err = codec.HashChunkSelected(self.rowContainerForProbe.sc.TypeCtx(), hCtx.HashVals, probeSideChk, hCtx.AllTypes[keyIdx], i, hCtx.Buf, hCtx.HasNull, selected, ignoreNull)
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//     }
// 2: write the how data of NA join key to hashVals. (NA EQ key should collect all how including null value, store null value in a special position)
//     isNAAJ = len(hCtx.NaKeyColIdx) > 0
//     for keyIdx, i = range hCtx.NaKeyColIdx {
// NAAJ won't ignore any null values, but collect them up to probe.
//         err = codec.HashChunkSelected(self.rowContainerForProbe.sc.TypeCtx(), hCtx.HashVals, probeSideChk, hCtx.AllTypes[keyIdx], i, hCtx.Buf, hCtx.HasNull, selected, false)
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
// after fetch one NA column, collect the null value to null bitmap for every how. (use hasNull flag to accelerate)
// eg: if a NA Join cols is (a, b, c), for every build row here we maintained a 3-bit map to mark which column is null for them.
//         for rowIdx = range numRows {
//             if hCtx.HasNull[rowIdx] {
//                 hCtx.naColNullBitMap[rowIdx].UnsafeSet(keyIdx)
// clean and try fetch Next NA join col.
//                 hCtx.HasNull[rowIdx] = false
//                 hCtx.naHasNull[rowIdx] = true
//             }
//         }
//     }
//     err = self.HashJoinCtx.SessCtx.GetSessionVars().SQLKiller.HandleSignal()
//     failpoint.Inject("killedInJoin2Chunk", func(val failpoint.Value) {
//         if val.(bool) {
//             err = exeerrors.ErrQueryInterrupted
//         }
//     })
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     for i = range selected {
//         if isNAAJ {
//             if !selected[i] {
// since this is the case of using inner to build, so for an outer row unselected, we should fill the result when it's outer join.
//                 self.Joiner.OnMissMatch(false, probeSideChk.GetRow(i), joinResult.chk)
//             } else if hCtx.naHasNull[i] {
// here means the probe join connecting column has null value in it and this is special for matching all the hash buckets
// for it. (probeKey is not necessary here)
//                 probeRow = probeSideChk.GetRow(i)
//                 ok, oneWaitTime, joinResult = self.joinNAAJMatchProbeSideRow2Chunk(0, hCtx.naColNullBitMap[i].Clone(), probeRow, hCtx, joinResult)
//                 waitTime += oneWaitTime
//                 if !ok {
//                     return false, waitTime, joinResult
//                 }
//             } else {
// here means the probe join connecting column without null values, where we should match same key bucket and null bucket for it at its order.
// step1: process same key matched probe side rows
//                 probeKey, probeRow = hCtx.HashVals[i].Sum64(), probeSideChk.GetRow(i)
//                 ok, oneWaitTime, joinResult = self.joinNAAJMatchProbeSideRow2Chunk(probeKey, None, probeRow, hCtx, joinResult)
//                 waitTime += oneWaitTime
//                 if !ok {
//                     return false, waitTime, joinResult
//                 }
//             }
//         } else {
// since this is the case of using inner to build, so for an outer row unselected, we should fill the result when it's outer join.
//             if !selected[i] || hCtx.HasNull[i] { // process unmatched probe side rows
//                 self.Joiner.OnMissMatch(false, probeSideChk.GetRow(i), joinResult.chk)
//             } else { // process matched probe side rows
//                 probeKey, probeRow = hCtx.HashVals[i].Sum64(), probeSideChk.GetRow(i)
//                 ok, oneWaitTime, joinResult = self.joinMatchedProbeSideRow2Chunk(probeKey, probeRow, hCtx, joinResult)
//                 waitTime += oneWaitTime
//                 if !ok {
//                     return false, waitTime, joinResult
//                 }
//             }
//         }
//         if joinResult.chk.IsFull() {
//             ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//             waitTime += oneWaitTime
//             if !ok {
//                 return false, waitTime, joinResult
//             }
//         }
//     }
//     return true, waitTime, joinResult
// }
// }
// sendingResult 对应 Go 声明 `func (w *ProbeWorkerV1) sendingResult(joinResult *hashjoinWorkerResult) (ok bool, cost int64, newJoinResult *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn sendingResult(&mut self, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     start = time.Now()
//     self.HashJoinCtx.joinResultCh <- joinResult
//     ok, newJoinResult = self.getNewJoinResult()
//     cost = int64(time.Since(start))
//     return ok, cost, newJoinResult
// }
// }
// sendingResultAndCheckSignal 对应 Go 声明 `func (w *ProbeWorkerV1) sendingResultAndCheckSignal(joinResult *hashjoinWorkerResult) (ok bool, waitTime int64, newJoinResult *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn sendingResultAndCheckSignal(&mut self, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     ok, waitTime, newJoinResult = self.sendingResult(joinResult)
//     if !ok {
//         return false, waitTime, newJoinResult
//     }
//     if err = self.HashJoinCtx.SessCtx.GetSessionVars().SQLKiller.HandleSignal(); err != None {
//         newJoinResult.err = err
//         return false, waitTime, newJoinResult
//     }
//     return true, waitTime, newJoinResult
// }
// }
// join2ChunkForOuterHashJoin joins chunks when using the outer to build a hash table (refer to outer hash join)
// join2ChunkForOuterHashJoin 对应 Go 声明 `func (w *ProbeWorkerV1) join2ChunkForOuterHashJoin(probeSideChk *chunk.Chunk, hCtx *HashContext, joinResult *hashjoinWorkerResult) (ok bool, waitTime int64, _ *hashjoinWorkerResult) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl ProbeWorkerV1 {
//     pub fn join2ChunkForOuterHashJoin(&mut self, probeSideChk: Option<Box<chunk::Chunk>>, hCtx: Option<Box<HashContext>>, joinResult: Option<Box<hashjoinWorkerResult>>) -> (bool, i64, Option<Box<hashjoinWorkerResult>>) {
//     waitTime = 0
//     oneWaitTime = int64(0)
//     hCtx.InitHash(probeSideChk.NumRows())
//     for keyIdx, i = range hCtx.KeyColIdx {
//         err = codec.HashChunkColumns(self.rowContainerForProbe.sc.TypeCtx(), hCtx.HashVals, probeSideChk, hCtx.AllTypes[keyIdx], i, hCtx.Buf, hCtx.HasNull)
//         if err != None {
//             joinResult.err = err
//             return false, waitTime, joinResult
//         }
//     }
//     err = self.HashJoinCtx.SessCtx.GetSessionVars().SQLKiller.HandleSignal()
//     failpoint.Inject("killedInJoin2ChunkForOuterHashJoin", func(val failpoint.Value) {
//         if val.(bool) {
//             err = exeerrors.ErrQueryInterrupted
//         }
//     })
//     if err != None {
//         joinResult.err = err
//         return false, waitTime, joinResult
//     }
//     for i = range probeSideChk.NumRows() {
//         probeKey, probeRow = hCtx.HashVals[i].Sum64(), probeSideChk.GetRow(i)
//         ok, oneWaitTime, joinResult = self.joinMatchedProbeSideRow2ChunkForOuterHashJoin(probeKey, probeRow, hCtx, joinResult)
//         waitTime += oneWaitTime
//         if !ok {
//             return false, waitTime, joinResult
//         }
//         if joinResult.chk.IsFull() {
//             ok, oneWaitTime, joinResult = self.sendingResultAndCheckSignal(joinResult)
//             waitTime += oneWaitTime
//             if !ok {
//                 return false, waitTime, joinResult
//             }
//         }
//     }
//     return true, waitTime, joinResult
// }
// }
// Next implements the Executor Next interface.
// hash join constructs the result following these steps:
// step 1. fetch data from build side child and build a hash table;
// step 2. fetch data from probe child in a background goroutine and probe the hash table in multiple join workers.
// Next 对应 Go 声明 `func (e *HashJoinV1Exec) Next(ctx context.Context, req *chunk.Chunk) (err error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn Next(&mut self, ctx: context::Context, req: Option<Box<chunk::Chunk>>) -> (errors::Error) {
//     if !self.Prepared {
//         self.buildFinished = make(chan error, 1)
//         hCtx = &HashContext{
//             AllTypes:    self.BuildTypes,
//             KeyColIdx:   self.BuildWorker.BuildKeyColIdx,
//             NaKeyColIdx: self.BuildWorker.BuildNAKeyColIdx,
//         }
//         self.RowContainer = newHashRowContainer(self.Ctx(), hCtx, exec.RetTypes(self.BuildWorker.BuildSideExec))
// we shallow copies RowContainer for each probe worker to avoid lock contention
//         for i = range self.Concurrency {
//             if i == 0 {
//                 self.ProbeWorkers[i].rowContainerForProbe = self.RowContainer
//             } else {
//                 self.ProbeWorkers[i].rowContainerForProbe = self.RowContainer.ShallowCopy()
//             }
//         }
//         for i = range self.Concurrency {
//             self.ProbeWorkers[i].rowIters = chunk.NewIterator4Slice([]chunk.Row{})
//         }
//         self.workerWg.RunWithRecover(func() {
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//             defer trace.StartRegion(ctx, "HashJoinHashTableBuilder").End()
//             self.fetchAndBuildHashTable(ctx)
//         }, self.handleFetchAndBuildHashTablePanic)
//         self.fetchAndProbeHashTable(ctx)
//         self.Prepared = true
//     }
//     if self.IsOuterJoin {
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
// handleFetchAndBuildHashTablePanic 对应 Go 声明 `func (e *HashJoinV1Exec) handleFetchAndBuildHashTablePanic(r any) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn handleFetchAndBuildHashTablePanic(&mut self, r: Box<dyn Any>) {
//     if r != None {
//         self.buildFinished <- util.GetRecoverError(r)
//     }
//     close(self.buildFinished)
// }
// }
// fetchAndBuildHashTable 对应 Go 声明 `func (e *HashJoinV1Exec) fetchAndBuildHashTable(ctx context.Context) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashJoinV1Exec {
//     pub fn fetchAndBuildHashTable(&mut self, ctx: context::Context) {
//     if self.stats != None {
//         start = time.Now()
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer func() {
//             self.stats.fetchAndBuildHashTable = time.Since(start)
//         }()
//     }
// buildSideResultCh transfers build side chunk from build side fetch to build hash table.
//     buildSideResultCh = make(chan *chunk.Chunk, 1)
//     doneCh = make(chan struct{})
//     fetchBuildSideRowsOk = make(chan error, 1)
//     self.workerWg.RunWithRecover(
//         func() {
//             defer trace.StartRegion(ctx, "HashJoinBuildSideFetcher").End()
//             self.BuildWorker.fetchBuildSideRows(ctx, &self.BuildWorker.HashJoinCtx.hashJoinCtxBase, None, None, buildSideResultCh, fetchBuildSideRowsOk, doneCh)
//         },
//         func(r any) {
//             if r != None {
//                 fetchBuildSideRowsOk <- util.GetRecoverError(r)
//             }
//             close(fetchBuildSideRowsOk)
//         },
//     )
// TODO: Parallel build hash table. Currently not support because `unsafeHashTable` is not thread-safe.
//     err = self.BuildWorker.BuildHashTableForList(buildSideResultCh)
//     if err != None {
//         self.buildFinished <- errors.Trace(err)
//         close(doneCh)
//     }
// Wait fetchBuildSideRows be Finished.
// 1. if BuildHashTableForList fails
// 2. if probeSideResult.NumRows() == 0, fetchProbeSideChunks will not wait for the build side.
// channel 工具调用保留 Go 的队列清理/发送语义。
//     channel.Clear(buildSideResultCh)
// Check whether err is None to avoid sending redundant error into buildFinished.
//     if err == None {
//         if err = <-fetchBuildSideRowsOk; err != None {
//             self.buildFinished <- err
//         }
//     }
// }
// }
// BuildHashTableForList builds hash table from `list`.
// BuildHashTableForList 对应 Go 声明 `func (w *BuildWorkerV1) BuildHashTableForList(buildSideResultCh <-chan *chunk.Chunk) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl BuildWorkerV1 {
//     pub fn BuildHashTableForList(&mut self, buildSideResultCh <-chan: Option<Box<chunk::Chunk>>) -> Result<(), errors::Error> {
//     let mut err: errors::Error = Default::default()
//     let mut selected: Vec<bool> = Default::default()
//     rowContainer = self.HashJoinCtx.RowContainer
//     rowContainer.GetMemTracker().AttachTo(self.HashJoinCtx.memTracker)
//     rowContainer.GetMemTracker().SetLabel(memory.LabelForBuildSideResult)
//     rowContainer.GetDiskTracker().AttachTo(self.HashJoinCtx.diskTracker)
//     rowContainer.GetDiskTracker().SetLabel(memory.LabelForBuildSideResult)
//     if vardef.EnableTmpStorageOnOOM.Load() {
//         actionSpill = rowContainer.ActionSpill()
//         failpoint.Inject("testRowContainerSpill", func(val failpoint.Value) {
//             if val.(bool) {
//                 actionSpill = rowContainer.rowContainer.ActionSpillForTest()
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//                 defer actionSpill.(*chunk.SpillDiskAction).WaitForTest()
//             }
//         })
//         self.HashJoinCtx.SessCtx.GetSessionVars().MemTracker.FallbackOldAndSetNewAction(actionSpill)
//     }
//     for chk = range buildSideResultCh {
//         if self.HashJoinCtx.finished.Load() {
//             return None
//         }
//         if !self.HashJoinCtx.UseOuterToBuild {
//             err = rowContainer.PutChunk(chk, self.HashJoinCtx.IsNullEQ)
//         } else {
//             let mut bitMap: = bitmap::NewConcurrentBitmap(chk::NumRows()) = Default::default()
//             self.HashJoinCtx.outerMatchedStatus = append(self.HashJoinCtx.outerMatchedStatus, bitMap)
//             self.HashJoinCtx.memTracker.Consume(bitMap.BytesConsumed())
//             if len(self.HashJoinCtx.OuterFilter) == 0 {
//                 err = self.HashJoinCtx.RowContainer.PutChunk(chk, self.HashJoinCtx.IsNullEQ)
//             } else {
//                 selected, err = expression.VectorizedFilter(self.HashJoinCtx.SessCtx.GetExprCtx().GetEvalCtx(), self.HashJoinCtx.SessCtx.GetSessionVars().EnableVectorizedExpression, self.HashJoinCtx.OuterFilter, chunk.NewIterator4Chunk(chk), selected)
//                 if err != None {
//                     return err
//                 }
//                 err = rowContainer.PutChunkSelected(chk, selected, self.HashJoinCtx.IsNullEQ)
//             }
//         }
//         failpoint.Inject("ConsumeRandomPanic", None)
//         if err != None {
//             return err
//         }
//     }
//     return None
// }
// }
// NestedLoopApplyExec is the executor for apply.
// NestedLoopApplyExec 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct NestedLoopApplyExec {
//     pub BaseExecutor: exec::BaseExecutor,
//     pub Sctx: sessionctx::Context,
//     pub innerRows: Vec<chunk::Row>,
//     pub cursor: i32,
//     pub InnerExec: exec::Executor,
//     pub OuterExec: exec::Executor,
//     pub InnerFilter: expression::CNFExprs,
//     pub OuterFilter: expression::CNFExprs,
//     pub Joiner: Joiner,
//     pub cache: Option<Box<applycache::ApplyCache>>,
//     pub CanUseCache: bool,
//     pub cacheHitCounter: i32,
//     pub cacheAccessCounter: i32,
//     pub OuterSchema: Vec<Option<Box<expression::CorrelatedColumn>>>,
//     pub OuterChunk: Option<Box<chunk::Chunk>>,
//     pub outerChunkCursor: i32,
//     pub outerSelected: Vec<bool>,
//     pub InnerList: Option<Box<chunk::List>>,
//     pub InnerChunk: Option<Box<chunk::Chunk>>,
//     pub innerSelected: Vec<bool>,
//     pub innerIter: chunk::Iterator,
//     pub outerRow: Option<Box<chunk::Row>>,
//     pub hasMatch: bool,
//     pub hasNull: bool,
//     pub Outer: bool,
//     pub memTracker *memory.Tracker // track memory: usage::,
// }
// Close implements the Executor interface.
// Close 对应 Go 声明 `func (e *NestedLoopApplyExec) Close() error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl NestedLoopApplyExec {
//     pub fn Close(&mut self) -> Result<(), errors::Error> {
//     self.innerRows = None
//     self.memTracker = None
//     if self.RuntimeStats() != None {
//         runtimeStats = NewJoinRuntimeStats()
//         if self.CanUseCache {
//             let mut hitRatio: f64 = Default::default()
//             if self.cacheAccessCounter > 0 {
//                 hitRatio = float64(self.cacheHitCounter) / float64(self.cacheAccessCounter)
//             }
//             runtimeStats.SetCacheInfo(true, hitRatio)
//         } else {
//             runtimeStats.SetCacheInfo(false, 0)
//         }
//         runtimeStats.SetConcurrencyInfo(execdetails.NewConcurrencyInfo("concurrency", 0))
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//         defer self.Ctx().GetSessionVars().StmtCtx.RuntimeStatsColl.RegisterStats(self.ID(), runtimeStats)
//     }
//     return exec.Close(self.OuterExec)
// }
// }
// Open implements the Executor interface.
// Open 对应 Go 声明 `func (e *NestedLoopApplyExec) Open(ctx context.Context) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl NestedLoopApplyExec {
//     pub fn Open(&mut self, ctx: context::Context) -> Result<(), errors::Error> {
//     err = exec.Open(ctx, self.OuterExec)
//     if err != None {
//         return err
//     }
//     self.cursor = 0
//     self.innerRows = self.innerRows[:0]
//     self.OuterChunk = exec.TryNewCacheChunk(self.OuterExec)
//     self.InnerChunk = exec.TryNewCacheChunk(self.InnerExec)
//     self.InnerList = chunk.NewList(exec.RetTypes(self.InnerExec), self.InitCap(), self.MaxChunkSize())
//     self.memTracker = memory.NewTracker(self.ID(), -1)
//     self.memTracker.AttachTo(self.Ctx().GetSessionVars().StmtCtx.MemTracker)
//     self.InnerList.GetMemTracker().SetLabel(memory.LabelForInnerList)
//     self.InnerList.GetMemTracker().AttachTo(self.memTracker)
//     if self.CanUseCache {
//         self.cache, err = applycache.NewApplyCache(self.Sctx)
//         if err != None {
//             return err
//         }
//         self.cacheHitCounter = 0
//         self.cacheAccessCounter = 0
//         self.cache.GetMemTracker().AttachTo(self.memTracker)
//     }
//     return None
// }
// }
// aggExecutorTreeInputEmpty checks whether the executor tree returns empty if without aggregate operators.
// Note that, the prerequisite is that this executor tree has been executed already and it returns one Row.
// aggExecutorTreeInputEmpty 对应 Go 声明 `func aggExecutorTreeInputEmpty(e exec.Executor) bool {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn aggExecutorTreeInputEmpty(e: exec::Executor) -> bool {
//     children = e.AllChildren()
//     if len(children) == 0 {
//         return false
//     }
//     if len(children) > 1 {
//         _, ok = e.(*unionexec.UnionExec)
//         if !ok {
// It is a Join executor.
//             return false
//         }
//         for _, child = range children {
//             if !aggExecutorTreeInputEmpty(child) {
//                 return false
//             }
//         }
//         return true
//     }
// Single child executors.
//     if aggExecutorTreeInputEmpty(children[0]) {
//         return true
//     }
//     if hashAgg, ok = e.(*aggregate.HashAggExec); ok {
//         return hashAgg.IsChildReturnEmpty
//     }
//     if streamAgg, ok = e.(*aggregate.StreamAggExec); ok {
//         return streamAgg.IsChildReturnEmpty
//     }
//     return false
// }
// fetchSelectedOuterRow 对应 Go 声明 `func (e *NestedLoopApplyExec) fetchSelectedOuterRow(ctx context.Context, chk *chunk.Chunk) (*chunk.Row, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl NestedLoopApplyExec {
//     pub fn fetchSelectedOuterRow(&mut self, ctx: context::Context, chk: Option<Box<chunk::Chunk>>) -> (Option<Box<chunk::Row>>, errors::Error) {
//     outerIter = chunk.NewIterator4Chunk(self.OuterChunk)
//     for {
//         if self.outerChunkCursor >= self.OuterChunk.NumRows() {
//             err = exec.Next(ctx, self.OuterExec, self.OuterChunk)
//             if err != None {
//                 return None, err
//             }
//             if self.OuterChunk.NumRows() == 0 {
//                 return None, None
//             }
//             self.outerSelected, err = expression.VectorizedFilter(self.Sctx.GetExprCtx().GetEvalCtx(), self.Sctx.GetSessionVars().EnableVectorizedExpression, self.OuterFilter, outerIter, self.outerSelected)
//             if err != None {
//                 return None, err
//             }
// Go select 同时监听 channel/context；保留分支结构，后续需替换为异步 select。
// For cases like `select count(1), (select count(1) from s where s.a > t.a) as sub from t where t.a = 1`,
// if outer child has no row satisfying `t.a = 1`, `sub` should be `null` instead of `0` theoretically; however, the
// outer `count(1)` produces one row <0, null> over the empty input, we should specially mark this outer row
// as not selected, to trigger the mismatch join procedure.
//             if self.outerChunkCursor == 0 && self.OuterChunk.NumRows() == 1 && self.outerSelected[0] && aggExecutorTreeInputEmpty(self.OuterExec) {
//                 self.outerSelected[0] = false
//             }
//             self.outerChunkCursor = 0
//         }
//         outerRow = self.OuterChunk.GetRow(self.outerChunkCursor)
//         selected = self.outerSelected[self.outerChunkCursor]
//         self.outerChunkCursor++
//         if selected {
//             return &outerRow, None
//         } else if self.Outer {
//             self.Joiner.OnMissMatch(false, outerRow, chk)
//             if chk.IsFull() {
//                 return None, None
//             }
//         }
//     }
// }
// }
// fetchAllInners reads all data from the inner table and stores them in a List.
// fetchAllInners 对应 Go 声明 `func (e *NestedLoopApplyExec) fetchAllInners(ctx context.Context) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl NestedLoopApplyExec {
//     pub fn fetchAllInners(&mut self, ctx: context::Context) -> Result<(), errors::Error> {
//     err = exec.Open(ctx, self.InnerExec)
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() { terror.Log(exec.Close(self.InnerExec)) }()
//     if err != None {
//         return err
//     }
//     if self.CanUseCache {
// create a new one since it may be in the cache
//         self.InnerList = chunk.NewListWithMemTracker(exec.RetTypes(self.InnerExec), self.InitCap(), self.MaxChunkSize(), self.InnerList.GetMemTracker())
//     } else {
//         self.InnerList.Reset()
//     }
//     innerIter = chunk.NewIterator4Chunk(self.InnerChunk)
//     for {
//         err = exec.Next(ctx, self.InnerExec, self.InnerChunk)
//         if err != None {
//             return err
//         }
//         if self.InnerChunk.NumRows() == 0 {
//             return None
//         }
//         self.innerSelected, err = expression.VectorizedFilter(self.Sctx.GetExprCtx().GetEvalCtx(), self.Sctx.GetSessionVars().EnableVectorizedExpression, self.InnerFilter, innerIter, self.innerSelected)
//         if err != None {
//             return err
//         }
//         for row = innerIter.Begin(); row != innerIter.End(); row = innerIter.Next() {
//             if self.innerSelected[row.Idx()] {
//                 self.InnerList.AppendRow(row)
//             }
//         }
//     }
// }
// }
// Next implements the Executor interface.
// Next 对应 Go 声明 `func (e *NestedLoopApplyExec) Next(ctx context.Context, req *chunk.Chunk) (err error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl NestedLoopApplyExec {
//     pub fn Next(&mut self, ctx: context::Context, req: Option<Box<chunk::Chunk>>) -> (errors::Error) {
//     req.Reset()
//     for {
//         if self.innerIter == None || self.innerIter.Current() == self.innerIter.End() {
//             if self.outerRow != None && !self.hasMatch {
//                 self.Joiner.OnMissMatch(self.hasNull, *self.outerRow, req)
//             }
//             self.outerRow, err = self.fetchSelectedOuterRow(ctx, req)
//             if self.outerRow == None || err != None {
//                 return err
//             }
//             self.hasMatch = false
//             self.hasNull = false
//             if self.CanUseCache {
//                 let mut key: Vec<u8> = Default::default()
//                 for _, col = range self.OuterSchema {
//                     *col.Data = self.outerRow.GetDatum(col.Index, col.RetType)
//                     key, err = codec.EncodeKey(self.Ctx().GetSessionVars().StmtCtx.TimeZone(), key, *col.Data)
//                     err = self.Ctx().GetSessionVars().StmtCtx.HandleError(err)
//                     if err != None {
//                         return err
//                     }
//                 }
//                 self.cacheAccessCounter++
//                 value, err = self.cache.Get(key)
//                 if err != None {
//                     return err
//                 }
//                 if value != None {
//                     self.InnerList = value
//                     self.cacheHitCounter++
//                 } else {
//                     err = self.fetchAllInners(ctx)
//                     if err != None {
//                         return err
//                     }
//                     if _, err = self.cache.Set(key, self.InnerList); err != None {
//                         return err
//                     }
//                 }
//             } else {
//                 for _, col = range self.OuterSchema {
//                     *col.Data = self.outerRow.GetDatum(col.Index, col.RetType)
//                 }
//                 err = self.fetchAllInners(ctx)
//                 if err != None {
//                     return err
//                 }
//             }
//             self.innerIter = chunk.NewIterator4List(self.InnerList)
//             self.innerIter.Begin()
//         }
//         matched, isNull, err = self.Joiner.TryToMatchInners(*self.outerRow, self.innerIter, req)
//         self.hasMatch = self.hasMatch || matched
//         self.hasNull = self.hasNull || isNull
//         if err != None || req.IsFull() {
//             return err
//         }
//     }
// }
// }
// cacheInfo is used to save the concurrency information of the executor operator
// cacheInfo 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct cacheInfo {
//     pub hitRatio: f64,
//     pub useCache: bool,
// }
// joinRuntimeStats 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct joinRuntimeStats {
//     pub RuntimeStatsWithConcurrencyInfo: Option<Box<execdetails::RuntimeStatsWithConcurrencyInfo>>,
//     pub applyCache: bool,
//     pub cache: cacheInfo,
//     pub hasHashStat: bool,
//     pub hashStat: hashStatistic,
// }
// NewJoinRuntimeStats returns a new joinRuntimeStats
// NewJoinRuntimeStats 对应 Go 声明 `func NewJoinRuntimeStats() *joinRuntimeStats {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn NewJoinRuntimeStats() -> Option<Box<joinRuntimeStats>> {
//     stats = &joinRuntimeStats{
//         RuntimeStatsWithConcurrencyInfo: &execdetails.RuntimeStatsWithConcurrencyInfo{},
//     }
//     return stats
// }
// SetCacheInfo sets the cache information. Only used for apply executor.
// SetCacheInfo 对应 Go 声明 `func (e *joinRuntimeStats) SetCacheInfo(useCache bool, hitRatio float64) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl joinRuntimeStats {
//     pub fn SetCacheInfo(&mut self, useCache: bool, hitRatio: f64) {
//     self.Lock()
//     self.applyCache = true
//     self.cache.useCache = useCache
//     self.cache.hitRatio = hitRatio
//     self.Unlock()
// }
// }
// */
use crate::hash_join_base::{
    BuildWorkerBase, HashJoinContextBase, HashJoinWorkerResult, ProbeSideTupleFetcherBase,
    ProbeWorkerBase,
};
use crate::hash_join_stats::{HashJoinRuntimeStats, HashStatistic};
use crate::hash_table_v1::{HashRowContainer, RowPointer};
use crate::joiner::{JoinType, Joiner, NaajType, OuterRowStatus, Row};
use crate::row_table_builder::{Chunk, Value};
use std::collections::HashMap;
use std::time::Instant;

/// 执行器生命周期状态：未打开 / 已打开 / 已关闭。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExecutorState {
    #[default]
    Created,
    Open,
    Exhausted,
    Closed,
}

/// Hash Join v1 上下文：连接类型、键列、null-aware、并发与 chunk 大小。

#[derive(Clone)]
pub struct HashJoinCtxV1 {
    pub base: HashJoinContextBase,
    pub join_type: JoinType,
    pub build_key_indices: Vec<usize>,
    pub probe_key_indices: Vec<usize>,
    pub null_aware: bool,
    pub build_side_is_outer: bool,
    pub concurrency: usize,
    pub max_chunk_size: usize,
}

impl HashJoinCtxV1 {
    /// 校验键列数量与并发/chunk 配置合法。
    pub fn validate(&self) -> Result<(), String> {
        if self.build_key_indices.len() != self.probe_key_indices.len() {
            return Err("build and probe key counts differ".into());
        }
        if self.build_key_indices.is_empty() {
            return Err("hash join requires at least one key".into());
        }
        if self.concurrency == 0 || self.max_chunk_size == 0 {
            return Err("concurrency and max chunk size must be positive".into());
        }
        if self.null_aware
            && !matches!(
                self.join_type,
                JoinType::AntiSemi | JoinType::AntiLeftOuterSemi
            )
        {
            return Err("null-aware hash join requires an anti join".into());
        }
        Ok(())
    }
}

/// v1 探测侧取数器，包装基类取数逻辑。

pub struct ProbeSideTupleFetcherV1 {
    pub base: ProbeSideTupleFetcherBase,
}
impl ProbeSideTupleFetcherV1 {
    /// 构造实例。
    pub fn new(chunks: Vec<Chunk>) -> Self {
        Self {
            base: ProbeSideTupleFetcherBase::new(chunks),
        }
    }
}

/// v1 探测 worker：对探测行做哈希查找并交给 Joiner 产出结果。

#[derive(Clone)]
pub struct ProbeWorkerV1 {
    pub base: ProbeWorkerBase,
}
impl ProbeWorkerV1 {
    /// 构造实例。
    pub fn new(id: usize, context: HashJoinContextBase) -> Self {
        Self {
            base: ProbeWorkerBase::new(id, context),
        }
    }
    /// 对单行探测：查找匹配构建行并应用 join 语义。
    fn join_probe_row(
        &self,
        ctx: &HashJoinCtxV1,
        table: &mut HashRowContainer,
        joiner: &Joiner,
        probe: &Row,
        output: &mut Vec<Row>,
    ) -> Result<(), String> {
        let pointers = if ctx.null_aware {
            table.get_na_rows_by_indices(probe, &ctx.probe_key_indices)?
        } else {
            table.get_matched_rows_by_indices(probe, &ctx.probe_key_indices)?
        };
        let build_rows: Vec<Row> = pointers
            .iter()
            .filter_map(|pointer| table.row(*pointer).cloned())
            .collect();
        if ctx.build_side_is_outer {
            let statuses = joiner.try_to_match_outers(&build_rows, probe, output)?;
            for (pointer, status) in pointers.into_iter().zip(statuses) {
                if status == OuterRowStatus::Matched {
                    table.mark_used(pointer);
                }
            }
            return Ok(());
        }
        let naaj = classify_naaj(table, probe, &ctx.probe_key_indices, &pointers);
        let result = joiner.try_to_match_inners(probe, &build_rows, output, naaj)?;
        if result.matched {
            for pointer in pointers {
                table.mark_used(pointer);
            }
        }
        if !result.matched {
            joiner.on_miss_match(result.has_null || naaj_has_null(naaj), probe, output);
        }
        Ok(())
    }
}

/// v1 构建 worker：把构建侧 chunk 灌入哈希行容器。

#[derive(Clone)]
pub struct BuildWorkerV1 {
    pub base: BuildWorkerBase,
}
impl BuildWorkerV1 {
    /// 构造实例。
    pub fn new(id: usize, context: HashJoinContextBase, memory_limit: Option<i64>) -> Self {
        Self {
            base: BuildWorkerBase::new(id, context, memory_limit),
        }
    }
    /// 执行构建：写入哈希表并标记共享上下文完成或失败。
    pub fn build(&self, ctx: &HashJoinCtxV1, chunks: &[Chunk]) -> Result<HashRowContainer, String> {
        self.base.run_guarded(|| {
            let rows = self.base.fetch_build_side_rows(chunks)?;
            let mut container = HashRowContainer::new(
                ctx.build_key_indices.clone(),
                ctx.concurrency > 1,
                rows.len(),
            );
            container.put_chunk(rows)?;
            let bytes = container.memory_bytes();
            self.base.check_and_spill_row_table_if_needed(bytes, || {
                container.spill();
                Ok(())
            })?;
            Ok(container)
        })
    }
}

/// Hash Join v1 执行器：open/build/probe/next 驱动整个连接。

pub struct HashJoinV1Exec {
    pub context: HashJoinCtxV1,
    pub joiner: Joiner,
    build_chunks: Vec<Chunk>,
    probe_chunks: Vec<Chunk>,
    table: Option<HashRowContainer>,
    output: Vec<Row>,
    cursor: usize,
    state: ExecutorState,
    memory_limit: Option<i64>,
    pub stats: HashJoinRuntimeStats,
}

impl HashJoinV1Exec {
    /// 构造实例。
    pub fn new(
        context: HashJoinCtxV1,
        joiner: Joiner,
        build_chunks: Vec<Chunk>,
        probe_chunks: Vec<Chunk>,
    ) -> Result<Self, String> {
        context.validate()?;
        if context.join_type != joiner.join_type() {
            return Err("joiner type differs from hash join context".into());
        }
        Ok(Self {
            context,
            joiner,
            build_chunks,
            probe_chunks,
            table: None,
            output: Vec::new(),
            cursor: 0,
            state: ExecutorState::Created,
            memory_limit: None,
            stats: HashJoinRuntimeStats::default(),
        })
    }
    /// 设置内存限额（供 spill 判定）。
    pub fn SetMemoryLimit(&mut self, memory_limit: Option<i64>) {
        self.memory_limit = memory_limit;
    }
    /// 是否已触发 spill。
    pub fn IsSpillTriggered(&self) -> bool {
        self.context.base.is_spilled()
            || self
                .table
                .as_ref()
                .is_some_and(HashRowContainer::already_spilled)
    }
    /// 已计入磁盘的字节数。
    pub fn DiskBytes(&self) -> i64 {
        self.table.as_ref().map_or(0, HashRowContainer::disk_bytes)
    }
    /// 当前执行器状态。
    pub fn state(&self) -> ExecutorState {
        self.state
    }
    /// 打开执行器：重置状态并构建哈希表。
    pub fn open(&mut self) -> Result<(), String> {
        if self.state == ExecutorState::Open {
            return Ok(());
        }
        self.context.base.reset();
        self.output.clear();
        self.cursor = 0;
        let start = Instant::now();
        let worker = BuildWorkerV1::new(0, self.context.base.clone(), self.memory_limit);
        match worker.build(&self.context, &self.build_chunks) {
            Ok(table) => {
                self.table = Some(table);
                self.context.base.finish_build();
            }
            Err(error) => {
                self.context.base.fail(error.clone());
                return Err(error);
            }
        }
        self.stats.fetch_and_build += start.elapsed();
        self.stats.concurrency = self.context.concurrency;
        self.state = ExecutorState::Open;
        Ok(())
    }
    /// 处理一块探测数据，产出全部连接结果行。
    fn produce_all(&mut self) -> Result<(), String> {
        // Probe 前必须等待构建侧哈希表就绪。
        self.context.base.wait_for_build_side()?;
        let start = Instant::now();
        let mut fetcher = ProbeSideTupleFetcherV1::new(self.probe_chunks.clone());
        let worker = ProbeWorkerV1::new(0, self.context.base.clone());
        let table = self
            .table
            .as_mut()
            .ok_or_else(|| "hash table is not built".to_string())?;
        while let Some(resource) = fetcher.base.fetch_next(&self.context.base)? {
            worker.base.run_guarded(|| {
                for probe in &resource.chunk {
                    worker.join_probe_row(
                        &self.context,
                        table,
                        &self.joiner,
                        probe,
                        &mut self.output,
                    )?;
                }
                Ok(())
            })?;
            fetcher.base.recycle(resource);
        }
        if self.context.build_side_is_outer
            && matches!(
                self.context.join_type,
                JoinType::LeftOuter | JoinType::RightOuter
            )
        {
            for build in table.unmatched_rows() {
                self.joiner.on_miss_match(false, build, &mut self.output);
            }
        }
        self.stats.fetch_and_probe += start.elapsed();
        self.stats.probe += start.elapsed();
        Ok(())
    }
    /// 拉取下一批连接结果；空行表示结束。
    pub fn next(&mut self) -> Result<HashJoinWorkerResult, String> {
        if self.state == ExecutorState::Created {
            self.open()?;
        }
        if self.state == ExecutorState::Closed {
            return Err("hash join is closed".into());
        }
        if self.output.is_empty() && self.cursor == 0 {
            self.produce_all()?;
        }
        if self.cursor >= self.output.len() {
            self.state = ExecutorState::Exhausted;
            return Ok(HashJoinWorkerResult::default());
        }
        let end = (self.cursor + self.context.max_chunk_size).min(self.output.len());
        let result = self.output[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(HashJoinWorkerResult {
            rows: result,
            error: None,
        })
    }
    /// 一次性执行并收集全部结果行。
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
    /// 关闭并释放资源。
    pub fn close(&mut self) {
        self.context.base.cancel();
        if let Some(table) = self.table.as_mut() {
            table.close();
        }
        self.table = None;
        self.output.clear();
        self.state = ExecutorState::Closed;
    }
}

/// 相关子查询相关键：一行值向量。

pub type CorrelatedKey = Vec<Value>;
/// Nested Loop Apply 内表构建回调。
pub type InnerBuilder = Box<dyn Fn(&Row) -> Result<Vec<Row>, String> + Send + Sync>;

/// Apply 缓存命中信息。

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CacheInfo {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

/// Nested Loop Apply 执行器（相关子查询按外表行驱动内表）。

pub struct NestedLoopApplyExec {
    outer_rows: Vec<Row>,
    inner_builder: InnerBuilder,
    joiner: Joiner,
    correlated_indices: Vec<usize>,
    cache: HashMap<Vec<u8>, Vec<Row>>,
    cursor: usize,
    output: Vec<Row>,
    pub cache_info: CacheInfo,
    pub hash_stat: HashStatistic,
}

impl NestedLoopApplyExec {
    /// 构造实例。
    pub fn new(
        outer_rows: Vec<Row>,
        inner_builder: InnerBuilder,
        joiner: Joiner,
        correlated_indices: Vec<usize>,
    ) -> Self {
        Self {
            outer_rows,
            inner_builder,
            joiner,
            correlated_indices,
            cache: HashMap::new(),
            cursor: 0,
            output: Vec::new(),
            cache_info: CacheInfo::default(),
            hash_stat: HashStatistic::default(),
        }
    }
    /// 打开执行器：重置状态并构建哈希表。
    pub fn open(&mut self) {
        self.cursor = 0;
        self.output.clear();
        self.cache.clear();
        self.cache_info = CacheInfo::default();
    }
    /// 拉取下一批连接结果；空行表示结束。
    pub fn next(&mut self, required_rows: usize) -> Result<Vec<Row>, String> {
        if required_rows == 0 {
            return Ok(Vec::new());
        }
        while self.output.len() < required_rows && self.cursor < self.outer_rows.len() {
            let outer = self.outer_rows[self.cursor].clone();
            self.cursor += 1;
            let key = encode_correlated_key(&outer, &self.correlated_indices)?;
            let inners = if let Some(cached) = self.cache.get(&key) {
                self.cache_info.hits += 1;
                cached.clone()
            } else {
                self.cache_info.misses += 1;
                let rows = (self.inner_builder)(&outer)?;
                self.cache.insert(key, rows.clone());
                rows
            };
            let result = self.joiner.try_to_match_inners(
                &outer,
                &inners,
                &mut self.output,
                NaajType::Unknown,
            )?;
            if !result.matched {
                self.joiner
                    .on_miss_match(result.has_null, &outer, &mut self.output);
            }
        }
        self.cache_info.entries = self.cache.len();
        Ok(self
            .output
            .drain(..required_rows.min(self.output.len()))
            .collect())
    }
    /// 关闭并释放资源。
    pub fn close(&mut self) {
        self.outer_rows.clear();
        self.output.clear();
        self.cache.clear();
    }
}

/// Join 运行时统计包装。

#[derive(Clone, Debug, Default)]
pub struct JoinRuntimeStats {
    pub cache: CacheInfo,
    pub hash: HashStatistic,
    pub has_hash_stat: bool,
}
impl JoinRuntimeStats {
    /// 合并另一份运行时统计。
    pub fn merge(&mut self, other: &Self) {
        self.cache.hits += other.cache.hits;
        self.cache.misses += other.cache.misses;
        self.cache.entries = self.cache.entries.max(other.cache.entries);
        self.hash.probe_collision += other.hash.probe_collision;
        self.hash.build_table_elapsed += other.hash.build_table_elapsed;
        self.has_hash_stat |= other.has_hash_stat;
    }
}

/// 根据连接类型与 null-aware 标志分类 NAAJ 变体。

fn classify_naaj(
    table: &HashRowContainer,
    probe: &Row,
    indices: &[usize],
    pointers: &[RowPointer],
) -> NaajType {
    let left_null = indices.iter().any(|index| {
        probe
            .get(*index)
            .is_some_and(|value| matches!(value, Value::Null))
    });
    let right_null = pointers
        .iter()
        .filter_map(|pointer| table.row(*pointer))
        .any(|row| {
            indices.iter().any(|index| {
                row.get(*index)
                    .is_some_and(|value| matches!(value, Value::Null))
            })
        });
    match (left_null, right_null) {
        (true, true) => NaajType::LeftHasNullRightHasNull,
        (true, false) => NaajType::LeftHasNullRightNotNull,
        (false, true) => NaajType::LeftNotNullRightHasNull,
        (false, false) => NaajType::LeftNotNullRightNotNull,
    }
}
/// 该 NAAJ 变体是否需要特殊处理 NULL。
fn naaj_has_null(kind: NaajType) -> bool {
    matches!(
        kind,
        NaajType::LeftHasNullRightHasNull
            | NaajType::LeftHasNullRightNotNull
            | NaajType::LeftNotNullRightHasNull
    )
}
/// 把相关键列编码为字节，用作 Apply 缓存键。
fn encode_correlated_key(row: &Row, indices: &[usize]) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    for index in indices {
        let value = row
            .get(*index)
            .ok_or_else(|| format!("correlated index {index} is out of bounds"))?;
        output.extend(format!("{value:?}").as_bytes());
        output.push(0xff);
    }
    Ok(output)
}

/*
// String 对应 Go 声明 `func (e *joinRuntimeStats) String() string {`；保留原控制流、错误处理和外部依赖调用形状。
impl joinRuntimeStats {
    pub fn String(&mut self) -> String {
    buf = bytes.NewBuffer(make([]byte, 0, 16))
    buf.WriteString(self.RuntimeStatsWithConcurrencyInfo.String())
    if self.applyCache {
        if self.cache.useCache {
            fmt.Fprintf(buf, ", cache:ON, cacheHitRatio:%.3f%%", self.cache.hitRatio*100)
        } else {
            buf.WriteString(", cache:OFF")
        }
    }
    if self.hasHashStat {
        buf.WriteString(", " + self.hashStat.String())
    }
    return buf.String()
}
}
*/

/*
// Tp implements the RuntimeStats interface.
// Tp 对应 Go 声明 `func (*joinRuntimeStats) Tp() int {`；保留原控制流、错误处理和外部依赖调用形状。
impl joinRuntimeStats {
    pub fn Tp(&mut self) -> i32 {
    return execdetails.TpJoinRuntimeStats
}
}

// Clone 对应 Go 声明 `func (e *joinRuntimeStats) Clone() execdetails.RuntimeStats {`；保留原控制流、错误处理和外部依赖调用形状。
impl joinRuntimeStats {
    pub fn Clone(&mut self) -> execdetails::RuntimeStats {
    newJRS = &joinRuntimeStats{
        RuntimeStatsWithConcurrencyInfo: self.RuntimeStatsWithConcurrencyInfo,
        applyCache:                      self.applyCache,
        cache:                           self.cache,
        hasHashStat:                     self.hasHashStat,
        hashStat:                        self.hashStat,
    }
    return newJRS
}
}
*/
