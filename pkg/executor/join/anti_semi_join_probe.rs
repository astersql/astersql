// Copyright 2026 AsterSQL.
// Anti Semi Join（反半连接）的 Probe 实现。
//
// Anti Semi Join：仅当左侧行在右侧找不到匹配时才输出该左侧行（类似 `NOT EXISTS` /
// `NOT IN`）。本文件在 Hash Join 的 Probe 阶段实现该语义：右侧为 build side 时
// 输出未匹配 probe 行；左侧为 build side 时在扫描 build 表阶段输出未使用的 build 行。
/*
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

// antiSemiJoinProbe 对应 Go 结构体，嵌入 baseSemiJoin 以复用 semi/anti semi join 状态。
pub struct antiSemiJoinProbe {
    pub baseSemiJoin: baseSemiJoin,
}

// newAntiSemiJoinProbe 对应 Go 构造函数；存在 other condition 时提前分配 isNulls 缓冲。
pub fn newAntiSemiJoinProbe(base: baseJoinProbe, isLeftSideBuild: bool) -> Box<antiSemiJoinProbe> {
    let mut ret = Box::new(antiSemiJoinProbe {
        baseSemiJoin: *newBaseSemiJoin(base, isLeftSideBuild),
    });

    if ret.baseSemiJoin.baseJoinProbe.ctx.hasOtherCondition() {
        ret.baseSemiJoin.isNulls = Vec::with_capacity(chunk::InitialCapacity as usize);
    }

    ret
}

impl antiSemiJoinProbe {
    // InitForScanRowTable 对应左侧作为 build side 时扫描 row table 的初始化。
    pub fn InitForScanRowTable(&mut self) {
        if !self.baseSemiJoin.isLeftSideBuild {
            panic!("should not reach here");
        }
        self.baseSemiJoin.rowIter = Some(commonInitForScanRowTable(&mut self.baseSemiJoin.baseJoinProbe));
    }

    // NeedScanRowTable 在 anti semi join 中仅左侧 build side 需要扫描未被使用的 build row。
    pub fn NeedScanRowTable(&self) -> bool {
        self.baseSemiJoin.isLeftSideBuild
    }

    // IsScanRowTableDone 保留 Go 的防御性 panic：非左侧 build side 不应调用。
    pub fn IsScanRowTableDone(&self) -> bool {
        if !self.baseSemiJoin.isLeftSideBuild {
            panic!("should not reach here");
        }
        unsafe { (*self.baseSemiJoin.rowIter.unwrap()).isEnd() }
    }

    // ScanRowTable 扫描 build row table，输出未被 probe 命中的左侧行。
    pub fn ScanRowTable(
        &mut self,
        mut joinResult: *mut hashjoinWorkerResult,
        sqlKiller: *mut sqlkiller::SQLKiller,
    ) -> *mut hashjoinWorkerResult {
        if !self.baseSemiJoin.isLeftSideBuild {
            panic!("should not reach here");
        }
        if unsafe { (*(*joinResult).chk).IsFull() } {
            return joinResult;
        }
        if self.baseSemiJoin.rowIter.is_none() {
            panic!("scanRowTable before init");
        }
        self.baseSemiJoin.baseJoinProbe.nextCachedBuildRowIndex = 0;
        let meta = self.baseSemiJoin.baseJoinProbe.ctx.hashTableMeta;
        let mut insertedRows = 0;
        let remainCap = unsafe { (*(*joinResult).chk).RequiredRows() - (*(*joinResult).chk).NumRows() };
        while insertedRows < remainCap && !unsafe { (*self.baseSemiJoin.rowIter.unwrap()).isEnd() } {
            let currentRow = unsafe { (*self.baseSemiJoin.rowIter.unwrap()).getValue() };
            if !meta.isCurrentRowUsed(currentRow) {
                // append build side of this row
                // Go 这里把未使用 build row 写入批量缓存，随后 batchConstructBuildRows 一次性构造列。
                self.baseSemiJoin.baseJoinProbe.appendBuildRowToCachedBuildRowsV1(
                    0,
                    currentRow,
                    unsafe { (*joinResult).chk },
                    0,
                    false,
                );
                insertedRows += 1;
            }
            unsafe { (*self.baseSemiJoin.rowIter.unwrap()).next() };
        }
        let err = checkSQLKiller(sqlKiller, "killedDuringProbe");
        if let Err(e) = err {
            unsafe { (*joinResult).err = Some(e) };
            return joinResult;
        }
        if self.baseSemiJoin.baseJoinProbe.nextCachedBuildRowIndex > 0 {
            self.baseSemiJoin.baseJoinProbe.batchConstructBuildRows(
                unsafe { (*joinResult).chk },
                0,
                false,
            );
        }
        joinResult
    }

    // ResetProbe 清理 row iterator 并复用 baseJoinProbe 的缓存重置。
    pub fn ResetProbe(&mut self) {
        self.baseSemiJoin.rowIter = None;
        self.baseSemiJoin.baseJoinProbe.ResetProbe();
    }

    // resetProbeState 在 baseSemiJoin 基础上处理 anti semi join 的 spill 特例。
    pub fn resetProbeState(&mut self) {
        self.baseSemiJoin.resetProbeState();

        if !self.baseSemiJoin.isLeftSideBuild {
            if self.baseSemiJoin.baseJoinProbe.ctx.spillHelper.isSpillTriggered() {
                for idx in &self.baseSemiJoin.baseJoinProbe.spilledIdx {
                    // We see rows that have be spilled as matched rows
                    // Go 把已 spill 的 probe 行视作 matched，避免当前分区输出错误的 anti 结果。
                    self.baseSemiJoin.isMatchedRows[*idx as usize] = true;
                }
            }
        }
    }

    // SetChunkForProbe 先执行 baseJoinProbe 的序列化 key / lookup 预处理，再重置 anti semi 状态。
    pub fn SetChunkForProbe(&mut self, chk: *mut chunk::Chunk) -> Result<(), errors::Error> {
        self.baseSemiJoin.baseJoinProbe.SetChunkForProbe(chk)?;
        self.resetProbeState();
        Ok(())
    }

    // SetRestoredChunkForProbe 对应从磁盘恢复 probe chunk 的预处理路径。
    pub fn SetRestoredChunkForProbe(&mut self, chk: *mut chunk::Chunk) -> Result<(), errors::Error> {
        self.baseSemiJoin.baseJoinProbe.SetRestoredChunkForProbe(chk)?;
        self.resetProbeState();
        Ok(())
    }

    // Probe 根据 build side 和是否存在 other condition 分派到四种 probe 逻辑。
    pub fn Probe(
        &mut self,
        mut joinResult: *mut hashjoinWorkerResult,
        sqlKiller: *mut sqlkiller::SQLKiller,
    ) -> (bool, *mut hashjoinWorkerResult) {
        if unsafe { (*(*joinResult).chk).IsFull() } {
            return (true, joinResult);
        }

        let prepared = self.baseSemiJoin.baseJoinProbe.prepareForProbe(unsafe { (*joinResult).chk });
        let (joinedChk, remainCap) = match prepared {
            Ok(v) => v,
            Err(err) => {
                unsafe { (*joinResult).err = Some(err) };
                return (false, joinResult);
            }
        };

        let hasOtherCondition = self.baseSemiJoin.baseJoinProbe.ctx.hasOtherCondition();
        let err = if self.baseSemiJoin.isLeftSideBuild {
            if hasOtherCondition {
                self.probeForLeftSideBuildHasOtherCondition(joinedChk, sqlKiller)
            } else {
                self.probeForLeftSideBuildNoOtherCondition(sqlKiller)
            }
        } else if hasOtherCondition {
            self.probeForRightSideBuildHasOtherCondition(
                unsafe { (*joinResult).chk },
                joinedChk,
                remainCap,
                sqlKiller,
            )
        } else {
            self.probeForRightSideBuildNoOtherCondition(unsafe { (*joinResult).chk }, remainCap, sqlKiller)
        };
        if let Err(e) = err {
            unsafe { (*joinResult).err = Some(e) };
            return (false, joinResult);
        }
        (true, joinResult)
    }

    // probeForLeftSideBuildHasOtherCondition 拼接候选行后用 OtherCondition 过滤，并把通过/NULL 的 build 行标记 used。
    pub fn probeForLeftSideBuildHasOtherCondition(
        &mut self,
        joinedChk: *mut chunk::Chunk,
        sqlKiller: *mut sqlkiller::SQLKiller,
    ) -> Result<(), errors::Error> {
        self.baseSemiJoin.concatenateProbeAndBuildRows(joinedChk, sqlKiller, false)?;

        if self.baseSemiJoin.unFinishedProbeRowIdxQueue.as_ref().unwrap().IsEmpty() {
            // To avoid `Previous chunk is not probed yet` error
            self.baseSemiJoin.baseJoinProbe.currentProbeRow = self.baseSemiJoin.baseJoinProbe.chunkRows;
        }

        let meta = self.baseSemiJoin.baseJoinProbe.ctx.hashTableMeta;
        if unsafe { (*joinedChk).NumRows() } > 0 {
            let (selected, isNulls) = expression::VecEvalBool(
                self.baseSemiJoin.baseJoinProbe.ctx.SessCtx.GetExprCtx().GetEvalCtx(),
                self.baseSemiJoin.baseJoinProbe.ctx.SessCtx.GetSessionVars().EnableVectorizedExpression,
                self.baseSemiJoin.baseJoinProbe.ctx.OtherCondition,
                joinedChk,
                &mut self.baseSemiJoin.baseJoinProbe.selected,
                &mut self.baseSemiJoin.isNulls,
            )?;
            self.baseSemiJoin.baseJoinProbe.selected = selected;
            self.baseSemiJoin.isNulls = isNulls;

            for (index, result) in self.baseSemiJoin.baseJoinProbe.selected.iter().enumerate() {
                if *result || self.baseSemiJoin.isNulls[index] {
                    // Go 通过 unsafe 把 uintptr 还原为 row 起始地址；这里保留 used flag 的语义。
                    meta.setUsedFlag(self.baseSemiJoin.baseJoinProbe.rowIndexInfos[index].buildRowStart);
                }
            }
        }

        Ok(())
    }

    // probeForLeftSideBuildNoOtherCondition 不构造 joined chunk，只沿 hash 链匹配 key 并设置 used flag。
    pub fn probeForLeftSideBuildNoOtherCondition(
        &mut self,
        sqlKiller: *mut sqlkiller::SQLKiller,
    ) -> Result<(), errors::Error> {
        let meta = self.baseSemiJoin.baseJoinProbe.ctx.hashTableMeta;
        let tagHelper = self.baseSemiJoin.baseJoinProbe.ctx.hashTableContext.tagHelper;

        let mut loopCnt = 0;
        while self.baseSemiJoin.baseJoinProbe.currentProbeRow < self.baseSemiJoin.baseJoinProbe.chunkRows {
            let row = self.baseSemiJoin.baseJoinProbe.currentProbeRow as usize;
            if self.baseSemiJoin.baseJoinProbe.matchedRowsHeaders[row] != 0 {
                let candidateRow = tagHelper.toUnsafePointer(self.baseSemiJoin.baseJoinProbe.matchedRowsHeaders[row]);
                if !meta.isCurrentRowUsedWithAtomic(candidateRow) {
                    if isKeyMatched(
                        meta.keyMode,
                        &self.baseSemiJoin.baseJoinProbe.serializedKeys[row],
                        candidateRow,
                        meta,
                    ) {
                        meta.setUsedFlag(candidateRow);
                    } else {
                        self.baseSemiJoin.baseJoinProbe.probeCollision += 1;
                    }
                }
                self.baseSemiJoin.baseJoinProbe.matchedRowsHeaders[row] = getNextRowAddress(
                    candidateRow,
                    tagHelper,
                    self.baseSemiJoin.baseJoinProbe.matchedRowsHashValue[row],
                );
            } else {
                self.baseSemiJoin.baseJoinProbe.currentProbeRow += 1;
            }

            loopCnt += 1;
            if loopCnt % 2000 == 0 {
                checkSQLKiller(sqlKiller, "killedDuringProbe")?;
            }
        }

        checkSQLKiller(sqlKiller, "killedDuringProbe")
    }

    // produceResult 处理右侧 build 且存在 other condition 时的中间 joined chunk，并更新 matched 标记。
    pub fn produceResult(
        &mut self,
        joinedChk: *mut chunk::Chunk,
        sqlKiller: *mut sqlkiller::SQLKiller,
    ) -> Result<(), errors::Error> {
        self.baseSemiJoin.concatenateProbeAndBuildRows(joinedChk, sqlKiller, true)?;

        if unsafe { (*joinedChk).NumRows() } > 0 {
            let (selected, isNulls) = expression::VecEvalBool(
                self.baseSemiJoin.baseJoinProbe.ctx.SessCtx.GetExprCtx().GetEvalCtx(),
                self.baseSemiJoin.baseJoinProbe.ctx.SessCtx.GetSessionVars().EnableVectorizedExpression,
                self.baseSemiJoin.baseJoinProbe.ctx.OtherCondition,
                joinedChk,
                &mut self.baseSemiJoin.baseJoinProbe.selected,
                &mut self.baseSemiJoin.isNulls,
            )?;
            self.baseSemiJoin.baseJoinProbe.selected = selected;
            self.baseSemiJoin.isNulls = isNulls;

            for i in 0..self.baseSemiJoin.baseJoinProbe.selected.len() {
                if self.baseSemiJoin.baseJoinProbe.selected[i] || self.baseSemiJoin.isNulls[i] {
                    let probeRowIdx = self.baseSemiJoin.baseJoinProbe.rowIndexInfos[i].probeRowIndex;
                    self.baseSemiJoin.isMatchedRows[probeRowIdx as usize] = true;
                }
            }
        }
        Ok(())
    }

    // probeForRightSideBuildHasOtherCondition 先消费未完成队列，再输出未匹配 probe 行。
    pub fn probeForRightSideBuildHasOtherCondition(
        &mut self,
        chk: *mut chunk::Chunk,
        joinedChk: *mut chunk::Chunk,
        remainCap: i32,
        sqlKiller: *mut sqlkiller::SQLKiller,
    ) -> Result<(), errors::Error> {
        if !self.baseSemiJoin.unFinishedProbeRowIdxQueue.as_ref().unwrap().IsEmpty() {
            self.produceResult(joinedChk, sqlKiller)?;
            self.baseSemiJoin.baseJoinProbe.currentProbeRow = 0;
        }

        if self.baseSemiJoin.unFinishedProbeRowIdxQueue.as_ref().unwrap().IsEmpty() {
            self.baseSemiJoin.generateResultChkForRightBuildWithOtherCondition(
                remainCap,
                chk,
                &self.baseSemiJoin.isMatchedRows,
                false,
            );
        }

        Ok(())
    }

    // probeForRightSideBuildNoOtherCondition 沿匹配链判断是否命中；未命中行的物理行号写入 offsets。
    pub fn probeForRightSideBuildNoOtherCondition(
        &mut self,
        chk: *mut chunk::Chunk,
        mut remainCap: i32,
        sqlKiller: *mut sqlkiller::SQLKiller,
    ) -> Result<(), errors::Error> {
        let meta = self.baseSemiJoin.baseJoinProbe.ctx.hashTableMeta;
        let tagHelper = self.baseSemiJoin.baseJoinProbe.ctx.hashTableContext.tagHelper;
        let mut matched = false;

        if self.baseSemiJoin.offsets.capacity() == 0 {
            self.baseSemiJoin.offsets = Vec::with_capacity(remainCap as usize);
        }

        self.baseSemiJoin.offsets.clear();

        while remainCap > 0 && self.baseSemiJoin.baseJoinProbe.currentProbeRow < self.baseSemiJoin.baseJoinProbe.chunkRows {
            let row = self.baseSemiJoin.baseJoinProbe.currentProbeRow as usize;
            if self.baseSemiJoin.baseJoinProbe.matchedRowsHeaders[row] != 0 {
                let candidateRow = tagHelper.toUnsafePointer(self.baseSemiJoin.baseJoinProbe.matchedRowsHeaders[row]);
                if isKeyMatched(meta.keyMode, &self.baseSemiJoin.baseJoinProbe.serializedKeys[row], candidateRow, meta) {
                    matched = true;
                    self.baseSemiJoin.baseJoinProbe.matchedRowsHeaders[row] = 0;
                } else {
                    self.baseSemiJoin.baseJoinProbe.probeCollision += 1;
                    self.baseSemiJoin.baseJoinProbe.matchedRowsHeaders[row] =
                        getNextRowAddress(candidateRow, tagHelper, self.baseSemiJoin.baseJoinProbe.matchedRowsHashValue[row]);
                }
            } else {
                if self.baseSemiJoin.baseJoinProbe.ctx.spillHelper.isSpillTriggered()
                    && self.baseSemiJoin.isMatchedRows[row]
                {
                    // We see rows that have be spilled as matched rows
                    matched = true;
                }

                if !matched {
                    remainCap -= 1;
                    self.baseSemiJoin.offsets.push(self.baseSemiJoin.baseJoinProbe.usedRows[row]);
                }

                matched = false;
                self.baseSemiJoin.baseJoinProbe.currentProbeRow += 1;
            }
        }

        checkSQLKiller(sqlKiller, "killedDuringProbe")?;
        self.baseSemiJoin.generateResultChkForRightBuildNoOtherCondition(chk);
        Ok(())
    }

    // IsCurrentChunkProbeDone 在 other condition 队列未清空时返回 false，否则委托 baseJoinProbe。
    pub fn IsCurrentChunkProbeDone(&self) -> bool {
        if self.baseSemiJoin.baseJoinProbe.ctx.hasOtherCondition()
            && !self.baseSemiJoin.unFinishedProbeRowIdxQueue.as_ref().unwrap().IsEmpty()
        {
            return false;
        }
        self.baseSemiJoin.baseJoinProbe.IsCurrentChunkProbeDone()
    }
}
*/

