// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Hash Join spill（落盘）辅助与状态机。
//
// 管理分区级构建/探测侧数据的写出与恢复：在内存超限时选择分区写入
// `SpillDisk`，多轮 spill 通过 `RestoreStack` 恢复。对应 Go `hash_join_spill_helper.go`。

// Hash Join V2 的 spill/restore 辅助器：选择分区、把 build/probe chunk 写入磁盘、维护 restore stack。
// 和测试 failpoint 均按 Go 语义保留调用形状并用中文解释关键资源收尾。
//
// use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
//
// pub const exceedMaxSpillRoundErrInfo: &str = "Exceed max spill round";
// pub const memFactorAfterSpill: f64 = 0.5;
//
// hashJoinSpillHelper 对应 Go 的 spill 总控状态，保存磁盘 chunk、字段类型、分区位图、统计和测试标志。
// pub struct hashJoinSpillHelper {
//     pub cond: sync::Cond,
//     pub spillStatus: i32,
//     pub hashJoinExec: HashJoinV2Exec,
//
//     pub buildRowsInDisk: Vec<Vec<Option<chunk::DataInDiskByChunks>>>,
//     pub probeRowsInDisk: Vec<Vec<Option<chunk::DataInDiskByChunks>>>,
//
//     pub buildSpillChkFieldTypes: Vec<types::FieldType>,
//     pub probeSpillFieldTypes: Vec<types::FieldType>,
//     pub tmpSpillBuildSideChunks: Vec<chunk::Chunk>,
//
// When respilling a row, we need to recalculate the row's hash value.
// These are auxiliary utility for rehash.
//     pub hash: hash::Hash64,
//     pub rehashBuf: bytes::Buffer,
//
//     pub stack: restoreStack,
//
//     pub memTracker: memory::Tracker,
//     pub diskTracker: disk::Tracker,
//
//     pub bytesConsumed: AtomicI64,
//     pub bytesLimit: AtomicI64,
//
// The hash value in restored probe row needs to be updated before we respill this row,
// and other columns in the row can be directly repilled.
// This variable describes which columns can be directed respilled.
//     pub probeSpilledRowIdx: Vec<i32>,
//
//     pub spilledPartitions: Vec<bool>,
//     pub validJoinKeysBuffer: Vec<Vec<u8>>,
//     pub spilledValidRowNum: AtomicU64,
//
// This variable will be set to false before restoring
//     pub spillTriggered: bool,
//
//     pub canSpillFlag: AtomicBool,
//     pub round: i32,
//
//     pub spillTriggeredForTest: bool,
//     pub spillRoundForTest: i32,
//     pub spillTriggedInBuildingStageForTest: bool,
//     pub spillTriggeredBeforeBuildingHashTableForTest: bool,
//     pub allPartitionsSpilledForTest: bool,
//     pub skipProbeInRestoreForTest: AtomicBool,
//     pub fileNamePrefixForTest: String,
// }
//
// newHashJoinSpillHelper 对应 Go 构造函数：准备落盘字段类型、分区位图、rehash 缓冲和 tracker 引用。
// pub fn newHashJoinSpillHelper(
//     hashJoinExec: Option<HashJoinV2Exec>,
//     partitionNum: i32,
//     probeFieldTypes: Vec<types::FieldType>,
//     fileNamePrefixForTest: String,
// ) -> hashJoinSpillHelper {
//     let mut buildSpillChkFieldTypes = Vec::with_capacity(3);
//     let mut hashValueField = types::NewFieldType(mysql::TypeLonglong);
//     hashValueField.AddFlag(mysql::UnsignedFlag);
//     buildSpillChkFieldTypes.push(hashValueField); // hash value
//     buildSpillChkFieldTypes.push(types::NewFieldType(mysql::TypeBit)); // valid join key
//     buildSpillChkFieldTypes.push(types::NewFieldType(mysql::TypeBit)); // row data
//
//     let probeSpillFieldTypes = getProbeSpillChunkFieldTypes(probeFieldTypes);
//     let mut probeSpilledRowIdx = Vec::with_capacity(probeSpillFieldTypes.len().saturating_sub(1));
//     for i in 1..probeSpillFieldTypes.len() {
//         probeSpilledRowIdx.push(i as i32);
//     }
//
//     let validJoinKeysBuffer = hashJoinExec
//         .as_ref()
//         .map(|exec| vec![Vec::new(); exec.Concurrency as usize])
//         .unwrap_or_default();
//     let memTracker = hashJoinExec
//         .as_ref()
//         .map(|exec| exec.memTracker.clone())
//         .unwrap_or_default();
//     let diskTracker = hashJoinExec
//         .as_ref()
//         .map(|exec| exec.diskTracker.clone())
//         .unwrap_or_default();
//
//     hashJoinSpillHelper {
//         cond: sync::NewCond(sync::Mutex::new()),
//         spillStatus: notSpilled,
//         hashJoinExec: hashJoinExec.unwrap_or_default(),
//         buildRowsInDisk: Vec::new(),
//         probeRowsInDisk: Vec::new(),
//         buildSpillChkFieldTypes,
//         probeSpillFieldTypes,
//         tmpSpillBuildSideChunks: Vec::new(),
//         hash: fnv::New64(),
//         rehashBuf: bytes::Buffer::new(),
//         stack: restoreStack { elems: Vec::new() },
//         memTracker,
//         diskTracker,
//         bytesConsumed: AtomicI64::new(0),
//         bytesLimit: AtomicI64::new(0),
//         probeSpilledRowIdx,
//         spilledPartitions: vec![false; partitionNum as usize],
//         validJoinKeysBuffer,
//         spilledValidRowNum: AtomicU64::new(0),
//         spillTriggered: false,
//         canSpillFlag: AtomicBool::new(false),
//         round: 0,
//         spillTriggeredForTest: false,
//         spillRoundForTest: 0,
//         spillTriggedInBuildingStageForTest: false,
//         spillTriggeredBeforeBuildingHashTableForTest: false,
//         allPartitionsSpilledForTest: false,
//         skipProbeInRestoreForTest: AtomicBool::new(false),
//         fileNamePrefixForTest,
//     }
// }
//
// impl hashJoinSpillHelper {
// close 对应 Go 资源收尾：关闭当前轮 build/probe 磁盘文件，并弹出 restore stack 中遗留分区逐一关闭。
//     pub fn close(&mut self) {
//         for inDisks in &mut self.buildRowsInDisk {
//             for inDisk in inDisks {
//                 if let Some(disk) = inDisk {
//                     disk.Close();
//                 }
//             }
//         }
//         for inDisks in &mut self.probeRowsInDisk {
//             for inDisk in inDisks {
//                 if let Some(disk) = inDisk {
//                     disk.Close();
//                 }
//             }
//         }
//
//         let mut partition = self.stack.pop();
//         while let Some(mut partition) = partition {
//             for inDisk in &mut partition.buildSideChunks {
//                 inDisk.Close();
//             }
//             for inDisk in &mut partition.probeSideChunks {
//                 inDisk.Close();
//             }
//             partition = self.stack.pop();
//         }
//     }
//
// areAllPartitionsSpilled 对应 Go：检查当前轮所有分区是否都已 spill。
//     pub fn areAllPartitionsSpilled(&self) -> bool {
//         self.spilledPartitions.iter().all(|spilled| *spilled)
//     }
//
// setCanSpillFlag 对应 Go：row table 合并后 hash join 不能再 spill，用此 flag 引导其他 executor spill。
//     pub fn setCanSpillFlag(&self, canSpill: bool) {
//         self.canSpillFlag.store(canSpill, Ordering::SeqCst);
//     }
//
//     pub fn canSpill(&self) -> bool {
//         self.canSpillFlag.load(Ordering::SeqCst)
//     }
//
//     pub fn getSpilledPartitions(&self) -> Vec<i32> {
//         self.spilledPartitions
//             .iter()
//             .enumerate()
//             .filter_map(|(i, spilled)| if *spilled { Some(i as i32) } else { None })
//             .collect()
//     }
//
//     pub fn getUnspilledPartitions(&self) -> Vec<i32> {
//         self.spilledPartitions
//             .iter()
//             .enumerate()
//             .filter_map(|(i, spilled)| if !*spilled { Some(i as i32) } else { None })
//             .collect()
//     }
//
//     pub fn setPartitionSpilled(&mut self, partIDs: &[i32]) {
//         for partID in partIDs {
//             self.spilledPartitions[*partID as usize] = true;
//         }
//         self.spillTriggered = true;
//     }
//
//     pub fn setNotSpilled(&mut self) {
//         let _guard = self.cond.L.Lock();
//         self.spillStatus = notSpilled;
//     }
//
//     pub fn setInSpilling(&mut self) {
//         let _guard = self.cond.L.Lock();
//         self.spillStatus = inSpilling;
//     }
//
//     pub fn setNeedSpillNoLock(&mut self) {
//         self.spillStatus = needSpill;
//     }
//
//     pub fn isNotSpilledNoLock(&self) -> bool {
//         self.spillStatus == notSpilled
//     }
//
//     pub fn isInSpillingNoLock(&self) -> bool {
//         self.spillStatus == inSpilling
//     }
//
//     pub fn isSpillNeeded(&self) -> bool {
//         let _guard = self.cond.L.Lock();
//         self.spillStatus == needSpill
//     }
//
//     pub fn isSpillTriggered(&self) -> bool {
//         self.spillTriggered
//     }
//
//     pub fn isPartitionSpilled(&self, partID: i32) -> bool {
//         self.spilledPartitions[partID as usize]
//     }
//
// choosePartitionsToSpill 对应 Go：优先复用已 spill 分区释放内存，不够时按内存占用降序选择更多分区。
//     pub fn choosePartitionsToSpill(&self, hashTableMemUsage: Option<Vec<i64>>) -> (Vec<i32>, i64) {
//         let partitionNum = self.hashJoinExec.partitionNumber;
//         let mut partitionsMemoryUsage = vec![0_i64; partitionNum as usize];
//         for i in 0..partitionNum as usize {
//             partitionsMemoryUsage[i] = self.hashJoinExec.hashTableContext.getPartitionMemoryUsage(i as i32);
//             if let Some(extra) = &hashTableMemUsage {
//                 partitionsMemoryUsage[i] += extra[i];
//             }
//         }
//
//         let mut spilledPartitions = self.getSpilledPartitions();
//         let mut releasedMemoryUsage = 0_i64;
//         for partID in &spilledPartitions {
//             releasedMemoryUsage += partitionsMemoryUsage[*partID as usize];
//         }
//
//         let bytesConsumed = self.memTracker.BytesConsumed();
//         let bytesLimit = self.bytesLimit.load(Ordering::SeqCst);
//         let mut bytesConsumedAfterReleased = bytesConsumed - releasedMemoryUsage;
//         if (bytesConsumedAfterReleased as f64) <= (bytesLimit as f64) * memFactorAfterSpill {
//             return (spilledPartitions, releasedMemoryUsage);
//         }
//
//         let mut candidates: Vec<(i32, i64)> = self
//             .getUnspilledPartitions()
//             .into_iter()
//             .map(|partID| (partID, partitionsMemoryUsage[partID as usize]))
//             .collect();
// Go 使用 SliceStable 按内存占用降序，确保释放最少分区达到阈值。
//         candidates.sort_by(|a, b| b.1.cmp(&a.1));
//
//         for (partID, memoryUsage) in candidates {
//             spilledPartitions.push(partID);
//             releasedMemoryUsage += memoryUsage;
//             bytesConsumedAfterReleased -= memoryUsage;
//             if (bytesConsumedAfterReleased as f64) <= (bytesLimit as f64) * memFactorAfterSpill {
//                 return (spilledPartitions, releasedMemoryUsage);
//             }
//         }
//         (spilledPartitions, releasedMemoryUsage)
//     }
//
// generateSpilledValidJoinKey 对应 Go：把 segment 中有效 join key 位置转为 0/1 字节数组并累计统计。
//     pub fn generateSpilledValidJoinKey(
//         &self,
//         seg: &rowTableSegment,
//         mut validJoinKeys: Vec<u8>,
//     ) -> Vec<u8> {
//         let rowLen = seg.rowStartOffset.len();
//         validJoinKeys.resize(rowLen, 0);
//         for byte in &mut validJoinKeys {
//             *byte = 0;
//         }
//         for pos in &seg.validJoinKeyPos {
//             validJoinKeys[*pos as usize] = 1;
//         }
//         self.spilledValidRowNum
//             .fetch_add(seg.validJoinKeyPos.len() as u64, Ordering::SeqCst);
//         validJoinKeys
//     }
//
// spillBuildSegmentToDisk 对应 Go：惰性创建 worker/partition 对应的 build/probe 磁盘 chunk。
//     pub fn spillBuildSegmentToDisk(
//         &mut self,
//         workerID: i32,
//         partID: i32,
//         segments: Vec<rowTableSegment>,
//     ) -> Result<(), errors::Error> {
//         let workerID = workerID as usize;
//         let partID = partID as usize;
//         if self.buildRowsInDisk[workerID].is_empty() {
//             self.buildRowsInDisk[workerID] = vec![None; self.hashJoinExec.partitionNumber as usize];
//             self.probeRowsInDisk[workerID] = vec![None; self.hashJoinExec.partitionNumber as usize];
//         }
//
//         if self.buildRowsInDisk[workerID][partID].is_none() {
//             let mut inDisk = chunk::NewDataInDiskByChunks(
//                 self.buildSpillChkFieldTypes.clone(),
//                 self.fileNamePrefixForTest.clone(),
//             );
//             inDisk.GetDiskTracker().AttachTo(&self.diskTracker);
//             self.buildRowsInDisk[workerID][partID] = Some(inDisk);
//
//             let mut probeDisk = chunk::NewDataInDiskByChunks(
//                 self.probeSpillFieldTypes.clone(),
//                 self.fileNamePrefixForTest.clone(),
//             );
//             probeDisk.GetDiskTracker().AttachTo(&self.diskTracker);
//             self.probeRowsInDisk[workerID][partID] = Some(probeDisk);
//         }
//
//         let disk = self.buildRowsInDisk[workerID][partID].as_mut().unwrap();
//         self.spillSegmentsToDiskImpl(workerID as i32, disk, segments)
//     }
//
// spillSegmentsToDiskImpl 对应 Go：把 row table segment 展开成三列 chunk：hash、valid key 标志、row bytes。
//     pub fn spillSegmentsToDiskImpl(
//         &mut self,
//         workerID: i32,
//         disk: &mut chunk::DataInDiskByChunks,
//         segments: Vec<rowTableSegment>,
//     ) -> Result<(), errors::Error> {
//         let workerID = workerID as usize;
//         self.validJoinKeysBuffer[workerID].clear();
//         self.tmpSpillBuildSideChunks[workerID].Reset();
//
//         for seg in segments {
//             self.validJoinKeysBuffer[workerID] =
//                 self.generateSpilledValidJoinKey(&seg, self.validJoinKeysBuffer[workerID].clone());
//             for i in 0..seg.getRowNum() as usize {
//                 let row = seg.getRowBytes(i as i32);
//                 if self.tmpSpillBuildSideChunks[workerID].IsFull() {
//                     disk.Add(&self.tmpSpillBuildSideChunks[workerID])?;
//                     self.tmpSpillBuildSideChunks[workerID].Reset();
//                     if let Some(err) = triggerIntest(2) {
//                         return Err(err);
//                     }
//                 }
//                 self.tmpSpillBuildSideChunks[workerID].AppendUint64(0, seg.hashValues[i]);
//                 self.tmpSpillBuildSideChunks[workerID]
//                     .AppendBytes(1, &self.validJoinKeysBuffer[workerID][i..i + 1]);
//                 self.tmpSpillBuildSideChunks[workerID].AppendBytes(2, row);
//             }
//         }
//
//         if self.tmpSpillBuildSideChunks[workerID].NumRows() > 0 {
//             disk.Add(&self.tmpSpillBuildSideChunks[workerID])?;
//             self.tmpSpillBuildSideChunks[workerID].Reset();
//         }
//         Ok(())
//     }
//
//     pub fn spillProbeChk(
//         &mut self,
//         workerID: i32,
//         partID: i32,
//         chk: &chunk::Chunk,
//     ) -> Result<(), errors::Error> {
//         self.probeRowsInDisk[workerID as usize][partID as usize]
//             .as_mut()
//             .unwrap()
//             .Add(chk)
//     }
//
// init 对应 Go 首次 spill 初始化：创建磁盘数组和 worker 的 restored chunk buffer。
//     pub fn init(&mut self) {
//         if self.buildRowsInDisk.is_empty() {
//             self.initTmpSpillBuildSideChunks();
//             self.buildRowsInDisk = vec![Vec::new(); self.hashJoinExec.Concurrency as usize];
//             self.probeRowsInDisk = vec![Vec::new(); self.hashJoinExec.Concurrency as usize];
//
//             for worker in &mut self.hashJoinExec.BuildWorkers {
//                 if worker.restoredChkBuf.is_none() {
//                     worker.restoredChkBuf =
//                         Some(chunk::NewEmptyChunk(self.buildSpillChkFieldTypes.clone()));
//                 }
//             }
//             for worker in &mut self.hashJoinExec.ProbeWorkers {
//                 if worker.restoredChkBuf.is_none() {
//                     worker.restoredChkBuf =
//                         Some(chunk::NewEmptyChunk(self.probeSpillFieldTypes.clone()));
//                 }
//             }
//         }
//     }
//
//     pub fn getSpilledPartitionsNum(&self) -> i32 {
//         self.getSpilledPartitions().len() as i32
//     }
//
//     pub fn getBuildSpillBytes(&self) -> i64 {
//         self.getSpillBytesImpl(&self.buildRowsInDisk)
//     }
//
//     pub fn getProbeSpillBytes(&self) -> i64 {
//         self.getSpillBytesImpl(&self.probeRowsInDisk)
//     }
//
//     pub fn getSpillBytesImpl(&self, disks: &Vec<Vec<Option<chunk::DataInDiskByChunks>>>) -> i64 {
//         let mut totalBytes = 0_i64;
//         for disk in disks {
//             for d in disk {
//                 if let Some(d) = d {
//                     totalBytes += d.GetTotalBytesInDisk();
//                 }
//             }
//         }
//         totalBytes
//     }
//
// spillRowTableImpl 对应 Go：并发收集各 worker 的目标分区 segment，写盘后释放 hash table tracker 内存。
//     pub fn spillRowTableImpl(
//         &mut self,
//         partitionsNeedSpill: Vec<i32>,
//         totalReleasedMemory: i64,
//     ) -> Result<(), errors::Error> {
//         let workerNum = self.hashJoinExec.BuildWorkers.len();
//         let errChannel = make_error_channel(workerNum);
//         let mut wg = util::WaitGroupWrapper::new();
//
//         self.setPartitionSpilled(&partitionsNeedSpill);
//         if intest::InTest {
//             if partitionsNeedSpill.len() == self.hashJoinExec.partitionNumber as usize {
//                 self.allPartitionsSpilledForTest = true;
//             }
//             self.spillTriggeredForTest = true;
//         }
//
//         logutil::BgLogger().Info(
//             spillInfo,
//             zap::Int64("consumed", self.bytesConsumed.load(Ordering::SeqCst)),
//             zap::Int64("quota", self.bytesLimit.load(Ordering::SeqCst)),
//         );
//
//         for workerID in 0..workerNum {
//             let parts = partitionsNeedSpill.clone();
//             wg.RunWithRecover(
//                 || {
//                     for partID in &parts {
// 每个 worker 先 finalize 当前 segment，再清空 row table 中对应分区，避免重复 spill。
//                         let worker = &mut self.hashJoinExec.BuildWorkers[workerID];
//                         let spilledSegments = worker.getSegmentsInRowTable(*partID);
//                         worker.clearSegmentsInRowTable(*partID);
//                         if let Err(err) =
//                             self.spillBuildSegmentToDisk(workerID as i32, *partID, spilledSegments)
//                         {
//                             errChannel.send(util::GetRecoverError(err));
//                         }
//                     }
//                 },
//                 |r| {
//                     if let Some(r) = r {
//                         errChannel.send(util::GetRecoverError(r));
//                     }
//                 },
//             );
//         }
//
//         wg.Wait();
//         errChannel.close();
//         if let Some(err) = errChannel.recv() {
//             return Err(err);
//         }
//         self.hashJoinExec
//             .hashTableContext
//             .memoryTracker
//             .Consume(-totalReleasedMemory);
//
//         if let Some(err) = triggerIntest(10) {
//             return Err(err);
//         }
//         Ok(())
//     }
//
// spillRemainingRows 对应 Go：restore 前把已标记 spill 分区的剩余 build rows 继续写盘。
//     pub fn spillRemainingRows(&mut self) -> Result<(), errors::Error> {
//         self.setInSpilling();
//         let _broadcast_on_return = BroadcastOnDrop::new(&self.cond);
//         let _reset_on_return = ResetSpillStatusOnDrop::new(self);
//
//         checkSQLKiller(
//             &self.hashJoinExec.HashJoinCtxV2.SessCtx.GetSessionVars().SQLKiller,
//             "killedDuringBuildSpill",
//         )?;
//         self.init();
//
//         let spilledPartitions = self.getSpilledPartitions();
//         let mut totalReleasedMemoryUsage = 0_i64;
//         for partID in &spilledPartitions {
//             totalReleasedMemoryUsage += self
//                 .hashJoinExec
//                 .hashTableContext
//                 .getPartitionMemoryUsage(*partID);
//         }
//         self.bytesConsumed
//             .store(self.memTracker.BytesConsumed(), Ordering::SeqCst);
//         self.spillRowTableImpl(spilledPartitions, totalReleasedMemoryUsage)
//     }
//
// spillRowTable 对应 Go：真正执行一次 row table spill，包含状态切换、SQL killer 检查和分区选择。
//     pub fn spillRowTable(&mut self, hashTableMemUsage: Option<Vec<i64>>) -> Result<(), errors::Error> {
//         self.setInSpilling();
//         let _broadcast_on_return = BroadcastOnDrop::new(&self.cond);
//         let _reset_on_return = ResetSpillStatusOnDrop::new(self);
//
//         checkSQLKiller(
//             &self.hashJoinExec.HashJoinCtxV2.SessCtx.GetSessionVars().SQLKiller,
//             "killedDuringBuildSpill",
//         )?;
//         self.init();
//
//         let (partitionsNeedSpill, totalReleasedMemory) =
//             self.choosePartitionsToSpill(hashTableMemUsage);
//         self.spillRowTableImpl(partitionsNeedSpill, totalReleasedMemory)
//     }
//
// reset 对应 Go restore 前重置当前轮临时 spill 状态，但不清理 restore stack。
//     pub fn reset(&mut self) {
//         for i in 0..self.buildRowsInDisk.len() {
//             self.buildRowsInDisk[i].clear();
//             self.probeRowsInDisk[i].clear();
//         }
//         for spilled in &mut self.spilledPartitions {
//             *spilled = false;
//         }
//         self.spilledValidRowNum.store(0, Ordering::SeqCst);
//         self.spillTriggered = false;
//     }
//
// prepareForRestoring 对应 Go：把本轮已 spill 分区按分区聚合进 restore stack，并检查最大 spill 轮次。
//     pub fn prepareForRestoring(&mut self, lastRound: i32) -> Result<(), errors::Error> {
//         if let Some(err) = triggerIntest(10) {
//             return Err(err);
//         }
//         if lastRound + 1 > self.hashJoinExec.maxSpillRound {
//             return Err(errors::NewNoStackError(exceedMaxSpillRoundErrInfo));
//         }
//         if self.buildRowsInDisk.is_empty() {
//             return Ok(());
//         }
//
//         let partNum = self.hashJoinExec.partitionNumber as usize;
//         let concurrency = self.hashJoinExec.Concurrency as usize;
//         for i in 0..partNum {
//             if self.spilledPartitions[i] {
//                 let mut buildInDisks = Vec::new();
//                 let mut probeInDisks = Vec::new();
//                 for j in 0..concurrency {
//                     if !self.buildRowsInDisk[j].is_empty()
//                         && self.buildRowsInDisk[j][i].is_some()
//                     {
//                         buildInDisks.push(self.buildRowsInDisk[j][i].take().unwrap());
//                         probeInDisks.push(self.probeRowsInDisk[j][i].take().unwrap());
//                     }
//                 }
//                 if buildInDisks.is_empty() {
//                     continue;
//                 }
//                 self.stack.push(restorePartition {
//                     buildSideChunks: buildInDisks,
//                     probeSideChunks: probeInDisks,
//                     round: lastRound + 1,
//                 });
//             }
//         }
//
// spill 可能在 restore 期间再次触发，所以进入 restore 前清理当前轮位图和统计。
//         self.reset();
//         Ok(())
//     }
//
//     pub fn initTmpSpillBuildSideChunks(&mut self) {
//         while self.tmpSpillBuildSideChunks.len() < self.hashJoinExec.Concurrency as usize {
//             self.tmpSpillBuildSideChunks.push(chunk::NewChunkWithCapacity(
//                 self.buildSpillChkFieldTypes.clone(),
//                 unsafe { spillChunkSize },
//             ));
//         }
//     }
//
//     pub fn isProbeSkippedInRestoreForTest(&self) -> bool {
//         self.skipProbeInRestoreForTest.load(Ordering::SeqCst)
//     }
//
//     pub fn isRespillTriggeredForTest(&self) -> bool {
//         self.spillRoundForTest > 1
//     }
//
//     pub fn isSpillTriggeredForTest(&self) -> bool {
//         self.spillTriggeredForTest
//     }
//
//     pub fn isSpillTriggedInBuildingStageForTest(&self) -> bool {
//         self.spillTriggedInBuildingStageForTest
//     }
//
//     pub fn areAllPartitionsSpilledForTest(&self) -> bool {
//         self.allPartitionsSpilledForTest
//     }
//
//     pub fn isSpillTriggeredBeforeBuildingHashTableForTest(&self) -> bool {
//         self.spillTriggeredBeforeBuildingHashTableForTest
//     }
// }
//
// Data in this structure are in same partition
// restorePartition 对应 Go 的恢复栈元素，同一元素内 build/probe 磁盘 chunk 属于同一个分区。
// pub struct restorePartition {
//     pub buildSideChunks: Vec<chunk::DataInDiskByChunks>,
//     pub probeSideChunks: Vec<chunk::DataInDiskByChunks>,
//     pub round: i32,
// }
//
// restoreStack 对应 Go 的简单栈，restore 时后进先出处理 spill 分区。
// pub struct restoreStack {
//     pub elems: Vec<restorePartition>,
// }
//
// impl restoreStack {
//     pub fn pop(&mut self) -> Option<restorePartition> {
//         self.elems.pop()
//     }
//
//     pub fn push(&mut self, elem: restorePartition) {
//         self.elems.push(elem);
//     }
// }
// */
use crate::join_row_table::{RowTable, RowTableSegment};
use crate::joiner::Row;
use crate::row_table_builder::Chunk;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

