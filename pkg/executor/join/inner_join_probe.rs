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

// Hash Join V2 的 Inner Join（内连接）探测（probe）实现。
//
// `InnerJoinProbe` 嵌入 `BaseJoinProbe`：按探测侧行查找构建侧候选，
// 经 Joiner 做 key 匹配与 other condition（附加条件）过滤后写出结果。
// Inner join 不需要 probe 结束后扫描整张 row table。

// hash join v2 inner join probe：按 probe row 查 build-side row table、填充 join chunk，
//
// InnerJoinProbe 对应 Go innerJoinProbe，仅嵌入 baseJoinProbe。
// pub struct InnerJoinProbe {
//     pub base: BaseJoinProbe,
// }
//
// impl InnerJoinProbe {
// Probe 对应 Go innerJoinProbe.Probe：在结果 chunk 未满时不断查找当前 probe row 的候选 build rows。
//     pub fn probe(&mut self, join_result: &mut HashJoinWorkerResult, sql_killer: &sqlkiller::SQLKiller) -> (bool, &mut HashJoinWorkerResult) {
//         if join_result.chk.IsFull() {
//             return (true, join_result);
//         }
//         let has_other_condition = self.base.ctx.hasOtherCondition();
//         let (mut joined_chk, mut remain_cap, err) = self.base.prepareForProbe(join_result.chk.clone());
//         if let Some(err) = err {
//             join_result.err = Some(err);
//             return (false, join_result);
//         }
//         let meta = self.base.ctx.hashTableMeta.clone();
//         let is_in_complete_chunk = joined_chk.IsInCompleteChunk();
// Go 这里保护 virtual rows：probe 阶段强制把 joinedChk 标记为 incomplete chunk，退出时恢复原状态。
//         joined_chk.SetNumVirtualRows(joined_chk.NumRows());
//         joined_chk.SetInCompleteChunk(true);
//         defer(|| joined_chk.SetInCompleteChunk(is_in_complete_chunk));
//
//         let tag_helper = self.base.ctx.hashTableContext.tagHelper.clone();
//         while remain_cap > 0 && self.base.currentProbeRow < self.base.chunkRows {
//             if self.base.matchedRowsHeaders[self.base.currentProbeRow] != 0 {
//                 let candidate_row = tag_helper.toUnsafePointer(self.base.matchedRowsHeaders[self.base.currentProbeRow]);
//                 if isKeyMatched(meta.keyMode, self.base.serializedKeys[self.base.currentProbeRow].clone(), candidate_row, meta.clone()) {
// key matched 后把 build side row 从行格式转换为列格式，供 Joiner/other condition 使用。
//                     self.base.appendBuildRowToCachedBuildRowsV1(self.base.currentProbeRow, candidate_row, &mut joined_chk, 0, has_other_condition);
//                     self.base.matchedRowsForCurrentProbeRow += 1;
//                     remain_cap -= 1;
//                 } else {
//                     self.base.probeCollision += 1;
//                 }
// Go 的 row table 用链表挂同 hash 值的行，这里推进到下一条候选地址。
//                 self.base.matchedRowsHeaders[self.base.currentProbeRow] =
//                     getNextRowAddress(candidate_row, &tag_helper, self.base.matchedRowsHashValue[self.base.currentProbeRow]);
//             } else {
//                 self.base.finishLookupCurrentProbeRow();
//                 self.base.currentProbeRow += 1;
//             }
//         }
//
//         if let Some(err) = checkSQLKiller(sql_killer, "killedDuringProbe") {
//             join_result.err = Some(err);
//             return (false, join_result);
//         }
//         self.base.finishCurrentLookupLoop(&mut joined_chk);
//
//         if self.base.ctx.hasOtherCondition() && joined_chk.NumRows() > 0 {
// Go 使用 VectorizedFilter 计算 other condition，再把筛选后结果构造到最终 joinResult.chk。
//             self.base.selected.clear();
//             match expression::VectorizedFilter(
//                 self.base.ctx.SessCtx.GetExprCtx().GetEvalCtx(),
//                 self.base.ctx.SessCtx.GetSessionVars().EnableVectorizedExpression,
//                 self.base.ctx.OtherCondition.clone(),
//                 chunk::NewIterator4Chunk(joined_chk.clone()),
//                 self.base.selected.clone(),
//             ) {
//                 Ok(selected) => self.base.selected = selected,
//                 Err(err) => {
//                     join_result.err = Some(err);
//                     return (false, join_result);
//                 }
//             }
//             join_result.err = self.base.buildResultAfterOtherCondition(&mut join_result.chk, joined_chk);
//         }
// 没有 other condition 时，joinedChk 已经就是最终结果，保持 Go 的快速路径。
//         if join_result.err.is_some() {
//             return (false, join_result);
//         }
//         (true, join_result)
//     }
//
// NeedScanRowTable 对应 Go：inner join probe 不需要在 probe 结束后扫描整张 row table。
//     pub fn need_scan_row_table(&self) -> bool {
//         false
//     }
//
// ScanRowTable 对应 Go：inner join 不应调用该路径，保留 panic 语义。
//     pub fn scan_row_table(&self, _result: &mut HashJoinWorkerResult, _killer: &sqlkiller::SQLKiller) -> &mut HashJoinWorkerResult {
//         panic!("should not reach here");
//     }
//
// InitForScanRowTable 对应 Go：inner join 不需要初始化扫描状态。
//     pub fn init_for_scan_row_table(&self) {
//         panic!("should not reach here");
//     }
//
// IsScanRowTableDone 对应 Go：inner join 不存在扫描完成状态。
//     pub fn is_scan_row_table_done(&self) -> bool {
//         panic!("should not reach here");
//     }
// }
// */
use crate::base_join_probe::{BaseJoinProbe, Probe, WorkerResult};
use crate::joiner::NaajType;
/// Inner Join 探测实现：委托 `BaseJoinProbe` 管理探测 chunk 与候选行。
#[derive(Clone)]
pub struct InnerJoinProbe {
    /// 共享探测状态（当前行、碰撞计数、Joiner 上下文等）。
    pub base: BaseJoinProbe,
}
impl Probe for InnerJoinProbe {
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.base.set_chunk_for_probe(chunk)
    }
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.base.set_restored_chunk_for_probe(chunk)
    }
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.base.spill_remaining_probe_chunks()
    }
    /// 在结果未满时逐行探测：取候选内表行，经 Joiner 匹配后推进探测游标。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 结果 chunk 容量未满且当前探测 chunk 未完成时持续匹配。
        while !self.base.is_current_chunk_probe_done()
            && rows.len() < self.base.context.max_chunk_size
        {
            let index = self.base.current_probe_row;
            let outer = self.base.probe_chunk[index].clone();
            let candidate_start = self.base.current_candidate;
            let candidate_end = (candidate_start
                + (self.base.context.max_chunk_size - rows.len()).max(1))
            .min(self.base.matched_rows[index].len());
            let inners: Vec<_> = self.base.matched_rows[index][candidate_start..candidate_end]
                .iter()
                .map(|build| self.base.context.build_rows[*build].clone())
                .collect();
            match self.base.context.joiner.try_to_match_inners(
                &outer,
                &inners,
                &mut rows,
                NaajType::Unknown,
            ) {
                Ok(result) => {
                    // 未真正 key 命中的候选计为 probe collision。
                    self.base.probe_collision +=
                        inners.len().saturating_sub(usize::from(result.matched)) as u64;
                    self.base.current_candidate += result.consumed;
                    if result.matched {
                        self.base.mark_build_rows_used(index);
                    }
                    if self.base.current_candidate >= self.base.matched_rows[index].len() {
                        self.base.finish_current_lookup_loop();
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
        WorkerResult { rows, error: None }
    }
    /// Inner join 不需要在探测结束后扫描整张构建侧 row table。
    fn need_scan_row_table(&self) -> bool {
        false
    }
    fn init_for_scan_row_table(&mut self) {
        panic!("should not reach here");
    }
    /// Inner join 不应走扫描路径，与 Go 一样视为不可达。
    fn scan_row_table(&mut self) -> WorkerResult {
        panic!("should not reach here");
    }
    fn is_scan_row_table_done(&self) -> bool {
        panic!("should not reach here");
    }
    fn is_current_chunk_probe_done(&self) -> bool {
        self.base.is_current_chunk_probe_done()
    }
    fn reset_probe(&mut self) {
        self.base.reset_probe();
    }
}
