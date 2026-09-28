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

// Outer Join（外连接）Hash Join probe 实现。
//
// Outer Join 保证 outer 侧每一行至少输出一次：有匹配则拼 inner 列，无匹配则为
// inner 侧补 NULL（默认值）。当 outer 侧作为 build 时，probe 结束后还需扫描
// row table 补出未匹配的 build 行。对应 Go 的 `outerJoinProbe`。

// hash join v2 outer join probe 的两种路径。
// outerJoinProbe 对应 Go 结构体，记录 outer/build 侧关系和结果 chunk 中的列映射。
// pub struct outerJoinProbe {
//     pub baseJoinProbe: baseJoinProbe,
// isOuterSideBuild is true means the outer side is build side, otherwise is probe side.
// Outer side is the all-fetched side, and inner side is the null-append side.
// For left out join, left side is outer side, and right side is inner side.
// For right out join, right side is outer side, and left side is inner side.
//     pub isOuterSideBuild: bool,
// used when use inner side to build, isNotMatchedRows is indexed by logical row index
//     pub isNotMatchedRows: Vec<bool>,
// used when use outer side to build
//     pub rowIter: Option<rowIter>,
// build/probe side used columns and offset in result chunk
//     pub buildColUsed: Vec<usize>,
//     pub buildColOffsetInResultChk: usize,
//     pub probeColUsed: Vec<usize>,
//     pub probeColOffsetInResultChk: usize,
// }
// newOuterJoinProbe 对应 Go 构造函数，根据 build 侧方向计算输出列偏移。
// pub fn newOuterJoinProbe(base: baseJoinProbe, isOuterSideBuild: bool, isRightSideBuild: bool) -> Box<outerJoinProbe> {
//     let mut probe = outerJoinProbe {
//         baseJoinProbe: base,
//         isOuterSideBuild,
//         isNotMatchedRows: Vec::new(),
//         rowIter: None,
//         buildColUsed: Vec::new(),
//         buildColOffsetInResultChk: 0,
//         probeColUsed: Vec::new(),
//         probeColOffsetInResultChk: 0,
//     };
//     if isRightSideBuild {
//         probe.buildColUsed = probe.baseJoinProbe.rUsed.clone();
//         probe.buildColOffsetInResultChk = probe.baseJoinProbe.lUsed.len();
//         probe.probeColUsed = probe.baseJoinProbe.lUsed.clone();
//         probe.probeColOffsetInResultChk = 0;
//     } else {
//         probe.buildColUsed = probe.baseJoinProbe.lUsed.clone();
//         probe.buildColOffsetInResultChk = 0;
//         probe.probeColUsed = probe.baseJoinProbe.rUsed.clone();
//         probe.probeColOffsetInResultChk = probe.baseJoinProbe.lUsed.len();
//     }
//     Box::new(probe)
// }
// impl outerJoinProbe {
// prepareIsNotMatchedRows 对应 Go 方法：仅 inner side build 时需要跟踪 probe 行是否未匹配。
//     pub fn prepareIsNotMatchedRows(&mut self) {
//         if !self.isOuterSideBuild {
//             self.isNotMatchedRows.clear();
//             for _ in 0..self.baseJoinProbe.chunkRows {
//                 self.isNotMatchedRows.push(true);
//             }
//             for spilledIdx in &self.baseJoinProbe.spilledIdx {
// Go 注释称这里可能是 hack：本轮 spill 行暂时视为不能 join，未来轮次还可能成功。
//                 self.isNotMatchedRows[*spilledIdx] = false;
//             }
//         }
//     }
// SetChunkForProbe 对应 Go 方法：设置当前 probe chunk 后准备 unmatched 标记。
//     pub fn SetChunkForProbe(&mut self, chunk: &mut chunk::Chunk) -> Result<(), errors::Error> {
//         self.baseJoinProbe.SetChunkForProbe(chunk)?;
//         self.prepareIsNotMatchedRows();
//         Ok(())
//     }
// SetRestoredChunkForProbe 对应 spill 恢复 chunk 的 probe 初始化。
//     pub fn SetRestoredChunkForProbe(&mut self, chk: &mut chunk::Chunk) -> Result<(), errors::Error> {
//         self.baseJoinProbe.SetRestoredChunkForProbe(chk)?;
//         self.prepareIsNotMatchedRows();
//         Ok(())
//     }
// NeedScanRowTable 对应 Go 方法：outer side build 时，probe 结束后还要扫未使用 build 行。
//     pub fn NeedScanRowTable(&self) -> bool {
//         self.isOuterSideBuild
//     }
// IsScanRowTableDone 对应 Go 方法：非 outer build 路径不可调用。
//     pub fn IsScanRowTableDone(&self) -> bool {
//         if !self.isOuterSideBuild {
//             panic!("should not reach here");
//         }
//         self.rowIter.as_ref().unwrap().isEnd()
//     }
// InitForScanRowTable 对应 Go 方法：为扫描 build row table 初始化迭代器。
//     pub fn InitForScanRowTable(&mut self) {
//         if !self.isOuterSideBuild {
//             panic!("should not reach here");
//         }
//         self.rowIter = Some(commonInitForScanRowTable(&mut self.baseJoinProbe));
//     }
// ScanRowTable 对应 Go 方法：输出 outer build 侧中仍未被匹配的行，并为 probe 侧追加 NULL。
//     pub fn ScanRowTable(
//         &mut self,
//         joinResult: *mut hashjoinWorkerResult,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> *mut hashjoinWorkerResult {
//         if !self.isOuterSideBuild {
//             panic!("should not reach here");
//         }
//         unsafe {
//             if (*joinResult).chk.IsFull() {
//                 return joinResult;
//             }
//         }
//         if self.rowIter.is_none() {
//             panic!("scanRowTable before init");
//         }
//         self.baseJoinProbe.nextCachedBuildRowIndex = 0;
//         let meta = self.baseJoinProbe.ctx.hashTableMeta;
//         let mut insertedRows = 0;
//         let remainCap = unsafe { (*joinResult).chk.RequiredRows() - (*joinResult).chk.NumRows() };
//         while insertedRows < remainCap && !self.rowIter.as_ref().unwrap().isEnd() {
//             let currentRow = self.rowIter.as_ref().unwrap().getValue();
//             if !meta.isCurrentRowUsed(currentRow) {
// append build side of this row
//                 self.baseJoinProbe
//                     .appendBuildRowToCachedBuildRowsV1(0, currentRow, unsafe { &mut (*joinResult).chk }, 0, false);
//                 insertedRows += 1;
//             }
//             self.rowIter.as_mut().unwrap().next();
//         }
//         if let Err(err) = checkSQLKiller(sqlKiller, "killedDuringProbe") {
//             unsafe { (*joinResult).err = Some(err) };
//             return joinResult;
//         }
//         if self.baseJoinProbe.nextCachedBuildRowIndex > 0 {
//             self.baseJoinProbe.batchConstructBuildRows(unsafe { &mut (*joinResult).chk }, 0, false);
//         }
// append probe side in batch
//         for index in 0..self.probeColUsed.len() {
//             unsafe {
//                 (*joinResult)
//                     .chk
//                     .Column(index + self.probeColOffsetInResultChk)
//                     .AppendNNulls(insertedRows);
//             }
//         }
//         joinResult
//     }
// buildResultForMatchedRowsAfterOtherCondition 对应 Go 方法：other condition 通过后复制 probe/build 两侧列。
//     pub fn buildResultForMatchedRowsAfterOtherCondition(&mut self, chk: &mut chunk::Chunk, joinedChk: &mut chunk::Chunk) {
//         let (mut probeColOffsetInJoinedChunk, mut buildColOffsetInJoinedChunk) =
//             (self.baseJoinProbe.ctx.hashTableMeta.totalColumnNumber, 0);
//         if self.baseJoinProbe.rightAsBuildSide {
//             probeColOffsetInJoinedChunk = 0;
//             buildColOffsetInJoinedChunk = self.baseJoinProbe.currentChunk.NumCols();
//         }
//         let rowCount = chk.NumRows();
//         let mut markedJoined = false;
//         for (index, colIndex) in self.probeColUsed.iter().enumerate() {
//             let dstCol = chk.Column(self.probeColOffsetInResultChk + index);
//             if joinedChk.Column(colIndex + probeColOffsetInJoinedChunk).Rows() > 0 {
// probe column that is already in joinedChk
//                 let srcCol = joinedChk.Column(colIndex + probeColOffsetInJoinedChunk);
//                 chunk::CopySelectedRows(dstCol, srcCol, &self.baseJoinProbe.selected);
//             } else {
//                 markedJoined = true;
//                 let srcCol = self.baseJoinProbe.currentChunk.Column(*colIndex);
//                 chunk::CopySelectedRowsWithRowIDFunc(
//                     dstCol,
//                     srcCol,
//                     &self.baseJoinProbe.selected,
//                     0,
//                     self.baseJoinProbe.selected.len(),
//                     |i| {
//                         let ret = self.baseJoinProbe.rowIndexInfos[i].probeRowIndex;
//                         self.isNotMatchedRows[ret] = false;
//                         self.baseJoinProbe.usedRows[ret]
//                     },
//                 );
//             }
//         }
//         let mut hasRemainCols = false;
//         for (index, colIndex) in self.buildColUsed.iter().enumerate() {
//             let dstCol = chk.Column(self.buildColOffsetInResultChk + index);
//             let srcCol = joinedChk.Column(buildColOffsetInJoinedChunk + colIndex);
//             if srcCol.Rows() > 0 {
// build column that is already in joinedChk
//                 chunk::CopySelectedRows(dstCol, srcCol, &self.baseJoinProbe.selected);
//             } else {
//                 hasRemainCols = true;
//             }
//         }
//         if hasRemainCols {
//             self.baseJoinProbe.nextCachedBuildRowIndex = 0;
//             markedJoined = true;
//             let meta = self.baseJoinProbe.ctx.hashTableMeta;
//             for (index, result) in self.baseJoinProbe.selected.iter().enumerate() {
//                 if *result {
//                     let rowIndexInfo = self.baseJoinProbe.rowIndexInfos[index].clone();
//                     self.isNotMatchedRows[rowIndexInfo.probeRowIndex] = false;
//                     self.baseJoinProbe.appendBuildRowToCachedBuildRowsV2(
//                         &rowIndexInfo,
//                         chk,
//                         meta.columnCountNeededForOtherCondition,
//                         false,
//                     );
//                 }
//             }
//             if self.baseJoinProbe.nextCachedBuildRowIndex > 0 {
//                 self.baseJoinProbe
//                     .batchConstructBuildRows(chk, meta.columnCountNeededForOtherCondition, false);
//             }
//         }
//         if !markedJoined {
//             for (index, result) in self.baseJoinProbe.selected.iter().enumerate() {
//                 if *result {
//                     self.isNotMatchedRows[self.baseJoinProbe.rowIndexInfos[index].probeRowIndex] = false;
//                 }
//             }
//         }
//         let rowsAdded = self.baseJoinProbe.selected.iter().filter(|result| **result).count();
//         chk.SetNumVirtualRows(rowCount + rowsAdded);
//     }
// buildResultForNotMatchedRows 对应 Go 方法：输出未匹配 probe 行，build 侧列统一补 NULL。
//     pub fn buildResultForNotMatchedRows(&mut self, chk: &mut chunk::Chunk, startProbeRow: usize) {
//         let prevRows = chk.NumRows();
//         let mut afterRows = prevRows;
//         for (index, colIndex) in self.probeColUsed.iter().enumerate() {
//             let dstCol = chk.Column(self.probeColOffsetInResultChk + index);
//             let srcCol = self.baseJoinProbe.currentChunk.Column(*colIndex);
//             chunk::CopySelectedRowsWithRowIDFunc(
//                 dstCol,
//                 srcCol,
//                 &self.isNotMatchedRows,
//                 startProbeRow,
//                 self.baseJoinProbe.currentProbeRow,
//                 |i| self.baseJoinProbe.usedRows[i],
//             );
//             afterRows = dstCol.Rows();
//         }
//         let mut nullRows = afterRows - prevRows;
//         if self.probeColUsed.is_empty() {
//             for i in startProbeRow..self.baseJoinProbe.currentProbeRow {
//                 if self.isNotMatchedRows[i] {
//                     nullRows += 1;
//                 }
//             }
//         }
//         if nullRows > 0 {
//             for index in 0..self.buildColUsed.len() {
//                 let dstCol = chk.Column(self.buildColOffsetInResultChk + index);
//                 dstCol.AppendNNulls(nullRows);
//             }
//             chk.SetNumVirtualRows(prevRows + nullRows);
//         }
//     }
// probeForInnerSideBuild 对应 Go 分支：probe 侧是 outer，匹配失败的 probe 行要补 NULL 输出。
//     pub fn probeForInnerSideBuild(
//         &mut self,
//         chk: &mut chunk::Chunk,
//         joinedChk: &mut chunk::Chunk,
//         mut remainCap: usize,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> Result<(), errors::Error> {
//         let meta = self.baseJoinProbe.ctx.hashTableMeta;
//         let startProbeRow = self.baseJoinProbe.currentProbeRow;
//         let hasOtherCondition = self.baseJoinProbe.ctx.hasOtherCondition();
//         let tagHelper = self.baseJoinProbe.ctx.hashTableContext.tagHelper;
//         while remainCap > 0 && self.baseJoinProbe.currentProbeRow < self.baseJoinProbe.chunkRows {
//             let row = self.baseJoinProbe.currentProbeRow;
//             if self.baseJoinProbe.matchedRowsHeaders[row] != 0 {
// hash value match
//                 let candidateRow = tagHelper.toUnsafePointer(self.baseJoinProbe.matchedRowsHeaders[row]);
//                 if isKeyMatched(meta.keyMode, self.baseJoinProbe.serializedKeys[row].clone(), candidateRow, meta) {
// join key match
//                     self.baseJoinProbe
//                         .appendBuildRowToCachedBuildRowsV1(row, candidateRow, joinedChk, 0, hasOtherCondition);
//                     if !hasOtherCondition {
// has no other condition, key match mean join match
//                         self.isNotMatchedRows[row] = false;
//                     }
//                     self.baseJoinProbe.matchedRowsForCurrentProbeRow += 1;
//                 } else {
//                     self.baseJoinProbe.probeCollision += 1;
//                 }
//                 self.baseJoinProbe.matchedRowsHeaders[row] =
//                     getNextRowAddress(candidateRow, tagHelper, self.baseJoinProbe.matchedRowsHashValue[row]);
//             } else {
// 可能是 hash table 无匹配、probeFilter 过滤，或行已 spill 到磁盘。
//                 self.baseJoinProbe.finishLookupCurrentProbeRow();
//                 self.baseJoinProbe.currentProbeRow += 1;
//             }
//             remainCap -= 1;
//         }
//         checkSQLKiller(sqlKiller, "killedDuringProbe")?;
//         self.baseJoinProbe.finishCurrentLookupLoop(joinedChk);
//         if hasOtherCondition {
//             if joinedChk.NumRows() > 0 {
//                 self.baseJoinProbe.selected.clear();
//                 self.baseJoinProbe.selected = expression::VectorizedFilter(
//                     self.baseJoinProbe.ctx.SessCtx.GetExprCtx().GetEvalCtx(),
//                     self.baseJoinProbe.ctx.SessCtx.GetSessionVars().EnableVectorizedExpression,
//                     &self.baseJoinProbe.ctx.OtherCondition,
//                     chunk::NewIterator4Chunk(joinedChk),
//                     self.baseJoinProbe.selected.clone(),
//                 )?;
//                 self.buildResultForMatchedRowsAfterOtherCondition(chk, joinedChk);
//             }
// append the not matched rows
//             self.buildResultForNotMatchedRows(chk, startProbeRow);
//         } else {
// no condition 时 chk == joinedChk，匹配行已在 joinedChk 中，只需补 unmatched。
//             self.buildResultForNotMatchedRows(joinedChk, startProbeRow);
//         }
//         Ok(())
//     }
// probeForOuterSideBuild 对应 Go 分支：build 侧是 outer，匹配成功时标记 build row 已使用。
//     pub fn probeForOuterSideBuild(
//         &mut self,
//         chk: &mut chunk::Chunk,
//         joinedChk: &mut chunk::Chunk,
//         mut remainCap: usize,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> Result<(), errors::Error> {
//         let meta = self.baseJoinProbe.ctx.hashTableMeta;
//         let hasOtherCondition = self.baseJoinProbe.ctx.hasOtherCondition();
//         let tagHelper = self.baseJoinProbe.ctx.hashTableContext.tagHelper;
//         while remainCap > 0 && self.baseJoinProbe.currentProbeRow < self.baseJoinProbe.chunkRows {
//             let row = self.baseJoinProbe.currentProbeRow;
//             if self.baseJoinProbe.matchedRowsHeaders[row] != 0 {
// hash value match
//                 let candidateRow = tagHelper.toUnsafePointer(self.baseJoinProbe.matchedRowsHeaders[row]);
//                 if isKeyMatched(meta.keyMode, self.baseJoinProbe.serializedKeys[row].clone(), candidateRow, meta) {
// join key match
//                     self.baseJoinProbe
//                         .appendBuildRowToCachedBuildRowsV1(row, candidateRow, joinedChk, 0, hasOtherCondition);
//                     if !hasOtherCondition {
// has no other condition, key match means join match
//                         meta.setUsedFlag(candidateRow);
//                     }
//                     self.baseJoinProbe.matchedRowsForCurrentProbeRow += 1;
//                     remainCap -= 1;
//                 } else {
//                     self.baseJoinProbe.probeCollision += 1;
//                 }
//                 self.baseJoinProbe.matchedRowsHeaders[row] =
//                     getNextRowAddress(candidateRow, tagHelper, self.baseJoinProbe.matchedRowsHashValue[row]);
//             } else {
//                 self.baseJoinProbe.finishLookupCurrentProbeRow();
//                 self.baseJoinProbe.currentProbeRow += 1;
//             }
//         }
//         checkSQLKiller(sqlKiller, "killedDuringProbe")?;
//         self.baseJoinProbe.finishCurrentLookupLoop(joinedChk);
//         if self.baseJoinProbe.ctx.hasOtherCondition() && joinedChk.NumRows() > 0 {
//             self.baseJoinProbe.selected = expression::VectorizedFilter(
//                 self.baseJoinProbe.ctx.SessCtx.GetExprCtx().GetEvalCtx(),
//                 self.baseJoinProbe.ctx.SessCtx.GetSessionVars().EnableVectorizedExpression,
//                 &self.baseJoinProbe.ctx.OtherCondition,
//                 chunk::NewIterator4Chunk(joinedChk),
//                 self.baseJoinProbe.selected.clone(),
//             )?;
//             self.baseJoinProbe.buildResultAfterOtherCondition(chk, joinedChk)?;
//             for (index, result) in self.baseJoinProbe.selected.iter().enumerate() {
//                 if *result {
// Go 这里用 unsafe 从 buildRowStart 反解 row pointer；保留标记已使用的语义。
//                     meta.setUsedFlag(self.baseJoinProbe.rowIndexInfos[index].buildRowStart.as_pointer());
//                 }
//             }
//         }
//         Ok(())
//     }
// Probe 对应 Go 主入口：保护 joinedChk 的 incomplete 状态，再分派到 inner/outer build 两条路径。
//     pub fn Probe(
//         &mut self,
//         joinResult: *mut hashjoinWorkerResult,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> (bool, *mut hashjoinWorkerResult) {
//         unsafe {
//             if (*joinResult).chk.IsFull() {
//                 return (true, joinResult);
//             }
//         }
//         let (joinedChk, remainCap, err) = self.baseJoinProbe.prepareForProbe(unsafe { &mut (*joinResult).chk });
//         if let Some(err) = err {
//             unsafe { (*joinResult).err = Some(err) };
//             return (false, joinResult);
//         }
//         let isInCompleteChunk = joinedChk.IsInCompleteChunk();
// in case that virtual rows is not maintained correctly
//         joinedChk.SetNumVirtualRows(joinedChk.NumRows());
// always set in complete chunk during probe；Go 用 defer 恢复原状态。
//         joinedChk.SetInCompleteChunk(true);
//         let err = if self.isOuterSideBuild {
//             self.probeForOuterSideBuild(unsafe { &mut (*joinResult).chk }, joinedChk, remainCap, sqlKiller)
//         } else {
//             self.probeForInnerSideBuild(unsafe { &mut (*joinResult).chk }, joinedChk, remainCap, sqlKiller)
//         };
//         joinedChk.SetInCompleteChunk(isInCompleteChunk);
//         if let Err(err) = err {
//             unsafe { (*joinResult).err = Some(err) };
//             return (false, joinResult);
//         }
//         (true, joinResult)
//     }
// ResetProbe 对应 Go 方法：清理 row table 扫描状态后复用 base 重置逻辑。
//     pub fn ResetProbe(&mut self) {
//         self.rowIter = None;
//         self.baseJoinProbe.ResetProbe();
//     }
// }
// */
use crate::base_join_probe::{BaseJoinProbe, Probe, WorkerResult};
use crate::joiner::{NaajType, OuterRowStatus};

