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

// Left Outer Semi / Anti Semi Join 的 Hash Join probe 实现。
//
// Left Outer Semi Join（左外半连接）为左表每一行输出匹配标记（true/false/NULL），
// 而不是展开右表列；Anti 变体对匹配结果取反。null-aware（空值感知）路径会按
// `NaajType` 处理三值逻辑。对应 Go 的 `leftOuterSemiJoinProbe`。

// hash join v2 的 left outer semi/anti semi probe 流程。
// leftOuterSemiJoinProbe 对应 Go 结构体，嵌入 baseSemiJoin 并记录每个 probe 行是否得到 NULL 匹配结果。
// pub struct leftOuterSemiJoinProbe {
//     pub baseSemiJoin: baseSemiJoin,
// isNullRows marks whether the left side row matched result is null
//     pub isNullRows: Vec<bool>,
// isAnti marks whether the join is anti semi join
//     pub isAnti: bool,
// }
// newLeftOuterSemiJoinProbe 对应 Go 构造函数，保持 baseSemiJoin 的初始化入口。
// pub fn newLeftOuterSemiJoinProbe(base: baseJoinProbe, isAnti: bool) -> Box<leftOuterSemiJoinProbe> {
//     Box::new(leftOuterSemiJoinProbe {
//         baseSemiJoin: *newBaseSemiJoin(base, false),
//         isNullRows: Vec::new(),
//         isAnti,
//     })
// }
// impl leftOuterSemiJoinProbe {
// SetChunkForProbe 对应 Go 方法：先设置 probe chunk，再重置本类型的匹配状态。
//     pub fn SetChunkForProbe(&mut self, chunk: &mut chunk::Chunk) -> Result<(), errors::Error> {
//         self.baseSemiJoin.baseJoinProbe.SetChunkForProbe(chunk)?;
//         self.resetProbeState();
//         Ok(())
//     }
// SetRestoredChunkForProbe 对应 Go 的 spill 恢复路径，同样需要重新初始化本轮状态。
//     pub fn SetRestoredChunkForProbe(&mut self, chunk: &mut chunk::Chunk) -> Result<(), errors::Error> {
//         self.baseSemiJoin.baseJoinProbe.SetRestoredChunkForProbe(chunk)?;
//         self.resetProbeState();
//         Ok(())
//     }
// resetProbeState 对应 Go 方法：isNullRows 长度跟随当前 chunkRows，baseSemiJoin 状态随后清理。
//     pub fn resetProbeState(&mut self) {
//         self.isNullRows.clear();
//         for _ in 0..self.baseSemiJoin.baseJoinProbe.chunkRows {
//             self.isNullRows.push(false);
//         }
//         self.baseSemiJoin.resetProbeState();
//     }
// NeedScanRowTable 对应 Go 实现：left outer semi join 当前不扫描整张 row table。
//     pub fn NeedScanRowTable(&self) -> bool {
//         false
//     }
// IsScanRowTableDone 保留 Go 的防御性 panic，说明该路径不应被调用。
//     pub fn IsScanRowTableDone(&self) -> bool {
//         panic!("should not reach here")
//     }
// InitForScanRowTable 保留 Go 的不可达路径。
//     pub fn InitForScanRowTable(&mut self) {
//         panic!("should not reach here")
//     }
// ScanRowTable 对应 Go 方法：直接返回已有 joinResult。
//     pub fn ScanRowTable(
//         &mut self,
//         joinResult: *mut hashjoinWorkerResult,
//         _sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> *mut hashjoinWorkerResult {
//         joinResult
//     }
// Probe 对应 Go 主入口：准备输出 chunk 后按是否存在 other condition 选择 probe 策略。
//     pub fn Probe(
//         &mut self,
//         joinResult: *mut hashjoinWorkerResult,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> (bool, *mut hashjoinWorkerResult) {
//         let (joinedChk, remainCap, err) = self.baseSemiJoin.prepareForProbe(unsafe { &mut (*joinResult).chk });
//         if let Some(err) = err {
//             unsafe { (*joinResult).err = Some(err) };
//             return (false, joinResult);
//         }
//         let err = if self.baseSemiJoin.ctx.hasOtherCondition() {
//             self.probeWithOtherCondition(unsafe { &mut (*joinResult).chk }, joinedChk, remainCap, sqlKiller)
//         } else {
//             self.probeWithoutOtherCondition(unsafe { &mut (*joinResult).chk }, joinedChk, remainCap, sqlKiller)
//         };
//         if let Err(err) = err {
//             unsafe { (*joinResult).err = Some(err) };
//             return (false, joinResult);
//         }
//         (true, joinResult)
//     }
// probeWithOtherCondition 对应 Go 分支：先消化上次未完成的 build 行，再按剩余容量构造结果。
//     pub fn probeWithOtherCondition(
//         &mut self,
//         chk: &mut chunk::Chunk,
//         joinedChk: &mut chunk::Chunk,
//         remainCap: usize,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> Result<(), errors::Error> {
//         if !self.baseSemiJoin.unFinishedProbeRowIdxQueue.IsEmpty() {
//             self.produceResult(joinedChk, sqlKiller)?;
//             self.baseSemiJoin.currentProbeRow = 0;
//         }
//         if self.baseSemiJoin.unFinishedProbeRowIdxQueue.IsEmpty() {
//             let startProbeRow = self.baseSemiJoin.currentProbeRow;
//             self.baseSemiJoin.currentProbeRow = std::cmp::min(startProbeRow + remainCap, self.baseSemiJoin.chunkRows);
//             self.buildResult(chk, startProbeRow);
//         }
//         Ok(())
//     }
// produceResult 对应 Go 方法：拼接 probe/build 行，执行 other condition，并回写 matched/null 标记。
//     pub fn produceResult(
//         &mut self,
//         joinedChk: &mut chunk::Chunk,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> Result<(), errors::Error> {
// Go 第三个参数固定为 true，因为 left outer semi join v2 当前只支持右侧作为 build 侧。
//         self.baseSemiJoin.concatenateProbeAndBuildRows(joinedChk, sqlKiller, true)?;
//         if joinedChk.NumRows() > 0 {
//             let (selected, isNulls) = expression::VecEvalBool(
//                 self.baseSemiJoin.ctx.SessCtx.GetExprCtx().GetEvalCtx(),
//                 self.baseSemiJoin.ctx.SessCtx.GetSessionVars().EnableVectorizedExpression,
//                 &self.baseSemiJoin.ctx.OtherCondition,
//                 joinedChk,
//                 self.baseSemiJoin.selected.clone(),
//                 self.baseSemiJoin.isNulls.clone(),
//             )?;
//             self.baseSemiJoin.selected = selected;
//             self.baseSemiJoin.isNulls = isNulls;
//             for i in 0..joinedChk.NumRows() {
//                 let probe_idx = self.baseSemiJoin.rowIndexInfos[i].probeRowIndex;
//                 if self.baseSemiJoin.selected[i] {
//                     self.baseSemiJoin.isMatchedRows[probe_idx] = true;
//                 }
//                 if self.baseSemiJoin.isNulls[i] {
//                     self.isNullRows[probe_idx] = true;
//                 }
//             }
//         }
//         Ok(())
//     }
// probeWithoutOtherCondition 对应无 other condition 快路径：只需判断 hash key 是否匹配。
//     pub fn probeWithoutOtherCondition(
//         &mut self,
//         _chk: &mut chunk::Chunk,
//         joinedChk: &mut chunk::Chunk,
//         mut remainCap: usize,
//         sqlKiller: *mut sqlkiller::SQLKiller,
//     ) -> Result<(), errors::Error> {
//         let meta = self.baseSemiJoin.ctx.hashTableMeta;
//         let startProbeRow = self.baseSemiJoin.currentProbeRow;
//         let tagHelper = self.baseSemiJoin.ctx.hashTableContext.tagHelper;
//         while remainCap > 0 && self.baseSemiJoin.currentProbeRow < self.baseSemiJoin.chunkRows {
//             let row = self.baseSemiJoin.currentProbeRow;
//             if self.baseSemiJoin.matchedRowsHeaders[row] != 0 {
//                 let candidateRow = tagHelper.toUnsafePointer(self.baseSemiJoin.matchedRowsHeaders[row]);
//                 if !isKeyMatched(meta.keyMode, self.baseSemiJoin.serializedKeys[row].clone(), candidateRow, meta) {
//                     self.baseSemiJoin.probeCollision += 1;
//                     self.baseSemiJoin.matchedRowsHeaders[row] =
//                         getNextRowAddress(candidateRow, tagHelper, self.baseSemiJoin.matchedRowsHashValue[row]);
//                     continue;
//                 }
//                 self.baseSemiJoin.isMatchedRows[row] = true;
//             }
//             self.baseSemiJoin.matchedRowsHeaders[row] = 0;
//             remainCap -= 1;
//             self.baseSemiJoin.currentProbeRow += 1;
//         }
//         checkSQLKiller(sqlKiller, "killedDuringProbe")?;
//         self.buildResult(joinedChk, startProbeRow);
//         Ok(())
//     }
// buildResult 对应 Go 输出构造：复制左侧列后追加 semi join 的 0/1/NULL 标志列。
//     pub fn buildResult(&mut self, chk: &mut chunk::Chunk, startProbeRow: usize) {
//         let mut selected: Option<Vec<bool>> = None;
//         if startProbeRow == 0
//             && self.baseSemiJoin.currentProbeRow == self.baseSemiJoin.chunkRows
//             && self.baseSemiJoin.currentChunk.Sel().is_none()
//             && chk.NumRows() == 0
//             && self.baseSemiJoin.spilledIdx.is_empty()
//         {
// Go TODO: 可以直接复制 Column 指针；这里保留 CopyConstruct 的参数形状。
//             for (index, colIndex) in self.baseSemiJoin.lUsed.iter().enumerate() {
//                 self.baseSemiJoin.currentChunk.Column(*colIndex).CopyConstruct(chk.Column(index));
//             }
//         } else {
//             let mut flags = vec![false; self.baseSemiJoin.chunkRows];
//             for i in startProbeRow..self.baseSemiJoin.currentProbeRow {
//                 flags[i] = true;
//             }
//             for spilledIdx in &self.baseSemiJoin.spilledIdx {
// ignore spilled rows
//                 flags[*spilledIdx] = false;
//             }
//             for (index, colIndex) in self.baseSemiJoin.lUsed.iter().enumerate() {
//                 let dstCol = chk.Column(index);
//                 let srcCol = self.baseSemiJoin.currentChunk.Column(*colIndex);
//                 chunk::CopySelectedRowsWithRowIDFunc(dstCol, srcCol, &flags, 0, flags.len(), |i| {
//                     self.baseSemiJoin.usedRows[i]
//                 });
//             }
//             selected = Some(flags);
//         }
//         for i in startProbeRow..self.baseSemiJoin.currentProbeRow {
//             if selected.as_ref().is_some_and(|flags| !flags[i]) {
//                 continue;
//             }
// isAnti 决定 matched/unmatched 输出位是否取反，NULL 仍保持 NULL。
//             if self.isAnti {
//                 if self.baseSemiJoin.isMatchedRows[i] {
//                     chk.AppendInt64(self.baseSemiJoin.lUsed.len(), 0);
//                 } else if self.isNullRows[i] {
//                     chk.AppendNull(self.baseSemiJoin.lUsed.len());
//                 } else {
//                     chk.AppendInt64(self.baseSemiJoin.lUsed.len(), 1);
//                 }
//             } else if self.baseSemiJoin.isMatchedRows[i] {
//                 chk.AppendInt64(self.baseSemiJoin.lUsed.len(), 1);
//             } else if self.isNullRows[i] {
//                 chk.AppendNull(self.baseSemiJoin.lUsed.len());
//             } else {
//                 chk.AppendInt64(self.baseSemiJoin.lUsed.len(), 0);
//             }
//         }
//         chk.SetNumVirtualRows(chk.NumRows());
//     }
// IsCurrentChunkProbeDone 对应 Go 方法：有 other condition 且还有未完成队列时不能结束。
//     pub fn IsCurrentChunkProbeDone(&self) -> bool {
//         if self.baseSemiJoin.ctx.hasOtherCondition() && !self.baseSemiJoin.unFinishedProbeRowIdxQueue.IsEmpty() {
//             return false;
//         }
//         self.baseSemiJoin.baseJoinProbe.IsCurrentChunkProbeDone()
//     }
// }
// */
use crate::base_join_probe::{Probe, WorkerResult};
use crate::base_semi_join::BaseSemiJoin;
use crate::joiner::NaajType;