/// 超过最大 spill 轮次时的错误文案。

pub const EXCEED_MAX_SPILL_ROUND_ERROR: &str = "Exceed max spill round";
/// spill 后期望内存占用不超过限额乘以此系数。
pub const MEM_FACTOR_AFTER_SPILL: f64 = 0.5;
/// 每次写出磁盘的默认 chunk 行容量。
pub const SPILL_CHUNK_SIZE: usize = 1024;

/// Spill 状态机：未 spill / 需要 spill / 正在 spill。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpillStatus {
    #[default]
    NotSpilled,
    NeedSpill,
    InSpilling,
}

/// 简易内存配额跟踪器：记录已消费字节与上限。

#[derive(Debug, Default)]
pub struct MemoryTracker {
    consumed: AtomicI64,
    limit: AtomicI64,
}
impl MemoryTracker {
    /// 构造带初始限额的 Tracker。
    pub fn new(limit: i64) -> Self {
        Self {
            consumed: AtomicI64::new(0),
            limit: AtomicI64::new(limit),
        }
    }
    /// 累加（可为负）已消费字节。
    pub fn consume(&self, bytes: i64) {
        self.consumed.fetch_add(bytes, Ordering::AcqRel);
    }
    /// 当前已消费字节。
    pub fn bytes_consumed(&self) -> i64 {
        self.consumed.load(Ordering::Acquire)
    }
    /// 内存上限（负数表示无限制）。
    pub fn bytes_limit(&self) -> i64 {
        self.limit.load(Ordering::Acquire)
    }
    /// 更新内存上限。
    pub fn set_limit(&self, limit: i64) {
        self.limit.store(limit, Ordering::Release);
    }
    /// 是否已超过限额。
    pub fn check_exceed(&self) -> bool {
        let limit = self.bytes_limit();
        limit >= 0 && self.bytes_consumed() > limit
    }
}

