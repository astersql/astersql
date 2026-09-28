// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Hash Join v1 哈希表与构建侧行容器。
//
// 提供单线程 `UnsafeHashTable`、并发 `ConcurrentMapHashTable`，以及承载 chunk、
// NULL 桶与匹配标记的 `HashRowContainer`。对应 Go `hash_table.go` / v1 实现。

// V1 行容器哈希表、NULL-aware bucket、entry store 与并发 map 哈希表。
// 下面按文件顺序保留常量、变量、类型、函数和方法。
// #![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables)]
// use std::any::Any;
// use std::collections::HashMap;
// HashContext keeps the needed hash context of a db table in hash join.
// HashContext 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct HashContext {
// AllTypes one-to-one correspondence with KeyColIdx
//     pub AllTypes: Vec<Option<Box<types::FieldType>>>,
//     pub KeyColIdx: Vec<i32>,
//     pub NaKeyColIdx: Vec<i32>,
//     pub Buf: Vec<u8>,
//     pub HashVals: Vec<hash::Hash64>,
//     pub HasNull: Vec<bool>,
//     pub naHasNull: Vec<bool>,
//     pub naColNullBitMap: Vec<Option<Box<bitmap::ConcurrentBitmap>>>,
// }
// InitHash init HashContext
// InitHash 对应 Go 声明 `func (hc *HashContext) InitHash(rows int) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl HashContext {
//     pub fn InitHash(&mut self, rows: i32) {
//     if self.Buf == None {
//         self.Buf = make([]byte, 1)
//     }
//     if len(self.HashVals) < rows {
//         self.HasNull = make([]bool, rows)
//         self.HashVals = make([]hash.Hash64, rows)
//         for i = range rows {
//             self.HashVals[i] = fnv.New64()
//         }
//     } else {
//         for i = range rows {
//             self.HasNull[i] = false
//             self.HashVals[i].Reset()
//         }
//     }
//     if len(self.NaKeyColIdx) > 0 {
// isNAAJ
//         if len(self.naColNullBitMap) < rows {
//             self.naHasNull = make([]bool, rows)
//             self.naColNullBitMap = make([]*bitmap.ConcurrentBitmap, rows)
//             for i = range rows {
//                 self.naColNullBitMap[i] = bitmap.NewConcurrentBitmap(len(self.NaKeyColIdx))
//             }
//         } else {
//             for i = range rows {
//                 self.naHasNull[i] = false
//                 self.naColNullBitMap[i].Reset(len(self.NaKeyColIdx))
//             }
//         }
//     }
// }
// }
// hashNANullBucket 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct hashNANullBucket {
//     pub entries: Vec<Option<Box<naEntry>>>,
// }
// hashRowContainer handles the rows and the hash map of a table.
// NOTE: a hashRowContainer may be shallow copied by the invoker, define all the
// member attributes as pointer type to avoid unexpected problems.
// hashRowContainer 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct hashRowContainer {
//     pub sc: Option<Box<stmtctx::StatementContext>>,
//     pub hCtx: Option<Box<HashContext>>,
//     pub stat: Option<Box<hashStatistic>>,
// hashTable stores the map of hashKey and RowPtr
//     pub hashTable: BaseHashTable,
// hashNANullBucket stores the rows with any null value in NAAJ join key columns.
// After build process, NANUllBucket is read only here for multi probe worker.
//     pub hashNANullBucket: Option<Box<hashNANullBucket>>,
//     pub rowContainer: Option<Box<chunk::RowContainer>>,
//     pub memTracker: Option<Box<memory::Tracker>>,
// chkBuf buffer the data reads from the disk if rowContainer is spilled.
//     pub chkBuf: Option<Box<chunk::Chunk>>,
//     pub chkBufSizeForOneProbe: i64,
// }
// newHashRowContainer 对应 Go 声明 `func newHashRowContainer(sCtx sessionctx.Context, hCtx *HashContext, allTypes []*types.FieldType) *hashRowContainer {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn newHashRowContainer(sCtx: sessionctx::Context, hCtx: Option<Box<HashContext>>, allTypes: Vec<Option<Box<types::FieldType>>>) -> Option<Box<hashRowContainer>> {
//     maxChunkSize = sCtx.GetSessionVars().MaxChunkSize
//     rc = chunk.NewRowContainer(allTypes, maxChunkSize)
//     c = &hashRowContainer{
//         sc:           sCtx.GetSessionVars().StmtCtx,
//         hCtx:         hCtx,
//         stat:         new(hashStatistic),
//         hashTable:    NewConcurrentMapHashTable(),
//         rowContainer: rc,
//         memTracker:   memory.NewTracker(memory.LabelForRowContainer, -1),
//     }
//     if isNAAJ = len(hCtx.NaKeyColIdx) > 0; isNAAJ {
//         c.hashNANullBucket = &hashNANullBucket{}
//     }
//     rc.GetMemTracker().AttachTo(c.GetMemTracker())
//     return c
// }
// ShallowCopy 对应 Go 声明 `func (c *hashRowContainer) ShallowCopy() *hashRowContainer {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn ShallowCopy(&mut self) -> Option<Box<hashRowContainer>> {
//     newHRC = *c
//     newHRC.rowContainer = self.rowContainer.ShallowCopyWithNewMutex()
// multi hashRowContainer ref to one single NA-NULL bucket slice.
// newHRC.hashNANullBucket = self.hashNANullBucket
//     return &newHRC
// }
// }
// GetMatchedRows get matched rows from probeRow. It can be called
// in multiple goroutines while each goroutine should keep its own
// h and buf.
// GetMatchedRows 对应 Go 声明 `func (c *hashRowContainer) GetMatchedRows(probeKey uint64, probeRow chunk.Row, hCtx *HashContext, matched []chunk.Row) ([]chunk.Row, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetMatchedRows(&mut self, probeKey: u64, probeRow: chunk::Row, hCtx: Option<Box<HashContext>>, matched: Vec<chunk::Row>) -> (Vec<chunk::Row>, errors::Error) {
//     matchedRows, _, err = self.GetMatchedRowsAndPtrs(probeKey, probeRow, hCtx, matched, None, false)
//     return matchedRows, err
// }
// }
// GetOneMatchedRow get one matched rows from probeRow.
// GetOneMatchedRow 对应 Go 声明 `func (c *hashRowContainer) GetOneMatchedRow(probeKey uint64, probeRow chunk.Row, hCtx *HashContext) (*chunk.Row, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetOneMatchedRow(&mut self, probeKey: u64, probeRow: chunk::Row, hCtx: Option<Box<HashContext>>) -> (Option<Box<chunk::Row>>, errors::Error) {
//     let mut err: errors::Error = Default::default()
//     innerEntry = self.hashTable.Get(probeKey)
//     if innerEntry == None {
//         return None, err
//     }
//     let mut matchedRow: chunk::Row = Default::default()
//     if self.chkBuf != None {
//         self.chkBuf.Reset()
//     }
//     capacity = 0
//     for i = 0; innerEntry != None; i, innerEntry = i+1, innerEntry.Next {
//         ptr = innerEntry.Ptr
//         matchedRow, self.chkBuf, err = self.rowContainer.GetRowAndAppendToChunkIfInDisk(ptr, self.chkBuf)
//         if err != None {
//             return None, err
//         }
//         let mut ok: bool = Default::default()
//         ok, err = self.matchJoinKey(matchedRow, probeRow, hCtx)
//         if err != None {
//             return None, err
//         }
//         if ok {
//             return &matchedRow, None
//         }
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//         atomic.AddInt64(&self.stat.probeCollision, 1)
//         if i == 0 {
//             capacity = max(self.chkBuf.Capacity(), 128)
//         } else if (i+1)%capacity == 0 {
//             self.chkBuf.Reset()
//         }
//     }
//     return None, err
// }
// }
// GetAllMatchedRows 对应 Go 声明 `func (c *hashRowContainer) GetAllMatchedRows(probeHCtx *HashContext, probeSideRow chunk.Row, probeKeyNullBits *bitmap.ConcurrentBitmap, matched []chunk.Row, needCheckBuildColPos, needCheckProbeColPos []int, needCheckBuildTypes, needCheckProbeTypes []*types.FieldType) ([]chunk.Row, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetAllMatchedRows(&mut self, probeHCtx: Option<Box<HashContext>>, probeSideRow: chunk::Row, probeKeyNullBits: Option<Box<bitmap::ConcurrentBitmap>>, matched: Vec<chunk::Row>, needCheckBuildColPos: Vec<chunk::Row>, needCheckProbeColPos: Vec<i32>, needCheckBuildTypes: Vec<i32>, needCheckProbeTypes: Vec<Option<Box<types::FieldType>>>) -> (Vec<chunk::Row>, errors::Error) {
// for NAAJ probe row with null, we should match them with all build rows.
//     var (
//         ok        bool
//         err       error
//         innerPtrs []chunk.RowPtr
//     )
//     self.hashTable.Iter(
//         func(_ uint64, e *entry) {
//             entryAddr = e
//             for entryAddr != None {
//                 innerPtrs = append(innerPtrs, entryAddr.Ptr)
//                 entryAddr = entryAddr.Next
//             }
//         })
//     matched = matched[:0]
//     if len(innerPtrs) == 0 {
//         return matched, None
//     }
// all built bucket rows come from hash table, their bitmap are all None (doesn't contain any null). so
// we could only use the probe null bits to filter valid rows.
//     if probeKeyNullBits != None && len(probeHCtx.NaKeyColIdx) > 1 {
// if len(probeHCtx.NaKeyColIdx)=1
//     that means the NA-Join probe key is directly a (null) <-> (fetch all buckets), nothing to do.
// else like
//	   (null, 1, 2), we should use the not-null probe bit to filter rows. Only fetch rows like
// ( ? , 1, 2), that exactly with value as 1 and 2 in the second and third join key column.
//         needCheckProbeColPos = needCheckProbeColPos[:0]
//         needCheckBuildColPos = needCheckBuildColPos[:0]
//         needCheckBuildTypes = needCheckBuildTypes[:0]
//         needCheckProbeTypes = needCheckProbeTypes[:0]
//         keyColLen = len(self.hCtx.NaKeyColIdx)
//         for i = range keyColLen {
// since all bucket is from hash table (Not Null), so the buildSideNullBits check is eliminated.
//             if probeKeyNullBits.UnsafeIsSet(i) {
//                 continue
//             }
//             needCheckBuildColPos = append(needCheckBuildColPos, self.hCtx.NaKeyColIdx[i])
//             needCheckBuildTypes = append(needCheckBuildTypes, self.hCtx.AllTypes[i])
//             needCheckProbeColPos = append(needCheckProbeColPos, probeHCtx.NaKeyColIdx[i])
//             needCheckProbeTypes = append(needCheckProbeTypes, probeHCtx.AllTypes[i])
//         }
//     }
//     let mut mayMatchedRow: chunk::Row = Default::default()
//     for _, ptr = range innerPtrs {
//         mayMatchedRow, self.chkBuf, err = self.rowContainer.GetRowAndAppendToChunkIfInDisk(ptr, self.chkBuf)
//         if err != None {
//             return None, err
//         }
//         if probeKeyNullBits != None && len(probeHCtx.NaKeyColIdx) > 1 {
// check the idxs-th value of the join columns.
//             ok, err = codec.EqualChunkRow(self.sc.TypeCtx(), mayMatchedRow, needCheckBuildTypes, needCheckBuildColPos, probeSideRow, needCheckProbeTypes, needCheckProbeColPos)
//             if err != None {
//                 return None, err
//             }
//             if !ok {
//                 continue
//             }
// once ok. just append the (maybe) valid build row for latter other conditions check if any.
//         }
//         matched = append(matched, mayMatchedRow)
//     }
//     return matched, None
// }
// }
// signalCheckpointForJoinMask indicates the times of row probe that a signal detection will be triggered.
// 常量声明对应 Go const；保留原始取值和分组顺序。
// const signalCheckpointForJoinMask int = 1<<17 - 1
// rowSize is the size of Row.
// 常量声明对应 Go const；保留原始取值和分组顺序。
// const rowSize = int64(unsafe.Sizeof(chunk.Row{}))
// rowPtrSize is the size of RowPtr.
// 常量声明对应 Go const；保留原始取值和分组顺序。
// const rowPtrSize = int64(unsafe.Sizeof(chunk.RowPtr{}))
// GetMatchedRowsAndPtrs get matched rows and Ptrs from probeRow. It can be called
// in multiple goroutines while each goroutine should keep its own
// h and buf.
// GetMatchedRowsAndPtrs 对应 Go 声明 `func (c *hashRowContainer) GetMatchedRowsAndPtrs(probeKey uint64, probeRow chunk.Row, hCtx *HashContext, matched []chunk.Row, matchedPtrs []chunk.RowPtr, needPtr bool) ([]chunk.Row, []chunk.RowPtr, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetMatchedRowsAndPtrs(&mut self, probeKey: u64, probeRow: chunk::Row, hCtx: Option<Box<HashContext>>, matched: Vec<chunk::Row>, matchedPtrs: Vec<chunk::RowPtr>, needPtr: bool) -> (Vec<chunk::Row>, Vec<chunk::RowPtr>, errors::Error) {
//     let mut err: errors::Error = Default::default()
//     entry = self.hashTable.Get(probeKey)
//     let mut innerPtrs: Vec<chunk::RowPtr> = Default::default()
//     for ; entry != None; entry = entry.Next {
//         innerPtrs = append(innerPtrs, entry.Ptr)
//     }
//     if len(innerPtrs) == 0 {
//         return None, None, err
//     }
//     matched = matched[:0]
//     let mut matchedRow: chunk::Row = Default::default()
//     matchedPtrs = matchedPtrs[:0]
// Some variables used for memTracker.
//     var (
//         matchedDataSize     = int64(cap(matched))*rowSize + int64(cap(matchedPtrs))*rowPtrSize
//         needTrackMemUsage   = cap(innerPtrs) > signalCheckpointForJoinMask
//         lastChunkBufPointer = self.chkBuf
//         memDelta            int64
//     )
//     self.memTracker.Consume(-self.chkBufSizeForOneProbe)
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() { self.memTracker.Consume(memDelta) }()
//     if needTrackMemUsage {
//         self.memTracker.Consume(int64(cap(innerPtrs)) * rowPtrSize)
//         defer self.memTracker.Consume(-int64(cap(innerPtrs)) * rowPtrSize)
//     }
//     self.chkBufSizeForOneProbe = 0
//     for i, ptr = range innerPtrs {
//         matchedRow, self.chkBuf, err = self.rowContainer.GetRowAndAppendToChunkIfInDisk(ptr, self.chkBuf)
//         if err != None {
//             return None, None, err
//         }
//         let mut ok: bool = Default::default()
//         ok, err = self.matchJoinKey(matchedRow, probeRow, hCtx)
//         if err != None {
//             return None, None, err
//         }
//         if self.chkBuf != lastChunkBufPointer && lastChunkBufPointer != None {
//             lastChunkSize = lastChunkBufPointer.MemoryUsage()
//             self.chkBufSizeForOneProbe += lastChunkSize
//             memDelta += lastChunkSize
//         }
//         lastChunkBufPointer = self.chkBuf
//         if needTrackMemUsage && (i&signalCheckpointForJoinMask == signalCheckpointForJoinMask) {
// Trigger Consume for checking the OOM Action signal
//             memDelta += int64(cap(matched))*rowSize + int64(cap(matchedPtrs))*rowPtrSize - matchedDataSize
//             matchedDataSize = int64(cap(matched))*rowSize + int64(cap(matchedPtrs))*rowPtrSize
//             self.memTracker.Consume(memDelta + 1)
//             memDelta = 0
//         }
//         if !ok {
// 原 Go 使用 atomic 保证并发可见性；Rust 后续应换成对应原子类型或锁。
//             atomic.AddInt64(&self.stat.probeCollision, 1)
//             continue
//         }
//         matched = append(matched, matchedRow)
//         if needPtr {
//             matchedPtrs = append(matchedPtrs, ptr)
//         }
//     }
//     return matched, matchedPtrs, err
// }
// }
// GetNullBucketRows 对应 Go 声明 `func (c *hashRowContainer) GetNullBucketRows(probeHCtx *HashContext, probeSideRow chunk.Row, probeKeyNullBits *bitmap.ConcurrentBitmap, matched []chunk.Row, needCheckBuildColPos, needCheckProbeColPos []int, needCheckBuildTypes, needCheckProbeTypes []*types.FieldType) ([]chunk.Row, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetNullBucketRows(&mut self, probeHCtx: Option<Box<HashContext>>, probeSideRow: chunk::Row, probeKeyNullBits: Option<Box<bitmap::ConcurrentBitmap>>, matched: Vec<chunk::Row>, needCheckBuildColPos: Vec<chunk::Row>, needCheckProbeColPos: Vec<i32>, needCheckBuildTypes: Vec<i32>, needCheckProbeTypes: Vec<Option<Box<types::FieldType>>>) -> (Vec<chunk::Row>, errors::Error) {
//     var (
//         ok            bool
//         err           error
//         mayMatchedRow chunk.Row
//     )
//     matched = matched[:0]
//     for _, nullEntry = range self.hashNANullBucket.entries {
//         mayMatchedRow, self.chkBuf, err = self.rowContainer.GetRowAndAppendToChunkIfInDisk(nullEntry.ptr, self.chkBuf)
//         if err != None {
//             return None, err
//         }
// since null bucket is a unified bucket. cases like below:
// case1: left side (probe side) has null
//    left side key <1,null>, actually we can fetch all bucket <1, ?> and filter 1 at the first join key, once
//    got a valid right row after other condition, then we can just return.
// case2: left side (probe side) don't have null
//    left side key <1, 2>, actually we should fetch <1,null>, <null, 2>, <null, null> from the null bucket because
//    case like <3,null> is obviously not matched with the probe key.
//         needCheckProbeColPos = needCheckProbeColPos[:0]
//         needCheckBuildColPos = needCheckBuildColPos[:0]
//         needCheckBuildTypes = needCheckBuildTypes[:0]
//         needCheckProbeTypes = needCheckProbeTypes[:0]
//         keyColLen = len(self.hCtx.NaKeyColIdx)
//         if probeKeyNullBits != None {
// when the probeKeyNullBits is not None, it means the probe key has null values, where we should distinguish
// whether is empty set or not. In other words, we should fetch at least a valid from the null bucket here.
// for values at the same index of the join key in which they are both not null, the values should be exactly the same.
// step: probeKeyNullBits & buildKeyNullBits, for those bits with 0, we should check if both values are the same.
// we can just use the UnsafeIsSet here, because insert action of the build side has all finished.
// 1 0 1 0 means left join key : null ? null ?
// 1 0 0 0 means right join key : null ? ? ?
// ---------------------------------------------
// left & right: 1 0 1 0: just do the explicit column value check for whose bit is 0. (means no null from both side)
//             for i = range keyColLen {
//                 if probeKeyNullBits.UnsafeIsSet(i) || nullEntry.nullBitMap.UnsafeIsSet(i) {
//                     continue
//                 }
//                 needCheckBuildColPos = append(needCheckBuildColPos, self.hCtx.NaKeyColIdx[i])
//                 needCheckBuildTypes = append(needCheckBuildTypes, self.hCtx.AllTypes[i])
//                 needCheckProbeColPos = append(needCheckProbeColPos, probeHCtx.NaKeyColIdx[i])
//                 needCheckProbeTypes = append(needCheckProbeTypes, probeHCtx.AllTypes[i])
//             }
// check the idxs-th value of the join columns.
//             ok, err = codec.EqualChunkRow(self.sc.TypeCtx(), mayMatchedRow, needCheckBuildTypes, needCheckBuildColPos, probeSideRow, needCheckProbeTypes, needCheckProbeColPos)
//             if err != None {
//                 return None, err
//             }
//             if !ok {
//                 continue
//             }
//         } else {
// when the probeKeyNullBits is None, it means the probe key is not null. But in the process of matching the null bucket,
// we still need to do the non-null (explicit) value check.
// eg: the probe key is <1,2>, we only get <2, null> in the null bucket, even we can take the null as a wildcard symbol,
// the first value of this two tuple is obviously not a match. So we need filter it here.
//             for i = range keyColLen {
//                 if nullEntry.nullBitMap.UnsafeIsSet(i) {
//                     continue
//                 }
//                 needCheckBuildColPos = append(needCheckBuildColPos, self.hCtx.NaKeyColIdx[i])
//                 needCheckBuildTypes = append(needCheckBuildTypes, self.hCtx.AllTypes[i])
//                 needCheckProbeColPos = append(needCheckProbeColPos, probeHCtx.NaKeyColIdx[i])
//                 needCheckProbeTypes = append(needCheckProbeTypes, probeHCtx.AllTypes[i])
//             }
// check the idxs-th value of the join columns.
//             ok, err = codec.EqualChunkRow(self.sc.TypeCtx(), mayMatchedRow, needCheckBuildTypes, needCheckBuildColPos, probeSideRow, needCheckProbeTypes, needCheckProbeColPos)
//             if err != None {
//                 return None, err
//             }
//             if !ok {
//                 continue
//             }
//         }
// once ok. just append the (maybe) valid build row for latter other conditions check if any.
//         matched = append(matched, mayMatchedRow)
//     }
//     return matched, err
// }
// }
// matchJoinKey checks if join keys of buildRow and probeRow are logically equal.
// matchJoinKey 对应 Go 声明 `func (c *hashRowContainer) matchJoinKey(buildRow, probeRow chunk.Row, probeHCtx *HashContext) (ok bool, err error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn matchJoinKey(&mut self, buildRow: _, probeRow: chunk::Row, probeHCtx: Option<Box<HashContext>>) -> (bool, errors::Error) {
//     if len(self.hCtx.NaKeyColIdx) > 0 {
//         return codec.EqualChunkRow(self.sc.TypeCtx(),
//             buildRow, self.hCtx.AllTypes, self.hCtx.NaKeyColIdx,
//             probeRow, probeHCtx.AllTypes, probeHCtx.NaKeyColIdx)
//     }
//     return codec.EqualChunkRow(self.sc.TypeCtx(),
//         buildRow, self.hCtx.AllTypes, self.hCtx.KeyColIdx,
//         probeRow, probeHCtx.AllTypes, probeHCtx.KeyColIdx)
// }
// }
// AlreadySpilledSafeForTest indicates that records have spilled out into disk. It's thread-safe.
// nolint: unused
// AlreadySpilledSafeForTest 对应 Go 声明 `func (c *hashRowContainer) AlreadySpilledSafeForTest() bool {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn AlreadySpilledSafeForTest(&mut self) -> bool {
//     return self.rowContainer.AlreadySpilledSafeForTest()
// }
// }
// PutChunk puts a chunk into hashRowContainer and build hash map. It's not thread-safe.
// key of hash table: hash value of key columns
// value of hash table: RowPtr of the corresponded row
// PutChunk 对应 Go 声明 `func (c *hashRowContainer) PutChunk(chk *chunk.Chunk, ignoreNulls []bool) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn PutChunk(&mut self, chk: Option<Box<chunk::Chunk>>, ignoreNulls: Vec<bool>) -> Result<(), errors::Error> {
//     return self.PutChunkSelected(chk, None, ignoreNulls)
// }
// }
// PutChunkSelected selectively puts a chunk into hashRowContainer and build hash map. It's not thread-safe.
// key of hash table: hash value of key columns
// value of hash table: RowPtr of the corresponded Row
// PutChunkSelected 对应 Go 声明 `func (c *hashRowContainer) PutChunkSelected(chk *chunk.Chunk, selected, ignoreNulls []bool) error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn PutChunkSelected(&mut self, chk: Option<Box<chunk::Chunk>>, selected: Option<Box<chunk::Chunk>>, ignoreNulls: Vec<bool>) -> Result<(), errors::Error> {
//     start = time.Now()
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer func() { self.stat.buildTableElapse += time.Since(start) }()
//     chkIdx = uint32(self.rowContainer.NumChunks())
//     err = self.rowContainer.Add(chk)
//     if err != None {
//         return err
//     }
//     numRows = chk.NumRows()
//     self.hCtx.InitHash(numRows)
//     hCtx = self.hCtx
// By now, the combination of 1 and 2 can't take a run at same time.
// 1: write the row data of join key to hashVals. (normal EQ key should ignore the null values.) null-EQ for Except statement is an exception.
//     for keyIdx, colIdx = range self.hCtx.KeyColIdx {
//         ignoreNull = len(ignoreNulls) > keyIdx && ignoreNulls[keyIdx]
//         err = codec.HashChunkSelected(self.sc.TypeCtx(), hCtx.HashVals, chk, hCtx.AllTypes[keyIdx], colIdx, hCtx.Buf, hCtx.HasNull, selected, ignoreNull)
//         if err != None {
//             return errors.Trace(err)
//         }
//     }
// 2: write the row data of NA join key to hashVals. (NA EQ key should collect all rows including null value as one bucket.)
//     isNAAJ = len(self.hCtx.NaKeyColIdx) > 0
//     hasNullMark = make([]bool, len(hCtx.HasNull))
//     for keyIdx, colIdx = range self.hCtx.NaKeyColIdx {
// NAAJ won't ignore any null values, but collect them as one hash bucket.
//         err = codec.HashChunkSelected(self.sc.TypeCtx(), hCtx.HashVals, chk, hCtx.AllTypes[keyIdx], colIdx, hCtx.Buf, hCtx.HasNull, selected, false)
//         if err != None {
//             return errors.Trace(err)
//         }
// todo: we can collect the bitmap in codec.HashChunkSelected to avoid loop here, but the params modification is quite big.
// after fetch one NA column, collect the null value to null bitmap for every row. (use hasNull flag to accelerate)
// eg: if a NA Join cols is (a, b, c), for every build row here we maintained a 3-bit map to mark which column are null for them.
//         for rowIdx = range numRows {
//             if hCtx.HasNull[rowIdx] {
//                 hCtx.naColNullBitMap[rowIdx].UnsafeSet(keyIdx)
// clean and try fetch Next NA join col.
//                 hCtx.HasNull[rowIdx] = false
// just a mark variable for whether there is a null in at least one NA join column.
//                 hasNullMark[rowIdx] = true
//             }
//         }
//     }
//     for i = range numRows {
//         if isNAAJ {
//             if selected != None && !selected[i] {
//                 continue
//             }
//             if hasNullMark[i] {
// collect the null rows to slice.
//                 rowPtr = chunk.RowPtr{ChkIdx: chkIdx, RowIdx: uint32(i)}
// do not directly ref the null bits map here, because the bit map will be reset and reused in next batch of chunk data.
//                 self.hashNANullBucket.entries = append(self.hashNANullBucket.entries, &naEntry{rowPtr, self.hCtx.naColNullBitMap[i].Clone()})
//             } else {
// insert the not-null rows to hash table.
//                 key = self.hCtx.HashVals[i].Sum64()
//                 rowPtr = chunk.RowPtr{ChkIdx: chkIdx, RowIdx: uint32(i)}
//                 self.hashTable.Put(key, rowPtr)
//             }
//         } else {
//             if (selected != None && !selected[i]) || self.hCtx.HasNull[i] {
//                 continue
//             }
//             key = self.hCtx.HashVals[i].Sum64()
//             rowPtr = chunk.RowPtr{ChkIdx: chkIdx, RowIdx: uint32(i)}
//             self.hashTable.Put(key, rowPtr)
//         }
//     }
//     self.GetMemTracker().Consume(self.hashTable.GetAndCleanMemoryDelta())
//     return None
// }
// }
// NumChunks returns the number of chunks in the RowContainer
// NumChunks 对应 Go 声明 `func (c *hashRowContainer) NumChunks() int {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn NumChunks(&mut self) -> i32 {
//     return self.rowContainer.NumChunks()
// }
// }
// NumRowsOfChunk returns the number of rows of a chunk
// NumRowsOfChunk 对应 Go 声明 `func (c *hashRowContainer) NumRowsOfChunk(chkID int) int {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn NumRowsOfChunk(&mut self, chkID: i32) -> i32 {
//     return self.rowContainer.NumRowsOfChunk(chkID)
// }
// }
// GetChunk returns chkIdx th chunk of in memory records, only works if RowContainer is not spilled
// GetChunk 对应 Go 声明 `func (c *hashRowContainer) GetChunk(chkIdx int) (*chunk.Chunk, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetChunk(&mut self, chkIdx: i32) -> (Option<Box<chunk::Chunk>>, errors::Error) {
//     return self.rowContainer.GetChunk(chkIdx)
// }
// }
// GetRow returns the Row the Ptr pointed to in the RowContainer
// GetRow 对应 Go 声明 `func (c *hashRowContainer) GetRow(ptr chunk.RowPtr) (chunk.Row, error) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetRow(&mut self, ptr: chunk::RowPtr) -> (chunk::Row, errors::Error) {
//     return self.rowContainer.GetRow(ptr)
// }
// }
// Len returns number of records in the hash table.
// Len 对应 Go 声明 `func (c *hashRowContainer) Len() uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn Len(&mut self) -> u64 {
//     return self.hashTable.Len()
// }
// }
// Close 对应 Go 声明 `func (c *hashRowContainer) Close() error {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn Close(&mut self) -> Result<(), errors::Error> {
//     failpoint.Inject("issue60926", None)
// Go defer 的资源收尾/统计注册语义在此保留为迁移线索，后续需接入 Rust Drop 或显式收尾。
//     defer self.memTracker.Detach()
//     self.chkBuf = None
//     return self.rowContainer.Close()
// }
// }
// GetMemTracker returns the underlying memory usage tracker in hashRowContainer.
// GetMemTracker 对应 Go 声明 `func (c *hashRowContainer) GetMemTracker() *memory.Tracker { return c.memTracker }`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetMemTracker(&mut self) -> Option<Box<memory::Tracker { return c::memTracker }>> {
// }
// GetDiskTracker returns the underlying disk usage tracker in hashRowContainer.
// GetDiskTracker 对应 Go 声明 `func (c *hashRowContainer) GetDiskTracker() *disk.Tracker { return c.rowContainer.GetDiskTracker() }`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn GetDiskTracker(&mut self) -> Option<Box<disk::Tracker { return c::rowContainer::GetDiskTracker() }>> {
// }
// ActionSpill returns a memory.ActionOnExceed for spilling over to disk.
// ActionSpill 对应 Go 声明 `func (c *hashRowContainer) ActionSpill() memory.ActionOnExceed {`；保留原控制流、错误处理和外部依赖调用形状。
// impl hashRowContainer {
//     pub fn ActionSpill(&mut self) -> memory::ActionOnExceed {
//     return self.rowContainer.ActionSpill()
// }
// }
// 常量声明对应 Go const；保留原始取值和分组顺序。
// const (
//     initialEntrySliceLen = 64
//     maxEntrySliceLen     = 8192
// )
// entry 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct entry {
//     pub Ptr: chunk::RowPtr,
//     pub Next: Option<Box<entry>>,
// }
// naEntry 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct naEntry {
//     pub ptr: chunk::RowPtr,
//     pub nullBitMap: Option<Box<bitmap::ConcurrentBitmap>>,
// }
// entryStore 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct entryStore {
//     pub slices: Vec<Vec<entry>>,
//     pub cursor: i32,
// }
// newEntryStore 对应 Go 声明 `func newEntryStore() *entryStore {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn newEntryStore() -> Option<Box<entryStore>> {
//     es = new(entryStore)
//     es.slices = [][]entry{make([]entry, initialEntrySliceLen)}
//     es.cursor = 0
//     return es
// }
// GetStore 对应 Go 声明 `func (es *entryStore) GetStore() (e *entry, memDelta int64) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl entryStore {
//     pub fn GetStore(&mut self) -> (Option<Box<entry>>, i64) {
//     sliceIdx = uint32(len(self.slices) - 1)
//     slice = self.slices[sliceIdx]
//     if self.cursor >= cap(slice) {
//         size = min(cap(slice)*2, maxEntrySliceLen)
//         slice = make([]entry, size)
//         self.slices = append(self.slices, slice)
//         sliceIdx++
//         self.cursor = 0
// unsafe 指针/地址操作来自 Go 实现，只保留数据流并提示后续审查所有权。
//         memDelta = int64(unsafe.Sizeof(entry{})) * int64(size)
//     }
//     e = &self.slices[sliceIdx][self.cursor]
//     self.cursor++
//     return
// }
// }
// BaseHashTable is the interface of the hash table used in hash join
// BaseHashTable 对应 Go interface；方法签名保持为 trait ，具体动态分发后续接线。
// pub trait BaseHashTable {
//     fn Put(hashKey: u64, rowPtr: chunk::RowPtr);
// e := Get(hashKey)
// for ; e != nil; e = e.Next {
//    rowPtr := e.Ptr
//    ...
// }
//     fn Get(hashKey: u64) -> Option<Box<entry>>;
//     fn Len() -> u64;
// GetAndCleanMemoryDelta gets and cleans the memDelta of the BaseHashTable. Memory delta will be cleared after each fetch.
// It indicates the memory delta of the BaseHashTable since the last calling GetAndCleanMemoryDelta().
//     fn GetAndCleanMemoryDelta() -> i64;
//     fn Iter(func(uint64: Option<Box<entry)>>);
// }
// TODO (fangzhuhe) remove unsafeHashTable later if it not used anymore
// unsafeHashTable stores multiple rowPtr of rows for a given key with minimum GC overhead.
// A given key can store multiple values.
// It is not thread-safe, should only be used in one goroutine.
// unsafeHashTable 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct unsafeHashTable {
//     pub entryStore: Option<Box<entryStore>>,
//     pub length: u64,
//     pub hashMap hack.MemAwareMap[uint64: Option<Box<entry]>>,
//     pub memDelta int64 // the memory delta of the unsafeHashTable since the last calling: GetAndCleanMemoryDelta(),
// }
// newUnsafeHashTable creates a new unsafeHashTable. estCount means the estimated size of the hashMap.
// If unknown, set it to 0.
// newUnsafeHashTable 对应 Go 声明 `func newUnsafeHashTable(estCount int) *unsafeHashTable {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn newUnsafeHashTable(estCount: i32) -> Option<Box<unsafeHashTable>> {
//     ht = &unsafeHashTable{}
//     ht.hashMap.Init(make(map[uint64]*entry, estCount))
//     ht.entryStore = newEntryStore()
//     return ht
// }
// Put puts the key/rowPtr pairs to the unsafeHashTable, multiple rowPtrs are stored in a list.
// Put 对应 Go 声明 `func (ht *unsafeHashTable) Put(hashKey uint64, rowPtr chunk.RowPtr) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl unsafeHashTable {
//     pub fn Put(&mut self, hashKey: u64, rowPtr: chunk::RowPtr) {
//     oldEntry = self.hashMap.M[hashKey]
//     newEntry, memDelta = self.entryStore.GetStore()
//     newEntry.Ptr = rowPtr
//     newEntry.Next = oldEntry
//     memDelta += self.hashMap.Set(hashKey, newEntry)
//     self.length++
//     self.memDelta += memDelta
// }
// }
// Get gets the values of the "key" and appends them to "values".
// Get 对应 Go 声明 `func (ht *unsafeHashTable) Get(hashKey uint64) *entry {`；保留原控制流、错误处理和外部依赖调用形状。
// impl unsafeHashTable {
//     pub fn Get(&mut self, hashKey: u64) -> Option<Box<entry>> {
//     entryAddr = self.hashMap.M[hashKey]
//     return entryAddr
// }
// }
// Len returns the number of rowPtrs in the unsafeHashTable, the number of keys may be less than Len
// if the same key is put more than once.
// Len 对应 Go 声明 `func (ht *unsafeHashTable) Len() uint64 { return ht.length }`；保留原控制流、错误处理和外部依赖调用形状。
// impl unsafeHashTable {
//     pub fn Len(&mut self) -> uint64 { return ht::length } {
// }
// GetAndCleanMemoryDelta gets and cleans the memDelta of the unsafeHashTable.
// GetAndCleanMemoryDelta 对应 Go 声明 `func (ht *unsafeHashTable) GetAndCleanMemoryDelta() int64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl unsafeHashTable {
//     pub fn GetAndCleanMemoryDelta(&mut self) -> i64 {
//     memDelta = self.memDelta
//     self.memDelta = 0
//     return memDelta
// }
// }
// Iter 对应 Go 声明 `func (ht *unsafeHashTable) Iter(traverse func(key uint64, e *entry)) {`；保留原控制流、错误处理和外部依赖调用形状。
// impl unsafeHashTable {
//     pub fn Iter(&mut self, traverse func(key uint64: Option<Box<entry)>>, e: Option<Box<entry)>>) {
//     for k, entryAddr = range self.hashMap.M {
//         traverse(k, entryAddr)
//     }
// }
// }
// concurrentMapHashTable is a concurrent hash table built on concurrentMap
// concurrentMapHashTable 对应 Go 的同名 struct；字段顺序、嵌入字段和指针形状按源码保留。
// pub struct concurrentMapHashTable {
//     pub hashMap: concurrentMap,
//     pub entryStore: Option<Box<entryStore>>,
//     pub length: u64,
//     pub memDelta int64 // the memory delta of the concurrentMapHashTable since the last calling: GetAndCleanMemoryDelta(),
// }
// NewConcurrentMapHashTable creates a concurrentMapHashTable
// NewConcurrentMapHashTable 对应 Go 声明 `func NewConcurrentMapHashTable() *concurrentMapHashTable {`；保留原控制流、错误处理和外部依赖调用形状。
// pub fn NewConcurrentMapHashTable() -> Option<Box<concurrentMapHashTable>> {
//     ht = &concurrentMapHashTable{}
//     ht.hashMap = newConcurrentMap()
//     ht.entryStore = newEntryStore()
//     ht.length = 0
// unsafe 指针/地址操作来自 Go 实现，只保留数据流并提示后续审查所有权。
//     ht.memDelta = int64(unsafe.Sizeof(concurrentMapHashTable{})) + int64(len(ht.hashMap))*int64((unsafe.Sizeof(concurrentMapShared{})))
//     for _, m = range ht.hashMap {
//         ht.memDelta += int64(m.items.Bytes)
//     }
//     ht.memDelta += int64(unsafe.Sizeof(entryStore{})) + int64(unsafe.Sizeof(entry{}))*initialEntrySliceLen
//     return ht
// }
// Len return the number of rowPtrs in the concurrentMapHashTable
// Len 对应 Go 声明 `func (ht *concurrentMapHashTable) Len() uint64 {`；保留原控制流、错误处理和外部依赖调用形状。
// impl concurrentMapHashTable {
//     pub fn Len(&mut self) -> u64 {
//     return self.length
// }
// }
// */
use crate::concurrent_map::ConcurrentMap;
use crate::joiner::Row;
use crate::row_table_builder::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