/// Left Outer Semi / Anti Semi Join 的 probe 状态。
///
/// 嵌入 `BaseSemiJoin` 复用匹配队列与标记列输出；`anti` 决定标记是否取反，
/// `null_aware` 启用空值感知半连接（NULL-Aware Anti/Semi Join）路径。
#[derive(Clone)]
pub struct LeftOuterSemiJoinProbe {
    /// Semi Join 共享基座（含 base probe 与匹配状态）。
    pub semi: BaseSemiJoin,
    /// 为 true 时按 Anti Semi 语义输出匹配标记。
    pub anti: bool,
    /// 为 true 时按 null-aware 三值逻辑探测。
    pub null_aware: bool,
}

impl Probe for LeftOuterSemiJoinProbe {
    /// 设置本轮 probe chunk，并重置 semi 匹配状态。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.semi.base.set_chunk_for_probe(chunk)?;
        self.semi.reset_probe_state();
        Ok(())
    }
    /// Spill 恢复后的 probe chunk：复用 `set_chunk_for_probe`。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.set_chunk_for_probe(chunk)
    }
    /// 将尚未探测完的 probe 行整块 spill 出去。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.semi.base.spill_remaining_probe_chunks()
    }
    /// 对当前 probe chunk 逐行匹配 build 侧，输出左行 + 标记列。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在输出 chunk 容量允许时持续推进当前 probe 行。
        while self.semi.base.current_probe_row < self.semi.base.probe_chunk.len()
            && rows.len() < self.semi.base.context.max_chunk_size
        {
            let index = self.semi.base.current_probe_row;
            let probe_key_has_null = self.semi.base.context.probe_key_indices.iter().any(|key| {
                matches!(
                    self.semi.base.probe_chunk[index].get(*key),
                    Some(crate::row_table_builder::Value::Null)
                )
            });
            let build_has_null_key =
                self.semi.base.context.build_rows.iter().any(|row| {
                    self.semi.base.context.build_key_indices.iter().any(|key| {
                        matches!(row.get(*key), Some(crate::row_table_builder::Value::Null))
                    })
                });
            // null-aware 路径传入具体 NaajType；否则走普通半连接匹配。
            let naaj = if self.null_aware {
                if probe_key_has_null {
                    NaajType::LeftHasNullRightNotNull
                } else {
                    NaajType::LeftNotNullRightNotNull
                }
            } else {
                NaajType::Unknown
            };
            match self.semi.match_probe_row(index, &mut rows, naaj) {
                Ok(result) => {
                    // 未匹配时由 Joiner 按 has_null 写出 false 或 NULL 标记。
                    if !result.matched {
                        let has_null = if self.null_aware {
                            // NAAJ 的 NULL 只来自 join key；inner filter 的 NULL 仍按未命中处理。
                            probe_key_has_null || build_has_null_key
                        } else {
                            result.has_null
                        };
                        self.semi.base.context.joiner.on_miss_match(
                            has_null,
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
            self.semi.base.finish_current_lookup_loop();
        }
        WorkerResult { rows, error: None }
    }
    /// Left Outer Semi 不需要扫描整张 build row table 补未匹配行。
    fn need_scan_row_table(&self) -> bool {
        false
    }
    fn init_for_scan_row_table(&mut self) {
        panic!("should not reach here")
    }
    fn scan_row_table(&mut self) -> WorkerResult {
        WorkerResult::default()
    }
    fn is_scan_row_table_done(&self) -> bool {
        panic!("should not reach here")
    }
    /// 有未完成 other-condition 队列或 base 未结束时，当前 chunk 仍需继续 probe。
    fn is_current_chunk_probe_done(&self) -> bool {
        self.semi.unfinished_probe_rows.is_empty() && self.semi.base.is_current_chunk_probe_done()
    }
    /// 重置 base 与 semi 两侧的 probe 状态，供下一轮复用。
    fn reset_probe(&mut self) {
        self.semi.base.reset_probe();
        self.semi.reset_probe_state();
    }
}