/// 落盘的构建侧行：哈希值、键是否有效、编码字节。

#[derive(Clone, Debug, Default)]
pub struct SpilledBuildRow {
    pub hash_value: u64,
    pub valid_join_key: bool,
    pub row_bytes: Vec<u8>,
}

/// 内存模拟的 spill 磁盘：按 chunk 追加并累计字节。

#[derive(Clone, Debug, Default)]
pub struct SpillDisk<T> {
    chunks: Vec<Vec<T>>,
    total_bytes: i64,
    closed: bool,
}
impl<T: Clone> SpillDisk<T> {
    /// 追加一块数据并累计字节；已关闭则报错。
    pub fn add_chunk(&mut self, chunk: &[T], bytes: i64) -> Result<(), String> {
        if self.closed {
            return Err("spill disk is closed".into());
        }
        self.chunks.push(chunk.to_vec());
        self.total_bytes += bytes;
        Ok(())
    }
    /// 已写出的各块数据。
    pub fn chunks(&self) -> &[Vec<T>] {
        &self.chunks
    }
    /// 累计写出字节。
    pub fn total_bytes(&self) -> i64 {
        self.total_bytes
    }
    /// 关闭所有磁盘缓冲并清空恢复栈。
    pub fn close(&mut self) {
        self.chunks.clear();
        self.total_bytes = 0;
        self.closed = true;
    }
}