/// Left / Right Outer Join 共用的 probe 状态。
///
/// `outer_side_build` 为 true 表示 outer 侧建哈希表，probe 后需扫描未使用 build 行；
/// `right_side_build` 记录右表是否为 build 侧，影响结果列顺序。
#[derive(Clone)]
pub struct OuterJoinProbe {
    /// 公共 probe 基座（chunk、匹配链表、上下文）。
    pub base: BaseJoinProbe,
    /// outer 侧是否作为 build 侧。
    pub outer_side_build: bool,
    /// 右表是否作为 build 侧。
    pub right_side_build: bool,
}

impl Probe for OuterJoinProbe {
    /// 设置本轮 probe chunk。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.base.set_chunk_for_probe(chunk)
    }
    /// 设置 spill 恢复后的 probe chunk。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.base.set_restored_chunk_for_probe(chunk)
    }
    /// 溢出尚未探测完的 probe 行。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.base.spill_remaining_probe_chunks()
    }
    /// 按 outer/inner build 两条路径探测并产出连接结果。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在输出容量允许时推进当前 probe 行。
        while self.base.current_probe_row < self.base.probe_chunk.len()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.current_probe_row;
            let probe_row = self.base.probe_chunk[index].clone();
            let candidate_start = self.base.current_candidate;
            let candidate_end = (candidate_start
                + (self.base.context.max_chunk_size - rows.len()).max(1))
            .min(self.base.matched_rows[index].len());
            let candidate_complete = candidate_end == self.base.matched_rows[index].len();
            let build_indices =
                self.base.matched_rows[index][candidate_start..candidate_end].to_vec();
            let build_rows: Vec<_> = build_indices
                .iter()
                .map(|build| self.base.context.build_rows[*build].clone())
                .collect();
            if self.outer_side_build {
                // Outer build：以 build 行为 outer，匹配成功则标记 used。
                match self.base.context.joiner.try_to_match_outers(
                    &build_rows,
                    &probe_row,
                    &mut rows,
                ) {
                    Ok(statuses) => {
                        for (position, status) in statuses.iter().enumerate() {
                            if *status == OuterRowStatus::Matched {
                                self.base.context.build_row_used[build_indices[position]] = true;
                            }
                        }
                    }
                    Err(error) => {
                        return WorkerResult {
                            rows,
                            error: Some(error),
                        };
                    }
                }
                self.base.current_candidate += build_indices.len();
            } else {
                // Inner build：probe 行为 outer，未匹配时补默认 inner 行。
                match self.base.context.joiner.try_to_match_inners(
                    &probe_row,
                    &build_rows,
                    &mut rows,
                    NaajType::Unknown,
                ) {
                    Ok(result) => {
                        if result.matched {
                            self.base.mark_build_rows_used(index);
                        } else if candidate_complete {
                            self.base.context.joiner.on_miss_match(
                                result.has_null,
                                &probe_row,
                                &mut rows,
                            );
                        }
                    }
                    Err(error) => {
                        return WorkerResult {
                            rows,
                            error: Some(error),
                        };
                    }
                }
                self.base.current_candidate += build_indices.len();
            }
            if self.base.current_candidate >= self.base.matched_rows[index].len() {
                self.base.finish_current_lookup_loop();
            }
        }
        WorkerResult { rows, error: None }
    }
    /// 仅 outer 侧 build 时需要扫描未匹配的 build 行。
    fn need_scan_row_table(&self) -> bool {
        self.outer_side_build
    }
    /// 初始化 build row table 扫描游标。
    fn init_for_scan_row_table(&mut self) {
        assert!(self.outer_side_build, "should not reach here");
        self.base.scan_row_index = 0;
    }
    /// 扫描未使用的 build 行，为 probe 侧补默认值后输出。
    fn scan_row_table(&mut self) -> WorkerResult {
        assert!(self.outer_side_build, "should not reach here");
        let mut rows = Vec::new();
        while self.base.scan_row_index < self.base.context.build_rows.len()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.scan_row_index;
            self.base.scan_row_index += 1;
            if !self.base.context.build_row_used[index] {
                let build = self.base.context.build_rows[index].clone();
                self.base
                    .context
                    .joiner
                    .on_miss_match(false, &build, &mut rows);
            }
        }
        WorkerResult { rows, error: None }
    }
    /// 是否已扫完全部 build 行。
    fn is_scan_row_table_done(&self) -> bool {
        assert!(self.outer_side_build, "should not reach here");
        self.base.scan_row_index >= self.base.context.build_rows.len()
    }
    fn is_current_chunk_probe_done(&self) -> bool {
        self.base.is_current_chunk_probe_done()
    }
    fn reset_probe(&mut self) {
        self.base.reset_probe();
    }
}