/// 连接键哈希上下文：列下标、每行哈希值、NULL 位图。

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashContext {
    pub key_indices: Vec<usize>,
    pub hash_values: Vec<u64>,
    pub has_null: Vec<bool>,
    pub null_bits: Vec<u64>,
}

impl HashContext {
    /// 用连接键列下标构造空上下文。
    pub fn new(key_indices: Vec<usize>) -> Self {
        Self {
            key_indices,
            ..Self::default()
        }
    }

    /// 对一批行编码连接键并计算哈希；记录每行是否含 NULL 及 null_bits。

    pub fn init_hash(&mut self, rows: &[Row]) -> Result<(), String> {
        self.hash_values.clear();
        self.has_null.clear();
        self.null_bits.clear();
        for row in rows {
            let mut encoded = Vec::new();
            let mut null_bits = 0_u64;
            let mut has_null = false;
            for (key_pos, index) in self.key_indices.iter().copied().enumerate() {
                let value = row
                    .get(index)
                    .ok_or_else(|| format!("join key index {index} is out of bounds"))?;
                if matches!(value, Value::Null) {
                    has_null = true;
                    if key_pos < 64 {
                        null_bits |= 1_u64 << key_pos;
                    }
                }
                encode_value(value, &mut encoded);
            }
            self.hash_values.push(hash_bytes(&encoded));
            self.has_null.push(has_null);
            self.null_bits.push(null_bits);
        }
        Ok(())
    }
}