use crate::base_join_probe::{Probe, WorkerResult};
use crate::base_semi_join::BaseSemiJoin;
use crate::joiner::NaajType;
#[derive(Clone)]
/// Anti Semi Join 的 Probe 执行器；内嵌 `BaseSemiJoin` 复用半连接共享状态。
pub struct AntiSemiJoinProbe {
    /// 半连接共用状态（匹配标记、未完成队列、build side 标记等）。
    pub semi: BaseSemiJoin,
}
impl Probe for AntiSemiJoinProbe {
    /// 设置本批 probe Chunk，并重置 anti semi 匹配状态。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.semi.base.set_chunk_for_probe(chunk)?;
        self.semi.reset_probe_state();
        Ok(())
    }
    /// 从 spill 恢复的 Chunk 走与普通 probe 相同的预处理路径。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.set_chunk_for_probe(chunk)
    }
    /// 将尚未处理完的 probe 行刷出为 spill Chunk 列表。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.semi.base.spill_remaining_probe_chunks()
    }
    /// 执行 anti semi probe：按 build side 分支处理匹配或输出未匹配行。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在输出容量用尽前逐行推进 current_probe_row。
        while self.semi.base.current_probe_row < self.semi.base.probe_chunk.len()
            && rows.len() < self.semi.base.context.max_chunk_size
        {
            let index = self.semi.base.current_probe_row;
            if self.semi.is_left_side_build {
                // 左侧 build：在 probe 阶段只标记被命中的 build 行，输出留到 scan_row_table。
                let build_indices = self.semi.base.matched_rows[index].clone();
                let build_rows: Vec<_> = build_indices
                    .iter()
                    .map(|build| self.semi.base.context.build_rows[*build].clone())
                    .collect();
                match self.semi.base.context.joiner.try_to_match_outers(
                    &build_rows,
                    &self.semi.base.probe_chunk[index],
                    &mut Vec::new(),
                ) {
                    Ok(statuses) => {
                        for (position, status) in statuses.iter().enumerate() {
                            if matches!(
                                status,
                                crate::joiner::OuterRowStatus::Matched
                                    | crate::joiner::OuterRowStatus::HasNull
                            ) {
                                self.semi.base.context.build_row_used[build_indices[position]] =
                                    true;
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
            } else {
                // 右侧 build：未匹配的 probe 行通过 on_miss_match 写入结果。
                match self
                    .semi
                    .match_probe_row(index, &mut Vec::new(), NaajType::Unknown)
                {
                    Ok(result) => {
                        if !result.matched {
                            self.semi.base.context.joiner.on_miss_match(
                                result.has_null,
                                &self.semi.base.probe_chunk[index],
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
            }
            self.semi.base.finish_current_lookup_loop();
        }
        WorkerResult { rows, error: None }
    }
    /// 仅左侧作为 build side 时需要事后扫描未使用的 build 行。
    fn need_scan_row_table(&self) -> bool {
        self.semi.is_left_side_build
    }
    /// 初始化 build 表扫描游标。
    fn init_for_scan_row_table(&mut self) {
        self.semi.base.scan_row_index = 0;
    }
    /// 扫描并输出未被 probe 命中的 build 行（anti 语义下 expected_used=false）。
    fn scan_row_table(&mut self) -> WorkerResult {
        self.semi.scan_build_rows(false)
    }
    /// build 表扫描是否已结束。
    fn is_scan_row_table_done(&self) -> bool {
        self.semi.base.scan_row_index >= self.semi.base.context.build_rows.len()
    }
    /// 当前 probe Chunk 是否处理完毕（含 other condition 未完成队列为空）。
    fn is_current_chunk_probe_done(&self) -> bool {
        self.semi.unfinished_probe_rows.is_empty() && self.semi.base.is_current_chunk_probe_done()
    }
    /// 重置 probe 与 anti semi 共享状态，准备下一批。
    fn reset_probe(&mut self) {
        self.semi.base.reset_probe();
        self.semi.reset_probe_state();
    }
}