/// 待恢复的分区：构建/探测侧落盘数据与轮次。

#[derive(Clone, Debug, Default)]
pub struct RestorePartition {
    pub build_side_chunks: Vec<Vec<SpilledBuildRow>>,
    pub probe_side_chunks: Vec<Chunk>,
    pub round: usize,
}

/// 恢复分区栈（后进先出，用于多轮 spill 恢复）。

#[derive(Clone, Debug, Default)]
pub struct RestoreStack {
    elems: Vec<RestorePartition>,
}
impl RestoreStack {
    /// 弹出栈顶恢复分区。
    pub fn pop(&mut self) -> Option<RestorePartition> {
        self.elems.pop()
    }
    /// 压入恢复分区。
    pub fn push(&mut self, partition: RestorePartition) {
        self.elems.push(partition);
    }
    /// 栈中分区数。
    pub fn len(&self) -> usize {
        self.elems.len()
    }
    /// 栈是否为空。
    pub fn is_empty(&self) -> bool {
        self.elems.is_empty()
    }
}

/// 受 Mutex 保护的 spill 内部状态。

#[derive(Debug)]
struct SpillState {
    status: SpillStatus,
    spilled_partitions: Vec<bool>,
    spill_triggered: bool,
    round: usize,
    build_rows_in_disk: Vec<Vec<Option<SpillDisk<SpilledBuildRow>>>>,
    probe_rows_in_disk: Vec<Vec<Option<SpillDisk<Row>>>>,
    stack: RestoreStack,
    spill_triggered_for_test: bool,
    spill_triggered_in_building_stage_for_test: bool,
    spill_triggered_before_building_hash_table_for_test: bool,
    all_partitions_spilled_for_test: bool,
}