/// 行在 chunk 数组中的位置（chunk 下标 + 行下标）。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RowPointer {
    pub chunk_index: usize,
    pub row_index: usize,
}

/// Null-aware join 用的 NULL 键桶：存放含 NULL 的构建侧行。

#[derive(Clone, Debug, Default)]
pub struct HashNaNullBucket {
    rows: Vec<(u64, RowPointer)>,
}

impl HashNaNullBucket {
    /// 放入条目（具体类型见实现）。
    pub fn put(&mut self, null_bits: u64, pointer: RowPointer) {
        self.rows.push((null_bits, pointer));
    }
    /// 元素个数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// 哈希表冲突链节点：行指针与下一节点。

#[derive(Clone, Debug)]
pub struct Entry {
    pub pointer: RowPointer,
    pub next: Option<Box<Entry>>,
}

/// Null-aware 条目：null_bits 与行指针。

#[derive(Clone, Debug, Default)]
pub struct NaEntry {
    pub null_bits: u64,
    pub pointer: RowPointer,
}

/// Entry / NaEntry 的线性存储池。

#[derive(Clone, Debug, Default)]
pub struct EntryStore {
    entries: Vec<Entry>,
    na_entries: Vec<NaEntry>,
}

impl EntryStore {
    /// 放入条目（具体类型见实现）。
    pub fn put(&mut self, entry: Entry) -> usize {
        self.entries.push(entry);
        self.entries.len() - 1
    }
    /// 存入 NaEntry 并返回下标。
    pub fn put_na(&mut self, entry: NaEntry) -> usize {
        self.na_entries.push(entry);
        self.na_entries.len() - 1
    }
    /// 按索引取 Entry。
    pub fn entry(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }
    /// 按索引取 NaEntry。
    pub fn na_entry(&self, index: usize) -> Option<&NaEntry> {
        self.na_entries.get(index)
    }
    /// 清空两类条目。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.na_entries.clear();
    }
}

