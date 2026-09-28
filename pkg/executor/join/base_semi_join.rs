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

// Semi / Anti Semi Join 共享的 Probe 基类。
//
// Semi Join（半连接）只输出在对侧存在匹配的左侧行，不展开笛卡尔积；
// Anti Semi 则输出无匹配的左侧行。本模块提供匹配标记、未完成 probe 队列，
// 以及扫描 build 行等共用逻辑。块注释内保留 Go 版详细中文说明。

// The following described case has other condition.
// During the probe, when a probe matches one build row, we need to put the probe and build rows
// together and generate a new row. If one probe row could match n build row, then we will get
// n new rows. If n is very big, there will generate too much rows. In order to avoid this case
// we need to limit the max generated row number. This variable describe this max number.
// NOTE: Suppose probe chunk has n rows and n*maxMatchedRowNum << chunkRemainingCapacity.
// We will keep on join probe rows that have been matched before with build rows, though
// probe row with idx i may have produced `maxMatchedRowNum` number rows before. So that
// we can process as many rows as possible.
// maxMatchedRowNum 对应 Go 包级变量，用于限制一次 other condition 中间 chunk 的膨胀。
// pub static mut maxMatchedRowNum: i32 = 4;
//
// baseSemiJoin 对应 Go 的嵌入 baseJoinProbe 的结构体。
// 字段顺序保持原文件，便于人工核对 semi/anti semi join 的共享状态。
// pub struct baseSemiJoin {
//     pub baseJoinProbe: baseJoinProbe,
//     pub isLeftSideBuild: bool,
//
// isMatchedRows marks whether the left side row is matched
// It's used only when right side is build side.
//     pub isMatchedRows: Vec<bool>,
//
//     pub isNulls: Vec<bool>,
//
// used when left side is build side
//     pub rowIter: Option<*mut rowIter>,
//
// used in other condition to record which rows need to be processed
//     pub unFinishedProbeRowIdxQueue: Option<queue::Queue<i32>>,
//
// Used for right side build without other condition in semi and anti semi join
//     pub offsets: Vec<i32>,
// }
//
// newBaseSemiJoin 对应 Go 构造函数：复制 baseJoinProbe 并记录 build side。
// pub fn newBaseSemiJoin(base: baseJoinProbe, isLeftSideBuild: bool) -> Box<baseSemiJoin> {
//     Box::new(baseSemiJoin {
//         baseJoinProbe: base,
//         isLeftSideBuild,
//         isMatchedRows: Vec::new(),
//         isNulls: Vec::new(),
//         rowIter: None,
//         unFinishedProbeRowIdxQueue: None,
//         offsets: Vec::new(),
//     })
// }
//
// impl baseSemiJoin {
// resetProbeState 对应 Go 的同名方法：为当前 probe chunk 重建匹配标记和未完成队列。
//     pub fn resetProbeState(&mut self) {
//         if !self.isLeftSideBuild {
//             self.isMatchedRows.clear();
//             for _ in 0..self.baseJoinProbe.chunkRows {
//                 self.isMatchedRows.push(false);
//             }
//         }
//
//         if self.baseJoinProbe.ctx.hasOtherCondition() {
//             if self.unFinishedProbeRowIdxQueue.is_none() {
//                 self.unFinishedProbeRowIdxQueue = Some(queue::NewQueue(self.baseJoinProbe.chunkRows));
//             } else {
// Go 这里复用并按需扩容队列，避免每个 chunk 都重新分配。
//                 self.unFinishedProbeRowIdxQueue
//                     .as_mut()
//                     .unwrap()
//                     .ClearAndExpandIfNeed(self.baseJoinProbe.chunkRows);
//             }
//
//             for i in 0..self.baseJoinProbe.chunkRows {
//                 if self.baseJoinProbe.matchedRowsHeaders[i as usize] != 0 {
//                     self.unFinishedProbeRowIdxQueue.as_mut().unwrap().Push(i);
//                 }
//             }
//         }
//     }
//
// matchMultiBuildRows 对应 Go 中一次 probe row 匹配多个 build row 的循环。
// 它会推进 matchedRowsHeaders 链表，并把命中的 build row 批量缓存到 baseJoinProbe。
//     pub fn matchMultiBuildRows(
//         &mut self,
//         joinedChk: *mut chunk::Chunk,
//         joinedChkRemainCap: &mut i32,
//         isRightSideBuild: bool,
//     ) {
//         let tagHelper = self.baseJoinProbe.ctx.hashTableContext.tagHelper;
//         let meta = self.baseJoinProbe.ctx.hashTableMeta;
//         while self.baseJoinProbe.matchedRowsHeaders[self.baseJoinProbe.currentProbeRow as usize] != 0
//             && *joinedChkRemainCap > 0
//             && self.baseJoinProbe.matchedRowsForCurrentProbeRow < unsafe { maxMatchedRowNum }
//         {
//             let candidateRow = tagHelper.toUnsafePointer(
//                 self.baseJoinProbe.matchedRowsHeaders[self.baseJoinProbe.currentProbeRow as usize],
//             );
//             if isRightSideBuild || !meta.isCurrentRowUsedWithAtomic(candidateRow) {
//                 if isKeyMatched(
//                     meta.keyMode,
//                     &self.baseJoinProbe.serializedKeys[self.baseJoinProbe.currentProbeRow as usize],
//                     candidateRow,
//                     meta,
//                 ) {
//                     self.baseJoinProbe.appendBuildRowToCachedBuildRowsV1(
//                         self.baseJoinProbe.currentProbeRow,
//                         candidateRow,
//                         joinedChk,
//                         0,
//                         true,
//                     );
//                     self.baseJoinProbe.matchedRowsForCurrentProbeRow += 1;
//                     *joinedChkRemainCap -= 1;
//                 } else {
//                     self.baseJoinProbe.probeCollision += 1;
//                 }
//             }
//
//             self.baseJoinProbe.matchedRowsHeaders[self.baseJoinProbe.currentProbeRow as usize] =
//                 getNextRowAddress(
//                     candidateRow,
//                     tagHelper,
//                     self.baseJoinProbe.matchedRowsHashValue[self.baseJoinProbe.currentProbeRow as usize],
//                 );
//         }
//
//         self.baseJoinProbe.finishLookupCurrentProbeRow();
//     }
//
// concatenateProbeAndBuildRows 对应 Go other condition 的中间行拼接。
// 队列保存尚未处理完的 probe row；SQL killer 检查保留在批处理尾部。
//     pub fn concatenateProbeAndBuildRows(
//         &mut self,
//         joinedChk: *mut chunk::Chunk,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//         isRightSideBuild: bool,
//     ) -> Result<(), errors::Error> {
//         let mut joinedChkRemainCap = unsafe { (*joinedChk).Capacity() };
//
//         while joinedChkRemainCap > 0
//             && !self.unFinishedProbeRowIdxQueue.as_ref().unwrap().IsEmpty()
//         {
//             let probeRowIdx = self.unFinishedProbeRowIdxQueue.as_mut().unwrap().Pop();
//             if isRightSideBuild && self.isMatchedRows[probeRowIdx as usize] {
//                 continue;
//             }
//
//             self.baseJoinProbe.currentProbeRow = probeRowIdx;
//             self.matchMultiBuildRows(joinedChk, &mut joinedChkRemainCap, isRightSideBuild);
//
//             if self.baseJoinProbe.matchedRowsHeaders[probeRowIdx as usize] == 0 {
//                 continue;
//             }
//
//             self.unFinishedProbeRowIdxQueue.as_mut().unwrap().Push(probeRowIdx);
//         }
//
//         let err = checkSQLKiller(sqlKiller, "killedDuringProbe");
//         if err.is_err() {
//             return err;
//         }
//
//         self.baseJoinProbe.finishCurrentLookupLoop(joinedChk);
//         Ok(())
//     }
//
// Only used for semi and anti semi join
// generateResultChkForRightBuildNoOtherCondition 只复制左侧被保留的列；无列时手动维护虚拟行数。
//     pub fn generateResultChkForRightBuildNoOtherCondition(&mut self, resultChk: *mut chunk::Chunk) {
//         if self.offsets.is_empty() {
//             return;
//         }
//
//         for (index, colIndex) in self.baseJoinProbe.lUsed.iter().enumerate() {
//             let srcCol = self.baseJoinProbe.currentChunk.Column(*colIndex);
//             let dstCol = unsafe { (*resultChk).Column(index as i32) };
//             chunk::CopyRows(dstCol, srcCol, &self.offsets);
//         }
//
//         if self.baseJoinProbe.lUsed.is_empty() {
//             unsafe {
//                 (*resultChk).SetNumVirtualRows((*resultChk).NumRows() + self.offsets.len() as i32);
//             }
//         } else {
//             unsafe {
//                 (*resultChk).SetNumVirtualRows((*resultChk).NumRows());
//             }
//         }
//     }
//
// Only used for semi and anti semi join
// generateResultChkForRightBuildWithOtherCondition 按 resultRows/expectedResult 批量复制符合条件的 probe 行。
//     pub fn generateResultChkForRightBuildWithOtherCondition(
//         &mut self,
//         mut remainCap: i32,
//         chk: *mut chunk::Chunk,
//         resultRows: &[bool],
//         expectedResult: bool,
//     ) {
//         while remainCap > 0 && self.baseJoinProbe.currentProbeRow < self.baseJoinProbe.chunkRows {
//             let rowNumToTryAppend = std::cmp::min(
//                 remainCap,
//                 self.baseJoinProbe.chunkRows - self.baseJoinProbe.currentProbeRow,
//             );
//             let start = self.baseJoinProbe.currentProbeRow;
//             let end = self.baseJoinProbe.currentProbeRow + rowNumToTryAppend;
//
//             for (index, usedColIdx) in self.baseJoinProbe.lUsed.iter().enumerate() {
//                 let dstCol = unsafe { (*chk).Column(index as i32) };
//                 let srcCol = self.baseJoinProbe.currentChunk.Column(*usedColIdx);
//                 chunk::CopyExpectedRowsWithRowIDFunc(
//                     dstCol,
//                     srcCol,
//                     resultRows,
//                     expectedResult,
//                     start,
//                     end,
//                     |i| self.baseJoinProbe.usedRows[i as usize],
//                 );
//             }
//
//             if self.baseJoinProbe.lUsed.is_empty() {
// Go 在没有输出列时手动计算 virtual row num；这是 chunk 零列场景的必要收尾。
//                 let mut virtualRowNum = unsafe { (*chk).GetNumVirtualRows() };
//                 for i in start..end {
//                     if resultRows[i as usize] == expectedResult {
//                         virtualRowNum += 1;
//                     }
//                 }
//                 unsafe { (*chk).SetNumVirtualRows(virtualRowNum) };
//             } else {
//                 unsafe { (*chk).SetNumVirtualRows((*chk).NumRows()) };
//             }
//
//             self.baseJoinProbe.currentProbeRow += rowNumToTryAppend;
//             remainCap = unsafe { (*chk).RequiredRows() - (*chk).NumRows() };
//         }
//     }
// }
// */
use crate::base_join_probe::{BaseJoinProbe, WorkerResult};
use crate::joiner::{MatchResult, NaajType, Row};
use std::collections::VecDeque;