/// Hash Join spill 辅助：分区落盘、状态机、恢复栈与内存/磁盘 Tracker。

pub struct HashJoinSpillHelper {
    state: Mutex<SpillState>,
    cond: Condvar,
    partition_num: usize,
    concurrency: usize,
    max_spill_round: usize,
    can_spill_flag: AtomicBool,
    pub memory_tracker: MemoryTracker,
    pub disk_tracker: MemoryTracker,
    bytes_consumed: AtomicI64,
    bytes_limit: AtomicI64,
    spilled_valid_row_num: AtomicU64,
    skip_probe_in_restore_for_test: AtomicBool,
}

impl HashJoinSpillHelper {
    /// 构造带初始限额的 Tracker。
    pub fn new(
        partition_num: usize,
        concurrency: usize,
        max_spill_round: usize,
        memory_limit: i64,
    ) -> Result<Self, String> {
        if partition_num == 0 || concurrency == 0 {
            return Err("partition number and concurrency must be positive".into());
        }
        Ok(Self {
            state: Mutex::new(SpillState {
                status: SpillStatus::NotSpilled,
                spilled_partitions: vec![false; partition_num],
                spill_triggered: false,
                round: 0,
                build_rows_in_disk: vec![vec![None; partition_num]; concurrency],
                probe_rows_in_disk: vec![vec![None; partition_num]; concurrency],
                stack: RestoreStack::default(),
                spill_triggered_for_test: false,
                spill_triggered_in_building_stage_for_test: false,
                spill_triggered_before_building_hash_table_for_test: false,
                all_partitions_spilled_for_test: false,
            }),
            cond: Condvar::new(),
            partition_num,
            concurrency,
            max_spill_round,
            can_spill_flag: AtomicBool::new(false),
            memory_tracker: MemoryTracker::new(memory_limit),
            disk_tracker: MemoryTracker::new(-1),
            bytes_consumed: AtomicI64::new(0),
            bytes_limit: AtomicI64::new(memory_limit),
            spilled_valid_row_num: AtomicU64::new(0),
            skip_probe_in_restore_for_test: AtomicBool::new(false),
        })
    }
    /// 关闭所有磁盘缓冲并清空恢复栈。
    pub fn close(&self) {
        let mut state = self.state.lock().expect("spill helper poisoned");
        for worker in &mut state.build_rows_in_disk {
            for disk in worker.iter_mut().flatten() {
                disk.close();
            }
        }
        for worker in &mut state.probe_rows_in_disk {
            for disk in worker.iter_mut().flatten() {
                disk.close();
            }
        }
        while state.stack.pop().is_some() {}
    }
    /// 是否所有分区都已 spill。
    pub fn are_all_partitions_spilled(&self) -> bool {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spilled_partitions
            .iter()
            .all(|spilled| *spilled)
    }
    /// 设置是否允许触发 spill。
    pub fn set_can_spill_flag(&self, can_spill: bool) {
        self.can_spill_flag.store(can_spill, Ordering::Release);
    }
    /// 当前是否允许 spill。
    pub fn can_spill(&self) -> bool {
        self.can_spill_flag.load(Ordering::Acquire)
    }
    /// 已 spill 的分区下标列表。
    pub fn spilled_partitions(&self) -> Vec<usize> {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spilled_partitions
            .iter()
            .enumerate()
            .filter_map(|(partition, spilled)| (*spilled).then_some(partition))
            .collect()
    }
    /// 尚未 spill 的分区下标列表。
    pub fn unspilled_partitions(&self) -> Vec<usize> {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spilled_partitions
            .iter()
            .enumerate()
            .filter_map(|(partition, spilled)| (!*spilled).then_some(partition))
            .collect()
    }
    /// 标记给定分区已 spill，并更新测试用标志。
    pub fn set_partition_spilled(&self, partitions: &[usize]) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "spill helper poisoned".to_string())?;
        for partition in partitions {
            let spilled = state
                .spilled_partitions
                .get_mut(*partition)
                .ok_or_else(|| format!("partition {partition} out of bounds"))?;
            *spilled = true;
        }
        state.spill_triggered = true;
        state.spill_triggered_for_test = true;
        state.all_partitions_spilled_for_test =
            state.spilled_partitions.iter().all(|spilled| *spilled);
        Ok(())
    }
    /// 当前 spill 状态。
    pub fn status(&self) -> SpillStatus {
        self.state.lock().expect("spill helper poisoned").status
    }
    /// 置为 NotSpilled 并唤醒等待者。
    pub fn set_not_spilled(&self) {
        let mut state = self.state.lock().expect("spill helper poisoned");
        state.status = SpillStatus::NotSpilled;
        self.cond.notify_all();
    }
    /// 置为 InSpilling。
    pub fn set_in_spilling(&self) {
        self.state.lock().expect("spill helper poisoned").status = SpillStatus::InSpilling;
    }
    /// 置为 NeedSpill 并记录触发时内存快照。
    pub fn set_need_spill(&self, consumed: i64, limit: i64) {
        let mut state = self.state.lock().expect("spill helper poisoned");
        state.status = SpillStatus::NeedSpill;
        self.bytes_consumed.store(consumed, Ordering::Release);
        self.bytes_limit.store(limit, Ordering::Release);
    }
    /// 是否处于 NeedSpill。
    pub fn is_spill_needed(&self) -> bool {
        self.status() == SpillStatus::NeedSpill
    }
    /// 本轮是否已触发过 spill。
    pub fn is_spill_triggered(&self) -> bool {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spill_triggered
    }
    /// 指定分区是否已 spill。
    pub fn is_partition_spilled(&self, partition: usize) -> bool {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spilled_partitions
            .get(partition)
            .copied()
            .unwrap_or(false)
    }
    /// 若正在 spill 则阻塞等待结束。
    pub fn wait_while_spilling(&self) {
        let mut state = self.state.lock().expect("spill helper poisoned");
        while state.status == SpillStatus::InSpilling {
            state = self.cond.wait(state).expect("spill helper poisoned");
        }
    }
    /// 按内存占用从大到小选择要 spill 的分区，直到预估内存降至限额*系数以下。
    pub fn choose_partitions_to_spill(
        &self,
        partition_memory_usage: &[i64],
        hash_table_memory_usage: Option<&[i64]>,
    ) -> Result<(Vec<usize>, i64), String> {
        if partition_memory_usage.len() != self.partition_num
            || hash_table_memory_usage.is_some_and(|usage| usage.len() != self.partition_num)
        {
            return Err("partition memory usage length mismatch".into());
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "spill helper poisoned".to_string())?;
        let mut usage = partition_memory_usage.to_vec();
        if let Some(hash_usage) = hash_table_memory_usage {
            for (value, hash) in usage.iter_mut().zip(hash_usage) {
                *value += *hash;
            }
        }
        let mut selected: Vec<usize> = state
            .spilled_partitions
            .iter()
            .enumerate()
            .filter_map(|(index, spilled)| (*spilled).then_some(index))
            .collect();
        let mut released: i64 = selected.iter().map(|partition| usage[*partition]).sum();
        let consumed = self.memory_tracker.bytes_consumed();
        let limit = self.bytes_limit.load(Ordering::Acquire);
        if (consumed - released) as f64 <= limit as f64 * MEM_FACTOR_AFTER_SPILL {
            return Ok((selected, released));
        }
        let mut candidates: Vec<(usize, i64)> = state
            .spilled_partitions
            .iter()
            .enumerate()
            .filter_map(|(partition, spilled)| (!*spilled).then_some((partition, usage[partition])))
            .collect();
        // 未 spill 分区按内存占用降序挑选，直到预估占用降至限额的 MEM_FACTOR_AFTER_SPILL。
        candidates.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        for (partition, bytes) in candidates {
            selected.push(partition);
            released += bytes;
            if (consumed - released) as f64 <= limit as f64 * MEM_FACTOR_AFTER_SPILL {
                break;
            }
        }
        Ok((selected, released))
    }
    /// 根据 segment 有效键数生成 valid_join_key 位图并累计有效行数。
    pub fn generate_spilled_valid_join_key(&self, segment: &RowTableSegment, buffer: &mut Vec<u8>) {
        buffer.clear();
        buffer.resize(segment.rows.len(), 0);
        for index in 0..segment.valid_key_count.min(segment.rows.len() as u64) as usize {
            buffer[index] = 1;
        }
        self.spilled_valid_row_num
            .fetch_add(segment.valid_key_count, Ordering::AcqRel);
    }
    /// 将构建侧 row table 段写入指定 worker/分区的磁盘缓冲。
    pub fn spill_build_segments(
        &self,
        worker: usize,
        partition: usize,
        segments: &[RowTableSegment],
    ) -> Result<(), String> {
        if worker >= self.concurrency || partition >= self.partition_num {
            return Err("worker or partition out of bounds".into());
        }
        let mut rows = Vec::new();
        let mut valid = Vec::new();
        for segment in segments {
            self.generate_spilled_valid_join_key(segment, &mut valid);
            for (index, encoded) in segment.rows.iter().enumerate() {
                rows.push(SpilledBuildRow {
                    hash_value: segment.hash_values[index],
                    valid_join_key: valid[index] != 0,
                    row_bytes: encoded.bytes.clone(),
                });
            }
        }
        let bytes = rows
            .iter()
            .map(|row| row.row_bytes.len() + 9)
            .sum::<usize>() as i64;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "spill helper poisoned".to_string())?;
        let disk =
            state.build_rows_in_disk[worker][partition].get_or_insert_with(SpillDisk::default);
        disk.add_chunk(&rows, bytes)?;
        self.disk_tracker.consume(bytes);
        Ok(())
    }
    /// 将探测侧 chunk 写入指定 worker/分区的磁盘缓冲。
    pub fn spill_probe_chunk(
        &self,
        worker: usize,
        partition: usize,
        chunk: &Chunk,
    ) -> Result<(), String> {
        if worker >= self.concurrency || partition >= self.partition_num {
            return Err("worker or partition out of bounds".into());
        }
        let bytes = chunk.iter().map(row_size).sum::<usize>() as i64;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "spill helper poisoned".to_string())?;
        let disk =
            state.probe_rows_in_disk[worker][partition].get_or_insert_with(SpillDisk::default);
        disk.add_chunk(chunk, bytes)?;
        self.disk_tracker.consume(bytes);
        Ok(())
    }
    fn spill_selected_partitions(
        &self,
        worker_tables: &mut [Vec<RowTable>],
        partitions: &[usize],
        released: i64,
    ) -> Result<i64, String> {
        self.set_partition_spilled(partitions)?;
        for (worker, tables) in worker_tables.iter_mut().enumerate() {
            for partition in partitions {
                let segments = std::mem::take(
                    tables
                        .get_mut(*partition)
                        .ok_or_else(|| "partition table missing".to_string())?
                        .segments_mut(),
                );
                self.spill_build_segments(worker, *partition, &segments)?;
            }
        }
        self.memory_tracker.consume(-released);
        Ok(released)
    }
    /// 进入 InSpilling，选择分区并落盘构建侧表，完成后回到 NotSpilled。
    pub fn spill_row_tables(
        &self,
        worker_tables: &mut [Vec<RowTable>],
        partition_usage: &[i64],
        hash_usage: Option<&[i64]>,
    ) -> Result<i64, String> {
        // 整段 spill 包在 InSpilling 状态内，结束（成功或失败）后恢复 NotSpilled 并唤醒等待者。
        self.set_in_spilling();
        let result = (|| {
            let (partitions, released) =
                self.choose_partitions_to_spill(partition_usage, hash_usage)?;
            self.spill_selected_partitions(worker_tables, &partitions, released)
        })();
        self.set_not_spilled();
        result
    }
    /// 对已标记 spill 的分区再次写出残留行表。
    pub fn spill_remaining_rows(
        &self,
        worker_tables: &mut [Vec<RowTable>],
        partition_usage: &[i64],
    ) -> Result<i64, String> {
        if partition_usage.len() != self.partition_num {
            return Err("partition memory usage length mismatch".into());
        }
        self.set_in_spilling();
        let result = (|| {
            let spilled = self.spilled_partitions();
            let released = spilled
                .iter()
                .map(|partition| partition_usage[*partition])
                .sum();
            self.spill_selected_partitions(worker_tables, &spilled, released)
        })();
        self.set_not_spilled();
        result
    }
    /// 构建侧已写出磁盘的总字节。
    pub fn build_spill_bytes(&self) -> i64 {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .build_rows_in_disk
            .iter()
            .flatten()
            .filter_map(Option::as_ref)
            .map(SpillDisk::total_bytes)
            .sum()
    }
    /// 探测侧已写出磁盘的总字节。
    pub fn probe_spill_bytes(&self) -> i64 {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .probe_rows_in_disk
            .iter()
            .flatten()
            .filter_map(Option::as_ref)
            .map(SpillDisk::total_bytes)
            .sum()
    }
    /// 清空磁盘缓冲与 spill 标记，准备新一轮。
    pub fn reset(&self) {
        let mut state = self.state.lock().expect("spill helper poisoned");
        state.build_rows_in_disk = vec![vec![None; self.partition_num]; self.concurrency];
        state.probe_rows_in_disk = vec![vec![None; self.partition_num]; self.concurrency];
        state.spilled_partitions.fill(false);
        state.spill_triggered = false;
        self.spilled_valid_row_num.store(0, Ordering::Release);
    }
    /// 把已 spill 分区压入恢复栈并递增轮次；超最大轮次则报错。
    pub fn prepare_for_restoring(&self, last_round: usize) -> Result<(), String> {
        if last_round + 1 > self.max_spill_round {
            return Err(EXCEED_MAX_SPILL_ROUND_ERROR.into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "spill helper poisoned".to_string())?;
        for partition in 0..self.partition_num {
            if !state.spilled_partitions[partition] {
                continue;
            }
            let mut build_side_chunks = Vec::new();
            let mut probe_side_chunks = Vec::new();
            for worker in 0..self.concurrency {
                if let Some(disk) = &state.build_rows_in_disk[worker][partition] {
                    build_side_chunks.extend(disk.chunks().iter().cloned());
                }
                if let Some(disk) = &state.probe_rows_in_disk[worker][partition] {
                    probe_side_chunks.extend(disk.chunks().iter().cloned());
                }
            }
            if !build_side_chunks.is_empty() {
                state.stack.push(RestorePartition {
                    build_side_chunks,
                    probe_side_chunks,
                    round: last_round + 1,
                });
            }
        }
        state.round = last_round + 1;
        state.build_rows_in_disk = vec![vec![None; self.partition_num]; self.concurrency];
        state.probe_rows_in_disk = vec![vec![None; self.partition_num]; self.concurrency];
        state.spilled_partitions.fill(false);
        state.spill_triggered = false;
        Ok(())
    }
    /// 弹出一个待恢复分区。
    pub fn pop_restore_partition(&self) -> Option<RestorePartition> {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .stack
            .pop()
    }
    /// 已 spill 分区数量。
    pub fn spilled_partition_count(&self) -> usize {
        self.spilled_partitions().len()
    }
    /// 测试：恢复时是否跳过 probe。
    pub fn is_probe_skipped_in_restore_for_test(&self) -> bool {
        self.skip_probe_in_restore_for_test.load(Ordering::Acquire)
    }
    /// 测试：是否发生过多轮（respill）。
    pub fn is_respill_triggered_for_test(&self) -> bool {
        self.state.lock().expect("spill helper poisoned").round > 1
    }
    /// 测试：是否触发过 spill。
    pub fn is_spill_triggered_for_test(&self) -> bool {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spill_triggered_for_test
    }
    /// 测试：是否在构建阶段触发 spill。
    pub fn is_spill_triggered_in_building_stage_for_test(&self) -> bool {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spill_triggered_in_building_stage_for_test
    }
    /// 测试：是否所有分区都 spill。
    pub fn are_all_partitions_spilled_for_test(&self) -> bool {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .all_partitions_spilled_for_test
    }
    /// 测试：是否在建哈希表前触发 spill。
    pub fn is_spill_triggered_before_building_hash_table_for_test(&self) -> bool {
        self.state
            .lock()
            .expect("spill helper poisoned")
            .spill_triggered_before_building_hash_table_for_test
    }
}

/// 估算一行占用的字节数（变长列按长度，其它按 8）。

fn row_size(row: &Row) -> usize {
    row.iter()
        .map(|value| match value {
            crate::row_table_builder::Value::Bytes(value) => value.len(),
            crate::row_table_builder::Value::Text(value) => value.len(),
            _ => 8,
        })
        .sum()
}