/// 哈希表公共接口：插入、查找、遍历与内存增量。

pub trait BaseHashTable {
    /// 放入条目（具体类型见实现）。
    fn put(&mut self, hash: u64, pointer: RowPointer) -> i64;
    /// 按哈希取冲突链上的全部行指针。
    fn get(&self, hash: u64) -> Vec<RowPointer>;
    /// 元素个数。
    fn len(&self) -> usize;
    /// 遍历所有 (hash, 行指针) 对。
    fn for_each(&self, visitor: &mut dyn FnMut(u64, RowPointer));
    /// 取出并清零累计内存增量。
    fn get_and_clean_memory_delta(&self) -> i64;
    /// 是否为空。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 单线程哈希表：`HashMap` 桶存行指针向量。

#[derive(Default)]
pub struct UnsafeHashTable {
    buckets: HashMap<u64, Vec<RowPointer>>,
    length: usize,
    memory_delta: AtomicI64,
}

impl UnsafeHashTable {
    /// 预分配桶容量。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buckets: HashMap::with_capacity(capacity),
            ..Self::default()
        }
    }
}

impl BaseHashTable for UnsafeHashTable {
    /// 放入条目（具体类型见实现）。
    fn put(&mut self, hash: u64, pointer: RowPointer) -> i64 {
        let is_new = !self.buckets.contains_key(&hash);
        self.buckets.entry(hash).or_default().push(pointer);
        self.length += 1;
        let delta = std::mem::size_of::<RowPointer>() as i64
            + if is_new {
                std::mem::size_of::<u64>() as i64
            } else {
                0
            };
        self.memory_delta.fetch_add(delta, Ordering::Relaxed);
        delta
    }
    /// 按哈希取冲突链上的全部行指针。
    fn get(&self, hash: u64) -> Vec<RowPointer> {
        self.buckets.get(&hash).cloned().unwrap_or_default()
    }
    /// 元素个数。
    fn len(&self) -> usize {
        self.length
    }
    /// 遍历所有 (hash, 行指针) 对。
    fn for_each(&self, visitor: &mut dyn FnMut(u64, RowPointer)) {
        for (hash, pointers) in &self.buckets {
            for pointer in pointers {
                visitor(*hash, *pointer);
            }
        }
    }
    /// 取出并清零累计内存增量。
    fn get_and_clean_memory_delta(&self) -> i64 {
        self.memory_delta.swap(0, Ordering::AcqRel)
    }
}

