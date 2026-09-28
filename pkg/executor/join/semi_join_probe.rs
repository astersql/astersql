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

// Semi Join（半连接）探测侧实现。
//
// 半连接只判断外表行是否在内表存在匹配，命中时输出外表行本身（不拼接内表列）。
// 本文件实现 `Probe`：按 build 侧在左/右分派匹配；左侧 build 时还要扫描 row table
// 输出已被标记使用的 build 行。上方大段注释块保留 Go 原版探测路径的对照说明。

// semi join probe 如何扫描 row table、过滤连接结果并生成输出；不会真正访问 TiDB 存储或执行 SQL。
//
// semiJoinProbe 对应 Go 结构体：只嵌入 baseSemiJoin。
// pub struct SemiJoinProbe {
//     pub base_semi_join: BaseSemiJoin,
// }
//
// newSemiJoinProbe 对应 Go 构造函数：从 baseJoinProbe 创建 semi join probe。
// pub fn new_semi_join_probe(base: BaseJoinProbe, is_left_side_build: bool) -> Box<SemiJoinProbe> {
//     Box::new(SemiJoinProbe {
//         base_semi_join: *new_base_semi_join(base, is_left_side_build),
//     })
// }
//
// impl SemiJoinProbe {
// InitForScanRowTable 对应左侧 build 的 row table 扫描初始化。
//     pub fn init_for_scan_row_table(&mut self) {
//         if !self.base_semi_join.is_left_side_build {
//             panic!("should not reach here");
//         }
//         self.base_semi_join.row_iter = common_init_for_scan_row_table(&mut self.base_semi_join.base_join_probe);
//     }
//
// SetChunkForProbe 对应 Go 方法：设置 probe chunk 后重置 probe 状态。
//     pub fn set_chunk_for_probe(&mut self, chk: &mut chunk::Chunk) -> Result<(), Error> {
//         self.base_semi_join.base_join_probe.set_chunk_for_probe(chk)?;
//         self.base_semi_join.reset_probe_state();
//         Ok(())
//     }
//
// SetRestoredChunkForProbe 对应 spill 恢复后的 probe chunk 设置。
//     pub fn set_restored_chunk_for_probe(&mut self, chk: &mut chunk::Chunk) -> Result<(), Error> {
//         self.base_semi_join.base_join_probe.set_restored_chunk_for_probe(chk)?;
//         self.base_semi_join.reset_probe_state();
//         Ok(())
//     }
//
// NeedScanRowTable 对应 Go 方法：只有左侧 build 的 semi join 需要扫描 row table。
//     pub fn need_scan_row_table(&self) -> bool {
//         self.base_semi_join.is_left_side_build
//     }
//
// IsScanRowTableDone 对应 Go 方法：检查 row iterator 是否结束。
//     pub fn is_scan_row_table_done(&self) -> bool {
//         if !self.base_semi_join.is_left_side_build {
//             panic!("should not reach here");
//         }
//         self.base_semi_join.row_iter.as_ref().unwrap().is_end()
//     }
//
// ScanRowTable 对应左侧 build 场景：把已使用的 build rows 追加到输出 chunk。
//     pub fn scan_row_table(
//         &mut self,
//         mut join_result: Box<HashjoinWorkerResult>,
//         sql_killer: &mut sqlkiller::SQLKiller,
//     ) -> Box<HashjoinWorkerResult> {
//         if !self.base_semi_join.is_left_side_build {
//             panic!("should not reach here");
//         }
//         if join_result.chk.is_full() {
//             return join_result;
//         }
//         if self.base_semi_join.row_iter.is_none() {
//             panic!("scanRowTable before init");
//         }
//
//         self.base_semi_join.next_cached_build_row_index = 0;
//         let meta = self.base_semi_join.ctx.hash_table_meta;
//         let mut inserted_rows = 0;
//         let remain_cap = join_result.chk.required_rows() - join_result.chk.num_rows();
//
// Go 逐行扫描 row table，遇到已使用 build row 才追加；这里保留相同推进顺序。
//         while inserted_rows < remain_cap && !self.base_semi_join.row_iter.as_ref().unwrap().is_end() {
//             let current_row = self.base_semi_join.row_iter.as_ref().unwrap().get_value();
//             if meta.is_current_row_used(current_row) {
//                 self.base_semi_join.append_build_row_to_cached_build_rows_v1(0, current_row, &mut join_result.chk, 0, false);
//                 inserted_rows += 1;
//             }
//             self.base_semi_join.row_iter.as_mut().unwrap().next();
//         }
//
//         if let Err(err) = check_sql_killer(sql_killer, "killedDuringProbe") {
//             join_result.err = Some(err);
//             return join_result;
//         }
//         if self.base_semi_join.next_cached_build_row_index > 0 {
//             self.base_semi_join.batch_construct_build_rows(&mut join_result.chk, 0, false);
//         }
//         join_result
//     }
//
// ResetProbe 对应 Go 方法：清空 row iterator 并重置基类 probe 状态。
//     pub fn reset_probe(&mut self) {
//         self.base_semi_join.row_iter = None;
//         self.base_semi_join.base_join_probe.reset_probe();
//     }
//
// Probe 对应 Go 主 probe 入口：根据 build 侧和 other condition 分派到不同路径。
//     pub fn probe(
//         &mut self,
//         mut join_result: Box<HashjoinWorkerResult>,
//         sql_killer: &mut sqlkiller::SQLKiller,
//     ) -> (bool, Box<HashjoinWorkerResult>) {
//         if join_result.chk.is_full() {
//             return (true, join_result);
//         }
//
//         let (mut joined_chk, remain_cap) = match self.base_semi_join.prepare_for_probe(&mut join_result.chk) {
//             Ok(v) => v,
//             Err(err) => {
//                 join_result.err = Some(err);
//                 return (false, join_result);
//             }
//         };
//
//         let has_other_condition = self.base_semi_join.ctx.has_other_condition();
//         let err = if self.base_semi_join.is_left_side_build {
//             if has_other_condition {
//                 self.probe_for_left_side_build_has_other_condition(&mut joined_chk, sql_killer)
//             } else {
//                 self.probe_for_left_side_build_no_other_condition(sql_killer)
//             }
//         } else if has_other_condition {
//             self.probe_for_right_side_build_has_other_condition(&mut join_result.chk, &mut joined_chk, remain_cap, sql_killer)
//         } else {
//             self.probe_for_right_side_build_no_other_condition(&mut join_result.chk, remain_cap, sql_killer)
//         };
//
//         if let Err(err) = err {
//             join_result.err = Some(err);
//             return (false, join_result);
//         }
//         (true, join_result)
//     }
//
// setIsMatchedRows 对应 Go 方法：把 selected 中命中的 probe 行标记到 isMatchedRows。
//     fn set_is_matched_rows(&mut self) {
//         for (i, selected) in self.base_semi_join.selected.iter().enumerate() {
//             if !*selected {
//                 continue;
//             }
//             let probe_row_index = self.base_semi_join.row_index_infos[i].probe_row_index;
//             self.base_semi_join.is_matched_rows[probe_row_index] = true;
//         }
//     }
//
// probeForLeftSideBuildHasOtherCondition 对应带 other condition 的左侧 build 路径。
//     fn probe_for_left_side_build_has_other_condition(
//         &mut self,
//         joined_chk: &mut chunk::Chunk,
//         sql_killer: &mut sqlkiller::SQLKiller,
//     ) -> Result<(), Error> {
//         self.base_semi_join.concatenate_probe_and_build_rows(joined_chk, sql_killer, false)?;
//
//         let meta = self.base_semi_join.ctx.hash_table_meta;
//         if joined_chk.num_rows() > 0 {
//             self.base_semi_join.selected = expression::vectorized_filter(
//                 self.base_semi_join.ctx.sess_ctx.get_expr_ctx().get_eval_ctx(),
//                 self.base_semi_join.ctx.sess_ctx.get_session_vars().enable_vectorized_expression,
//                 self.base_semi_join.ctx.other_condition,
//                 chunk::new_iterator4_chunk(joined_chk),
//                 self.base_semi_join.selected,
//             )?;
//
//             for (index, result) in self.base_semi_join.selected.iter().enumerate() {
//                 if *result {
// Go 通过 unsafe.Pointer(&buildRowStart) 取 row 起始地址；保留裸指针语义，不解引用。
//                     let row_start = self.base_semi_join.row_index_infos[index].build_row_start as *mut std::ffi::c_void;
//                     meta.set_used_flag(row_start);
//                 }
//             }
//         }
//
//         if self.base_semi_join.un_finished_probe_row_idx_queue.is_empty() {
// 避免 Go 中 “Previous chunk is not probed yet” 的状态错误。
//             self.base_semi_join.current_probe_row = self.base_semi_join.chunk_rows;
//         }
//         Ok(())
//     }
//
// probeForLeftSideBuildNoOtherCondition 对应无 other condition 的左侧 build 快路径。
//     fn probe_for_left_side_build_no_other_condition(
//         &mut self,
//         sql_killer: &mut sqlkiller::SQLKiller,
//     ) -> Result<(), Error> {
//         let meta = self.base_semi_join.ctx.hash_table_meta;
//         let tag_helper = self.base_semi_join.ctx.hash_table_context.tag_helper;
//         let mut loop_cnt = 0;
//
//         while self.base_semi_join.current_probe_row < self.base_semi_join.chunk_rows {
//             let row_idx = self.base_semi_join.current_probe_row;
//             if self.base_semi_join.matched_rows_headers[row_idx] != 0 {
//                 let candidate_row = tag_helper.to_unsafe_pointer(self.base_semi_join.matched_rows_headers[row_idx]);
//                 if !meta.is_current_row_used_with_atomic(candidate_row)
//                     && is_key_matched(meta.key_mode, &self.base_semi_join.serialized_keys[row_idx], candidate_row, meta)
//                 {
//                     meta.set_used_flag(candidate_row);
//                 } else {
//                     self.base_semi_join.probe_collision += 1;
//                 }
//                 self.base_semi_join.matched_rows_headers[row_idx] =
//                     get_next_row_address(candidate_row, tag_helper, self.base_semi_join.matched_rows_hash_value[row_idx]);
//             } else {
//                 self.base_semi_join.current_probe_row += 1;
//             }
//
// Go 每 2000 次检查 SQL killer，避免长循环无法响应取消。
//             loop_cnt += 1;
//             if loop_cnt % 2000 == 0 {
//                 check_sql_killer(sql_killer, "killedDuringProbe")?;
//             }
//         }
//
//         check_sql_killer(sql_killer, "killedDuringProbe")
//     }
//
// produceResult 对应右侧 build 带 other condition 时生成一批结果。
//     fn produce_result(
//         &mut self,
//         joined_chk: &mut chunk::Chunk,
//         sql_killer: &mut sqlkiller::SQLKiller,
//     ) -> Result<(), Error> {
//         self.base_semi_join.concatenate_probe_and_build_rows(joined_chk, sql_killer, true)?;
//         if joined_chk.num_rows() > 0 {
//             self.base_semi_join.selected.clear();
//             self.base_semi_join.selected = expression::vectorized_filter(
//                 self.base_semi_join.ctx.sess_ctx.get_expr_ctx().get_eval_ctx(),
//                 self.base_semi_join.ctx.sess_ctx.get_session_vars().enable_vectorized_expression,
//                 self.base_semi_join.ctx.other_condition,
//                 chunk::new_iterator4_chunk(joined_chk),
//                 self.base_semi_join.selected,
//             )?;
//             self.set_is_matched_rows();
//         }
//         Ok(())
//     }
//
// probeForRightSideBuildHasOtherCondition 对应右侧 build 且带 other condition 的路径。
//     fn probe_for_right_side_build_has_other_condition(
//         &mut self,
//         chk: &mut chunk::Chunk,
//         joined_chk: &mut chunk::Chunk,
//         remain_cap: usize,
//         sql_killer: &mut sqlkiller::SQLKiller,
//     ) -> Result<(), Error> {
//         if !self.base_semi_join.un_finished_probe_row_idx_queue.is_empty() {
//             self.produce_result(joined_chk, sql_killer)?;
//             self.base_semi_join.current_probe_row = 0;
//         }
//
//         if self.base_semi_join.un_finished_probe_row_idx_queue.is_empty() {
//             self.base_semi_join.generate_result_chk_for_right_build_with_other_condition(
//                 remain_cap,
//                 chk,
//                 &self.base_semi_join.is_matched_rows,
//                 true,
//             );
//         }
//         Ok(())
//     }
//
// probeForRightSideBuildNoOtherCondition 对应右侧 build 无 other condition 的快路径。
//     fn probe_for_right_side_build_no_other_condition(
//         &mut self,
//         chk: &mut chunk::Chunk,
//         mut remain_cap: usize,
//         sql_killer: &mut sqlkiller::SQLKiller,
//     ) -> Result<(), Error> {
//         let meta = self.base_semi_join.ctx.hash_table_meta;
//         let tag_helper = self.base_semi_join.ctx.hash_table_context.tag_helper;
//
//         if self.base_semi_join.offsets.capacity() == 0 {
//             self.base_semi_join.offsets = Vec::with_capacity(remain_cap);
//         }
//         self.base_semi_join.offsets.clear();
//
//         while remain_cap > 0 && self.base_semi_join.current_probe_row < self.base_semi_join.chunk_rows {
//             let row_idx = self.base_semi_join.current_probe_row;
//             if self.base_semi_join.matched_rows_headers[row_idx] != 0 {
//                 let candidate_row = tag_helper.to_unsafe_pointer(self.base_semi_join.matched_rows_headers[row_idx]);
//                 if is_key_matched(meta.key_mode, &self.base_semi_join.serialized_keys[row_idx], candidate_row, meta) {
//                     self.base_semi_join.matched_rows_headers[row_idx] = 0;
//                     self.base_semi_join.offsets.push(self.base_semi_join.used_rows[row_idx]);
//                     remain_cap -= 1;
//                     self.base_semi_join.current_probe_row += 1;
//                 } else {
//                     self.base_semi_join.probe_collision += 1;
//                     self.base_semi_join.matched_rows_headers[row_idx] =
//                         get_next_row_address(candidate_row, tag_helper, self.base_semi_join.matched_rows_hash_value[row_idx]);
//                 }
//             } else {
//                 self.base_semi_join.current_probe_row += 1;
//             }
//         }
//
//         check_sql_killer(sql_killer, "killedDuringProbe")?;
//         self.base_semi_join.generate_result_chk_for_right_build_no_other_condition(chk);
//         Ok(())
//     }
//
// IsCurrentChunkProbeDone 对应 Go 方法：有未完成队列时不能认为当前 chunk 已处理完。
//     pub fn is_current_chunk_probe_done(&self) -> bool {
//         if self.base_semi_join.ctx.has_other_condition()
//             && !self.base_semi_join.un_finished_probe_row_idx_queue.is_empty()
//         {
//             return false;
//         }
//         self.base_semi_join.base_join_probe.is_current_chunk_probe_done()
//     }
// }
// */
use crate::base_join_probe::{Probe, WorkerResult};
use crate::base_semi_join::BaseSemiJoin;
use crate::joiner::NaajType;
/// Semi Join 探测器：嵌入 `BaseSemiJoin`，实现哈希连接探测接口。
#[derive(Clone)]
pub struct SemiJoinProbe {
    /// 半连接共享状态（匹配标记、未完成队列、build 侧标志等）。
    pub semi: BaseSemiJoin,
}
impl Probe for SemiJoinProbe {
    /// 设置本轮 probe chunk，并重置半连接探测状态。
    fn set_chunk_for_probe(&mut self, chunk: Vec<crate::joiner::Row>) -> Result<(), String> {
        self.semi.base.set_chunk_for_probe(chunk)?;
        self.semi.reset_probe_state();
        Ok(())
    }
    /// Spill 恢复后的 probe chunk 设置；语义与 `set_chunk_for_probe` 相同。
    fn set_restored_chunk_for_probe(
        &mut self,
        chunk: Vec<crate::joiner::Row>,
    ) -> Result<(), String> {
        self.set_chunk_for_probe(chunk)
    }
    /// 把尚未探测完的 probe chunk 溢写出去，供后续恢复。
    fn spill_remaining_probe_chunks(&mut self) -> Vec<Vec<crate::joiner::Row>> {
        self.semi.base.spill_remaining_probe_chunks()
    }
    /// 主探测入口：在 chunk 容量内逐行匹配，左侧 build 标记 used，右侧 build 产出命中行。
    fn probe(&mut self) -> WorkerResult {
        let mut rows = Vec::new();
        // 在 max_chunk_size 限制下推进 current_probe_row。
        while self.semi.base.current_probe_row < self.semi.base.probe_chunk.len()
            && rows.len() < self.semi.base.context.max_chunk_size
        {
            let index = self.semi.base.current_probe_row;
            if self.semi.is_left_side_build {
                // 左侧为 build：只标记命中的 build 行，本阶段不直接输出。
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
                            if *status == crate::joiner::OuterRowStatus::Matched {
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
            } else if let Err(error) =
                // 右侧为 build：命中则把 probe 行写入结果。
                self.semi
                        .match_probe_row(index, &mut rows, NaajType::Unknown)
            {
                return WorkerResult {
                    rows,
                    error: Some(error),
                };
            }
            self.semi.base.finish_current_lookup_loop();
        }
        WorkerResult { rows, error: None }
    }
    /// 仅左侧 build 的半连接需要在探测后再扫描 row table。
    fn need_scan_row_table(&self) -> bool {
        self.semi.is_left_side_build
    }
    /// 初始化 row table 扫描游标。
    fn init_for_scan_row_table(&mut self) {
        if !self.semi.is_left_side_build {
            panic!("should not reach here");
        }
        self.semi.base.scan_row_index = 0;
    }
    /// 扫描已被标记使用的 build 行并输出（半连接结果）。
    fn scan_row_table(&mut self) -> WorkerResult {
        if !self.semi.is_left_side_build {
            panic!("should not reach here");
        }
        self.semi.scan_build_rows(true)
    }
    /// row table 扫描是否已到末尾。
    fn is_scan_row_table_done(&self) -> bool {
        if !self.semi.is_left_side_build {
            panic!("should not reach here");
        }
        self.semi.base.scan_row_index >= self.semi.base.context.build_rows.len()
    }
    /// 当前 probe chunk 是否处理完毕（含 other condition 未完成队列为空）。
    fn is_current_chunk_probe_done(&self) -> bool {
        self.semi.unfinished_probe_rows.is_empty() && self.semi.base.is_current_chunk_probe_done()
    }
    /// 重置探测状态，准备下一轮。
    fn reset_probe(&mut self) {
        self.semi.base.reset_probe();
        self.semi.reset_probe_state();
    }
}