/// 存在 other condition 时，单次中间 joined chunk 允许为每个 probe 行匹配的最大 build 行数。
/// 用于限制中间结果膨胀（对应 Go 的 maxMatchedRowNum）。
pub const MAX_MATCHED_ROW_NUM: usize = 4;
#[derive(Clone)]
/// Semi / Anti Semi 共用状态：嵌入 BaseJoinProbe，并记录 build side 与匹配信息。
pub struct BaseSemiJoin {
    /// 底层 Probe 状态（哈希查找、游标等）。
    pub base: BaseJoinProbe,
    /// 是否以左侧作为 build side。
    pub is_left_side_build: bool,
    /// 各 probe 行是否已找到匹配（右侧 build 时使用）。
    pub matched: Vec<bool>,
    /// 各 probe 行匹配过程中是否遇到过 NULL（影响 NULL-aware 语义）。
    pub has_null: Vec<bool>,
    /// 因容量限制尚未处理完的 probe 行下标队列。
    pub unfinished_probe_rows: VecDeque<usize>,
}
impl BaseSemiJoin {
    /// 用 BaseJoinProbe 与 build side 标记构造共享状态。
    pub fn new(base: BaseJoinProbe, is_left_side_build: bool) -> Self {
        Self {
            base,
            is_left_side_build,
            matched: Vec::new(),
            has_null: Vec::new(),
            unfinished_probe_rows: VecDeque::new(),
        }
    }
    /// 按当前 probe Chunk 行数重建 matched / has_null，并清空未完成队列。
    pub fn reset_probe_state(&mut self) {
        let count = self.base.probe_chunk.len();
        self.matched.clear();
        self.matched.resize(count, false);
        self.has_null.clear();
        self.has_null.resize(count, false);
        self.unfinished_probe_rows.clear();
    }
    /// 对指定 probe 行与其候选 build 行做匹配，更新 matched/has_null，必要时入队未完成行。
    pub fn match_probe_row(
        &mut self,
        probe_index: usize,
        output: &mut Vec<Row>,
        naaj: NaajType,
    ) -> Result<MatchResult, String> {
        let outer = self.base.probe_chunk[probe_index].clone();
        let inners = self.base.candidate_rows(probe_index);
        let result = self
            .base
            .context
            .joiner
            .try_to_match_inners(&outer, &inners, output, naaj)?;
        self.matched[probe_index] |= result.matched;
        self.has_null[probe_index] |= result.has_null;
        // 候选未全部消费完时，稍后继续处理该 probe 行。
        if result.consumed < inners.len() {
            self.unfinished_probe_rows.push_back(probe_index);
        }
        Ok(result)
    }
    /// 将 matched 标志等于 expected_match 的 probe 行交给 Joiner.on_miss_match 输出。
    pub fn produce_probe_mismatches(&self, expected_match: bool, output: &mut Vec<Row>) {
        for (index, matched) in self.matched.iter().enumerate() {
            if *matched == expected_match {
                self.base.context.joiner.on_miss_match(
                    self.has_null[index],
                    &self.base.probe_chunk[index],
                    output,
                );
            }
        }
    }
    /// 扫描 build 行表，输出 used 标记等于 expected_used 的行（容量受 max_chunk_size 限制）。
    pub fn scan_build_rows(&mut self, expected_used: bool) -> WorkerResult {
        let mut rows = Vec::new();
        while self.base.scan_row_index < self.base.context.build_rows.len()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.scan_row_index;
            self.base.scan_row_index += 1;
            if self.base.context.build_row_used[index] == expected_used {
                rows.push(self.base.context.build_rows[index].clone());
            }
        }
        WorkerResult { rows, error: None }
    }
}