/// 基于 `ConcurrentMap` 的并发哈希表，冲突链存于分片 map。

pub struct ConcurrentMapHashTable {
    buckets: ConcurrentMap<RowPointer>,
    length: usize,
    memory_delta: AtomicI64,
}

impl Default for ConcurrentMapHashTable {
    fn default() -> Self {
        Self {
            buckets: ConcurrentMap::new(),
            length: 0,
            memory_delta: AtomicI64::new(0),
        }
    }
}

impl BaseHashTable for ConcurrentMapHashTable {
    /// 放入条目（具体类型见实现）。
    fn put(&mut self, hash: u64, pointer: RowPointer) -> i64 {
        let delta = self.buckets.insert(hash, pointer) + std::mem::size_of::<RowPointer>() as i64;
        self.length += 1;
        self.memory_delta.fetch_add(delta, Ordering::Relaxed);
        delta
    }
    /// 按哈希取冲突链上的全部行指针。
    fn get(&self, hash: u64) -> Vec<RowPointer> {
        let mut rows = Vec::new();
        let mut entry = self.buckets.get(hash);
        while let Some(current) = entry {
            rows.push(current.value);
            entry = current.next.clone();
        }
        rows
    }
    /// 元素个数。
    fn len(&self) -> usize {
        self.length
    }
    /// 遍历所有 (hash, 行指针) 对。
    fn for_each(&self, visitor: &mut dyn FnMut(u64, RowPointer)) {
        self.buckets.for_each(|hash, head| {
            let mut entry = Some(head.clone());
            while let Some(current) = entry {
                visitor(hash, current.value);
                entry = current.next.clone();
            }
        });
    }
    /// 取出并清零累计内存增量。
    fn get_and_clean_memory_delta(&self) -> i64 {
        self.memory_delta.swap(0, Ordering::AcqRel)
    }
}

/// 构建侧行容器：持有 chunk、哈希表、NULL 桶与 used 标记。

pub struct HashRowContainer {
    chunks: Vec<Vec<Row>>,
    hash_table: Box<dyn BaseHashTable + Send>,
    null_bucket: HashNaNullBucket,
    key_indices: Vec<usize>,
    used: Vec<Vec<bool>>,
    spilled: bool,
    memory_bytes: i64,
    disk_bytes: i64,
}

impl HashRowContainer {
    /// 用连接键列下标构造空上下文。
    pub fn new(key_indices: Vec<usize>, concurrent: bool, estimated_rows: usize) -> Self {
        let table: Box<dyn BaseHashTable + Send> = if concurrent {
            Box::new(ConcurrentMapHashTable::default())
        } else {
            Box::new(UnsafeHashTable::with_capacity(estimated_rows))
        };
        Self {
            chunks: Vec::new(),
            hash_table: table,
            null_bucket: HashNaNullBucket::default(),
            key_indices,
            used: Vec::new(),
            spilled: false,
            memory_bytes: 0,
            disk_bytes: 0,
        }
    }

    /// 写入一块构建侧行：算哈希，NULL 进 null_bucket，否则进哈希表。

    pub fn put_chunk(&mut self, rows: Vec<Row>) -> Result<(), String> {
        let mut context = HashContext::new(self.key_indices.clone());
        context.init_hash(&rows)?;
        let chunk_index = self.chunks.len();
        self.used.push(vec![false; rows.len()]);
        for row_index in 0..rows.len() {
            let pointer = RowPointer {
                chunk_index,
                row_index,
            };
            // 含 NULL 的连接键进入 null-aware 桶；其余进入普通哈希表。
            // 含 NULL 的连接键进入 null-aware 桶；其余进入普通哈希表。
            if context.has_null[row_index] {
                self.null_bucket.put(context.null_bits[row_index], pointer);
            } else {
                self.memory_bytes += self.hash_table.put(context.hash_values[row_index], pointer);
            }
        }
        self.memory_bytes += rows.iter().map(row_size).sum::<usize>() as i64;
        self.chunks.push(rows);
        Ok(())
    }

    /// 用容器自身键列探测匹配行。

    pub fn get_matched_rows(&self, probe: &Row) -> Result<Vec<RowPointer>, String> {
        self.get_matched_rows_by_indices(probe, &self.key_indices)
    }
    /// 按探测侧键列哈希查找并做键等值过滤；探测键含 NULL 则无匹配。
    pub fn get_matched_rows_by_indices(
        &self,
        probe: &Row,
        probe_key_indices: &[usize],
    ) -> Result<Vec<RowPointer>, String> {
        if probe_key_indices.len() != self.key_indices.len() {
            return Err("build and probe key counts differ".into());
        }
        let mut context = HashContext::new(probe_key_indices.to_vec());
        context.init_hash(std::slice::from_ref(probe))?;
        // 普通等值 join：探测键含 NULL 时不匹配任何构建行。
        // 普通等值 join：探测键含 NULL 时不匹配任何构建行。
        if context.has_null[0] {
            return Ok(Vec::new());
        }
        Ok(self
            .hash_table
            .get(context.hash_values[0])
            .into_iter()
            .filter(|pointer| {
                self.row(*pointer).is_some_and(|build| {
                    keys_equal_cross(build, probe, &self.key_indices, probe_key_indices)
                })
            })
            .collect())
    }
    /// Null-aware 查找：等值匹配并集兼容的 NULL 桶行。
    pub fn get_na_rows(&self, probe: &Row) -> Result<Vec<RowPointer>, String> {
        self.get_na_rows_by_indices(probe, &self.key_indices)
    }
    /// 按探测侧键列做 null-aware 匹配。
    pub fn get_na_rows_by_indices(
        &self,
        probe: &Row,
        probe_key_indices: &[usize],
    ) -> Result<Vec<RowPointer>, String> {
        let mut rows = self.get_matched_rows_by_indices(probe, probe_key_indices)?;
        rows.extend(self.null_bucket.rows.iter().filter_map(|(_, pointer)| {
            self.row(*pointer)
                .filter(|build| {
                    na_keys_compatible(build, probe, &self.key_indices, probe_key_indices)
                })
                .map(|_| *pointer)
        }));
        Ok(rows)
    }
    /// 按行指针取构建侧行。
    pub fn row(&self, pointer: RowPointer) -> Option<&Row> {
        self.chunks.get(pointer.chunk_index)?.get(pointer.row_index)
    }
    /// 标记该构建行已被匹配（供 outer/anti 扫未匹配行）。
    pub fn mark_used(&mut self, pointer: RowPointer) {
        if let Some(used) = self
            .used
            .get_mut(pointer.chunk_index)
            .and_then(|chunk| chunk.get_mut(pointer.row_index))
        {
            *used = true;
        }
    }
    /// 返回尚未被 mark_used 的构建侧行。
    pub fn unmatched_rows(&self) -> Vec<&Row> {
        self.chunks
            .iter()
            .enumerate()
            .flat_map(|(chunk_index, chunk)| {
                chunk
                    .iter()
                    .enumerate()
                    .filter(move |(row_index, _)| !self.used[chunk_index][*row_index])
                    .map(|(_, row)| row)
            })
            .collect()
    }
    /// 标记已 spill，并把内存字节计入磁盘字节。
    pub fn spill(&mut self) {
        if !self.spilled {
            self.spilled = true;
            self.disk_bytes += self.memory_bytes;
            self.memory_bytes = 0;
        }
    }
    /// 是否已 spill。
    pub fn already_spilled(&self) -> bool {
        self.spilled
    }
    /// 元素个数。
    pub fn len(&self) -> usize {
        self.used.iter().map(Vec::len).sum()
    }
    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// 当前估算内存占用。
    pub fn memory_bytes(&self) -> i64 {
        self.memory_bytes
    }
    /// 已计入磁盘的字节。
    pub fn disk_bytes(&self) -> i64 {
        self.disk_bytes
    }
    /// 释放 chunk 与 used 标记。
    pub fn close(&mut self) {
        self.chunks.clear();
        self.used.clear();
        self.null_bucket.rows.clear();
        self.memory_bytes = 0;
    }
}

/// 同侧键列等值比较；任一侧为 NULL 则不等。

fn keys_equal(left: &Row, right: &Row, indices: &[usize]) -> bool {
    indices
        .iter()
        .all(|index| match (left.get(*index), right.get(*index)) {
            (Some(Value::Null), _) | (_, Some(Value::Null)) => false,
            (Some(a), Some(b)) => a == b,
            _ => false,
        })
}
/// 构建/探测两侧不同键列下标的交叉等值比较。
fn keys_equal_cross(
    left: &Row,
    right: &Row,
    left_indices: &[usize],
    right_indices: &[usize],
) -> bool {
    left_indices
        .iter()
        .zip(right_indices)
        .all(
            |(left_index, right_index)| match (left.get(*left_index), right.get(*right_index)) {
                (Some(Value::Null), _) | (_, Some(Value::Null)) => false,
                (Some(a), Some(b)) => a == b,
                _ => false,
            },
        )
}
/// NAAJ 键比较：任一侧为 NULL 的列不参与比较，其余列必须相等。
fn na_keys_compatible(
    build: &Row,
    probe: &Row,
    build_indices: &[usize],
    probe_indices: &[usize],
) -> bool {
    build_indices
        .iter()
        .zip(probe_indices)
        .all(|(build_index, probe_index)| {
            match (build.get(*build_index), probe.get(*probe_index)) {
                (Some(Value::Null), _) | (_, Some(Value::Null)) => true,
                (Some(build_value), Some(probe_value)) => build_value == probe_value,
                _ => false,
            }
        })
}
/// FNV-1a 风格字节哈希。
fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1469598103934665603, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}
/// 把 `Value` 编码进字节缓冲供哈希。
fn encode_value(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Null => output.push(0),
        Value::Bool(v) => output.extend_from_slice(&[1, u8::from(*v)]),
        Value::Int(v) => {
            output.push(2);
            output.extend_from_slice(&v.to_le_bytes());
        }
        Value::UInt(v) => {
            output.push(3);
            output.extend_from_slice(&v.to_le_bytes());
        }
        Value::Float(v) => {
            output.push(4);
            output.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        Value::Bytes(v) => {
            output.push(5);
            output.extend_from_slice(&(v.len() as u64).to_le_bytes());
            output.extend_from_slice(v);
        }
        Value::Text(v) => {
            output.push(6);
            output.extend_from_slice(&(v.len() as u64).to_le_bytes());
            output.extend_from_slice(v.as_bytes());
        }
    }
}
/// 估算一行字节占用。
fn row_size(row: &Row) -> usize {
    row.iter()
        .map(|v| match v {
            Value::Bytes(v) => v.len(),
            Value::Text(v) => v.len(),
            _ => 8,
        })
        .sum()
}
